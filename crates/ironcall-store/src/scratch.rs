//! Scratch requests: tabs not saved to any workspace (PLAN.md §4.6).
//!
//! Stored as request YAML files named by ID in the app data dir, so they survive
//! restarts. Files that can't be read are skipped (and logged), never deleted.
//!
//! Folders are a display layout only: `folders.json` lists them and says which folder
//! each request is in. Request files stay where they are, so autosave and lookups by ID
//! don't depend on the layout, and a missing or corrupt layout just shows a flat list.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ironcall_core::ItemId;
use ironcall_core::model::RequestFile;

use serde::{Deserialize, Serialize};

use crate::atomic;
use crate::session::{read_json, write_json};
use crate::yaml;

const LAYOUT_FILE: &str = "folders.json";

#[derive(Debug, thiserror::Error)]
pub enum ScratchError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("scratch request {0}: {1}")]
    Invalid(String, String),
}

/// A folder in the scratch list. Folders nest through `parent`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScratchFolder {
    pub id: ItemId,
    pub name: String,
    #[serde(default)]
    pub parent: Option<ItemId>,
}

/// How scratch requests are grouped into folders.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScratchLayout {
    /// In display order.
    #[serde(default)]
    pub folders: Vec<ScratchFolder>,
    /// Request ID → folder ID. Requests not listed are at the top level.
    #[serde(default)]
    pub placement: BTreeMap<ItemId, ItemId>,
}

impl ScratchLayout {
    /// Makes the layout consistent: unique folder IDs, parents that exist, no cycles,
    /// non-empty names, and placements only into known folders.
    pub fn normalize(&mut self) {
        let mut seen = HashSet::new();
        self.folders.retain(|f| seen.insert(f.id));
        for f in &mut self.folders {
            if f.parent.is_some_and(|p| !seen.contains(&p) || p == f.id) {
                f.parent = None;
            }
            let trimmed = f.name.trim();
            f.name = if trimmed.is_empty() {
                "New folder".to_string()
            } else {
                trimmed.to_string()
            };
        }
        // Break cycles: a folder whose ancestor chain comes back to itself moves to the top.
        for i in 0..self.folders.len() {
            let parents: HashMap<ItemId, ItemId> = self
                .folders
                .iter()
                .filter_map(|f| f.parent.map(|p| (f.id, p)))
                .collect();
            let start = self.folders[i].id;
            let mut at = parents.get(&start).copied();
            let mut steps = 0;
            while let Some(p) = at {
                if p == start || steps > parents.len() {
                    self.folders[i].parent = None;
                    break;
                }
                at = parents.get(&p).copied();
                steps += 1;
            }
        }
        self.placement.retain(|_, folder| seen.contains(folder));
    }
}

#[derive(Debug, Clone)]
pub struct ScratchStore {
    dir: PathBuf,
}

impl ScratchStore {
    pub fn open(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: ItemId) -> PathBuf {
        self.dir.join(format!("{id}.yaml"))
    }

