//! Builds the [`Plan`] (folders, requests, environment, auth) from the neutral model.

use ironcall_core::ItemId;
use ironcall_core::model::{
    ApiKeyLocation, Auth, Body, Extra, KeyValue, MultipartKind, MultipartPart, PathParam, RequestFile,
};

use super::ops::{BodySpec, Operation, ParamIn, Scheme, Spec};
use super::{EnvVar, Plan, PlanItem, SpecVersion, Warning};

/// Variables an auth scheme needs: (name, secret).
fn scheme_vars(scheme: &Scheme, api_key_var: &str) -> Vec<(String, bool)> {
    match scheme {
        Scheme::Basic => vec![("username".into(), false), ("password".into(), true)],
        Scheme::Bearer | Scheme::OAuth => vec![("token".into(), true)],
        Scheme::ApiKeyHeader(_) | Scheme::ApiKeyQuery(_) => vec![(api_key_var.into(), true)],
        Scheme::ApiKeyCookie(_) | Scheme::Unsupported(_) => Vec::new(),
    }
}

/// A variable name from a scheme name: `api_key` → `api_key`, `X-Key` → `XKey`.
fn var_name(scheme: &str) -> String {
    let name: String = scheme
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() { "apiKey".into() } else { name }
}

struct AuthMap {
    /// (scheme name, scheme, variable name for API keys)
    schemes: Vec<(String, Scheme, String)>,
}

impl AuthMap {
    fn new(schemes: Vec<(String, Scheme)>) -> Self {
        let mut first_key = true;
        let schemes = schemes
            .into_iter()
            .map(|(name, scheme)| {
                let var = if matches!(scheme, Scheme::ApiKeyHeader(_) | Scheme::ApiKeyQuery(_)) {
                    if std::mem::take(&mut first_key) {
                        "apiKey".to_string()
                    } else {
                        var_name(&name)
                    }
                } else {
                    String::new()
                };
                (name, scheme, var)
            })
            .collect();
        Self { schemes }
    }

    fn find(&self, name: &str) -> Option<&(String, Scheme, String)> {
        self.schemes.iter().find(|(n, _, _)| n == name)
    }

    /// The auth for a scheme name, or `None` if it can't be expressed.
    fn auth(&self, name: &str) -> Option<Auth> {
        let (_, scheme, var) = self.find(name)?;
        let extra = Extra::default;
        Some(match scheme {
            Scheme::Basic => Auth::Basic {
                username: "{{username}}".into(),
                password: "{{password}}".into(),
                extra: extra(),
            },
            Scheme::Bearer | Scheme::OAuth => Auth::Bearer {
                token: "{{token}}".into(),
                extra: extra(),
            },
            Scheme::ApiKeyHeader(key) => Auth::ApiKey {
                key: key.clone(),
                value: format!("{{{{{var}}}}}"),
                location: ApiKeyLocation::Header,
                extra: extra(),
            },
            Scheme::ApiKeyQuery(key) => Auth::ApiKey {
                key: key.clone(),
                value: format!("{{{{{var}}}}}"),
                location: ApiKeyLocation::Query,
                extra: extra(),
            },
            Scheme::ApiKeyCookie(_) | Scheme::Unsupported(_) => return None,
        })
    }

    fn summary(&self, name: &str, target: &str) -> String {
        match self.find(name) {
            Some((_, Scheme::Basic, _)) => {
                format!("{name} → Basic auth ({{{{username}}}}, {{{{password}}}}) on {target}")
            }
            Some((_, Scheme::Bearer, _)) => format!("{name} → Bearer {{{{token}}}} on {target}"),
            Some((_, Scheme::OAuth, _)) => {
                format!("{name} → Bearer {{{{token}}}} on {target} (OAuth flows are post-V1)")
            }
            Some((_, Scheme::ApiKeyHeader(k), v)) => {
                format!("{name} → API key header `{k}: {{{{{v}}}}}` on {target}")
            }
            Some((_, Scheme::ApiKeyQuery(k), v)) => {
                format!("{name} → API key query `{k}={{{{{v}}}}}` on {target}")
            }
            Some((_, Scheme::ApiKeyCookie(k), _)) => format!("{name} → not mapped (API key in cookie `{k}`)"),
            Some((_, Scheme::Unsupported(kind), _)) => format!("{name} → not mapped ({kind})"),
            None => format!("{name} → not defined in the spec"),
        }
    }
}

