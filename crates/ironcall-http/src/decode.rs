//! Streaming `Content-Encoding` decoding (gzip, deflate, br, zstd).

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_compression::tokio::bufread::{
    BrotliDecoder, DeflateDecoder, GzipDecoder, ZlibDecoder, ZstdDecoder,
};
use futures_util::TryStreamExt;
use http::HeaderMap;
use http::header::CONTENT_ENCODING;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, BufReader};
use tokio_util::io::StreamReader;

/// The `Accept-Encoding` value the engine sends by default.
pub(crate) const ACCEPT_ENCODING: &str = "gzip, deflate, br, zstd";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Coding {
    Gzip,
    Deflate,
    Brotli,
    Zstd,
}

impl Coding {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Coding::Gzip => "gzip",
            Coding::Deflate => "deflate",
            Coding::Brotli => "br",
            Coding::Zstd => "zstd",
        }
    }
}

/// How the body will be decoded.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Plan {
    /// Codings in the order they must be undone (last applied first).
    Decode(Vec<Coding>),
    /// An unknown coding is present, so the body is passed through untouched.
    Passthrough(String),
}

/// Reads `Content-Encoding` (possibly several headers / comma-separated values, in the
/// order the server applied them) into a decoding plan.
pub(crate) fn plan(headers: &HeaderMap) -> Plan {
    let mut applied = Vec::new();
    for value in headers.get_all(CONTENT_ENCODING) {
        let Ok(value) = value.to_str() else {
            return Plan::Passthrough(String::from_utf8_lossy(value.as_bytes()).into_owned());
        };
        for token in value.split(',').map(|t| t.trim().to_ascii_lowercase()) {
            match token.as_str() {
                "" | "identity" => {}
                "gzip" | "x-gzip" => applied.push(Coding::Gzip),
                "deflate" => applied.push(Coding::Deflate),
                "br" => applied.push(Coding::Brotli),
                "zstd" => applied.push(Coding::Zstd),
                _ => return Plan::Passthrough(token),
            }
        }
    }
    applied.reverse();
    Plan::Decode(applied)
}

type BoxBufRead = Pin<Box<dyn AsyncBufRead + Send>>;
pub(crate) type BoxRead = Pin<Box<dyn AsyncRead + Send>>;

/// Turns a hyper body into a buffered reader, counting wire bytes into `wire`.
/// Transport errors are wrapped in `io::Error` with the `hyper::Error` inside.
pub(crate) fn wire_reader(body: Incoming, wire: Arc<AtomicU64>) -> BoxBufRead {
    let stream = body
        .into_data_stream()
        .inspect_ok(move |chunk| {
            wire.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        })
        .map_err(std::io::Error::other);
    Box::pin(StreamReader::new(stream))
}

/// Wraps `reader` in decoders that undo `codings` in order.
pub(crate) async fn decoder(mut reader: BoxBufRead, codings: &[Coding]) -> std::io::Result<BoxRead> {
    let Some((&last, rest)) = codings.split_last() else {
        return Ok(Box::pin(reader));
    };
    for &coding in rest {
        let decoded = decode_one(reader, coding).await?;
        reader = Box::pin(BufReader::new(decoded));
    }
    decode_one(reader, last).await
}

async fn decode_one(mut reader: BoxBufRead, coding: Coding) -> std::io::Result<BoxRead> {
    Ok(match coding {
        Coding::Gzip => {
            let mut d = GzipDecoder::new(reader);
            d.multiple_members(true);
            Box::pin(d)
        }
        Coding::Brotli => Box::pin(BrotliDecoder::new(reader)),
        Coding::Zstd => Box::pin(ZstdDecoder::new(reader)),
        Coding::Deflate => {
            // "deflate" should be zlib-wrapped (RFC 9110), but some servers send raw
            // deflate. Peek at the header to tell them apart.
            let zlib = looks_like_zlib(reader.fill_buf().await?);
            if zlib {
                Box::pin(ZlibDecoder::new(reader))
            } else {
                Box::pin(DeflateDecoder::new(reader))
            }
        }
    })
}

/// A zlib stream starts with CMF (method 8 = deflate, window ≤ 32K) and FLG such that
/// `CMF * 256 + FLG` is a multiple of 31 (RFC 1950 §2.2).
fn looks_like_zlib(head: &[u8]) -> bool {
    match head {
        [cmf, flg, ..] => {
            cmf & 0x0f == 8 && cmf >> 4 <= 7 && (u16::from(*cmf) << 8 | u16::from(*flg)) % 31 == 0
        }
        [cmf] => cmf & 0x0f == 8 && cmf >> 4 <= 7,
        [] => true,
    }
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;

    use super::*;

    fn headers(values: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in values {
            h.append(CONTENT_ENCODING, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn plan_undoes_codings_in_reverse_order() {
        assert_eq!(plan(&headers(&[])), Plan::Decode(vec![]));
        assert_eq!(plan(&headers(&["identity"])), Plan::Decode(vec![]));
        assert_eq!(
            plan(&headers(&["gzip, br"])),
            Plan::Decode(vec![Coding::Brotli, Coding::Gzip])
        );
        assert_eq!(
            plan(&headers(&["deflate", "ZSTD"])),
            Plan::Decode(vec![Coding::Zstd, Coding::Deflate])
        );
    }

    #[test]
    fn unknown_coding_passes_through() {
        assert_eq!(
            plan(&headers(&["gzip, compress"])),
            Plan::Passthrough("compress".into())
        );
    }

    #[test]
    fn zlib_header_detection() {
        assert!(looks_like_zlib(&[0x78, 0x9c]));
        assert!(looks_like_zlib(&[0x78, 0x01]));
        assert!(!looks_like_zlib(&[0xcb, 0x48])); // raw deflate of "hello"
    }
}
