//! IronCall Tauri shell: wires commands, state and logging together.
//!
//! Keep logic in the `ironcall-*` crates; this crate should stay thin.

mod body;
mod commands;
mod diagnostics;
mod history;
mod import;
mod network;
mod protocol;
mod run;
mod secrets;
mod send;
mod sender;
mod settings;
mod vars;
mod workspace;

#[cfg(test)]
mod test_support;

use std::path::PathBuf;
use std::sync::Arc;

use ironcall_http::{Engine, EngineConfig};
use ironcall_store::PathGuard;
use ironcall_store::body::BodyStore;
use ironcall_store::spool::{self, Spool};
use tauri::Manager;
use tauri_specta::{Builder, collect_commands};

/// Shared application state, managed by Tauri.
pub struct AppState {
    pub path_guard: PathGuard,
    pub log_dir: PathBuf,
    pub bodies: Arc<BodyStore>,
    pub sender: Arc<sender::Sender>,
    pub workspaces: workspace::Workspaces,
    pub secrets: Arc<secrets::Secrets>,
    pub globals: ironcall_store::globals::GlobalsStore,
    pub network: network::Network,
    pub searches: body::Searches,
    pub runs: run::Runs,
    pub history: Arc<ironcall_store::history::History>,
    pub imports: import::Plans,
}

/// Registers every IPC command. Also used by the `export_bindings` test to generate
/// the TypeScript bindings in `ui/src/ipc/bindings.ts`.
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::app_info,
            body::open_body,
            body::get_body_lines,
            body::search_body,
            body::close_body,
            send::new_scratch_request,
            send::preview_request,
            send::send_request,
            send::cancel_send,
            send::get_body_text,
            send::recent_sends,
            send::body_pretty,
            send::body_save_as,
            history::history_list,
            history::history_get,
            history::history_open_body,
            history::history_delete,
            history::history_clear,
            history::history_settings_get,
            history::history_settings_set,
            history::workspace_history_bodies_get,
            history::workspace_history_bodies_set,
            import::curl_parse,
            import::curl_detect,
            import::curl_export,
            import::openapi_pick_and_preview,
            import::openapi_preview_text,
            import::openapi_import,
            workspace::workspace_pick_folder,
            workspace::workspace_create,
            workspace::workspace_open,
            workspace::workspace_create_sample,
            workspace::workspace_close,
            workspace::workspace_current,
            workspace::recent_workspaces,
            workspace::forget_recent_workspace,
            workspace::item_create,
            workspace::item_rename,
            workspace::item_rename_file_to_match,
            workspace::item_duplicate,
            workspace::item_delete,
            workspace::item_move,
            workspace::item_reveal,
            workspace::item_open_external,
            workspace::request_load,
            workspace::request_save,
            workspace::folder_load,
            workspace::folder_save,
            workspace::collection_load,
            workspace::collection_save,
            workspace::scratch_list,
            workspace::scratch_save,
            workspace::scratch_delete,
            workspace::scratch_layout,
            workspace::scratch_save_layout,
            workspace::scratch_folder_create,
            workspace::scratch_save_to_workspace,
            workspace::session_load,
            workspace::session_save,
            vars::variables_for,
            vars::environments_list,
            vars::environment_create,
            vars::environment_duplicate,
            vars::environment_rename,
            vars::environment_delete,
            vars::environment_load,
            vars::environment_save,
            vars::active_environment_get,
            vars::active_environment_set,
            vars::globals_load,
            vars::globals_save,
            vars::secret_backend_status,
            vars::secret_backend_set,
            vars::secrets_unlock,
            vars::secret_set,
            vars::secret_clear,
            vars::secret_reveal,
            vars::secrets_missing,
            network::auth_effective,
            network::pick_body_file,
            network::body_files_check,
            network::network_settings_get,
            network::network_settings_set,
            network::proxy_password_set,
            network::pick_ca_cert,
            network::workspace_ca_certs_get,
            network::workspace_ca_certs_set,
            network::cookies_list,
            network::cookie_set,
            network::cookie_delete,
            network::cookies_clear,
            diagnostics::diagnostic_info,
            diagnostics::open_logs_folder,
            diagnostics::log_frontend_error,
            settings::settings_get,
            settings::settings_set,
            run::run_items,
            run::run_collection,
            run::run_cancel,
            run::run_report_markdown,
            run::runtime_vars,
            run::runtime_vars_clear,
            vars::runtime_var_save,
        ])
        .events(tauri_specta::collect_events![
            send::SendProgress,
            workspace::WorkspaceChanged,
            run::RunProgress
        ])
}

