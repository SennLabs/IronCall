//! Per-machine workspace settings in `.ironcall/local.yaml` (PLAN.md §4): the active
//! environment, CA certificates and history settings. Never committed: `.ironcall/` has its own
//! `.gitignore` containing `*`.

use ironcall_core::ItemId;
use ironcall_core::model::Extra;
use serde::{Deserialize, Serialize};

use super::{LOCAL_DIR, Result, Workspace, parse, to_yaml};
use crate::atomic;

pub const LOCAL_FILE: &str = "local.yaml";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_environment: Option<ItemId>,
    /// `keychain`, `encryptedFile` or `plaintextFile`. Written before 1.0; no longer used
    /// to pick the backend, since this file can be committed. The choice is kept per
    /// machine in the app data folder instead, and this is only read to warn about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_backend: Option<String>,
    /// CA certificates trusted for this workspace only (absolute paths; machine-specific).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ca_certs: Vec<String>,
    /// Whether response bodies from this workspace are stored in history; `None` means
    /// the default (yes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_bodies: Option<bool>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl LocalSettings {
    /// Whether to store response bodies in history (default true).
    pub fn stores_history_bodies(&self) -> bool {
        self.history_bodies.unwrap_or(true)
    }
}

impl Workspace {
    /// Reads `.ironcall/local.yaml`. A missing or unreadable file gives defaults, and an
    /// active environment that no longer exists reads as `None`.
    pub fn load_local(&self) -> LocalSettings {
        let path = self.root.join(LOCAL_DIR).join(LOCAL_FILE);
        let mut settings = match std::fs::read(&path) {
            Ok(bytes) => parse::<LocalSettings>(&bytes).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), error = %e, "ignoring unreadable local settings");
                LocalSettings::default()
            }),
            Err(_) => LocalSettings::default(),
        };
        if let Some(id) = settings.active_environment
            && !self
                .environments
                .iter()
                .any(|e| e.summary.id == id && e.summary.error.is_none())
        {
            settings.active_environment = None;
        }
        settings
    }

    /// Writes `.ironcall/local.yaml`, creating `.ironcall/` and its `.gitignore` if an
    /// older workspace lacks them.
    pub fn save_local(&self, settings: &LocalSettings) -> Result<()> {
        let dir = self.root.join(LOCAL_DIR);
        std::fs::create_dir_all(&dir)?;
        let gitignore = dir.join(".gitignore");
        if !gitignore.exists() {
            atomic::write_own(&self.own, &gitignore, b"*\n")?;
        }
        atomic::write_own(&self.own, &dir.join(LOCAL_FILE), to_yaml(settings)?.as_bytes())?;
        Ok(())
    }
}
