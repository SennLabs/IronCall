//! Splits a pasted cURL command into arguments.
//!
//! Two shells are supported, detected automatically:
//! - **POSIX** (bash, zsh, sh): single and double quotes, backslash escapes, `$'…'`
//!   ANSI-C strings and `\` line continuations. What Chrome's and Firefox's "Copy as
//!   cURL (bash)" produce, and what most API docs show.
//! - **Windows cmd**: Chrome's "Copy as cURL (cmd)". cmd removes `^` escapes and `^`
//!   line continuations first; the program (curl.exe, using the MSVC runtime) then
//!   splits the result on spaces outside `"…"`, with `\"` for a literal quote.

use super::ParseError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Posix,
    Cmd,
}

/// Guesses the shell a command was written for.
pub fn detect(text: &str) -> Shell {
    let cmd_continuation = text
        .lines()
        .any(|l| l.trim_end_matches('\r').ends_with(" ^") || l.trim_end_matches('\r') == "^");
    if text.contains("^\"") || cmd_continuation {
        Shell::Cmd
    } else {
        Shell::Posix
    }
}

/// Splits `text` into arguments for `shell`.
pub fn tokenize(text: &str, shell: Shell) -> Result<Vec<String>, ParseError> {
    match shell {
        Shell::Posix => posix(&text.replace("\r\n", "\n")),
        Shell::Cmd => Ok(msvc_argv(&cmd_unescape(text))),
    }
}

fn posix(text: &str) -> Result<Vec<String>, ParseError> {
    let mut args = Vec::new();
    let mut current = String::new();
    // Whether `current` is a word, even if empty (`''` is an empty argument).
    let mut in_word = false;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' => {
                if in_word {
                    args.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            '\\' => match chars.next() {
                // Line continuation.
                Some('\n') => {}
                Some(next) => {
                    current.push(next);
                    in_word = true;
                }
                None => {
                    current.push('\\');
                    in_word = true;
                }
            },
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => current.push(ch),
                        None => return Err(ParseError::UnterminatedQuote('\'')),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            // Inside double quotes, backslash only escapes these.
                            Some(ch @ ('$' | '`' | '"' | '\\')) => current.push(ch),
                            Some('\n') => {}
                            Some(ch) => {
                                current.push('\\');
                                current.push(ch);
                            }
                            None => return Err(ParseError::UnterminatedQuote('"')),
                        },
                        Some(ch) => current.push(ch),
                        None => return Err(ParseError::UnterminatedQuote('"')),
                    }
                }
            }
            '$' if chars.peek() == Some(&'\'') => {
                chars.next();
                in_word = true;
                ansi_c(&mut chars, &mut current)?;
            }
            '#' if !in_word => {
                // A comment runs to the end of the line.
                for ch in chars.by_ref() {
                    if ch == '\n' {
                        break;
                    }
                }
            }
            _ => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        args.push(current);
    }
    Ok(args)
}

/// Reads the rest of a `$'…'` string (after the opening quote).
fn ansi_c(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, out: &mut String) -> Result<(), ParseError> {
    loop {
        match chars.next() {
            Some('\'') => return Ok(()),
            Some('\\') => {
                let Some(esc) = chars.next() else {
                    return Err(ParseError::UnterminatedQuote('\''));
                };
                match esc {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'a' => out.push('\u{7}'),
                    'b' => out.push('\u{8}'),
                    'e' | 'E' => out.push('\u{1b}'),
                    'f' => out.push('\u{c}'),
                    'v' => out.push('\u{b}'),
                    '0'..='7' => {
                        let mut value = esc.to_digit(8).unwrap_or(0);
                        for _ in 0..2 {
                            match chars.peek().and_then(|c| c.to_digit(8)) {
                                Some(d) => {
                                    value = value * 8 + d;
                                    chars.next();
                                }
                                None => break,
                            }
                        }
                        push_code(out, value);
                    }
                    'x' => {
                        let value = hex_digits(chars, 2);
                        push_code(out, value);
                    }
                    'u' => push_code(out, hex_digits(chars, 4)),
                    'U' => push_code(out, hex_digits(chars, 8)),
                    other => out.push(other),
                }
            }
            Some(ch) => out.push(ch),
            None => return Err(ParseError::UnterminatedQuote('\'')),
        }
    }
}

fn hex_digits(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, max: usize) -> u32 {
    let mut value = 0;
    for _ in 0..max {
        match chars.peek().and_then(|c| c.to_digit(16)) {
            Some(d) => {
                value = value * 16 + d;
                chars.next();
            }
            None => break,
        }
    }
    value
}

fn push_code(out: &mut String, value: u32) {
    if let Some(c) = char::from_u32(value) {
        out.push(c);
    }
}

