//! Workspaces, the sidebar tree, saving, scratch requests and session restore
//! (PLAN.md §4, Phase 2): the IPC contract between the UI and `ironcall-store`.
//!
//! Every file the UI loads comes with a content `hash`. Saving sends that hash back as
//! `baseHash`; if the file changed on disk in the meantime, the save is refused with
//! [`SaveOutcome::Conflict`] so the UI can offer keep mine / take theirs / compare.
//! External changes (git pull, editors) arrive as [`WorkspaceChanged`] events.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use ironcall_core::ItemId;
use ironcall_core::model::{CollectionFile, FolderFile, RequestFile};
use ironcall_store::PathGuard;
use ironcall_store::scratch::{self as store_scratch, ScratchStore};
use ironcall_store::session::{self as store_session, RecentWorkspaces};
use ironcall_store::watch::WorkspaceWatcher;
use ironcall_store::workspace::{self as store, Loaded, Node, Workspace};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

use crate::AppState;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NodeKind {
    /// The workspace root (`ironcall.yaml`).
    Collection,
    Folder,
    Request,
    /// A file that couldn't be read (bad YAML, merge-conflict markers, schema error).
    /// It is shown in the tree and never overwritten (PLAN.md §4.4).
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TreeNode {
    /// Stable for requests and saved folders. Folders without a `_folder.yaml` get an
    /// ID that is stable only for this session. Error nodes get a session ID too.
    pub id: ItemId,
    pub kind: NodeKind,
    pub name: String,
    /// HTTP method, for request nodes.
    pub method: Option<String>,
    /// URL without the query string, for request nodes (used by the sidebar filter).
    pub url: Option<String>,
    /// Path relative to the workspace root, with `/` separators.
    pub path: String,
    /// What went wrong, for error nodes (includes the line number when known).
    pub error: Option<String>,
    /// True if the file uses a newer schema than this app understands (read-only).
    pub read_only: bool,
    /// Children in display order (PLAN.md §4.3 ordering rules).
    pub children: Vec<TreeNode>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    /// Workspace ID from `ironcall.yaml`.
    pub id: ItemId,
    pub name: String,
    /// Absolute path of the workspace folder.
    pub root: String,
    pub tree: TreeNode,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RecentWorkspace {
    pub path: String,
    pub name: String,
    /// RFC 3339 timestamp.
    pub last_opened: String,
    /// False if the folder no longer exists or has no `ironcall.yaml`.
    pub available: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoadedRequest {
    pub request: RequestFile,
    /// Content hash of the file as loaded; send it back as `baseHash` when saving.
    pub hash: String,
    pub read_only: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoadedFolder {
    pub folder: FolderFile,
    pub hash: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoadedCollection {
    pub collection: CollectionFile,
    pub hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum SaveOutcome {
    Saved {
        hash: String,
    },
    /// The file changed on disk since `baseHash`. Nothing was written.
    Conflict {
        /// The current file on disk, or `None` if it was deleted.
        disk: Option<Box<LoadedRequest>>,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NewItemKind {
    Folder,
    Request,
}

/// One open tab, for session restore (PLAN.md Phase 2).
#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TabRef {
    /// A request in the open workspace, by ID.
    Request { id: ItemId },
    /// A scratch request (PLAN.md §4.6), by ID.
    Scratch { id: ItemId },
}

/// A folder in the scratch list (PLAN.md §4.6). Folders nest through `parent`.
#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScratchFolder {
    pub id: ItemId,
    pub name: String,
    pub parent: Option<ItemId>,
}

/// How scratch requests are grouped into folders.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScratchLayout {
    /// In display order.
    pub folders: Vec<ScratchFolder>,
    /// Request ID → folder ID. Requests not listed are at the top level.
    pub placement: std::collections::BTreeMap<ItemId, ItemId>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// Workspace to reopen on start, if any.
    pub workspace: Option<String>,
    pub tabs: Vec<TabRef>,
    /// Index into `tabs`.
    pub active: Option<u32>,
}

/// Emitted when files in the open workspace change outside IronCall, or the tree changes.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChanged {
    /// The new tree, if its structure, names, methods or errors changed.
    pub tree: Option<TreeNode>,
    /// Requests whose file content changed on disk (not by IronCall itself).
    pub changed: Vec<ItemId>,
    /// Requests whose file disappeared.
    pub removed: Vec<ItemId>,
    /// True if `ironcall.yaml` or a `_folder.yaml` changed.
    pub meta_changed: bool,
    /// True if an environment file was added, removed or changed.
    pub environments_changed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceError {
    pub message: String,
    /// True if a save was refused because the file changed on disk since `baseHash`.
    pub conflict: bool,
}

impl WorkspaceError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            conflict: false,
        }
    }

    fn conflict() -> Self {
        Self {
            conflict: true,
            ..Self::new(META_CONFLICT)
        }
    }
}

type Result<T> = std::result::Result<T, WorkspaceError>;

impl From<store::WorkspaceError> for WorkspaceError {
    fn from(e: store::WorkspaceError) -> Self {
        Self::new(e.to_string())
    }
}

impl From<ironcall_store::scratch::ScratchError> for WorkspaceError {
    fn from(e: ironcall_store::scratch::ScratchError) -> Self {
        Self::new(e.to_string())
    }
}

impl From<std::io::Error> for WorkspaceError {
    fn from(e: std::io::Error) -> Self {
        Self::new(e.to_string())
    }
}

// ---------------------------------------------------------------------------------
// State

struct OpenWorkspace {
    workspace: Arc<Mutex<Workspace>>,
    _watcher: Option<WorkspaceWatcher>,
}

/// The open workspace plus app-level stores (scratch, session, recent workspaces).
pub struct Workspaces {
    open: Mutex<Option<OpenWorkspace>>,
    scratch: ScratchStore,
    recent: RecentWorkspaces,
    session_file: PathBuf,
    /// Folders picked in the native folder dialog this session (canonical). Workspace
    /// commands only accept these, plus recent workspaces for opening, so script in the
    /// webview can't grant itself an arbitrary folder in the path guard.
    picked: Mutex<Vec<PathBuf>>,
}

impl Workspaces {
    /// `data_dir` is the app data dir; scratch requests, the session and the recent
    /// list live there (PLAN.md §4.6).
    pub(crate) fn new(data_dir: &Path) -> std::io::Result<Self> {
        Ok(Self {
            open: Mutex::new(None),
            scratch: ScratchStore::open(data_dir.join("scratch"))?,
            recent: RecentWorkspaces::new(data_dir.join("recent-workspaces.json")),
            session_file: data_dir.join("session.json"),
            picked: Mutex::new(Vec::new()),
        })
    }

    /// Records a folder the user picked in the native dialog.
    fn add_picked(&self, dir: &Path) -> Result<PathBuf> {
        let dir = canonical(dir)?;
        let mut picked = self.picked.lock().expect("picked lock poisoned");
        if !picked.contains(&dir) {
            picked.push(dir.clone());
        }
        Ok(dir)
    }

    /// Checks that `path` is a folder picked this session or, if `recent_ok`, a recent
    /// workspace, and returns its canonical form.
    fn authorize(&self, path: &Path, recent_ok: bool) -> Result<PathBuf> {
        let refused = || WorkspaceError::new("choose the folder with Open or New workspace first");
        let dir = canonical(path).map_err(|_| refused())?;
        if self.picked.lock().expect("picked lock poisoned").contains(&dir) {
            return Ok(dir);
        }
        if recent_ok
            && self
                .recent
                .list()
                .iter()
                .any(|w| canonical(&w.path).is_ok_and(|p| p == dir))
        {
            return Ok(dir);
        }
        Err(refused())
    }

    pub(crate) fn current(&self) -> Result<Arc<Mutex<Workspace>>> {
        self.open
            .lock()
            .expect("workspace lock poisoned")
            .as_ref()
            .map(|o| o.workspace.clone())
            .ok_or_else(|| WorkspaceError::new("no workspace is open"))
    }

    pub(crate) fn install<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        guard: &PathGuard,
        workspace: Workspace,
    ) -> Result<WorkspaceInfo> {
        self.close(guard);
        guard
            .allow_dir(workspace.root())
            .map_err(|e| WorkspaceError::new(e.to_string()))?;
        let info = info_of(&workspace);
        let _ = self.recent.touch(workspace.root(), workspace.name());

        let root = workspace.root().to_path_buf();
        let own = workspace.own_writes();
        let workspace = Arc::new(Mutex::new(workspace));
        let watched = workspace.clone();
        let app = app.clone();
        let watcher = WorkspaceWatcher::start(&root, own, move |paths| {
            let event = match lock(&watched).rescan(&paths) {
                Ok(event) => event,
                Err(e) => {
                    tracing::warn!(error = %e, "rescan after external change failed");
                    return;
                }
            };
            if event.is_empty() {
                return;
            }
            let tree = event.tree_changed.then(|| tree_node(lock(&watched).tree()));
            let _ = WorkspaceChanged {
                tree,
                changed: event.changed,
                removed: event.removed,
                meta_changed: event.meta_changed,
                environments_changed: event.environments_changed,
            }
            .emit(&app);
        })
        .map_err(
            |e| tracing::warn!(error = %e, "could not watch workspace; external changes won't be detected"),
        )
        .ok();

        *self.open.lock().expect("workspace lock poisoned") = Some(OpenWorkspace {
            workspace,
            _watcher: watcher,
        });
        Ok(info)
    }

    fn close(&self, guard: &PathGuard) {
        if let Some(open) = self.open.lock().expect("workspace lock poisoned").take() {
            guard.revoke_dir(lock(&open.workspace).root());
        }
    }
}

fn canonical(path: &Path) -> Result<PathBuf> {
    ironcall_store::path_guard::dunce_canonicalize(path)
        .map_err(|e| WorkspaceError::new(format!("{}: {e}", path.display())))
}

fn lock(ws: &Mutex<Workspace>) -> MutexGuard<'_, Workspace> {
    ws.lock().expect("workspace lock poisoned")
}

