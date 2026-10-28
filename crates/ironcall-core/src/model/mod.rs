//! On-disk workspace file schema, version 1 (PLAN.md §4).
//!
//! These types are exactly what is stored in `ironcall.yaml`, `_folder.yaml`,
//! `environments/*.yaml` and request files. Reading and writing them lives in
//! `ironcall-store::yaml`.
//!
//! Every struct, including list items, has a flattened `extra` map so keys written by
//! newer versions (or by hand) survive a load/save round-trip. Unknown keys are written
//! after the known ones.

mod common;
mod environment;
mod folder;
mod request;
pub(crate) mod style;

pub use common::{ApiKeyLocation, Auth, KeyValue, Variable, VariableSpec, Vars};
pub use environment::EnvironmentFile;
pub use folder::{CollectionFile, FolderFile};
pub use request::{
    AssertOp, AssertTarget, Assertion, Body, ExtractSource, Extraction, HttpVersion, MultipartKind,
    MultipartPart, PathParam, RequestFile, RequestSettings,
};

/// Unknown keys preserved across a round-trip.
pub type Extra = indexmap::IndexMap<String, serde_json::Value>;

/// TypeScript shape of `serde_json::Value`. Numbers are exported as `number` (specta
/// refuses `i64`/`u64`); values outside the safe-integer range lose precision in JS.
#[cfg(feature = "specta")]
#[derive(serde::Serialize, specta::Type)]
#[serde(untagged, rename = "JsonValue")]
#[allow(dead_code)]
pub(crate) enum JsonValueJs {
    Null(()),
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValueJs>),
    Object(std::collections::BTreeMap<String, JsonValueJs>),
}

/// TypeScript shape of [`Extra`]: `{ [key: string]: unknown }`, so the UI can intersect
/// it with known fields and still assign to them.
///
/// Enum variants (`Auth`, `Body`) skip their `extra` in TypeScript because specta exports
/// flattened variant maps as a literal `extra` field. Unknown keys still round-trip at
/// runtime as long as the UI copies objects with spreads.
#[cfg(feature = "specta")]
pub(crate) type ExtraJs = std::collections::BTreeMap<String, specta_typescript::Unknown>;

/// `schema` values for version 1 of each file kind.
pub mod schema {
    pub const COLLECTION: &str = "ironcall/collection@1";
    pub const FOLDER: &str = "ironcall/folder@1";
    pub const REQUEST: &str = "ironcall/request@1";
    pub const ENVIRONMENT: &str = "ironcall/environment@1";
}
