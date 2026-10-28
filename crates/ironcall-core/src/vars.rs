//! Variables and templating (PLAN.md §5).
//!
//! A [`Resolver`] holds variable [`Layer`]s, one per scope. Precedence, lowest to
//! highest: Global < Collection < Environment < Folder (outermost first) < Request <
//! Runtime; the highest layer that defines a name wins. `{{name}}` references are
//! replaced with values, which may themselves contain references (up to
//! [`MAX_DEPTH`] levels, with cycle detection). `{{$helper}}` references produce a
//! fresh dynamic value per occurrence.
//!
//! Secret values are supplied by the caller (from a secret store) and kept in
//! [`SecretString`]s; everything that substitutes a secret also returns it so callers
//! can redact it from logs, history and previews (see [`crate::redact::mask_values`]).

use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

use secrecy::{ExposeSecret, SecretString};

use crate::ItemId;
use crate::model::{Auth, Body, KeyValue, MultipartKind, RequestFile, Variable, Vars};
use crate::recent::rfc3339_utc;
use crate::redact;

/// How deep `{{refs}}` inside variable values are expanded.
pub const MAX_DEPTH: usize = 10;

/// A variable scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Global,
    Collection,
    Environment {
        id: ItemId,
        name: String,
    },
    Folder {
        id: ItemId,
        name: String,
    },
    Request,
    /// Set by `extract` during a session or run (PLAN.md Phase 7).
    Runtime,
    /// A built-in `{{$helper}}`. Never a layer.
    Dynamic,
}

impl Source {
    /// Precedence rank: higher wins. Folders share one rank; their layer order (outer
    /// before inner) decides between them.
    fn rank(&self) -> u8 {
        match self {
            Source::Global => 0,
            Source::Collection => 1,
            Source::Environment { .. } => 2,
            Source::Folder { .. } => 3,
            Source::Request => 4,
            Source::Runtime => 5,
            Source::Dynamic => 6,
        }
    }
}

/// One variable definition.
#[derive(Clone, Debug)]
pub enum Def {
    Plain {
        value: String,
        description: Option<String>,
    },
    /// A secret variable. `value` is `None` if nothing is stored for it on this machine.
    Secret {
        value: Option<SecretString>,
        description: Option<String>,
    },
}

impl Def {
    fn is_secret(&self) -> bool {
        matches!(self, Def::Secret { .. })
    }

    fn description(&self) -> Option<&str> {
        match self {
            Def::Plain { description, .. } | Def::Secret { description, .. } => description.as_deref(),
        }
    }
}

/// The variables defined at one scope, in file order.
#[derive(Clone, Debug)]
pub struct Layer {
    pub source: Source,
    pub vars: Vec<(String, Def)>,
}

impl Layer {
    pub fn new(source: Source) -> Self {
        Self {
            source,
            vars: Vec::new(),
        }
    }

    /// Builds a layer from file variables. `secret` looks up the stored value of each
    /// variable declared `secret: true` (by name); core never touches a secret store.
    pub fn from_vars(
        source: Source,
        vars: &Vars,
        mut secret: impl FnMut(&str) -> Option<SecretString>,
    ) -> Self {
        let vars = vars
            .iter()
            .map(|(name, var)| {
                let def = match var {
                    Variable::Plain(value) => Def::Plain {
                        value: value.clone(),
                        description: None,
                    },
                    Variable::Detailed(spec) if spec.secret => Def::Secret {
                        value: secret(name),
                        description: spec.description.clone(),
                    },
                    Variable::Detailed(spec) => Def::Plain {
                        value: spec.value.clone().unwrap_or_default(),
                        description: spec.description.clone(),
                    },
                };
                (name.clone(), def)
            })
            .collect();
        Self { source, vars }
    }

    pub fn with(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.vars.push((
            name.into(),
            Def::Plain {
                value: value.into(),
                description: None,
            },
        ));
        self
    }

    pub fn with_secret(mut self, name: impl Into<String>, value: Option<&str>) -> Self {
        self.vars.push((
            name.into(),
            Def::Secret {
                value: value.map(|v| SecretString::from(v.to_string())),
                description: None,
            },
        ));
        self
    }

    fn get(&self, name: &str) -> Option<&Def> {
        // A later duplicate in the same file wins, like a YAML map would.
        self.vars.iter().rev().find(|(n, _)| n == name).map(|(_, d)| d)
    }
}

/// One `{{reference}}` found in a string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    /// Name without braces or whitespace, e.g. `baseUrl` or `$uuid`.
    pub name: String,
    /// Byte range of the whole `{{ … }}` in the input.
    pub range: Range<usize>,
    /// Where the value came from; `None` if unresolved.
    pub source: Option<Source>,
    pub secret: bool,
    pub unresolved: bool,
    /// Why expanding the value failed (cycle, depth limit), if it did.
    pub error: Option<String>,
}

