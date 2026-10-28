//! Streaming pretty-printers for bodies of any size (PLAN.md Phase 5).
//!
//! [`pretty_json_stream`] produces exactly the output of [`super::pretty_json`] but reads
//! and writes in chunks with bounded memory, so a 100MB response can be pretty-printed
//! without holding it (or a huge string token inside it) in memory. On error the writer
//! may hold partial output; the caller should discard it.

use std::io::{self, BufReader, BufWriter, Read, Write};

use super::BodyKind;

/// Read and write buffer size.
const CHUNK: usize = 64 * 1024;

/// Containers nested deeper than this are rejected (the stack is explicit, so this is a
/// sanity limit, not a stack-overflow guard).
pub const MAX_DEPTH: usize = 10_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrettyStats {
    pub bytes_in: u64,
    pub bytes_out: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum PrettyError {
    /// The input isn't valid JSON or XML. `offset` is the byte offset in the input.
    #[error("invalid {format} at byte {offset}: {message}")]
    Invalid {
        format: &'static str,
        offset: u64,
        message: String,
    },
    #[error("{0} bodies can't be pretty-printed")]
    Unsupported(&'static str),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Pretty-prints a JSON or XML body from `reader` to `writer`.
pub fn pretty_stream(
    kind: BodyKind,
    reader: impl Read,
    writer: impl Write,
) -> Result<PrettyStats, PrettyError> {
    match kind {
        BodyKind::Json => pretty_json_stream(reader, writer),
        BodyKind::Xml => pretty_xml_stream(reader, writer),
        BodyKind::Empty => Err(PrettyError::Unsupported("empty")),
        BodyKind::Html => Err(PrettyError::Unsupported("HTML")),
        BodyKind::Text => Err(PrettyError::Unsupported("text")),
        BodyKind::Binary => Err(PrettyError::Unsupported("binary")),
    }
}

// ---------------------------------------------------------------------------------
// JSON

/// What the parser expects next, outside of a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    /// A value (top level, after `:`, or after `,` in an array).
    Value,
    /// Right after `[` or `{` (output of the bracket is held back until we know whether
    /// the container is empty, so `[ ]` prints as `[]`).
    AfterOpen(u8),
    /// A key string (after `,` in an object, or after a non-empty `{`).
    Key,
    Colon,
    /// `,` or the closing bracket of the current container.
    CommaOrClose,
    /// The top-level value is complete; only whitespace may follow.
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Num {
    /// After `-`.
    Minus,
    /// After a leading `0`.
    Zero,
    /// In the integer part after a non-zero digit.
    Int,
    /// After `.`.
    Dot,
    Frac,
    /// After `e`/`E`.
    Exp,
    /// After the exponent sign.
    ExpSign,
    ExpDigits,
}

impl Num {
    fn can_end(self) -> bool {
        matches!(self, Num::Zero | Num::Int | Num::Frac | Num::ExpDigits)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    None,
    /// Inside a string. `escape`: 0 = normal, 1 = after `\`, 2..=5 = in `\uXXXX`
    /// (number of hex digits still to read is `6 - escape`).
    Str {
        key: bool,
        escape: u8,
    },
    Num(Num),
    Lit {
        word: &'static [u8],
        pos: u8,
    },
}

struct JsonPrinter<W: Write> {
    out: W,
    buf: Vec<u8>,
    stack: Vec<u8>,
    expect: Expect,
    token: Token,
    offset: u64,
    bytes_out: u64,
}

impl<W: Write> JsonPrinter<W> {
    fn new(out: W) -> Self {
        Self {
            out,
            buf: Vec::with_capacity(CHUNK * 2),
            stack: Vec::new(),
            expect: Expect::Value,
            token: Token::None,
            offset: 0,
            bytes_out: 0,
        }
    }

    fn err(&self, at: usize, message: impl Into<String>) -> PrettyError {
        PrettyError::Invalid {
            format: "JSON",
            offset: self.offset + at as u64,
            message: message.into(),
        }
    }

    fn flush_if_full(&mut self) -> io::Result<()> {
        if self.buf.len() >= CHUNK {
            self.out.write_all(&self.buf)?;
            self.bytes_out += self.buf.len() as u64;
            self.buf.clear();
        }
        Ok(())
    }

    fn newline(&mut self) {
        self.buf.push(b'\n');
        let depth = self.stack.len();
        self.buf.extend(std::iter::repeat_n(b' ', depth * 2));
    }

    fn after_value(&mut self) {
        self.expect = if self.stack.is_empty() {
            Expect::End
        } else {
            Expect::CommaOrClose
        };
    }

    /// Processes one chunk of input.
    fn feed(&mut self, chunk: &[u8]) -> Result<(), PrettyError> {
        let mut i = 0;
        while i < chunk.len() {
            match self.token {
                Token::Str { key, escape } => {
                    if escape == 0 {
                        // Fast path: copy the run of ordinary string bytes.
                        let run = chunk[i..]
                            .iter()
                            .position(|&b| b == b'"' || b == b'\\' || b < 0x20)
                            .unwrap_or(chunk.len() - i);
                        self.buf.extend_from_slice(&chunk[i..i + run]);
                        i += run;
                        if i == chunk.len() {
                            break;
                        }
                        let b = chunk[i];
                        self.buf.push(b);
                        match b {
                            b'"' => {
                                self.token = Token::None;
                                if key {
                                    self.expect = Expect::Colon;
                                } else {
                                    self.after_value();
                                }
                            }
                            b'\\' => self.token = Token::Str { key, escape: 1 },
                            _ => return Err(self.err(i, "control character in string")),
                        }
                    } else {
                        let b = chunk[i];
                        if escape == 1 {
                            match b {
                                b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {
                                    self.token = Token::Str { key, escape: 0 }
                                }
                                b'u' => self.token = Token::Str { key, escape: 2 },
                                _ => return Err(self.err(i, "invalid escape")),
                            }
                        } else {
                            if !b.is_ascii_hexdigit() {
                                return Err(self.err(i, "invalid \\u escape"));
                            }
                            let next = if escape == 5 { 0 } else { escape + 1 };
                            self.token = Token::Str { key, escape: next };
                        }
                        self.buf.push(b);
                    }
                    i += 1;
                }
                Token::Num(state) => {
                    let b = chunk[i];
                    let next = match (state, b) {
                        (Num::Minus, b'0') => Some(Num::Zero),
                        (Num::Minus, b'1'..=b'9') => Some(Num::Int),
                        (Num::Int, b'0'..=b'9') => Some(Num::Int),
                        (Num::Zero | Num::Int, b'.') => Some(Num::Dot),
                        (Num::Dot | Num::Frac, b'0'..=b'9') => Some(Num::Frac),
                        (Num::Zero | Num::Int | Num::Frac, b'e' | b'E') => Some(Num::Exp),
                        (Num::Exp, b'+' | b'-') => Some(Num::ExpSign),
                        (Num::Exp | Num::ExpSign | Num::ExpDigits, b'0'..=b'9') => Some(Num::ExpDigits),
                        _ => None,
                    };
                    match next {
                        Some(n) => {
                            self.buf.push(b);
                            self.token = Token::Num(n);
                            i += 1;
                        }
                        None if state.can_end() => {
                            // The number ended; reprocess this byte as structure.
                            self.token = Token::None;
                            self.after_value();
                        }
                        None => return Err(self.err(i, "invalid number")),
                    }
                }
                Token::Lit { word, pos } => {
                    let b = chunk[i];
                    if b != word[pos as usize] {
                        return Err(self.err(i, "invalid literal"));
                    }
                    self.buf.push(b);
                    if pos as usize + 1 == word.len() {
                        self.token = Token::None;
                        self.after_value();
                    } else {
                        self.token = Token::Lit { word, pos: pos + 1 };
                    }
                    i += 1;
                }
                Token::None => {
                    let b = chunk[i];
                    if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                        i += 1;
                        continue;
                    }
                    self.structural(b, i)?;
                    i += 1;
                }
            }
            self.flush_if_full()?;
        }
        self.offset += chunk.len() as u64;
        Ok(())
    }

    /// Handles a non-whitespace byte outside any token.
    fn structural(&mut self, b: u8, at: usize) -> Result<(), PrettyError> {
        if let Expect::AfterOpen(open) = self.expect {
            let close = if open == b'{' { b'}' } else { b']' };
            if b == close {
                self.stack.pop();
                self.buf.push(open);
                self.buf.push(close);
                self.after_value();
                return Ok(());
            }
            self.buf.push(open);
            self.newline();
            self.expect = if open == b'{' { Expect::Key } else { Expect::Value };
        }
        match self.expect {
            Expect::Value => self.start_value(b, at),
            Expect::Key => {
                if b != b'"' {
                    return Err(self.err(at, "expected a key string"));
                }
                self.buf.push(b);
                self.token = Token::Str { key: true, escape: 0 };
                Ok(())
            }
            Expect::Colon => {
                if b != b':' {
                    return Err(self.err(at, "expected `:`"));
                }
                self.buf.extend_from_slice(b": ");
                self.expect = Expect::Value;
                Ok(())
            }
            Expect::CommaOrClose => {
                let top = *self.stack.last().expect("CommaOrClose implies an open container");
                let close = if top == b'{' { b'}' } else { b']' };
                if b == b',' {
                    self.buf.push(b',');
                    self.newline();
                    self.expect = if top == b'{' { Expect::Key } else { Expect::Value };
                    Ok(())
                } else if b == close {
                    self.stack.pop();
                    self.newline();
                    self.buf.push(b);
                    self.after_value();
                    Ok(())
                } else {
                    Err(self.err(at, "expected `,` or a closing bracket"))
                }
            }
            Expect::End => Err(self.err(at, "trailing characters after the JSON value")),
            Expect::AfterOpen(_) => unreachable!("handled above"),
        }
    }

    fn start_value(&mut self, b: u8, at: usize) -> Result<(), PrettyError> {
        match b {
            b'{' | b'[' => {
                if self.stack.len() >= MAX_DEPTH {
                    return Err(self.err(at, format!("nested deeper than {MAX_DEPTH} levels")));
                }
                self.stack.push(b);
                self.expect = Expect::AfterOpen(b);
            }
            b'"' => {
                self.buf.push(b);
                self.token = Token::Str {
                    key: false,
                    escape: 0,
                };
            }
            b'-' => {
                self.buf.push(b);
                self.token = Token::Num(Num::Minus);
            }
            b'0' => {
                self.buf.push(b);
                self.token = Token::Num(Num::Zero);
            }
            b'1'..=b'9' => {
                self.buf.push(b);
                self.token = Token::Num(Num::Int);
            }
            b't' | b'f' | b'n' => {
                let word: &'static [u8] = match b {
                    b't' => b"true",
                    b'f' => b"false",
                    _ => b"null",
                };
                self.buf.push(b);
                self.token = Token::Lit { word, pos: 1 };
            }
            _ => return Err(self.err(at, "expected a value")),
        }
        Ok(())
    }

    fn finish(mut self) -> Result<u64, PrettyError> {
        if let Token::Num(state) = self.token {
            if !state.can_end() {
                return Err(self.err(0, "invalid number at end of input"));
            }
            self.token = Token::None;
            self.after_value();
        }
        if self.token != Token::None || self.expect != Expect::End {
            return Err(self.err(0, "unexpected end of input"));
        }
        self.out.write_all(&self.buf)?;
        self.bytes_out += self.buf.len() as u64;
        self.out.flush()?;
        Ok(self.bytes_out)
    }
}

/// Validates UTF-8 across chunk boundaries.
#[derive(Default)]
struct Utf8Check {
    /// The start of a multi-byte sequence cut off at the end of the previous chunk.
    carry: Vec<u8>,
}

/// Length of a UTF-8 sequence from its (valid) lead byte.
fn utf8_len(lead: u8) -> usize {
    match lead {
        0xF0..=0xF7 => 4,
        0xE0..=0xEF => 3,
        _ => 2,
    }
}

impl Utf8Check {
    /// Checks one chunk. On error returns the offset of the bad byte in this chunk.
    fn check(&mut self, chunk: &[u8]) -> Result<(), usize> {
        let mut start = 0;
        if !self.carry.is_empty() {
            let need = utf8_len(self.carry[0]) - self.carry.len();
            let take = need.min(chunk.len());
            self.carry.extend_from_slice(&chunk[..take]);
            match std::str::from_utf8(&self.carry) {
                Ok(_) => self.carry.clear(),
                Err(e) if e.error_len().is_none() && take < need => return Ok(()),
                Err(_) => return Err(0),
            }
            start = take;
        }
        let data = &chunk[start..];
        match std::str::from_utf8(data) {
            Ok(_) => Ok(()),
            Err(e) if e.error_len().is_none() => {
                self.carry = data[e.valid_up_to()..].to_vec();
                Ok(())
            }
            Err(e) => Err(start + e.valid_up_to()),
        }
    }

    fn finish(&self) -> bool {
        self.carry.is_empty()
    }
}

/// Pretty-prints JSON from `reader` to `writer` with the same output as
/// [`super::pretty_json`] (2-space indent; numbers, strings and escapes copied byte for
/// byte), using bounded memory. A leading UTF-8 BOM is skipped. Unlike `pretty_json`,
/// nesting is allowed up to [`MAX_DEPTH`] levels (serde_json stops at 128).
pub fn pretty_json_stream(mut reader: impl Read, writer: impl Write) -> Result<PrettyStats, PrettyError> {
    let mut printer = JsonPrinter::new(BufWriter::new(writer));
    let mut utf8 = Utf8Check::default();
    let mut chunk = vec![0u8; CHUNK];
    let mut bytes_in = 0u64;
    let mut first = true;
    loop {
        let n = read_full(&mut reader, &mut chunk)?;
        if n == 0 {
            break;
        }
        bytes_in += n as u64;
        let mut start = 0;
        if first {
            first = false;
            // `read_full` fills the buffer unless the input ends, so a BOM is never split.
            if chunk[..n].starts_with(&[0xEF, 0xBB, 0xBF]) {
                start = 3;
                printer.offset = 3;
            }
        }
        if let Err(at) = utf8.check(&chunk[start..n]) {
            return Err(PrettyError::Invalid {
                format: "JSON",
                offset: printer.offset + at as u64,
                message: "invalid UTF-8".into(),
            });
        }
        printer.feed(&chunk[start..n])?;
    }
    if !utf8.finish() {
        return Err(PrettyError::Invalid {
            format: "JSON",
            offset: bytes_in,
            message: "incomplete UTF-8 sequence at end of input".into(),
        });
    }
    let bytes_out = printer.finish()?;
    Ok(PrettyStats { bytes_in, bytes_out })
}

/// Reads until `buf` is full or the input ends.
fn read_full(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

// ---------------------------------------------------------------------------------
// XML

/// Counts bytes read through it, for [`PrettyStats`].
struct Counting<R> {
    inner: R,
    count: u64,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count += n as u64;
        Ok(n)
    }
}

/// Counts bytes written through it.
struct CountingWriter<W> {
    inner: W,
    count: u64,
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.count += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Pretty-prints XML from `reader` to `writer` with the same output as
/// [`super::pretty_xml`], one event at a time (memory is bounded by the largest single
/// tag or text node).
pub fn pretty_xml_stream(reader: impl Read, writer: impl Write) -> Result<PrettyStats, PrettyError> {
    use quick_xml::events::Event;

    let counting = Counting {
        inner: reader,
        count: 0,
    };
    let mut reader = quick_xml::Reader::from_reader(BufReader::with_capacity(CHUNK, counting));
    reader.config_mut().trim_text(true);
    let out = CountingWriter {
        inner: BufWriter::with_capacity(CHUNK, writer),
        count: 0,
    };
    let mut writer = quick_xml::Writer::new_with_indent(out, b' ', 2);
    let invalid = |reader: &quick_xml::Reader<_>, message: String| PrettyError::Invalid {
        format: "XML",
        offset: reader.buffer_position(),
        message,
    };

    let mut buf = Vec::new();
    let mut depth = 0i64;
    let mut saw_element = false;
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| invalid(&reader, e.to_string()))?;
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
        writer.write_event(event).map_err(io::Error::other)?;
        buf.clear();
    }
    if depth != 0 {
        return Err(invalid(&reader, "unclosed element at end of input".into()));
    }
    if !saw_element {
        return Err(invalid(&reader, "no elements".into()));
    }
    let bytes_in = reader.get_ref().get_ref().count;
    let mut out = writer.into_inner();
    out.flush()?;
    Ok(PrettyStats {
        bytes_in,
        bytes_out: out.count,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{pretty_json, pretty_xml};
    use super::*;

    /// Reader that returns at most `n` bytes per call, to exercise chunk boundaries.
    struct Trickle<'a> {
        data: &'a [u8],
        n: usize,
    }

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.n.min(buf.len()).min(self.data.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    fn stream(input: &[u8]) -> Result<String, PrettyError> {
        let mut out = Vec::new();
        pretty_json_stream(input, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    fn corpus() -> Vec<String> {
        let mut c: Vec<String> = [
            r#"{"a":1,"b":[true,null,{"c":"x"}],"d":{},"e":[ ]}"#,
            r#"{"big":123456789012345678901234567890,"f":1.0e10,"neg":-0.5E-3,"z":0}"#,
            r#"{"s":"quote \" backslash \\ slash \/ tab \t nl \n \u00e9 \uD83D\uDE00 end"}"#,
            r#"{"unicode":"café ☕ 😀","key with spaces":"v","":"empty key"}"#,
            r#"  [ 1 , 2 ,3 ]  "#,
            r#""just a string""#,
            "42",
            "-0",
            "true",
            "null",
            r#"[[],[[]],{},[{}],{"a":{}}]"#,
            "{\n  \"already\": [\n    1,\n    2\n  ]\n}",
            r#"{"lone":"\uD800"}"#,
            r#"[1e5,1E+5,1e-5,0.0,10]"#,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        // Deep nesting within serde_json's recursion limit.
        c.push(format!("{}1{}", "[".repeat(100), "]".repeat(100)));
        c.push(format!("{}0{}", "{\"k\":".repeat(60), "}".repeat(60)));
        c
    }

    #[test]
    fn matches_pretty_json_on_corpus() {
        for input in corpus() {
            let expected = pretty_json(&input).unwrap_or_else(|| panic!("corpus entry invalid: {input}"));
            assert_eq!(stream(input.as_bytes()).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn matches_across_every_chunk_boundary() {
        for input in corpus() {
            let expected = pretty_json(&input).unwrap();
            for n in [1, 2, 3, 7] {
                let mut out = Vec::new();
                pretty_json_stream(
                    Trickle {
                        data: input.as_bytes(),
                        n,
                    },
                    &mut out,
                )
                .unwrap();
                assert_eq!(String::from_utf8(out).unwrap(), expected, "n={n} {input}");
            }
        }
    }

    #[test]
    fn rejects_what_pretty_json_rejects() {
        for bad in [
            "",
            "   ",
            "{",
            "[1,]",
            "{\"a\" 1}",
            "{\"a\":1,}",
            "{1:2}",
            "01",
            "1.",
            "1e",
            "-",
            "+1",
            ".5",
            "tru",
            "nulll",
            "[1 2]",
            "{} {}",
            "\"unterminated",
            "\"bad \\x escape\"",
            "\"bad \\u12G4\"",
            "\"control \u{1} char\"",
            "[1]]",
            "}",
            "NaN",
        ] {
            assert_eq!(pretty_json(bad), None, "pretty_json accepted {bad:?}");
            assert!(stream(bad.as_bytes()).is_err(), "stream accepted {bad:?}");
        }
    }

    #[test]
    fn error_reports_byte_offset() {
        match stream(b"[1, 2, x]") {
            Err(PrettyError::Invalid { offset, .. }) => assert_eq!(offset, 7),
            other => panic!("{other:?}"),
        }
        // Past several 64KB chunks.
        let mut input = b"[".to_vec();
        input.extend(b"1,".repeat(100_000));
        input.extend_from_slice(b"x");
        match stream(&input) {
            Err(PrettyError::Invalid { offset, .. }) => assert_eq!(offset, 200_001),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn skips_bom_and_rejects_bad_utf8() {
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice(br#"{"a":1}"#);
        assert_eq!(stream(&with_bom).unwrap(), "{\n  \"a\": 1\n}");
        assert!(stream(b"\"\xff\"").is_err());
        assert!(stream(b"\"\xe2\x82\"").is_err(), "truncated sequence at end");
        // A multi-byte character split across reads is fine.
        let mut out = Vec::new();
        pretty_json_stream(
            Trickle {
                data: "\"€😀\"".as_bytes(),
                n: 1,
            },
            &mut out,
        )
        .unwrap();
        assert_eq!(out, "\"€😀\"".as_bytes());
    }

    #[test]
    fn deep_nesting_uses_an_explicit_stack() {
        let depth = MAX_DEPTH;
        let input = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let out = stream(input.as_bytes()).unwrap();
        // Innermost container is empty: `[]` on the deepest line.
        assert!(out.contains(&format!("\n{}[]", " ".repeat((depth - 1) * 2))));
        let too_deep = format!("{}{}", "[".repeat(depth + 1), "]".repeat(depth + 1));
        assert!(matches!(
            stream(too_deep.as_bytes()),
            Err(PrettyError::Invalid { .. })
        ));
    }

    #[test]
    fn huge_string_token_streams() {
        let big = "x".repeat(1024 * 1024);
        let input = format!(r#"{{"s":"{big}","n":1}}"#);
        assert_eq!(stream(input.as_bytes()).unwrap(), pretty_json(&input).unwrap());
    }

    #[test]
    fn reports_stats() {
        let mut out = Vec::new();
        let stats = pretty_json_stream(&br#"{"a":[1,2]}"#[..], &mut out).unwrap();
        assert_eq!(stats.bytes_in, 11);
        assert_eq!(stats.bytes_out, out.len() as u64);
    }

    #[test]
    fn xml_matches_pretty_xml() {
        for input in [
            "<?xml version=\"1.0\"?><a><b x=\"1\">hi</b><c/></a>",
            "<root>\n  <item id=\"1\">  text  </item>\n<!-- c --><![CDATA[ raw <x> ]]></root>",
            "<a xmlns:p=\"urn:x\"><p:b>&amp;&lt;</p:b></a>",
        ] {
            let expected = pretty_xml(input).unwrap();
            let mut out = Vec::new();
            pretty_xml_stream(
                Trickle {
                    data: input.as_bytes(),
                    n: 3,
                },
                &mut out,
            )
            .unwrap();
            assert_eq!(String::from_utf8(out).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn xml_rejects_invalid() {
        for bad in ["<a><b></a>", "<a>", "just text"] {
            assert_eq!(pretty_xml(bad), None);
            assert!(pretty_xml_stream(bad.as_bytes(), Vec::new()).is_err(), "{bad}");
        }
    }

    #[test]
    fn pretty_stream_dispatches() {
        let mut out = Vec::new();
        pretty_stream(BodyKind::Json, &b"[1]"[..], &mut out).unwrap();
        assert_eq!(out, b"[\n  1\n]");
        assert!(matches!(
            pretty_stream(BodyKind::Text, &b"x"[..], Vec::new()),
            Err(PrettyError::Unsupported(_))
        ));
    }
}
