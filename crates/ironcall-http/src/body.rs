//! Request bodies: in-memory bytes, a file streamed from disk, or `multipart/form-data`
//! with text and file parts (PLAN.md §3.4, Phase 4).
//!
//! Files are never read into memory. Sizes are taken when the request is built so the
//! exact `Content-Length` is sent (no chunked encoding), and each send re-opens the
//! files, so a 307/308 redirect can replay the body.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt, stream};
use http::HeaderValue;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::Frame;
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;

use crate::error::Error;

/// The body type handed to hyper.
pub(crate) type ReqBody = UnsyncBoxBody<Bytes, std::io::Error>;

const READ_CHUNK: usize = 64 * 1024;

/// A request body.
#[derive(Clone, Debug)]
pub enum Body {
    Bytes(Bytes),
    /// A file's bytes, streamed from disk.
    File {
        path: PathBuf,
        /// `Content-Type` to send if the request doesn't set one (default
        /// `application/octet-stream`).
        content_type: Option<String>,
    },
    /// `multipart/form-data` (RFC 7578).
    Multipart(Vec<Part>),
}

impl Default for Body {
    fn default() -> Self {
        Body::Bytes(Bytes::new())
    }
}

impl From<Bytes> for Body {
    fn from(b: Bytes) -> Self {
        Body::Bytes(b)
    }
}

impl From<Vec<u8>> for Body {
    fn from(b: Vec<u8>) -> Self {
        Body::Bytes(Bytes::from(b))
    }
}

impl From<&'static str> for Body {
    fn from(s: &'static str) -> Self {
        Body::Bytes(Bytes::from_static(s.as_bytes()))
    }
}

impl From<String> for Body {
    fn from(s: String) -> Self {
        Body::Bytes(Bytes::from(s))
    }
}

/// One `multipart/form-data` part.
#[derive(Clone, Debug)]
pub enum Part {
    Text {
        name: String,
        value: String,
        content_type: Option<String>,
    },
    File {
        name: String,
        path: PathBuf,
        /// `filename` in `Content-Disposition`; defaults to the path's file name.
        file_name: Option<String>,
        /// Defaults to `application/octet-stream`.
        content_type: Option<String>,
    },
}

impl Body {
    /// True for an empty in-memory body.
    pub fn is_empty(&self) -> bool {
        matches!(self, Body::Bytes(b) if b.is_empty())
    }

    /// True for bodies streamed from disk, which report upload progress.
    pub fn is_streamed(&self) -> bool {
        !matches!(self, Body::Bytes(_))
    }

    /// Exact size in bytes. Fails if a file is missing or unreadable.
    pub fn len(&self) -> Result<u64, Error> {
        Ok(self.segments()?.iter().map(Segment::len).sum())
    }

    /// The `Content-Type` this body needs when the request doesn't set one.
    pub fn default_content_type(&self) -> Option<HeaderValue> {
        match self {
            Body::Bytes(_) => None,
            Body::File { content_type, .. } => Some(
                content_type
                    .as_deref()
                    .and_then(|c| HeaderValue::from_str(c).ok())
                    .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream")),
            ),
            Body::Multipart(parts) => {
                HeaderValue::from_str(&format!("multipart/form-data; boundary={}", boundary(parts))).ok()
            }
        }
    }

