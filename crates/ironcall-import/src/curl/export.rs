//! [`WireRequest`] → cURL command.

use base64::Engine as _;
use ironcall_core::model::HttpVersion;
use ironcall_core::wire::{PartContent, WireBody, WireDefaults, WireRequest};

use super::{Export, Warning};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    /// bash, zsh, sh: single quotes, `\` line continuations.
    Posix,
    /// PowerShell: `curl.exe` (in Windows PowerShell 5.1 `curl` is an alias for
    /// `Invoke-WebRequest`), single quotes, backtick line continuations.
    PowerShell,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ExportOptions {
    /// Put the whole command on one line instead of one option per line.
    pub single_line: bool,
}

/// One argument: an option (written as is) or a value (quoted).
enum Arg {
    Flag(&'static str),
    Value(String),
}

/// Builds a cURL command for `req`. Only headers in `req` are written: the engine's own
/// (User-Agent, Accept-Encoding, Host, Content-Length) aren't part of a [`WireRequest`]
/// unless the user set them.
pub fn export(req: &WireRequest, dialect: Dialect, options: ExportOptions) -> Export {
    let mut warnings = Vec::new();
    // Each line is one option with its value (or the URL).
    let mut lines: Vec<Vec<Arg>> = vec![vec![Arg::Value(req.url.clone())]];
    let mut line = |args: Vec<Arg>| lines.push(args);

    let has_body = match &req.body {
        WireBody::Bytes(b) => !b.is_empty(),
        WireBody::File { .. } | WireBody::Multipart(_) => true,
    };
    match (req.method.as_str(), has_body) {
        ("HEAD", false) => line(vec![Arg::Flag("--head")]),
        ("GET", false) | ("POST", true) => {}
        (method, _) => line(vec![Arg::Flag("-X"), Arg::Value(method.to_string())]),
    }

    for (name, value) in &req.headers {
        if name.eq_ignore_ascii_case("authorization")
            && let Some(user) = basic_credentials(value)
        {
            line(vec![Arg::Flag("-u"), Arg::Value(user)]);
            continue;
        }
        let header = if value.is_empty() {
            // `Name;` is curl's way to send a header with an empty value.
            format!("{name};")
        } else {
            format!("{name}: {value}")
        };
        line(vec![Arg::Flag("-H"), Arg::Value(header)]);
    }

    match &req.body {
        WireBody::Bytes(bytes) if bytes.is_empty() => {}
        WireBody::Bytes(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => line(vec![Arg::Flag("--data-raw"), Arg::Value(text.to_string())]),
            Err(_) => warnings.push(Warning {
                message: "the body isn't text, so it can't be written into the command; it was left out"
                    .into(),
                location: Some("--data-binary".into()),
            }),
        },
        WireBody::File { path, .. } => {
            warnings.push(file_warning());
            line(vec![
                Arg::Flag("--data-binary"),
                Arg::Value(format!("@{}", path.display())),
            ]);
        }
        WireBody::Multipart(parts) => {
            let mut has_file = false;
            for part in parts {
                match &part.content {
                    // `--form-string` never treats a leading `@` or `<` specially.
                    PartContent::Text(value) => line(vec![
                        Arg::Flag("--form-string"),
                        Arg::Value(format!("{}={value}", part.name)),
                    ]),
                    PartContent::File {
                        path, content_type, ..
                    } => {
                        has_file = true;
                        let path = path.display().to_string();
                        let path = if path.contains([';', ',', '"']) {
                            format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
                        } else {
                            path
                        };
                        line(vec![
                            Arg::Flag("-F"),
                            Arg::Value(format!("{}=@{path};type={content_type}", part.name)),
                        ]);
                    }
                }
            }
            if has_file {
                warnings.push(file_warning());
            }
        }
    }

    let defaults = WireDefaults::default();
    if !req.verify_tls {
        line(vec![Arg::Flag("-k")]);
    }
    match req.http_version {
        HttpVersion::Http1 => line(vec![Arg::Flag("--http1.1")]),
        HttpVersion::Http2 => line(vec![Arg::Flag("--http2")]),
        HttpVersion::Auto => {}
    }
    if req.follow_redirects {
        line(vec![Arg::Flag("-L")]);
        if req.max_redirects != defaults.max_redirects {
            line(vec![
                Arg::Flag("--max-redirs"),
                Arg::Value(req.max_redirects.to_string()),
            ]);
        }
    }
    if let Some(ms) = req.timeout_ms
        && Some(ms) != defaults.timeout_ms
    {
        line(vec![Arg::Flag("-m"), Arg::Value(seconds(ms))]);
    }

    if dialect == Dialect::PowerShell
        && lines
            .iter()
            .flatten()
            .any(|a| matches!(a, Arg::Value(v) if v.contains('"')))
    {
        warnings.push(Warning {
            message: "Windows PowerShell 5.1 removes double quotes from arguments to curl.exe; use PowerShell 7.3 or later for this command".into(),
            location: None,
        });
    }

    let program = match dialect {
        Dialect::Posix => "curl",
        Dialect::PowerShell => "curl.exe",
    };
    let rendered: Vec<String> = lines
        .iter()
        .map(|args| {
            args.iter()
                .map(|a| match a {
                    Arg::Flag(f) => (*f).to_string(),
                    Arg::Value(v) => quote(v, dialect),
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    let command = if options.single_line {
        format!("{program} {}", rendered.join(" "))
    } else {
        let continuation = match dialect {
            Dialect::Posix => " \\\n  ",
            Dialect::PowerShell => " `\n  ",
        };
        format!("{program} {}", rendered.join(continuation))
    };
    Export { command, warnings }
}

fn file_warning() -> Warning {
    Warning {
        message: "the command reads files from their paths on this machine".into(),
        location: None,
    }
}

/// `user:pass` from `Basic <base64>`, if it decodes to text with a colon.
fn basic_credentials(value: &str) -> Option<String> {
    let encoded = value
        .strip_prefix("Basic ")
        .or_else(|| value.strip_prefix("basic "))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.contains(':').then_some(text)
}

fn seconds(ms: u64) -> String {
    if ms.is_multiple_of(1000) {
        (ms / 1000).to_string()
    } else {
        let s = format!("{:.3}", ms as f64 / 1000.0);
        s.trim_end_matches('0').to_string()
    }
}

/// Quotes a value for the shell; everything is single-quoted.
fn quote(value: &str, dialect: Dialect) -> String {
    match dialect {
        Dialect::Posix => format!("'{}'", value.replace('\'', "'\\''")),
        Dialect::PowerShell => format!("'{}'", value.replace('\'', "''")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(quote("it's", Dialect::Posix), r"'it'\''s'");
        assert_eq!(quote("it's", Dialect::PowerShell), "'it''s'");
        assert_eq!(quote("$HOME `x`", Dialect::Posix), "'$HOME `x`'");
    }

    #[test]
    fn seconds_formatting() {
        assert_eq!(seconds(5000), "5");
        assert_eq!(seconds(1500), "1.5");
        assert_eq!(seconds(250), "0.25");
    }

    #[test]
    fn basic_auth_becomes_user_flag() {
        assert_eq!(
            basic_credentials("Basic YWRhOnNlY3JldA==").as_deref(),
            Some("ada:secret")
        );
        assert_eq!(basic_credentials("Bearer x"), None);
        assert_eq!(basic_credentials("Basic !!!"), None);
    }
}
