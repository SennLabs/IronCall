//! cURL command → [`RequestFile`].

use std::path::Path;

use ironcall_core::model::{
    Auth, Body, Extra, HttpVersion, KeyValue, MultipartKind, MultipartPart, RequestFile,
};

use super::tokenize;
use super::{ParseError, Parsed, Warning, percent_decode};

/// What a flag does, by its long name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flag {
    Request,
    Header,
    Data,
    DataRaw,
    DataBinary,
    DataUrlencode,
    Json,
    Form,
    FormString,
    User,
    Cookie,
    Insecure,
    Location,
    MaxRedirs,
    Url,
    Get,
    UserAgent,
    Referer,
    OAuth2Bearer,
    MaxTime,
    Http10,
    Http11,
    Http2,
    Http2PriorKnowledge,
    Http3,
    Head,
    /// Accepted and ignored without a warning (output and progress options that don't
    /// change the request, and `--compressed`, which IronCall always does).
    Quiet,
    /// Ignored with a warning: the flag (and its value) can't be represented.
    Unsupported(&'static str),
}

/// Returns (flag, takes a value) for a long flag name.
fn long(name: &str) -> Option<(Flag, bool)> {
    use Flag::*;
    const OUTPUT: &str = "output options don't apply in IronCall";
    const TLS: &str =
        "client certificates and TLS options aren't supported yet (Settings → Network has custom CAs)";
    const PROXY: &str = "proxy options are ignored; IronCall uses Settings → Network";
    const AUTH: &str = "only Basic, Bearer and API key auth are supported";
    Some(match name {
        "request" => (Request, true),
        "header" => (Header, true),
        "data" | "data-ascii" => (Data, true),
        "data-raw" => (DataRaw, true),
        "data-binary" => (DataBinary, true),
        "data-urlencode" => (DataUrlencode, true),
        "json" => (Json, true),
        "form" => (Form, true),
        "form-string" => (FormString, true),
        "user" => (User, true),
        "cookie" => (Cookie, true),
        "insecure" => (Insecure, false),
        "location" | "location-trusted" => (Location, false),
        "max-redirs" => (MaxRedirs, true),
        "url" => (Url, true),
        "get" => (Get, false),
        "user-agent" => (UserAgent, true),
        "referer" => (Referer, true),
        "oauth2-bearer" => (OAuth2Bearer, true),
        "max-time" => (MaxTime, true),
        "http1.0" => (Http10, false),
        "http1.1" => (Http11, false),
        "http2" => (Http2, false),
        "http2-prior-knowledge" => (Http2PriorKnowledge, false),
        "http3" | "http3-only" => (Http3, false),
        "head" => (Head, false),
        "compressed" | "silent" | "show-error" | "verbose" | "progress-bar" | "no-progress-meter"
        | "include" | "fail" | "fail-with-body" | "no-buffer" | "globoff" | "basic" | "no-keepalive"
        | "styled-output" | "no-styled-output" | "raw" | "tr-encoding" | "ssl-no-revoke" => (Quiet, false),
        "output" | "dump-header" | "write-out" | "trace" | "trace-ascii" | "stderr" | "output-dir"
        | "etag-save" | "etag-compare" => (Unsupported(OUTPUT), true),
        "remote-name" | "remote-name-all" | "remote-header-name" | "create-dirs" | "trace-time" => {
            (Unsupported(OUTPUT), false)
        }
        "cert" | "key" | "cacert" | "capath" | "cert-type" | "key-type" | "pass" | "ciphers" | "tls-max"
        | "pinnedpubkey" | "crlfile" => (Unsupported(TLS), true),
        "tlsv1" | "tlsv1.0" | "tlsv1.1" | "tlsv1.2" | "tlsv1.3" | "sslv2" | "sslv3" | "cert-status" => {
            (Unsupported(TLS), false)
        }
        "proxy" | "proxy-user" | "proxy-header" | "noproxy" | "socks5" | "socks5-hostname" | "preproxy" => {
            (Unsupported(PROXY), true)
        }
        "proxytunnel" => (Unsupported(PROXY), false),
        "digest" | "ntlm" | "negotiate" | "anyauth" => (Unsupported(AUTH), false),
        "aws-sigv4" => (Unsupported(AUTH), true),
        "connect-timeout" => (Unsupported("only an overall timeout (-m) is supported"), true),
        "cookie-jar" => (
            Unsupported(
                "saving cookies to a file isn't supported; IronCall keeps them in the workspace cookie jar",
            ),
            true,
        ),
        "retry" | "retry-delay" | "retry-max-time" | "limit-rate" | "speed-limit" | "speed-time"
        | "range" | "time-cond" | "interface" | "resolve" | "connect-to" | "dns-servers" | "local-port"
        | "config" | "upload-file" | "variable" | "expand-url" | "expand-data" | "expand-header" => {
            (Unsupported("not supported; ignored"), true)
        }
        "ipv4"
        | "ipv6"
        | "netrc"
        | "netrc-optional"
        | "junk-session-cookies"
        | "parallel"
        | "path-as-is"
        | "post301"
        | "post302"
        | "post303"
        | "ignore-content-length" => (Unsupported("not supported; ignored"), false),
        _ => return None,
    })
}

