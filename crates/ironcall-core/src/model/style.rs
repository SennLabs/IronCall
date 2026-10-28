//! Serialization style hints for stable, diff-friendly YAML (PLAN.md §4.4).
//!
//! These only affect the YAML emitter: multiline strings become `|` literal blocks and
//! table rows (headers, params, assertions) become one-line flow maps, so each row is
//! one line in a diff. Other serde formats see plain strings and maps.

use serde::{Serialize, Serializer};
use serde_saphyr::{FlowMap, LitStr};

/// Writes strings containing a newline as a `|` block, others as normal scalars.
pub fn multiline<S: Serializer>(value: &str, serializer: S) -> Result<S::Ok, S::Error> {
    if value.contains('\n') {
        LitStr(value).serialize(serializer)
    } else {
        serializer.serialize_str(value)
    }
}

pub fn multiline_opt<S: Serializer>(value: &Option<String>, serializer: S) -> Result<S::Ok, S::Error> {
    match value {
        Some(v) if v.contains('\n') => serializer.serialize_some(&LitStr(v)),
        Some(v) => serializer.serialize_some(v),
        None => serializer.serialize_none(),
    }
}

/// Writes each list item as a single-line flow map: `- {name: 'a', value: 'b'}`.
pub fn flow_rows<T: Serialize, S: Serializer>(rows: &[T], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(rows.iter().map(FlowMap))
}

pub fn is_false(value: &bool) -> bool {
    !*value
}

pub fn is_true(value: &bool) -> bool {
    *value
}

pub fn default_true() -> bool {
    true
}
