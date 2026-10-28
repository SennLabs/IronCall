//! Response body handling (PLAN.md Phase 1): deciding how to present a body, decoding
//! it to UTF-8 and pretty-printing JSON and XML.

use std::io::{self, Read, Write};

use encoding_rs::{CoderResult, Encoding, UTF_8};

mod stream;

pub use stream::{MAX_DEPTH, PrettyError, PrettyStats, pretty_json_stream, pretty_stream, pretty_xml_stream};

/// How a response body should be presented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    /// No body (e.g. 204, 304, HEAD, or zero bytes).
    Empty,
    Json,
    Xml,
    Html,
    Text,
    Binary,
}

impl BodyKind {
    /// Whether [`pretty`] can reformat bodies of this kind.
    pub fn can_pretty_print(self) -> bool {
        matches!(self, Self::Json | Self::Xml)
    }
}

/// How many leading bytes [`classify`] and [`detect_charset`] look at.
pub const SNIFF_LEN: usize = 1024;

/// Decides how to present a body from its `Content-Type`, its first bytes (up to
/// [`SNIFF_LEN`] is enough), the status code and the request method.
///
/// Declared types win where they are specific (`application/json`, `*+json`,
/// `image/*`, …). Missing, generic or `text/plain` types are sniffed: a body starting
/// like a JSON object or array is JSON (a prefix check; callers holding the whole body
/// can confirm with [`pretty_json`]), `<?xml` is XML, `<!doctype html`/`<html` is HTML,
/// NUL bytes mean binary.
pub fn classify(content_type: Option<&str>, first_bytes: &[u8], status: u16, method: &str) -> BodyKind {
    if method.eq_ignore_ascii_case("HEAD")
        || matches!(status, 100..=199 | 204 | 304)
        || first_bytes.is_empty()
    {
        return BodyKind::Empty;
    }
    let sample = &first_bytes[..first_bytes.len().min(SNIFF_LEN)];
    let essence = content_type.map(mime_essence).unwrap_or_default();

    let declared = match essence.as_str() {
        "application/json" | "text/json" | "application/x-ndjson" => Some(BodyKind::Json),
        e if e.ends_with("+json") => Some(BodyKind::Json),
        "application/xml" | "text/xml" => Some(BodyKind::Xml),
        e if e.ends_with("+xml") && e != "application/xhtml+xml" && e != "image/svg+xml" => {
            Some(BodyKind::Xml)
        }
        "image/svg+xml" => Some(BodyKind::Xml),
        "text/html" | "application/xhtml+xml" => Some(BodyKind::Html),
        "application/octet-stream" | "application/pdf" | "application/zip" | "application/gzip" => {
            Some(BodyKind::Binary)
        }
        e if e.starts_with("image/")
            || e.starts_with("audio/")
            || e.starts_with("video/")
            || e.starts_with("font/") =>
        {
            Some(BodyKind::Binary)
        }
        "application/javascript"
        | "text/javascript"
        | "application/x-www-form-urlencoded"
        | "application/graphql"
        | "text/csv"
        | "text/css"
        | "text/markdown" => Some(BodyKind::Text),
        _ => None,
    };
    if let Some(kind) = declared {
        return kind;
    }

    let has_utf16_bom = sample.starts_with(&[0xFF, 0xFE]) || sample.starts_with(&[0xFE, 0xFF]);
    if sample.contains(&0) && !has_utf16_bom {
        return BodyKind::Binary;
    }
    let text = strip_utf8_bom(sample);
    let trimmed = trim_ascii_start(text);
    if looks_like_json(trimmed) {
        return BodyKind::Json;
    }
    let lower: Vec<u8> = trimmed.iter().take(32).map(u8::to_ascii_lowercase).collect();
    if lower.starts_with(b"<?xml") {
        // XHTML documents start with an XML declaration too.
        if contains(&sample.to_ascii_lowercase(), b"<html") {
            return BodyKind::Html;
        }
        return BodyKind::Xml;
    }
    if lower.starts_with(b"<!doctype html") || lower.starts_with(b"<html") {
        return BodyKind::Html;
    }
    BodyKind::Text
}