    /// Splits the body into in-memory and file segments, checking every file.
    fn segments(&self) -> Result<Vec<Segment>, Error> {
        match self {
            Body::Bytes(b) => Ok(vec![Segment::Bytes(b.clone())]),
            Body::File { path, .. } => Ok(vec![file_segment(path)?]),
            Body::Multipart(parts) => {
                let boundary = boundary(parts);
                let mut out = Vec::with_capacity(parts.len() * 3 + 1);
                for part in parts {
                    let mut head = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"");
                    match part {
                        Part::Text {
                            name,
                            value,
                            content_type,
                        } => {
                            head.push_str(&escape(name));
                            head.push_str("\"\r\n");
                            if let Some(ct) = content_type {
                                head.push_str(&format!("Content-Type: {}\r\n", strip_crlf(ct)));
                            }
                            head.push_str("\r\n");
                            head.push_str(value);
                            head.push_str("\r\n");
                            out.push(Segment::Bytes(Bytes::from(head)));
                        }
                        Part::File {
                            name,
                            path,
                            file_name,
                            content_type,
                        } => {
                            let file_name = file_name
                                .clone()
                                .or_else(|| path.file_name().map(|f| f.to_string_lossy().into_owned()))
                                .unwrap_or_else(|| "file".to_string());
                            head.push_str(&format!(
                                "{}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
                                escape(name),
                                escape(&file_name),
                                strip_crlf(content_type.as_deref().unwrap_or("application/octet-stream"))
                            ));
                            out.push(Segment::Bytes(Bytes::from(head)));
                            out.push(file_segment(path)?);
                            out.push(Segment::Bytes(Bytes::from_static(b"\r\n")));
                        }
                    }
                }
                out.push(Segment::Bytes(Bytes::from(format!("--{boundary}--\r\n"))));
                Ok(out)
            }
        }
    }

    /// Builds the body for one send. `sent` counts bytes handed to the connection.
    pub(crate) fn to_hyper(&self, sent: Arc<AtomicU64>) -> Result<ReqBody, Error> {
        if let Body::Bytes(b) = self {
            sent.fetch_add(b.len() as u64, Ordering::Relaxed);
            return Ok(Full::new(b.clone())
                .map_err(|never| match never {})
                .boxed_unsync());
        }
        let segments = self.segments()?;
        let chunks = stream::iter(segments).flat_map(|segment| match segment {
            Segment::Bytes(b) => stream::once(async move { Ok(b) }).boxed(),
            Segment::File { path, len } => file_stream(path, len).boxed(),
        });
        let counted = chunks.inspect_ok(move |chunk| {
            sent.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        });
        Ok(StreamBody::new(counted.map_ok(Frame::data)).boxed_unsync())
    }
}

enum Segment {
    Bytes(Bytes),
    File { path: PathBuf, len: u64 },
}

impl Segment {
    fn len(&self) -> u64 {
        match self {
            Segment::Bytes(b) => b.len() as u64,
            Segment::File { len, .. } => *len,
        }
    }
}

fn file_segment(path: &Path) -> Result<Segment, Error> {
    let body_file = |source| Error::BodyFile {
        path: path.to_path_buf(),
        source,
    };
    let meta = std::fs::metadata(path).map_err(body_file)?;
    if !meta.is_file() {
        return Err(body_file(std::io::Error::other("not a regular file")));
    }
    // Opening now catches permission problems before connecting.
    std::fs::File::open(path).map_err(body_file)?;
    Ok(Segment::File {
        path: path.to_path_buf(),
        len: meta.len(),
    })
}

/// Streams exactly `len` bytes of a file. A file that shrank since its size was taken
/// is an error, because the declared `Content-Length` can no longer be met.
fn file_stream(path: PathBuf, len: u64) -> impl futures_util::Stream<Item = std::io::Result<Bytes>> + Send {
    stream::once(async move {
        let file = tokio::fs::File::open(&path).await?;
        Ok::<_, std::io::Error>(ReaderStream::with_capacity(file.take(len), READ_CHUNK))
    })
    .try_flatten()
    .chain(stream::once(async { Ok(Bytes::new()) }))
    .scan(0u64, move |seen, chunk| {
        let item = match chunk {
            Ok(b) if b.is_empty() && *seen < len => Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "a file got shorter while it was being uploaded",
            )),
            Ok(b) => {
                *seen += b.len() as u64;
                Ok(b)
            }
            Err(e) => Err(e),
        };
        std::future::ready(Some(item))
    })
    .try_filter(|b| std::future::ready(!b.is_empty()))
}

