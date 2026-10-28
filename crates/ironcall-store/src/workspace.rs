//! One open workspace: scanning the folder tree, CRUD on folders and requests, and
//! conflict-checked saving (PLAN.md §4, Phase 2).
//!
//! The workspace folder is the source of truth. After every operation the tree is
//! rescanned; parsed files are cached by modification time and size, so rescans only
//! read files that changed. Files that can't be parsed become error nodes and are never
//! written. Files are only rewritten when the user changes them, with one exception:
//! a file that duplicates another file's ID (e.g. copied in a file manager) is given a
//! new ID, and the reassignment is reported.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use ironcall_core::ItemId;
use ironcall_core::model::{Auth, CollectionFile, Extra, FolderFile, RequestFile, Vars, schema};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::atomic::{self, OwnWrites, content_hash};
use crate::slug;
use crate::yaml::{self, YamlError};

mod environments;
mod local;
mod vars;

pub use environments::EnvironmentSummary;
pub use local::{LOCAL_FILE, LocalSettings};
pub use vars::{FolderVars, SecretScopeRef, declared_secrets};

pub const COLLECTION_FILE: &str = "ironcall.yaml";
pub const FOLDER_FILE: &str = "_folder.yaml";
/// Local-only state inside a workspace (never committed).
pub const LOCAL_DIR: &str = ".ironcall";
pub const ENVIRONMENTS_DIR: &str = "environments";
const REQUEST_EXT: &str = ".yaml";
/// Highest file schema version this build understands. Newer files are read-only.
const SUPPORTED_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{path}: {source}")]
    Yaml { path: String, source: YamlError },
    #[error("no item with ID {0} in this workspace")]
    NotFound(ItemId),
    #[error("{0}")]
    Invalid(String),
    #[error("{0} already contains a workspace (ironcall.yaml)")]
    AlreadyExists(String),
    #[error("{0} is not an IronCall workspace (it has no ironcall.yaml)")]
    NotAWorkspace(String),
    #[error("{0} was written by a newer version of IronCall and is read-only")]
    ReadOnly(String),
}

pub type Result<T> = std::result::Result<T, WorkspaceError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Collection,
    Folder,
    Request,
    /// A request file that couldn't be read. Never written.
    Error,
}

/// A node of the workspace tree, in display order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// Stable for requests and folders with a `_folder.yaml`; session-only otherwise.
    pub id: ItemId,
    pub kind: NodeKind,
    pub name: String,
    pub method: Option<String>,
    /// Request URL (without query), for the sidebar filter.
    pub url: Option<String>,
    /// Relative to the workspace root, `/`-separated. Empty for the collection.
    pub path: String,
    /// Why the file (or a folder's `_folder.yaml`, or `ironcall.yaml`) couldn't be read.
    pub error: Option<String>,
    pub read_only: bool,
    pub children: Vec<Node>,
}

impl Node {
    /// Finds a node by ID in this subtree.
    pub fn find(&self, id: ItemId) -> Option<&Node> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }

    /// Number of request nodes in this subtree.
    pub fn request_count(&self) -> usize {
        usize::from(self.kind == NodeKind::Request)
            + self.children.iter().map(Node::request_count).sum::<usize>()
    }
}

/// A file loaded for editing. `hash` is the content hash to pass back when saving;
/// `None` only for folders that have no `_folder.yaml` yet.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded<T> {
    pub file: T,
    pub hash: Option<String>,
    pub read_only: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SaveOutcome<T> {
    Saved {
        hash: String,
    },
    /// The file changed on disk since the base hash. Nothing was written.
    /// `disk` is `None` if the file was deleted or can no longer be parsed.
    Conflict {
        disk: Option<Loaded<T>>,
    },
}

/// A new node, plus a warning if its path is close to Windows' `MAX_PATH`.
#[derive(Clone, Debug)]
pub struct Created {
    pub node: Node,
    pub warning: Option<String>,
}

/// A file whose ID duplicated another file's and was replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reassignment {
    pub path: String,
    pub old: ItemId,
    pub new: ItemId,
}

/// What a rescan found compared with the previous state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceEvent {
    /// The tree's structure, names, methods or errors changed.
    pub tree_changed: bool,
    /// Requests whose file content changed.
    pub changed: Vec<ItemId>,
    /// Requests that disappeared.
    pub removed: Vec<ItemId>,
    /// `ironcall.yaml` or a `_folder.yaml` changed.
    pub meta_changed: bool,
    /// An environment file was added, removed, renamed or changed.
    pub environments_changed: bool,
    pub reassigned: Vec<Reassignment>,
}

impl WorkspaceEvent {
    pub fn is_empty(&self) -> bool {
        !self.tree_changed
            && self.changed.is_empty()
            && self.removed.is_empty()
            && !self.meta_changed
            && !self.environments_changed
            && self.reassigned.is_empty()
    }
}

/// Where deleted items go. The app uses [`SystemTrash`]; tests use [`DirTrash`].
pub trait Trash: Send + Sync {
    fn trash(&self, path: &Path) -> std::io::Result<()>;
}

/// The OS trash / recycle bin.
#[derive(Debug, Default)]
pub struct SystemTrash;

impl Trash for SystemTrash {
    fn trash(&self, path: &Path) -> std::io::Result<()> {
        trash::delete(path).map_err(std::io::Error::other)
    }
}

/// Moves deleted items into a directory. For tests, and systems without a trash.
#[derive(Debug)]
pub struct DirTrash(pub PathBuf);

impl Trash for DirTrash {
    fn trash(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.0)?;
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let mut target = self.0.join(name.as_ref());
        let mut n = 2;
        while target.exists() {
            target = self.0.join(format!("{name}.{n}"));
            n += 1;
        }
        std::fs::rename(path, target)
    }
}

/// Bookkeeping for one node.
#[derive(Clone, Debug)]
struct Entry {
    kind: NodeKind,
    /// Request or error: the file. Folder: the directory. Collection: the root.
    path: PathBuf,
    parent: Option<ItemId>,
    /// Request file hash, or folder `_folder.yaml` hash.
    hash: Option<String>,
    /// The ID is stored in a file (not session-only).
    persistent: bool,
    /// A folder whose `_folder.yaml` is invalid (so it can't be written).
    meta_error: bool,
    read_only: bool,
}

