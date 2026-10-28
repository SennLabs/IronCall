//! Central redaction rules (PLAN.md §6.2, §6.3).
//!
//! Everything that writes requests, responses or values to logs, history or exports
//! must go through these functions.

use secrecy::{ExposeSecret, SecretString};

/// Shown in place of a redacted value.
pub const MASK: &str = "••••";

/// Secret values shorter than this aren't masked by [`mask_values`]: replacing every
/// `a` or `42` in a text would hide far more than the secret and say nothing useful.
/// Such secrets are still never written to files (they live in the secret store).
pub const MIN_MASKED_LEN: usize = 4;

/// Replaces every occurrence of each secret value in `text` with [`MASK`]. Longer
/// values are replaced first, so a secret containing another is masked whole. Values
/// shorter than [`MIN_MASKED_LEN`] characters are ignored.
pub fn mask_values(text: &str, secrets: &[SecretString]) -> String {
    let mut values: Vec<&str> = secrets
        .iter()
        .map(|s| s.expose_secret())
        .filter(|v| v.chars().count() >= MIN_MASKED_LEN)
        .collect();
    if values.is_empty() {
        return text.to_string();
    }
    values.sort_by_key(|v| std::cmp::Reverse(v.len()));
    values.dedup();
    let mut out = text.to_string();
    for value in values {
        if out.contains(value) {
            out = out.replace(value, MASK);
        }
    }
    out
}

/// [`redact_url`] plus [`mask_values`], also masking secrets in their percent-encoded
/// forms (as `build_url` writes them into paths and queries).
pub fn redact_url_masking(url: &str, secrets: &[SecretString]) -> String {
    let mut forms: Vec<SecretString> = Vec::with_capacity(secrets.len() * 3);
    for secret in secrets {
        let v = secret.expose_secret();
        forms.push(secret.clone());
        for encoded in [
            crate::url::encode_query_component(v),
            crate::url::encode_segment(v),
        ] {
            if encoded != v {
                forms.push(SecretString::from(encoded));
            }
        }
    }
    mask_values(&redact_url(url), &forms)
}

/// Headers whose values are always treated as sensitive.
const SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
];

/// Substrings that mark a log field or variable name as sensitive.
const SENSITIVE_NAME_PARTS: &[&str] = &[
    "authorization",
    "cookie",
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "api-key",
];

/// Every header name treated as sensitive: the built-in list plus `extra`.
pub fn sensitive_header_names(extra: &[String]) -> Vec<String> {
    SENSITIVE_HEADERS
        .iter()
        .map(|h| h.to_string())
        .chain(extra.iter().cloned())
        .collect()
}

/// Whether a header's value must be redacted. `extra` holds user-configured additions.
pub fn is_sensitive_header(name: &str, extra: &[String]) -> bool {
    SENSITIVE_HEADERS.iter().any(|h| name.eq_ignore_ascii_case(h))
        || extra.iter().any(|h| name.eq_ignore_ascii_case(h))
}

/// Whether a log field or variable name looks like it holds a secret.
pub fn is_sensitive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SENSITIVE_NAME_PARTS.iter().any(|part| lower.contains(part))
}

/// Short query parameter names that carry credentials but are too generic to match as
/// substrings (`key` would match `monkey`).
const SENSITIVE_PARAM_NAMES: &[&str] = &[
    "key",
    "sig",
    "signature",
    "auth",
    "code",
    "session",
    "sessionid",
    "sid",
    "jwt",
    "pwd",
    "pass",
];

/// Substrings that mark a query parameter name as sensitive, in addition to
/// [`SENSITIVE_NAME_PARTS`] (e.g. AWS `X-Amz-Signature`, `X-Amz-Credential`).
const SENSITIVE_PARAM_PARTS: &[&str] = &["signature", "credential"];

/// Whether a URL query parameter's value must be redacted.
pub fn is_sensitive_param(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    is_sensitive_name(&lower)
        || SENSITIVE_PARAM_NAMES.contains(&lower.as_str())
        || SENSITIVE_PARAM_PARTS.iter().any(|part| lower.contains(part))
}

/// Masks the password in `user:password@` and the values of sensitive query
/// parameters, for logs, history and the recent-sends list. Everything else is kept.
pub fn redact_url(url: &str) -> String {
    let (before_fragment, fragment) = match url.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (url, None),
    };
    let (base, query) = match before_fragment.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (before_fragment, None),
    };

    let mut out = redact_userinfo(base);
    if let Some(query) = query {
        out.push('?');
        let pairs: Vec<String> = query
            .split('&')
            .map(|pair| match pair.split_once('=') {
                Some((name, value))
                    if !value.is_empty() && is_sensitive_param(&percent_decode_lossy(name)) =>
                {
                    format!("{name}={MASK}")
                }
                _ => pair.to_string(),
            })
            .collect();
        out.push_str(&pairs.join("&"));
    }
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