/// A boundary derived from the parts, so the preview shows the same `Content-Type`
/// that is sent. It can't collide with the part headers, and a file containing it by
/// chance is vanishingly unlikely (128 bits).
fn boundary(parts: &[Part]) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    for part in parts {
        match part {
            Part::Text { name, value, .. } => {
                ctx.update(b"t\0");
                ctx.update(name.as_bytes());
                ctx.update(b"\0");
                ctx.update(value.as_bytes());
            }
            Part::File { name, path, .. } => {
                ctx.update(b"f\0");
                ctx.update(name.as_bytes());
                ctx.update(b"\0");
                ctx.update(path.to_string_lossy().as_bytes());
            }
        }
        ctx.update(b"\x1e");
    }
    let digest = ctx.finish();
    let hex: String = digest.as_ref()[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("----IronCallFormBoundary{hex}")
}

/// Escapes a `Content-Disposition` parameter value as browsers do (WHATWG HTML).
fn escape(value: &str) -> String {
    value
        .replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn strip_crlf(value: &str) -> String {
    value.replace(['\r', '\n'], "")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn collect(body: &Body) -> Vec<u8> {
        let counter = Arc::new(AtomicU64::new(0));
        let bytes = body
            .to_hyper(counter.clone())
            .unwrap()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(counter.load(Ordering::Relaxed), bytes.len() as u64);
        bytes.to_vec()
    }

    #[tokio::test]
    async fn multipart_encodes_parts_and_length_matches() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"file content").unwrap();
        let body = Body::Multipart(vec![
            Part::Text {
                name: "title".into(),
                value: "hi".into(),
                content_type: None,
            },
            Part::File {
                name: "up\"load".into(),
                path: file.clone(),
                file_name: None,
                content_type: Some("text/plain".into()),
            },
        ]);
        let bytes = collect(&body).await;
        assert_eq!(body.len().unwrap(), bytes.len() as u64);
        let text = String::from_utf8(bytes).unwrap();
        let ct = body.default_content_type().unwrap();
        let boundary = ct.to_str().unwrap().split("boundary=").nth(1).unwrap();
        assert!(text.starts_with(&format!("--{boundary}\r\n")));
        assert!(text.contains("Content-Disposition: form-data; name=\"title\"\r\n\r\nhi\r\n"));
        assert!(text.contains(
            "name=\"up%22load\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nfile content\r\n"
        ));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
    }

    #[test]
    fn boundary_is_stable_for_the_same_parts() {
        let parts = vec![Part::Text {
            name: "a".into(),
            value: "b".into(),
            content_type: None,
        }];
        assert_eq!(boundary(&parts), boundary(&parts.clone()));
    }

    #[test]
    fn missing_file_is_a_body_file_error() {
        let body = Body::File {
            path: "/definitely/not/here.bin".into(),
            content_type: None,
        };
        let err = body.len().unwrap_err();
        assert!(matches!(err, Error::BodyFile { .. }), "{err:?}");
        assert_eq!(err.kind(), crate::ErrorKind::InvalidRequest);
    }

    #[tokio::test]
    async fn file_body_streams_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.bin");
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&file, &data).unwrap();
        let body = Body::File {
            path: file,
            content_type: None,
        };
        assert_eq!(body.len().unwrap(), data.len() as u64);
        assert_eq!(collect(&body).await, data);
    }

    #[tokio::test]
    async fn file_that_shrinks_mid_upload_errors() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.bin");
        std::fs::write(&file, vec![1u8; 1000]).unwrap();
        let body = Body::File {
            path: file.clone(),
            content_type: None,
        };
        let hyper_body = body.to_hyper(Arc::default()).unwrap();
        std::fs::write(&file, vec![1u8; 10]).unwrap();
        assert!(hyper_body.collect().await.is_err());
    }
}