pub(super) fn build(
    version: SpecVersion,
    spec: Spec,
    mut warnings: Vec<Warning>,
    next_id: &mut dyn FnMut() -> ItemId,
) -> Plan {
    let warn = |warnings: &mut Vec<Warning>, message: String, location: Option<String>| {
        warnings.push(Warning { message, location });
    };
    let auth_map = AuthMap::new(spec.schemes.clone());

    // --- Security: which schemes are used, and where.
    let mut used: Vec<String> = Vec::new();
    let mut auth_summary = Vec::new();
    let root_scheme = spec.security.clone().flatten();
    let root_auth = match &spec.security {
        None => None,
        Some(None) => Some(Auth::None {
            extra: Extra::default(),
        }),
        Some(Some(name)) => {
            used.push(name.clone());
            auth_summary.push(auth_map.summary(name, "the collection"));
            match auth_map.auth(name) {
                Some(a) => Some(a),
                None => {
                    warn(
                        &mut warnings,
                        format!("security scheme `{name}` can't be mapped to an auth type; set auth by hand"),
                        Some("#/security".into()),
                    );
                    None
                }
            }
        }
    };

    // --- Requests, grouped into folders by first tag.
    let mut folders: Vec<(String, Option<String>, Vec<PlanItem>)> = spec
        .tags
        .iter()
        .map(|(name, desc)| (name.clone(), desc.clone(), Vec::new()))
        .collect();
    let mut root_items: Vec<PlanItem> = Vec::new();
    for op in &spec.operations {
        let auth = match &op.security {
            None => Auth::Inherit,
            Some(None) => Auth::None {
                extra: Extra::default(),
            },
            Some(Some(name)) if Some(name) == root_scheme.as_ref() => Auth::Inherit,
            Some(Some(name)) => {
                if !used.contains(name) {
                    used.push(name.clone());
                    auth_summary.push(auth_map.summary(name, "the requests that use it"));
                }
                auth_map.auth(name).unwrap_or_else(|| {
                    warn(
                        &mut warnings,
                        format!("security scheme `{name}` can't be mapped to an auth type"),
                        Some(op.location.clone()),
                    );
                    Auth::Inherit
                })
            }
        };
        let request = request(op, auth, next_id, &mut warnings);
        let item = PlanItem::Request(Box::new(request));
        match &op.tag {
            Some(tag) => match folders.iter_mut().find(|(n, _, _)| n == tag) {
                Some((_, _, items)) => items.push(item),
                None => folders.push((tag.clone(), None, vec![item])),
            },
            None => root_items.push(item),
        }
    }
    let mut tree: Vec<PlanItem> = folders
        .into_iter()
        .filter(|(_, _, items)| !items.is_empty())
        .map(|(name, description, items)| PlanItem::Folder {
            name,
            description,
            items,
        })
        .collect();
    tree.extend(root_items);

    // --- Environment: baseUrl, server variables, auth variables.
    let mut environment = Vec::new();
    match spec.servers.first() {
        Some(url) => {
            if !url.contains("://") {
                warn(
                    &mut warnings,
                    format!("the server URL `{url}` is relative; set `baseUrl` to the full URL"),
                    Some("#/servers".into()),
                );
            }
            environment.push(EnvVar {
                name: "baseUrl".into(),
                value: url.clone(),
                secret: false,
            });
        }
        None => {
            warn(
                &mut warnings,
                "the spec lists no servers; set `baseUrl` in the environment".into(),
                Some("#/servers".into()),
            );
            environment.push(EnvVar {
                name: "baseUrl".into(),
                value: String::new(),
                secret: false,
            });
        }
    }
    for (name, value) in &spec.server_vars {
        environment.push(EnvVar {
            name: name.clone(),
            value: value.clone(),
            secret: false,
        });
    }
    for name in &used {
        let Some((_, scheme, var)) = auth_map.find(name) else {
            warn(
                &mut warnings,
                format!("security scheme `{name}` is used but not defined"),
                None,
            );
            continue;
        };
        if let Scheme::ApiKeyCookie(k) = scheme {
            warn(
                &mut warnings,
                format!("`{name}` sends an API key in cookie `{k}`; add a `Cookie` header by hand"),
                None,
            );
        }
        if let Scheme::Unsupported(kind) = scheme {
            warn(
                &mut warnings,
                format!("`{name}` uses {kind}, which isn't supported yet"),
                None,
            );
        }
        if *scheme == Scheme::OAuth {
            warn(
                &mut warnings,
                format!(
                    "`{name}` is OAuth 2.0 / OpenID Connect: sent as a Bearer `{{{{token}}}}`; getting the token is up to you until OAuth support (post-V1)"
                ),
                None,
            );
        }
        for (var_name, secret) in scheme_vars(scheme, var) {
            if !environment.iter().any(|e| e.name == var_name) {
                environment.push(EnvVar {
                    name: var_name,
                    value: String::new(),
                    secret,
                });
            }
        }
    }

    Plan {
        version,
        title: spec.title,
        description: spec.description,
        servers: spec.servers,
        environment,
        root_auth,
        auth_summary,
        tree,
        warnings,
    }
}

