//! cURL import and export (PLAN.md Phase 6).
//!
//! - [`parse`] turns a pasted command (bash/POSIX, or Chrome's "Copy as cURL (cmd)")
//!   into a [`RequestFile`]. Anything that can't be represented produces a [`Warning`];
//!   nothing is dropped silently.
//! - [`export`] turns a built [`WireRequest`] into a command for bash or PowerShell.
//!
//! Behaviour notes:
//! - curl doesn't follow redirects without `-L`, but IronCall does by default. Imported
//!   requests keep the app default either way (an import without `-L` doesn't turn
//!   following off), and exported commands always say `-L` when following.
//! - Query strings and form bodies are percent-decoded on import (shown readable in the
//!   tables) and encoded again when sent (PLAN.md §5). This is equivalent for servers
//!   except in one corner: an encoded `%` followed by hex (`%2541`) comes back as
//!   `%41`, because already-encoded sequences aren't encoded twice.

mod export;
mod parse;
mod tokenize;

pub use export::{Dialect, ExportOptions, export};
pub use ironcall_core::model::RequestFile;
pub use parse::parse;
pub use tokenize::Shell;

/// Something in the command that couldn't be imported exactly, or a note about how it
/// was interpreted.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Warning {
    pub message: String,
    /// The flag or argument it applies to, e.g. `--cert`.
    pub location: Option<String>,
}

impl Warning {
    fn new(message: impl Into<String>, location: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            location: Some(location.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Parsed {
    pub request: RequestFile,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("paste a cURL command (it should start with `curl`)")]
    NotCurl,
    #[error("the command has an unterminated {0} quote")]
    UnterminatedQuote(char),
    #[error("`{0}` needs a value")]
    MissingValue(String),
    #[error("the command has no URL")]
    NoUrl,
    #[error("{0}")]
    Invalid(String),
}

/// Something written by [`export`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    pub command: String,
    pub warnings: Vec<Warning>,
}

/// Percent-decodes `s`, with `+` as a space if `plus_is_space`. Invalid UTF-8 after
/// decoding returns the input unchanged.
pub(crate) fn percent_decode(s: &str, plus_is_space: bool) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = |b: u8| (b as char).to_digit(16);
                match (
                    bytes.get(i + 1).and_then(|b| hex(*b)),
                    bytes.get(i + 2).and_then(|b| hex(*b)),
                ) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                        continue;
                    }
                    _ => out.push(b'%'),
                }
            }
            b'+' if plus_is_space => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::percent_decode;

    #[test]
    fn decodes_percent_and_plus() {
        assert_eq!(percent_decode("a%20b+c", true), "a b c");
        assert_eq!(percent_decode("a+b", false), "a+b");
        assert_eq!(percent_decode("100%", true), "100%");
        assert_eq!(percent_decode("%zz%4", true), "%zz%4");
        assert_eq!(percent_decode("caf%C3%A9", true), "café");
        // Invalid UTF-8 after decoding: keep the original text.
        assert_eq!(percent_decode("%FF", true), "%FF");
    }
}