fn tree_node(node: &Node) -> TreeNode {
    TreeNode {
        id: node.id,
        kind: match node.kind {
            store::NodeKind::Collection => NodeKind::Collection,
            store::NodeKind::Folder => NodeKind::Folder,
            store::NodeKind::Request => NodeKind::Request,
            store::NodeKind::Error => NodeKind::Error,
        },
        name: node.name.clone(),
        method: node.method.clone(),
        url: node.url.clone(),
        path: node.path.clone(),
        error: node.error.clone(),
        read_only: node.read_only,
        children: node.children.iter().map(tree_node).collect(),
    }
}

fn info_of(ws: &Workspace) -> WorkspaceInfo {
    WorkspaceInfo {
        id: ws.id(),
        name: ws.name().to_string(),
        root: ws.root().display().to_string(),
        tree: tree_node(ws.tree()),
    }
}

fn loaded_request(l: Loaded<RequestFile>) -> LoadedRequest {
    LoadedRequest {
        request: l.file,
        hash: l.hash.unwrap_or_default(),
        read_only: l.read_only,
    }
}

fn node_by_id(ws: &Workspace, id: ItemId) -> Result<TreeNode> {
    ws.tree()
        .find(id)
        .map(tree_node)
        .ok_or_else(|| WorkspaceError::new("the new item is missing from the tree"))
}