/// `/pets/{petId}` → `/pets/:petId`.
fn convert_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        match rest[start..].find('}') {
            Some(end) => {
                out.push(':');
                out.push_str(&rest[start + 1..start + end]);
                rest = &rest[start + end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn request(
    op: &Operation,
    auth: Auth,
    next_id: &mut dyn FnMut() -> ItemId,
    warnings: &mut Vec<Warning>,
) -> RequestFile {
    let label = format!("{} {}", op.method, op.path);
    let name = op
        .summary
        .clone()
        .or_else(|| op.operation_id.clone())
        .unwrap_or_else(|| label.clone());
    let mut req = RequestFile::new(name);
    req.id = next_id();
    req.method = op.method.clone();
    let path = if op.path.starts_with('/') {
        op.path.clone()
    } else {
        format!("/{}", op.path)
    };
    req.url = format!("{{{{baseUrl}}}}{}", convert_path(&path));
    req.auth = auth;

    let mut description: Vec<String> = Vec::new();
    if op.deprecated {
        description.push("**Deprecated.**".into());
    }
    if let Some(d) = &op.description {
        description.push(d.clone());
    } else if op.summary.is_some()
        && let Some(id) = &op.operation_id
    {
        description.push(format!("Operation `{id}`."));
    }

    // Path params in the order they appear in the path.
    let names = ironcall_core::url::path_param_names(&req.url);
    req.path_params = names
        .into_iter()
        .map(|n| PathParam {
            value: op
                .params
                .iter()
                .find(|p| p.location == ParamIn::Path && p.name == n)
                .and_then(|p| p.value.clone())
                .unwrap_or_default(),
            name: n,
            extra: Extra::default(),
        })
        .collect();

    let mut cookies = Vec::new();
    for p in &op.params {
        let row = || KeyValue {
            enabled: p.required,
            ..KeyValue::new(p.name.clone(), p.value.clone().unwrap_or_default())
        };
        match p.location {
            ParamIn::Path => {}
            ParamIn::Query => req.params.push(row()),
            ParamIn::Header => req.headers.push(row()),
            ParamIn::Cookie => cookies.push(format!("{}={}", p.name, p.value.clone().unwrap_or_default())),
        }
    }
    if !cookies.is_empty() {
        req.headers.push(KeyValue::new("Cookie", cookies.join("; ")));
        warnings.push(Warning {
            message: "cookie parameters were combined into a `Cookie` header".into(),
            location: Some(label.clone()),
        });
    }

    match &op.body {
        None => {}
        Some(BodySpec::Form {
            multipart: false,
            fields,
        }) => {
            req.body = Body::FormUrlencoded {
                fields: fields
                    .iter()
                    .map(|(n, v, _)| KeyValue::new(n.clone(), v.clone().unwrap_or_default()))
                    .collect(),
                extra: Extra::default(),
            };
        }
        Some(BodySpec::Form {
            multipart: true,
            fields,
        }) => {
            req.body = Body::Multipart {
                parts: fields
                    .iter()
                    .map(|(n, v, is_file)| MultipartPart {
                        name: n.clone(),
                        kind: if *is_file {
                            MultipartKind::File
                        } else {
                            MultipartKind::Text
                        },
                        value: if *is_file {
                            None
                        } else {
                            Some(v.clone().unwrap_or_default())
                        },
                        path: None,
                        content_type: None,
                        enabled: true,
                        extra: Extra::default(),
                    })
                    .collect(),
                extra: Extra::default(),
            };
        }
        Some(BodySpec::Text { media_type, example }) => {
            let base = media_type
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            let content = example.clone();
            if base.ends_with("/json") || base.ends_with("+json") {
                if content.is_none() {
                    description.push("The spec has no example body; fill it in.".into());
                }
                req.body = Body::Json {
                    content: content.unwrap_or_else(|| "{}".into()),
                    extra: Extra::default(),
                };
                if base != "application/json" {
                    req.headers
                        .push(KeyValue::new("Content-Type", media_type.clone()));
                }
            } else if base.ends_with("/xml") || base.ends_with("+xml") {
                req.body = Body::Xml {
                    content: content.unwrap_or_default(),
                    extra: Extra::default(),
                };
                if base != "application/xml" {
                    req.headers
                        .push(KeyValue::new("Content-Type", media_type.clone()));
                }
            } else {
                req.body = Body::Text {
                    content: content.unwrap_or_default(),
                    extra: Extra::default(),
                };
                if base != "text/plain" {
                    req.headers
                        .push(KeyValue::new("Content-Type", media_type.clone()));
                    if !base.starts_with("text/") {
                        warnings.push(Warning {
                            message: format!("`{media_type}` bodies are imported as text; switch to a binary file body if needed"),
                            location: Some(label.clone()),
                        });
                    }
                }
            }
        }
    }
    if !description.is_empty() {
        req.description = Some(description.join("\n\n"));
    }
    req
}

#[cfg(test)]
mod tests {
    use super::convert_path;

    #[test]
    fn converts_path_templates() {
        assert_eq!(convert_path("/pets/{petId}"), "/pets/:petId");
        assert_eq!(convert_path("/a/{b}/c/{d}"), "/a/:b/c/:d");
        assert_eq!(convert_path("/broken/{x"), "/broken/{x");
        assert_eq!(convert_path("/plain"), "/plain");
    }
}
