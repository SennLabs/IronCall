//! Turning a request file into what goes on the wire (PLAN.md Phases 1 and 4).
//!
//! [`build`] resolves the URL ([`crate::url::build_url`]), merges default headers from the
//! collection and folders, applies the effective auth, derives `Content-Type` from the
//! body type and prepares the body. The HTTP engine adds its own headers (Host,
//! Content-Length, User-Agent, Accept-Encoding, Cookie, multipart boundary) at send time.
//!
//! Header precedence, highest first: the user's request headers, auth, default headers
//! (innermost folder first, then the collection), derived `Content-Type`. Lower ones
//! are still listed by [`derived`] with `effective: false`, so the preview can show them
//! struck through.
//!
//! Variables must already be resolved (see [`crate::vars::resolve_request`],
//! [`crate::vars::resolve_auth`] and [`crate::vars::resolve_rows`]); leftover `{{refs}}`
//! in the URL, headers, auth or file paths are errors.

use std::path::{Path, PathBuf};

use base64::Engine as _;

use crate::model::{ApiKeyLocation, Auth, Body, HttpVersion, KeyValue, MultipartKind, RequestFile};
use crate::paths;
use crate::url::{self, UrlError};
use crate::vars;

/// Values used when a request doesn't set them (app settings in Phase 8).
#[derive(Clone, Debug)]
pub struct WireDefaults {
    pub timeout_ms: Option<u64>,
    pub follow_redirects: bool,
    pub max_redirects: u32,
    pub verify_tls: bool,
    pub http_version: HttpVersion,
    pub fresh_connection: bool,
}

impl Default for WireDefaults {
    fn default() -> Self {
        Self {
            timeout_ms: Some(30_000),
            follow_redirects: true,
            max_redirects: 10,
            verify_tls: true,
            http_version: HttpVersion::Auto,
            fresh_connection: false,
        }
    }
}

/// Default headers from one scope (the collection or a folder).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DefaultHeaders {
    /// Shown in the preview, e.g. `Collection` or `Folder › Users`.
    pub origin: String,
    pub headers: Vec<KeyValue>,
}

/// Everything besides the request itself that shapes what is sent.
#[derive(Clone, Debug, Default)]
pub struct WireContext {
    pub defaults: WireDefaults,
    /// The effective auth after inheritance ([`crate::auth::effective_auth`]), with
    /// variables resolved. `Inherit` is treated as no auth.
    pub auth: Auth,
    /// Where the auth comes from, for the preview (`None` for request-level auth).
    pub auth_origin: Option<String>,
    /// Default headers, collection first, then folders from outermost to innermost.
    /// Values must be resolved ([`crate::vars::resolve_rows`]).
    pub default_headers: Vec<DefaultHeaders>,
    /// Workspace root for relative file paths. `None` for scratch requests.
    pub root: Option<PathBuf>,
    /// Build even if `{{references}}` are left unresolved, keeping them as written.
    /// Only for templates (e.g. "Copy as cURL" with variables); never for sending.
    pub allow_unresolved: bool,
}

/// A request ready for the HTTP engine.
#[derive(Clone, Debug, PartialEq)]
pub struct WireRequest {
    /// Uppercased method token.
    pub method: String,
    pub url: String,
    /// User headers (enabled rows) followed by effective derived headers, in send order.
    pub headers: Vec<(String, String)>,
    pub body: WireBody,
    pub timeout_ms: Option<u64>,
    pub follow_redirects: bool,
    pub max_redirects: u32,
    pub verify_tls: bool,
    pub http_version: HttpVersion,
    pub fresh_connection: bool,
}

/// The request body. File and multipart bodies are streamed from disk by the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireBody {
    Bytes(Vec<u8>),
    /// A binary file body. `Content-Type` is already among the derived headers.
    File {
        path: PathBuf,
        content_type: String,
    },
    /// `multipart/form-data`. The engine picks the boundary and sets `Content-Type`.
    Multipart(Vec<WirePart>),
}

