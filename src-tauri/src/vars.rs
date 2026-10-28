//! Variables, environments and secrets (PLAN.md §5, §6, Phase 3): the IPC contract.
//!
//! Scopes, lowest to highest precedence: Global < Collection < Environment < Folder <
//! Request < Runtime (PLAN.md §5). Secret variables are declared in YAML (`secret: true`)
//! but their values live only in the workspace's chosen [`SecretBackend`]. Values of
//! secret variables never cross IPC unless the UI explicitly asks (`reveal`).
//!
//! Scratch requests resolve against globals plus, if a workspace is open, its collection
//! variables and active environment (they have no folder chain).

use std::sync::{Arc, MutexGuard};

use ironcall_core::ItemId;
use ironcall_core::model::{EnvironmentFile, RequestFile, Variable, VariableSpec, Vars};
use ironcall_core::vars::{self as core_vars, Layer, Resolver, Source};
use ironcall_secrets::{ExposeSecret, SecretKey, SecretStore, SecretString, scope};
use ironcall_store::workspace::{self as store, SecretScopeRef, Workspace};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::AppState;
use crate::secrets::backend_from_setting;

/// Where a variable's value comes from.
#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(tag = "scope", rename_all = "camelCase")]
pub enum VarSource {
    Global,
    Collection,
    Environment {
        id: ItemId,
        name: String,
    },
    Folder {
        id: ItemId,
        name: String,
    },
    Request,
    /// Set by `extract` during a session or run.
    Runtime,
    /// A built-in helper such as `{{$uuid}}`.
    Dynamic,
}