fn path_of(ws: &Workspace, id: ItemId) -> Result<PathBuf> {
    ws.path_of(id)
        .map(Path::to_path_buf)
        .ok_or_else(|| WorkspaceError::new(format!("no item with ID {id} in this workspace")))
}

/// Runs blocking workspace work off the async runtime's worker threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| WorkspaceError::new(e.to_string()))?
}

// ---------------------------------------------------------------------------------
// Workspaces

/// Opens the native folder picker. Only folders picked here (or recent workspaces, for
/// opening) are accepted by the workspace commands below. `None` if cancelled.
#[tauri::command]
#[specta::specta]
pub async fn workspace_pick_folder(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    title: String,
) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_title(title).blocking_pick_folder()
    })
    .await
    .map_err(|e| WorkspaceError::new(e.to_string()))?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else {
        return Ok(None);
    };
    let dir = state.workspaces.add_picked(&path)?;
    Ok(Some(dir.display().to_string()))
}

/// Creates a workspace in an existing (preferably empty) folder: writes `ironcall.yaml`,
/// `.gitattributes` and `.ironcall/.gitignore`, then opens it. The folder must come
/// from [`workspace_pick_folder`].
#[tauri::command]
#[specta::specta]
pub async fn workspace_create(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    path: String,
    name: String,
) -> Result<WorkspaceInfo> {
    let path = state.workspaces.authorize(Path::new(&path), false)?;
    let ws = blocking(move || Ok(Workspace::create(&path, &name)?)).await?;
    state.workspaces.install(&app, &state.path_guard, ws)
}

