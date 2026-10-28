use serde::{Deserialize, Serialize};

use super::{Extra, Vars};
use crate::ItemId;

/// `environments/<name>.yaml` (PLAN.md §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct EnvironmentFile {
    pub schema: String,
    pub id: ItemId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vars::is_empty")]
    pub vars: Vars,
    #[serde(flatten)]
    #[cfg_attr(feature = "specta", specta(type = super::ExtraJs))]
    pub extra: Extra,
}
