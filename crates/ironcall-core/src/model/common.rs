//! Types shared by several file kinds: key/value rows, auth and variables.

use std::fmt;

use indexmap::IndexMap;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use super::Extra;
use super::style;

/// A header, query param or form field row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct KeyValue {
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default = "style::default_true")]
    pub enabled: bool,
    #[serde(flatten)]
    #[cfg_attr(feature = "specta", specta(type = super::ExtraJs))]
    pub extra: Extra,
}

impl KeyValue {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            enabled: true,
            extra: Extra::new(),
        }
    }
}

/// Where an API key is sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ApiKeyLocation {
    #[default]
    Header,
    Query,
}

/// Authentication for a request, folder or collection (PLAN.md Phase 4).
///
/// Secret parts (passwords, tokens, keys) normally hold `{{variable}}` references to
/// secret variables rather than literal values.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Auth {
    /// Use the parent folder's or collection's auth.
    #[default]
    Inherit,
    None {
        #[serde(flatten)]
        #[cfg_attr(feature = "specta", specta(skip))]
        extra: Extra,
    },
    Basic {
        #[serde(default)]
        username: String,
        #[serde(default)]
        password: String,
        #[serde(flatten)]
        #[cfg_attr(feature = "specta", specta(skip))]
        extra: Extra,
    },
    Bearer {
        #[serde(default)]
        token: String,
        #[serde(flatten)]
        #[cfg_attr(feature = "specta", specta(skip))]
        extra: Extra,
    },
    #[serde(rename_all = "camelCase")]
    ApiKey {
        key: String,
        #[serde(default)]
        value: String,
        #[serde(default, rename = "in")]
        location: ApiKeyLocation,
        #[serde(flatten)]
        #[cfg_attr(feature = "specta", specta(skip))]
        extra: Extra,
    },
}

impl Auth {
    pub fn is_inherit(&self) -> bool {
        matches!(self, Self::Inherit)
    }
}

/// Variables at one scope, in file order.
pub type Vars = IndexMap<String, Variable>;

/// A variable definition: either a plain value (`baseUrl: "https://…"`) or a detailed
/// form, used for secrets whose value lives in the secret store (`apiKey: {secret: true}`).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type), specta(type = VariableJs))]
pub enum Variable {
    Plain(String),
    Detailed(VariableSpec),
}

/// TypeScript shape of [`Variable`], which has hand-written serde impls: a plain string
/// or a [`VariableSpec`] object.
#[cfg(feature = "specta")]
#[derive(Serialize, specta::Type)]
#[serde(untagged, rename = "Variable")]
#[allow(dead_code)]
enum VariableJs {
    Plain(String),
    Detailed(VariableSpec),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct VariableSpec {
    /// Value for non-secret variables. Never set for secrets.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "style::multiline_opt"
    )]
    #[cfg_attr(feature = "specta", specta(type = Option<String>))]
    pub value: Option<String>,
    /// The value lives in the workspace's secret store, never in this file (PLAN.md §6).
    #[serde(default, skip_serializing_if = "style::is_false")]
    pub secret: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "style::multiline_opt"
    )]
    #[cfg_attr(feature = "specta", specta(type = Option<String>))]
    pub description: Option<String>,
    #[serde(flatten)]
    #[cfg_attr(feature = "specta", specta(type = super::ExtraJs))]
    pub extra: Extra,
}

impl Variable {
    pub fn is_secret(&self) -> bool {
        matches!(self, Self::Detailed(spec) if spec.secret)
    }
}

impl Serialize for Variable {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Plain(value) => style::multiline(value, serializer),
            Self::Detailed(spec) => spec.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Variable {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct VariableVisitor;

        impl<'de> Visitor<'de> for VariableVisitor {
            type Value = Variable;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string value or a map like {secret: true}")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Variable, E> {
                Ok(Variable::Plain(v.to_owned()))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<Variable, E> {
                Ok(Variable::Plain(v))
            }

            // Unquoted scalars in hand-edited files (`port: 8080`, `debug: true`) are
            // still variables; variables are always text.
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Variable, E> {
                Ok(Variable::Plain(v.to_string()))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Variable, E> {
                Ok(Variable::Plain(v.to_string()))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Variable, E> {
                Ok(Variable::Plain(v.to_string()))
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Variable, E> {
                Ok(Variable::Plain(v.to_string()))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Variable, A::Error> {
                VariableSpec::deserialize(de::value::MapAccessDeserializer::new(map)).map(Variable::Detailed)
            }
        }

        deserializer.deserialize_any(VariableVisitor)
    }
}