impl WireBody {
    /// Size of an in-memory body; `None` for file and multipart bodies.
    pub fn bytes_len(&self) -> Option<u64> {
        match self {
            WireBody::Bytes(b) => Some(b.len() as u64),
            WireBody::File { .. } | WireBody::Multipart(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WirePart {
    pub name: String,
    pub content: PartContent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartContent {
    Text(String),
    File {
        path: PathBuf,
        /// Sent as the part's `filename`.
        file_name: String,
        content_type: String,
    },
}

/// What kind of derived header a row is, for the preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DerivedKind {
    /// Derived from the request, e.g. `Content-Type` from the body type.
    Derived,
    /// From the effective auth.
    Auth,
    /// A default header from the collection or a folder.
    Default,
}

/// A header added on top of the user's rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedHeader {
    pub name: String,
    pub value: String,
    pub kind: DerivedKind,
    /// Where an auth or default header comes from, e.g. `Folder › Users`.
    pub origin: Option<String>,
    /// False if a higher-precedence header with the same name replaces it.
    pub effective: bool,
}

/// A file a request would upload, as written and as resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyFile {
    pub written: String,
    /// `None` if the path is empty, relative without a workspace, or has `{{refs}}`.
    pub resolved: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    #[error("{0}")]
    Url(#[from] UrlError),
    #[error("enter a method")]
    EmptyMethod,
    #[error("`{0}` is not a valid HTTP method")]
    InvalidMethod(String),
    #[error("`{0}` is not a valid header name")]
    InvalidHeaderName(String),
    #[error("the value of header `{0}` contains a line break or control character")]
    InvalidHeaderValue(String),
    #[error("{0}")]
    Unsupported(String),
    /// A `{{reference}}` left in a header after variable resolution.
    #[error("variable `{variable}` in header `{header}` is not defined in any scope")]
    UnresolvedHeaderVariable { header: String, variable: String },
    /// Auth is set up but incomplete, e.g. a Bearer token is empty.
    #[error("{0}")]
    Auth(String),
    #[error("variable `{0}` in the auth settings is not defined in any scope")]
    UnresolvedAuthVariable(String),
    #[error("choose a file for {0}")]
    MissingFilePath(String),
    #[error("variable `{variable}` in the file path for {what} is not defined in any scope")]
    UnresolvedPathVariable { what: String, variable: String },
    #[error("`{0}` is a relative path, but this request isn't in a workspace; choose the file again")]
    RelativePathWithoutWorkspace(String),
}

/// Builds the wire request for `req`.
pub fn build(req: &RequestFile, ctx: &WireContext) -> Result<WireRequest, BuildError> {
    let method = method(&req.method)?;
    let encode = req.settings.encode_url.unwrap_or(true);

    let mut params = req.params.clone();
    if let Some(param) = api_key_query_param(req, &ctx.auth, ctx.allow_unresolved)? {
        params.push(param);
    }
    let allow = ctx.allow_unresolved;
    let url = match url::build_url(&req.url, &req.path_params, &params, encode) {
        Err(UrlError::UnresolvedVariable(_)) if allow => template_url(&req.url, &req.path_params, &params),
        other => other?,
    };
    // Braces are never encoded, so anything left here is a reference that variable
    // resolution couldn't fill. Sending `{{x}}` literally is almost always a mistake.
    if !allow && let Some(variable) = vars::first_ref(&url) {
        return Err(BuildError::Url(UrlError::UnresolvedVariable(variable)));
    }

    // Validate auth first, so an incomplete setup is reported even if overridden.
    match auth_header(&ctx.auth, ctx.allow_unresolved) {
        Err(BuildError::UnresolvedAuthVariable(_)) if allow => None,
        other => other?,
    };
    let mut headers = user_headers(&req.headers)?;
    for h in derived(ctx, req).into_iter().filter(|h| h.effective) {
        check_header(&h.name, &h.value)?;
        headers.push((h.name, h.value));
    }
    if !allow
        && let Some((header, variable)) = headers.iter().find_map(|(n, v)| {
            vars::first_ref(n)
                .or_else(|| vars::first_ref(v))
                .map(|r| (n.clone(), r))
        })
    {
        return Err(BuildError::UnresolvedHeaderVariable { header, variable });
    }
    let body = body(&req.body, ctx.root.as_deref(), allow)?;

    let s = &req.settings;
    let d = &ctx.defaults;
    Ok(WireRequest {
        method,
        url,
        headers,
        body,
        // 0 means no timeout, for a request's own setting as for the app default.
        timeout_ms: match s.timeout_ms {
            Some(0) => None,
            Some(ms) => Some(ms),
            None => d.timeout_ms,
        },
        follow_redirects: s.follow_redirects.unwrap_or(d.follow_redirects),
        max_redirects: s.max_redirects.unwrap_or(d.max_redirects),
        verify_tls: s.verify_tls.unwrap_or(d.verify_tls),
        http_version: s.http_version.unwrap_or(d.http_version),
        fresh_connection: s.fresh_connection.unwrap_or(d.fresh_connection),
    })
}

/// Headers the build adds on top of the user's rows, in send order: default headers,
/// derived `Content-Type`, then auth. Overridden ones are included with
/// `effective: false`. Incomplete auth is left out (and reported by [`build`]).
pub fn derived(ctx: &WireContext, req: &RequestFile) -> Vec<DerivedHeader> {
    let user_set = |name: &str| {
        req.headers
            .iter()
            .any(|h| h.enabled && h.name.trim().eq_ignore_ascii_case(name))
    };

    let auth = auth_header(&ctx.auth, ctx.allow_unresolved)
        .ok()
        .flatten()
        .map(|(name, value)| {
            let effective = !user_set(&name);
            DerivedHeader {
                name,
                value,
                kind: DerivedKind::Auth,
                origin: ctx.auth_origin.clone(),
                effective,
            }
        });
    let auth_sets = |name: &str| {
        auth.as_ref()
            .is_some_and(|a| a.effective && a.name.eq_ignore_ascii_case(name))
    };

    let defaults: Vec<DerivedHeader> = merge_defaults(&ctx.default_headers)
        .into_iter()
        .map(|(name, value, origin)| {
            let effective = !user_set(&name) && !auth_sets(&name);
            DerivedHeader {
                name,
                value,
                kind: DerivedKind::Default,
                origin: Some(origin),
                effective,
            }
        })
        .collect();
    let default_sets = |name: &str| {
        defaults
            .iter()
            .any(|d| d.effective && d.name.eq_ignore_ascii_case(name))
    };

    let content_type = body_content_type(&req.body).map(|value| DerivedHeader {
        effective: !user_set("content-type") && !auth_sets("content-type") && !default_sets("content-type"),
        name: "Content-Type".to_string(),
        value,
        kind: DerivedKind::Derived,
        origin: None,
    });

    defaults.into_iter().chain(content_type).chain(auth).collect()
}

/// The files `req` would upload (enabled multipart file parts, or the binary body).
pub fn body_files(req: &RequestFile, root: Option<&Path>) -> Vec<BodyFile> {
    let resolve = |written: &str| BodyFile {
        written: written.to_string(),
        resolved: (!written.trim().is_empty() && vars::first_ref(written).is_none())
            .then(|| paths::resolve(root, written.trim()))
            .flatten(),
    };
    match &req.body {
        Body::BinaryFile { path, .. } => vec![resolve(path)],
        Body::Multipart { parts, .. } => parts
            .iter()
            .filter(|p| p.enabled && p.kind == MultipartKind::File)
            .map(|p| resolve(p.path.as_deref().unwrap_or_default()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Merges default headers: later layers (inner folders) replace earlier ones by
/// case-insensitive name, keeping the position of the first occurrence. Only enabled,
/// named rows count.
fn merge_defaults(layers: &[DefaultHeaders]) -> Vec<(String, String, String)> {
    let mut merged: Vec<(String, String, String)> = Vec::new();
    for layer in layers {
        for h in layer
            .headers
            .iter()
            .filter(|h| h.enabled && !h.name.trim().is_empty())
        {
            let name = h.name.trim();
            let value = h.value.trim().to_string();
            match merged.iter_mut().find(|(n, _, _)| n.eq_ignore_ascii_case(name)) {
                Some(existing) => *existing = (existing.0.clone(), value, layer.origin.clone()),
                None => merged.push((name.to_string(), value, layer.origin.clone())),
            }
        }
    }
    merged
}

fn body_content_type(body: &Body) -> Option<String> {
    Some(match body {
        Body::Json { .. } => "application/json".to_string(),
        Body::Xml { .. } => "application/xml".to_string(),
        Body::Text { .. } => "text/plain; charset=utf-8".to_string(),
        Body::FormUrlencoded { .. } => "application/x-www-form-urlencoded".to_string(),
        Body::BinaryFile {
            path, content_type, ..
        } => file_content_type(content_type.as_deref(), path),
        Body::None | Body::Multipart { .. } => return None,
    })
}

fn file_content_type(explicit: Option<&str>, path: &str) -> String {
    explicit
        .map(str::trim)
        .filter(|ct| !ct.is_empty())
        .map(str::to_string)
        .or_else(|| paths::guess_content_type(Path::new(path)).map(str::to_string))
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

fn check_auth_refs(fields: &[&str]) -> Result<(), BuildError> {
    match fields.iter().find_map(|f| vars::first_ref(f)) {
        Some(variable) => Err(BuildError::UnresolvedAuthVariable(variable)),
        None => Ok(()),
    }
}

/// The header an auth adds, or `None` for no auth or an API key sent in the query.
/// With `template` (exports with `{{variables}}`), Bearer and API key references are kept
/// as text; Basic can't be, since its value is base64-encoded.
fn auth_header(auth: &Auth, template: bool) -> Result<Option<(String, String)>, BuildError> {
    Ok(match auth {
        Auth::Inherit | Auth::None { .. } => None,
        Auth::Basic {
            username, password, ..
        } => {
            check_auth_refs(&[username, password])?;
            if username.trim().is_empty() {
                return Err(BuildError::Auth("enter a username for Basic auth".into()));
            }
            let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
            Some(("Authorization".into(), format!("Basic {encoded}")))
        }
        Auth::Bearer { token, .. } => {
            if !template {
                check_auth_refs(&[token])?;
            }
            if token.trim().is_empty() {
                return Err(BuildError::Auth("enter a token for Bearer auth".into()));
            }
            Some(("Authorization".into(), format!("Bearer {}", token.trim())))
        }
        Auth::ApiKey {
            key, value, location, ..
        } => {
            if !template {
                check_auth_refs(&[key, value])?;
            }
            if key.trim().is_empty() {
                return Err(BuildError::Auth(
                    "enter the API key's header or parameter name".into(),
                ));
            }
            if value.is_empty() {
                return Err(BuildError::Auth(format!(
                    "enter a value for the API key `{}`",
                    key.trim()
                )));
            }
            match location {
                ApiKeyLocation::Header => Some((key.trim().to_string(), value.trim().to_string())),
                ApiKeyLocation::Query => None,
            }
        }
    })
}

/// The query param for an API key sent in the query, unless the user already has an
/// enabled param with the same name.
fn api_key_query_param(
    req: &RequestFile,
    auth: &Auth,
    template: bool,
) -> Result<Option<KeyValue>, BuildError> {
    let Auth::ApiKey {
        key,
        value,
        location: ApiKeyLocation::Query,
        ..
    } = auth
    else {
        return Ok(None);
    };
    auth_header(auth, template)?;
    let key = key.trim();
    let user_set = req.params.iter().any(|p| p.enabled && p.name == key);
    Ok((!user_set).then(|| KeyValue::new(key, value.clone())))
}

fn method(raw: &str) -> Result<String, BuildError> {
    let method = raw.trim();
    if method.is_empty() {
        return Err(BuildError::EmptyMethod);
    }
    if !method.bytes().all(is_token_char) {
        return Err(BuildError::InvalidMethod(method.to_string()));
    }
    Ok(method.to_ascii_uppercase())
}

fn check_header(name: &str, value: &str) -> Result<(), BuildError> {
    if !name.bytes().all(is_token_char) {
        return Err(BuildError::InvalidHeaderName(name.to_string()));
    }
    // Obsolete line folding isn't allowed; tabs inside values are.
    if value.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7f) {
        return Err(BuildError::InvalidHeaderValue(name.to_string()));
    }
    Ok(())
}

fn user_headers(rows: &[KeyValue]) -> Result<Vec<(String, String)>, BuildError> {
    rows.iter()
        .filter(|h| h.enabled)
        .filter(|h| !h.name.trim().is_empty())
        .map(|h| {
            let name = h.name.trim();
            check_header(name, &h.value)?;
            Ok((name.to_string(), h.value.trim().to_string()))
        })
        .collect()
}

/// Resolves a file path for upload, with a description of what it is for errors.
fn file_path(
    written: Option<&str>,
    what: &str,
    root: Option<&Path>,
    allow_unresolved: bool,
) -> Result<PathBuf, BuildError> {
    let written = written.map(str::trim).unwrap_or_default();
    if written.is_empty() {
        return Err(BuildError::MissingFilePath(what.to_string()));
    }
    if allow_unresolved && vars::first_ref(written).is_some() {
        return Ok(PathBuf::from(written));
    }
    if let Some(variable) = vars::first_ref(written) {
        return Err(BuildError::UnresolvedPathVariable {
            what: what.to_string(),
            variable,
        });
    }
    paths::resolve(root, written).ok_or_else(|| BuildError::RelativePathWithoutWorkspace(written.to_string()))
}

/// The URL as a template: path params and enabled query params joined as written,
/// without validation or encoding (it still contains `{{references}}`).
fn template_url(url: &str, path_params: &[crate::model::PathParam], params: &[KeyValue]) -> String {
    let mut out = url.to_string();
    for p in path_params {
        if !p.value.is_empty() {
            out = out.replace(&format!(":{}", p.name), &p.value);
        }
    }
    let query: Vec<String> = params
        .iter()
        .filter(|p| p.enabled && !p.name.is_empty())
        .map(|p| {
            if p.value.is_empty() {
                p.name.clone()
            } else {
                format!("{}={}", p.name, p.value)
            }
        })
        .collect();
    if !query.is_empty() {
        out.push(if out.contains('?') { '&' } else { '?' });
        out.push_str(&query.join("&"));
    }
    out
}

fn body(body: &Body, root: Option<&Path>, allow_unresolved: bool) -> Result<WireBody, BuildError> {
    Ok(match body {
        Body::None => WireBody::Bytes(Vec::new()),
        Body::Json { content, .. } | Body::Text { content, .. } | Body::Xml { content, .. } => {
            WireBody::Bytes(content.as_bytes().to_vec())
        }
        Body::FormUrlencoded { fields, .. } => WireBody::Bytes(
            fields
                .iter()
                .filter(|f| f.enabled && !f.name.is_empty())
                .map(|f| {
                    format!(
                        "{}={}",
                        url::form_urlencode(&f.name),
                        url::form_urlencode(&f.value)
                    )
                })
                .collect::<Vec<_>>()
                .join("&")
                .into_bytes(),
        ),
        Body::BinaryFile {
            path, content_type, ..
        } => WireBody::File {
            path: file_path(Some(path), "the binary body", root, allow_unresolved)?,
            content_type: file_content_type(content_type.as_deref(), path),
        },
        Body::Multipart { parts, .. } => WireBody::Multipart(
            parts
                .iter()
                .filter(|p| p.enabled && !p.name.trim().is_empty())
                .map(|p| {
                    let name = p.name.trim().to_string();
                    let content = match p.kind {
                        MultipartKind::Text => PartContent::Text(p.value.clone().unwrap_or_default()),
                        MultipartKind::File => {
                            let what = format!("part `{name}`");
                            let path = file_path(p.path.as_deref(), &what, root, allow_unresolved)?;
                            let file_name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "file".to_string());
                            PartContent::File {
                                content_type: file_content_type(
                                    p.content_type.as_deref(),
                                    p.path.as_deref().unwrap_or_default(),
                                ),
                                path,
                                file_name,
                            }
                        }
                    };
                    Ok(WirePart { name, content })
                })
                .collect::<Result<_, BuildError>>()?,
        ),
    })
}

/// RFC 9110 `tchar`.
fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Extra, MultipartPart, PathParam};

    /// Effective derived headers as (name, value).
    fn effective(req: &RequestFile, ctx: &WireContext) -> Vec<(String, String)> {
        derived(ctx, req)
            .into_iter()
            .filter(|h| h.effective)
            .map(|h| (h.name, h.value))
            .collect()
    }

    fn ctx_with_auth(auth: Auth, origin: Option<&str>) -> WireContext {
        WireContext {
            auth,
            auth_origin: origin.map(str::to_string),
            ..WireContext::default()
        }
    }

    fn basic(user: &str, pass: &str) -> Auth {
        Auth::Basic {
            username: user.into(),
            password: pass.into(),
            extra: Extra::new(),
        }
    }

    fn bearer(token: &str) -> Auth {
        Auth::Bearer {
            token: token.into(),
            extra: Extra::new(),
        }
    }

    fn api_key(key: &str, value: &str, location: ApiKeyLocation) -> Auth {
        Auth::ApiKey {
            key: key.into(),
            value: value.into(),
            location,
            extra: Extra::new(),
        }
    }

    fn part_text(name: &str, value: &str) -> MultipartPart {
        MultipartPart {
            name: name.into(),
            kind: MultipartKind::Text,
            value: Some(value.into()),
            path: None,
            content_type: None,
            enabled: true,
            extra: Extra::new(),
        }
    }

    fn part_file(name: &str, path: &str) -> MultipartPart {
        MultipartPart {
            kind: MultipartKind::File,
            value: None,
            path: Some(path.into()),
            ..part_text(name, "")
        }
    }

    #[cfg(unix)]
    const ROOT: &str = "/ws/project";
    #[cfg(windows)]
    const ROOT: &str = r"C:\ws\project";

    #[test]
    fn a_zero_timeout_means_none() {
        let ctx = WireContext::default();
        let mut req = RequestFile::new("t");
        req.url = "https://example.com/".into();
        assert_eq!(build(&req, &ctx).unwrap().timeout_ms, Some(30_000), "app default");
        req.settings.timeout_ms = Some(0);
        assert_eq!(build(&req, &ctx).unwrap().timeout_ms, None);
        req.settings.timeout_ms = Some(500);
        assert_eq!(build(&req, &ctx).unwrap().timeout_ms, Some(500));
    }

    #[test]
    fn allow_unresolved_builds_a_template() {
        let mut req = RequestFile::new("t");
        req.url = "{{baseUrl}}/users/:id".into();
        req.path_params = vec![crate::model::PathParam {
            name: "id".into(),
            value: "{{userId}}".into(),
            extra: Default::default(),
        }];
        req.params = vec![KeyValue::new("q", "{{term}}")];
        req.headers = vec![KeyValue::new("X-Token", "{{token}}")];
        let ctx = WireContext {
            allow_unresolved: true,
            ..WireContext::default()
        };
        let wire = build(&req, &ctx).unwrap();
        assert_eq!(wire.url, "{{baseUrl}}/users/{{userId}}?q={{term}}");
        assert!(wire.headers.contains(&("X-Token".into(), "{{token}}".into())));
        // Without the flag, the same request is refused.
        assert!(build(&req, &WireContext::default()).is_err());
    }

    #[test]
    fn basic_auth_is_base64_utf8() {
        let req = request("GET", "https://a.example/");
        let wire = build(&req, &ctx_with_auth(basic("ada", "pässword"), None)).unwrap();
        // base64("ada:pässword") with ä as UTF-8.
        assert_eq!(
            wire.headers,
            vec![("Authorization".into(), "Basic YWRhOnDDpHNzd29yZA==".into())]
        );
        // An empty password is allowed.
        let wire = build(&req, &ctx_with_auth(basic("ada", ""), None)).unwrap();
        assert_eq!(wire.headers[0].1, "Basic YWRhOg==");
    }

    #[test]
    fn bearer_and_header_api_key() {
        let req = request("GET", "https://a.example/");
        let wire = build(&req, &ctx_with_auth(bearer(" abc.def "), None)).unwrap();
        assert_eq!(
            wire.headers,
            vec![("Authorization".into(), "Bearer abc.def".into())]
        );

        let wire = build(
            &req,
            &ctx_with_auth(api_key("X-Api-Key", "k1", ApiKeyLocation::Header), None),
        )
        .unwrap();
        assert_eq!(wire.headers, vec![("X-Api-Key".into(), "k1".into())]);
    }

    #[test]
    fn query_api_key_is_appended_and_encoded() {
        let mut req = request("GET", "https://a.example/x");
        req.params = vec![KeyValue::new("page", "2")];
        let ctx = ctx_with_auth(api_key("api key", "a&b c", ApiKeyLocation::Query), None);
        let wire = build(&req, &ctx).unwrap();
        assert_eq!(wire.url, "https://a.example/x?page=2&api%20key=a%26b%20c");
        assert!(wire.headers.is_empty());

        // A user param with the same name wins.
        req.params.push(KeyValue::new("api key", "mine"));
        let wire = build(&req, &ctx).unwrap();
        assert_eq!(wire.url, "https://a.example/x?page=2&api%20key=mine");
    }

    #[test]
    fn user_authorization_overrides_auth_and_is_shown_as_overridden() {
        let mut req = request("GET", "https://a.example/");
        req.headers = vec![KeyValue::new("authorization", "Custom xyz")];
        let ctx = ctx_with_auth(bearer("abc"), Some("Folder › Users"));
        let wire = build(&req, &ctx).unwrap();
        assert_eq!(wire.headers, vec![("authorization".into(), "Custom xyz".into())]);
        let d = derived(&ctx, &req);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].kind, DerivedKind::Auth);
        assert!(!d[0].effective);
        assert_eq!(d[0].origin.as_deref(), Some("Folder › Users"));
    }

    #[test]
    fn incomplete_auth_is_an_error() {
        let req = request("GET", "https://a.example/");
        for auth in [
            basic(" ", "x"),
            bearer(""),
            api_key("", "v", ApiKeyLocation::Header),
            api_key("k", "", ApiKeyLocation::Query),
        ] {
            assert!(
                matches!(
                    build(&req, &ctx_with_auth(auth.clone(), None)),
                    Err(BuildError::Auth(_))
                ),
                "{auth:?}"
            );
        }
        // Inherit and none add nothing.
        for auth in [Auth::Inherit, Auth::None { extra: Extra::new() }] {
            assert!(
                build(&req, &ctx_with_auth(auth, None))
                    .unwrap()
                    .headers
                    .is_empty()
            );
        }
    }

    #[test]
    fn unresolved_auth_variables_are_reported_before_encoding() {
        let req = request("GET", "https://a.example/");
        assert_eq!(
            build(&req, &ctx_with_auth(basic("{{user}}", "x"), None)).unwrap_err(),
            BuildError::UnresolvedAuthVariable("{{user}}".into())
        );
        assert_eq!(
            build(&req, &ctx_with_auth(bearer("{{token}}"), None)).unwrap_err(),
            BuildError::UnresolvedAuthVariable("{{token}}".into())
        );
    }

    #[test]
    fn default_headers_merge_inner_wins_and_request_overrides() {
        let ctx = WireContext {
            default_headers: vec![
                DefaultHeaders {
                    origin: "Collection".into(),
                    headers: vec![
                        KeyValue::new("User-Agent", "col"),
                        KeyValue::new("X-Team", "core"),
                        KeyValue {
                            enabled: false,
                            ..KeyValue::new("X-Off", "1")
                        },
                    ],
                },
                DefaultHeaders {
                    origin: "Folder › Users".into(),
                    headers: vec![
                        KeyValue::new("user-agent", "folder"),
                        KeyValue::new("X-Area", "users"),
                    ],
                },
            ],
            ..WireContext::default()
        };
        let mut req = request("GET", "https://a.example/");
        req.headers = vec![KeyValue::new("X-Team", "mine")];

        let d = derived(&ctx, &req);
        let rows: Vec<_> = d
            .iter()
            .map(|h| {
                (
                    h.name.as_str(),
                    h.value.as_str(),
                    h.origin.as_deref().unwrap(),
                    h.effective,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("User-Agent", "folder", "Folder › Users", true),
                ("X-Team", "core", "Collection", false),
                ("X-Area", "users", "Folder › Users", true),
            ]
        );
        let wire = build(&req, &ctx).unwrap();
        assert_eq!(
            wire.headers,
            vec![
                ("X-Team".into(), "mine".into()),
                ("User-Agent".into(), "folder".into()),
                ("X-Area".into(), "users".into()),
            ]
        );
    }

    #[test]
    fn auth_beats_default_authorization_and_default_beats_derived_content_type() {
        let ctx = WireContext {
            auth: bearer("t"),
            default_headers: vec![DefaultHeaders {
                origin: "Collection".into(),
                headers: vec![
                    KeyValue::new("Authorization", "Basic old"),
                    KeyValue::new("Content-Type", "application/vnd.api+json"),
                ],
            }],
            ..WireContext::default()
        };
        let mut req = request("POST", "https://a.example/");
        req.body = json("{}");
        let d = derived(&ctx, &req);
        let state: Vec<_> = d.iter().map(|h| (h.kind, h.effective)).collect();
        assert_eq!(
            state,
            vec![
                (DerivedKind::Default, false),
                (DerivedKind::Default, true),
                (DerivedKind::Derived, false),
                (DerivedKind::Auth, true),
            ]
        );
        assert_eq!(
            effective(&req, &ctx),
            vec![
                ("Content-Type".to_string(), "application/vnd.api+json".to_string()),
                ("Authorization".to_string(), "Bearer t".to_string()),
            ]
        );
    }

    #[test]
    fn binary_file_body_resolves_relative_path_and_guesses_type() {
        let mut req = request("PUT", "https://a.example/upload");
        req.body = Body::BinaryFile {
            path: "files/../scan.dcm".into(),
            content_type: None,
            extra: Extra::new(),
        };
        let ctx = WireContext {
            root: Some(PathBuf::from(ROOT)),
            ..WireContext::default()
        };
        let wire = build(&req, &ctx).unwrap();
        assert_eq!(
            wire.body,
            WireBody::File {
                path: Path::new(ROOT).join("scan.dcm"),
                content_type: "application/dicom".into()
            }
        );
        assert_eq!(
            effective(&req, &ctx),
            vec![("Content-Type".to_string(), "application/dicom".to_string())]
        );

        // Explicit type wins; unknown extensions fall back to octet-stream.
        req.body = Body::BinaryFile {
            path: "blob.xyz".into(),
            content_type: None,
            extra: Extra::new(),
        };
        assert!(matches!(
            build(&req, &ctx).unwrap().body,
            WireBody::File { content_type, .. } if content_type == "application/octet-stream"
        ));
    }

    #[test]
    fn file_paths_need_a_workspace_or_an_absolute_path() {
        let mut req = request("PUT", "https://a.example/upload");
        req.body = Body::BinaryFile {
            path: "rel.bin".into(),
            content_type: None,
            extra: Extra::new(),
        };
        assert_eq!(
            build(&req, &WireContext::default()).unwrap_err(),
            BuildError::RelativePathWithoutWorkspace("rel.bin".into())
        );
        req.body = Body::BinaryFile {
            path: " ".into(),
            content_type: None,
            extra: Extra::new(),
        };
        assert!(matches!(
            build(&req, &WireContext::default()),
            Err(BuildError::MissingFilePath(_))
        ));
        #[cfg(unix)]
        {
            req.body = Body::BinaryFile {
                path: "/tmp/abs.bin".into(),
                content_type: Some("application/x-custom".into()),
                extra: Extra::new(),
            };
            assert_eq!(
                build(&req, &WireContext::default()).unwrap().body,
                WireBody::File {
                    path: PathBuf::from("/tmp/abs.bin"),
                    content_type: "application/x-custom".into()
                }
            );
        }
    }

    #[test]
    fn multipart_parts_skip_disabled_and_resolve_files() {
        let mut off = part_text("off", "x");
        off.enabled = false;
        let mut req = request("POST", "https://a.example/upload");
        req.body = Body::Multipart {
            parts: vec![part_text("title", "Scan"), off, part_file("file", "scans/a.png")],
            extra: Extra::new(),
        };
        let ctx = WireContext {
            root: Some(PathBuf::from(ROOT)),
            ..WireContext::default()
        };
        let wire = build(&req, &ctx).unwrap();
        assert_eq!(
            wire.body,
            WireBody::Multipart(vec![
                WirePart {
                    name: "title".into(),
                    content: PartContent::Text("Scan".into())
                },
                WirePart {
                    name: "file".into(),
                    content: PartContent::File {
                        path: Path::new(ROOT).join("scans").join("a.png"),
                        file_name: "a.png".into(),
                        content_type: "image/png".into()
                    }
                },
            ])
        );
        // The engine sets the multipart Content-Type with its boundary.
        assert!(effective(&req, &ctx).is_empty());

        assert_eq!(
            body_files(&req, ctx.root.as_deref()),
            vec![BodyFile {
                written: "scans/a.png".into(),
                resolved: Some(Path::new(ROOT).join("scans").join("a.png"))
            }]
        );
    }

    #[test]
    fn unresolved_file_path_variables_are_errors() {
        let mut req = request("POST", "https://a.example/upload");
        req.body = Body::Multipart {
            parts: vec![part_file("file", "{{dir}}/a.png")],
            extra: Extra::new(),
        };
        assert_eq!(
            build(&req, &WireContext::default()).unwrap_err(),
            BuildError::UnresolvedPathVariable {
                what: "part `file`".into(),
                variable: "{{dir}}".into()
            }
        );
        assert_eq!(body_files(&req, None)[0].resolved, None);
    }

    fn request(method: &str, url: &str) -> RequestFile {
        RequestFile {
            method: method.to_string(),
            url: url.to_string(),
            ..RequestFile::new("test")
        }
    }

    fn json(content: &str) -> Body {
        Body::Json {
            content: content.to_string(),
            extra: Extra::new(),
        }
    }

    #[test]
    fn unresolved_references_in_url_or_headers_are_errors() {
        let err = build(
            &request("GET", "https://a.example/{{version}}/x"),
            &WireContext::default(),
        )
        .unwrap_err();
        assert_eq!(
            err,
            BuildError::Url(UrlError::UnresolvedVariable("{{version}}".into()))
        );
        assert_eq!(
            err.to_string(),
            "variable `{{version}}` is not defined in any scope"
        );

        let mut req = request("GET", "https://a.example/x");
        req.params = vec![KeyValue::new("key", "{{apiKey}}")];
        assert!(matches!(
            build(&req, &WireContext::default()),
            Err(BuildError::Url(UrlError::UnresolvedVariable(v))) if v == "{{apiKey}}"
        ));

        let mut req = request("GET", "https://a.example/x");
        req.headers = vec![KeyValue::new("Authorization", "Bearer {{ token }}")];
        assert_eq!(
            build(&req, &WireContext::default()).unwrap_err(),
            BuildError::UnresolvedHeaderVariable {
                header: "Authorization".into(),
                variable: "{{ token }}".into()
            }
        );

        // Bodies may legitimately contain `{{`, e.g. templates sent to a server.
        let mut req = request("POST", "https://a.example/x");
        req.body = json(r#"{"tpl": "{{name}}"}"#);
        assert!(build(&req, &WireContext::default()).is_ok());
    }

    #[test]
    fn builds_get_with_defaults() {
        let wire = build(&request("get", "https://a.example/x"), &WireContext::default()).unwrap();
        assert_eq!(wire.method, "GET");
        assert_eq!(wire.url, "https://a.example/x");
        assert!(wire.headers.is_empty());
        assert_eq!(wire.body, WireBody::Bytes(Vec::new()));
        assert_eq!(wire.timeout_ms, Some(30_000));
        assert!(wire.follow_redirects && wire.verify_tls);
        assert_eq!(wire.max_redirects, 10);
        assert_eq!(wire.http_version, HttpVersion::Auto);
    }

    #[test]
    fn settings_override_defaults() {
        let mut req = request("GET", "https://a.example/x");
        req.settings.timeout_ms = Some(500);
        req.settings.follow_redirects = Some(false);
        req.settings.verify_tls = Some(false);
        req.settings.http_version = Some(HttpVersion::Http2);
        req.settings.fresh_connection = Some(true);
        let wire = build(&req, &WireContext::default()).unwrap();
        assert_eq!(wire.timeout_ms, Some(500));
        assert!(!wire.follow_redirects && !wire.verify_tls && wire.fresh_connection);
        assert_eq!(wire.http_version, HttpVersion::Http2);
    }

    #[test]
    fn custom_methods_are_allowed_and_uppercased() {
        let wire = build(
            &request("propfind", "https://a.example/"),
            &WireContext::default(),
        )
        .unwrap();
        assert_eq!(wire.method, "PROPFIND");
    }

    #[test]
    fn invalid_methods_are_rejected() {
        assert_eq!(
            build(&request("GET X", "https://a.example/"), &WireContext::default()),
            Err(BuildError::InvalidMethod("GET X".into()))
        );
        assert_eq!(
            build(&request(" ", "https://a.example/"), &WireContext::default()),
            Err(BuildError::EmptyMethod)
        );
    }

    #[test]
    fn only_enabled_non_empty_headers_are_sent() {
        let mut req = request("GET", "https://a.example/");
        req.headers = vec![
            KeyValue::new(" Accept ", "application/json "),
            KeyValue {
                enabled: false,
                ..KeyValue::new("X-Off", "1")
            },
            KeyValue::new("", "ignored"),
        ];
        let wire = build(&req, &WireContext::default()).unwrap();
        assert_eq!(wire.headers, vec![("Accept".into(), "application/json".into())]);
    }

    #[test]
    fn invalid_header_names_and_values_are_rejected() {
        let mut req = request("GET", "https://a.example/");
        req.headers = vec![KeyValue::new("Bad Name", "x")];
        assert!(matches!(
            build(&req, &WireContext::default()),
            Err(BuildError::InvalidHeaderName(_))
        ));
        req.headers = vec![KeyValue::new("X-Inject", "a\r\nSet-Cookie: x")];
        assert!(matches!(
            build(&req, &WireContext::default()),
            Err(BuildError::InvalidHeaderValue(_))
        ));
    }

    #[test]
    fn content_type_is_derived_from_body_type() {
        let cases = [
            (json("{}"), "application/json"),
            (
                Body::Xml {
                    content: "<a/>".into(),
                    extra: Extra::new(),
                },
                "application/xml",
            ),
            (
                Body::Text {
                    content: "hi".into(),
                    extra: Extra::new(),
                },
                "text/plain; charset=utf-8",
            ),
            (
                Body::FormUrlencoded {
                    fields: vec![],
                    extra: Extra::new(),
                },
                "application/x-www-form-urlencoded",
            ),
        ];
        for (body, expected) in cases {
            let mut req = request("POST", "https://a.example/");
            req.body = body;
            assert_eq!(
                effective(&req, &WireContext::default()),
                vec![("Content-Type".to_string(), expected.to_string())]
            );
            let wire = build(&req, &WireContext::default()).unwrap();
            assert_eq!(wire.headers.last().unwrap().1, expected);
        }
    }

    #[test]
    fn user_content_type_wins() {
        let mut req = request("POST", "https://a.example/");
        req.body = json("{}");
        req.headers = vec![KeyValue::new("content-type", "application/vnd.api+json")];
        assert!(effective(&req, &WireContext::default()).is_empty());
        let wire = build(&req, &WireContext::default()).unwrap();
        assert_eq!(
            wire.headers,
            vec![("content-type".into(), "application/vnd.api+json".into())]
        );
    }

    #[test]
    fn disabled_user_content_type_does_not_win() {
        let mut req = request("POST", "https://a.example/");
        req.body = json("{}");
        req.headers = vec![KeyValue {
            enabled: false,
            ..KeyValue::new("Content-Type", "text/plain")
        }];
        assert_eq!(effective(&req, &WireContext::default())[0].1, "application/json");
    }

    #[test]
    fn json_body_is_sent_verbatim_even_if_invalid() {
        let mut req = request("POST", "https://a.example/");
        req.body = json("{ not json");
        let wire = build(&req, &WireContext::default()).unwrap();
        assert_eq!(wire.body, WireBody::Bytes(b"{ not json".to_vec()));
    }

    #[test]
    fn form_body_is_urlencoded() {
        let mut req = request("POST", "https://a.example/");
        req.body = Body::FormUrlencoded {
            fields: vec![
                KeyValue::new("name", "Ada Lovelace"),
                KeyValue::new("note", "a&b=c"),
                KeyValue {
                    enabled: false,
                    ..KeyValue::new("off", "1")
                },
            ],
            extra: Extra::new(),
        };
        let wire = build(&req, &WireContext::default()).unwrap();
        assert_eq!(
            wire.body,
            WireBody::Bytes(b"name=Ada+Lovelace&note=a%26b%3Dc".to_vec())
        );
    }

    #[test]
    fn url_is_built_from_path_params_and_params() {
        let mut req = request("GET", "https://a.example/users/:id");
        req.path_params = vec![PathParam {
            name: "id".into(),
            value: "4 2".into(),
            extra: Extra::new(),
        }];
        req.params = vec![KeyValue::new("q", "x y")];
        let wire = build(&req, &WireContext::default()).unwrap();
        assert_eq!(wire.url, "https://a.example/users/4%202?q=x%20y");
    }

    #[test]
    fn encode_url_false_sends_raw() {
        let mut req = request("GET", "https://a.example/a b");
        req.settings.encode_url = Some(false);
        let wire = build(&req, &WireContext::default()).unwrap();
        assert_eq!(wire.url, "https://a.example/a b");
    }

    #[test]
    fn url_errors_are_reported() {
        assert!(matches!(
            build(&request("GET", "example.com"), &WireContext::default()),
            Err(BuildError::Url(UrlError::MissingScheme(_)))
        ));
    }
}
