//! Extraction: saves parts of a response into runtime variables (PLAN.md Phase 7).

use regex::Regex;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{ExtractSource, Extraction};
use crate::redact;
use crate::response::{ResponseData, json_path};

/// The outcome of one extraction. Holds the value itself; never serialize this, use
/// [`Extracted::view`].
#[derive(Clone, Debug)]
pub struct Extracted {
    pub var: String,
    pub value: Result<SecretString, String>,
    /// Effective sensitivity (see [`is_sensitive`]).
    pub sensitive: bool,
}

/// What the UI shows for an extraction: sensitive values masked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ExtractedView {
    pub var: String,
    /// The value, or `••••` if sensitive. `None` if extraction failed.
    pub value: Option<String>,
    pub sensitive: bool,
    pub error: Option<String>,
}

impl Extracted {
    pub fn view(&self) -> ExtractedView {
        let (value, error) = match &self.value {
            Ok(_) if self.sensitive => (Some(redact::MASK.to_string()), None),
            Ok(v) => (Some(v.expose_secret().to_string()), None),
            Err(e) => (None, Some(e.clone())),
        };
        ExtractedView {
            var: self.var.clone(),
            value,
            sensitive: self.sensitive,
            error,
        }
    }
}

/// Whether an extracted value is sensitive (PLAN.md §6.2): marked so, or taken from an
/// auth-like source. Cookies, sensitive headers (`Set-Cookie`, `Authorization`, …) and
/// token- or password-like variable names and JSON fields always count.
pub fn is_sensitive(e: &Extraction, sensitive_headers: &[String]) -> bool {
    e.sensitive
        || e.from == ExtractSource::Cookie
        || redact::is_sensitive_name(&e.var)
        || (e.from == ExtractSource::Header
            && e.name
                .as_deref()
                .is_some_and(|n| redact::is_sensitive_header(n, sensitive_headers)))
        || (e.from == ExtractSource::Body && e.path.as_deref().is_some_and(redact::is_sensitive_name))
}

/// Runs each extraction against the response, in order.
pub fn run(list: &[Extraction], response: &ResponseData, sensitive_headers: &[String]) -> Vec<Extracted> {
    list.iter()
        .map(|e| Extracted {
            var: e.var.trim().to_string(),
            value: if e.var.trim().is_empty() {
                Err("no variable name".into())
            } else {
                one(e, response).map(SecretString::from)
            },
            sensitive: is_sensitive(e, sensitive_headers),
        })
        .collect()
}

/// Whether any extraction needs the body (so the caller can skip loading it).
pub fn needs_body(list: &[Extraction]) -> bool {
    list.iter()
        .any(|e| matches!(e.from, ExtractSource::Body | ExtractSource::Regex))
}