/// Result of [`Resolver::resolve_str`].
#[derive(Clone, Debug, Default)]
pub struct Resolved {
    pub text: String,
    /// Top-level references, in order.
    pub refs: Vec<Ref>,
    /// Every secret value substituted (including through nested references).
    pub secrets: Vec<SecretString>,
    /// Names that couldn't be resolved, top-level or nested, without duplicates.
    pub unresolved: Vec<String>,
}

/// A variable as seen from a request: the winning definition plus what it overrides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visible {
    pub name: String,
    /// Fully expanded value. `None` for secrets unless revealed, or missing secrets.
    /// Unless revealed, secrets reached through nested references are masked.
    pub value: Option<String>,
    /// The value as written (`None` for unrevealed secrets).
    pub raw: Option<String>,
    pub secret: bool,
    /// A secret with no stored value.
    pub missing: bool,
    pub source: Source,
    /// Lower-precedence definitions of the same name, highest first.
    pub overrides: Vec<Source>,
    pub error: Option<String>,
    pub description: Option<String>,
}

/// A built-in dynamic helper, for autocomplete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Helper {
    /// With the `$`, e.g. `$uuid`.
    pub name: &'static str,
    pub description: &'static str,
    pub example: String,
}

const HELPERS: &[(&str, &str)] = &[
    ("$uuid", "Random UUID v4"),
    ("$ulid", "Random ULID (sortable by time)"),
    ("$timestamp", "Current Unix time in seconds"),
    ("$isoTimestamp", "Current time as ISO 8601 / RFC 3339 (UTC)"),
    ("$randomInt", "Random integer from 0 to 1000"),
    ("$randomEmail", "Random email address at example.com"),
    (
        "$localEcho",
        "Base URL of IronCall's built-in echo server (works offline)",
    ),
];

/// Base URL of the built-in echo server, set once by the app when it starts it.
static LOCAL_ECHO: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Makes `{{$localEcho}}` resolve to `url`. Only the first call has an effect; without
/// a call, `{{$localEcho}}` stays unresolved.
pub fn set_local_echo(url: impl Into<String>) {
    let _ = LOCAL_ECHO.set(url.into());
}

/// The built-in helpers with fresh example values.
pub fn helpers() -> Vec<Helper> {
    HELPERS
        .iter()
        .map(|(name, description)| Helper {
            name,
            description,
            example: helper_value(name).unwrap_or_default(),
        })
        .collect()
}

fn helper_value(name: &str) -> Option<String> {
    let now = SystemTime::now();
    Some(match name {
        "$uuid" => uuid::Uuid::new_v4().to_string(),
        "$ulid" => ItemId::new().to_string(),
        "$timestamp" => now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string(),
        "$isoTimestamp" => rfc3339_utc(now),
        "$randomInt" => fastrand::u32(0..=1000).to_string(),
        "$localEcho" => LOCAL_ECHO.get()?.clone(),
        "$randomEmail" => {
            let user: String = std::iter::repeat_with(fastrand::lowercase).take(10).collect();
            format!("{user}@example.com")
        }
        _ => return None,
    })
}

/// A `{{name}}` occurrence in a string.
#[derive(Clone, Debug)]
struct Occurrence<'a> {
    name: &'a str,
    range: Range<usize>,
}

