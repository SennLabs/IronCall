//! Reads Swagger 2.0 and OpenAPI 3.x documents into one version-neutral model.

use serde_json::{Map, Value};

use super::refs::{Doc, pointer_segment};

const METHODS: &[&str] = &[
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Everything the builder needs, independent of the spec version.
pub(super) struct Spec {
    pub title: String,
    pub description: Option<String>,
    /// Server URLs with variables expanded to their defaults.
    pub servers: Vec<String>,
    /// Server variables (name, default), for the environment.
    pub server_vars: Vec<(String, String)>,
    /// Tag order and descriptions from the top-level `tags` array.
    pub tags: Vec<(String, Option<String>)>,
    pub schemes: Vec<(String, Scheme)>,
    /// Global `security`: `None` if absent, `Some(None)` for `[]` (no auth), else the
    /// first scheme of the first requirement.
    pub security: Option<Option<String>>,
    pub operations: Vec<Operation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Scheme {
    Basic,
    Bearer,
    /// OAuth2 / OpenID Connect: sent as a Bearer token (flows are post-V1).
    OAuth,
    ApiKeyHeader(String),
    ApiKeyQuery(String),
    ApiKeyCookie(String),
    /// Something we can't map (e.g. HTTP digest, mutual TLS).
    Unsupported(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ParamIn {
    Path,
    Query,
    Header,
    Cookie,
}

pub(super) struct Param {
    pub name: String,
    pub location: ParamIn,
    pub required: bool,
    pub value: Option<String>,
}

pub(super) enum BodySpec {
    /// A text body (JSON, XML, plain…) with its media type and example, if any.
    Text {
        media_type: String,
        example: Option<String>,
    },
    /// Form fields: (name, example value, is a file).
    Form {
        multipart: bool,
        fields: Vec<(String, Option<String>, bool)>,
    },
}

pub(super) struct Operation {
    pub method: String,
    pub path: String,
    /// Where it is in the spec, for warnings.
    pub location: String,
    pub tag: Option<String>,
    pub summary: Option<String>,
    pub operation_id: Option<String>,
    pub description: Option<String>,
    pub deprecated: bool,
    pub params: Vec<Param>,
    pub body: Option<BodySpec>,
    /// Like [`Spec::security`], for this operation only.
    pub security: Option<Option<String>>,
}

fn text(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// An example value as request text: strings as-is, everything else as JSON.
fn example_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Pretty JSON for a JSON body example (strings that already contain JSON are kept).
fn json_body(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

fn security_choice(doc: &Doc<'_>, v: Option<&Value>, location: &str) -> Option<Option<String>> {
    let reqs = v?.as_array()?;
    let Some(first) = reqs.first() else {
        return Some(None);
    };
    let obj = first.as_object()?;
    if obj.is_empty() {
        // `security: [{}]` makes auth optional; treat as no auth.
        return Some(None);
    }
    if obj.len() > 1 || reqs.len() > 1 {
        doc.warn(
            "several security options are allowed here; only the first one is used",
            Some(location),
        );
    }
    obj.keys().next().map(|k| Some(k.clone()))
}

fn tags(doc: &Doc<'_>) -> Vec<(String, Option<String>)> {
    doc.root
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(|t| Some((text(t.get("name"))?, text(t.get("description")))))
                .collect()
        })
        .unwrap_or_default()
}

fn info(doc: &Doc<'_>) -> (String, Option<String>) {
    let info = doc.root.get("info");
    let title = text(info.and_then(|i| i.get("title"))).unwrap_or_else(|| "Imported API".into());
    (title, text(info.and_then(|i| i.get("description"))))
}

/// Merges path-level and operation-level parameters (operation wins by name + location).
fn merged_params<'a>(doc: &Doc<'a>, item: &'a Value, op: &'a Value) -> Vec<&'a Value> {
    let mut out: Vec<&'a Value> = Vec::new();
    for list in [item.get("parameters"), op.get("parameters")] {
        for p in list.and_then(Value::as_array).into_iter().flatten() {
            let Some(p) = doc.deref(p) else { continue };
            let key = (p.get("name"), p.get("in"));
            out.retain(|q| (q.get("name"), q.get("in")) != key);
            out.push(p);
        }
    }
    out
}

/// An example for a parameter: `example`, the first of `examples`, the schema's
/// `example`, then `default` (on the parameter or its schema).
fn param_example(doc: &Doc<'_>, p: &Value) -> Option<String> {
    let schema = p.get("schema").and_then(|s| doc.deref(s));
    p.get("example")
        .or_else(|| first_example(doc, p.get("examples")))
        .or_else(|| schema.and_then(|s| s.get("example")))
        .or_else(|| p.get("x-example"))
        .or_else(|| p.get("default"))
        .or_else(|| schema.and_then(|s| s.get("default")))
        .filter(|v| !v.is_null())
        .map(example_text)
}

/// The first value of an OpenAPI 3 `examples` map (`{name: {value: …}}`, possibly refs).
fn first_example<'a>(doc: &Doc<'a>, examples: Option<&'a Value>) -> Option<&'a Value> {
    let (_, first) = examples?.as_object()?.iter().next()?;
    let first = doc.deref(first)?;
    first.get("value").or_else(|| {
        if first.get("externalValue").is_some() {
            None
        } else {
            Some(first)
        }
    })
}

fn param_location(s: &str) -> Option<ParamIn> {
    Some(match s {
        "path" => ParamIn::Path,
        "query" => ParamIn::Query,
        "header" => ParamIn::Header,
        "cookie" => ParamIn::Cookie,
        _ => return None,
    })
}

/// Form fields from an object schema's properties (example or default as the value).
fn schema_fields(
    doc: &Doc<'_>,
    schema: Option<&Value>,
    example: Option<&Value>,
) -> Vec<(String, Option<String>, bool)> {
    let Some(schema) = schema.and_then(|s| doc.deref(s)) else {
        return Vec::new();
    };
    let mut props: Map<String, Value> = Map::new();
    // allOf members contribute properties too (one level).
    for part in schema
        .get("allOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(p) = doc.get(part, "properties").and_then(Value::as_object) {
            props.extend(p.clone());
        }
    }
    if let Some(p) = schema.get("properties").and_then(Value::as_object) {
        props.extend(p.clone());
    }
    props
        .iter()
        .map(|(name, prop)| {
            let prop = doc.deref(prop).unwrap_or(prop);
            let is_file = prop.get("format").and_then(Value::as_str) == Some("binary")
                || prop.get("type").and_then(Value::as_str) == Some("file")
                || prop.get("contentMediaType").is_some();
            let value = example
                .and_then(|e| e.get(name))
                .or_else(|| prop.get("example"))
                .or_else(|| prop.get("default"))
                .filter(|_| !is_file)
                .map(example_text);
            (name.clone(), value, is_file)
        })
        .collect()
}

/// Picks the media type to use: JSON first, then forms, XML, text, anything else.
fn pick_media_type<'a>(types: impl Iterator<Item = &'a str> + Clone) -> Option<&'a str> {
    let rank = |t: &str| {
        let t = t.split(';').next().unwrap_or(t).trim().to_ascii_lowercase();
        if t == "application/json" {
            0
        } else if t.ends_with("+json") || t.ends_with("/json") {
            1
        } else if t == "application/x-www-form-urlencoded" {
            2
        } else if t == "multipart/form-data" {
            3
        } else if t.ends_with("/xml") || t.ends_with("+xml") {
            4
        } else if t.starts_with("text/") {
            5
        } else {
            6
        }
    };
    types.min_by_key(|t| rank(t))
}