    /// All readable scratch requests, most recently modified first.
    pub fn list(&self) -> std::io::Result<Vec<RequestFile>> {
        let mut found: Vec<(SystemTime, RequestFile)> = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.extension().is_none_or(|e| e != "yaml") {
                continue;
            }
            match read(&path) {
                Ok(file) => {
                    let mtime = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    found.push((mtime, file));
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "skipping unreadable scratch request")
                }
            }
        }
        found.sort_by_key(|f| std::cmp::Reverse(f.0));
        Ok(found.into_iter().map(|(_, f)| f).collect())
    }

    pub fn load(&self, id: ItemId) -> Result<RequestFile, ScratchError> {
        read(&self.path(id))
    }

    pub fn save(&self, request: &RequestFile) -> Result<(), ScratchError> {
        let text = yaml::to_string(request)
            .map_err(|e| ScratchError::Invalid(request.id.to_string(), e.to_string()))?;
        atomic::write_atomic(&self.path(request.id), text.as_bytes())?;
        Ok(())
    }

    /// Deletes a scratch request (and its folder placement). Missing files are fine.
    pub fn delete(&self, id: ItemId) -> std::io::Result<()> {
        match std::fs::remove_file(self.path(id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        let mut layout = self.layout();
        if layout.placement.remove(&id).is_some() {
            self.save_layout(&layout)?;
        }
        Ok(())
    }

    /// The folder layout. Missing or unreadable gives an empty (flat) layout.
    pub fn layout(&self) -> ScratchLayout {
        let mut layout: ScratchLayout = read_json(&self.dir.join(LAYOUT_FILE)).unwrap_or_default();
        layout.normalize();
        layout
    }

    /// Replaces the folder layout (normalised first). Placements of requests that no
    /// longer exist are dropped.
    pub fn save_layout(&self, layout: &ScratchLayout) -> std::io::Result<()> {
        let mut layout = layout.clone();
        layout.normalize();
        layout.placement.retain(|id, _| self.path(*id).is_file());
        write_json(&self.dir.join(LAYOUT_FILE), &layout)
    }

    /// Adds a folder at the end of the layout.
    pub fn create_folder(&self, name: &str, parent: Option<ItemId>) -> std::io::Result<ScratchFolder> {
        let mut layout = self.layout();
        let folder = ScratchFolder {
            id: ItemId::new(),
            name: name.to_string(),
            parent,
        };
        layout.folders.push(folder);
        self.save_layout(&layout)?;
        // Normalised on save (an unknown parent becomes the top level).
        Ok(self.layout().folders.pop().expect("just added"))
    }
}

fn read(path: &Path) -> Result<RequestFile, ScratchError> {
    let name = path.display().to_string();
    let text = std::fs::read_to_string(path)?;
    yaml::from_str(&text).map_err(|e| ScratchError::Invalid(name, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_list_load_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ScratchStore::open(tmp.path().join("scratch")).unwrap();
        let mut a = RequestFile::new("A");
        a.url = "https://example.com".into();
        store.save(&a).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let b = RequestFile::new("B");
        store.save(&b).unwrap();

        let names: Vec<_> = store.list().unwrap().into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["B", "A"]);
        assert_eq!(store.load(a.id).unwrap(), a);

        store.delete(a.id).unwrap();
        store.delete(a.id).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
    }

    fn folder(name: &str, parent: Option<ItemId>) -> ScratchFolder {
        ScratchFolder {
            id: ItemId::new(),
            name: name.into(),
            parent,
        }
    }

    #[test]
    fn layout_round_trips_and_delete_prunes_placement() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ScratchStore::open(tmp.path()).unwrap();
        assert_eq!(store.layout(), ScratchLayout::default());

        let r = RequestFile::new("A");
        store.save(&r).unwrap();
        let outer = folder("Outer", None);
        let inner = folder("Inner", Some(outer.id));
        let layout = ScratchLayout {
            placement: BTreeMap::from([(r.id, inner.id)]),
            folders: vec![outer, inner],
        };
        store.save_layout(&layout).unwrap();
        assert_eq!(store.layout(), layout);
        // The layout file isn't mistaken for a request.
        assert_eq!(store.list().unwrap().len(), 1);

        store.delete(r.id).unwrap();
        assert!(store.layout().placement.is_empty());
        assert_eq!(store.layout().folders.len(), 2);

        // Saving drops placements of requests that are gone.
        store.save_layout(&layout).unwrap();
        assert!(store.layout().placement.is_empty());
    }

    #[test]
    fn create_folder_appends_and_normalizes() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ScratchStore::open(tmp.path()).unwrap();
        let a = store.create_folder("A", None).unwrap();
        let b = store.create_folder("B", Some(a.id)).unwrap();
        let c = store.create_folder(" ", Some(ItemId::new())).unwrap();
        assert_eq!(b.parent, Some(a.id));
        assert_eq!((c.name.as_str(), c.parent), ("New folder", None));
        assert_eq!(store.layout().folders, [a, b, c]);
    }

    #[test]
    fn layout_normalizes_bad_parents_cycles_and_placements() {
        let mut a = folder("  A  ", None);
        let mut b = folder("", Some(a.id));
        a.parent = Some(b.id);
        let orphan = folder("Orphan", Some(ItemId::new()));
        let dup = ScratchFolder {
            name: "Dup".into(),
            ..orphan.clone()
        };
        let kept = ItemId::new();
        let dropped = ItemId::new();
        let mut layout = ScratchLayout {
            folders: vec![a.clone(), b.clone(), orphan.clone(), dup],
            placement: BTreeMap::from([(kept, b.id), (dropped, ItemId::new())]),
        };
        layout.normalize();

        a.parent = None;
        a.name = "A".into();
        b.name = "New folder".into();
        let orphan = ScratchFolder {
            parent: None,
            ..orphan
        };
        assert_eq!(layout.folders, [a, b.clone(), orphan]);
        assert_eq!(layout.placement, BTreeMap::from([(kept, b.id)]));
    }

    #[test]
    fn corrupt_layout_is_flat() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ScratchStore::open(tmp.path()).unwrap();
        std::fs::write(tmp.path().join(LAYOUT_FILE), "{ nope").unwrap();
        assert_eq!(store.layout(), ScratchLayout::default());
    }

    #[test]
    fn unreadable_files_are_skipped_not_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ScratchStore::open(tmp.path()).unwrap();
        let bad = tmp.path().join("bad.yaml");
        std::fs::write(&bad, "<<<<<<< HEAD\n").unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(bad.exists());
    }
}