/// Whether `name` (already trimmed) is a valid variable or helper name.
fn valid_name(name: &str) -> bool {
    let body = name.strip_prefix('$').unwrap_or(name);
    !body.is_empty()
        && body
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && (!name.starts_with('$') || body.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Finds `{{name}}` references. Anything that doesn't look like one (no closing `}}`,
/// or invalid characters, as in JSON like `{{"a": 1}}`) is literal text.
fn occurrences(text: &str) -> Vec<Occurrence<'_>> {
    let mut found = Vec::new();
    let mut i = 0;
    while let Some(rel) = text[i..].find("{{") {
        let start = i + rel;
        let Some(close) = text[start + 2..].find("}}") else {
            break;
        };
        let end = start + 2 + close + 2;
        let name = text[start + 2..start + 2 + close].trim();
        if valid_name(name) {
            found.push(Occurrence {
                name,
                range: start..end,
            });
            i = end;
        } else {
            // Step one character so `{{{name}}}` still finds `{{name}}`.
            i = start + 1;
        }
    }
    found
}

/// The first `{{reference}}` in `text`, as written (with braces), if any.
pub fn first_ref(text: &str) -> Option<String> {
    occurrences(text)
        .first()
        .map(|o| text[o.range.clone()].to_string())
}

/// Variable layers in precedence order.
#[derive(Clone, Debug, Default)]
pub struct Resolver {
    /// Lowest precedence first.
    layers: Vec<Layer>,
}

struct Expansion {
    text: String,
    secrets: Vec<SecretString>,
    unresolved: Vec<String>,
}

impl Resolver {
    /// Layers may come in any order; they are sorted by scope precedence. Folder layers
    /// must be passed outermost first.
    pub fn new(mut layers: Vec<Layer>) -> Self {
        layers.sort_by_key(|l| l.source.rank()); // stable: keeps folder order
        Self { layers }
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// The winning definition of `name` and its layer.
    fn lookup(&self, name: &str) -> Option<(&Layer, &Def)> {
        self.layers.iter().rev().find_map(|l| l.get(name).map(|d| (l, d)))
    }

    /// Replaces every `{{reference}}` in `text`. Unresolved references are left as
    /// written and listed.
    pub fn resolve_str(&self, text: &str) -> Resolved {
        let mut out = Resolved::default();
        let mut last = 0;
        for occ in occurrences(text) {
            out.text.push_str(&text[last..occ.range.start]);
            last = occ.range.end;
            let mut r = Ref {
                name: occ.name.to_string(),
                range: occ.range.clone(),
                source: None,
                secret: false,
                unresolved: false,
                error: None,
            };
            match self.value_of(occ.name, &mut Vec::new()) {
                Ok(Some((source, secret, exp))) => {
                    r.source = Some(source);
                    r.secret = secret;
                    out.text.push_str(&exp.text);
                    out.secrets.extend(exp.secrets);
                    merge(&mut out.unresolved, exp.unresolved);
                }
                Ok(None) => {
                    r.unresolved = true;
                    r.secret = self.lookup(occ.name).is_some_and(|(_, d)| d.is_secret());
                    out.text.push_str(&text[occ.range.clone()]);
                    merge(&mut out.unresolved, vec![occ.name.to_string()]);
                }
                Err(e) => {
                    r.unresolved = true;
                    r.error = Some(e);
                    out.text.push_str(&text[occ.range.clone()]);
                    merge(&mut out.unresolved, vec![occ.name.to_string()]);
                }
            }
            out.refs.push(r);
        }
        out.text.push_str(&text[last..]);
        out
    }

    /// Expands one name. `Ok(None)` means undefined (or a secret with no value).
    /// `stack` holds the names being expanded, for cycle detection.
    fn value_of(
        &self,
        name: &str,
        stack: &mut Vec<String>,
    ) -> Result<Option<(Source, bool, Expansion)>, String> {
        if name.starts_with('$') {
            return Ok(helper_value(name).map(|text| {
                (
                    Source::Dynamic,
                    false,
                    Expansion {
                        text,
                        secrets: Vec::new(),
                        unresolved: Vec::new(),
                    },
                )
            }));
        }
        let Some((layer, def)) = self.lookup(name) else {
            return Ok(None);
        };
        let (raw, secret) = match def {
            Def::Plain { value, .. } => (value.clone(), None),
            Def::Secret { value: Some(v), .. } => (v.expose_secret().to_string(), Some(v.clone())),
            Def::Secret { value: None, .. } => return Ok(None),
        };
        if stack.iter().any(|n| n == name) {
            let mut cycle = stack.clone();
            cycle.push(name.to_string());
            return Err(format!("circular reference: {}", cycle.join(" → ")));
        }
        if stack.len() >= MAX_DEPTH {
            return Err(format!("references nested more than {MAX_DEPTH} levels deep"));
        }
        stack.push(name.to_string());
        let expanded = self.expand(&raw, stack);
        stack.pop();
        let mut exp = expanded?;
        if let Some(secret) = secret {
            // Mask what's actually sent: the expanded value. A secret like
            // `sk-{{tenant}}-…` appears in requests only in its expanded form.
            if exp.text != secret.expose_secret() {
                exp.secrets.push(SecretString::from(exp.text.clone()));
            }
            exp.secrets.push(secret);
        }
        Ok(Some((layer.source.clone(), def.is_secret(), exp)))
    }

    fn expand(&self, raw: &str, stack: &mut Vec<String>) -> Result<Expansion, String> {
        let mut exp = Expansion {
            text: String::with_capacity(raw.len()),
            secrets: Vec::new(),
            unresolved: Vec::new(),
        };
        let mut last = 0;
        for occ in occurrences(raw) {
            exp.text.push_str(&raw[last..occ.range.start]);
            last = occ.range.end;
            match self.value_of(occ.name, stack)? {
                Some((_, _, inner)) => {
                    exp.text.push_str(&inner.text);
                    exp.secrets.extend(inner.secrets);
                    merge(&mut exp.unresolved, inner.unresolved);
                }
                None => {
                    exp.text.push_str(&raw[occ.range.clone()]);
                    merge(&mut exp.unresolved, vec![occ.name.to_string()]);
                }
            }
        }
        exp.text.push_str(&raw[last..]);
        Ok(exp)
    }

    /// Every defined variable, sorted by name, with its winning definition, the
    /// definitions it overrides and its expanded value. Secrets are masked unless
    /// `reveal`.
    pub fn visible(&self, reveal: bool) -> Vec<Visible> {
        let mut names: Vec<&str> = self
            .layers
            .iter()
            .flat_map(|l| l.vars.iter().map(|(n, _)| n.as_str()))
            .collect();
        names.sort_unstable();
        names.dedup();

        names
            .into_iter()
            .filter_map(|name| {
                let (layer, def) = self.lookup(name)?;
                let overrides = self
                    .layers
                    .iter()
                    .rev()
                    .filter(|l| l.get(name).is_some() && !std::ptr::eq(*l, layer))
                    .map(|l| l.source.clone())
                    .collect();
                let secret = def.is_secret();
                let missing = matches!(def, Def::Secret { value: None, .. });
                let raw = match def {
                    Def::Plain { value, .. } => Some(value.clone()),
                    Def::Secret { value: Some(v), .. } if reveal => Some(v.expose_secret().to_string()),
                    Def::Secret { .. } => None,
                };
                let (value, error) = match self.value_of(name, &mut Vec::new()) {
                    Ok(Some(_)) if secret && !reveal => (None, None),
                    Ok(Some((_, _, exp))) if reveal => (Some(exp.text), None),
                    Ok(Some((_, _, exp))) => (Some(redact::mask_values(&exp.text, &exp.secrets)), None),
                    Ok(None) => (None, None),
                    Err(e) => (None, Some(e)),
                };
                Some(Visible {
                    name: name.to_string(),
                    value,
                    raw,
                    secret,
                    missing,
                    source: layer.source.clone(),
                    overrides,
                    error,
                    description: def.description().map(str::to_string),
                })
            })
            .collect()
    }
}

fn merge(into: &mut Vec<String>, names: Vec<String>) {
    for name in names {
        if !into.contains(&name) {
            into.push(name);
        }
    }
}

/// A request with every `{{reference}}` resolved.
#[derive(Clone, Debug)]
pub struct ResolvedRequest {
    pub request: RequestFile,
    /// Every secret value substituted, for redaction.
    pub secrets: Vec<SecretString>,
    /// Names that couldn't be resolved, without duplicates.
    pub unresolved: Vec<String>,
}

impl ResolvedRequest {
    /// Adds the secrets and unresolved names of another resolution (e.g. inherited auth
    /// or default headers) to this one.
    pub fn absorb<T>(&mut self, other: &Resolution<T>) {
        self.secrets.extend(other.secrets.iter().cloned());
        merge(&mut self.unresolved, other.unresolved.clone());
    }
}

/// Something resolved outside a request, such as inherited auth or default headers.
#[derive(Clone, Debug)]
pub struct Resolution<T> {
    pub value: T,
    pub secrets: Vec<SecretString>,
    pub unresolved: Vec<String>,
}

/// Collects secrets and unresolved names while resolving strings in place.
struct Collector<'r> {
    resolver: &'r Resolver,
    secrets: Vec<SecretString>,
    unresolved: Vec<String>,
}

impl<'r> Collector<'r> {
    fn new(resolver: &'r Resolver) -> Self {
        Self {
            resolver,
            secrets: Vec::new(),
            unresolved: Vec::new(),
        }
    }

    fn apply(&mut self, s: &mut String) {
        if !s.contains("{{") {
            return;
        }
        let r = self.resolver.resolve_str(s);
        *s = r.text;
        self.secrets.extend(r.secrets);
        merge(&mut self.unresolved, r.unresolved);
    }

    fn apply_opt(&mut self, s: &mut Option<String>) {
        if let Some(s) = s {
            self.apply(s);
        }
    }

    /// Enabled rows only: disabled rows aren't sent.
    fn rows(&mut self, rows: &mut [KeyValue]) {
        for row in rows.iter_mut().filter(|r| r.enabled) {
            self.apply(&mut row.name);
            self.apply(&mut row.value);
        }
    }

    fn auth(&mut self, auth: &mut Auth) {
        match auth {
            Auth::Inherit | Auth::None { .. } => {}
            Auth::Basic {
                username, password, ..
            } => {
                self.apply(username);
                self.apply(password);
            }
            Auth::Bearer { token, .. } => self.apply(token),
            Auth::ApiKey { key, value, .. } => {
                self.apply(key);
                self.apply(value);
            }
        }
    }

    fn finish<T>(self, value: T) -> Resolution<T> {
        Resolution {
            value,
            secrets: self.secrets,
            unresolved: self.unresolved,
        }
    }
}

/// Resolves the parts of `req` that are sent: URL, path param values, enabled query
/// params, enabled headers, the request's own auth fields, text bodies (JSON, text,
/// XML), enabled form fields, and enabled multipart parts (names, text values and file
/// paths) and the binary body's path. Disabled rows are left as they are.
///
/// Inherited auth and default headers come from other files; resolve them with
/// [`resolve_auth`] and [`resolve_rows`] and [`ResolvedRequest::absorb`] the results.
pub fn resolve_request(req: &RequestFile, resolver: &Resolver) -> ResolvedRequest {
    let mut c = Collector::new(resolver);
    let mut request = req.clone();
    let r = &mut request;
    c.apply(&mut r.url);
    for p in &mut r.path_params {
        c.apply(&mut p.value);
    }
    c.rows(&mut r.params);
    c.rows(&mut r.headers);
    c.auth(&mut r.auth);
    match &mut r.body {
        Body::Json { content, .. } | Body::Text { content, .. } | Body::Xml { content, .. } => {
            c.apply(content)
        }
        Body::FormUrlencoded { fields, .. } => c.rows(fields),
        Body::Multipart { parts, .. } => {
            for part in parts.iter_mut().filter(|p| p.enabled) {
                c.apply(&mut part.name);
                match part.kind {
                    MultipartKind::Text => c.apply_opt(&mut part.value),
                    MultipartKind::File => c.apply_opt(&mut part.path),
                }
            }
        }
        Body::BinaryFile { path, .. } => c.apply(path),
        Body::None => {}
    }
    let Resolution {
        secrets, unresolved, ..
    } = c.finish(());
    ResolvedRequest {
        request,
        secrets,
        unresolved,
    }
}

/// Resolves an auth's fields (username, password, token, API key name and value).
pub fn resolve_auth(auth: &Auth, resolver: &Resolver) -> Resolution<Auth> {
    let mut c = Collector::new(resolver);
    let mut auth = auth.clone();
    c.auth(&mut auth);
    c.finish(auth)
}

/// Resolves the names and values of enabled rows (e.g. default headers).
pub fn resolve_rows(rows: &[KeyValue], resolver: &Resolver) -> Resolution<Vec<KeyValue>> {
    let mut c = Collector::new(resolver);
    let mut rows = rows.to_vec();
    c.rows(&mut rows);
    c.finish(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PathParam, VariableSpec};

    fn env() -> Source {
        Source::Environment {
            id: "01J9Z3M0A1B2C3D4E5F6G7H8J9".parse().unwrap(),
            name: "Dev".into(),
        }
    }

    fn folder(name: &str) -> Source {
        Source::Folder {
            id: ItemId::new(),
            name: name.into(),
        }
    }

    fn all_scopes() -> Vec<Source> {
        vec![
            Source::Global,
            Source::Collection,
            env(),
            folder("f"),
            Source::Request,
            Source::Runtime,
        ]
    }

    #[test]
    fn precedence_for_every_pair_of_scopes() {
        let scopes = all_scopes();
        for (i, low) in scopes.iter().enumerate() {
            for high in &scopes[i + 1..] {
                // Pass the layers in both orders: precedence must not depend on it.
                for reversed in [false, true] {
                    let mut layers = vec![
                        Layer::new(low.clone()).with("v", "low"),
                        Layer::new(high.clone()).with("v", "high"),
                    ];
                    if reversed {
                        layers.reverse();
                    }
                    let r = Resolver::new(layers);
                    let resolved = r.resolve_str("{{v}}");
                    assert_eq!(resolved.text, "high", "{high:?} should beat {low:?}");
                    assert_eq!(resolved.refs[0].source.as_ref(), Some(high));
                }
            }
        }
    }

    #[test]
    fn inner_folder_beats_outer_folder() {
        let (outer, inner) = (folder("outer"), folder("inner"));
        let r = Resolver::new(vec![
            Layer::new(outer.clone())
                .with("v", "outer")
                .with("only_outer", "o"),
            Layer::new(inner.clone()).with("v", "inner"),
        ]);
        assert_eq!(r.resolve_str("{{v}}/{{only_outer}}").text, "inner/o");
        let v = r.visible(false).into_iter().find(|v| v.name == "v").unwrap();
        assert_eq!(v.source, inner);
        assert_eq!(v.overrides, vec![outer]);
    }

    #[test]
    fn nested_references_expand() {
        let r = Resolver::new(vec![
            Layer::new(Source::Collection)
                .with("host", "api.example.com")
                .with("baseUrl", "https://{{host}}/v1"),
            Layer::new(env()).with("host", "dev.example.com"),
        ]);
        // The nested reference resolves with full precedence, not within its own layer.
        assert_eq!(
            r.resolve_str("{{baseUrl}}/users").text,
            "https://dev.example.com/v1/users"
        );
    }

    #[test]
    fn cycles_are_reported_not_followed() {
        let r = Resolver::new(vec![
            Layer::new(Source::Collection)
                .with("a", "x{{b}}")
                .with("b", "y{{a}}"),
        ]);
        let resolved = r.resolve_str("start {{a}} end");
        assert_eq!(resolved.text, "start {{a}} end");
        let err = resolved.refs[0].error.as_deref().unwrap();
        assert!(err.contains("circular reference: a → b → a"), "{err}");
        assert_eq!(resolved.unresolved, ["a"]);

        let self_ref = Resolver::new(vec![Layer::new(Source::Collection).with("a", "{{a}}")]);
        assert!(self_ref.resolve_str("{{a}}").refs[0].error.is_some());
    }

    #[test]
    fn depth_limit_is_enforced() {
        let mut layer = Layer::new(Source::Collection);
        for i in 0..=MAX_DEPTH + 1 {
            layer = layer.with(format!("v{i}"), format!("{{{{v{}}}}}", i + 1));
        }
        layer = layer.with(format!("v{}", MAX_DEPTH + 2), "end");
        let r = Resolver::new(vec![layer]);
        let resolved = r.resolve_str("{{v0}}");
        assert!(resolved.refs[0].error.as_deref().unwrap().contains("levels deep"));

        // Just inside the limit works.
        let mut ok = Layer::new(Source::Collection);
        for i in 0..MAX_DEPTH - 1 {
            ok = ok.with(format!("v{i}"), format!("{{{{v{}}}}}", i + 1));
        }
        ok = ok.with(format!("v{}", MAX_DEPTH - 1), "end");
        assert_eq!(Resolver::new(vec![ok]).resolve_str("{{v0}}").text, "end");
    }

    #[test]
    fn unresolved_references_are_kept_and_listed() {
        let r = Resolver::new(vec![
            Layer::new(Source::Collection).with("a", "{{missing_inner}}"),
        ]);
        let resolved = r.resolve_str("{{ nope }}/{{a}}/{{nope}}");
        assert_eq!(resolved.text, "{{ nope }}/{{missing_inner}}/{{nope}}");
        assert_eq!(resolved.unresolved, ["nope", "missing_inner"]);
        assert!(resolved.refs[0].unresolved);
        assert!(!resolved.refs[1].unresolved, "a itself resolved");
    }

    #[test]
    fn whitespace_and_name_rules() {
        let r = Resolver::new(vec![Layer::new(Source::Collection).with("my-var.v_2", "ok")]);
        assert_eq!(r.resolve_str("{{  my-var.v_2  }}").text, "ok");
        // Not references: no closing braces, invalid characters, empty, bad helper.
        for literal in [
            "{{ unclosed",
            "{{a b}}",
            "{{}}",
            "{{$}}",
            "{{\"a\": 1}}",
            "{{$no-dash}}",
        ] {
            assert_eq!(r.resolve_str(literal).text, literal, "{literal}");
            assert!(r.resolve_str(literal).refs.is_empty(), "{literal}");
        }
        assert_eq!(r.resolve_str("{{{my-var.v_2}}}").text, "{ok}");
    }

    #[test]
    fn braces_in_json_bodies_are_untouched() {
        let r = Resolver::new(vec![Layer::new(Source::Collection).with("b", "X")]);
        for json in [
            r#"{"a": {"b": 1}}"#,
            r#"{"a":{"b":{"c":1}}}"#,
            "{\n  \"a\": {}\n}",
        ] {
            let resolved = r.resolve_str(json);
            assert_eq!(resolved.text, json);
            assert!(resolved.refs.is_empty());
        }
    }

    #[test]
    fn secrets_with_references_are_masked_as_sent() {
        let r = Resolver::new(vec![
            Layer::new(Source::Collection).with("tenant", "acme"),
            Layer::new(Source::Environment {
                id: ItemId::new(),
                name: "Dev".into(),
            })
            .with_secret("tok", Some("sk-{{tenant}}-0123456789")),
        ]);
        let resolved = r.resolve_str("Bearer {{tok}}");
        assert_eq!(resolved.text, "Bearer sk-acme-0123456789");
        assert_eq!(
            crate::redact::mask_values(&resolved.text, &resolved.secrets),
            format!("Bearer {}", crate::redact::MASK)
        );
    }

    #[test]
    fn helpers_are_fresh_per_occurrence() {
        let r = Resolver::default();
        let resolved = r.resolve_str("{{$uuid}} {{$uuid}}");
        let parts: Vec<&str> = resolved.text.split(' ').collect();
        assert_eq!(parts.len(), 2);
        assert_ne!(parts[0], parts[1]);
        assert!(uuid::Uuid::parse_str(parts[0]).is_ok());
        assert_eq!(resolved.refs[0].source, Some(Source::Dynamic));

        set_local_echo("http://127.0.0.1:9");
        assert_eq!(r.resolve_str("{{$localEcho}}/get").text, "http://127.0.0.1:9/get");
        for (name, _) in HELPERS {
            let text = r.resolve_str(&format!("{{{{{name}}}}}")).text;
            assert!(!text.contains("{{"), "{name} resolved to {text}");
        }
        let n: u32 = r.resolve_str("{{$randomInt}}").text.parse().unwrap();
        assert!(n <= 1000);
        assert!(r.resolve_str("{{$randomEmail}}").text.ends_with("@example.com"));
        assert!(r.resolve_str("{{$isoTimestamp}}").text.ends_with('Z'));
        assert!(r.resolve_str("{{$unknown}}").unresolved == ["$unknown"]);
        assert_eq!(helpers().len(), HELPERS.len());
    }

    #[test]
    fn variables_shadowing_a_helper_name_cannot_exist() {
        // `$` names are always helpers; a variable can't be named `$uuid`.
        let r = Resolver::new(vec![Layer::new(Source::Collection).with("$uuid", "fixed")]);
        assert_ne!(r.resolve_str("{{$uuid}}").text, "fixed");
    }

    #[test]
    fn secrets_are_collected_and_masked() {
        let r = Resolver::new(vec![
            Layer::new(Source::Collection).with("auth", "Bearer {{token}}"),
            Layer::new(env()).with_secret("token", Some("s3cr3t-value")),
            Layer::new(Source::Global).with_secret("missing", None),
        ]);
        let resolved = r.resolve_str("{{auth}}");
        assert_eq!(resolved.text, "Bearer s3cr3t-value");
        assert_eq!(resolved.secrets.len(), 1);
        assert_eq!(resolved.secrets[0].expose_secret(), "s3cr3t-value");

        let visible = r.visible(false);
        let token = visible.iter().find(|v| v.name == "token").unwrap();
        assert!(token.secret && token.value.is_none() && token.raw.is_none() && !token.missing);
        let auth = visible.iter().find(|v| v.name == "auth").unwrap();
        assert_eq!(auth.value.as_deref(), Some("Bearer ••••"), "nested secret masked");
        let missing = visible.iter().find(|v| v.name == "missing").unwrap();
        assert!(missing.missing && missing.value.is_none());

        let revealed = r.visible(true);
        let token = revealed.iter().find(|v| v.name == "token").unwrap();
        assert_eq!(token.value.as_deref(), Some("s3cr3t-value"));
        assert_eq!(token.raw.as_deref(), Some("s3cr3t-value"));

        // A missing secret stays unresolved.
        let missing = r.resolve_str("{{missing}}");
        assert_eq!(missing.unresolved, ["missing"]);
        assert!(missing.refs[0].secret);
    }

    #[test]
    fn visible_lists_provenance_and_overrides() {
        let r = Resolver::new(vec![
            Layer::new(Source::Global).with("x", "g"),
            Layer::new(Source::Collection).with("x", "c").with("y", "{{x}}!"),
            Layer::new(env()).with("x", "e"),
            Layer::new(Source::Request).with("z", "{{a}}{{b}}"),
        ]);
        let visible = r.visible(false);
        assert_eq!(
            visible.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
            ["x", "y", "z"]
        );
        let x = &visible[0];
        assert_eq!(x.source, env());
        assert_eq!(x.overrides, vec![Source::Collection, Source::Global]);
        assert_eq!(visible[1].value.as_deref(), Some("e!"));
        assert_eq!(visible[1].raw.as_deref(), Some("{{x}}!"));
        assert_eq!(
            visible[2].value.as_deref(),
            Some("{{a}}{{b}}"),
            "unresolved nested refs kept"
        );

        let cyclic = Resolver::new(vec![Layer::new(Source::Collection).with("a", "{{a}}")]);
        assert!(cyclic.visible(false)[0].error.is_some());
    }

    #[test]
    fn from_vars_reads_plain_detailed_and_secret_variables() {
        let mut vars = Vars::new();
        vars.insert("plain".into(), Variable::Plain("p".into()));
        vars.insert(
            "detailed".into(),
            Variable::Detailed(VariableSpec {
                value: Some("d".into()),
                description: Some("about d".into()),
                ..VariableSpec::default()
            }),
        );
        vars.insert(
            "key".into(),
            Variable::Detailed(VariableSpec {
                secret: true,
                ..VariableSpec::default()
            }),
        );
        let mut asked = Vec::new();
        let layer = Layer::from_vars(Source::Collection, &vars, |name| {
            asked.push(name.to_string());
            Some(SecretString::from("k3y-value".to_string()))
        });
        assert_eq!(asked, ["key"], "only secrets are looked up");
        let r = Resolver::new(vec![layer]);
        assert_eq!(r.resolve_str("{{plain}}{{detailed}}{{key}}").text, "pdk3y-value");
        let d = r
            .visible(false)
            .into_iter()
            .find(|v| v.name == "detailed")
            .unwrap();
        assert_eq!(d.description.as_deref(), Some("about d"));
    }

    #[test]
    fn first_ref_finds_written_reference() {
        assert_eq!(first_ref("a/{{ x }}/b").as_deref(), Some("{{ x }}"));
        assert_eq!(first_ref(r#"{"a": {"b": 1}}"#), None);
    }

    #[test]
    fn resolve_request_covers_every_sent_field() {
        let r = Resolver::new(vec![
            Layer::new(Source::Collection)
                .with("base", "https://api.example.com")
                .with("id", "42")
                .with("n", "name"),
            Layer::new(env()).with_secret("tok", Some("tok-secret")),
        ]);
        let mut req = RequestFile::new("t");
        req.url = "{{base}}/users/:id".into();
        req.path_params = vec![PathParam {
            name: "id".into(),
            value: "{{id}}".into(),
            extra: Default::default(),
        }];
        req.params = vec![KeyValue::new("{{n}}", "{{id}}"), {
            let mut off = KeyValue::new("off", "{{id}}");
            off.enabled = false;
            off
        }];
        req.headers = vec![
            KeyValue::new("Authorization", "Bearer {{tok}}"),
            KeyValue::new("X", "{{nope}}"),
        ];
        req.body = Body::Json {
            content: r#"{"id": "{{id}}", "nested": {"a": 1}}"#.into(),
            extra: Default::default(),
        };

        let out = resolve_request(&req, &r);
        let q = &out.request;
        assert_eq!(q.url, "https://api.example.com/users/:id");
        assert_eq!(q.path_params[0].value, "42");
        assert_eq!(
            (q.params[0].name.as_str(), q.params[0].value.as_str()),
            ("name", "42")
        );
        assert_eq!(q.params[1].value, "{{id}}", "disabled rows untouched");
        assert_eq!(q.headers[0].value, "Bearer tok-secret");
        let Body::Json { content, .. } = &q.body else {
            panic!("json body")
        };
        assert_eq!(content, r#"{"id": "42", "nested": {"a": 1}}"#);
        assert_eq!(out.unresolved, ["nope"]);
        assert_eq!(out.secrets.len(), 1);

        let mut form = RequestFile::new("f");
        form.body = Body::FormUrlencoded {
            fields: vec![KeyValue::new("{{n}}", "{{tok}}")],
            extra: Default::default(),
        };
        let out = resolve_request(&form, &r);
        let Body::FormUrlencoded { fields, .. } = &out.request.body else {
            panic!("form body")
        };
        assert_eq!(
            (fields[0].name.as_str(), fields[0].value.as_str()),
            ("name", "tok-secret")
        );
    }

    #[test]
    fn resolve_request_covers_auth_multipart_and_paths() {
        use crate::model::{ApiKeyLocation, Extra, MultipartPart};
        let r = Resolver::new(vec![
            Layer::new(Source::Collection)
                .with("user", "ada")
                .with("dir", "scans")
                .with("field", "title"),
            Layer::new(env())
                .with_secret("pass", Some("p4ssw0rd"))
                .with_secret("key", Some("k3y-value")),
        ]);

        let mut req = RequestFile::new("t");
        req.auth = Auth::Basic {
            username: "{{user}}".into(),
            password: "{{pass}}".into(),
            extra: Extra::new(),
        };
        let text = MultipartPart {
            name: "{{field}}".into(),
            kind: MultipartKind::Text,
            value: Some("by {{user}}".into()),
            path: None,
            content_type: None,
            enabled: true,
            extra: Extra::new(),
        };
        let file = MultipartPart {
            name: "file".into(),
            kind: MultipartKind::File,
            value: None,
            path: Some("{{dir}}/a.png".into()),
            ..text.clone()
        };
        let mut off = text.clone();
        off.enabled = false;
        req.body = Body::Multipart {
            parts: vec![text, file, off],
            extra: Extra::new(),
        };

        let out = resolve_request(&req, &r);
        let Auth::Basic {
            username, password, ..
        } = &out.request.auth
        else {
            panic!("basic auth")
        };
        assert_eq!((username.as_str(), password.as_str()), ("ada", "p4ssw0rd"));
        let Body::Multipart { parts, .. } = &out.request.body else {
            panic!("multipart")
        };
        assert_eq!(parts[0].name, "title");
        assert_eq!(parts[0].value.as_deref(), Some("by ada"));
        assert_eq!(parts[1].path.as_deref(), Some("scans/a.png"));
        assert_eq!(parts[2].name, "{{field}}", "disabled parts untouched");
        assert_eq!(out.secrets.len(), 1);
        assert_eq!(out.secrets[0].expose_secret(), "p4ssw0rd");

        let mut bin = RequestFile::new("b");
        bin.body = Body::BinaryFile {
            path: "{{dir}}/x.bin".into(),
            content_type: None,
            extra: Extra::new(),
        };
        let Body::BinaryFile { path, .. } = resolve_request(&bin, &r).request.body else {
            panic!("binary")
        };
        assert_eq!(path, "scans/x.bin");

        // Inherited auth and default headers are resolved separately and absorbed.
        let inherited = resolve_auth(
            &Auth::ApiKey {
                key: "X-Key".into(),
                value: "{{key}}".into(),
                location: ApiKeyLocation::Header,
                extra: Extra::new(),
            },
            &r,
        );
        let Auth::ApiKey { value, .. } = &inherited.value else {
            panic!("api key")
        };
        assert_eq!(value, "k3y-value");
        let headers = resolve_rows(
            &[
                KeyValue::new("X-User", "{{user}}"),
                KeyValue::new("X-Missing", "{{nope}}"),
            ],
            &r,
        );
        assert_eq!(headers.value[0].value, "ada");
        assert_eq!(headers.unresolved, ["nope"]);

        let mut out = resolve_request(&RequestFile::new("e"), &r);
        out.absorb(&inherited);
        out.absorb(&headers);
        assert_eq!(out.secrets.len(), 1);
        assert_eq!(out.unresolved, ["nope"]);
    }
}