/// What we keep from parsing a file, cached by (mtime, len).
#[derive(Clone, Debug)]
enum Parsed {
    Request {
        id: ItemId,
        name: String,
        method: String,
        url: String,
        read_only: bool,
    },
    Folder {
        id: ItemId,
        name: String,
        order: Vec<ItemId>,
        read_only: bool,
    },
    Error(String),
}

#[derive(Clone, Debug)]
struct Cached {
    mtime: Option<SystemTime>,
    len: u64,
    hash: String,
    parsed: Parsed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Request,
    Folder,
}

pub struct Workspace {
    root: PathBuf,
    collection: CollectionFile,
    collection_hash: Option<String>,
    collection_error: Option<String>,
    collection_read_only: bool,
    tree: Node,
    index: HashMap<ItemId, Entry>,
    /// Session-only IDs for folders without `_folder.yaml` and for error nodes, by
    /// relative path.
    session_ids: HashMap<PathBuf, ItemId>,
    cache: HashMap<PathBuf, Cached>,
    own: Arc<OwnWrites>,
    trash: Box<dyn Trash>,
    reassigned: Vec<Reassignment>,
    /// Files in `environments/`, sorted by name.
    environments: Vec<environments::EnvEntry>,
    env_cache: HashMap<PathBuf, environments::EnvCached>,
}

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Workspace {
    /// Creates a workspace in `root` (created if missing): `ironcall.yaml`,
    /// `.gitattributes` (if missing) and `.ironcall/.gitignore`. Then opens it.
    pub fn create(root: &Path, name: &str) -> Result<Self> {
        let name = clean_name(name)?;
        std::fs::create_dir_all(root)?;
        let collection_path = root.join(COLLECTION_FILE);
        if collection_path.exists() {
            return Err(WorkspaceError::AlreadyExists(root.display().to_string()));
        }
        let collection = CollectionFile {
            schema: schema::COLLECTION.to_string(),
            id: ItemId::new(),
            name,
            description: None,
            vars: Vars::default(),
            headers: Vec::new(),
            auth: Auth::default(),
            order: Vec::new(),
            extra: Extra::default(),
        };
        atomic::write_atomic(&collection_path, to_yaml(&collection)?.as_bytes())?;
        let gitattributes = root.join(".gitattributes");
        if !gitattributes.exists() {
            atomic::write_atomic(&gitattributes, b"*.yaml text eol=lf\n")?;
        }
        let local = root.join(LOCAL_DIR);
        std::fs::create_dir_all(&local)?;
        atomic::write_atomic(&local.join(".gitignore"), b"*\n")?;
        Self::open(root)
    }

    /// Opens the workspace in `root`, which must contain `ironcall.yaml`.
    pub fn open(root: &Path) -> Result<Self> {
        let root = std::fs::canonicalize(root)?;
        if !root.join(COLLECTION_FILE).is_file() {
            return Err(WorkspaceError::NotAWorkspace(root.display().to_string()));
        }
        let name = root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut ws = Self {
            collection: CollectionFile {
                schema: schema::COLLECTION.to_string(),
                id: ItemId::new(),
                name: name.clone(),
                description: None,
                vars: Vars::default(),
                headers: Vec::new(),
                auth: Auth::default(),
                order: Vec::new(),
                extra: Extra::default(),
            },
            collection_hash: None,
            collection_error: None,
            collection_read_only: false,
            tree: Node {
                id: ItemId::new(),
                kind: NodeKind::Collection,
                name,
                method: None,
                url: None,
                path: String::new(),
                error: None,
                read_only: false,
                children: Vec::new(),
            },
            index: HashMap::new(),
            session_ids: HashMap::new(),
            cache: HashMap::new(),
            own: Arc::new(OwnWrites::new()),
            trash: Box::new(SystemTrash),
            reassigned: Vec::new(),
            environments: Vec::new(),
            env_cache: HashMap::new(),
            root,
        };
        ws.rescan(&[])?;
        Ok(ws)
    }

    /// Uses `trash` for deletions instead of the OS trash.
    pub fn with_trash(mut self, trash: Box<dyn Trash>) -> Self {
        self.trash = trash;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The workspace ID from `ironcall.yaml`.
    pub fn id(&self) -> ItemId {
        self.collection.id
    }

    pub fn name(&self) -> &str {
        &self.collection.name
    }

    pub fn tree(&self) -> &Node {
        &self.tree
    }

    /// Shared with the watcher so it can ignore our own writes.
    pub fn own_writes(&self) -> Arc<OwnWrites> {
        self.own.clone()
    }

    /// Duplicate-ID fixes made since the last call (including during `open`).
    pub fn take_reassigned(&mut self) -> Vec<Reassignment> {
        std::mem::take(&mut self.reassigned)
    }

    /// Absolute path of an item's file (requests, error nodes) or directory (folders,
    /// the collection).
    pub fn path_of(&self, id: ItemId) -> Option<&Path> {
        self.index.get(&id).map(|e| e.path.as_path())
    }

    // -----------------------------------------------------------------------------
    // Scanning

    /// Rescans the workspace and reports what changed since the previous scan.
    /// `changed_paths` (from the watcher) are re-read even if their mtime looks unchanged.
    pub fn rescan(&mut self, changed_paths: &[PathBuf]) -> Result<WorkspaceEvent> {
        for path in changed_paths {
            self.cache.remove(path);
            self.env_cache.remove(path);
        }
        let old_tree = self.tree.clone();
        // A copy, not a take: if the walk fails, the workspace keeps its last good index
        // instead of every request becoming "not found".
        let old_index = self.index.clone();
        let old_collection_hash = self.collection_hash.clone();

        self.read_collection();
        let mut items = vec![RawItem::root()];
        let mut seen_files = HashSet::new();
        self.walk(&self.root.clone(), Path::new(""), 0, &mut items, &mut seen_files)?;
        self.cache.retain(|p, _| seen_files.contains(p));
        let reassigned = self.dedupe_ids(&mut items)?;
        self.build(&items);

        let mut event = WorkspaceEvent {
            tree_changed: self.tree != old_tree,
            meta_changed: self.collection_hash != old_collection_hash,
            reassigned: reassigned.clone(),
            ..WorkspaceEvent::default()
        };
        for (id, old) in &old_index {
            match self.index.get(id) {
                None if old.kind == NodeKind::Request => event.removed.push(*id),
                Some(new) if new.kind == NodeKind::Request && old.kind == NodeKind::Request => {
                    if new.hash != old.hash {
                        event.changed.push(*id);
                    }
                }
                Some(new)
                    if new.kind == NodeKind::Folder
                        && old.kind == NodeKind::Folder
                        && new.hash != old.hash =>
                {
                    event.meta_changed = true;
                }
                _ => {}
            }
        }
        event.changed.sort();
        event.removed.sort();
        self.reassigned.extend(reassigned);

        let (environments_changed, env_reassigned) = self.scan_environments()?;
        event.environments_changed = environments_changed;
        event.reassigned.extend(env_reassigned.iter().cloned());
        self.reassigned.extend(env_reassigned);
        Ok(event)
    }

    fn read_collection(&mut self) {
        let path = self.root.join(COLLECTION_FILE);
        match std::fs::read(&path) {
            Ok(bytes) => {
                self.collection_hash = Some(content_hash(&bytes));
                match parse::<CollectionFile>(&bytes) {
                    Ok(c) => match yaml::schema_version(&c.schema, "collection") {
                        Some(v) => {
                            self.collection_read_only = v > SUPPORTED_VERSION;
                            self.collection = c;
                            self.collection_error = None;
                        }
                        None => {
                            self.collection_error =
                                Some(format!("not a collection file (schema `{}`)", c.schema));
                        }
                    },
                    Err(e) => self.collection_error = Some(e),
                }
            }
            Err(e) => {
                self.collection_hash = None;
                self.collection_error = Some(format!("cannot read {COLLECTION_FILE}: {e}"));
            }
        }
    }

    fn walk(
        &mut self,
        dir: &Path,
        rel: &Path,
        parent: usize,
        items: &mut Vec<RawItem>,
        seen: &mut HashSet<PathBuf>,
    ) -> Result<()> {
        let is_root = parent == 0;
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            let abs = entry.path();
            let child_rel = rel.join(&file_name);
            // Symlinks are skipped: a linked folder can loop (`a -> .`) or show one file
            // under two paths, whose duplicate ID would then be rewritten on every scan,
            // and edits would land outside the workspace.
            if entry.file_type().is_ok_and(|t| t.is_symlink()) {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&abs) else {
                continue;
            };
            if meta.is_dir() {
                if is_root && name == ENVIRONMENTS_DIR {
                    continue;
                }
                let mut item =
                    RawItem::new(NodeKind::Folder, child_rel.clone(), abs.clone(), name.to_string());
                let meta_path = abs.join(FOLDER_FILE);
                if meta_path.is_file() {
                    seen.insert(meta_path.clone());
                    // Gone or unreadable since the listing (a git checkout in progress):
                    // skip this folder now; the watcher rescans when the change settles.
                    let Ok(cached) = self.cached(&meta_path, FileKind::Folder) else {
                        continue;
                    };
                    item.hash = Some(cached.hash.clone());
                    item.mtime = cached.mtime;
                    match cached.parsed {
                        Parsed::Folder {
                            id,
                            name,
                            order,
                            read_only,
                        } => {
                            item.id = Some(id);
                            item.name = name;
                            item.order = order;
                            item.read_only = read_only;
                        }
                        Parsed::Error(e) => {
                            item.error = Some(format!("{FOLDER_FILE}: {e}"));
                            item.meta_error = true;
                        }
                        Parsed::Request { .. } => unreachable!("folder files parse as folders"),
                    }
                }
                let index = items.len();
                items[parent].children.push(index);
                items.push(item);
                // An unreadable folder (no permission) shows as an error node; the rest of
                // the workspace still opens.
                if let Err(e) = self.walk(&abs, &child_rel, index, items, seen) {
                    items.truncate(index + 1);
                    items[index].error = Some(format!("can't read this folder: {e}"));
                    items[index].children.clear();
                }
            } else if meta.is_file() {
                if name == FOLDER_FILE || (is_root && name == COLLECTION_FILE) || !name.ends_with(REQUEST_EXT)
                {
                    continue;
                }
                seen.insert(abs.clone());
                let Ok(cached) = self.cached(&abs, FileKind::Request) else {
                    continue;
                };
                let mut item = RawItem::new(NodeKind::Request, child_rel, abs, name.to_string());
                item.hash = Some(cached.hash.clone());
                item.mtime = cached.mtime;
                match cached.parsed {
                    Parsed::Request {
                        id,
                        name,
                        method,
                        url,
                        read_only,
                    } => {
                        item.url = Some(url);
                        item.id = Some(id);
                        item.name = name;
                        item.method = Some(method);
                        item.read_only = read_only;
                    }
                    Parsed::Error(e) => {
                        item.kind = NodeKind::Error;
                        item.error = Some(e);
                    }
                    Parsed::Folder { .. } => unreachable!("request files parse as requests"),
                }
                let index = items.len();
                items[parent].children.push(index);
                items.push(item);
            }
        }
        Ok(())
    }

    /// Parses `path`, reusing the cached result if its mtime and size are unchanged.
    fn cached(&mut self, path: &Path, kind: FileKind) -> Result<Cached> {
        let meta = std::fs::metadata(path)?;
        let mtime = meta.modified().ok();
        if let Some(c) = self.cache.get(path)
            && c.mtime == mtime
            && c.len == meta.len()
            && mtime.is_some()
        {
            return Ok(c.clone());
        }
        let bytes = std::fs::read(path)?;
        let parsed = match kind {
            FileKind::Request => match parse::<RequestFile>(&bytes) {
                Ok(r) => match yaml::schema_version(&r.schema, "request") {
                    Some(v) => Parsed::Request {
                        id: r.id,
                        name: r.name,
                        method: r.method,
                        url: r.url,
                        read_only: v > SUPPORTED_VERSION,
                    },
                    None => Parsed::Error(format!("not a request file (schema `{}`)", r.schema)),
                },
                Err(e) => Parsed::Error(e),
            },
            FileKind::Folder => match parse::<FolderFile>(&bytes) {
                Ok(f) => match yaml::schema_version(&f.schema, "folder") {
                    Some(v) => Parsed::Folder {
                        id: f.id,
                        name: f.name,
                        order: f.order,
                        read_only: v > SUPPORTED_VERSION,
                    },
                    None => Parsed::Error(format!("not a folder file (schema `{}`)", f.schema)),
                },
                Err(e) => Parsed::Error(e),
            },
        };
        let cached = Cached {
            mtime,
            len: bytes.len() as u64,
            hash: content_hash(&bytes),
            parsed,
        };
        self.cache.insert(path.to_path_buf(), cached.clone());
        Ok(cached)
    }

    /// Gives files that share an ID (e.g. copied on disk) new IDs, keeping the oldest.
    fn dedupe_ids(&mut self, items: &mut [RawItem]) -> Result<Vec<Reassignment>> {
        // Only files that share an ID are stat'ed again, so this stays cheap.
        let mut by_id: HashMap<ItemId, Vec<usize>> = HashMap::new();
        for (i, item) in items.iter().enumerate() {
            if let Some(id) = item.id {
                by_id.entry(id).or_default().push(i);
            }
        }
        if let Some(c) = self.collection_error.is_none().then_some(self.collection.id) {
            // A request or folder must never share the workspace's own ID.
            if let Some(list) = by_id.get_mut(&c) {
                list.insert(0, usize::MAX);
            }
        }
        let mut reassigned = Vec::new();
        for (old, mut list) in by_id {
            if list.len() < 2 {
                continue;
            }
            list.sort_by(|&a, &b| {
                let key = |i: usize| {
                    items
                        .get(i)
                        .map(|it| (copy_age(&it.abs), name_len(&it.rel), it.rel.clone()))
                };
                if a == usize::MAX {
                    return std::cmp::Ordering::Less;
                }
                if b == usize::MAX {
                    return std::cmp::Ordering::Greater;
                }
                key(a).cmp(&key(b))
            });
            for &i in &list[1..] {
                let item = &mut items[i];
                if item.read_only {
                    continue;
                }
                let new = ItemId::new();
                let file = if item.kind == NodeKind::Folder {
                    item.abs.join(FOLDER_FILE)
                } else {
                    item.abs.clone()
                };
                let bytes = std::fs::read(&file)?;
                let text = match item.kind {
                    NodeKind::Folder => {
                        let mut f: FolderFile = parse(&bytes).map_err(WorkspaceError::Invalid)?;
                        f.id = new;
                        to_yaml(&f)?
                    }
                    _ => {
                        let mut r: RequestFile = parse(&bytes).map_err(WorkspaceError::Invalid)?;
                        r.id = new;
                        to_yaml(&r)?
                    }
                };
                atomic::write_own(&self.own, &file, text.as_bytes())?;
                self.cache.remove(&file);
                item.id = Some(new);
                item.hash = Some(content_hash(text.as_bytes()));
                tracing::info!(path = %item.rel.display(), %old, %new, "reassigned duplicate ID");
                reassigned.push(Reassignment {
                    path: slash_path(&item.rel),
                    old,
                    new,
                });
            }
        }
        reassigned.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(reassigned)
    }

    /// Builds the tree and index from scanned items.
    fn build(&mut self, items: &[RawItem]) {
        let mut ids = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let id = if i == 0 {
                self.collection.id
            } else if let Some(id) = item.id {
                id
            } else {
                *self.session_ids.entry(item.rel.clone()).or_default()
            };
            ids.push(id);
        }
        let live: HashSet<&PathBuf> = items.iter().filter(|i| i.id.is_none()).map(|i| &i.rel).collect();
        self.session_ids.retain(|p, _| live.contains(p));

        let mut index = HashMap::with_capacity(items.len());
        let tree = self.build_node(items, &ids, 0, None, &mut index);
        self.tree = tree;
        self.index = index;
    }

    fn build_node(
        &self,
        items: &[RawItem],
        ids: &[ItemId],
        i: usize,
        parent: Option<ItemId>,
        index: &mut HashMap<ItemId, Entry>,
    ) -> Node {
        let item = &items[i];
        let id = ids[i];
        let order: &[ItemId] = if i == 0 {
            &self.collection.order
        } else {
            &item.order
        };
        let position: HashMap<ItemId, usize> = order.iter().enumerate().map(|(n, id)| (*id, n)).collect();

        let mut children: Vec<usize> = item.children.clone();
        children.sort_by(|&a, &b| {
            let (ia, ib) = (&items[a], &items[b]);
            let pa = ia.id.and_then(|id| position.get(&id));
            let pb = ib.id.and_then(|id| position.get(&id));
            match (pa, pb) {
                (Some(x), Some(y)) => x.cmp(y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => {
                    let rank = |it: &RawItem| u8::from(it.kind != NodeKind::Folder);
                    (rank(ia), ia.name.to_lowercase(), &ia.rel).cmp(&(
                        rank(ib),
                        ib.name.to_lowercase(),
                        &ib.rel,
                    ))
                }
            }
        });

        let (kind, name, error, read_only) = if i == 0 {
            (
                NodeKind::Collection,
                self.collection.name.clone(),
                self.collection_error.clone(),
                self.collection_read_only,
            )
        } else {
            (item.kind, item.name.clone(), item.error.clone(), item.read_only)
        };
        index.insert(
            id,
            Entry {
                kind,
                path: if i == 0 {
                    self.root.clone()
                } else {
                    item.abs.clone()
                },
                parent,
                hash: if i == 0 {
                    self.collection_hash.clone()
                } else {
                    item.hash.clone()
                },
                persistent: i == 0 || item.id.is_some(),
                meta_error: item.meta_error,
                read_only,
            },
        );
        Node {
            id,
            kind,
            name,
            method: item.method.clone(),
            url: item.url.clone(),
            path: slash_path(&item.rel),
            error,
            read_only,
            children: children
                .into_iter()
                .map(|c| self.build_node(items, ids, c, Some(id), index))
                .collect(),
        }
    }

    // -----------------------------------------------------------------------------
    // Loading and saving

    pub fn load_request(&self, id: ItemId) -> Result<Loaded<RequestFile>> {
        let entry = self.entry(id)?;
        match entry.kind {
            NodeKind::Request => {}
            NodeKind::Error => {
                let err = self
                    .tree
                    .find(id)
                    .and_then(|n| n.error.clone())
                    .unwrap_or_default();
                return Err(WorkspaceError::Invalid(format!(
                    "this file can't be opened: {err}"
                )));
            }
            _ => return Err(WorkspaceError::Invalid("not a request".into())),
        }
        let bytes = std::fs::read(&entry.path)?;
        let file: RequestFile = parse(&bytes).map_err(|e| self.yaml_error(&entry.path, e))?;
        Ok(Loaded {
            file,
            hash: Some(content_hash(&bytes)),
            read_only: entry.read_only,
        })
    }

    /// Saves a request. Refuses with `Conflict` if the file on disk doesn't match
    /// `base_hash` (unless `force`). Writing identical content is a no-op.
    pub fn save_request(
        &mut self,
        file: &RequestFile,
        base_hash: Option<&str>,
        force: bool,
    ) -> Result<SaveOutcome<RequestFile>> {
        let entry = self.entry(file.id)?.clone();
        if entry.kind != NodeKind::Request {
            return Err(WorkspaceError::Invalid("not a request".into()));
        }
        if entry.read_only {
            return Err(WorkspaceError::ReadOnly(self.rel(&entry.path)));
        }
        ensure_no_secret_values(&file.vars)?;
        let text = to_yaml(file)?;
        match self.check_and_write(&entry.path, text.as_bytes(), base_hash, force)? {
            Some(hash) => {
                // The file is saved; a failed rescan is only logged (the watcher retries).
                if let Err(e) = self.rescan(std::slice::from_ref(&entry.path)) {
                    tracing::warn!(error = %e, "rescan after save failed");
                }
                Ok(SaveOutcome::Saved { hash })
            }
            None => Ok(SaveOutcome::Conflict {
                disk: self.load_request(file.id).ok(),
            }),
        }
    }

    pub fn load_folder(&self, id: ItemId) -> Result<Loaded<FolderFile>> {
        let entry = self.entry(id)?;
        if entry.kind != NodeKind::Folder {
            return Err(WorkspaceError::Invalid("not a folder".into()));
        }
        let path = entry.path.join(FOLDER_FILE);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Loaded {
                file: parse(&bytes).map_err(|e| self.yaml_error(&path, e))?,
                hash: Some(content_hash(&bytes)),
                read_only: entry.read_only,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Loaded {
                file: self.new_folder_file(id, &entry.path),
                hash: None,
                read_only: false,
            }),
            Err(e) => Err(e.into()),
        }
    }

    /// Saves a folder's `_folder.yaml`, creating it if needed.
    pub fn save_folder(
        &mut self,
        file: &FolderFile,
        base_hash: Option<&str>,
        force: bool,
    ) -> Result<SaveOutcome<FolderFile>> {
        let entry = self.entry(file.id)?.clone();
        if entry.kind != NodeKind::Folder {
            return Err(WorkspaceError::Invalid("not a folder".into()));
        }
        if entry.read_only {
            return Err(WorkspaceError::ReadOnly(self.rel(&entry.path)));
        }
        ensure_no_secret_values(&file.vars)?;
        let path = entry.path.join(FOLDER_FILE);
        let text = to_yaml(file)?;
        match self.check_and_write(&path, text.as_bytes(), base_hash, force)? {
            Some(hash) => {
                // The file is saved; a failed rescan is only logged (the watcher retries).
                if let Err(e) = self.rescan(&[path]) {
                    tracing::warn!(error = %e, "rescan after save failed");
                }
                Ok(SaveOutcome::Saved { hash })
            }
            None => Ok(SaveOutcome::Conflict {
                disk: self.load_folder(file.id).ok().filter(|l| l.hash.is_some()),
            }),
        }
    }

    pub fn load_collection(&self) -> Result<Loaded<CollectionFile>> {
        let path = self.root.join(COLLECTION_FILE);
        let bytes = std::fs::read(&path)?;
        Ok(Loaded {
            file: parse(&bytes).map_err(|e| self.yaml_error(&path, e))?,
            hash: Some(content_hash(&bytes)),
            read_only: self.collection_read_only,
        })
    }

    pub fn save_collection(
        &mut self,
        file: &CollectionFile,
        base_hash: Option<&str>,
        force: bool,
    ) -> Result<SaveOutcome<CollectionFile>> {
        if self.collection_read_only {
            return Err(WorkspaceError::ReadOnly(COLLECTION_FILE.into()));
        }
        if file.id != self.collection.id && self.collection_error.is_none() {
            return Err(WorkspaceError::Invalid(
                "the collection ID can't be changed".into(),
            ));
        }
        ensure_no_secret_values(&file.vars)?;
        let path = self.root.join(COLLECTION_FILE);
        let text = to_yaml(file)?;
        match self.check_and_write(&path, text.as_bytes(), base_hash, force)? {
            Some(hash) => {
                // The file is saved; a failed rescan is only logged (the watcher retries).
                if let Err(e) = self.rescan(&[]) {
                    tracing::warn!(error = %e, "rescan after save failed");
                }
                Ok(SaveOutcome::Saved { hash })
            }
            None => Ok(SaveOutcome::Conflict {
                disk: self.load_collection().ok(),
            }),
        }
    }

    /// Writes `bytes` unless the disk content no longer matches `base_hash`.
    /// Returns the new hash, or `None` on conflict.
    fn check_and_write(
        &mut self,
        path: &Path,
        bytes: &[u8],
        base_hash: Option<&str>,
        force: bool,
    ) -> Result<Option<String>> {
        let current = match std::fs::read(path) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        if !force {
            let current_hash = current.as_deref().map(content_hash);
            if let Some(base) = base_hash
                && current_hash.as_deref() != Some(base)
            {
                return Ok(None);
            }
        }
        let hash = content_hash(bytes);
        if current.as_deref() != Some(bytes) {
            atomic::write_own(&self.own, path, bytes)?;
            self.cache.remove(path);
        }
        Ok(Some(hash))
    }

    // -----------------------------------------------------------------------------
    // Tree operations

    /// Creates a request named `name` under `parent` at `index` (end if `None`).
    ///
    /// If `index` is `None` and the parent has no explicit order, the parent file isn't
    /// touched and the request takes its alphabetical place.
    pub fn create_request(&mut self, parent: ItemId, name: &str, index: Option<usize>) -> Result<Created> {
        let name = clean_name(name)?;
        let dir = self.container_dir(parent)?;
        let unique = slug::unique_name(&dir, &slug::slugify(&name), REQUEST_EXT, None)?;
        let request = RequestFile::new(name);
        let path = dir.join(&unique.name);
        atomic::write_own(&self.own, &path, to_yaml(&request)?.as_bytes())?;
        self.place_new(parent, request.id, index)?;
        self.created(request.id, unique.warning)
    }

    /// Creates a folder (with a `_folder.yaml`, so its ID is stable) under `parent`.
    pub fn create_folder(&mut self, parent: ItemId, name: &str, index: Option<usize>) -> Result<Created> {
        let name = clean_name(name)?;
        let dir = self.container_dir(parent)?;
        let unique = slug::unique_name(&dir, &slug::slugify(&name), "", None)?;
        let path = dir.join(&unique.name);
        std::fs::create_dir(&path)?;
        self.own.record_tree(&path);
        let id = ItemId::new();
        let mut folder = self.new_folder_file(id, &path);
        folder.name = name;
        atomic::write_own(&self.own, &path.join(FOLDER_FILE), to_yaml(&folder)?.as_bytes())?;
        self.place_new(parent, id, index)?;
        self.created(id, unique.warning)
    }

    /// Changes an item's display name. File names never change here (PLAN.md §4.3).
    pub fn rename(&mut self, id: ItemId, name: &str) -> Result<()> {
        let name = clean_name(name)?;
        let entry = self.entry(id)?.clone();
        match entry.kind {
            NodeKind::Request => {
                let mut loaded = self.load_request(id)?;
                if entry.read_only {
                    return Err(WorkspaceError::ReadOnly(self.rel(&entry.path)));
                }
                loaded.file.name = name;
                self.save_request(&loaded.file, None, true)?;
            }
            NodeKind::Folder => self.update_folder_meta(id, |f| f.name = name)?,
            NodeKind::Collection => self.update_collection(|c| c.name = name)?,
            NodeKind::Error => return Err(WorkspaceError::Invalid("fix the file before renaming it".into())),
        }
        self.rescan(&[])?;
        Ok(())
    }

    /// Renames an item's file (or folder directory) to match its display name.
    pub fn rename_file_to_match(&mut self, id: ItemId) -> Result<Option<String>> {
        let entry = self.entry(id)?.clone();
        let (ext, name) = match entry.kind {
            NodeKind::Request => (REQUEST_EXT, self.load_request(id)?.file.name),
            NodeKind::Folder => ("", self.tree.find(id).map(|n| n.name.clone()).unwrap_or_default()),
            _ => {
                return Err(WorkspaceError::Invalid(
                    "only requests and folders have file names".into(),
                ));
            }
        };
        let dir = entry.path.parent().expect("items live inside the workspace");
        let current = entry
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let unique = slug::unique_name(dir, &slug::slugify(&name), ext, Some(&current))?;
        if unique.name == current {
            return Ok(unique.warning);
        }
        self.move_path(&entry.path, &dir.join(&unique.name), entry.kind)?;
        self.rescan(&[])?;
        Ok(unique.warning)
    }

    /// Duplicates a request or folder (new IDs throughout), placed right after it.
    pub fn duplicate(&mut self, id: ItemId) -> Result<Created> {
        let entry = self.entry(id)?.clone();
        let parent = entry
            .parent
            .ok_or_else(|| WorkspaceError::Invalid("the collection can't be duplicated".into()))?;
        let dir = entry
            .path
            .parent()
            .expect("items live inside the workspace")
            .to_path_buf();
        let (new_id, warning) = match entry.kind {
            NodeKind::Request => {
                let mut file = self.load_request(id)?.file;
                file.id = ItemId::new();
                file.name = format!("{} copy", file.name);
                let unique = slug::unique_name(&dir, &slug::slugify(&file.name), REQUEST_EXT, None)?;
                atomic::write_own(&self.own, &dir.join(&unique.name), to_yaml(&file)?.as_bytes())?;
                (file.id, unique.warning)
            }
            NodeKind::Folder => {
                let name = format!(
                    "{} copy",
                    self.tree.find(id).map(|n| n.name.as_str()).unwrap_or("Folder")
                );
                let unique = slug::unique_name(&dir, &slug::slugify(&name), "", None)?;
                let target = dir.join(&unique.name);
                self.own.record_tree(&target);
                let new_id = copy_folder_with_new_ids(&entry.path, &target, &name, &self.own)?;
                (new_id, unique.warning)
            }
            _ => return Err(WorkspaceError::Invalid("this item can't be duplicated".into())),
        };
        self.rescan(&[])?;
        let mut order = self.child_order(parent);
        let pos = order.iter().position(|c| *c == id).map_or(order.len(), |p| p + 1);
        order.retain(|c| *c != new_id);
        order.insert(pos.min(order.len()), new_id);
        self.write_order(parent, order)?;
        self.created(new_id, warning)
    }

    /// Moves an item to the trash.
    pub fn delete(&mut self, id: ItemId) -> Result<()> {
        let entry = self.entry(id)?.clone();
        let parent = entry
            .parent
            .ok_or_else(|| WorkspaceError::Invalid("the collection can't be deleted".into()))?;
        if entry.kind == NodeKind::Folder {
            self.own.record_tree(&entry.path);
        } else {
            self.own.record_removed(&entry.path);
        }
        self.trash.trash(&entry.path)?;
        self.rescan(&[])?;
        // Tidy the parent's order list, but only if it explicitly listed the item.
        if self.listed_order(parent).is_some_and(|o| o.contains(&id)) {
            let order = self.child_order(parent);
            self.write_order(parent, order)?;
        }
        Ok(())
    }

    /// Moves an item to `parent` at `index`, or reorders it within its folder.
    pub fn move_item(&mut self, id: ItemId, parent: ItemId, index: usize) -> Result<()> {
        let entry = self.entry(id)?.clone();
        let old_parent = entry
            .parent
            .ok_or_else(|| WorkspaceError::Invalid("the collection can't be moved".into()))?;
        if entry.kind == NodeKind::Error {
            return Err(WorkspaceError::Invalid("fix the file before moving it".into()));
        }
        let target_dir = self.container_dir(parent)?;
        if entry.kind == NodeKind::Folder && target_dir.starts_with(&entry.path) {
            return Err(WorkspaceError::Invalid(
                "a folder can't be moved into itself".into(),
            ));
        }

        let mut id = id;
        if old_parent != parent {
            let current = entry
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let (stem, ext) = match entry.kind {
                NodeKind::Request => (current.trim_end_matches(REQUEST_EXT).to_string(), REQUEST_EXT),
                _ => (current.clone(), ""),
            };
            let unique = slug::unique_name(&target_dir, &stem, ext, None)?;
            let target = target_dir.join(&unique.name);
            let was_persistent = entry.persistent;
            self.move_path(&entry.path, &target, entry.kind)?;
            self.rescan(&[])?;
            if !was_persistent {
                // A folder without `_folder.yaml` keeps its session ID through the move.
                id = self
                    .index
                    .iter()
                    .find(|(_, e)| e.path == target)
                    .map(|(id, _)| *id)
                    .unwrap_or(id);
            }
            if self.listed_order(old_parent).is_some_and(|o| o.contains(&id)) {
                let order = self.child_order(old_parent);
                self.write_order(old_parent, order)?;
            }
        }
        let mut order = self.child_order(parent);
        order.retain(|c| *c != id);
        order.insert(index.min(order.len()), id);
        self.write_order(parent, order)
    }

    // -----------------------------------------------------------------------------
    // Helpers

    fn entry(&self, id: ItemId) -> Result<&Entry> {
        self.index.get(&id).ok_or(WorkspaceError::NotFound(id))
    }

    /// Directory of a collection or folder.
    fn container_dir(&self, id: ItemId) -> Result<PathBuf> {
        let entry = self.entry(id)?;
        match entry.kind {
            NodeKind::Collection | NodeKind::Folder => Ok(entry.path.clone()),
            _ => Err(WorkspaceError::Invalid(
                "items can only be added to folders".into(),
            )),
        }
    }

    /// IDs of a container's children in display order (error nodes excluded).
    fn child_order(&self, parent: ItemId) -> Vec<ItemId> {
        self.tree
            .find(parent)
            .map(|n| {
                n.children
                    .iter()
                    .filter(|c| c.kind != NodeKind::Error)
                    .map(|c| c.id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The parent's explicit `order` list, if it has a valid metadata file.
    fn listed_order(&self, parent: ItemId) -> Option<Vec<ItemId>> {
        if parent == self.collection.id {
            return Some(self.collection.order.clone());
        }
        self.load_folder(parent)
            .ok()
            .filter(|l| l.hash.is_some())
            .map(|l| l.file.order)
    }

    /// Places a new item: appended to an explicit order, or inserted at `index`.
    fn place_new(&mut self, parent: ItemId, id: ItemId, index: Option<usize>) -> Result<()> {
        self.rescan(&[])?;
        let has_order = self.listed_order(parent).is_some_and(|o| !o.is_empty());
        if index.is_none() && !has_order {
            return Ok(());
        }
        let mut order = self.child_order(parent);
        order.retain(|c| *c != id);
        let at = index.unwrap_or(order.len()).min(order.len());
        order.insert(at, id);
        self.write_order(parent, order)
    }

    /// Writes a container's `order`. Child folders with session-only IDs get a
    /// `_folder.yaml` first so their IDs can be listed.
    fn write_order(&mut self, parent: ItemId, order: Vec<ItemId>) -> Result<()> {
        for child in &order {
            if let Some(e) = self.index.get(child)
                && e.kind == NodeKind::Folder
                && !e.persistent
            {
                let child = *child;
                self.update_folder_meta(child, |_| {})?;
            }
        }
        if parent == self.collection.id {
            self.update_collection(|c| c.order = order)?;
        } else {
            self.update_folder_meta(parent, |f| f.order = order)?;
        }
        self.rescan(&[])?;
        Ok(())
    }

    /// Edits (or creates) a folder's `_folder.yaml`.
    fn update_folder_meta(&mut self, id: ItemId, edit: impl FnOnce(&mut FolderFile)) -> Result<()> {
        let entry = self.entry(id)?.clone();
        if entry.kind != NodeKind::Folder {
            return Err(WorkspaceError::Invalid("not a folder".into()));
        }
        if entry.meta_error {
            return Err(WorkspaceError::Invalid(format!(
                "fix {}/{FOLDER_FILE} first",
                self.rel(&entry.path)
            )));
        }
        if entry.read_only {
            return Err(WorkspaceError::ReadOnly(self.rel(&entry.path)));
        }
        let mut loaded = self.load_folder(id)?;
        edit(&mut loaded.file);
        let path = entry.path.join(FOLDER_FILE);
        let text = to_yaml(&loaded.file)?;
        if std::fs::read(&path).ok().as_deref() != Some(text.as_bytes()) {
            atomic::write_own(&self.own, &path, text.as_bytes())?;
            self.cache.remove(&path);
        }
        // Keep the folder's ID stable now that it is persistent.
        let rel = entry
            .path
            .strip_prefix(&self.root)
            .unwrap_or(&entry.path)
            .to_path_buf();
        self.session_ids.remove(&rel);
        Ok(())
    }

    fn update_collection(&mut self, edit: impl FnOnce(&mut CollectionFile)) -> Result<()> {
        if let Some(e) = &self.collection_error {
            return Err(WorkspaceError::Invalid(format!(
                "fix {COLLECTION_FILE} first: {e}"
            )));
        }
        if self.collection_read_only {
            return Err(WorkspaceError::ReadOnly(COLLECTION_FILE.into()));
        }
        let mut file = self.load_collection()?.file;
        edit(&mut file);
        let path = self.root.join(COLLECTION_FILE);
        let text = to_yaml(&file)?;
        if std::fs::read(&path).ok().as_deref() != Some(text.as_bytes()) {
            atomic::write_own(&self.own, &path, text.as_bytes())?;
        }
        self.collection = file;
        Ok(())
    }

    /// Renames a file or directory, recording it as our own change.
    fn move_path(&mut self, from: &Path, to: &Path, kind: NodeKind) -> Result<()> {
        if kind == NodeKind::Folder {
            self.own.record_tree(from);
            self.own.record_tree(to);
        } else {
            self.own.record_removed(from);
            if let Ok(bytes) = std::fs::read(from) {
                self.own.record_write(to, &bytes);
            }
        }
        atomic::rename_with_retry(from, to)?;
        // Session IDs of folders under the old path follow the move.
        let (old_rel, new_rel) = (self.rel_path(from), self.rel_path(to));
        let moved: Vec<(PathBuf, ItemId)> = self
            .session_ids
            .iter()
            .filter(|(p, _)| p.starts_with(&old_rel))
            .map(|(p, id)| (p.clone(), *id))
            .collect();
        for (path, id) in moved {
            self.session_ids.remove(&path);
            let suffix = path.strip_prefix(&old_rel).expect("filtered by prefix");
            self.session_ids.insert(new_rel.join(suffix), id);
        }
        Ok(())
    }

    fn new_folder_file(&self, id: ItemId, dir: &Path) -> FolderFile {
        FolderFile {
            schema: schema::FOLDER.to_string(),
            id,
            name: dir.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            description: None,
            vars: Vars::default(),
            headers: Vec::new(),
            auth: Auth::default(),
            order: Vec::new(),
            extra: Extra::default(),
        }
    }

    fn created(&self, id: ItemId, warning: Option<String>) -> Result<Created> {
        let node = self.tree.find(id).cloned().ok_or(WorkspaceError::NotFound(id))?;
        Ok(Created { node, warning })
    }

    fn rel_path(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.root).unwrap_or(path).to_path_buf()
    }

    fn rel(&self, path: &Path) -> String {
        slash_path(&self.rel_path(path))
    }

    fn yaml_error(&self, path: &Path, message: String) -> WorkspaceError {
        WorkspaceError::Invalid(format!("{}: {message}", self.rel(path)))
    }
}

/// A scanned file or folder, before IDs and order are final.
/// When a file became what it is: the later of its creation and modification times.
///
/// Deciding which of two files with the same ID is the copy can't use the modification
/// time alone: on Windows, copying a file (Explorer, `CopyFileEx`) keeps the source's
/// modification time and gives the copy a new *creation* time. On Linux, `cp` gives the
/// copy a new modification time. The later of the two covers both.
pub(crate) fn copy_age(path: &Path) -> Option<SystemTime> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok();
    match (meta.created().ok(), modified) {
        (Some(c), Some(m)) => Some(c.max(m)),
        (c, m) => c.or(m),
    }
}

/// Tie-break between files of the same age: copies get longer names (`dev - Copy.yaml`,
/// `dev (2).yaml`, `dev-copy.yaml`), so the shorter name is taken as the original.
pub(crate) fn name_len(path: &Path) -> usize {
    path.file_name().map_or(0, |n| n.len())
}

#[derive(Debug)]
struct RawItem {
    kind: NodeKind,
    rel: PathBuf,
    abs: PathBuf,
    id: Option<ItemId>,
    name: String,
    method: Option<String>,
    url: Option<String>,
    error: Option<String>,
    read_only: bool,
    meta_error: bool,
    hash: Option<String>,
    mtime: Option<SystemTime>,
    order: Vec<ItemId>,
    children: Vec<usize>,
}

impl RawItem {
    fn root() -> Self {
        Self::new(
            NodeKind::Collection,
            PathBuf::new(),
            PathBuf::new(),
            String::new(),
        )
    }

    fn new(kind: NodeKind, rel: PathBuf, abs: PathBuf, name: String) -> Self {
        Self {
            kind,
            rel,
            abs,
            id: None,
            name,
            method: None,
            url: None,
            error: None,
            read_only: false,
            meta_error: false,
            hash: None,
            mtime: None,
            order: Vec::new(),
            children: Vec::new(),
        }
    }
}

/// Copies `from` to `to`, giving every request and folder a new ID (and remapping
/// `order` lists). The top folder is named `name`. Returns the copy's ID.
fn copy_folder_with_new_ids(from: &Path, to: &Path, name: &str, own: &OwnWrites) -> Result<ItemId> {
    // First pass: map every ID in the subtree to a new one.
    let mut ids: HashMap<ItemId, ItemId> = HashMap::new();
    collect_ids(from, &mut ids)?;
    copy_dir(from, to, &ids, own)?;

    let meta = to.join(FOLDER_FILE);
    let mut folder = match std::fs::read(&meta) {
        Ok(bytes) => parse::<FolderFile>(&bytes).map_err(WorkspaceError::Invalid)?,
        Err(_) => FolderFile {
            schema: schema::FOLDER.to_string(),
            id: ItemId::new(),
            name: String::new(),
            description: None,
            vars: Vars::default(),
            headers: Vec::new(),
            auth: Auth::default(),
            order: Vec::new(),
            extra: Extra::default(),
        },
    };
    folder.name = name.to_string();
    atomic::write_own(own, &meta, to_yaml(&folder)?.as_bytes())?;
    Ok(folder.id)
}

fn collect_ids(dir: &Path, ids: &mut HashMap<ItemId, ItemId>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_ids(&path, ids)?;
        } else if name == FOLDER_FILE {
            if let Ok(f) = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| parse::<FolderFile>(&b))
            {
                ids.insert(f.id, ItemId::new());
            }
        } else if name.ends_with(REQUEST_EXT)
            && let Ok(r) = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| parse::<RequestFile>(&b))
        {
            ids.insert(r.id, ItemId::new());
        }
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path, ids: &HashMap<ItemId, ItemId>, own: &OwnWrites) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let name = entry.file_name().to_string_lossy().into_owned();
        // Symlinks aren't part of the tree (see `walk`), so they aren't copied either;
        // following one would copy files from outside the workspace into it.
        if name.starts_with('.') || entry.file_type().is_ok_and(|t| t.is_symlink()) {
            continue;
        }
        if src.is_dir() {
            copy_dir(&src, &dst, ids, own)?;
            continue;
        }
        let bytes = std::fs::read(&src)?;
        let remapped = if name == FOLDER_FILE {
            parse::<FolderFile>(&bytes).ok().map(|mut f| {
                f.id = ids.get(&f.id).copied().unwrap_or_else(ItemId::new);
                f.order = f.order.iter().filter_map(|id| ids.get(id).copied()).collect();
                to_yaml(&f)
            })
        } else if name.ends_with(REQUEST_EXT) {
            parse::<RequestFile>(&bytes).ok().map(|mut r| {
                r.id = ids.get(&r.id).copied().unwrap_or_else(ItemId::new);
                to_yaml(&r)
            })
        } else {
            None
        };
        match remapped {
            Some(text) => atomic::write_own(own, &dst, text?.as_bytes())?,
            // Unparseable and non-request files are copied as they are.
            None => atomic::write_own(own, &dst, &bytes)?,
        }
    }
    Ok(())
}

/// Secret values live only in the secret store (PLAN.md §6). Refuses any `vars` map
/// that would write one to disk.
pub(crate) fn ensure_no_secret_values(vars: &Vars) -> Result<()> {
    for (name, var) in vars {
        if let ironcall_core::model::Variable::Detailed(spec) = var
            && spec.secret
            && spec.value.is_some()
        {
            return Err(WorkspaceError::Invalid(format!(
                "secret variable `{name}` has a value; secret values are kept in the secret store, never in files"
            )));
        }
    }
    Ok(())
}

fn clean_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(WorkspaceError::Invalid("enter a name".into()));
    }
    Ok(name.to_string())
}

fn parse<T: DeserializeOwned>(bytes: &[u8]) -> std::result::Result<T, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the file is not valid UTF-8 text".to_string())?;
    yaml::from_str(text).map_err(|e| e.to_string())
}

fn to_yaml<T: Serialize + DeserializeOwned + PartialEq>(value: &T) -> Result<String> {
    yaml::to_string(value).map_err(|source| WorkspaceError::Yaml {
        path: String::new(),
        source,
    })
}

/// A relative path with `/` separators on every platform.
fn slash_path(path: &Path) -> String {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}