/// Creates the first-run sample workspace in `path` (PLAN.md Phase 8) and opens it. Its
/// requests call the built-in echo server, so they work offline. The folder must come
/// from [`workspace_pick_folder`].
#[tauri::command]
#[specta::specta]
pub async fn workspace_create_sample(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    path: String,
) -> Result<WorkspaceInfo> {
    let path = state.workspaces.authorize(Path::new(&path), false)?;
    let ws = blocking(move || Ok(ironcall_store::sample::create(&path)?)).await?;
    state.workspaces.install(&app, &state.path_guard, ws)
}

/// Opens a workspace folder (must contain `ironcall.yaml`), closing any open one. The
/// folder must come from [`workspace_pick_folder`] or the recent list.
#[tauri::command]
#[specta::specta]
pub async fn workspace_open(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    path: String,
) -> Result<WorkspaceInfo> {
    let path = state.workspaces.authorize(Path::new(&path), true)?;
    let ws = blocking(move || Ok(Workspace::open(&path)?)).await?;
    state.workspaces.install(&app, &state.path_guard, ws)
}

#[tauri::command]
#[specta::specta]
pub fn workspace_close(state: tauri::State<'_, AppState>) {
    state.workspaces.close(&state.path_guard);
}

#[tauri::command]
#[specta::specta]
pub fn workspace_current(state: tauri::State<'_, AppState>) -> Option<WorkspaceInfo> {
    let ws = state.workspaces.current().ok()?;
    let ws = lock(&ws);
    Some(info_of(&ws))
}

/// Most recently opened first.
#[tauri::command]
#[specta::specta]
pub fn recent_workspaces(state: tauri::State<'_, AppState>) -> Vec<RecentWorkspace> {
    state
        .workspaces
        .recent
        .list()
        .into_iter()
        .map(|r| RecentWorkspace {
            path: r.path.display().to_string(),
            name: r.name,
            last_opened: r.last_opened,
            available: r.available,
        })
        .collect()
}

#[tauri::command]
#[specta::specta]
pub fn forget_recent_workspace(state: tauri::State<'_, AppState>, path: String) {
    let _ = state.workspaces.recent.forget(Path::new(&path));
}

// ---------------------------------------------------------------------------------
// Tree operations (all on the open workspace)

/// Creates a folder or request under `parent` (a folder or the collection) at `index`
/// (end if `None`). Returns the new node.
#[tauri::command]
#[specta::specta]
pub async fn item_create(
    state: tauri::State<'_, AppState>,
    parent: ItemId,
    kind: NewItemKind,
    name: String,
    index: Option<u32>,
) -> Result<TreeNode> {
    let ws = state.workspaces.current()?;
    let mut ws = lock(&ws);
    let index = index.map(|i| i as usize);
    let created = match kind {
        NewItemKind::Folder => ws.create_folder(parent, &name, index)?,
        NewItemKind::Request => ws.create_request(parent, &name, index)?,
    };
    if let Some(warning) = &created.warning {
        tracing::warn!(%warning, "new item path is long");
    }
    Ok(tree_node(&created.node))
}

/// Renames the display name only (`name:` in the file). File names never change on
/// rename (PLAN.md §4.3).
#[tauri::command]
#[specta::specta]
pub async fn item_rename(state: tauri::State<'_, AppState>, id: ItemId, name: String) -> Result<()> {
    let ws = state.workspaces.current()?;
    Ok(lock(&ws).rename(id, &name)?)
}

/// Renames the file (or folder directory) to match the display name.
#[tauri::command]
#[specta::specta]
pub async fn item_rename_file_to_match(state: tauri::State<'_, AppState>, id: ItemId) -> Result<()> {
    let ws = state.workspaces.current()?;
    if let Some(warning) = lock(&ws).rename_file_to_match(id)? {
        tracing::warn!(%warning, "renamed path is long");
    }
    Ok(())
}