fn short(c: char) -> Option<&'static str> {
    Some(match c {
        'X' => "request",
        'H' => "header",
        'd' => "data",
        'F' => "form",
        'u' => "user",
        'b' => "cookie",
        'k' => "insecure",
        'L' => "location",
        'G' => "get",
        'A' => "user-agent",
        'e' => "referer",
        'm' => "max-time",
        'I' => "head",
        '0' => "http1.0",
        's' => "silent",
        'S' => "show-error",
        'v' => "verbose",
        'i' => "include",
        '#' => "progress-bar",
        'f' => "fail",
        'N' => "no-buffer",
        'g' => "globoff",
        'o' => "output",
        'O' => "remote-name",
        'J' => "remote-header-name",
        'D' => "dump-header",
        'w' => "write-out",
        'E' => "cert",
        'x' => "proxy",
        'U' => "proxy-user",
        'p' => "proxytunnel",
        'c' => "cookie-jar",
        'T' => "upload-file",
        'r' => "range",
        'z' => "time-cond",
        'K' => "config",
        'Y' => "speed-limit",
        'y' => "speed-time",
        '4' => "ipv4",
        '6' => "ipv6",
        'n' => "netrc",
        'j' => "junk-session-cookies",
        'Z' => "parallel",
        '1' => "tlsv1",
        _ => return None,
    })
}

