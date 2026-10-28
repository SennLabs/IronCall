//! Declarative assertions on a response (PLAN.md Phase 7).
//!
//! Targets: status, header, body (JSONPath), body text, duration and size. Body JSONPath
//! values are typed: `201` and `"201"` differ. The other targets are looser, because
//! headers and text are always strings and status/duration/size always numbers: `42`
//! matches a `Content-Length: 42` header, and `"201"` matches status 201. An expected
//! value written with `{{variables}}` is compared loosely too, since variables are text.

use regex::Regex;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{AssertOp, AssertTarget, Assertion};
use crate::redact;
use crate::response::{ResponseData, json_path};
use crate::vars::{Resolution, Resolver};

/// Longest actual value shown in results.
const MAX_SHOWN: usize = 200;

/// The outcome of one enabled assertion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct AssertionResult {
    /// Index in the request's `assert` list (disabled assertions are skipped).
    #[cfg_attr(feature = "specta", specta(type = specta_typescript::Number))]
    pub index: usize,
    pub passed: bool,
    /// The assertion as written, e.g. `status eq 201` or `body $.id exists`.
    pub label: String,
    /// The value that was checked, shortened; masked if sensitive. `None` if there was none.
    pub actual: Option<String>,
    /// Why it failed, if it did.
    pub message: Option<String>,
}

/// Resolves `{{variables}}` in each assertion's header name, path and expected value
/// (strings, and strings inside an `in` list).
pub fn resolve(asserts: &[Assertion], resolver: &Resolver) -> Resolution<Vec<Assertion>> {
    let mut secrets = Vec::new();
    let mut unresolved = Vec::new();
    let mut apply = |s: &mut String| {
        if s.contains("{{") {
            let r = resolver.resolve_str(s);
            *s = r.text;
            secrets.extend(r.secrets);
            for name in r.unresolved {
                if !unresolved.contains(&name) {
                    unresolved.push(name);
                }
            }
        }
    };
    let value = asserts
        .iter()
        .map(|a| {
            let mut a = a.clone();
            a.name.iter_mut().for_each(&mut apply);
            a.path.iter_mut().for_each(&mut apply);
            match &mut a.value {
                Some(Value::String(s)) => apply(s),
                Some(Value::Array(items)) => items.iter_mut().for_each(|v| {
                    if let Value::String(s) = v {
                        apply(s)
                    }
                }),
                _ => {}
            }
            a
        })
        .collect();
    Resolution {
        value,
        secrets,
        unresolved,
    }
}

/// Evaluates the enabled assertions. `authored` gives the labels (so a resolved secret
/// never appears in one) and `resolved` the values checked; both come from [`resolve`].
/// `secrets` are masked wherever they appear in actual values and messages.
pub fn evaluate(
    authored: &[Assertion],
    resolved: &[Assertion],
    response: &ResponseData,
    secrets: &[SecretString],
    sensitive_headers: &[String],
) -> Vec<AssertionResult> {
    let secrets = with_json_escaped(secrets);
    authored
        .iter()
        .zip(resolved)
        .enumerate()
        .filter(|(_, (a, _))| a.enabled)
        .map(|(index, (authored, a))| {
            let (passed, actual, message) = match check(a, response, templated(authored)) {
                Ok((passed, actual, message)) => (passed, actual, message),
                Err(e) => (false, None, Some(e)),
            };
            let hide = a.target == AssertTarget::Header
                && a.name
                    .as_deref()
                    .is_some_and(|n| redact::is_sensitive_header(n, sensitive_headers));
            // Mask the whole value first, then shorten: a secret cut in half at the limit
            // would otherwise show its first part.
            let actual = actual.map(|v| {
                if hide {
                    redact::MASK.to_string()
                } else {
                    // Strings are masked before display() quotes and escapes them.
                    let shown = match &v {
                        Value::String(text) => display(&Value::String(redact::mask_values(text, &secrets))),
                        other => redact::mask_values(&display(other), &secrets),
                    };
                    shorten(&shown)
                }
            });
            let message = message.map(|m| redact::mask_values(&m, &secrets));
            AssertionResult {
                index,
                passed,
                label: label(authored),
                actual,
                message: if hide && !passed {
                    message.map(|_| "the value didn't match".into())
                } else {
                    message
                },
            }
        })
        .collect()
}

/// The secrets plus their JSON-escaped forms: actual values are shown as JSON, where a
/// secret containing `"` or `\` appears escaped.
fn with_json_escaped(secrets: &[SecretString]) -> Vec<SecretString> {
    use secrecy::ExposeSecret;
    let mut out = secrets.to_vec();
    for s in secrets {
        let json = serde_json::to_string(s.expose_secret()).unwrap_or_default();
        let inner = json.trim_matches('"');
        if inner != s.expose_secret() {
            out.push(SecretString::from(inner.to_string()));
        }
    }
    out
}

