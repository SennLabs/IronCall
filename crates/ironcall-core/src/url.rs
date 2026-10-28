//! Building the final request URL from a request file (PLAN.md §5).
//!
//! A request file stores the URL without its query string (`url`), the query rows
//! (`params`) and the path parameter values (`pathParams`). [`build_url`] combines them
//! into the exact URL that is sent.
//!
//! # Encoding rules (when `encode` is true)
//! - **Spaces become `%20`** everywhere (path and query). `+` in a query name or value is
//!   encoded as `%2B`, so it is never read as a space. This matches what
//!   `curl --get --data-urlencode` users expect and round-trips through curl unchanged.
//! - **Valid `%XX` sequences are kept as they are**, so already-encoded input isn't
//!   encoded twice. A `%` not followed by two hex digits becomes `%25`.
//! - **Path**: characters valid in a path segment (RFC 3986 `pchar`) are kept; `/`
//!   separates segments in the URL itself. Path param *values* are a single segment, so
//!   a `/` inside a value is encoded as `%2F`.
//! - **Query**: names and values keep RFC 3986 query characters, except `&`, `=`, `+`
//!   and `#`, which are encoded because they would change the meaning of the query.
//! - **`{{variables}}`**: braces are never encoded, so Phase 3 can resolve variables
//!   before encoding. Until then a `{{var}}` reaches the URL as-is.
//! - Fragments (`#...`) are never sent and are dropped.
//!
//! With `encode` false (`settings.encodeUrl: false`), names and values are inserted
//! exactly as typed.

use crate::model::{KeyValue, PathParam};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlError {
    #[error("enter a URL")]
    Empty,
    #[error("the URL needs a scheme, e.g. https://{0}")]
    MissingScheme(String),
    #[error("unsupported scheme `{0}`: only http and https are supported")]
    UnsupportedScheme(String),
    #[error("the URL has no host")]
    MissingHost,
    #[error("the host contains invalid characters: `{0}`")]
    InvalidHost(String),
    /// Holds the reference as written, e.g. `{{baseUrl}}`.
    #[error("variable `{0}` is not defined in any scope")]
    UnresolvedVariable(String),
    #[error("path parameter `:{0}` has no value")]
    MissingPathParam(String),
}

/// Names of path parameters in `url`, in order of first appearance, without duplicates.
///
/// A path parameter is a whole path segment written as `:name` or `{name}` (the OpenAPI
/// form, normalised to the same name). `{{var}}` segments are variables, not params.
pub fn path_param_names(url: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for segment in path_part(url).split('/') {
        if let Some(name) = param_name(segment)
            && !names.iter().any(|n| n == name)
        {
            names.push(name.to_string());
        }
    }
    names
}

/// Combines a request file's URL, path params and enabled query params into the URL
/// that is sent. See the module docs for the encoding rules.
pub fn build_url(
    url: &str,
    path_params: &[PathParam],
    params: &[KeyValue],
    encode: bool,
) -> Result<String, UrlError> {
    let url = url.trim();
    if url.is_empty() {
        return Err(UrlError::Empty);
    }
    let url = url.split_once('#').map_or(url, |(before, _)| before);

    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (scheme, rest),
        None if url.starts_with("{{") => return Err(UrlError::UnresolvedVariable(first_variable(url))),
        None => return Err(UrlError::MissingScheme(url.to_string())),
    };
    if scheme.contains("{{") {
        return Err(UrlError::UnresolvedVariable(first_variable(url)));
    }
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(UrlError::UnsupportedScheme(scheme));
    }

    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, after_authority) = rest.split_at(authority_end);
    validate_authority(authority)?;

    let (path, existing_query) = match after_authority.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (after_authority, None),
    };

    let mut out = String::with_capacity(url.len() + 32);
    out.push_str(&scheme);
    out.push_str("://");
    out.push_str(authority);

    if path.is_empty() {
        // Like curl: `https://host` and `https://host?q` request `/`.
        out.push('/');
    }
    let mut first = true;
    for segment in path.split('/') {
        if first {
            // `path` starts with `/` (or is empty), so the first split item is empty.
            first = false;
            continue;
        }
        out.push('/');
        match param_name(segment) {
            Some(name) => {
                let value = path_params
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| p.value.as_str())
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| UrlError::MissingPathParam(name.to_string()))?;
                push_encoded(&mut out, value, encode, Set::Segment);
            }
            None => push_encoded(&mut out, segment, encode, Set::Segment),
        }
    }

    let mut has_query = false;
    if let Some(query) = existing_query.filter(|q| !q.is_empty()) {
        out.push('?');
        push_encoded(&mut out, query, encode, Set::WholeQuery);
        has_query = true;
    }
    for param in params.iter().filter(|p| p.enabled && !p.name.is_empty()) {
        out.push(if has_query { '&' } else { '?' });
        has_query = true;
        push_encoded(&mut out, &param.name, encode, Set::QueryComponent);
        out.push('=');
        push_encoded(&mut out, &param.value, encode, Set::QueryComponent);
    }
    Ok(out)
}

