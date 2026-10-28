//! Session restore and the recent-workspaces list (PLAN.md Phase 2).
//!
//! Both are small JSON files in the app data dir, written atomically. A missing or
//! corrupt file is treated as empty, so a bad file never stops the app from starting.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ironcall_core::ItemId;
use ironcall_core::recent::rfc3339_utc;
use serde::{Deserialize, Serialize};

use crate::atomic;
use crate::workspace::COLLECTION_FILE;

/// How many recent workspaces are kept.
pub const RECENT_CAP: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TabRef {
    Request { id: ItemId },
    Scratch { id: ItemId },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub workspace: Option<PathBuf>,
    pub tabs: Vec<TabRef>,
    pub active: Option<u32>,
}

/// Reads the session file; missing or invalid files give an empty session.
pub fn load_session(path: &Path) -> Session {
    read_json(path).unwrap_or_default()
}

pub fn save_session(path: &Path, session: &Session) -> std::io::Result<()> {
    write_json(path, session)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentWorkspace {
    pub path: PathBuf,
    pub name: String,
    /// RFC 3339 timestamp.
    pub last_opened: String,
    /// Computed when listed: the folder still exists and has an `ironcall.yaml`.
    #[serde(skip)]
    pub available: bool,
}

/// The recent-workspaces list, stored in one JSON file.
#[derive(Clone, Debug)]
pub struct RecentWorkspaces {
    file: PathBuf,
}

impl RecentWorkspaces {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self { file: file.into() }
    }

    /// Most recently opened first, with `available` filled in.
    pub fn list(&self) -> Vec<RecentWorkspace> {
        let mut list: Vec<RecentWorkspace> = read_json(&self.file).unwrap_or_default();
        for w in &mut list {
            w.available = w.path.join(COLLECTION_FILE).is_file();
        }
        list
    }

    /// Moves (or adds) a workspace to the top of the list.
    pub fn touch(&self, path: &Path, name: &str) -> std::io::Result<()> {
        let mut list: Vec<RecentWorkspace> = read_json(&self.file).unwrap_or_default();
        list.retain(|w| w.path != path);
        list.insert(
            0,
            RecentWorkspace {
                path: path.to_path_buf(),
                name: name.to_string(),
                last_opened: rfc3339_utc(SystemTime::now()),
                available: true,
            },
        );
        list.truncate(RECENT_CAP);
        write_json(&self.file, &list)
    }

    pub fn forget(&self, path: &Path) -> std::io::Result<()> {
        let mut list: Vec<RecentWorkspace> = read_json(&self.file).unwrap_or_default();
        list.retain(|w| w.path != path);
        write_json(&self.file, &list)
    }
}

pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "ignoring unreadable file");
            None
        }
    }
}

pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    text.push('\n');
    atomic::write_atomic(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trips_and_tolerates_corruption() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session.json");
        assert_eq!(load_session(&path), Session::default());

        let session = Session {
            workspace: Some(PathBuf::from("/ws")),
            tabs: vec![
                TabRef::Request { id: ItemId::new() },
                TabRef::Scratch { id: ItemId::new() },
            ],
            active: Some(1),
        };
        save_session(&path, &session).unwrap();
        assert_eq!(load_session(&path), session);

        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_session(&path), Session::default());
    }

    #[test]
    fn recent_list_orders_caps_and_reports_availability() {
        let tmp = tempfile::tempdir().unwrap();
        let recent = RecentWorkspaces::new(tmp.path().join("recent.json"));
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join(COLLECTION_FILE), "").unwrap();

        for i in 0..12 {
            recent
                .touch(&tmp.path().join(format!("ws{i}")), &format!("WS {i}"))
                .unwrap();
        }
        recent.touch(&real, "Real").unwrap();
        let list = recent.list();
        assert_eq!(list.len(), RECENT_CAP);
        assert_eq!(list[0].name, "Real");
        assert!(list[0].available);
        assert!(!list[1].available);

        recent.touch(&tmp.path().join("ws11"), "WS 11").unwrap();
        assert_eq!(recent.list()[0].name, "WS 11");
        assert_eq!(recent.list()[1].name, "Real");

        recent.forget(&real).unwrap();
        assert!(recent.list().iter().all(|w| w.path != real));
    }
}