/// One variable as seen from a request: the winning definition plus what it overrides.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct VisibleVar {
    pub name: String,
    /// Fully resolved value (nested `{{refs}}` expanded). `None` if the variable is a
    /// secret that isn't revealed, or a secret with no stored value.
    pub value: Option<String>,
    /// The raw value as written, before nested resolution (masked for secrets).
    pub raw: Option<String>,
    pub secret: bool,
    /// A secret declared but with no value in the secret store.
    pub missing: bool,
    pub source: VarSource,
    /// Lower-precedence definitions this one overrides, highest first.
    pub overrides: Vec<VarSource>,
    /// Set if resolving failed (cycle, depth limit, undefined nested reference).
    pub error: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DynamicHelper {
    /// Without braces, e.g. `$uuid`.
    pub name: String,
    pub description: String,
    /// An example value.
    pub example: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct VariableContext {
    /// Every variable visible to the request, sorted by name.
    pub variables: Vec<VisibleVar>,
    pub helpers: Vec<DynamicHelper>,
    /// `{{names}}` used by the request that don't resolve.
    pub unresolved: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentSummary {
    pub id: ItemId,
    pub name: String,
    /// Path relative to the workspace root, `/`-separated.
    pub path: String,
    /// Set if the file can't be read; such environments can't be activated or edited.
    pub error: Option<String>,
    /// Number of secret variables with no stored value.
    pub missing_secrets: u32,
    /// Names of those secret variables.
    pub missing_secret_names: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoadedEnvironment {
    pub environment: EnvironmentFile,
    pub hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LoadedGlobals {
    pub vars: Vars,
    pub hash: Option<String>,
}

/// Where a secret belongs. Secrets are keyed by (workspace, scope, name) on this machine
/// (PLAN.md §4.5); globals are keyed by scope alone.
#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq, Eq, Hash)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SecretScope {
    Global,
    Collection,
    Environment { id: ItemId },
    Folder { id: ItemId },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SecretBackend {
    /// OS keychain: Windows Credential Manager, Linux Secret Service.
    Keychain,
    /// `.ironcall/secrets.enc`, Argon2id + XChaCha20-Poly1305, unlocked with a passphrase.
    EncryptedFile,
    /// `.ironcall/secrets.json`, unencrypted (the UI warns).
    PlaintextFile,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SecretBackendStatus {
    /// Backend chosen for the open workspace (globals always use the app-level choice).
    pub current: SecretBackend,
    /// False if no Secret Service / Credential Manager is reachable (PLAN.md §6.1).
    pub keychain_available: bool,
    pub encrypted_file_exists: bool,
    /// For `EncryptedFile`: whether it is unlocked for this session.
    pub unlocked: bool,
    /// A backend other than `current` named by the workspace's own
    /// `.ironcall/local.yaml` (written by IronCall before 1.0, or committed to the repo).
    /// It is not used, because a repo could pick the plaintext file for its users; the
    /// UI says so, and switching to it is the user's choice.
    pub folder_backend: Option<SecretBackend>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MissingSecret {
    pub scope: SecretScope,
    pub name: String,
    /// Human-readable scope, e.g. "Environment › Dev".
    pub scope_label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct VarsError {
    pub message: String,
    /// True if the encrypted secrets file must be unlocked first.
    pub locked: bool,
    /// True if a save was refused because the file changed on disk since `baseHash`.
    pub conflict: bool,
}

impl VarsError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            locked: false,
            conflict: false,
        }
    }

    fn conflict() -> Self {
        Self {
            conflict: true,
            ..Self::new(CONFLICT)
        }
    }
}

type Result<T> = std::result::Result<T, VarsError>;

impl From<ironcall_secrets::Error> for VarsError {
    fn from(e: ironcall_secrets::Error) -> Self {
        Self {
            locked: matches!(e, ironcall_secrets::Error::Locked),
            ..Self::new(e.to_string())
        }
    }
}

impl From<store::WorkspaceError> for VarsError {
    fn from(e: store::WorkspaceError) -> Self {
        Self::new(e.to_string())
    }
}

// ---------------------------------------------------------------------------------
// Helpers

fn lock(ws: &std::sync::Mutex<Workspace>) -> MutexGuard<'_, Workspace> {
    ws.lock().expect("workspace lock poisoned")
}

fn current(state: &AppState) -> Result<Arc<std::sync::Mutex<Workspace>>> {
    state
        .workspaces
        .current()
        .map_err(|_| VarsError::new("no workspace is open"))
}

fn backend_of(state: &AppState, ws: &Workspace) -> SecretBackend {
    state.secrets.backend_for(ws.root())
}

fn workspace_store(state: &AppState, ws: &Workspace) -> Result<Arc<dyn SecretStore>> {
    state.secrets.workspace_store(backend_of(state, ws), ws.root())
}

fn key_for(ws: Option<&Workspace>, scope: &SecretScope, name: &str) -> Result<SecretKey> {
    let ws_id = || {
        ws.map(|w| w.id())
            .ok_or_else(|| VarsError::new("no workspace is open"))
    };
    Ok(match scope {
        SecretScope::Global => SecretKey::global(name),
        SecretScope::Collection => SecretKey::new(Some(ws_id()?), scope::COLLECTION, name),
        SecretScope::Environment { id } => SecretKey::new(Some(ws_id()?), scope::environment(*id), name),
        SecretScope::Folder { id } => SecretKey::new(Some(ws_id()?), scope::folder(*id), name),
    })
}

fn scope_ref_to_ipc(r: &SecretScopeRef) -> SecretScope {
    match r {
        SecretScopeRef::Collection => SecretScope::Collection,
        SecretScopeRef::Environment(id) => SecretScope::Environment { id: *id },
        SecretScopeRef::Folder(id) => SecretScope::Folder { id: *id },
    }
}

fn source_to_ipc(s: &Source) -> VarSource {
    match s {
        Source::Global => VarSource::Global,
        Source::Collection => VarSource::Collection,
        Source::Environment { id, name } => VarSource::Environment {
            id: *id,
            name: name.clone(),
        },
        Source::Folder { id, name } => VarSource::Folder {
            id: *id,
            name: name.clone(),
        },
        Source::Request => VarSource::Request,
        Source::Runtime => VarSource::Runtime,
        Source::Dynamic => VarSource::Dynamic,
    }
}

/// Builds a lookup closure for one scope's secrets. Lookup failures (locked store,
/// unavailable keychain) count as "no value" and set `locked` when it's a lock.
fn lookup<'a>(
    store: Option<&'a Arc<dyn SecretStore>>,
    key: impl Fn(&str) -> SecretKey + 'a,
    locked: &'a std::cell::Cell<bool>,
) -> impl FnMut(&str) -> Option<SecretString> + 'a {
    move |name| match store.map(|s| s.get(&key(name))) {
        Some(Ok(v)) => v,
        Some(Err(ironcall_secrets::Error::Locked)) => {
            locked.set(true);
            None
        }
        _ => None,
    }
}

/// The active environment's ID and name, if one is set and readable.
fn active_environment(ws: &Workspace) -> Option<(ItemId, String)> {
    let id = ws.load_local().active_environment?;
    ws.environments()
        .into_iter()
        .find(|e| e.id == id && e.error.is_none())
        .map(|e| (e.id, e.name))
}

/// Resolver layers for `request` (PLAN.md §5). Returns the resolver and whether a
/// locked secret store hid values.
pub(crate) fn resolver_for(state: &AppState, request: &RequestFile) -> (Resolver, bool) {
    resolver_in(state, request, &Scope::Session)
}

/// Which environment a run resolves against.
#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EnvChoice {
    /// The workspace's active environment.
    Active,
    /// No environment.
    None,
    Id {
        id: ItemId,
    },
}