fn redact_userinfo(base: &str) -> String {
    let Some((scheme, rest)) = base.split_once("://") else {
        return base.to_string();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let (authority, path) = rest.split_at(authority_end);
    match authority.rsplit_once('@') {
        Some((userinfo, host)) => {
            let user = userinfo.split_once(':').map_or(userinfo, |(u, _)| u);
            let masked = if userinfo.contains(':') {
                format!("{user}:{MASK}")
            } else {
                user.to_string()
            };
            format!("{scheme}://{masked}@{host}{path}")
        }
        None => base.to_string(),
    }
}

fn percent_decode_lossy(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Best-effort masking of secrets in free text whose structure is unknown, such as panic
/// messages and frontend errors: URL credentials and sensitive query values, `Bearer` /
/// `Basic` credentials, and `name: value` / `name=value` pairs whose name looks
/// sensitive. A safety net, not a guarantee; secrets should never reach such text.
pub fn scrub(text: &str) -> String {
    use std::sync::LazyLock;

    use regex::Regex;
    static URL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"[a-zA-Z][a-zA-Z0-9+.-]*://[^\s"'<>]+"#).expect("valid regex"));
    static SCHEME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(Bearer|Basic|Token|Digest)\s+[A-Za-z0-9._~+/=-]{4,}").expect("valid regex")
    });
    static PAIR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?i)\b([A-Za-z0-9_-]*(?:authorization|cookie|password|passwd|secret|token|api[_-]?key)[A-Za-z0-9_-]*)("?\s*[:=]\s*)("[^"]*"|'[^']*'|[^\s,;&)}\]]+)"#,
        )
        .expect("valid regex")
    });
    let text = URL.replace_all(text, |c: &regex::Captures<'_>| redact_url(&c[0]));
    let text = SCHEME.replace_all(&text, |c: &regex::Captures<'_>| format!("{} {MASK}", &c[1]));
    let text = PAIR.replace_all(&text, |c: &regex::Captures<'_>| {
        if c[3].starts_with('{') || c[3] == *MASK {
            c[0].to_string()
        } else {
            format!("{}{}{MASK}", &c[1], &c[2])
        }
    });
    text.into_owned()
}

/// Returns the header value, or [`MASK`] if the header is sensitive.
pub fn header_value<'a>(name: &str, value: &'a str, extra: &[String]) -> &'a str {
    if is_sensitive_header(name, extra) {
        MASK
    } else {
        value
    }
}

/// True if `value` is exactly one `{{variable}}` reference (surrounding whitespace
/// allowed), so it carries no literal secret.
fn is_pure_reference(value: &str) -> bool {
    let v = value.trim();
    v.len() > 4 && v.starts_with("{{") && v.ends_with("}}") && !v[2..v.len() - 2].contains(['{', '}'])
}

fn mask_literal(value: &mut String) {
    if !value.is_empty() && !is_pure_reference(value) {
        *value = MASK.to_string();
    }
}

/// Masks values of sensitive headers (the denylist plus `extra`). For headers actually
/// sent or received, so `{{references}}` don't apply.
pub fn redact_headers(headers: &[(String, String)], extra: &[String]) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let value = if is_sensitive_header(name, extra) && !value.is_empty() {
                MASK.to_string()
            } else {
                value.clone()
            };
            (name.clone(), value)
        })
        .collect()
}

/// A copy of an authored request that is safe to store in history (PLAN.md §6.3).
///
/// Secrets stored as variables never appear in a request file, so only literal values
/// typed into sensitive places are masked: denylisted headers (plus `extra`), sensitive
/// query params and form fields (by name), request variables with sensitive names, and
/// auth credentials (password, token, API key value). Values that are exactly one
/// `{{variable}}` reference are kept, so the request still works when reopened.
/// Bodies are kept as they are: their content can't be classified reliably.
pub fn redact_request_file(
    req: &crate::model::RequestFile,
    extra_sensitive: &[String],
) -> crate::model::RequestFile {
    use crate::model::{Body, Variable};

    let mut out = req.clone();
    for h in &mut out.headers {
        if is_sensitive_header(&h.name, extra_sensitive) {
            mask_literal(&mut h.value);
        }
    }
    for p in &mut out.params {
        if is_sensitive_param(&p.name) {
            mask_literal(&mut p.value);
        }
    }
    if let Body::FormUrlencoded { fields, .. } = &mut out.body {
        for f in fields {
            if is_sensitive_name(&f.name) || is_sensitive_param(&f.name) {
                mask_literal(&mut f.value);
            }
        }
    }
    for (name, var) in &mut out.vars {
        if is_sensitive_name(name)
            && let Variable::Plain(value) = var
        {
            mask_literal(value);
        }
    }
    out.auth = redact_auth(&out.auth);
    if out.url.contains('@') {
        // `user:password@host` written literally in the URL.
        out.url = redact_url(&out.url);
    }
    out
}