/// cmd's own pass: `^x` becomes `x` outside quotes, and `^` at the end of a line joins
/// the next line (the first character of that line is taken literally, so Chrome's
/// `^` + newline + newline produces one literal newline).
fn cmd_unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                out.push('"');
            }
            '^' if !in_quotes => match chars.next() {
                Some('\r') if chars.peek() == Some(&'\n') => {
                    chars.next();
                    literal_after_continuation(&mut chars, &mut out);
                }
                Some('\n') => literal_after_continuation(&mut chars, &mut out),
                Some(next) => out.push(next),
                None => {}
            },
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

fn literal_after_continuation(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, out: &mut String) {
    match chars.peek() {
        Some('\r') => {
            chars.next();
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        }
        Some('\n') => {
            chars.next();
            out.push('\n');
        }
        _ => {}
    }
}

/// The MSVC runtime's command-line splitting (`CommandLineToArgvW` rules).
fn msvc_argv(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut in_quotes = false;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                let start = i;
                while i < chars.len() && chars[i] == '\\' {
                    i += 1;
                }
                let count = i - start;
                if i < chars.len() && chars[i] == '"' {
                    current.extend(std::iter::repeat_n('\\', count / 2));
                    if count % 2 == 1 {
                        current.push('"');
                        i += 1;
                    }
                } else {
                    current.extend(std::iter::repeat_n('\\', count));
                }
                in_word = true;
                continue;
            }
            '"' => {
                in_word = true;
                if in_quotes && chars.get(i + 1) == Some(&'"') {
                    // `""` inside quotes is a literal quote.
                    current.push('"');
                    i += 1;
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ' ' | '\t' | '\n' if !in_quotes => {
                if in_word {
                    args.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            _ => {
                current.push(c);
                in_word = true;
            }
        }
        i += 1;
    }
    if in_word {
        args.push(current);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn posix_args(s: &str) -> Vec<String> {
        tokenize(s, Shell::Posix).unwrap()
    }

    #[test]
    fn posix_quotes_escapes_and_continuations() {
        assert_eq!(
            posix_args("curl 'https://x/a b' \\\n  -H \"X: \\\"q\\\" \\$HOME\" --data a\\ b"),
            ["curl", "https://x/a b", "-H", "X: \"q\" $HOME", "--data", "a b"]
        );
        assert_eq!(posix_args("curl '' x"), ["curl", "", "x"]);
        assert_eq!(posix_args("curl 'it'\\''s'"), ["curl", "it's"]);
        assert_eq!(posix_args("curl\r\n  -k \\\r\n  x"), ["curl", "-k", "x"]);
    }

    #[test]
    fn posix_ansi_c_strings() {
        assert_eq!(
            posix_args(r"curl $'a\nb\t\x41\u00e9\'' $'\101'"),
            ["curl", "a\nb\tAé'", "A"]
        );
    }

    #[test]
    fn posix_comments_and_unterminated_quotes() {
        assert_eq!(posix_args("curl x # trailing comment\n -k"), ["curl", "x", "-k"]);
        assert_eq!(posix_args("curl a#b"), ["curl", "a#b"]);
        assert!(matches!(
            tokenize("curl 'oops", Shell::Posix),
            Err(ParseError::UnterminatedQuote('\''))
        ));
    }

    #[test]
    fn cmd_chrome_format() {
        let text = "curl ^\"https://example.com/api?a=1^&b=2^\" ^\r\n  -H ^\"accept: */*^\" ^\r\n  --data-raw ^\"^{^\\^\"name^\\^\":^\\^\"Ada^\\^\"^}^\"";
        assert_eq!(detect(text), Shell::Cmd);
        assert_eq!(
            tokenize(text, Shell::Cmd).unwrap(),
            [
                "curl",
                "https://example.com/api?a=1&b=2",
                "-H",
                "accept: */*",
                "--data-raw",
                r#"{"name":"Ada"}"#
            ]
        );
    }

    #[test]
    fn cmd_literal_newline_and_percent() {
        // Chrome writes a newline inside data as `^` + newline + newline.
        let text = "curl ^\"https://x^\" --data-raw ^\"line1^\n\nline2 100^%^\"";
        assert_eq!(tokenize(text, Shell::Cmd).unwrap()[3], "line1\nline2 100%");
    }

    #[test]
    fn msvc_backslash_rules() {
        assert_eq!(msvc_argv(r#"a\\"b c" d"#), [r"a\b c", "d"]);
        assert_eq!(msvc_argv(r#""a\"b""#), [r#"a"b"#]);
        assert_eq!(msvc_argv(r"C:\path\file"), [r"C:\path\file"]);
        assert_eq!(msvc_argv(r#""say ""hi""""#), [r#"say "hi""#]);
    }

    #[test]
    fn detection() {
        assert_eq!(detect("curl 'https://x' -H 'a: b'"), Shell::Posix);
        assert_eq!(detect("curl \"https://x\" ^\n -k"), Shell::Cmd);
    }
}