/// Where the environment and runtime variables come from.
pub(crate) enum Scope {
    /// Normal sends: the active environment and the session's runtime variables.
    Session,
    /// A collection run: its chosen environment and its own runtime variables.
    Run {
        environment: EnvChoice,
        runtime: ironcall_core::runtime::RuntimeVars,
    },
}

/// Like [`resolver_for`], for a given scope.
pub(crate) fn resolver_in(state: &AppState, request: &RequestFile, scope: &Scope) -> (Resolver, bool) {
    let locked = std::cell::Cell::new(false);
    let mut layers = Vec::new();

    let globals = state.globals.load().map(|(v, _)| v).unwrap_or_default();
    let global_store = state.secrets.global_store().ok();
    layers.push(Layer::from_vars(
        Source::Global,
        &globals,
        lookup(global_store.as_ref(), |n: &str| SecretKey::global(n), &locked),
    ));

    if let Ok(ws) = state.workspaces.current() {
        let ws = lock(&ws);
        let ws_id = ws.id();
        let store = workspace_store(state, &ws).ok();
        layers.push(Layer::from_vars(
            Source::Collection,
            ws.collection_vars(),
            lookup(
                store.as_ref(),
                |n| SecretKey::new(Some(ws_id), scope::COLLECTION, n),
                &locked,
            ),
        ));
        let environment = match scope {
            Scope::Session
            | Scope::Run {
                environment: EnvChoice::Active,
                ..
            } => active_environment(&ws),
            Scope::Run {
                environment: EnvChoice::None,
                ..
            } => None,
            Scope::Run {
                environment: EnvChoice::Id { id },
                ..
            } => ws
                .environments()
                .into_iter()
                .find(|e| e.id == *id && e.error.is_none())
                .map(|e| (e.id, e.name)),
        };
        if let Some((env_id, env_name)) = environment
            && let Ok(vars) = ws.environment_vars(env_id)
        {
            layers.push(Layer::from_vars(
                Source::Environment {
                    id: env_id,
                    name: env_name,
                },
                &vars,
                lookup(
                    store.as_ref(),
                    |n| SecretKey::new(Some(ws_id), scope::environment(env_id), n),
                    &locked,
                ),
            ));
        }
        // Scratch requests aren't in the tree, so they have no folder chain.
        if ws.tree().find(request.id).is_some()
            && let Ok(chain) = ws.folder_chain(request.id)
        {
            for folder in chain {
                let id = folder.id;
                layers.push(Layer::from_vars(
                    Source::Folder {
                        id,
                        name: folder.name.clone(),
                    },
                    &folder.vars,
                    lookup(
                        store.as_ref(),
                        |n| SecretKey::new(Some(ws_id), scope::folder(id), n),
                        &locked,
                    ),
                ));
            }
        }
    }

    // Request variables can't be secret (they live in the request file).
    layers.push(Layer::from_vars(Source::Request, &request.vars, |_| None));
    layers.push(match scope {
        Scope::Session => state.runs.session_runtime(state).layer(),
        Scope::Run { runtime, .. } => runtime.layer(),
    });
    (Resolver::new(layers), locked.get())
}

fn missing_names(
    store: Option<&Arc<dyn SecretStore>>,
    vars: &Vars,
    key: impl Fn(&str) -> SecretKey,
) -> Vec<String> {
    vars.iter()
        .filter(|(_, v)| v.is_secret())
        .filter(|(name, _)| !matches!(store.map(|s| s.get(&key(name))), Some(Ok(Some(_)))))
        .map(|(name, _)| name.clone())
        .collect()
}

fn summary(
    state: &AppState,
    ws: &Workspace,
    store: Option<&Arc<dyn SecretStore>>,
    e: store::EnvironmentSummary,
) -> EnvironmentSummary {
    let _ = state;
    let ws_id = ws.id();
    let missing_secret_names = match (&e.error, ws.environment_vars(e.id)) {
        (None, Ok(vars)) => missing_names(store, &vars, |n| {
            SecretKey::new(Some(ws_id), scope::environment(e.id), n)
        }),
        _ => Vec::new(),
    };
    EnvironmentSummary {
        id: e.id,
        name: e.name,
        path: e.path,
        error: e.error,
        missing_secrets: missing_secret_names.len() as u32,
        missing_secret_names,
    }
}

fn secret_names(vars: &Vars) -> Vec<String> {
    vars.iter()
        .filter(|(_, v)| v.is_secret())
        .map(|(n, _)| n.clone())
        .collect()
}

// ---------------------------------------------------------------------------------
// Resolution

