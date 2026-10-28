//! Redirect handling: which statuses redirect, and how the next request is built.

use http::header::{AUTHORIZATION, COOKIE, HOST, LOCATION, PROXY_AUTHORIZATION};
use http::{HeaderMap, Method, StatusCode, Uri};

use crate::body::Body;
use crate::engine::Request;
use crate::error::Error;

/// The `Location` of a redirect response, if it is one we follow.
pub(crate) fn location(status: StatusCode, headers: &HeaderMap) -> Option<&str> {
    match status.as_u16() {
        301 | 302 | 303 | 307 | 308 => headers.get(LOCATION)?.to_str().ok(),
        _ => None,
    }
}

/// Builds the request for the next hop (RFC 9110 §15.4, matching browser behaviour):
/// - 301/302 change POST to GET, and 303 changes anything but HEAD to GET; the body and
///   `Content-*` headers are dropped when the method changes.
/// - 307/308 keep the method and body.
/// - Credentials (`Authorization`, `Cookie`, `Proxy-Authorization`) and an explicit
///   `Host` are dropped when the redirect leaves the original origin.
pub(crate) fn next_request(req: &Request, status: StatusCode, location: &str) -> Result<Request, Error> {
    let invalid = |reason: String| Error::InvalidRedirect {
        location: location.to_string(),
        reason,
    };
    let base = url::Url::parse(&req.url.to_string()).map_err(|e| invalid(e.to_string()))?;
    let mut next = base.join(location).map_err(|e| invalid(e.to_string()))?;
    next.set_fragment(None);
    if !matches!(next.scheme(), "http" | "https") {
        return Err(invalid(format!("unsupported scheme `{}`", next.scheme())));
    }
    let uri: Uri = next
        .as_str()
        .parse()
        .map_err(|e: http::uri::InvalidUri| invalid(e.to_string()))?;

    let mut out = req.clone();
    let change_to_get = match status.as_u16() {
        301 | 302 => req.method == Method::POST,
        303 => req.method != Method::HEAD,
        _ => false,
    };
    if change_to_get {
        out.method = Method::GET;
        out.body = Body::default();
        let content: Vec<_> = out
            .headers
            .keys()
            .filter(|name| name.as_str().starts_with("content-"))
            .cloned()
            .collect();
        for name in content {
            out.headers.remove(name);
        }
    }
    if !same_origin(&req.url, &uri) {
        for name in [AUTHORIZATION, COOKIE, PROXY_AUTHORIZATION, HOST] {
            out.headers.remove(name);
        }
        for name in &req.options.strip_on_cross_origin {
            out.headers.remove(name.as_str());
        }
    }
    out.url = uri;
    Ok(out)
}

fn same_origin(a: &Uri, b: &Uri) -> bool {
    let port = |u: &Uri| {
        u.port_u16()
            .unwrap_or(if u.scheme_str() == Some("https") { 443 } else { 80 })
    };
    a.scheme_str() == b.scheme_str()
        && a.host().map(str::to_ascii_lowercase) == b.host().map(str::to_ascii_lowercase)
        && port(a) == port(b)
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use http::HeaderValue;
    use http::header::CONTENT_TYPE;

    use super::*;

    fn post(url: &str) -> Request {
        let mut req = Request::new(Method::POST, url).unwrap();
        req.body = Bytes::from_static(b"{}").into();
        req.headers
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        req.headers
            .insert(AUTHORIZATION, HeaderValue::from_static("Bearer t"));
        req.headers.insert(COOKIE, HeaderValue::from_static("a=b"));
        req
    }

    #[test]
    fn relative_location_resolves_against_current_url() {
        let req = post("https://api.example.com/v1/users?x=1");
        let next = next_request(&req, StatusCode::TEMPORARY_REDIRECT, "../v2/users#frag").unwrap();
        assert_eq!(next.url, "https://api.example.com/v2/users");
        let next = next_request(&req, StatusCode::TEMPORARY_REDIRECT, "/root").unwrap();
        assert_eq!(next.url, "https://api.example.com/root");
    }

    #[test]
    fn post_becomes_get_on_301_302_303() {
        for status in [301, 302, 303] {
            let next = next_request(
                &post("https://a.test/x"),
                StatusCode::from_u16(status).unwrap(),
                "/y",
            )
            .unwrap();
            assert_eq!(next.method, Method::GET, "{status}");
            assert!(next.body.is_empty(), "{status}");
            assert!(!next.headers.contains_key(CONTENT_TYPE), "{status}");
        }
    }

    #[test]
    fn method_and_body_kept_on_307_308() {
        for status in [307, 308] {
            let next = next_request(
                &post("https://a.test/x"),
                StatusCode::from_u16(status).unwrap(),
                "/y",
            )
            .unwrap();
            assert_eq!(next.method, Method::POST, "{status}");
            assert!(matches!(&next.body, Body::Bytes(b) if b == "{}"), "{status}");
            assert!(next.headers.contains_key(CONTENT_TYPE), "{status}");
        }
    }

    #[test]
    fn head_stays_head_on_303_and_put_stays_put_on_302() {
        let head = Request::new(Method::HEAD, "https://a.test/x").unwrap();
        assert_eq!(
            next_request(&head, StatusCode::SEE_OTHER, "/y").unwrap().method,
            Method::HEAD
        );
        let put = Request::new(Method::PUT, "https://a.test/x").unwrap();
        assert_eq!(
            next_request(&put, StatusCode::FOUND, "/y").unwrap().method,
            Method::PUT
        );
        assert_eq!(
            next_request(&put, StatusCode::SEE_OTHER, "/y").unwrap().method,
            Method::GET
        );
    }

    #[test]
    fn credentials_dropped_only_when_leaving_origin() {
        let req = post("https://a.test/x");
        let same = next_request(&req, StatusCode::TEMPORARY_REDIRECT, "https://A.test:443/y").unwrap();
        assert!(same.headers.contains_key(AUTHORIZATION));
        assert!(same.headers.contains_key(COOKIE));

        for target in ["https://b.test/y", "http://a.test/y", "https://a.test:8443/y"] {
            let other = next_request(&req, StatusCode::TEMPORARY_REDIRECT, target).unwrap();
            assert!(!other.headers.contains_key(AUTHORIZATION), "{target}");
            assert!(!other.headers.contains_key(COOKIE), "{target}");
        }
    }

    #[test]
    fn configured_sensitive_headers_are_dropped_across_origins() {
        let mut req = post("https://a.test/x");
        req.headers.insert("x-api-key", HeaderValue::from_static("k"));
        req.headers.insert("x-session", HeaderValue::from_static("s"));
        req.headers.insert("x-trace", HeaderValue::from_static("t"));
        req.options.strip_on_cross_origin = vec!["X-Api-Key".into(), "X-Session".into()];
        let same = next_request(&req, StatusCode::TEMPORARY_REDIRECT, "https://a.test/y").unwrap();
        assert!(same.headers.contains_key("x-api-key") && same.headers.contains_key("x-session"));
        let other = next_request(&req, StatusCode::TEMPORARY_REDIRECT, "https://evil.test/y").unwrap();
        assert!(!other.headers.contains_key("x-api-key"));
        assert!(!other.headers.contains_key("x-session"));
        assert!(other.headers.contains_key("x-trace"), "other headers are kept");
    }

    #[test]
    fn non_http_location_is_rejected() {
        let err = next_request(&post("https://a.test/x"), StatusCode::FOUND, "ftp://a.test/f").unwrap_err();
        assert!(matches!(err, Error::InvalidRedirect { .. }), "{err:?}");
    }
}
