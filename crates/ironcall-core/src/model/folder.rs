use serde::{Deserialize, Serialize};

use super::style;
use super::{Auth, Extra, KeyValue, Vars};
use crate::ItemId;

/// `ironcall.yaml` at the workspace root (PLAN.md §4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct CollectionFile {
    pub schema: String,
    /// Workspace ID, committed so every clone shares it (PLAN.md §4.5).
    pub id: ItemId,
    pub name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "style::multiline_opt"
    )]
    #[cfg_attr(feature = "specta", specta(type = Option<String>))]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vars::is_empty")]
    pub vars: Vars,
    /// Default headers sent with every request in the collection.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "style::flow_rows"
    )]
    #[cfg_attr(feature = "specta", specta(type = Vec<KeyValue>))]
    pub headers: Vec<KeyValue>,
    /// `inherit` at the root means no auth.
    #[serde(default, skip_serializing_if = "Auth::is_inherit")]
    pub auth: Auth,
    /// Display order of top-level folders and requests, by ID (PLAN.md §4.3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<ItemId>,
    #[serde(flatten)]
    #[cfg_attr(feature = "specta", specta(type = super::ExtraJs))]
    pub extra: Extra,
}

/// `_folder.yaml` inside each folder (PLAN.md §4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct FolderFile {
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
    #[serde(default, skip_serializing_if = "Vars::is_empty")]
    pub vars: Vars,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "style::flow_rows"
    )]
    #[cfg_attr(feature = "specta", specta(type = Vec<KeyValue>))]
    pub headers: Vec<KeyValue>,
    #[serde(default, skip_serializing_if = "Auth::is_inherit")]
    pub auth: Auth,
    /// Display order of child folders and requests, by ID (PLAN.md §4.3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<ItemId>,
    #[serde(flatten)]
    #[cfg_attr(feature = "specta", specta(type = super::ExtraJs))]
    pub extra: Extra,
}