/// Where a body's charset was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharsetSource {
    /// A byte order mark. It wins over the header, as in browsers (WHATWG Encoding).
    Bom,
    Header,
    /// `<meta charset>` in the first bytes of an HTML body.
    HtmlMeta,
    /// Nothing declared: UTF-8 (also the only valid encoding for JSON, RFC 8259).
    Default,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Charset {
    pub encoding: &'static Encoding,
    pub source: CharsetSource,
}

impl Charset {
    /// Lowercase WHATWG name, e.g. `utf-8`, `windows-1252`, `shift_jis`.
    pub fn name(&self) -> String {
        self.encoding.name().to_ascii_lowercase()
    }
}

/// Finds the charset of a text body: BOM, then the `charset` parameter of
/// `Content-Type`, then `<meta charset>` for HTML, then UTF-8.
///
/// Unknown labels are ignored. Note that WHATWG maps `iso-8859-1` and `us-ascii` to
/// `windows-1252`, which is a superset.
pub fn detect_charset(content_type: Option<&str>, first_bytes: &[u8], kind: BodyKind) -> Charset {
    if let Some((encoding, _)) = Encoding::for_bom(first_bytes) {
        return Charset {
            encoding,
            source: CharsetSource::Bom,
        };
    }
    if let Some(encoding) = content_type
        .and_then(charset_param)
        .and_then(|l| Encoding::for_label(l.as_bytes()))
    {
        return Charset {
            encoding,
            source: CharsetSource::Header,
        };
    }
    if kind == BodyKind::Html
        && let Some(encoding) = html_meta_charset(&first_bytes[..first_bytes.len().min(SNIFF_LEN)])
    {
        return Charset {
            encoding,
            source: CharsetSource::HtmlMeta,
        };
    }
    Charset {
        encoding: UTF_8,
        source: CharsetSource::Default,
    }
}

/// Result of [`transcode_to_utf8`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TranscodeStats {
    pub bytes_in: u64,
    pub bytes_out: u64,
    /// True if invalid input was replaced with U+FFFD.
    pub lossy: bool,
}

/// Decodes `reader` from `encoding` and writes UTF-8 to `writer`, streaming in chunks.
/// A leading BOM for `encoding` is removed. Invalid sequences become U+FFFD.
pub fn transcode_to_utf8(
    mut reader: impl Read,
    mut writer: impl Write,
    encoding: &'static Encoding,
) -> io::Result<TranscodeStats> {
    const CHUNK: usize = 64 * 1024;
    let mut decoder = encoding.new_decoder_with_bom_removal();
    let mut input = vec![0u8; CHUNK];
    let mut output = vec![0u8; CHUNK * 3 + 16];
    let mut stats = TranscodeStats::default();

    loop {
        let n = match reader.read(&mut input) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let last = n == 0;
        stats.bytes_in += n as u64;
        let mut pending = &input[..n];
        loop {
            let (result, read, written, replaced) = decoder.decode_to_utf8(pending, &mut output, last);
            stats.lossy |= replaced;
            writer.write_all(&output[..written])?;
            stats.bytes_out += written as u64;
            pending = &pending[read..];
            match result {
                CoderResult::InputEmpty => break,
                CoderResult::OutputFull => continue,
            }
        }
        if last {
            break;
        }
    }
    writer.flush()?;
    Ok(stats)
}

/// Pretty-prints `body` if its kind supports it.
pub fn pretty(kind: BodyKind, body: &str) -> Option<String> {
    match kind {
        BodyKind::Json => pretty_json(body),
        BodyKind::Xml => pretty_xml(body),
        _ => None,
    }
}