/// Absolute at compile time, so a debug build launched from any directory still finds it.
#[cfg(any(debug_assertions, test))]
const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/src/ipc/bindings.ts");

#[cfg(any(debug_assertions, test))]
fn export_bindings(builder: &Builder<tauri::Wry>) -> Result<(), String> {
    builder
        .export(
            specta_typescript::Typescript::default()
                .header("// @ts-nocheck\n// Generated by tauri-specta. Do not edit; run `just bindings`."),
            BINDINGS_PATH,
        )
        .map_err(|e| e.to_string())
}

/// URLs the main window may navigate to: the app itself (`tauri://localhost`, or
/// `http://tauri.localhost` on Windows; the Vite dev server in debug builds), its custom
/// schemes, and inert `about:`/`blob:`/`data:` documents.
fn is_app_url(url: &tauri::Url) -> bool {
    let host = url.host_str().unwrap_or("");
    match url.scheme() {
        "tauri" | "ironcall" | "ironcall-preview" | "about" | "blob" | "data" => true,
        "http" | "https" => {
            matches!(
                host,
                "tauri.localhost" | "ironcall.localhost" | "ironcall-preview.localhost"
            ) || (cfg!(debug_assertions) && host == "localhost" && url.port() == Some(1420))
        }
        _ => false,
    }
}