/// Duplicates a request or folder (with new IDs) right after the original.
/// Returns the new node.
#[tauri::command]
#[specta::specta]
pub async fn item_duplicate(state: tauri::State<'_, AppState>, id: ItemId) -> Result<TreeNode> {
    let ws = state.workspaces.current()?;
    let created = lock(&ws).duplicate(id)?;
    Ok(tree_node(&created.node))
}

/// Moves a request or folder to the OS trash.
#[tauri::command]
#[specta::specta]
pub async fn item_delete(state: tauri::State<'_, AppState>, id: ItemId) -> Result<()> {
    let ws = state.workspaces.current()?;
    Ok(lock(&ws).delete(id)?)
}

/// Moves an item to `parent` at `index` (also used for reordering within a folder).
#[tauri::command]
#[specta::specta]
pub async fn item_move(
    state: tauri::State<'_, AppState>,
    id: ItemId,
    parent: ItemId,
    index: u32,
) -> Result<()> {
    let ws = state.workspaces.current()?;
    Ok(lock(&ws).move_item(id, parent, index as usize)?)
}

/// Reveals the item's file in the OS file manager.
#[tauri::command]
#[specta::specta]
pub async fn item_reveal(state: tauri::State<'_, AppState>, id: ItemId) -> Result<()> {
    let ws = state.workspaces.current()?;
    let path = path_of(&lock(&ws), id)?;
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| WorkspaceError::new(e.to_string()))
}

/// Opens the item's file in the default external editor (used for error nodes).
#[tauri::command]
#[specta::specta]
pub async fn item_open_external(state: tauri::State<'_, AppState>, id: ItemId) -> Result<()> {
    let ws = state.workspaces.current()?;
    let path = path_of(&lock(&ws), id)?;
    tauri_plugin_opener::open_path(path.display().to_string(), None::<&str>)
        .map_err(|e| WorkspaceError::new(e.to_string()))
}

// ---------------------------------------------------------------------------------
// Loading and saving files

#[tauri::command]
#[specta::specta]
pub async fn request_load(state: tauri::State<'_, AppState>, id: ItemId) -> Result<LoadedRequest> {
    let ws = state.workspaces.current()?;
    let loaded = lock(&ws).load_request(id)?;
    Ok(loaded_request(loaded))
}

/// Saves a request. Refuses with `Conflict` if the file changed since `baseHash`,
/// unless `force` is set.
#[tauri::command]
#[specta::specta]
pub async fn request_save(
    state: tauri::State<'_, AppState>,
    request: RequestFile,
    base_hash: Option<String>,
    force: bool,
) -> Result<SaveOutcome> {
    let ws = state.workspaces.current()?;
    let outcome = lock(&ws).save_request(&request, base_hash.as_deref(), force)?;
    Ok(match outcome {
        store::SaveOutcome::Saved { hash } => SaveOutcome::Saved { hash },
        store::SaveOutcome::Conflict { disk } => SaveOutcome::Conflict {
            disk: disk.map(|d| Box::new(loaded_request(d))),
        },
    })
}

#[tauri::command]
#[specta::specta]
pub async fn folder_load(state: tauri::State<'_, AppState>, id: ItemId) -> Result<LoadedFolder> {
    let ws = state.workspaces.current()?;
    let loaded = lock(&ws).load_folder(id)?;
    Ok(LoadedFolder {
        folder: loaded.file,
        hash: loaded.hash,
    })
}

const META_CONFLICT: &str = "this file changed on disk since it was opened; reload it and try again";

/// Saves a folder's `_folder.yaml` (creating it if needed). Returns the new hash.
#[tauri::command]
#[specta::specta]
pub async fn folder_save(
    state: tauri::State<'_, AppState>,
    folder: FolderFile,
    base_hash: Option<String>,
) -> Result<String> {
    let ws = state.workspaces.current()?;
    match lock(&ws).save_folder(&folder, base_hash.as_deref(), false)? {
        store::SaveOutcome::Saved { hash } => Ok(hash),
        store::SaveOutcome::Conflict { .. } => Err(WorkspaceError::conflict()),
    }
}