/// Re-indents JSON with 2 spaces without re-encoding anything: numbers (`1.0e10`,
/// 30-digit integers) and strings (escapes included) are copied byte for byte.
/// Returns `None` if `json` isn't valid JSON (or nests deeper than serde_json's
/// recursion limit of 128).
pub fn pretty_json(json: &str) -> Option<String> {
    serde_json::from_str::<serde::de::IgnoredAny>(json).ok()?;

    let bytes = json.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() + bytes.len() / 2);
    let mut depth = 0usize;
    let mut i = 0;
    let newline = |out: &mut Vec<u8>, depth: usize| {
        out.push(b'\n');
        out.extend(std::iter::repeat_n(b' ', depth * 2));
    };

    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b' ' | b'\t' | b'\n' | b'\r' => {}
            b'"' => {
                let start = i;
                i += 1;
                while bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                out.extend_from_slice(&bytes[start..=i]);
            }
            b'{' | b'[' => {
                let close = if b == b'{' { b'}' } else { b']' };
                let mut j = i + 1;
                while matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
                    j += 1;
                }
                if bytes[j] == close {
                    out.push(b);
                    out.push(close);
                    i = j;
                } else {
                    out.push(b);
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            b'}' | b']' => {
                depth -= 1;
                newline(&mut out, depth);
                out.push(b);
            }
            b',' => {
                out.push(b',');
                newline(&mut out, depth);
            }
            b':' => out.extend_from_slice(b": "),
            _ => out.push(b),
        }
        i += 1;
    }
    // Input was valid UTF-8 and only ASCII bytes were added or removed.
    String::from_utf8(out).ok()
}

/// Re-indents XML with 2 spaces. Text nodes are trimmed. Returns `None` if `xml` isn't
/// well-formed or contains no elements.
pub fn pretty_xml(xml: &str) -> Option<String> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut writer = quick_xml::Writer::new_with_indent(Vec::new(), b' ', 2);
    let mut depth = 0i64;
    let mut saw_element = false;
    loop {
        let event = reader.read_event().ok()?;
        match &event {
            Event::Eof => break,
            Event::Start(_) => {
                depth += 1;
                saw_element = true;
            }
            Event::End(_) => depth -= 1,
            Event::Empty(_) => saw_element = true,
            _ => {}
        }
        writer.write_event(event).ok()?;
    }
    if depth != 0 || !saw_element {
        return None;
    }
    String::from_utf8(writer.into_inner()).ok()
}

/// Case-folds `text` for case-insensitive search (simple, one-to-one-ish folding).
///
/// Uses per-character lowercasing (not the context-sensitive `str::to_lowercase`), plus
/// final sigma `ς` → `σ` so all Greek sigmas match. Known limits, by design:
/// - no full folding: `ß` doesn't match `ss` (but `ẞ` matches `ß`);
/// - not locale-aware: Turkish dotless `ı` and dotted `İ` aren't special-cased (`I`
///   folds to `i`; `İ` folds to `i` plus a combining dot);
/// - no normalisation: precomposed `é` doesn't match `e` + combining accent.
pub fn fold_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        push_folded(&mut out, c);
    }
    out
}

fn push_folded(out: &mut String, c: char) {
    if c == 'ς' {
        out.push('σ');
    } else if c.is_ascii() {
        out.push(c.to_ascii_lowercase());
    } else {
        out.extend(c.to_lowercase());
    }
}

/// Finds `needle_folded` (already passed through [`fold_case`]) in `haystack`,
/// case-insensitively. Returns the byte range of the match **in `haystack`**, or
/// `None` if there's no match or the needle is empty.
pub fn find_folded(haystack: &str, needle_folded: &str) -> Option<std::ops::Range<usize>> {
    if needle_folded.is_empty() {
        return None;
    }
    // Fold the haystack, remembering where each folded byte came from.
    let mut folded = String::with_capacity(haystack.len());
    let mut origin: Vec<usize> = Vec::with_capacity(haystack.len() + 1);
    for (i, c) in haystack.char_indices() {
        let before = folded.len();
        push_folded(&mut folded, c);
        origin.extend(std::iter::repeat_n(i, folded.len() - before));
    }
    origin.push(haystack.len());
    let start = folded.find(needle_folded)?;
    let end = start + needle_folded.len();
    // The match may end inside the folding of one character (e.g. `İ` → `i̇`); extend
    // to the end of that original character.
    let orig_end = if end < origin.len() - 1 && origin[end] == origin[end - 1] {
        let c_start = origin[end - 1];
        c_start + haystack[c_start..].chars().next().map_or(0, char::len_utf8)
    } else {
        origin[end]
    };
    Some(origin[start]..orig_end)
}

