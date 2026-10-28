//! App-wide settings in `<app data>/settings.json` (PLAN.md Phase 4 network settings;
//! more in Phase 8). Unknown keys are kept, so newer versions' settings survive.

use std::path::{Path, PathBuf};

use ironcall_core::model::HttpVersion;
use serde::{Deserialize, Serialize};

use crate::atomic;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProxyMode {
    None,
    /// Use `HTTP(S)_PROXY` / `NO_PROXY` from the environment.
    #[default]
    Environment,
    Manual,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySettings {
    #[serde(default)]
    pub mode: ProxyMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The password lives in the OS keychain, never here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub no_proxy: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSettings {
    #[serde(default)]
    pub proxy: ProxySettings,
    /// CA certificates trusted for every workspace (absolute paths).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ca_certs: Vec<String>,
}

/// History retention (PLAN.md §12: 1MB per body, 500MB total, 90 days).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HistorySettingsStore {
    pub max_entries: u32,
    pub max_age_days: u32,
    pub max_total_mb: u32,
    pub max_body_kb: u32,
}

impl Default for HistorySettingsStore {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_age_days: 90,
            max_total_mb: 500,
            max_body_kb: 1024,
        }
    }
}

/// Defaults for request settings a request doesn't set itself (PLAN.md Phase 8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RequestDefaults {
    /// 0 means no timeout.
    pub timeout_ms: u64,
    pub follow_redirects: bool,
    pub max_redirects: u32,
    pub http_version: HttpVersion,
    /// Always open a new connection, so timings are measured cold (PLAN.md §3.4).
    pub fresh_connection: bool,
}

impl Default for RequestDefaults {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            follow_redirects: true,
            max_redirects: 10,
            http_version: HttpVersion::Auto,
            fresh_connection: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// Appearance, per machine (PLAN.md Phase 8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiSettings {
    pub theme: Theme,
    /// Editor and table font size, in px.
    pub font_size: u32,
    /// Whole-window zoom, in percent.
    pub zoom: u32,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            font_size: 13,
            zoom: 100,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default)]
    pub network: NetworkSettings,
    #[serde(default)]
    pub history: HistorySettingsStore,
    #[serde(default)]
    pub requests: RequestDefaults,
    #[serde(default)]
    pub ui: UiSettings,
    /// Headers treated as sensitive in addition to the built-in list (PLAN.md §6.2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sensitive_headers: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Missing or unreadable files give defaults (and a warning in the log).
    pub fn load(&self) -> AppSettings {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return AppSettings::default();
        };
        serde_json::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!(path = %self.path.display(), error = %e, "ignoring unreadable settings");
            AppSettings::default()
        })
    }

    pub fn save(&self, settings: &AppSettings) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut text = serde_json::to_string_pretty(settings).map_err(std::io::Error::other)?;
        text.push('\n');
        atomic::write_atomic(&self.path, text.as_bytes())
    }
}

/// Cookie jars persisted as JSON, one file per workspace (PLAN.md Phase 4), in the app
/// data dir rather than the workspace so they're never committed. Files are user-only.
#[derive(Debug)]
pub struct CookieFiles {
    dir: PathBuf,
}

/// Jar key for scratch requests sent with no workspace open.
pub const NO_WORKSPACE: &str = "no-workspace";

impl CookieFiles {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, key: &str) -> std::io::Result<PathBuf> {
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid cookie jar key",
            ));
        }
        Ok(self.dir.join(format!("{key}.json")))
    }

    /// The saved jar JSON for `key` (a workspace ID or [`NO_WORKSPACE`]), if any.
    pub fn load(&self, key: &str) -> std::io::Result<Option<String>> {
        match std::fs::read_to_string(self.path(key)?) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, key: &str, json: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        atomic::write_private_atomic(&self.path(key)?, json.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_keep_unknown_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SettingsStore::open(tmp.path().join("settings.json"));
        assert_eq!(store.load(), AppSettings::default());
        assert_eq!(store.load().network.proxy.mode, ProxyMode::Environment);

        std::fs::write(
            store.path(),
            r#"{"network": {"proxy": {"mode": "manual", "url": "http://proxy:3128"}}, "future": 1}"#,
        )
        .unwrap();
        let mut settings = store.load();
        assert_eq!(settings.network.proxy.mode, ProxyMode::Manual);
        settings.network.ca_certs.push("/etc/corp-ca.pem".into());
        store.save(&settings).unwrap();

        let reloaded = store.load();
        assert_eq!(reloaded.network.ca_certs, ["/etc/corp-ca.pem"]);
        assert_eq!(reloaded.history, HistorySettingsStore::default());
        assert_eq!(reloaded.extra.get("future"), Some(&serde_json::json!(1)));
    }

    #[test]
    fn partial_sections_fill_in_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SettingsStore::open(tmp.path().join("settings.json"));
        std::fs::write(
            store.path(),
            r#"{"requests": {"timeoutMs": 5000}, "ui": {"theme": "dark"}, "sensitiveHeaders": ["X-Session"]}"#,
        )
        .unwrap();
        let s = store.load();
        assert_eq!(s.requests.timeout_ms, 5000);
        assert!(s.requests.follow_redirects, "missing keys use defaults");
        assert_eq!(s.requests.max_redirects, 10);
        assert_eq!(s.ui.theme, Theme::Dark);
        assert_eq!(s.ui.zoom, 100);
        assert_eq!(s.sensitive_headers, ["X-Session"]);
    }

    #[test]
    fn corrupt_settings_give_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SettingsStore::open(tmp.path().join("settings.json"));
        std::fs::write(store.path(), "{not json").unwrap();
        assert_eq!(store.load(), AppSettings::default());
    }

    #[test]
    fn cookie_files_round_trip_privately() {
        let tmp = tempfile::tempdir().unwrap();
        let files = CookieFiles::new(tmp.path().join("cookies"));
        assert_eq!(files.load("01J9Z3M0A1B2C3D4E5F6G7H8J9").unwrap(), None);
        files.save("01J9Z3M0A1B2C3D4E5F6G7H8J9", "[]").unwrap();
        assert_eq!(
            files.load("01J9Z3M0A1B2C3D4E5F6G7H8J9").unwrap().as_deref(),
            Some("[]")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = tmp.path().join("cookies/01J9Z3M0A1B2C3D4E5F6G7H8J9.json");
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn cookie_files_reject_unsafe_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let files = CookieFiles::new(tmp.path());
        for bad in ["", "../x", "a/b", "a.b"] {
            assert!(files.save(bad, "[]").is_err(), "{bad}");
        }
    }
}