#[tauri::command]
#[specta::specta]
pub async fn collection_load(state: tauri::State<'_, AppState>) -> Result<LoadedCollection> {
    let ws = state.workspaces.current()?;
    let loaded = lock(&ws).load_collection()?;
    Ok(LoadedCollection {
        collection: loaded.file,
        hash: loaded.hash.unwrap_or_default(),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn collection_save(
    state: tauri::State<'_, AppState>,
    collection: CollectionFile,
    base_hash: Option<String>,
) -> Result<String> {
    let ws = state.workspaces.current()?;
    match lock(&ws).save_collection(&collection, base_hash.as_deref(), false)? {
        store::SaveOutcome::Saved { hash } => Ok(hash),
        store::SaveOutcome::Conflict { .. } => Err(WorkspaceError::conflict()),
    }
}

// ---------------------------------------------------------------------------------
// Scratch requests (PLAN.md §4.6) and session

/// All scratch requests, most recently modified first.
#[tauri::command]
#[specta::specta]
pub async fn scratch_list(state: tauri::State<'_, AppState>) -> Result<Vec<RequestFile>> {
    Ok(state.workspaces.scratch.list()?)
}

/// Saves a scratch request (creating it if new). Scratch requests autosave.
#[tauri::command]
#[specta::specta]
pub async fn scratch_save(state: tauri::State<'_, AppState>, request: RequestFile) -> Result<()> {
    Ok(state.workspaces.scratch.save(&request)?)
}

#[tauri::command]
#[specta::specta]
pub async fn scratch_delete(state: tauri::State<'_, AppState>, id: ItemId) -> Result<()> {
    Ok(state.workspaces.scratch.delete(id)?)
}

/// How scratch requests are grouped into folders.
#[tauri::command]
#[specta::specta]
pub async fn scratch_layout(state: tauri::State<'_, AppState>) -> Result<ScratchLayout> {
    let l = state.workspaces.scratch.layout();
    Ok(ScratchLayout {
        folders: l
            .folders
            .into_iter()
            .map(|f| ScratchFolder {
                id: f.id,
                name: f.name,
                parent: f.parent,
            })
            .collect(),
        placement: l.placement,
    })
}

/// Adds a scratch folder at the end of the layout under `parent` (`None`: top level).
#[tauri::command]
#[specta::specta]
pub async fn scratch_folder_create(
    state: tauri::State<'_, AppState>,
    name: String,
    parent: Option<ItemId>,
) -> Result<ScratchFolder> {
    let f = state.workspaces.scratch.create_folder(&name, parent)?;
    Ok(ScratchFolder {
        id: f.id,
        name: f.name,
        parent: f.parent,
    })
}

/// Replaces the scratch folder layout. Deleting a folder's requests is separate
/// (`scratch_delete`).
#[tauri::command]
#[specta::specta]
pub async fn scratch_save_layout(state: tauri::State<'_, AppState>, layout: ScratchLayout) -> Result<()> {
    let l = store_scratch::ScratchLayout {
        folders: layout
            .folders
            .into_iter()
            .map(|f| store_scratch::ScratchFolder {
                id: f.id,
                name: f.name,
                parent: f.parent,
            })
            .collect(),
        placement: layout.placement,
    };
    Ok(state.workspaces.scratch.save_layout(&l)?)
}

/// Moves a scratch request into the open workspace under `parent`, returning its node.
/// The scratch copy is deleted.
#[tauri::command]
#[specta::specta]
pub async fn scratch_save_to_workspace(
    state: tauri::State<'_, AppState>,
    request: RequestFile,
    parent: ItemId,
) -> Result<TreeNode> {
    let ws = state.workspaces.current()?;
    let mut ws = lock(&ws);
    // Create the file (slug, order), then save the full request into it. The request
    // takes the new file's ID; the UI re-keys the tab.
    let created = ws.create_request(parent, &request.name, None)?;
    let created_id = created.node.id;
    let base = ws.load_request(created_id)?.hash;
    let mut file = request.clone();
    file.id = created_id;
    ws.save_request(&file, base.as_deref(), true)?;
    let node = node_by_id(&ws, created_id)?;
    drop(ws);
    state.workspaces.scratch.delete(request.id)?;
    Ok(node)
}

#[tauri::command]
#[specta::specta]
pub fn session_load(state: tauri::State<'_, AppState>) -> Session {
    let s = store_session::load_session(&state.workspaces.session_file);
    Session {
        workspace: s.workspace.map(|p| p.display().to_string()),
        tabs: s
            .tabs
            .into_iter()
            .map(|t| match t {
                store_session::TabRef::Request { id } => TabRef::Request { id },
                store_session::TabRef::Scratch { id } => TabRef::Scratch { id },
            })
            .collect(),
        active: s.active,
    }
}

#[tauri::command]
#[specta::specta]
pub fn session_save(state: tauri::State<'_, AppState>, session: Session) -> Result<()> {
    let s = store_session::Session {
        workspace: session.workspace.map(PathBuf::from),
        tabs: session
            .tabs
            .into_iter()
            .map(|t| match t {
                TabRef::Request { id } => store_session::TabRef::Request { id },
                TabRef::Scratch { id } => store_session::TabRef::Scratch { id },
            })
            .collect(),
        active: session.active,
    };
    Ok(store_session::save_session(&state.workspaces.session_file, &s)?)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        tauri_specta::Builder::<tauri::test::MockRuntime>::new()
            .events(tauri_specta::collect_events![WorkspaceChanged])
            .mount_events(&app);
        app
    }

    #[test]
    fn external_edit_emits_workspace_changed_and_save_conflicts() {
        let data = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let app = mock_app();
        let guard = PathGuard::new();
        let workspaces = Workspaces::new(data.path()).unwrap();

        let ws = Workspace::create(root.path(), "Demo").unwrap();
        let info = workspaces.install(app.handle(), &guard, ws).unwrap();
        assert!(
            guard.check(&root.path().join("x.yaml")).is_ok(),
            "workspace root allowed"
        );
        assert_eq!(workspaces.recent.list()[0].name, "Demo");

        let current = workspaces.current().unwrap();
        let node = lock(&current)
            .create_request(info.tree.id, "Get user", None)
            .unwrap()
            .node;
        let loaded = lock(&current).load_request(node.id).unwrap();
        let path = path_of(&lock(&current), node.id).unwrap();

        let (tx, rx) = mpsc::channel();
        WorkspaceChanged::listen(app.handle(), move |event| {
            let _ = tx.send(event.payload);
        });

        // Someone else (git pull, an editor) changes the file.
        let mut external = loaded.file.clone();
        external.url = "https://example.com/changed".into();
        std::fs::write(&path, ironcall_store::yaml::to_string(&external).unwrap()).unwrap();

        let event = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("WorkspaceChanged event");
        assert!(event.changed.contains(&node.id), "{event:?}");

        // Saving with the stale base hash is refused, and the disk copy is returned.
        let mut mine = loaded.file.clone();
        mine.url = "https://example.com/mine".into();
        match lock(&current)
            .save_request(&mine, loaded.hash.as_deref(), false)
            .unwrap()
        {
            store::SaveOutcome::Conflict { disk: Some(disk) } => {
                assert_eq!(disk.file.url, "https://example.com/changed")
            }
            other => panic!("expected a conflict, got {other:?}"),
        }

        workspaces.close(&guard);
        assert!(
            guard.check(&root.path().join("x.yaml")).is_err(),
            "root revoked on close"
        );
    }

    #[test]
    fn only_picked_or_recent_folders_are_accepted() {
        let data = tempfile::tempdir().unwrap();
        let picked = tempfile::tempdir().unwrap();
        let recent = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let workspaces = Workspaces::new(data.path()).unwrap();

        // Script in the webview naming a folder itself is refused.
        assert!(workspaces.authorize(other.path(), true).is_err());
        assert!(
            workspaces
                .authorize(Path::new("/nonexistent/folder"), true)
                .is_err()
        );

        let dir = workspaces.add_picked(picked.path()).unwrap();
        assert_eq!(workspaces.authorize(picked.path(), false).unwrap(), dir);
        // A different spelling of a picked folder resolves to the same one.
        let dotted = picked.path().join(".");
        assert_eq!(workspaces.authorize(&dotted, false).unwrap(), dir);

        // Recent workspaces can be reopened, but not created over.
        workspaces.recent.touch(recent.path(), "Recent").unwrap();
        assert!(workspaces.authorize(recent.path(), true).is_ok());
        assert!(workspaces.authorize(recent.path(), false).is_err());
        assert!(workspaces.authorize(other.path(), true).is_err());
    }
}