fn mime_essence(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn charset_param(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|param| {
        let (name, value) = param.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"').trim().to_string())
    })
}

fn html_meta_charset(sample: &[u8]) -> Option<&'static Encoding> {
    let lower = sample.to_ascii_lowercase();
    let mut from = 0;
    while let Some(pos) = find(&lower[from..], b"<meta") {
        let tag_start = from + pos;
        let tag_end = find(&lower[tag_start..], b">").map_or(lower.len(), |e| tag_start + e);
        let tag = &lower[tag_start..tag_end];
        if let Some(cs) = find(tag, b"charset=") {
            let value = &tag[cs + b"charset=".len()..];
            let value: Vec<u8> = value
                .iter()
                .copied()
                .skip_while(|b| *b == b'"' || *b == b'\'')
                .take_while(|b| !matches!(b, b'"' | b'\'' | b' ' | b';' | b'/' | b'>'))
                .collect();
            if let Some(encoding) = Encoding::for_label(&value) {
                return Some(encoding);
            }
        }
        from = tag_end;
    }
    None
}

fn looks_like_json(trimmed: &[u8]) -> bool {
    let Some((&first, rest)) = trimmed.split_first() else {
        return false;
    };
    if first != b'{' && first != b'[' {
        return false;
    }
    match trim_ascii_start(rest).first() {
        None => true,
        Some(b) if first == b'{' => matches!(b, b'"' | b'}'),
        Some(b) => matches!(
            b,
            b'"' | b'{' | b'[' | b']' | b'-' | b'0'..=b'9' | b't' | b'f' | b'n'
        ),
    }
}

fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