/// Variables visible to `request` (a workspace request or scratch), for autocomplete,
/// hover tooltips, unresolved highlighting and the environment quick-look.
#[tauri::command]
#[specta::specta]
pub async fn variables_for(
    state: tauri::State<'_, AppState>,
    request: RequestFile,
    reveal: bool,
) -> Result<VariableContext> {
    let (resolver, _) = resolver_for(&state, &request);
    let unresolved = core_vars::resolve_request(&request, &resolver).unresolved;
    let variables = resolver
        .visible(reveal)
        .into_iter()
        .map(|v| VisibleVar {
            name: v.name,
            value: v.value,
            raw: v.raw,
            secret: v.secret,
            missing: v.missing,
            source: source_to_ipc(&v.source),
            overrides: v.overrides.iter().map(source_to_ipc).collect(),
            error: v.error,
            description: v.description,
        })
        .collect();
    let helpers = core_vars::helpers()
        .into_iter()
        .map(|h| DynamicHelper {
            name: h.name.to_string(),
            description: h.description.to_string(),
            example: h.example,
        })
        .collect();
    Ok(VariableContext {
        variables,
        helpers,
        unresolved,
    })
}

// ---------------------------------------------------------------------------------
// Environments (in the open workspace)

#[tauri::command]
#[specta::specta]
pub async fn environments_list(state: tauri::State<'_, AppState>) -> Result<Vec<EnvironmentSummary>> {
    let ws = current(&state)?;
    let ws = lock(&ws);
    let store = workspace_store(&state, &ws).ok();
    Ok(ws
        .environments()
        .into_iter()
        .map(|e| summary(&state, &ws, store.as_ref(), e))
        .collect())
}