/// Encodes a form field for `application/x-www-form-urlencoded` bodies (WHATWG URL
/// spec): spaces become `+`, everything except `*-._` and alphanumerics is encoded.
pub fn form_urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for &b in value.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => out.push(b as char),
            b' ' => out.push('+'),
            _ => push_pct(&mut out, b),
        }
    }
    out
}

/// The path part of a URL: after the authority, before `?` and `#`.
fn path_part(url: &str) -> &str {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path_start = after_scheme.find('/').unwrap_or(after_scheme.len());
    let path = &after_scheme[path_start..];
    let end = path.find(['?', '#']).unwrap_or(path.len());
    &path[..end]
}

fn param_name(segment: &str) -> Option<&str> {
    let name = if let Some(name) = segment.strip_prefix(':') {
        name
    } else if segment.starts_with('{') && !segment.starts_with("{{") && segment.ends_with('}') {
        &segment[1..segment.len() - 1]
    } else {
        return None;
    };
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    valid.then_some(name)
}

fn first_variable(url: &str) -> String {
    let start = url.find("{{").unwrap_or(0);
    let end = url[start..].find("}}").map_or(url.len(), |i| start + i + 2);
    url[start..end].to_string()
}

fn validate_authority(authority: &str) -> Result<(), UrlError> {
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if host_port.contains("{{") {
        return Err(UrlError::UnresolvedVariable(first_variable(host_port)));
    }
    let host = if host_port.starts_with('[') {
        // IPv6 literal: `[::1]:8080`.
        host_port.split_once(']').map_or(host_port, |(h, _)| &h[1..])
    } else {
        host_port.split_once(':').map_or(host_port, |(h, _)| h)
    };
    if host.is_empty() {
        return Err(UrlError::MissingHost);
    }
    let valid = host
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | '_' | ':' | '%'));
    if !valid {
        return Err(UrlError::InvalidHost(host.to_string()));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Set {
    /// One path segment (`pchar`): `/` is encoded.
    Segment,
    /// A query name or value: `&`, `=`, `+` and `#` are encoded.
    QueryComponent,
    /// An existing query string typed into the URL: `&` and `=` keep their meaning.
    WholeQuery,
}

fn keep(b: u8, set: Set) -> bool {
    let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~');
    // `{` and `}` are kept so `{{variables}}` survive until Phase 3 resolves them.
    let common = unreserved
        || matches!(
            b,
            b'!' | b'$' | b'\'' | b'(' | b')' | b'*' | b',' | b';' | b':' | b'@' | b'{' | b'}'
        );
    match set {
        Set::Segment => common || matches!(b, b'&' | b'=' | b'+'),
        Set::QueryComponent => common || matches!(b, b'/' | b'?'),
        Set::WholeQuery => common || matches!(b, b'/' | b'?' | b'&' | b'=' | b'+'),
    }
}

fn push_encoded(out: &mut String, value: &str, encode: bool, set: Set) {
    if !encode {
        out.push_str(value);
        return;
    }
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' && i + 2 < bytes.len() && is_hex(bytes[i + 1]) && is_hex(bytes[i + 2]) {
            out.push_str(&value[i..i + 3]);
            i += 3;
            continue;
        }
        if keep(b, set) {
            out.push(b as char);
        } else {
            push_pct(out, b);
        }
        i += 1;
    }
}