fn is_form(media_type: &str) -> Option<bool> {
    match media_type.split(';').next().unwrap_or("").trim() {
        "application/x-www-form-urlencoded" => Some(false),
        "multipart/form-data" => Some(true),
        _ => None,
    }
}

fn is_json(media_type: &str) -> bool {
    let t = media_type.split(';').next().unwrap_or("").trim();
    t.ends_with("/json") || t.ends_with("+json")
}

// ---------------------------------------------------------------------------------
// OpenAPI 3.x

pub(super) fn read_openapi3(doc: &Doc<'_>) -> Spec {
    let (title, description) = info(doc);
    let (servers, server_vars) = servers3(doc);
    let schemes = doc
        .root
        .pointer("/components/securitySchemes")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(name, s)| Some((name.clone(), scheme3(doc.deref(s)?))))
                .collect()
        })
        .unwrap_or_default();
    let security = security_choice(doc, doc.root.get("security"), "#/security");
    let operations = operations(doc, body3);
    Spec {
        title,
        description,
        servers,
        server_vars,
        tags: tags(doc),
        schemes,
        security,
        operations,
    }
}

fn servers3(doc: &Doc<'_>) -> (Vec<String>, Vec<(String, String)>) {
    let mut urls = Vec::new();
    let mut vars: Vec<(String, String)> = Vec::new();
    for s in doc
        .root
        .get("servers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(mut url) = text(s.get("url")) else {
            continue;
        };
        for (name, var) in s
            .get("variables")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let default = var.get("default").map(example_text).unwrap_or_default();
            url = url.replace(&format!("{{{name}}}"), &default);
            if !vars.iter().any(|(n, _)| n == name) {
                vars.push((name.clone(), default));
            }
        }
        let url = url.trim_end_matches('/').to_string();
        if !urls.contains(&url) {
            urls.push(url);
        }
    }
    (urls, vars)
}

