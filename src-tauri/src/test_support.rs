//! Shared test fixture: a full `AppState` with a workspace open, on a mock Tauri app.

use std::sync::Arc;

use ironcall_http::{Engine, EngineConfig};
use ironcall_store::PathGuard;
use ironcall_store::body::BodyStore;
use ironcall_store::spool::Spool;
use ironcall_store::workspace::Workspace;

use crate::AppState;
use crate::workspace::{WorkspaceChanged, Workspaces};

pub struct Fixture {
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    pub app: tauri::App<tauri::test::MockRuntime>,
    pub state: AppState,
    pub root: std::path::PathBuf,
    pub data: std::path::PathBuf,
}

pub fn fixture() -> Fixture {
    let data = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let app = tauri::test::mock_app();
    tauri_specta::Builder::<tauri::test::MockRuntime>::new()
        .events(tauri_specta::collect_events![
            WorkspaceChanged,
            crate::send::SendProgress,
            crate::run::RunProgress
        ])
        .mount_events(&app);
    let bodies = Arc::new(BodyStore::new());
    let state = AppState {
        path_guard: PathGuard::new(),
        log_dir: data.path().join("logs"),
        bodies: bodies.clone(),
        sender: Arc::new(crate::sender::Sender::new(
            Engine::new(EngineConfig::default()).unwrap(),
            Spool::open(data.path().join("spool"), 5).unwrap(),
            bodies,
        )),
        workspaces: Workspaces::new(data.path()).unwrap(),
        secrets: crate::secrets::Secrets::new(data.path()),
        globals: ironcall_store::globals::GlobalsStore::open(data.path().join("globals.yaml")),
        network: crate::network::Network::new(data.path()),
        searches: crate::body::Searches::default(),
        history: Arc::new(
            ironcall_store::history::History::open(&data.path().join("history.sqlite")).unwrap(),
        ),
        imports: crate::import::Plans::default(),
        runs: crate::run::Runs::new(),
    };
    let ws = Workspace::create(root.path(), "Demo").unwrap();
    state
        .workspaces
        .install(app.handle(), &state.path_guard, ws)
        .unwrap();
    let root_path = root.path().to_path_buf();
    let data_path = data.path().to_path_buf();
    Fixture {
        _dirs: (data, root),
        app,
        state,
        root: root_path,
        data: data_path,
    }
}
