//! Crash and error handling helpers (PLAN.md Phase 8): "Copy diagnostic info", "Open
//! logs folder", and logging errors from the frontend. Everything here is scrubbed of
//! secrets, and the home folder is shown as `~`, because users paste it into issues.

use std::path::Path;

use ironcall_core::redact;

use crate::AppState;

/// How many of the newest log lines the diagnostic info includes.
const LOG_TAIL: usize = 60;

/// `path` with the home folder replaced by `~` (whole path components only).
fn tilde(path: &Path) -> String {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    match home
        .as_deref()
        .map(Path::new)
        .and_then(|h| path.strip_prefix(h).ok())
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => path.display().to_string(),
    }
}

/// The last `n` lines of the newest log file, scrubbed.
fn log_tail(dir: &Path, n: usize) -> String {
    let newest = std::fs::read_dir(dir).ok().and_then(|entries| {
        entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "log"))
            .max()
    });
    // The end of the file is enough, and logs can hold invalid UTF-8.
    let Some(text) = newest.and_then(|p| tail_bytes(&p, 64 * 1024).ok()) else {
        return "(no log file yet)".into();
    };
    let lines: Vec<&str> = text.lines().collect();
    let tail = &lines[lines.len().saturating_sub(n)..];
    redact::scrub(&tail.join("\n"))
}

/// The last `max` bytes of a file, decoded lossily.
fn tail_bytes(path: &Path, max: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(max)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(crate) fn info(state: &AppState) -> String {
    let settings = state.network.settings.load();
    let general = state.network.general_settings();
    let mut out = String::new();
    let mut line = |k: &str, v: String| out.push_str(&format!("{k}: {v}\n"));
    line("IronCall", env!("CARGO_PKG_VERSION").to_string());
    line(
        "OS",
        format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
    );
    line(
        "Webview",
        tauri::webview_version().unwrap_or_else(|e| format!("unknown ({e})")),
    );
    line("Logs", tilde(&state.log_dir));
    match state.workspaces.current() {
        Ok(ws) => {
            let ws = ws.lock().expect("workspace lock poisoned");
            let backend = state.secrets.backend_for(ws.root());
            line(
                "Workspace",
                format!(
                    "{} requests, {} environments, secret backend {}",
                    ws.tree().request_count(),
                    ws.environments().len(),
                    crate::secrets::backend_setting(backend),
                ),
            );
        }
        Err(_) => line("Workspace", "none open".into()),
    }
    line(
        "Keychain available",
        state.secrets.keychain_available().to_string(),
    );
    line(
        "Proxy",
        format!(
            "{:?}, {} extra CA certificate(s)",
            settings.network.proxy.mode,
            settings.network.ca_certs.len()
        ),
    );
    let r = &general.requests;
    line(
        "Request defaults",
        format!(
            "timeout {} ms, redirects {} (max {}), {:?}, fresh connections {}",
            r.timeout_ms, r.follow_redirects, r.max_redirects, r.http_version, r.fresh_connection
        ),
    );
    line(
        "History",
        format!(
            "max {} entries, {} days, {} MB",
            settings.history.max_entries, settings.history.max_age_days, settings.history.max_total_mb
        ),
    );
    line(
        "Extra sensitive headers",
        general.sensitive_headers.len().to_string(),
    );
    out.push_str(&format!(
        "\nRecent log lines:\n{}\n",
        log_tail(&state.log_dir, LOG_TAIL)
    ));
    out
}

/// Version, platform, settings summary and recent log lines, for bug reports. Contains no
/// secrets, URLs' credentials or sensitive values, and no home folder path.
#[tauri::command]
#[specta::specta]
pub fn diagnostic_info(state: tauri::State<'_, AppState>) -> String {
    info(&state)
}

/// Opens the logs folder in the file manager.
#[tauri::command]
#[specta::specta]
pub fn open_logs_folder(state: tauri::State<'_, AppState>) -> Result<(), String> {
    std::fs::create_dir_all(&state.log_dir).map_err(|e| e.to_string())?;
    tauri_plugin_opener::open_path(state.log_dir.display().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Records an uncaught frontend error in the log (scrubbed).
#[tauri::command]
#[specta::specta]
pub fn log_frontend_error(message: String, stack: Option<String>) {
    let message = redact::scrub(&message);
    let stack = stack.map(|s| redact::scrub(&s)).unwrap_or_default();
    tracing::error!(%stack, "frontend error: {message}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fixture;

    #[test]
    fn diagnostic_info_summarises_without_secrets() {
        let f = fixture();
        std::fs::create_dir_all(&f.state.log_dir).unwrap();
        std::fs::write(
            f.state.log_dir.join("ironcall.2026-10-06.log"),
            "INFO started\nERROR send failed url=https://bob:pw999@api.test/?token=tk-1\n",
        )
        .unwrap();
        let text = info(&f.state);
        assert!(
            text.starts_with(&format!("IronCall: {}", env!("CARGO_PKG_VERSION"))),
            "{text}"
        );
        assert!(text.contains("Workspace: 0 requests"), "{text}");
        assert!(text.contains("Request defaults: timeout 30000 ms"), "{text}");
        assert!(text.contains("ERROR send failed"), "{text}");
        assert!(!text.contains("pw999") && !text.contains("tk-1"), "{text}");
    }

    #[test]
    fn home_is_shown_as_tilde() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/u".into());
        assert_eq!(
            tilde(&Path::new(&home).join(".local/share/x")),
            "~/.local/share/x"
        );
        assert_eq!(tilde(Path::new("/opt/x")), "/opt/x");
        let sibling = format!("{home}2/x");
        assert_eq!(tilde(Path::new(&sibling)), sibling, "only whole components");
    }
}