/// One `-d`-style argument, before joining.
enum Data {
    /// Sent as written.
    Text(String),
    /// `@file`: read at send time.
    File { path: String, flag: &'static str },
}

#[derive(Default)]
struct State {
    method: Option<String>,
    urls: Vec<String>,
    headers: Vec<(String, String)>,
    data: Vec<Data>,
    form: Vec<MultipartPart>,
    get: bool,
    head: bool,
    json: bool,
    user: Option<String>,
    bearer: Option<String>,
    cookies: Vec<String>,
    insecure: bool,
    max_redirs: Option<u32>,
    timeout_ms: Option<u64>,
    http_version: Option<HttpVersion>,
    warnings: Vec<Warning>,
    unknown: Vec<String>,
}

/// Parses a cURL command. See the module docs for what is supported.
pub fn parse(text: &str) -> Result<Parsed, ParseError> {
    let text = text.trim();
    let text = text.strip_prefix("$ ").unwrap_or(text);
    if text.lines().any(|l| l.trim_end().ends_with(" `")) {
        return Err(ParseError::Invalid(
            "PowerShell cURL commands can't be imported yet; copy it as bash or cmd instead".into(),
        ));
    }
    let shell = tokenize::detect(text);
    let args = tokenize::tokenize(text, shell)?;
    let mut args = args.into_iter();
    match args.next() {
        Some(first) if is_curl(&first) => {}
        _ => return Err(ParseError::NotCurl),
    }

    let mut st = State::default();
    let mut options_done = false;
    while let Some(arg) = args.next() {
        if options_done || arg == "-" || !arg.starts_with('-') {
            st.urls.push(arg);
            continue;
        }
        if arg == "--" {
            options_done = true;
            continue;
        }
        if let Some(name) = arg.strip_prefix("--") {
            let (name, inline) = match name.split_once('=') {
                Some((n, v)) if long(n).is_some_and(|(_, takes)| takes) => (n, Some(v.to_string())),
                _ => (name, None),
            };
            match long(name) {
                Some((flag, takes)) => {
                    let value = if takes {
                        Some(match inline {
                            Some(v) => v,
                            None => args.next().ok_or_else(|| ParseError::MissingValue(arg.clone()))?,
                        })
                    } else {
                        None
                    };
                    apply(&mut st, flag, &format!("--{name}"), value)?;
                }
                None => st.unknown.push(arg),
            }
            continue;
        }
        // A cluster of short flags: `-sSL`, `-XPOST`, `-HAccept:x`.
        let cluster: Vec<char> = arg[1..].chars().collect();
        let mut i = 0;
        while i < cluster.len() {
            let c = cluster[i];
            let Some(name) = short(c) else {
                st.unknown.push(format!("-{c}"));
                i += 1;
                continue;
            };
            let (flag, takes) = long(name).expect("every short flag has a long form");
            if takes {
                let rest: String = cluster[i + 1..].iter().collect();
                let value = if rest.is_empty() {
                    args.next()
                        .ok_or_else(|| ParseError::MissingValue(format!("-{c}")))?
                } else {
                    rest
                };
                apply(&mut st, flag, &format!("-{c}"), Some(value))?;
                break;
            }
            apply(&mut st, flag, &format!("-{c}"), None)?;
            i += 1;
        }
    }
    finish(st)
}

fn is_curl(arg: &str) -> bool {
    let name = arg.rsplit(['/', '\\']).next().unwrap_or(arg).to_ascii_lowercase();
    name == "curl" || name == "curl.exe"
}

fn apply(st: &mut State, flag: Flag, shown: &str, value: Option<String>) -> Result<(), ParseError> {
    let value = || value.clone().unwrap_or_default();
    match flag {
        Flag::Request => st.method = Some(value()),
        Flag::Header => header(st, &value()),
        Flag::Data => st.data.push(data_arg(value(), "-d")),
        Flag::DataRaw => st.data.push(Data::Text(value())),
        Flag::DataBinary => st.data.push(data_arg(value(), "--data-binary")),
        Flag::DataUrlencode => {
            if let Some(piece) = urlencode_arg(st, &value()) {
                st.data.push(Data::Text(piece));
            }
        }
        Flag::Json => {
            st.json = true;
            st.data.push(data_arg(value(), "--json"));
        }
        Flag::Form => form(st, &value(), false),
        Flag::FormString => form(st, &value(), true),
        Flag::User => st.user = Some(value()),
        Flag::Cookie => {
            let v = value();
            if v.contains('=') {
                st.cookies.push(v);
            } else {
                st.warnings.push(Warning::new(
                    "reading cookies from a file isn't supported; the request uses the workspace cookie jar",
                    shown,
                ));
            }
        }
        Flag::Insecure => st.insecure = true,
        Flag::Location | Flag::Quiet => {}
        Flag::MaxRedirs => {
            st.max_redirs = Some(
                value()
                    .parse()
                    .map_err(|_| ParseError::Invalid(format!("{shown} needs a number")))?,
            );
        }
        Flag::Url => st.urls.push(value()),
        Flag::Get => st.get = true,
        Flag::UserAgent => st.headers.push(("User-Agent".into(), value())),
        Flag::Referer => {
            let v = value();
            let v = v.strip_suffix(";auto").unwrap_or(&v).to_string();
            st.headers.push(("Referer".into(), v));
        }
        Flag::OAuth2Bearer => st.bearer = Some(value()),
        Flag::MaxTime => {
            let secs: f64 = value()
                .parse()
                .map_err(|_| ParseError::Invalid(format!("{shown} needs a number of seconds")))?;
            st.timeout_ms = Some((secs * 1000.0).round().max(1.0) as u64);
        }
        Flag::Http10 => {
            st.http_version = Some(HttpVersion::Http1);
            st.warnings
                .push(Warning::new("HTTP/1.0 isn't supported; using HTTP/1.1", shown));
        }
        Flag::Http11 => st.http_version = Some(HttpVersion::Http1),
        Flag::Http2 => st.http_version = Some(HttpVersion::Http2),
        Flag::Http2PriorKnowledge => {
            st.http_version = Some(HttpVersion::Http2);
            st.warnings.push(Warning::new(
                "HTTP/2 without TLS (prior knowledge) isn't supported; HTTP/2 is negotiated over TLS",
                shown,
            ));
        }
        Flag::Http3 => st.warnings.push(Warning::new(
            "HTTP/3 isn't supported yet; using the default",
            shown,
        )),
        Flag::Head => st.head = true,
        Flag::Unsupported(why) => st.warnings.push(Warning::new(format!("{shown}: {why}"), shown)),
    }
    Ok(())
}

fn header(st: &mut State, raw: &str) {
    if raw.starts_with('@') {
        st.warnings.push(Warning::new(
            "reading headers from a file isn't supported; ignored",
            "-H",
        ));
        return;
    }
    match raw.split_once(':') {
        Some((name, value)) => {
            let (name, value) = (name.trim(), value.trim());
            if name.is_empty() {
                st.warnings.push(Warning::new(
                    format!("ignored a header with no name: {raw}"),
                    "-H",
                ));
            } else if value.is_empty() {
                st.warnings.push(Warning::new(
                    format!("`{name}:` removes one of curl's own headers; ignored"),
                    "-H",
                ));
            } else {
                st.headers.push((name.to_string(), value.to_string()));
            }
        }
        // `Name;` sends the header with an empty value.
        None => match raw.trim().strip_suffix(';') {
            Some(name) if !name.trim().is_empty() => {
                st.headers.push((name.trim().to_string(), String::new()))
            }
            _ => st
                .warnings
                .push(Warning::new(format!("ignored an invalid header: {raw}"), "-H")),
        },
    }
}

fn data_arg(value: String, flag: &'static str) -> Data {
    match value.strip_prefix('@') {
        Some(path) => Data::File {
            path: path.to_string(),
            flag,
        },
        None => Data::Text(value),
    }
}

/// `--data-urlencode` forms: `content`, `=content`, `name=content`, `@file`, `name@file`.
fn urlencode_arg(st: &mut State, value: &str) -> Option<String> {
    if let Some((name, content)) = value.split_once('=') {
        return Some(if name.is_empty() {
            urlencode(content)
        } else {
            format!("{name}={}", urlencode(content))
        });
    }
    if value.contains('@') {
        st.warnings.push(Warning::new(
            "--data-urlencode with a file isn't supported; that part was skipped",
            "--data-urlencode",
        ));
        return None;
    }
    Some(urlencode(value))
}

/// curl's URL encoding for `--data-urlencode`: everything except `A-Za-z0-9-._~`.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn form(st: &mut State, raw: &str, literal: bool) {
    let flag = if literal { "--form-string" } else { "-F" };
    let Some((name, value)) = raw.split_once('=') else {
        st.warnings.push(Warning::new(
            format!("ignored a form field without `=`: {raw}"),
            flag,
        ));
        return;
    };
    let mut part = MultipartPart {
        name: name.to_string(),
        kind: MultipartKind::Text,
        value: None,
        path: None,
        content_type: None,
        enabled: true,
        extra: Extra::default(),
    };
    if literal {
        part.value = Some(value.to_string());
        st.form.push(part);
        return;
    }
    let (file, from_text) = match value.strip_prefix('@') {
        Some(rest) => (Some(rest), false),
        None => (value.strip_prefix('<'), true),
    };
    match file {
        Some(spec) => {
            let mut pieces = spec.split(';');
            let path = pieces.next().unwrap_or_default().trim_matches('"').to_string();
            for modifier in pieces {
                let (key, val) = modifier.split_once('=').unwrap_or((modifier, ""));
                match key.trim() {
                    "type" => part.content_type = Some(val.trim().to_string()),
                    "filename" => st.warnings.push(Warning::new(
                        "a custom file name isn't supported; the file's own name is sent",
                        flag,
                    )),
                    other => st.warnings.push(Warning::new(
                        format!("form modifier `{other}` isn't supported"),
                        flag,
                    )),
                }
            }
            if from_text {
                st.warnings.push(Warning::new(
                    format!("`{name}=<file` sends the file's contents as text; imported as a file part"),
                    flag,
                ));
            }
            warn_missing_file(st, &path, flag);
            part.kind = MultipartKind::File;
            part.path = Some(path);
        }
        None => match value.split_once(";type=") {
            Some((v, ct)) => {
                part.value = Some(v.to_string());
                part.content_type = Some(ct.to_string());
            }
            None => part.value = Some(value.to_string()),
        },
    }
    st.form.push(part);
}