fn scheme3(s: &Value) -> Scheme {
    let kind = s.get("type").and_then(Value::as_str).unwrap_or_default();
    match kind {
        "http" => match s
            .get("scheme")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "basic" => Scheme::Basic,
            "bearer" => Scheme::Bearer,
            other => Scheme::Unsupported(format!("HTTP {other}")),
        },
        "apiKey" => api_key(s),
        "oauth2" | "openIdConnect" => Scheme::OAuth,
        other => Scheme::Unsupported(other.to_string()),
    }
}

fn api_key(s: &Value) -> Scheme {
    let name = text(s.get("name")).unwrap_or_else(|| "X-API-Key".into());
    match s.get("in").and_then(Value::as_str) {
        Some("query") => Scheme::ApiKeyQuery(name),
        Some("cookie") => Scheme::ApiKeyCookie(name),
        _ => Scheme::ApiKeyHeader(name),
    }
}

fn body3(doc: &Doc<'_>, op: &Value, location: &str) -> Option<BodySpec> {
    let body = doc.deref(op.get("requestBody")?)?;
    let content = body.get("content").and_then(Value::as_object)?;
    let media_type = pick_media_type(content.keys().map(String::as_str))?;
    let media = doc.deref(&content[media_type])?;
    let schema = media.get("schema");
    let example = media
        .get("example")
        .or_else(|| first_example(doc, media.get("examples")))
        .or_else(|| schema.and_then(|s| doc.deref(s)).and_then(|s| s.get("example")));
    if let Some(multipart) = is_form(media_type) {
        return Some(BodySpec::Form {
            multipart,
            fields: schema_fields(doc, schema, example),
        });
    }
    if media.get("examples").is_some() && example.is_none() {
        doc.warn(
            "example uses `externalValue`, which isn't fetched",
            Some(location),
        );
    }
    Some(BodySpec::Text {
        media_type: media_type.to_string(),
        example: example.map(|e| {
            if is_json(media_type) {
                json_body(e)
            } else {
                example_text(e)
            }
        }),
    })
}

// ---------------------------------------------------------------------------------
// Swagger 2.0

pub(super) fn read_swagger2(doc: &Doc<'_>) -> Spec {
    let (title, description) = info(doc);
    let host = text(doc.root.get("host"));
    let base = text(doc.root.get("basePath")).unwrap_or_default();
    let base = base.trim_end_matches('/');
    let mut schemes_list: Vec<String> = doc
        .root
        .get("schemes")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|s| text(Some(s))).collect())
        .unwrap_or_default();
    // Prefer HTTPS when both are offered.
    schemes_list.sort_by_key(|s| s != "https");
    let servers = match host {
        Some(host) if schemes_list.is_empty() => vec![format!("https://{host}{base}")],
        Some(host) => schemes_list
            .iter()
            .map(|s| format!("{s}://{host}{base}"))
            .collect(),
        None => vec![base.to_string()],
    };
    let schemes = doc
        .root
        .get("securityDefinitions")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(name, s)| {
                    let scheme = match s.get("type").and_then(Value::as_str).unwrap_or_default() {
                        "basic" => Scheme::Basic,
                        "apiKey" => api_key(s),
                        "oauth2" => Scheme::OAuth,
                        other => Scheme::Unsupported(other.to_string()),
                    };
                    (name.clone(), scheme)
                })
                .collect()
        })
        .unwrap_or_default();
    let security = security_choice(doc, doc.root.get("security"), "#/security");
    let consumes = doc.root.get("consumes").cloned();
    let operations = operations(doc, move |doc, op, _| body2(doc, op, consumes.as_ref()));
    Spec {
        title,
        description,
        servers,
        server_vars: Vec::new(),
        tags: tags(doc),
        schemes,
        security,
        operations,
    }
}