/// `status eq 201`, `header Content-Type contains "json"`, `body $.id exists`.
pub fn label(a: &Assertion) -> String {
    let mut parts = vec![target_name(a.target).to_string()];
    match a.target {
        AssertTarget::Header => parts.push(a.name.clone().unwrap_or_default()),
        AssertTarget::Body => parts.push(a.path.clone().unwrap_or_else(|| "$".into())),
        _ => {}
    }
    parts.push(op_name(a.op).to_string());
    if let Some(v) = &a.value
        && !matches!(a.op, AssertOp::Exists | AssertOp::NotExists)
    {
        parts.push(v.to_string());
    }
    parts.join(" ")
}

fn target_name(t: AssertTarget) -> &'static str {
    match t {
        AssertTarget::Status => "status",
        AssertTarget::Header => "header",
        AssertTarget::Body => "body",
        AssertTarget::BodyText => "bodyText",
        AssertTarget::Duration => "duration",
        AssertTarget::Size => "size",
    }
}

fn op_name(op: AssertOp) -> &'static str {
    match op {
        AssertOp::Eq => "eq",
        AssertOp::Neq => "neq",
        AssertOp::Lt => "lt",
        AssertOp::Lte => "lte",
        AssertOp::Gt => "gt",
        AssertOp::Gte => "gte",
        AssertOp::Contains => "contains",
        AssertOp::Matches => "matches",
        AssertOp::Exists => "exists",
        AssertOp::NotExists => "notExists",
        AssertOp::Type => "type",
        AssertOp::In => "in",
    }
}

/// The value an assertion looks at. `Ok(None)`: nothing there (missing header, no match).
fn actual(a: &Assertion, r: &ResponseData) -> Result<Option<Value>, String> {
    Ok(match a.target {
        AssertTarget::Status => Some(Value::from(r.status)),
        AssertTarget::Header => {
            let name = a.name.as_deref().map(str::trim).unwrap_or_default();
            if name.is_empty() {
                return Err("no header name".into());
            }
            let values: Vec<&str> = r.header_values(name).collect();
            (!values.is_empty()).then(|| Value::from(values.join(", ")))
        }
        AssertTarget::Body => json_path(r.json()?, a.path.as_deref().unwrap_or("$"))?,
        AssertTarget::BodyText => Some(Value::from(r.text()?)),
        AssertTarget::Duration => Some(Value::from(r.duration_ms.round())),
        AssertTarget::Size => Some(Value::from(r.size)),
    })
}

type Checked = (bool, Option<Value>, Option<String>);

/// Whether the expected value uses `{{variables}}`.
fn templated(a: &Assertion) -> bool {
    match &a.value {
        Some(Value::String(s)) => s.contains("{{"),
        Some(Value::Array(items)) => items.iter().any(|v| v.as_str().is_some_and(|s| s.contains("{{"))),
        _ => false,
    }
}

fn check(a: &Assertion, r: &ResponseData, templated: bool) -> Result<Checked, String> {
    let loose = a.target != AssertTarget::Body || templated;
    let got = actual(a, r)?;
    match a.op {
        AssertOp::Exists => {
            let ok = got.is_some();
            return Ok((ok, got, (!ok).then(|| missing(a).to_string())));
        }
        AssertOp::NotExists => {
            let ok = got.is_none();
            return Ok((
                ok,
                got,
                (!ok).then(|| "expected no value, but there is one".into()),
            ));
        }
        _ => {}
    }
    let Some(got) = got else {
        return Ok((false, None, Some(missing(a).into())));
    };
    let Some(want) = &a.value else {
        return Err(format!("`{}` needs an expected value", op_name(a.op)));
    };
    let fail = |msg: String| Ok((false, Some(got.clone()), Some(msg)));
    let ok = match a.op {
        AssertOp::Eq => equal(&got, want, loose),
        AssertOp::Neq => !equal(&got, want, loose),
        AssertOp::Lt | AssertOp::Lte | AssertOp::Gt | AssertOp::Gte => {
            let (Some(x), Some(y)) = (number(&got, loose), number(want, true)) else {
                return fail(format!(
                    "can't compare {} with {} as numbers",
                    display(&got),
                    display(want)
                ));
            };
            match a.op {
                AssertOp::Lt => x < y,
                AssertOp::Lte => x <= y,
                AssertOp::Gt => x > y,
                _ => x >= y,
            }
        }
        AssertOp::Contains => match &got {
            Value::String(s) => s.contains(&text(want)),
            Value::Array(items) => items.iter().any(|v| equal(v, want, false)),
            Value::Object(map) => map.contains_key(&text(want)),
            other => text(other).contains(&text(want)),
        },
        AssertOp::Matches => {
            let pattern = text(want);
            let re = Regex::new(&pattern).map_err(|e| format!("invalid regex: {e}"))?;
            re.is_match(&text(&got))
        }
        AssertOp::Type => {
            let want = text(want);
            if !TYPES.contains(&want.as_str()) {
                return Err(format!("unknown type `{want}`; use one of {}", TYPES.join(", ")));
            }
            type_matches(&got, &want)
        }
        AssertOp::In => {
            let Value::Array(options) = want else {
                return Err("`in` needs a list of values, e.g. [200, 201]".into());
            };
            options.iter().any(|o| equal(&got, o, loose))
        }
        AssertOp::Exists | AssertOp::NotExists => unreachable!("handled above"),
    };
    if ok {
        Ok((true, Some(got), None))
    } else {
        let msg = match a.op {
            AssertOp::Type => format!("expected type {}, got {}", text(want), type_name(&got)),
            _ => format!("expected {} {}", op_name(a.op), display(want)),
        };
        fail(msg)
    }
}

