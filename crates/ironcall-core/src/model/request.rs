use serde::{Deserialize, Serialize};

use super::style;
use super::{Auth, Extra, KeyValue, Vars};
use crate::ItemId;

/// One request file, e.g. `users/create-user.yaml` (PLAN.md §4.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RequestFile {
    pub schema: String,
    pub id: ItemId,
    pub name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "style::multiline_opt"
    )]
    #[cfg_attr(feature = "specta", specta(type = Option<String>))]
    pub description: Option<String>,
    /// Any HTTP method token, including custom ones (`PROPFIND`).
    pub method: String,
    pub url: String,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "style::flow_rows"
    )]
    #[cfg_attr(feature = "specta", specta(type = Vec<PathParam>))]
    pub path_params: Vec<PathParam>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "style::flow_rows"
    )]
    #[cfg_attr(feature = "specta", specta(type = Vec<KeyValue>))]
    pub params: Vec<KeyValue>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "style::flow_rows"
    )]
    #[cfg_attr(feature = "specta", specta(type = Vec<KeyValue>))]
    pub headers: Vec<KeyValue>,
    #[serde(default, skip_serializing_if = "Auth::is_inherit")]
    pub auth: Auth,
    #[serde(default, skip_serializing_if = "Body::is_none")]
    pub body: Body,
    #[serde(default, skip_serializing_if = "Vars::is_empty")]
    pub vars: Vars,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "style::flow_rows"
    )]
    #[cfg_attr(feature = "specta", specta(type = Vec<Extraction>))]
    pub extract: Vec<Extraction>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "style::flow_rows"
    )]
    #[cfg_attr(feature = "specta", specta(type = Vec<Assertion>))]
    pub assert: Vec<Assertion>,
    #[serde(default, skip_serializing_if = "RequestSettings::is_empty")]
    pub settings: RequestSettings,
    #[serde(flatten)]
    #[cfg_attr(feature = "specta", specta(type = super::ExtraJs))]
    pub extra: Extra,
}

/// A `:name` path segment value (PLAN.md §5).
impl RequestFile {
    /// A new, empty `GET` request with a fresh ID.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema: super::schema::REQUEST.to_string(),
            id: ItemId::new(),
            name: name.into(),
            description: None,
            method: "GET".to_string(),
            url: String::new(),
            path_params: Vec::new(),
            params: Vec::new(),
            headers: Vec::new(),
            auth: Auth::default(),
            body: Body::default(),
            vars: Vars::default(),
            extract: Vec::new(),
            assert: Vec::new(),
            settings: RequestSettings::default(),
            extra: Extra::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct PathParam {
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(flatten)]
    #[cfg_attr(feature = "specta", specta(type = super::ExtraJs))]
    pub extra: Extra,
}

/// Request body (PLAN.md Phases 1 and 4).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Body {
    #[default]
    None,
    Json {
        #[serde(default, serialize_with = "style::multiline")]
        #[cfg_attr(feature = "specta", specta(type = String))]
        content: String,
        #[serde(flatten)]
        #[cfg_attr(feature = "specta", specta(skip))]
        extra: Extra,
    },
    Text {
        #[serde(default, serialize_with = "style::multiline")]
        #[cfg_attr(feature = "specta", specta(type = String))]
        content: String,
        #[serde(flatten)]
        #[cfg_attr(feature = "specta", specta(skip))]
        extra: Extra,
    },
    Xml {
        #[serde(default, serialize_with = "style::multiline")]
        #[cfg_attr(feature = "specta", specta(type = String))]
        content: String,
        #[serde(flatten)]
        #[cfg_attr(feature = "specta", specta(skip))]
        extra: Extra,
    },
    FormUrlencoded {
        #[serde(default, serialize_with = "style::flow_rows")]
        #[cfg_attr(feature = "specta", specta(type = Vec<KeyValue>))]
        fields: Vec<KeyValue>,
        #[serde(flatten)]
        #[cfg_attr(feature = "specta", specta(skip))]
        extra: Extra,
    },
    Multipart {
        #[serde(default)]
        parts: Vec<MultipartPart>,
        #[serde(flatten)]
        #[cfg_attr(feature = "specta", specta(skip))]
        extra: Extra,
    },
    /// Sends a file's bytes as the body. `path` is relative to the workspace root, with `/`.
    BinaryFile {
        path: String,
        #[serde(default, rename = "contentType", skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
        #[serde(flatten)]
        #[cfg_attr(feature = "specta", specta(skip))]
        extra: Extra,
    },
}

impl Body {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum MultipartKind {
    #[default]
    Text,
    File,
}

/// One part of a multipart/form-data body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct MultipartPart {
    pub name: String,
    #[serde(default, rename = "type")]
    pub kind: MultipartKind,
    /// Text value, for `type: text`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "style::multiline_opt"
    )]
    #[cfg_attr(feature = "specta", specta(type = Option<String>))]
    pub value: Option<String>,
    /// File path relative to the workspace root, for `type: file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default = "style::default_true")]
    pub enabled: bool,
    #[serde(flatten)]
    #[cfg_attr(feature = "specta", specta(type = super::ExtraJs))]
    pub extra: Extra,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ExtractSource {
    /// JSONPath into the response body (`path`).
    Body,
    /// Response header (`name`).
    Header,
    /// Response cookie (`name`).
    Cookie,
    /// Regex over the body text (`pattern`, first capture group).
    Regex,
}

/// Saves part of a response into a runtime variable (PLAN.md Phase 7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Extraction {
    pub var: String,
    pub from: ExtractSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Masks the value in the UI, history and logs (PLAN.md §6.2).
    #[serde(default)]
    pub sensitive: bool,
    #[serde(flatten)]
    #[cfg_attr(feature = "specta", specta(type = super::ExtraJs))]
    pub extra: Extra,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum AssertTarget {
    Status,
    Header,
    /// JSONPath into the body (`path`).
    Body,
    /// Whole body as text.
    BodyText,
    /// Total duration in milliseconds.
    Duration,
    /// Body size in bytes.
    Size,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum AssertOp {
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
    Contains,
    Matches,
    Exists,
    NotExists,
    Type,
    In,
}

/// A declarative check on the response (PLAN.md Phase 7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Assertion {
    pub target: AssertTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub op: AssertOp,
    /// Expected value. Absent for `exists` / `notExists`. Typed: `201` and `"201"` differ.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "specta", specta(type = Option<super::JsonValueJs>))]
    pub value: Option<serde_json::Value>,
    #[serde(default = "style::default_true", skip_serializing_if = "style::is_true")]
    pub enabled: bool,
    #[serde(flatten)]
    #[cfg_attr(feature = "specta", specta(type = super::ExtraJs))]
    pub extra: Extra,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum HttpVersion {
    #[default]
    Auto,
    Http1,
    Http2,
}

/// Per-request overrides. Absent fields use the folder, collection or app default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RequestSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "specta", specta(type = Option<f64>))]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_redirects: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_redirects: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_tls: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_version: Option<HttpVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresh_connection: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encode_url: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypass_proxy: Option<bool>,
    #[serde(flatten)]
    #[cfg_attr(feature = "specta", specta(type = super::ExtraJs))]
    pub extra: Extra,
}

impl RequestSettings {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}