/// Swagger 2 bodies: an `in: body` parameter, or `in: formData` parameters.
fn body2(doc: &Doc<'_>, op: &Value, global_consumes: Option<&Value>) -> Option<BodySpec> {
    let consumes: Vec<&str> = op
        .get("consumes")
        .or(global_consumes)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let params: Vec<&Value> = op
        .get("parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| doc.deref(p))
        .collect();
    if let Some(body) = params
        .iter()
        .find(|p| p.get("in").and_then(Value::as_str) == Some("body"))
    {
        let media_type = pick_media_type(consumes.iter().copied()).unwrap_or("application/json");
        let schema = body.get("schema").and_then(|s| doc.deref(s));
        let example = body
            .get("x-example")
            .or_else(|| schema.and_then(|s| s.get("example")))
            .or_else(|| {
                op.pointer("/x-examples")
                    .and_then(|e| e.as_object()?.values().next())
            });
        return Some(BodySpec::Text {
            media_type: media_type.to_string(),
            example: example.map(|e| {
                if is_json(media_type) {
                    json_body(e)
                } else {
                    example_text(e)
                }
            }),
        });
    }
    let form: Vec<&&Value> = params
        .iter()
        .filter(|p| p.get("in").and_then(Value::as_str) == Some("formData"))
        .collect();
    if form.is_empty() {
        return None;
    }
    let fields: Vec<_> = form
        .iter()
        .filter_map(|p| {
            let name = text(p.get("name"))?;
            let is_file = p.get("type").and_then(Value::as_str) == Some("file");
            let value = (!is_file).then(|| param_example(doc, p)).flatten();
            Some((name, value, is_file))
        })
        .collect();
    let multipart = fields.iter().any(|(_, _, f)| *f) || consumes.contains(&"multipart/form-data");
    Some(BodySpec::Form { multipart, fields })
}

// ---------------------------------------------------------------------------------
// Shared

fn operations(doc: &Doc<'_>, body: impl Fn(&Doc<'_>, &Value, &str) -> Option<BodySpec>) -> Vec<Operation> {
    let mut out = Vec::new();
    let Some(paths) = doc.root.get("paths").and_then(Value::as_object) else {
        doc.warn("the spec has no `paths`", Some("#/paths"));
        return out;
    };
    for (path, item) in paths {
        let Some(item) = doc.deref(item) else { continue };
        let Some(item_obj) = item.as_object() else {
            continue;
        };
        for (method, op) in item_obj {
            if !METHODS.contains(&method.as_str()) {
                continue;
            }
            let location = format!("#/paths/{}/{method}", pointer_segment(path));
            let Some(op) = doc.deref(op).filter(|o| o.is_object()) else {
                doc.warn("operation isn't an object; skipped", Some(&location));
                continue;
            };
            let mut params = Vec::new();
            for p in merged_params(doc, item, op) {
                let Some(name) = text(p.get("name")) else { continue };
                let Some(loc) = p.get("in").and_then(Value::as_str).and_then(param_location) else {
                    continue; // body/formData (Swagger 2) are handled as the body
                };
                if loc == ParamIn::Header
                    && ["accept", "content-type", "authorization"]
                        .contains(&name.to_ascii_lowercase().as_str())
                {
                    continue; // ignored by OpenAPI; set from the body and auth instead
                }
                params.push(Param {
                    required: loc == ParamIn::Path
                        || p.get("required").and_then(Value::as_bool).unwrap_or(false),
                    value: param_example(doc, p),
                    name,
                    location: loc,
                });
            }
            let body = body(doc, op, &location);
            out.push(Operation {
                method: method.to_ascii_uppercase(),
                path: path.clone(),
                tag: op
                    .get("tags")
                    .and_then(Value::as_array)
                    .and_then(|t| t.first())
                    .and_then(|t| text(Some(t))),
                summary: text(op.get("summary")),
                operation_id: text(op.get("operationId")),
                description: text(op.get("description")),
                deprecated: op.get("deprecated").and_then(Value::as_bool).unwrap_or(false),
                params,
                body,
                security: security_choice(doc, op.get("security"), &location),
                location,
            });
        }
    }
    out
}