fn trim_ascii_start(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    &bytes[start..]
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

#[cfg(test)]
mod tests {
    use encoding_rs::{SHIFT_JIS, UTF_16LE, WINDOWS_1252};

    use super::*;

    fn kind(ct: Option<&str>, body: &[u8]) -> BodyKind {
        classify(ct, body, 200, "GET")
    }

    #[test]
    fn empty_responses() {
        assert_eq!(
            classify(Some("application/json"), b"{}", 204, "GET"),
            BodyKind::Empty
        );
        assert_eq!(
            classify(Some("text/html"), b"<html>", 304, "GET"),
            BodyKind::Empty
        );
        assert_eq!(
            classify(Some("text/html"), b"<html>", 200, "head"),
            BodyKind::Empty
        );
        assert_eq!(kind(Some("application/json"), b""), BodyKind::Empty);
    }

    #[test]
    fn declared_types() {
        assert_eq!(
            kind(Some("application/json; charset=utf-8"), b"x"),
            BodyKind::Json
        );
        assert_eq!(kind(Some("application/problem+json"), b"{}"), BodyKind::Json);
        assert_eq!(kind(Some("Application/Atom+XML"), b"<feed/>"), BodyKind::Xml);
        assert_eq!(kind(Some("text/xml"), b"<a/>"), BodyKind::Xml);
        assert_eq!(kind(Some("image/svg+xml"), b"<svg/>"), BodyKind::Xml);
        assert_eq!(kind(Some("text/html"), b"hello"), BodyKind::Html);
        assert_eq!(kind(Some("application/xhtml+xml"), b"<html/>"), BodyKind::Html);
        assert_eq!(kind(Some("image/png"), b"\x89PNG"), BodyKind::Binary);
        assert_eq!(kind(Some("application/pdf"), b"%PDF"), BodyKind::Binary);
        assert_eq!(kind(Some("text/css"), b"{a:b}"), BodyKind::Text);
    }

    #[test]
    fn sniffs_mislabelled_or_missing_types() {
        assert_eq!(kind(Some("text/plain"), b"  {\"a\": 1}"), BodyKind::Json);
        assert_eq!(kind(None, b"[1, 2]"), BodyKind::Json);
        assert_eq!(kind(None, b"\xEF\xBB\xBF{\"a\":1}"), BodyKind::Json);
        assert_eq!(kind(None, b"<?xml version=\"1.0\"?><a/>"), BodyKind::Xml);
        assert_eq!(kind(None, b"<!DOCTYPE html><html>"), BodyKind::Html);
        assert_eq!(kind(Some("text/plain"), b"hello"), BodyKind::Text);
        assert_eq!(kind(None, b"GIF89a\x00\x01"), BodyKind::Binary);
        assert_eq!(kind(Some("application/unknown"), b"{oops"), BodyKind::Text);
        assert_eq!(kind(Some("text/plain"), b"[INFO] started"), BodyKind::Text);
    }

    #[test]
    fn utf16_with_bom_is_text_not_binary() {
        assert_eq!(kind(None, b"\xFF\xFEh\x00i\x00"), BodyKind::Text);
    }

    #[test]
    fn charset_detection_order() {
        let c = detect_charset(
            Some("text/plain; charset=\"ISO-8859-1\""),
            b"caf\xe9",
            BodyKind::Text,
        );
        assert_eq!((c.encoding, c.source), (WINDOWS_1252, CharsetSource::Header));
        assert_eq!(c.name(), "windows-1252");

        let c = detect_charset(
            Some("text/plain; charset=iso-8859-1"),
            b"\xEF\xBB\xBFhi",
            BodyKind::Text,
        );
        assert_eq!((c.encoding, c.source), (UTF_8, CharsetSource::Bom));

        let html = b"<html><head><meta charset=\"shift_jis\"></head>";
        let c = detect_charset(Some("text/html"), html, BodyKind::Html);
        assert_eq!((c.encoding, c.source), (SHIFT_JIS, CharsetSource::HtmlMeta));

        let html = b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1252\">";
        assert_eq!(detect_charset(None, html, BodyKind::Html).encoding, WINDOWS_1252);

        let c = detect_charset(Some("application/json"), b"{}", BodyKind::Json);
        assert_eq!((c.encoding, c.source), (UTF_8, CharsetSource::Default));

        let c = detect_charset(Some("text/plain; charset=bogus"), b"x", BodyKind::Text);
        assert_eq!(c.source, CharsetSource::Default);
    }

    #[test]
    fn meta_charset_ignored_for_non_html() {
        let c = detect_charset(None, b"<meta charset=\"shift_jis\">", BodyKind::Text);
        assert_eq!(c.encoding, UTF_8);
    }

    fn transcode(bytes: &[u8], encoding: &'static Encoding) -> (String, TranscodeStats) {
        let mut out = Vec::new();
        let stats = transcode_to_utf8(bytes, &mut out, encoding).unwrap();
        (String::from_utf8(out).unwrap(), stats)
    }

    #[test]
    fn transcodes_legacy_encodings() {
        let (text, stats) = transcode(b"caf\xe9 \x80", WINDOWS_1252);
        assert_eq!(text, "café €");
        assert!(!stats.lossy);
        assert_eq!(stats.bytes_in, 6);

        let (text, _) = transcode(&[0x93, 0xfa, 0x96, 0x7b], SHIFT_JIS);
        assert_eq!(text, "日本");

        let (text, _) = transcode(b"\xFF\xFEh\x00i\x00", UTF_16LE);
        assert_eq!(text, "hi");
    }

    #[test]
    fn invalid_utf8_is_replaced_and_reported() {
        let (text, stats) = transcode(b"ok \xff\xfe end", UTF_8);
        assert_eq!(text, "ok \u{FFFD}\u{FFFD} end");
        assert!(stats.lossy);
    }

    #[test]
    fn utf8_bom_removed_and_large_input_streams() {
        let mut input = b"\xEF\xBB\xBF".to_vec();
        input.extend(std::iter::repeat_n("é".as_bytes(), 200_000).flatten());
        let (text, stats) = transcode(&input, UTF_8);
        assert_eq!(text.chars().count(), 200_000);
        assert!(!text.starts_with('\u{FEFF}'));
        assert!(!stats.lossy);
        assert_eq!(stats.bytes_out, 400_000);
    }

    #[test]
    fn multibyte_sequence_split_across_chunks() {
        // 64 KiB chunk boundary falls inside a 2-byte sequence.
        let mut input = vec![b'a'; 64 * 1024 - 1];
        input.extend("é".as_bytes());
        let (text, stats) = transcode(&input, UTF_8);
        assert!(text.ends_with('é'));
        assert!(!stats.lossy);
    }

    #[test]
    fn pretty_json_reindents() {
        let pretty = pretty_json(r#"{"a":1,"b":[true,null,{"c":"x"}],"d":{},"e":[ ]}"#).unwrap();
        assert_eq!(
            pretty,
            "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    null,\n    {\n      \"c\": \"x\"\n    }\n  ],\n  \"d\": {},\n  \"e\": []\n}"
        );
    }

    #[test]
    fn pretty_json_preserves_numbers_and_strings_exactly() {
        let input = r#"[1.0e10,123456789012345678901234567890,-0.000,"a\"b\\cé {,:}","日本"]"#;
        let pretty = pretty_json(input).unwrap();
        for token in [
            "1.0e10",
            "123456789012345678901234567890",
            "-0.000",
            r#""a\"b\\cé {,:}""#,
            "\"日本\"",
        ] {
            assert!(pretty.contains(token), "{token} missing from {pretty}");
        }
    }

    #[test]
    fn pretty_json_scalars_and_already_pretty() {
        assert_eq!(pretty_json(" 42 ").unwrap(), "42");
        let once = pretty_json(r#"{"a":[1,2]}"#).unwrap();
        assert_eq!(pretty_json(&once).unwrap(), once);
    }

    #[test]
    fn pretty_json_rejects_invalid() {
        for bad in ["{", r#"{"a":}"#, "[1,]", "{} {}", "", "nope", r#"{"a":1}x"#] {
            assert_eq!(pretty_json(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn pretty_xml_reindents() {
        let pretty = pretty_xml("<?xml version=\"1.0\"?><a><b x=\"1\">hi</b><c/></a>").unwrap();
        assert_eq!(
            pretty,
            "<?xml version=\"1.0\"?>\n<a>\n  <b x=\"1\">hi</b>\n  <c/>\n</a>"
        );
    }

    #[test]
    fn fold_case_handles_common_scripts() {
        assert_eq!(fold_case("HeLLo"), "hello");
        assert_eq!(fold_case("ÉCOLE"), "école");
        assert_eq!(fold_case("ΣΊΣΥΦΟΣ"), fold_case("σίσυφος"), "all sigmas match");
        assert_eq!(fold_case("ẞ"), "ß");
        assert_ne!(fold_case("straße"), fold_case("STRASSE"), "no full folding");
        assert_eq!(fold_case("I"), "i", "not Turkish-specific");
        assert_eq!(fold_case("ı"), "ı");
    }

    #[test]
    fn find_folded_returns_original_byte_ranges() {
        let hay = "Ünïcode Café ΣΟΦΊΑ end";
        let r = find_folded(hay, &fold_case("café")).unwrap();
        assert_eq!(&hay[r], "Café");
        let r = find_folded(hay, &fold_case("σοφία")).unwrap();
        assert_eq!(&hay[r], "ΣΟΦΊΑ");
        let r = find_folded(hay, "ünï").unwrap();
        assert_eq!(&hay[r], "Ünï");
        assert_eq!(find_folded(hay, &fold_case("missing")), None);
        assert_eq!(find_folded(hay, ""), None);
        // A match ending inside a multi-char folding covers the whole original char.
        let r = find_folded("xİy", "xi").unwrap();
        assert_eq!(&"xİy"[r], "xİ");
    }

    #[test]
    fn pretty_xml_rejects_invalid() {
        assert_eq!(pretty_xml("<a><b></a>"), None);
        assert_eq!(pretty_xml("<a>"), None);
        assert_eq!(pretty_xml("just text"), None);
    }

    #[test]
    fn pretty_dispatches_by_kind() {
        assert!(pretty(BodyKind::Json, "[1]").is_some());
        assert!(pretty(BodyKind::Xml, "<a/>").is_some());
        assert!(pretty(BodyKind::Text, "[1]").is_none());
        assert!(BodyKind::Json.can_pretty_print() && !BodyKind::Html.can_pretty_print());
    }
}