#[tauri::command]
#[specta::specta]
pub async fn environment_create(
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<EnvironmentSummary> {
    let ws = current(&state)?;
    let mut ws = lock(&ws);
    let created = ws.create_environment(&name)?;
    let store = workspace_store(&state, &ws).ok();
    Ok(summary(&state, &ws, store.as_ref(), created))
}

/// Copies an environment, including its stored secret values.
#[tauri::command]
#[specta::specta]
pub async fn environment_duplicate(
    state: tauri::State<'_, AppState>,
    id: ItemId,
) -> Result<EnvironmentSummary> {
    let ws = current(&state)?;
    let mut ws = lock(&ws);
    let names = secret_names(&ws.environment_vars(id)?);
    let created = ws.duplicate_environment(id)?;
    let store = workspace_store(&state, &ws).ok();
    if let Some(store) = &store {
        let ws_id = Some(ws.id());
        for name in names {
            let from = SecretKey::new(ws_id, scope::environment(id), &name);
            if let Ok(Some(value)) = store.get(&from) {
                store.set(
                    &SecretKey::new(ws_id, scope::environment(created.id), &name),
                    &value,
                )?;
            }
        }
    }
    Ok(summary(&state, &ws, store.as_ref(), created))
}

/// Display name only; the file name stays (PLAN.md §4.3).
#[tauri::command]
#[specta::specta]
pub async fn environment_rename(state: tauri::State<'_, AppState>, id: ItemId, name: String) -> Result<()> {
    let ws = current(&state)?;
    Ok(lock(&ws).rename_environment(id, &name)?)
}

/// Moves the file to the OS trash and deletes its stored secrets.
#[tauri::command]
#[specta::specta]
pub async fn environment_delete(state: tauri::State<'_, AppState>, id: ItemId) -> Result<()> {
    let ws = current(&state)?;
    let mut ws = lock(&ws);
    let names = ws
        .environment_vars(id)
        .map(|v| secret_names(&v))
        .unwrap_or_default();
    ws.delete_environment(id)?;
    if let Ok(store) = workspace_store(&state, &ws) {
        for name in names {
            let _ = store.delete(&SecretKey::new(Some(ws.id()), scope::environment(id), &name));
        }
    }
    let mut local = ws.load_local();
    if local.active_environment == Some(id) {
        local.active_environment = None;
        ws.save_local(&local)?;
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn environment_load(state: tauri::State<'_, AppState>, id: ItemId) -> Result<LoadedEnvironment> {
    let ws = current(&state)?;
    let loaded = lock(&ws).load_environment(id)?;
    Ok(LoadedEnvironment {
        environment: loaded.file,
        hash: loaded.hash.unwrap_or_default(),
    })
}

const CONFLICT: &str = "this file changed on disk since it was opened; reload it and try again";

/// Saves an environment file. Secret variables must have no `value` (use `secret_set`).
/// Returns the new hash; errors if the file changed since `baseHash`.
#[tauri::command]
#[specta::specta]
pub async fn environment_save(
    state: tauri::State<'_, AppState>,
    environment: EnvironmentFile,
    base_hash: Option<String>,
) -> Result<String> {
    let ws = current(&state)?;
    match lock(&ws).save_environment(&environment, base_hash.as_deref(), false)? {
        store::SaveOutcome::Saved { hash } => Ok(hash),
        store::SaveOutcome::Conflict { .. } => Err(VarsError::conflict()),
    }
}

/// The active environment for the open workspace (stored in `.ironcall/local.yaml`).
#[tauri::command]
#[specta::specta]
pub fn active_environment_get(state: tauri::State<'_, AppState>) -> Option<ItemId> {
    let ws = state.workspaces.current().ok()?;
    let ws = lock(&ws);
    active_environment(&ws).map(|(id, _)| id)
}

#[tauri::command]
#[specta::specta]
pub async fn active_environment_set(state: tauri::State<'_, AppState>, id: Option<ItemId>) -> Result<()> {
    let ws = current(&state)?;
    let ws = lock(&ws);
    if let Some(id) = id
        && !ws.environments().iter().any(|e| e.id == id && e.error.is_none())
    {
        return Err(VarsError::new("that environment doesn't exist or can't be read"));
    }
    let mut local = ws.load_local();
    local.active_environment = id;
    Ok(ws.save_local(&local)?)
}

/// Saves a runtime variable (from `extract`) into the active environment. Sensitive
/// values, and variables the environment already declares secret, go to the secret
/// store with a `secret: true` declaration; others are written as plain values.
#[tauri::command]
#[specta::specta]
pub async fn runtime_var_save(state: tauri::State<'_, AppState>, name: String) -> Result<()> {
    let runtime = state.runs.session_runtime(&state);
    let (value, sensitive) = runtime
        .entry(&name)
        .map(|(v, s)| (v.clone(), s))
        .ok_or_else(|| VarsError::new(format!("there's no runtime variable `{name}`")))?;
    let ws = current(&state)?;
    let mut ws = lock(&ws);
    let (env_id, _) =
        active_environment(&ws).ok_or_else(|| VarsError::new("choose an environment to save to first"))?;
    let loaded = ws.load_environment(env_id)?;
    let mut file = loaded.file;
    let secret = sensitive || file.vars.get(&name).is_some_and(|v| v.is_secret());
    let entry = file.vars.entry(name.clone());
    if secret {
        let description = match entry {
            indexmap::map::Entry::Occupied(ref e) => match e.get() {
                Variable::Detailed(spec) => spec.description.clone(),
                Variable::Plain(_) => None,
            },
            indexmap::map::Entry::Vacant(_) => None,
        };
        entry.insert_entry(Variable::Detailed(VariableSpec {
            secret: true,
            description,
            ..VariableSpec::default()
        }));
    } else {
        match entry {
            indexmap::map::Entry::Occupied(mut e) => match e.get_mut() {
                Variable::Detailed(spec) => spec.value = Some(value.expose_secret().to_string()),
                plain => *plain = Variable::Plain(value.expose_secret().to_string()),
            },
            indexmap::map::Entry::Vacant(e) => {
                e.insert(Variable::Plain(value.expose_secret().to_string()));
            }
        }
    }
    match ws.save_environment(&file, loaded.hash.as_deref(), false)? {
        store::SaveOutcome::Saved { .. } => {}
        store::SaveOutcome::Conflict { .. } => return Err(VarsError::conflict()),
    }
    if secret {
        let scope = SecretScope::Environment { id: env_id };
        let key = key_for(Some(&ws), &scope, &name)?;
        workspace_store(&state, &ws)?.set(&key, &value)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------
// Global variables (app data dir)

#[tauri::command]
#[specta::specta]
pub async fn globals_load(state: tauri::State<'_, AppState>) -> Result<LoadedGlobals> {
    let (vars, hash) = state.globals.load()?;
    Ok(LoadedGlobals { vars, hash })
}

#[tauri::command]
#[specta::specta]
pub async fn globals_save(
    state: tauri::State<'_, AppState>,
    vars: Vars,
    base_hash: Option<String>,
) -> Result<String> {
    match state.globals.save(&vars, base_hash.as_deref())? {
        store::SaveOutcome::Saved { hash } => Ok(hash),
        store::SaveOutcome::Conflict { .. } => Err(VarsError::conflict()),
    }
}

// ---------------------------------------------------------------------------------
// Secrets

fn status(state: &AppState, ws: Option<&Workspace>) -> SecretBackendStatus {
    let keychain_available = state.secrets.keychain_available();
    match ws {
        Some(ws) => {
            let encrypted = state.secrets.encrypted(ws.root());
            let current = backend_of(state, ws);
            let folder_backend = ws
                .load_local()
                .secret_backend
                .map(|s| backend_from_setting(Some(&s)))
                .filter(|b| *b != current && *b != SecretBackend::Keychain);
            SecretBackendStatus {
                current,
                keychain_available,
                encrypted_file_exists: encrypted.exists(),
                unlocked: encrypted.is_unlocked(),
                folder_backend,
            }
        }
        None => SecretBackendStatus {
            current: SecretBackend::Keychain,
            keychain_available,
            encrypted_file_exists: false,
            unlocked: false,
            folder_backend: None,
        },
    }
}

#[tauri::command]
#[specta::specta]
pub async fn secret_backend_status(state: tauri::State<'_, AppState>) -> Result<SecretBackendStatus> {
    let ws = state.workspaces.current().ok();
    let guard = ws.as_ref().map(|w| lock(w));
    Ok(status(&state, guard.as_deref()))
}

/// Switches the open workspace to `backend`, migrating every declared secret's value
/// from the current backend. `passphrase` is required to create or unlock the
/// encrypted file.
#[tauri::command]
#[specta::specta]
pub async fn secret_backend_set(
    state: tauri::State<'_, AppState>,
    backend: SecretBackend,
    passphrase: Option<String>,
) -> Result<SecretBackendStatus> {
    let ws = current(&state)?;
    let ws = lock(&ws);
    let from_backend = backend_of(&state, &ws);

    if backend == SecretBackend::EncryptedFile {
        let encrypted = state.secrets.encrypted(ws.root());
        if !encrypted.is_unlocked() {
            let pass = passphrase
                .map(SecretString::from)
                .ok_or_else(|| VarsError::new("enter a passphrase for the encrypted secrets file"))?;
            if encrypted.exists() {
                encrypted.unlock(&pass)?;
            } else {
                encrypted.create(&pass)?;
            }
        }
    }
    let to = state.secrets.workspace_store(backend, ws.root())?;

    if from_backend != backend {
        // If the old backend can't be read (e.g. the keychain is gone), there is nothing
        // to migrate; the secrets will show as missing and can be entered again.
        if let Ok(from) = state.secrets.workspace_store(from_backend, ws.root()) {
            let keys: Vec<SecretKey> = store::declared_secrets(&ws)
                .iter()
                .map(|(scope, name)| key_for(Some(&ws), &scope_ref_to_ipc(scope), name))
                .collect::<Result<_>>()?;
            let report = ironcall_secrets::migrate(from.as_ref(), to.as_ref(), &keys)?;
            tracing::info!(
                copied = report.copied,
                skipped = report.skipped,
                "migrated secrets to new backend"
            );
        }
        state.secrets.set_backend_for(ws.root(), backend)?;
    }
    // The choice is now made on this machine; drop any old one from the folder.
    let mut local = ws.load_local();
    if local.secret_backend.take().is_some() {
        ws.save_local(&local)?;
    }
    Ok(status(&state, Some(&ws)))
}

/// Unlocks the encrypted secrets file for this session.
#[tauri::command]
#[specta::specta]
pub async fn secrets_unlock(state: tauri::State<'_, AppState>, passphrase: String) -> Result<()> {
    let ws = current(&state)?;
    let root = lock(&ws).root().to_path_buf();
    Ok(state
        .secrets
        .encrypted(&root)
        .unlock(&SecretString::from(passphrase))?)
}

/// Checks that `name` is declared `secret: true` in `scope`.
fn ensure_declared(state: &AppState, ws: Option<&Workspace>, scope: &SecretScope, name: &str) -> Result<()> {
    let declared = match scope {
        SecretScope::Global => state
            .globals
            .load()
            .map(|(vars, _)| vars.get(name).is_some_and(|v| v.is_secret()))
            .unwrap_or(false),
        _ => ws.is_some_and(|ws| {
            store::declared_secrets(ws)
                .iter()
                .any(|(s, n)| n == name && &scope_ref_to_ipc(s) == scope)
        }),
    };
    if declared {
        Ok(())
    } else {
        Err(VarsError::new(format!(
            "`{name}` isn't declared as a secret variable in that scope; mark it secret first"
        )))
    }
}

fn store_for_scope(
    state: &AppState,
    ws: Option<&Workspace>,
    scope: &SecretScope,
) -> Result<Arc<dyn SecretStore>> {
    match (scope, ws) {
        (SecretScope::Global, _) => state.secrets.global_store(),
        (_, Some(ws)) => workspace_store(state, ws),
        (_, None) => Err(VarsError::new("no workspace is open")),
    }
}

/// Stores a secret's value. The variable must be declared `secret: true` in its scope.
#[tauri::command]
#[specta::specta]
pub async fn secret_set(
    state: tauri::State<'_, AppState>,
    scope: SecretScope,
    name: String,
    value: String,
) -> Result<()> {
    let ws = state.workspaces.current().ok();
    let guard = ws.as_ref().map(|w| lock(w));
    let ws = guard.as_deref();
    ensure_declared(&state, ws, &scope, &name)?;
    let key = key_for(ws, &scope, &name)?;
    Ok(store_for_scope(&state, ws, &scope)?.set(&key, &SecretString::from(value))?)
}

#[tauri::command]
#[specta::specta]
pub async fn secret_clear(state: tauri::State<'_, AppState>, scope: SecretScope, name: String) -> Result<()> {
    let ws = state.workspaces.current().ok();
    let guard = ws.as_ref().map(|w| lock(w));
    let ws = guard.as_deref();
    let key = key_for(ws, &scope, &name)?;
    Ok(store_for_scope(&state, ws, &scope)?.delete(&key)?)
}

/// Returns one secret's value, for an explicit "reveal" in the UI.
#[tauri::command]
#[specta::specta]
pub async fn secret_reveal(
    state: tauri::State<'_, AppState>,
    scope: SecretScope,
    name: String,
) -> Result<Option<String>> {
    let ws = state.workspaces.current().ok();
    let guard = ws.as_ref().map(|w| lock(w));
    let ws = guard.as_deref();
    let key = key_for(ws, &scope, &name)?;
    let value = store_for_scope(&state, ws, &scope)?.get(&key)?;
    Ok(value.map(|v| v.expose_secret().to_string()))
}

/// Declared secrets with no stored value, for the active environment, the collection,
/// folders and globals (the "N secrets missing" banner, PLAN.md §4.5).
#[tauri::command]
#[specta::specta]
pub async fn secrets_missing(state: tauri::State<'_, AppState>) -> Result<Vec<MissingSecret>> {
    let mut missing = Vec::new();
    let has_value = |store: &Result<Arc<dyn SecretStore>>, key: &SecretKey| -> Result<bool> {
        match store {
            Ok(store) => match store.get(key) {
                Ok(v) => Ok(v.is_some()),
                Err(ironcall_secrets::Error::Locked) => Err(VarsError {
                    locked: true,
                    ..VarsError::new("the encrypted secrets file is locked")
                }),
                Err(_) => Ok(false),
            },
            Err(_) => Ok(false),
        }
    };

    if let Ok((globals, _)) = state.globals.load() {
        let store = state.secrets.global_store();
        for name in secret_names(&globals) {
            if !has_value(&store, &SecretKey::global(&name))? {
                missing.push(MissingSecret {
                    scope: SecretScope::Global,
                    name,
                    scope_label: "Globals".into(),
                });
            }
        }
    }

    if let Ok(ws) = state.workspaces.current() {
        let ws = lock(&ws);
        let store = workspace_store(&state, &ws);
        let active = active_environment(&ws);
        for (scope_ref, name) in store::declared_secrets(&ws) {
            let label = match &scope_ref {
                SecretScopeRef::Collection => "Collection".to_string(),
                SecretScopeRef::Environment(id) => match &active {
                    // Only the active environment matters for sending.
                    Some((active_id, env_name)) if active_id == id => format!("Environment › {env_name}"),
                    _ => continue,
                },
                SecretScopeRef::Folder(id) => format!(
                    "Folder › {}",
                    ws.tree().find(*id).map(|n| n.name.as_str()).unwrap_or("?")
                ),
            };
            let scope = scope_ref_to_ipc(&scope_ref);
            if !has_value(&store, &key_for(Some(&ws), &scope, &name)?)? {
                missing.push(MissingSecret {
                    scope,
                    name,
                    scope_label: label,
                });
            }
        }
    }
    Ok(missing)
}

#[cfg(test)]
mod tests {
    use ironcall_core::model::{KeyValue, Variable, VariableSpec};

    use ironcall_core::vars::ResolvedRequest;

    use super::*;
    use crate::test_support::fixture;

    fn resolve_for_send(state: &AppState, request: &RequestFile) -> (ResolvedRequest, bool) {
        let prepared = crate::network::prepare(state, request);
        (prepared.resolved, prepared.locked)
    }

    fn secret_var() -> Variable {
        Variable::Detailed(VariableSpec {
            secret: true,
            ..VariableSpec::default()
        })
    }

    #[test]
    fn environment_switch_changes_resolution_and_secrets_never_reach_files() {
        let f = fixture();
        let state = &f.state;
        let ws = state.workspaces.current().unwrap();

        // Encrypted-file backend, so the scan below also covers `.ironcall/`.
        let encrypted = state.secrets.encrypted(&f.root);
        encrypted.create(&SecretString::from("correct horse")).unwrap();
        state
            .secrets
            .set_backend_for(&f.root, SecretBackend::EncryptedFile)
            .unwrap();

        // Two environments with different base URLs and a secret token.
        let (dev, prod) = {
            let mut ws = lock(&ws);
            let dev = ws.create_environment("Dev").unwrap().id;
            let prod = ws.create_environment("Prod").unwrap().id;
            for (id, url) in [
                (dev, "https://dev.example.com"),
                (prod, "https://prod.example.com"),
            ] {
                let mut env = ws.load_environment(id).unwrap();
                env.file
                    .vars
                    .insert("baseUrl".into(), Variable::Plain(url.into()));
                env.file.vars.insert("token".into(), secret_var());
                ws.save_environment(&env.file, env.hash.as_deref(), false)
                    .unwrap();
            }
            (dev, prod)
        };
        let token = "s3cr3t-token-value";
        {
            let ws = lock(&ws);
            let store = workspace_store(state, &ws).unwrap();
            for id in [dev, prod] {
                let key = SecretKey::new(Some(ws.id()), scope::environment(id), "token");
                store.set(&key, &SecretString::from(token)).unwrap();
            }
        }

        let mut request = RequestFile::new("Get user");
        request.url = "{{baseUrl}}/users".into();
        request.headers = vec![KeyValue::new("Authorization", "Bearer {{token}}")];

        let set_active = |id: ItemId| {
            let ws = lock(&ws);
            let mut local = ws.load_local();
            local.active_environment = Some(id);
            ws.save_local(&local).unwrap();
        };

        set_active(dev);
        let (resolved, locked) = resolve_for_send(state, &request);
        assert!(!locked);
        assert_eq!(resolved.request.url, "https://dev.example.com/users");
        assert_eq!(resolved.request.headers[0].value, format!("Bearer {token}"));
        assert_eq!(resolved.secrets.len(), 1);

        set_active(prod);
        let (resolved, _) = resolve_for_send(state, &request);
        assert_eq!(resolved.request.url, "https://prod.example.com/users");

        // The preview masks the secret unless revealed.
        let masked = state
            .sender
            .preview(
                &resolved.request,
                &ironcall_core::wire::WireContext::default(),
                None,
                &resolved.secrets,
                false,
            )
            .unwrap();
        let auth = masked.headers.iter().find(|h| h.name == "Authorization").unwrap();
        assert_eq!(auth.value, "Bearer ••••");

        // Locked store: secrets resolve as missing and report locked.
        encrypted.lock();
        let (resolved, locked) = resolve_for_send(state, &request);
        assert!(locked);
        assert_eq!(resolved.unresolved, ["token"]);

        // No file in the workspace (including .ironcall/) contains the secret value.
        for entry in walk(&f.root) {
            let bytes = std::fs::read(&entry).unwrap();
            assert!(
                !bytes.windows(token.len()).any(|w| w == token.as_bytes()),
                "secret found in {}",
                entry.display()
            );
        }
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push(path);
            }
        }
        out
    }

    #[test]
    fn a_committed_local_yaml_does_not_pick_the_secret_backend() {
        let f = fixture();
        let state = &f.state;
        let ws = state.workspaces.current().unwrap();
        let ws = lock(&ws);

        // A repo ships `.ironcall/local.yaml` selecting the plaintext file.
        let mut local = ws.load_local();
        local.secret_backend = Some("plaintextFile".into());
        ws.save_local(&local).unwrap();

        let s = status(state, Some(&ws));
        assert_eq!(
            s.current,
            SecretBackend::Keychain,
            "the folder's choice is ignored"
        );
        assert_eq!(
            s.folder_backend,
            Some(SecretBackend::PlaintextFile),
            "and reported"
        );

        // The user's own choice is kept per machine, in the app data folder.
        state
            .secrets
            .set_backend_for(&f.root, SecretBackend::PlaintextFile)
            .unwrap();
        let s = status(state, Some(&ws));
        assert_eq!(s.current, SecretBackend::PlaintextFile);
        assert_eq!(s.folder_backend, None);
        assert_eq!(
            crate::secrets::Secrets::new(&f.data).backend_for(&f.root),
            SecretBackend::PlaintextFile,
            "survives a restart"
        );
        assert_eq!(
            crate::secrets::Secrets::new(&f.data).backend_for(&f.data),
            SecretBackend::Keychain,
            "other folders keep the default"
        );
    }
}