fn warn_missing_file(st: &mut State, path: &str, flag: &str) {
    if path.starts_with('~') {
        st.warnings.push(Warning::new(
            format!("`{path}` uses `~`; pick the file again in the body editor"),
            flag,
        ));
    } else if !Path::new(path).exists() {
        st.warnings.push(Warning::new(
            format!(
                "`{path}` doesn't exist relative to the current folder; pick it again in the body editor"
            ),
            flag,
        ));
    }
}

fn finish(mut st: State) -> Result<Parsed, ParseError> {
    if !st.unknown.is_empty() {
        let list = st.unknown.join(", ");
        st.warnings.push(Warning {
            message: format!("unknown options ignored: {list}"),
            location: Some(list),
        });
    }
    let url = pick_url(&mut st)?;

    // Data: text pieces join with `&`; a file body can't be combined with them.
    let mut texts = Vec::new();
    let mut file: Option<(String, &'static str)> = None;
    for d in std::mem::take(&mut st.data) {
        match d {
            Data::Text(t) => texts.push(t),
            Data::File { path, flag } if path == "-" => {
                st.warnings.push(Warning::new(
                    "reading the body from stdin isn't supported; skipped",
                    flag,
                ));
            }
            Data::File { path, flag } => {
                if file.is_some() {
                    st.warnings.push(Warning::new(
                        "only one file body is supported; extra ones skipped",
                        flag,
                    ));
                } else {
                    file = Some((path, flag));
                }
            }
        }
    }
    if file.is_some() && !texts.is_empty() {
        st.warnings.push(Warning::new(
            "a file body can't be combined with inline data; the inline data was used",
            "-d",
        ));
        file = None;
    }
    let data = (!texts.is_empty()).then(|| texts.join("&"));
    let has_body = data.is_some() || file.is_some();

    let method = match st.method.take() {
        Some(m) => m.to_ascii_uppercase(),
        None if st.head => "HEAD".into(),
        None if st.get => "GET".into(),
        None if has_body || !st.form.is_empty() => "POST".into(),
        None => "GET".into(),
    };

    let (base, mut query) = split_url(&url);
    if st.get {
        if let Some(d) = &data {
            query.push_str(if query.is_empty() { "" } else { "&" });
            query.push_str(d);
        }
        if file.is_some() || !st.form.is_empty() {
            st.warnings.push(Warning::new(
                "-G only moves inline data into the query; file and form data skipped",
                "-G",
            ));
        }
    }

    if st.json {
        add_if_missing(&mut st.headers, "Content-Type", "application/json");
        add_if_missing(&mut st.headers, "Accept", "application/json");
    }
    if !st.cookies.is_empty() {
        if st.headers.iter().any(|(n, _)| n.eq_ignore_ascii_case("cookie")) {
            st.warnings.push(Warning::new(
                "-b was ignored because a Cookie header is also set",
                "-b",
            ));
        } else {
            st.headers.push(("Cookie".into(), st.cookies.join("; ")));
        }
    }

    let content_type = st
        .headers
        .iter()
        .rev()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.to_ascii_lowercase());

    let body = if !st.form.is_empty() && !st.get {
        if has_body {
            st.warnings.push(Warning::new(
                "curl can't send -F and -d together; the form was used",
                "-F",
            ));
        }
        Body::Multipart {
            parts: std::mem::take(&mut st.form),
            extra: Extra::default(),
        }
    } else if st.get {
        Body::None
    } else if let Some((path, flag)) = file {
        if content_type.is_none() {
            implicit_form_type(&mut st);
        }
        if flag == "-d" {
            st.warnings.push(Warning::new(
                "curl -d @file strips line breaks from the file; IronCall sends the file as is (like --data-binary)",
                flag,
            ));
        }
        warn_missing_file(&mut st, &path, flag);
        Body::BinaryFile {
            path,
            content_type: None,
            extra: Extra::default(),
        }
    } else if let Some(data) = data {
        text_body(&mut st, content_type.as_deref(), data)
    } else {
        Body::None
    };

    let path = base
        .split_once("://")
        .map(|(_, rest)| rest.find('/').map_or("/", |i| &rest[i..]))
        .unwrap_or("/");
    let mut request = RequestFile::new(format!("{method} {path}"));
    request.method = method;
    request.url = base;
    request.params = parse_query(&query);
    request.headers = st
        .headers
        .iter()
        .map(|(n, v)| KeyValue::new(n.clone(), v.clone()))
        .collect();
    request.body = body;
    request.auth = auth(&mut st);
    let settings = &mut request.settings;
    if st.insecure {
        settings.verify_tls = Some(false);
    }
    settings.max_redirects = st.max_redirs;
    settings.timeout_ms = st.timeout_ms;
    settings.http_version = st.http_version;
    Ok(Parsed {
        request,
        warnings: st.warnings,
    })
}