/// Percent-encodes `value` as a query name or value, as `build_url` does.
pub(crate) fn encode_query_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    push_encoded(&mut out, value, true, Set::QueryComponent);
    out
}

/// Percent-encodes `value` as one path segment, as `build_url` does for path params.
pub(crate) fn encode_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    push_encoded(&mut out, value, true, Set::Segment);
    out
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

fn push_pct(out: &mut String, b: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push('%');
    out.push(HEX[(b >> 4) as usize] as char);
    out.push(HEX[(b & 0x0f) as usize] as char);
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Case {
        name: String,
        url: String,
        #[serde(default)]
        path_params: Vec<(String, String)>,
        #[serde(default)]
        params: Vec<(String, String, bool)>,
        #[serde(default = "yes")]
        encode: bool,
        expected: Option<String>,
        #[serde(default)]
        error: Option<String>,
    }

    fn yes() -> bool {
        true
    }

    const CASES: &str = include_str!("../../../tests/fixtures/url/cases.json");

    #[test]
    fn fixture_cases() {
        let cases: Vec<Case> = serde_json::from_str(CASES).expect("cases.json is valid");
        assert!(cases.len() >= 30, "need at least 30 cases, have {}", cases.len());
        for case in cases {
            let path_params: Vec<PathParam> = case
                .path_params
                .iter()
                .map(|(n, v)| PathParam {
                    name: n.clone(),
                    value: v.clone(),
                    extra: Default::default(),
                })
                .collect();
            let params: Vec<KeyValue> = case
                .params
                .iter()
                .map(|(n, v, enabled)| KeyValue {
                    enabled: *enabled,
                    ..KeyValue::new(n.clone(), v.clone())
                })
                .collect();
            let result = build_url(&case.url, &path_params, &params, case.encode);
            match (&case.expected, &case.error, result) {
                (Some(expected), _, Ok(actual)) => assert_eq!(&actual, expected, "case `{}`", case.name),
                (_, Some(kind), Err(err)) => {
                    assert_eq!(&error_kind(&err), kind, "case `{}`: {err}", case.name)
                }
                (_, _, result) => panic!("case `{}`: unexpected result {result:?}", case.name),
            }
        }
    }

    fn error_kind(err: &UrlError) -> String {
        match err {
            UrlError::Empty => "empty",
            UrlError::MissingScheme(_) => "missingScheme",
            UrlError::UnsupportedScheme(_) => "unsupportedScheme",
            UrlError::MissingHost => "missingHost",
            UrlError::InvalidHost(_) => "invalidHost",
            UrlError::UnresolvedVariable(_) => "unresolvedVariable",
            UrlError::MissingPathParam(_) => "missingPathParam",
        }
        .to_string()
    }

    #[test]
    fn path_param_names_in_order_without_duplicates() {
        assert_eq!(
            path_param_names("https://api.example.com:8443/orgs/:orgId/users/{userId}/:orgId?x=:no"),
            vec!["orgId", "userId"]
        );
    }

    #[test]
    fn port_and_variables_are_not_path_params() {
        assert!(path_param_names("http://localhost:8080/a/{{var}}/b").is_empty());
        assert!(path_param_names("{{baseUrl}}/users").is_empty());
    }

    #[test]
    fn path_param_names_accept_relative_variable_urls() {
        assert_eq!(path_param_names("{{baseUrl}}/users/:id"), vec!["id"]);
    }

    #[test]
    fn missing_scheme_suggests_https() {
        let err = build_url("api.example.com/x", &[], &[], true).unwrap_err();
        assert_eq!(
            err.to_string(),
            "the URL needs a scheme, e.g. https://api.example.com/x"
        );
    }

    #[test]
    fn form_urlencode_uses_plus_for_spaces() {
        assert_eq!(form_urlencode("a b&c=d/é"), "a+b%26c%3Dd%2F%C3%A9");
    }
}
