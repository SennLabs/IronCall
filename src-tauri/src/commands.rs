//! IPC commands exposed to the frontend. Each one must also be listed in
//! `specta_builder()` in `lib.rs`.

use serde::Serialize;
use specta::Type;

use crate::AppState;

#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub os: String,
    pub log_dir: String,
}

/// Basic app information. Also serves as the Phase 0 IPC round-trip check.
#[tauri::command]
#[specta::specta]
pub fn app_info(state: tauri::State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        log_dir: state.log_dir.display().to_string(),
    }
}