/// Picks the URL. Unknown flags that take a value can leave stray arguments, so a
/// candidate with a scheme wins over one without.
fn pick_url(st: &mut State) -> Result<String, ParseError> {
    let urls = std::mem::take(&mut st.urls);
    let chosen = urls
        .iter()
        .find(|u| u.contains("://"))
        .or_else(|| urls.first())
        .cloned()
        .ok_or(ParseError::NoUrl)?;
    if urls.len() > 1 {
        st.warnings.push(Warning {
            message: format!("more than one URL or stray argument; imported {chosen} and ignored the rest"),
            location: None,
        });
    }
    let chosen = chosen.trim().to_string();
    Ok(if chosen.contains("://") || chosen.starts_with("{{") {
        chosen
    } else {
        // curl assumes http:// for URLs without a scheme.
        format!("http://{chosen}")
    })
}

/// Splits off the query (and drops any fragment).
fn split_url(url: &str) -> (String, String) {
    let url = url.split_once('#').map_or(url, |(before, _)| before);
    match url.split_once('?') {
        Some((base, query)) => (base.to_string(), query.to_string()),
        None => (url.to_string(), String::new()),
    }
}

fn parse_query(query: &str) -> Vec<KeyValue> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|piece| {
            let (name, value) = piece.split_once('=').unwrap_or((piece, ""));
            KeyValue::new(percent_decode(name, true), percent_decode(value, true))
        })
        .collect()
}