fn one(e: &Extraction, r: &ResponseData) -> Result<String, String> {
    let field = |v: &Option<String>, what: &str| {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("no {what}"))
    };
    match e.from {
        ExtractSource::Body => {
            let path = e.path.as_deref().unwrap_or("$");
            match json_path(r.json()?, path)? {
                Some(Value::String(s)) => Ok(s),
                Some(other) => Ok(other.to_string()),
                None => Err("the JSONPath matched nothing".into()),
            }
        }
        ExtractSource::Header => {
            let name = field(&e.name, "header name")?;
            r.header_values(&name)
                .next()
                .map(str::to_string)
                .ok_or_else(|| "the header isn't in the response".into())
        }
        ExtractSource::Cookie => {
            let name = field(&e.name, "cookie name")?;
            r.cookie(&name)
                .map(str::to_string)
                .ok_or_else(|| "the response doesn't set this cookie".into())
        }
        ExtractSource::Regex => {
            let pattern = field(&e.pattern, "pattern")?;
            let re = Regex::new(&pattern).map_err(|e| format!("invalid regex: {e}"))?;
            let caps = re.captures(r.text()?).ok_or("the pattern matched nothing")?;
            Ok(caps
                .get(1)
                .or_else(|| caps.get(0))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response::BodyText;
    use serde_json::json;

    fn response() -> ResponseData {
        let body =
            json!({"id": 7, "user": {"name": "Ada"}, "access_token": "tok-123", "list": [1, 2]}).to_string();
        ResponseData::new(
            200,
            vec![
                ("Location".into(), "/users/7".into()),
                ("Set-Cookie".into(), "sid=abc; HttpOnly".into()),
                ("Authorization".into(), "Bearer z".into()),
            ],
            5.0,
            body.len() as u64,
            BodyText::Text(body),
        )
    }

    fn ex(var: &str, from: ExtractSource) -> Extraction {
        Extraction {
            var: var.into(),
            from,
            path: None,
            name: None,
            pattern: None,
            sensitive: false,
            extra: Default::default(),
        }
    }

    fn value(e: Extraction) -> Result<String, String> {
        run(&[e], &response(), &[])
            .remove(0)
            .value
            .map(|v| v.expose_secret().to_string())
    }

    #[test]
    fn body_header_cookie_and_regex() {
        let body = |p: &str| Extraction {
            path: Some(p.into()),
            ..ex("v", ExtractSource::Body)
        };
        assert_eq!(value(body("$.id")).unwrap(), "7", "numbers as JSON text");
        assert_eq!(
            value(body("$.user.name")).unwrap(),
            "Ada",
            "strings without quotes"
        );
        assert_eq!(value(body("$.list")).unwrap(), "[1,2]");
        assert!(value(body("$.nope")).unwrap_err().contains("matched nothing"));

        let header = Extraction {
            name: Some("location".into()),
            ..ex("loc", ExtractSource::Header)
        };
        assert_eq!(value(header).unwrap(), "/users/7");
        let cookie = Extraction {
            name: Some("sid".into()),
            ..ex("sid", ExtractSource::Cookie)
        };
        assert_eq!(value(cookie).unwrap(), "abc");
        let regex = |p: &str| Extraction {
            pattern: Some(p.into()),
            ..ex("r", ExtractSource::Regex)
        };
        assert_eq!(value(regex(r#""name":"(\w+)""#)).unwrap(), "Ada", "first group");
        assert_eq!(
            value(regex(r"tok-\d+")).unwrap(),
            "tok-123",
            "whole match without a group"
        );
        assert!(value(regex("(")).unwrap_err().contains("invalid regex"));
        assert!(value(regex("zzz")).unwrap_err().contains("matched nothing"));
        assert!(
            value(ex("x", ExtractSource::Header))
                .unwrap_err()
                .contains("no header name")
        );
        assert!(
            value(ex(" ", ExtractSource::Body))
                .unwrap_err()
                .contains("no variable name")
        );
    }

    #[test]
    fn sensitivity_defaults_for_auth_like_sources() {
        let plain = ex("userId", ExtractSource::Body);
        assert!(!is_sensitive(&plain, &[]));
        assert!(is_sensitive(
            &Extraction {
                sensitive: true,
                ..plain.clone()
            },
            &[]
        ));
        assert!(is_sensitive(&ex("sid", ExtractSource::Cookie), &[]));
        assert!(
            is_sensitive(&ex("authToken", ExtractSource::Body), &[]),
            "token-like variable name"
        );
        let path = Extraction {
            path: Some("$.access_token".into()),
            ..plain.clone()
        };
        assert!(is_sensitive(&path, &[]), "token-like JSON field");
        let header = |n: &str| Extraction {
            name: Some(n.into()),
            ..ex("h", ExtractSource::Header)
        };
        assert!(is_sensitive(&header("Authorization"), &[]));
        assert!(!is_sensitive(&header("Location"), &[]));
        assert!(is_sensitive(&header("X-Custom"), &["x-custom".into()]));
    }

    #[test]
    fn views_mask_sensitive_values() {
        let list = [
            Extraction {
                path: Some("$.access_token".into()),
                ..ex("tok", ExtractSource::Body)
            },
            Extraction {
                path: Some("$.id".into()),
                ..ex("id", ExtractSource::Body)
            },
        ];
        let views: Vec<_> = run(&list, &response(), &[]).iter().map(Extracted::view).collect();
        assert_eq!(views[0].value.as_deref(), Some(redact::MASK));
        assert_eq!(views[1].value.as_deref(), Some("7"));
        assert!(!format!("{views:?}").contains("tok-123"));
        assert!(
            !format!("{:?}", run(&list, &response(), &[])).contains("tok-123"),
            "Debug is redacted"
        );
    }

    #[test]
    fn needs_body_only_for_body_and_regex() {
        assert!(!needs_body(&[
            ex("a", ExtractSource::Header),
            ex("b", ExtractSource::Cookie)
        ]));
        assert!(needs_body(&[ex("a", ExtractSource::Regex)]));
    }
}