fn missing(a: &Assertion) -> &'static str {
    match a.target {
        AssertTarget::Header => "the header isn't in the response",
        AssertTarget::Body => "the JSONPath matched nothing",
        _ => "no value",
    }
}

const TYPES: &[&str] = &[
    "string", "number", "integer", "boolean", "array", "object", "null",
];

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_matches(v: &Value, want: &str) -> bool {
    match (v, want) {
        (Value::Number(_), "number") => true,
        (Value::Number(n), "integer") => n.as_f64().is_some_and(|f| f.fract() == 0.0),
        _ => type_name(v) == want,
    }
}

fn number(v: &Value, loose: bool) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if loose => s.trim().parse().ok(),
        _ => None,
    }
}

fn equal(got: &Value, want: &Value, loose: bool) -> bool {
    if let (Some(x), Some(y)) = (number(got, loose), number(want, loose))
        && (loose || (got.is_number() && want.is_number()))
    {
        return x == y;
    }
    if loose {
        text(got) == text(want)
    } else {
        got == want
    }
}

/// Strings as themselves, everything else as JSON.
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Like [`text`], but strings are quoted so `"201"` and `201` look different.
fn display(v: &Value) -> String {
    v.to_string()
}

fn shorten(s: &str) -> String {
    if s.chars().count() <= MAX_SHOWN {
        return s.to_string();
    }
    let cut: String = s.chars().take(MAX_SHOWN).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response::BodyText;
    use crate::vars::{Layer, Source};
    use serde_json::json;

    fn response() -> ResponseData {
        let body = json!({
            "id": 42, "name": "Ada", "price": 9.5, "count": "7", "active": true, "nothing": null,
            "tags": ["a", "b"], "nested": {"k": "v"}, "email": "ada@example.com"
        })
        .to_string();
        ResponseData::new(
            201,
            vec![
                ("Content-Type".into(), "application/json; charset=utf-8".into()),
                ("Content-Length".into(), body.len().to_string()),
                ("Set-Cookie".into(), "session=s3cr3t-cookie".into()),
            ],
            123.4,
            body.len() as u64,
            BodyText::Text(body),
        )
    }

    fn a(target: AssertTarget, op: AssertOp, value: Option<Value>) -> Assertion {
        Assertion {
            target,
            path: None,
            name: None,
            op,
            value,
            enabled: true,
            extra: Default::default(),
        }
    }

    fn body(path: &str, op: AssertOp, value: Option<Value>) -> Assertion {
        Assertion {
            path: Some(path.into()),
            ..a(AssertTarget::Body, op, value)
        }
    }

    fn header(name: &str, op: AssertOp, value: Option<Value>) -> Assertion {
        Assertion {
            name: Some(name.into()),
            ..a(AssertTarget::Header, op, value)
        }
    }

    fn run(x: Assertion) -> AssertionResult {
        let r = response();
        evaluate(std::slice::from_ref(&x), std::slice::from_ref(&x), &r, &[], &[]).remove(0)
    }

    fn passes(x: Assertion) -> bool {
        run(x).passed
    }

    use AssertOp::*;
    use AssertTarget::*;

    #[test]
    fn eq_and_neq() {
        assert!(passes(a(Status, Eq, Some(json!(201)))));
        assert!(passes(a(Status, Eq, Some(json!("201")))), "status is loose");
        assert!(!passes(a(Status, Eq, Some(json!(200)))));
        assert!(passes(a(Status, Neq, Some(json!(200)))));
        assert!(passes(body("$.id", Eq, Some(json!(42)))));
        assert!(
            passes(body("$.id", Eq, Some(json!(42.0)))),
            "numbers compare numerically"
        );
        assert!(
            !passes(body("$.id", Eq, Some(json!("42")))),
            "body values are typed"
        );
        assert!(passes(body("$.id", Neq, Some(json!("42")))));
        assert!(passes(body("$.nested", Eq, Some(json!({"k": "v"})))));
        assert!(passes(body("$.nothing", Eq, Some(Value::Null))));
        assert!(
            passes(header("content-length", Eq, Some(json!(response().size)))),
            "header vs number"
        );
    }

    #[test]
    fn ordering_ops() {
        assert!(passes(a(Duration, Lt, Some(json!(500)))));
        assert!(!passes(a(Duration, Gt, Some(json!(500)))));
        assert!(
            passes(a(Duration, Lte, Some(json!(123)))),
            "duration is rounded to whole ms"
        );
        assert!(passes(a(Duration, Gte, Some(json!(123)))));
        assert!(passes(body("$.price", Gt, Some(json!(9)))));
        assert!(passes(body("$.price", Lte, Some(json!(9.5)))));
        assert!(passes(a(Size, Gt, Some(json!(10)))));
        assert!(
            passes(a(Status, Gte, Some(json!("200")))),
            "expected numeric strings are fine"
        );
        let r = run(body("$.count", Gt, Some(json!(1))));
        assert!(!r.passed, "a string body value isn't a number");
        assert!(r.message.unwrap().contains("can't compare"));
    }

    #[test]
    fn contains_matches_and_type() {
        assert!(passes(header("Content-Type", Contains, Some(json!("json")))));
        assert!(passes(body("$.tags", Contains, Some(json!("b")))));
        assert!(!passes(body("$.tags", Contains, Some(json!("z")))));
        assert!(
            passes(body("$.nested", Contains, Some(json!("k")))),
            "object contains key"
        );
        assert!(
            passes(body("$.id", Contains, Some(json!("4")))),
            "numbers as text"
        );
        assert!(passes(body(
            "$.email",
            Matches,
            Some(json!(r"^[^@]+@example\.com$"))
        )));
        assert!(!passes(body("$.name", Matches, Some(json!("^b")))));
        assert!(passes(a(BodyText, Contains, Some(json!("\"Ada\"")))));
        for (path, ty) in [
            ("$.name", "string"),
            ("$.id", "integer"),
            ("$.id", "number"),
            ("$.price", "number"),
            ("$.active", "boolean"),
            ("$.tags", "array"),
            ("$.nested", "object"),
            ("$.nothing", "null"),
        ] {
            assert!(passes(body(path, Type, Some(json!(ty)))), "{path} is {ty}");
        }
        let r = run(body("$.price", Type, Some(json!("integer"))));
        assert_eq!(r.message.as_deref(), Some("expected type integer, got number"));
        assert!(
            run(body("$.id", Type, Some(json!("float"))))
                .message
                .unwrap()
                .contains("unknown type")
        );
        assert!(
            run(body("$.id", Matches, Some(json!("("))))
                .message
                .unwrap()
                .contains("invalid regex")
        );
    }

    #[test]
    fn exists_not_exists_and_in() {
        assert!(passes(body("$.id", Exists, None)));
        assert!(passes(body("$.nothing", Exists, None)), "null still exists");
        assert!(!passes(body("$.missing", Exists, None)));
        assert!(passes(body("$.missing", NotExists, None)));
        assert!(!passes(body("$.id", NotExists, None)));
        assert!(passes(header("x-missing", NotExists, None)));
        assert!(passes(header("content-type", Exists, None)));
        assert!(passes(a(Status, In, Some(json!([200, 201])))));
        assert!(!passes(a(Status, In, Some(json!([200, 204])))));
        assert!(passes(body("$.name", In, Some(json!(["Ada", "Bob"])))));
        assert!(
            run(a(Status, In, Some(json!(201))))
                .message
                .unwrap()
                .contains("needs a list")
        );
    }

    #[test]
    fn missing_values_and_errors_fail_with_a_reason() {
        let r = run(body("$.missing", Eq, Some(json!(1))));
        assert!(!r.passed);
        assert_eq!(r.message.as_deref(), Some("the JSONPath matched nothing"));
        assert_eq!(
            run(header("x-nope", Eq, Some(json!("a")))).message.as_deref(),
            Some("the header isn't in the response")
        );
        assert!(
            run(a(Status, Eq, None))
                .message
                .unwrap()
                .contains("needs an expected value")
        );
        assert!(
            run(header("", Exists, None))
                .message
                .unwrap()
                .contains("no header name")
        );

        let text = ResponseData::new(200, vec![], 1.0, 3, BodyText::Text("abc".into()));
        let x = body("$.a", Exists, None);
        let r = evaluate(
            std::slice::from_ref(&x),
            std::slice::from_ref(&x),
            &text,
            &[],
            &[],
        )
        .remove(0);
        assert!(r.message.unwrap().contains("isn't valid JSON"));
    }

    #[test]
    fn disabled_assertions_are_skipped_and_indexes_kept() {
        let mut off = a(Status, Eq, Some(json!(500)));
        off.enabled = false;
        let on = a(Status, Eq, Some(json!(201)));
        let list = [off, on];
        let results = evaluate(&list, &list, &response(), &[], &[]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].index, 1);
    }

    #[test]
    fn labels() {
        assert_eq!(label(&a(Status, Eq, Some(json!(201)))), "status eq 201");
        assert_eq!(label(&body("$.id", Exists, None)), "body $.id exists");
        assert_eq!(
            label(&header("Content-Type", Contains, Some(json!("json")))),
            "header Content-Type contains \"json\""
        );
    }

    #[test]
    fn values_resolve_variables_and_secrets_never_show() {
        let resolver = Resolver::new(vec![
            Layer::new(Source::Runtime)
                .with("userId", "42")
                .with_secret("cookie", Some("s3cr3t-cookie")),
        ]);
        let authored = vec![
            body("$.id", Eq, Some(json!("{{userId}}"))),
            header("Set-Cookie", Contains, Some(json!("{{cookie}}"))),
            header("Set-Cookie", Eq, Some(json!("nope"))),
            a(BodyText, Contains, Some(json!("{{cookie}}"))),
        ];
        let resolved = resolve(&authored, &resolver);
        assert_eq!(resolved.value[0].value, Some(json!("42")));
        let results = evaluate(&authored, &resolved.value, &response(), &resolved.secrets, &[]);
        // The body has the number 42 and variables are text, but a templated value is loose.
        assert!(results[0].passed);
        assert!(results[1].passed);
        assert_eq!(results[1].label, "header Set-Cookie contains \"{{cookie}}\"");
        assert_eq!(
            results[1].actual.as_deref(),
            Some(redact::MASK),
            "sensitive header is masked"
        );
        assert_eq!(results[2].message.as_deref(), Some("the value didn't match"));
        assert!(!results[3].passed);
        for r in &results {
            let all = format!("{r:?}");
            assert!(!all.contains("s3cr3t-cookie"), "{all}");
        }
    }

    #[test]
    fn secrets_are_masked_before_shortening_and_when_json_escaped() {
        let secret = "tok-very-secret-1234567890";
        let body = format!(
            r#"{{"pad":"{}","t":"{secret}","q":"a\"b-secret\\x"}}"#,
            "x".repeat(170)
        );
        let r = ResponseData::new(200, vec![], 1.0, body.len() as u64, BodyText::Text(body));
        let x = a(BodyText, Contains, Some(json!("nope")));
        let secrets = [SecretString::from(secret), SecretString::from("a\"b-secret\\x")];
        let res = evaluate(
            std::slice::from_ref(&x),
            std::slice::from_ref(&x),
            &r,
            &secrets,
            &[],
        )
        .remove(0);
        let shown = res.actual.unwrap();
        assert!(!shown.contains("tok-ver"), "{shown}");
        let all = evaluate(
            std::slice::from_ref(&x),
            std::slice::from_ref(&x),
            &ResponseData::new(
                200,
                vec![],
                1.0,
                20,
                BodyText::Text(r#"{"q":"a\"b-secret\\x"}"#.into()),
            ),
            &secrets,
            &[],
        )
        .remove(0);
        assert!(!all.actual.unwrap().contains("b-secret"));
    }

    #[test]
    fn long_actual_values_are_shortened() {
        let long = "x".repeat(500);
        let r = ResponseData::new(200, vec![], 1.0, 500, BodyText::Text(long));
        let x = a(BodyText, Eq, Some(json!("y")));
        let res = evaluate(std::slice::from_ref(&x), std::slice::from_ref(&x), &r, &[], &[]).remove(0);
        assert!(res.actual.unwrap().chars().count() <= MAX_SHOWN + 3);
    }
}
