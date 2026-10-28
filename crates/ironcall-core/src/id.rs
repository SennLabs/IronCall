//! Stable identifiers for workspace items (PLAN.md §4.3).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// Stable, lifetime ID of a request, folder, environment or workspace.
///
/// IDs are ULIDs: sortable by creation time and safe in file contents and URLs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ItemId(#[cfg_attr(feature = "specta", specta(type = String))] Ulid);

impl ItemId {
    pub fn new() -> Self {
        Self(Ulid::generate())
    }
}

impl Default for ItemId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for ItemId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_string() {
        let id = ItemId::new();
        assert_eq!(id.to_string().parse::<ItemId>().unwrap(), id);
    }

    #[test]
    fn new_ids_are_unique() {
        assert_ne!(ItemId::new(), ItemId::new());
    }
}