fn add_if_missing(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    if !headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
        headers.push((name.to_string(), value.to_string()));
    }
}

/// curl sends `-d` data as `application/x-www-form-urlencoded` when no Content-Type is
/// given. Keep that explicit so the request behaves the same.
fn implicit_form_type(st: &mut State) {
    st.headers
        .push(("Content-Type".into(), "application/x-www-form-urlencoded".into()));
    st.warnings.push(Warning::new(
        "no Content-Type was given, so curl sends this body as application/x-www-form-urlencoded; kept as is",
        "-d",
    ));
}

/// `a=1&b=2` (every piece has a name and `=`).
fn is_form_like(data: &str) -> bool {
    !data.is_empty()
        && data.split('&').all(|p| {
            p.split_once('=')
                .is_some_and(|(n, _)| !n.is_empty() && !n.contains(char::is_whitespace))
        })
        && !data.contains('\n')
}

fn text_body(st: &mut State, content_type: Option<&str>, data: String) -> Body {
    let extra = Extra::default();
    match content_type {
        None if is_form_like(&data) => form_body(&data),
        None => {
            implicit_form_type(st);
            Body::Text { content: data, extra }
        }
        Some(ct) if ct.contains("json") => Body::Json { content: data, extra },
        Some(ct) if ct.contains("xml") => Body::Xml { content: data, extra },
        Some(ct) if ct.contains("x-www-form-urlencoded") && is_form_like(&data) => form_body(&data),
        Some(_) => Body::Text { content: data, extra },
    }
}

fn form_body(data: &str) -> Body {
    Body::FormUrlencoded {
        fields: parse_query(data),
        extra: Extra::default(),
    }
}

fn auth(st: &mut State) -> Auth {
    let extra = Extra::default();
    match (st.user.take(), st.bearer.take()) {
        (Some(user), bearer) => {
            if bearer.is_some() {
                st.warnings.push(Warning::new(
                    "both -u and --oauth2-bearer were given; using Basic auth",
                    "--oauth2-bearer",
                ));
            }
            let (username, password) = match user.split_once(':') {
                Some((u, p)) => (u.to_string(), p.to_string()),
                None => {
                    st.warnings.push(Warning::new(
                        "-u has no password (curl would ask for it); enter it in the Auth tab",
                        "-u",
                    ));
                    (user, String::new())
                }
            };
            Auth::Basic {
                username,
                password,
                extra,
            }
        }
        (None, Some(token)) => Auth::Bearer { token, extra },
        // No credentials in the command: inherit, so folder or collection auth applies
        // when it's saved into a workspace (a scratch request has none to inherit).
        (None, None) => Auth::default(),
    }
}