/// Auth with literal credentials (password, token, API key value) masked; values that are
/// exactly one `{{variable}}` reference are kept.
pub fn redact_auth(auth: &crate::model::Auth) -> crate::model::Auth {
    use crate::model::Auth;
    let mut out = auth.clone();
    match &mut out {
        Auth::Basic { password, .. } => mask_literal(password),
        Auth::Bearer { token, .. } => mask_literal(token),
        Auth::ApiKey { value, .. } => mask_literal(value),
        Auth::Inherit | Auth::None { .. } => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secrets(values: &[&str]) -> Vec<SecretString> {
        values.iter().map(|v| SecretString::from(v.to_string())).collect()
    }

    #[test]
    fn mask_values_replaces_every_occurrence() {
        let s = secrets(&["hunter2!"]);
        assert_eq!(
            mask_values("a hunter2! b hunter2!", &s),
            format!("a {MASK} b {MASK}")
        );
        assert_eq!(mask_values("nothing here", &s), "nothing here");
    }

    #[test]
    fn mask_values_prefers_longest_and_skips_short_or_empty() {
        let s = secrets(&["abcd", "abcdefgh", "", "xy"]);
        assert_eq!(mask_values("abcdefgh abcd xy", &s), format!("{MASK} {MASK} xy"));
        assert_eq!(mask_values("text", &[]), "text");
    }

    #[test]
    fn mask_values_counts_characters_not_bytes() {
        // Four characters (but more bytes): masked.
        assert_eq!(
            mask_values("pw: héé!", &secrets(&["héé!"])),
            format!("pw: {MASK}")
        );
        // Three multi-byte characters: below the limit.
        assert_eq!(mask_values("ééé", &secrets(&["ééé"])), "ééé");
    }

    #[test]
    fn redact_url_masking_handles_encoded_secrets() {
        let s = secrets(&["a b&c", "path/seg"]);
        let url = "https://x.example/files/path%2Fseg?q=a%20b%26c&plain=a b&c";
        let out = redact_url_masking(url, &s);
        assert!(!out.contains("a%20b%26c") && !out.contains("path%2Fseg"), "{out}");
        assert!(out.contains(MASK));
    }

    #[test]
    fn denylisted_headers_are_sensitive_case_insensitively() {
        for name in [
            "Authorization",
            "COOKIE",
            "set-cookie",
            "X-Api-Key",
            "Proxy-Authorization",
        ] {
            assert!(is_sensitive_header(name, &[]), "{name}");
        }
        assert!(!is_sensitive_header("Content-Type", &[]));
    }

    #[test]
    fn user_configured_headers_are_sensitive() {
        let extra = vec!["X-Tenant-Key".to_string()];
        assert!(is_sensitive_header("x-tenant-key", &extra));
    }

    #[test]
    fn scrub_masks_secrets_in_free_text() {
        let text = concat!(
            "called `Result::unwrap()` on an `Err` value: Request { url: \"https://u:pw@api.example/x?token=t1&page=2\", ",
            "headers: [(\"Authorization\", \"Bearer abc.def.ghi\")], password=hunter2, \"api_key\": \"k-123\", ",
            "Cookie: sid=s3cr3t; user_id=42, status=500 }",
        );
        let out = scrub(text);
        for secret in ["pw@", "t1", "abc.def.ghi", "hunter2", "k-123", "s3cr3t"] {
            assert!(!out.contains(secret), "{secret} in {out}");
        }
        for kept in ["page=2", "user_id=42", "status=500", "api.example"] {
            assert!(out.contains(kept), "{kept} missing from {out}");
        }
        assert_eq!(
            scrub("index out of bounds: the len is 3"),
            "index out of bounds: the len is 3"
        );
        assert_eq!(
            scrub("token: {{authToken}}"),
            "token: {{authToken}}",
            "references aren't secrets"
        );
    }

    #[test]
    fn header_value_masks_only_sensitive_headers() {
        assert_eq!(header_value("Authorization", "Bearer abc", &[]), MASK);
        assert_eq!(
            header_value("Accept", "application/json", &[]),
            "application/json"
        );
    }

    #[test]
    fn redact_url_masks_sensitive_query_values() {
        assert_eq!(
            redact_url("https://a.example/x?q=rust&api_key=abc&token=t1&page=2"),
            format!("https://a.example/x?q=rust&api_key={MASK}&token={MASK}&page=2")
        );
        assert_eq!(
            redact_url("https://s3.example/o?X-Amz-Credential=AK&X-Amz-Signature=ff&key=k&monkey=1"),
            format!(
                "https://s3.example/o?X-Amz-Credential={MASK}&X-Amz-Signature={MASK}&key={MASK}&monkey=1"
            )
        );
    }

    #[test]
    fn redact_url_masks_password_in_userinfo() {
        assert_eq!(
            redact_url("https://ada:s3cret@a.example/x"),
            format!("https://ada:{MASK}@a.example/x")
        );
        assert_eq!(redact_url("https://ada@a.example/x"), "https://ada@a.example/x");
    }

    #[test]
    fn redact_url_keeps_other_parts() {
        let url = "https://a.example:8443/a/b?x=1&flag&empty_token=#frag";
        assert_eq!(redact_url(url), url);
        assert_eq!(
            redact_url("https://a.example/?access%5Ftoken=abc"),
            format!("https://a.example/?access%5Ftoken={MASK}")
        );
    }

    #[test]
    fn sensitive_names_are_detected() {
        for name in [
            "password",
            "accessToken",
            "client_secret",
            "apiKey",
            "auth_cookie",
        ] {
            assert!(is_sensitive_name(name), "{name}");
        }
        assert!(!is_sensitive_name("user_id"));
    }

    #[test]
    fn redact_headers_masks_sensitive_values() {
        let headers = vec![
            ("Authorization".to_string(), "Bearer abc".to_string()),
            ("Accept".to_string(), "*/*".to_string()),
            ("X-Tenant".to_string(), "t1".to_string()),
        ];
        let out = redact_headers(&headers, &["x-tenant".to_string()]);
        assert_eq!(out[0].1, MASK);
        assert_eq!(out[1].1, "*/*");
        assert_eq!(out[2].1, MASK);
    }

    #[test]
    fn redact_request_file_masks_literals_and_keeps_references() {
        use crate::model::{Auth, Body, KeyValue, RequestFile, Variable};

        let mut req = RequestFile::new("r");
        req.url = "https://api.example.com/x".into();
        req.headers = vec![
            KeyValue::new("Authorization", "Bearer literal-token"),
            KeyValue::new("Cookie", "{{session}}"),
            KeyValue::new("Accept", "application/json"),
        ];
        req.params = vec![KeyValue::new("api_key", "k-123"), KeyValue::new("page", "2")];
        req.body = Body::FormUrlencoded {
            fields: vec![KeyValue::new("password", "hunter2"), KeyValue::new("user", "ada")],
            extra: Default::default(),
        };
        req.vars
            .insert("accessToken".into(), Variable::Plain("tok".into()));
        req.vars.insert("region".into(), Variable::Plain("eu".into()));
        req.auth = Auth::Basic {
            username: "ada".into(),
            password: "pw-literal".into(),
            extra: Default::default(),
        };

        let out = redact_request_file(&req, &[]);
        assert_eq!(out.headers[0].value, MASK);
        assert_eq!(out.headers[1].value, "{{session}}", "pure reference kept");
        assert_eq!(out.headers[2].value, "application/json");
        assert_eq!(out.params[0].value, MASK);
        assert_eq!(out.params[1].value, "2");
        match &out.body {
            Body::FormUrlencoded { fields, .. } => {
                assert_eq!(fields[0].value, MASK);
                assert_eq!(fields[1].value, "ada");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(out.vars["accessToken"], Variable::Plain(MASK.into()));
        assert_eq!(out.vars["region"], Variable::Plain("eu".into()));
        match &out.auth {
            Auth::Basic {
                username, password, ..
            } => {
                assert_eq!(username, "ada");
                assert_eq!(password, MASK);
            }
            other => panic!("{other:?}"),
        }

        req.auth = Auth::Bearer {
            token: "{{token}}".into(),
            extra: Default::default(),
        };
        match redact_request_file(&req, &[]).auth {
            Auth::Bearer { token, .. } => assert_eq!(token, "{{token}}"),
            other => panic!("{other:?}"),
        }
        // A reference mixed with literal text still masks.
        req.auth = Auth::Bearer {
            token: "abc{{token}}".into(),
            extra: Default::default(),
        };
        match redact_request_file(&req, &[]).auth {
            Auth::Bearer { token, .. } => assert_eq!(token, MASK),
            other => panic!("{other:?}"),
        }
    }
}