/// `IRONCALL_DATA_DIR` moves the app data, the logs and the webview's data into one
/// directory. The E2E tests use it so they never touch the user's data (on Windows,
/// `XDG_DATA_HOME` has no effect).
fn data_dir_override() -> Option<PathBuf> {
    std::env::var_os("IRONCALL_DATA_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// WebKitGTK's DMA-BUF renderer uses about 25MB more memory when idle and scrolls less
/// smoothly than the older path ([ADR 0007](../../docs/adr/0007-webkit-dmabuf-renderer-off.md)).
/// The variable must be set before WebKit starts, and setting it in-process needs
/// `unsafe`, so the app restarts itself once with it set. A value the user set (even
/// `0`) is kept.
#[cfg(target_os = "linux")]
fn disable_dmabuf_renderer() {
    use std::os::unix::process::CommandExt;
    const VAR: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
    if std::env::var_os(VAR).is_some() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    // `exec` only returns on failure; then carry on with WebKit's default renderer.
    let err = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env(VAR, "1")
        .exec();
    eprintln!("warning: couldn't restart with {VAR}=1: {err}");
}

pub fn run() {
    #[cfg(target_os = "linux")]
    disable_dmabuf_renderer();

    let builder = specta_builder();

    // Keeps bindings fresh during `cargo tauri dev`. Not fatal: a debug binary may run
    // where the source tree isn't writable (e.g. under the E2E driver).
    #[cfg(debug_assertions)]
    if let Err(e) = export_bindings(&builder) {
        eprintln!("warning: could not export TypeScript bindings: {e}");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(builder.invoke_handler())
        .register_asynchronous_uri_scheme_protocol("ironcall", protocol::handle)
        .register_asynchronous_uri_scheme_protocol("ironcall-preview", protocol::handle_preview)
        .setup(move |app| {
            builder.mount_events(app);

            let data_override = data_dir_override();
            let log_dir = match &data_override {
                Some(dir) => dir.join("logs"),
                None => app.path().app_log_dir()?,
            };
            let guard = ironcall_core::logging::init(&log_dir)?;
            ironcall_core::logging::install_panic_hook();

            // The built-in echo server behind `{{$localEcho}}` (the sample workspace).
            match tauri::async_runtime::block_on(ironcall_http::echo::start()) {
                Ok(echo) => ironcall_core::vars::set_local_echo(echo.base_url()),
                Err(e) => tracing::warn!(error = %e, "couldn't start the local echo server"),
            }
            // The guard flushes buffered log lines on drop; keep it for the app's lifetime.
            app.manage(guard);

            let path_guard = PathGuard::new();
            let data_dir = match &data_override {
                Some(dir) => dir.clone(),
                None => app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&data_dir)?;
            // History, spool, cookies and settings are private (PLAN.md §6.3). On Windows
            // this runs `icacls`, so keep it off the startup path.
            {
                let dirs = [data_dir.clone(), log_dir.clone()];
                std::thread::spawn(move || {
                    for dir in dirs {
                        if let Err(e) = ironcall_core::perms::restrict_to_user(&dir) {
                            tracing::warn!(dir = %dir.display(), error = %e, "couldn't make folder user-only");
                        }
                    }
                });
            }
            let spool_dir = data_dir.join("spool");
            let spool = Spool::open(spool_dir.clone(), spool::DEFAULT_RETAIN)?;
            // Only the spool, where response, scratch and history bodies are kept. The rest
            // of the app data folder (history, cookies, settings) is never readable through
            // file-taking commands.
            path_guard.allow_dir(&spool_dir)?;
            let engine = Engine::new(EngineConfig::default())?;
            let bodies = Arc::new(BodyStore::new());
            let sender = Arc::new(sender::Sender::new(engine, spool, bodies.clone()));

            let workspaces = workspace::Workspaces::new(&data_dir)?;
            let secrets = secrets::Secrets::new(&data_dir);
            secrets.start_probe();
            let globals = ironcall_store::globals::GlobalsStore::open(data_dir.join("globals.yaml"));

            app.manage(AppState {
                path_guard,
                log_dir,
                bodies,
                sender,
                workspaces,
                secrets,
                globals,
                network: network::Network::new(&data_dir),
                searches: body::Searches::default(),
                history: Arc::new(ironcall_store::history::History::open(
                    &data_dir.join("history.sqlite"),
                )?),
                imports: import::Plans::default(),
                runs: run::Runs::new(),
            });

            // Created here rather than from config so the webview's own data (localStorage)
            // follows the override too.
            let mut window =
                tauri::WebviewWindowBuilder::from_config(app.handle(), &app.config().app.windows[0])?;
            if let Some(dir) = &data_override {
                window = window.data_directory(dir.join("webview"));
            }
            // The main window only ever shows the app. Links open in the system browser
            // (opener plugin), so any other navigation (a form in a description, a
            // redirecting preview) is refused.
            window = window.on_navigation(is_app_url);
            window.build()?;
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "IronCall started");
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building IronCall")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event
                && let Some(state) = app.try_state::<AppState>()
            {
                // Response bodies can hold sensitive data; never leave them on disk.
                state.sender.clear_spool();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_main_window_only_navigates_within_the_app() {
        let ok = |u: &str| is_app_url(&tauri::Url::parse(u).unwrap());
        for allowed in [
            "tauri://localhost/index.html",
            "http://tauri.localhost/",
            "ironcall://localhost/body/1",
            "http://ironcall-preview.localhost/x",
            "about:blank",
        ] {
            assert!(ok(allowed), "{allowed}");
        }
        for refused in [
            "https://evil.example/login",
            "http://localhost:8080/",
            "file:///etc/passwd",
            "http://tauri.localhost.evil.example/",
        ] {
            assert!(!ok(refused), "{refused}");
        }
    }

    /// The app version lives in three files that must agree (docs/building.md).
    #[test]
    fn versions_match() {
        let read = |rel: &str| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
            let text = std::fs::read_to_string(&path).unwrap();
            serde_json::from_str::<serde_json::Value>(&text).unwrap()["version"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let cargo = env!("CARGO_PKG_VERSION");
        assert_eq!(read("tauri.conf.json"), cargo, "src-tauri/tauri.conf.json");
        assert_eq!(read("../ui/package.json"), cargo, "ui/package.json");
    }

    /// Regenerates `ui/src/ipc/bindings.ts`. Run with `just bindings`.
    #[test]
    fn export_bindings() {
        super::export_bindings(&specta_builder()).expect("failed to export TypeScript bindings");
    }
}
