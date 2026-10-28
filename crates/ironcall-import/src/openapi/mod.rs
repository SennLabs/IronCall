//! OpenAPI 3.0/3.1 and Swagger 2.0 import (PLAN.md Phase 6).
//!
//! [`plan`] turns a spec into a [`Plan`]: folders and requests to create, environment
//! variables and collection auth. It writes nothing; the app shows the plan as a preview
//! and then creates the files through the normal workspace operations.
//!
//! **Parsing.** Every version is read into a `serde_json::Value` (JSON directly, YAML via
//! serde-saphyr) and mapped by hand, rather than through the typed `openapiv3`/`oas3`
//! crates the plan suggested. Real-world specs routinely break the strict typed models
//! (missing required fields, vendor extensions in odd places, 3.0/3.1 mixtures), and one
//! tolerant reader covering all three versions is simpler than three strict ones. Only
//! the parts needed to build requests are read; anything odd becomes a warning.
//!
//! **Bodies** come only from examples in the spec (`example`, `examples`, or a
//! schema-level `example`); bodies are never generated from schemas in V1.

mod build;
mod ops;
mod refs;

use ironcall_core::ItemId;
use ironcall_core::model::{Auth, RequestFile};
use serde::Serialize;
use serde_json::Value;

/// Which kind of spec was imported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SpecVersion {
    Swagger2,
    OpenApi30,
    OpenApi31,
}

/// A problem that didn't stop the import.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Warning {
    pub message: String,
    /// A JSON pointer (`#/paths/~1pets/get`), an operation (`GET /pets`) or a scheme name.
    pub location: Option<String>,
}

/// A variable for the environment the import creates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EnvVar {
    pub name: String,
    /// Empty for secrets: their values are entered by the user.
    pub value: String,
    pub secret: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PlanItem {
    Folder {
        name: String,
        description: Option<String>,
        items: Vec<PlanItem>,
    },
    Request(Box<RequestFile>),
}

/// What an import would create.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub version: SpecVersion,
    pub title: String,
    pub description: Option<String>,
    /// Server URLs, first one first (it becomes `baseUrl`).
    pub servers: Vec<String>,
    pub environment: Vec<EnvVar>,
    /// Auth for the collection (or the wrapping folder) from the global `security`.
    pub root_auth: Option<Auth>,
    /// One line per security scheme mapping, for the preview.
    pub auth_summary: Vec<String>,
    pub tree: Vec<PlanItem>,
    pub warnings: Vec<Warning>,
}

impl Plan {
    pub fn request_count(&self) -> usize {
        fn count(items: &[PlanItem]) -> usize {
            items
                .iter()
                .map(|i| match i {
                    PlanItem::Folder { items, .. } => count(items),
                    PlanItem::Request(_) => 1,
                })
                .sum()
        }
        count(&self.tree)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("the file is neither valid JSON nor valid YAML: {0}")]
    Parse(String),
    #[error("the file isn't an OpenAPI or Swagger document (no `openapi` or `swagger` field)")]
    NotASpec,
    #[error("unsupported spec version `{0}` (supported: Swagger 2.0, OpenAPI 3.0 and 3.1)")]
    UnsupportedVersion(String),
}

/// Plans an import with fresh IDs.
pub fn plan(bytes: &[u8]) -> Result<Plan, ImportError> {
    plan_with_ids(bytes, &mut ItemId::new)
}

/// Like [`plan`], with IDs from `next_id` (deterministic IDs for tests).
pub fn plan_with_ids(bytes: &[u8], next_id: &mut dyn FnMut() -> ItemId) -> Result<Plan, ImportError> {
    let root = parse(bytes)?;
    let version = detect_version(&root)?;
    let doc = refs::Doc::new(&root);
    let spec = match version {
        SpecVersion::Swagger2 => ops::read_swagger2(&doc),
        SpecVersion::OpenApi30 | SpecVersion::OpenApi31 => ops::read_openapi3(&doc),
    };
    if version == SpecVersion::OpenApi31 && root.get("webhooks").is_some_and(|w| !w.is_null()) {
        doc.warn(
            "webhooks aren't imported (they're requests the API sends, not ones you send)",
            Some("#/webhooks"),
        );
    }
    Ok(build::build(version, spec, doc.take_warnings(), next_id))
}

/// JSON or YAML into a `Value`. A leading BOM is ignored.
fn parse(bytes: &[u8]) -> Result<Value, ImportError> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|e| ImportError::Parse(format!("not UTF-8 text ({e})")))?;
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') {
        return serde_json::from_str(text).map_err(|e| ImportError::Parse(e.to_string()));
    }
    serde_saphyr::from_str::<Value>(text).map_err(|e| ImportError::Parse(e.to_string()))
}

fn detect_version(root: &Value) -> Result<SpecVersion, ImportError> {
    let as_text = |v: &Value| match v {
        Value::String(s) => Some(s.trim().to_string()),
        // YAML `swagger: 2.0` / `openapi: 3.0` (unquoted) reads as a number.
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    if let Some(v) = root.get("swagger").and_then(as_text) {
        return if v == "2.0" || v == "2" {
            Ok(SpecVersion::Swagger2)
        } else {
            Err(ImportError::UnsupportedVersion(v))
        };
    }
    let Some(v) = root.get("openapi").and_then(as_text) else {
        return Err(ImportError::NotASpec);
    };
    if v.starts_with("3.0") || v == "3" {
        Ok(SpecVersion::OpenApi30)
    } else if v.starts_with("3.1") {
        Ok(SpecVersion::OpenApi31)
    } else {
        Err(ImportError::UnsupportedVersion(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_versions() {
        let v = |text: &str| detect_version(&parse(text.as_bytes()).unwrap());
        assert_eq!(v(r#"{"swagger": "2.0"}"#).unwrap(), SpecVersion::Swagger2);
        assert_eq!(v("openapi: 3.0.3\n").unwrap(), SpecVersion::OpenApi30);
        assert_eq!(v("openapi: '3.1.0'\n").unwrap(), SpecVersion::OpenApi31);
        assert!(matches!(
            v("openapi: 4.0.0\n"),
            Err(ImportError::UnsupportedVersion(_))
        ));
        assert!(matches!(v(r#"{"info": {}}"#), Err(ImportError::NotASpec)));
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        for input in [
            &b""[..],
            b"\xff\xfe",
            b"{",
            b"[1, 2",
            b": :\n  - ]",
            b"null",
            b"42",
        ] {
            assert!(plan(input).is_err(), "{input:?}");
        }
    }
}
