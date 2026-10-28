//! Auth inheritance, file bodies, proxy, custom CAs and cookies (PLAN.md Phase 4):
//! the IPC contract.
//!
//! **Files referenced by requests** (multipart file parts, binary bodies) are read at send
//! time. Paths inside the workspace are stored relative to its root and always allowed.
//! A path outside the workspace is only sent if the user picked it with
//! [`pick_body_file`] in this session; otherwise the send fails with a clear message.
//! This stops a workspace cloned from someone else from silently uploading local files.
//! Pickers run in Rust for the same reason: the webview can't allow arbitrary paths.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ironcall_core::ItemId;
use ironcall_core::auth::{self as core_auth, FolderAuth};
use ironcall_core::model::{Auth, KeyValue, RequestFile};
use ironcall_core::recent::rfc3339_utc;
use ironcall_core::vars::{self as core_vars, ResolvedRequest};
use ironcall_core::wire::{DefaultHeaders, WireContext};
use ironcall_core::{paths, wire};
use ironcall_http::{CookieInfo, CookieJar, NoProxy, ProxyAuth, ProxyConfig, RootSet};
use ironcall_secrets::{ExposeSecret, SecretKey, SecretString};
use ironcall_store::settings::{self as store_settings, CookieFiles, SettingsStore};
use ironcall_store::workspace::Workspace;
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri_plugin_dialog::DialogExt;

use crate::AppState;

// ---------------------------------------------------------------------------------
// Auth

/// Where a request's effective auth comes from.
#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AuthOrigin {
    /// Set on the request itself.
    Request,
    Folder {
        id: ItemId,
        name: String,
    },
    Collection,
    /// Inherit all the way up with nothing set: no auth.
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveAuth {
    /// The auth that will be applied (never `inherit`), with `{{variables}}` unresolved
    /// and secret parts as written.
    pub auth: Auth,
    pub origin: AuthOrigin,
}

// ---------------------------------------------------------------------------------
// Files for bodies

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PickedFile {
    /// What to store in the request: relative to the workspace root with `/` if inside
    /// it, otherwise absolute.
    pub path: String,
    pub file_name: String,
    #[specta(type = Number)]
    pub size: u64,
    /// Guessed from the extension, e.g. `image/png`.
    pub content_type: Option<String>,
    pub outside_workspace: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BodyFileStatus {
    pub path: String,
    pub exists: bool,
    #[specta(type = Option<Number>)]
    pub size: Option<u64>,
    pub outside_workspace: bool,
    /// False for a file outside the workspace that hasn't been picked this session.
    pub allowed: bool,
}

// ---------------------------------------------------------------------------------
// Network settings (app-wide) and custom CAs

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProxyMode {
    /// Connect directly.
    None,
    /// Use `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` (and lowercase forms) from the environment.
    #[default]
    Environment,
    /// Use `url` below.
    Manual,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProxySettings {
    pub mode: ProxyMode,
    /// `http://host:port` (HTTPS goes through a CONNECT tunnel).
    pub url: Option<String>,
    pub username: Option<String>,
    /// Read-only: whether a proxy password is stored (in the OS keychain).
    pub has_password: bool,
    /// Hosts that bypass the proxy, `NO_PROXY` syntax: `example.com`, `.corp`, `10.0.0.0/8`,
    /// `localhost`, `*`. Added to the environment's `NO_PROXY` in Environment mode.
    pub no_proxy: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CaCertInfo {
    /// Absolute path of a PEM or DER file.
    pub path: String,
    /// Subject of the first certificate, e.g. `CN=Corp Root CA`.
    pub subject: Option<String>,
    /// RFC 3339.
    pub not_after: Option<String>,
    pub count: u32,
    /// Set if the file is missing or not a certificate.
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSettings {
    pub proxy: ProxySettings,
    /// Trusted for every workspace, in addition to the OS store.
    pub ca_certs: Vec<CaCertInfo>,
    /// Effective environment proxy values, for display (credentials masked).
    pub environment_proxy: Option<String>,
}

// ---------------------------------------------------------------------------------
// Cookies (per workspace; scratch requests without a workspace share an app-level jar)

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CookieRow {
    pub domain: String,
    pub path: String,
    pub name: String,
    pub value: String,
    /// RFC 3339; `None` for session cookies.
    pub expires: Option<String>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<SameSite>,
    /// True if the cookie only matches `domain` exactly (no `Domain` attribute).
    pub host_only: bool,
}

// ---------------------------------------------------------------------------------
// State

type RootsKey = Vec<(PathBuf, Option<SystemTime>)>;

/// App-wide network settings, cookie jars and the CA root cache.
pub struct Network {
    pub(crate) settings: SettingsStore,
    cookie_files: CookieFiles,
    jars: Mutex<HashMap<String, Arc<CookieJar>>>,
    /// Last built root set, keyed by (paths, modification times).
    roots: Mutex<Option<(RootsKey, RootSet)>>,
}

impl Network {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            settings: SettingsStore::open(data_dir.join("settings.json")),
            cookie_files: CookieFiles::new(data_dir.join("cookies")),
            jars: Mutex::new(HashMap::new()),
            roots: Mutex::new(None),
        }
    }

    pub(crate) fn history_settings(&self) -> crate::history::HistorySettings {
        let h = self.settings.load().history;
        crate::history::HistorySettings {
            max_entries: h.max_entries,
            max_age_days: h.max_age_days,
            max_total_mb: h.max_total_mb,
            max_body_kb: h.max_body_kb,
        }
    }

    pub(crate) fn save_history_settings(&self, h: &crate::history::HistorySettings) -> Result<(), String> {
        let mut settings = self.settings.load();
        settings.history = store_settings::HistorySettingsStore {
            max_entries: h.max_entries.max(1),
            max_age_days: h.max_age_days.max(1),
            max_total_mb: h.max_total_mb,
            max_body_kb: h.max_body_kb,
        };
        self.settings.save(&settings).map_err(|e| e.to_string())
    }

    /// The cookie jar for `key` (a workspace ID or `no-workspace`), loaded on first use.
    fn jar(&self, key: &str) -> Arc<CookieJar> {
        let mut jars = self.jars.lock().expect("cookie jars lock poisoned");
        jars.entry(key.to_string())
            .or_insert_with(|| {
                let loaded = self.cookie_files.load(key).ok().flatten().and_then(|json| {
                    match CookieJar::from_json(&json) {
                        Ok(jar) => Some(jar),
                        Err(e) => {
                            tracing::warn!(error = %e, "ignoring unreadable cookie jar");
                            None
                        }
                    }
                });
                Arc::new(loaded.unwrap_or_default())
            })
            .clone()
    }

    pub(crate) fn save_jar(&self, key: &str) {
        let jar = self.jar(key);
        if let Err(e) = self.cookie_files.save(key, &jar.to_json()) {
            tracing::warn!(error = %e, "could not save cookies");
        }
    }

    /// The proxy to use, from the settings (and the keychain for the password).
    fn proxy(&self, state: &AppState) -> Result<ProxyConfig, String> {
        let proxy = self.settings.load().network.proxy;
        let extra_no_proxy = proxy.no_proxy.join(",");
        match proxy.mode {
            store_settings::ProxyMode::None => Ok(ProxyConfig::none()),
            store_settings::ProxyMode::Environment => {
                let mut config = ProxyConfig::from_env().map_err(|e| e.to_string())?;
                if !extra_no_proxy.is_empty() {
                    config.no_proxy.add(&extra_no_proxy);
                }
                Ok(config)
            }
            store_settings::ProxyMode::Manual => {
                let url = proxy
                    .url
                    .filter(|u| !u.trim().is_empty())
                    .ok_or("manual proxy mode needs a proxy URL (Network settings)")?;
                let auth = match proxy.username.filter(|u| !u.is_empty()) {
                    Some(user) => {
                        let password = proxy_password(state)?;
                        Some(ProxyAuth::new(
                            user,
                            password
                                .map(|p| p.expose_secret().to_string())
                                .unwrap_or_default(),
                        ))
                    }
                    None => None,
                };
                ProxyConfig::manual(&url, auth, NoProxy::parse(&extra_no_proxy)).map_err(|e| e.to_string())
            }
        }
    }

    /// Global plus workspace CA certificates as one root set (cached while the files
    /// don't change). `None` if there are none.
    fn roots(&self, workspace_cas: &[String]) -> Result<Option<RootSet>, String> {
        let mut paths: Vec<PathBuf> = self
            .settings
            .load()
            .network
            .ca_certs
            .into_iter()
            .map(PathBuf::from)
            .collect();
        paths.extend(workspace_cas.iter().map(PathBuf::from));
        if paths.is_empty() {
            return Ok(None);
        }
        let key: Vec<_> = paths
            .iter()
            .map(|p| (p.clone(), std::fs::metadata(p).and_then(|m| m.modified()).ok()))
            .collect();
        let mut cache = self.roots.lock().expect("roots lock poisoned");
        if let Some((cached_key, roots)) = cache.as_ref()
            && *cached_key == key
        {
            return Ok(Some(roots.clone()));
        }
        let roots = RootSet::from_files(&paths).map_err(|e| format!("custom CA certificate: {e}"))?;
        *cache = Some((key, roots.clone()));
        Ok(Some(roots))
    }
}

fn proxy_password_key() -> SecretKey {
    SecretKey::new(None, "proxy", "password")
}

fn proxy_password(state: &AppState) -> Result<Option<SecretString>, String> {
    let store = state.secrets.global_store().map_err(|e| e.message)?;
    store.get(&proxy_password_key()).map_err(|e| e.to_string())
}

fn lock(ws: &std::sync::Mutex<Workspace>) -> std::sync::MutexGuard<'_, Workspace> {
    ws.lock().expect("workspace lock poisoned")
}

// ---------------------------------------------------------------------------------
// Preparing a request for preview or sending

/// Network options for one send.
pub struct NetOptions {
    pub proxy: ProxyConfig,
    pub roots: Option<RootSet>,
    pub jar: Arc<CookieJar>,
    pub jar_key: String,
    pub bypass_proxy: bool,
    /// User-configured sensitive headers, masked in the timeline.
    pub sensitive_headers: Vec<String>,
}

/// Everything needed to build and send a request.
pub(crate) struct Prepared {
    pub resolved: ResolvedRequest,
    pub ctx: WireContext,
    /// A locked encrypted secrets file hid some values.
    pub locked: bool,
    pub workspace_cas: Vec<String>,
    pub jar_key: String,
    /// The resolver used, for `{{variables}}` in assertions.
    pub resolver: core_vars::Resolver,
    /// User-configured sensitive headers (Settings), for redaction.
    pub sensitive_headers: Vec<String>,
}

/// Resolves variables, inherited auth and default headers (PLAN.md §5, Phase 4).
///
/// Scratch requests resolve collection and environment variables, but don't inherit
/// the collection's auth or default headers: they aren't part of the collection.
pub(crate) fn prepare(state: &AppState, request: &RequestFile) -> Prepared {
    prepare_with(state, request, true)
}

/// Like [`prepare`], but with `resolve = false` nothing is resolved: `{{references}}`
/// (including helpers like `{{$uuid}}`) stay as written in the request, inherited auth
/// and default headers. For templates such as "Copy as cURL" with variables.
pub(crate) fn prepare_with(state: &AppState, request: &RequestFile, resolve: bool) -> Prepared {
    prepare_in(state, request, resolve, &crate::vars::Scope::Session)
}

/// Like [`prepare_with`], with variables from `scope` (a collection run's own).
pub(crate) fn prepare_in(
    state: &AppState,
    request: &RequestFile,
    resolve: bool,
    scope: &crate::vars::Scope,
) -> Prepared {
    let (resolver, locked) = crate::vars::resolver_in(state, request, scope);
    let mut resolved = if resolve {
        core_vars::resolve_request(request, &resolver)
    } else {
        ResolvedRequest {
            request: request.clone(),
            secrets: Vec::new(),
            unresolved: Vec::new(),
        }
    };
    let mut ctx = WireContext {
        defaults: state.network.wire_defaults(),
        ..WireContext::default()
    };
    let mut workspace_cas = Vec::new();
    let mut jar_key = store_settings::NO_WORKSPACE.to_string();

    let mut chain: Vec<(ItemId, String, Auth, Vec<KeyValue>)> = Vec::new();
    let mut collection: Option<(Auth, Vec<KeyValue>)> = None;
    if let Ok(ws) = state.workspaces.current() {
        let ws = lock(&ws);
        ctx.root = Some(ws.root().to_path_buf());
        workspace_cas = ws.load_local().ca_certs;
        jar_key = ws.id().to_string();
        if ws.tree().find(request.id).is_some() {
            if let Ok(folders) = ws.folder_chain(request.id) {
                for f in folders {
                    if let Ok(loaded) = ws.load_folder(f.id) {
                        chain.push((f.id, f.name, loaded.file.auth, loaded.file.headers));
                    }
                }
            }
            if let Ok(c) = ws.load_collection() {
                collection = Some((c.file.auth, c.file.headers));
            }
        }
    }

    let folder_auths: Vec<FolderAuth<'_>> = chain
        .iter()
        .map(|(id, name, auth, _)| FolderAuth { id: *id, name, auth })
        .collect();
    let none = Auth::default();
    let collection_auth = collection.as_ref().map(|(a, _)| a).unwrap_or(&none);
    let (auth, origin) = core_auth::effective_auth(&request.auth, &folder_auths, collection_auth);
    let auth = if resolve {
        core_vars::resolve_auth(&auth, &resolver)
    } else {
        unresolved(auth)
    };
    resolved.absorb(&auth);
    if resolve {
        // Credentials are sensitive whatever header or query name carries them (an API key
        // as `X-Goog-Api-Key` or `?subscription-key=`): mask their values everywhere a
        // secret is masked (timeline, history, masked cURL export).
        resolved.secrets.extend(auth_credentials(&auth.value));
    }
    ctx.auth = auth.value;
    ctx.auth_origin = origin.label();

    if let Some((_, headers)) = &collection {
        let rows = if resolve {
            core_vars::resolve_rows(headers, &resolver)
        } else {
            unresolved(headers.clone())
        };
        resolved.absorb(&rows);
        ctx.default_headers.push(DefaultHeaders {
            origin: "Collection".into(),
            headers: rows.value,
        });
    }
    for (_, name, _, headers) in &chain {
        let rows = if resolve {
            core_vars::resolve_rows(headers, &resolver)
        } else {
            unresolved(headers.clone())
        };
        resolved.absorb(&rows);
        ctx.default_headers.push(DefaultHeaders {
            origin: format!("Folder › {name}"),
            headers: rows.value,
        });
    }

    Prepared {
        resolved,
        ctx,
        locked,
        workspace_cas,
        jar_key,
        resolver,
        sensitive_headers: state.network.sensitive_headers(),
    }
}

/// The secret parts of an auth: a Basic password, a Bearer token, an API key's value.
fn auth_credentials(auth: &Auth) -> Vec<ironcall_secrets::SecretString> {
    let value = match auth {
        Auth::Basic { password, .. } => password,
        Auth::Bearer { token, .. } => token,
        Auth::ApiKey { value, .. } => value,
        Auth::Inherit | Auth::None { .. } => return Vec::new(),
    };
    if value.is_empty() {
        Vec::new()
    } else {
        vec![ironcall_secrets::SecretString::from(value.clone())]
    }
}

/// True if a proxy URL has `user@` or `user:password@` before the host.
fn proxy_url_has_credentials(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority.contains('@')
}

/// A value passed through without resolution.
fn unresolved<T>(value: T) -> core_vars::Resolution<T> {
    core_vars::Resolution {
        value,
        secrets: Vec::new(),
        unresolved: Vec::new(),
    }
}

/// Network options for a prepared request.
pub(crate) fn net_options(state: &AppState, prepared: &Prepared) -> Result<NetOptions, String> {
    Ok(NetOptions {
        proxy: state.network.proxy(state)?,
        roots: state.network.roots(&prepared.workspace_cas)?,
        jar: state.network.jar(&prepared.jar_key),
        jar_key: prepared.jar_key.clone(),
        bypass_proxy: prepared.resolved.request.settings.bypass_proxy.unwrap_or(false),
        sensitive_headers: prepared.sensitive_headers.clone(),
    })
}

/// Whether `path` is really inside `root`: symlinks are resolved, so a link committed to
/// a repo (`data/key -> ~/.ssh/id_rsa`) counts as outside and needs picking first.
fn inside_workspace(root: &std::path::Path, path: &std::path::Path) -> bool {
    match (std::fs::canonicalize(root), std::fs::canonicalize(path)) {
        (Ok(root), Ok(path)) => paths::is_inside(&root, &path),
        // A file that doesn't exist can't be uploaded; the lexical answer is enough.
        _ => paths::is_inside(root, path),
    }
}

/// Checks the files a request would upload: they must exist, and be inside the
/// workspace or picked this session.
pub(crate) fn check_body_files(state: &AppState, prepared: &Prepared) -> Vec<BodyFileStatus> {
    let root = prepared.ctx.root.as_deref();
    wire::body_files(&prepared.resolved.request, root)
        .into_iter()
        .map(|f| {
            let Some(path) = f.resolved else {
                return BodyFileStatus {
                    path: f.written,
                    exists: false,
                    size: None,
                    outside_workspace: false,
                    allowed: false,
                };
            };
            let outside = !root.is_some_and(|r| inside_workspace(r, &path));
            let meta = std::fs::metadata(&path).ok().filter(|m| m.is_file());
            let allowed = !outside || state.path_guard.is_allowed_file(&path);
            BodyFileStatus {
                path: f.written,
                exists: meta.is_some(),
                size: meta.map(|m| m.len()),
                outside_workspace: outside,
                allowed,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------
// Commands: auth and files

/// The auth a request will use after inheritance (PLAN.md Phase 4), for the Auth tab's
/// "inherited from Folder › Users" display.
#[tauri::command]
#[specta::specta]
pub async fn auth_effective(
    state: tauri::State<'_, AppState>,
    request: RequestFile,
) -> Result<EffectiveAuth, String> {
    let mut chain: Vec<(ItemId, String, Auth)> = Vec::new();
    let mut collection_auth = Auth::default();
    if let Ok(ws) = state.workspaces.current() {
        let ws = lock(&ws);
        if ws.tree().find(request.id).is_some() {
            for f in ws.folder_chain(request.id).map_err(|e| e.to_string())? {
                let auth = ws.load_folder(f.id).map(|l| l.file.auth).unwrap_or_default();
                chain.push((f.id, f.name, auth));
            }
            collection_auth = ws.load_collection().map(|c| c.file.auth).unwrap_or_default();
        }
    }
    let folders: Vec<FolderAuth<'_>> = chain
        .iter()
        .map(|(id, name, auth)| FolderAuth { id: *id, name, auth })
        .collect();
    let (auth, origin) = core_auth::effective_auth(&request.auth, &folders, &collection_auth);
    Ok(EffectiveAuth {
        auth,
        origin: match origin {
            core_auth::AuthOrigin::Request => AuthOrigin::Request,
            core_auth::AuthOrigin::Folder { id, name } => AuthOrigin::Folder { id, name },
            core_auth::AuthOrigin::Collection => AuthOrigin::Collection,
            core_auth::AuthOrigin::None => AuthOrigin::None,
        },
    })
}

/// Opens a native file picker for a multipart file part or binary body. The picked file
/// is allowed for sending for the rest of this session. `None` if cancelled.
#[tauri::command]
#[specta::specta]
pub async fn pick_body_file(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Option<PickedFile>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_file())
        .await
        .map_err(|e| e.to_string())?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else {
        return Ok(None);
    };
    state.path_guard.allow_file(&path).map_err(|e| e.to_string())?;
    let root = state
        .workspaces
        .current()
        .ok()
        .map(|ws| lock(&ws).root().to_path_buf());
    let outside = !root.as_deref().is_some_and(|r| paths::is_inside(r, &path));
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    Ok(Some(PickedFile {
        path: paths::to_written(root.as_deref(), &path),
        file_name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size: meta.len(),
        content_type: paths::guess_content_type(&path).map(str::to_string),
        outside_workspace: outside,
    }))
}

/// Checks the files a request would upload, so the UI can show missing or not-allowed
/// files before sending.
#[tauri::command]
#[specta::specta]
pub async fn body_files_check(
    state: tauri::State<'_, AppState>,
    request: RequestFile,
) -> Result<Vec<BodyFileStatus>, String> {
    let prepared = prepare(&state, &request);
    Ok(check_body_files(&state, &prepared))
}

// ---------------------------------------------------------------------------------
// Commands: network settings and CAs

fn ca_info(path: &str) -> CaCertInfo {
    match ironcall_http::inspect_ca_file(Path::new(path)) {
        Ok(info) => CaCertInfo {
            path: path.to_string(),
            subject: info.subject,
            not_after: info
                .not_after
                .and_then(|s| u64::try_from(s).ok())
                .map(|s| rfc3339_utc(UNIX_EPOCH + Duration::from_secs(s))),
            count: info.count,
            error: None,
        },
        Err(e) => CaCertInfo {
            path: path.to_string(),
            subject: None,
            not_after: None,
            count: 0,
            error: Some(e.to_string()),
        },
    }
}

/// `HTTPS_PROXY` (or `HTTP_PROXY`) from the environment, with any credentials masked.
fn environment_proxy() -> Option<String> {
    [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ]
    .iter()
    .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
    .map(|v| ironcall_core::redact::redact_url(&v))
}

fn settings_view(state: &AppState) -> NetworkSettings {
    let network = state.network.settings.load().network;
    let has_password = proxy_password(state).ok().flatten().is_some();
    NetworkSettings {
        proxy: ProxySettings {
            mode: match network.proxy.mode {
                store_settings::ProxyMode::None => ProxyMode::None,
                store_settings::ProxyMode::Environment => ProxyMode::Environment,
                store_settings::ProxyMode::Manual => ProxyMode::Manual,
            },
            url: network.proxy.url,
            username: network.proxy.username,
            has_password,
            no_proxy: network.proxy.no_proxy,
        },
        ca_certs: network.ca_certs.iter().map(|p| ca_info(p)).collect(),
        environment_proxy: environment_proxy(),
    }
}

#[tauri::command]
#[specta::specta]
pub async fn network_settings_get(state: tauri::State<'_, AppState>) -> Result<NetworkSettings, String> {
    Ok(settings_view(&state))
}

/// Saves proxy settings (except the password) and the global CA list.
#[tauri::command]
#[specta::specta]
pub async fn network_settings_set(
    state: tauri::State<'_, AppState>,
    proxy: ProxySettings,
    ca_paths: Vec<String>,
) -> Result<NetworkSettings, String> {
    if proxy.mode == ProxyMode::Manual {
        let url = proxy.url.as_deref().unwrap_or_default();
        // Credentials in the URL would be saved in plain text in settings.json.
        if proxy_url_has_credentials(url) {
            return Err(
                "Put the proxy's username in the Username field and use Set password (stored in the OS keychain), not user:password@ in the URL."
                    .into(),
            );
        }
        ironcall_http::ProxyServer::parse(url, None)
            .map_err(|e| ironcall_core::redact::scrub(&e.to_string()))?;
    }
    let mut settings = state.network.settings.load();
    settings.network.proxy = store_settings::ProxySettings {
        mode: match proxy.mode {
            ProxyMode::None => store_settings::ProxyMode::None,
            ProxyMode::Environment => store_settings::ProxyMode::Environment,
            ProxyMode::Manual => store_settings::ProxyMode::Manual,
        },
        url: proxy.url.filter(|u| !u.trim().is_empty()),
        username: proxy.username.filter(|u| !u.is_empty()),
        no_proxy: proxy
            .no_proxy
            .into_iter()
            .filter(|h| !h.trim().is_empty())
            .collect(),
    };
    settings.network.ca_certs = ca_paths;
    state
        .network
        .settings
        .save(&settings)
        .map_err(|e| e.to_string())?;
    Ok(settings_view(&state))
}

/// Stores (or clears, with `None`) the manual proxy's password in the OS keychain.
#[tauri::command]
#[specta::specta]
pub async fn proxy_password_set(
    state: tauri::State<'_, AppState>,
    password: Option<String>,
) -> Result<(), String> {
    let store = state.secrets.global_store().map_err(|e| e.message)?;
    match password {
        Some(p) => store.set(&proxy_password_key(), &SecretString::from(p)),
        None => store.delete(&proxy_password_key()),
    }
    .map_err(|e| e.to_string())
}

/// Opens a native file picker for a CA certificate (PEM/DER/CRT) and returns its info.
#[tauri::command]
#[specta::specta]
pub async fn pick_ca_cert(app: tauri::AppHandle) -> Result<Option<CaCertInfo>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("Certificates", &["pem", "crt", "cer", "der"])
            .blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(picked
        .and_then(|p| p.into_path().ok())
        .map(|p| ca_info(&p.display().to_string())))
}

/// CA certificates trusted for the open workspace only (stored in `.ironcall/local.yaml`,
/// since paths are machine-specific).
#[tauri::command]
#[specta::specta]
pub async fn workspace_ca_certs_get(state: tauri::State<'_, AppState>) -> Result<Vec<CaCertInfo>, String> {
    let ws = state.workspaces.current().map_err(|e| e.message)?;
    let cas = lock(&ws).load_local().ca_certs;
    Ok(cas.iter().map(|p| ca_info(p)).collect())
}

#[tauri::command]
#[specta::specta]
pub async fn workspace_ca_certs_set(
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
) -> Result<Vec<CaCertInfo>, String> {
    let ws = state.workspaces.current().map_err(|e| e.message)?;
    let ws = lock(&ws);
    let mut local = ws.load_local();
    local.ca_certs = paths;
    ws.save_local(&local).map_err(|e| e.to_string())?;
    Ok(local.ca_certs.iter().map(|p| ca_info(p)).collect())
}

// ---------------------------------------------------------------------------------
// Commands: cookies

fn current_jar(state: &AppState) -> (String, Arc<CookieJar>) {
    let key = state
        .workspaces
        .current()
        .ok()
        .map(|ws| lock(&ws).id().to_string())
        .unwrap_or_else(|| store_settings::NO_WORKSPACE.to_string());
    let jar = state.network.jar(&key);
    (key, jar)
}

fn same_site_to_ipc(s: ironcall_http::SameSite) -> SameSite {
    match s {
        ironcall_http::SameSite::Strict => SameSite::Strict,
        ironcall_http::SameSite::Lax => SameSite::Lax,
        ironcall_http::SameSite::None => SameSite::None,
    }
}

#[tauri::command]
#[specta::specta]
pub async fn cookies_list(state: tauri::State<'_, AppState>) -> Result<Vec<CookieRow>, String> {
    let (_, jar) = current_jar(&state);
    Ok(jar
        .list()
        .into_iter()
        .map(|c| CookieRow {
            domain: c.domain,
            path: c.path,
            name: c.name,
            value: c.value,
            expires: c.expires.map(rfc3339_utc),
            secure: c.secure,
            http_only: c.http_only,
            same_site: c.same_site.map(same_site_to_ipc),
            host_only: c.host_only,
        })
        .collect())
}

/// Adds or replaces a cookie (matched by domain, path and name).
#[tauri::command]
#[specta::specta]
pub async fn cookie_set(state: tauri::State<'_, AppState>, cookie: CookieRow) -> Result<(), String> {
    let expires = match cookie.expires.as_deref().filter(|e| !e.is_empty()) {
        Some(e) => Some(humantime::parse_rfc3339_weak(e).map_err(|err| format!("expires: {err}"))?),
        None => None,
    };
    let (key, jar) = current_jar(&state);
    jar.set(CookieInfo {
        domain: cookie.domain,
        path: cookie.path,
        name: cookie.name,
        value: cookie.value,
        expires,
        secure: cookie.secure,
        http_only: cookie.http_only,
        same_site: cookie.same_site.map(|s| match s {
            SameSite::Strict => ironcall_http::SameSite::Strict,
            SameSite::Lax => ironcall_http::SameSite::Lax,
            SameSite::None => ironcall_http::SameSite::None,
        }),
        host_only: cookie.host_only,
    })?;
    state.network.save_jar(&key);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn cookie_delete(
    state: tauri::State<'_, AppState>,
    domain: String,
    path: String,
    name: String,
) -> Result<(), String> {
    let (key, jar) = current_jar(&state);
    jar.delete(&domain, &path, &name);
    state.network.save_jar(&key);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn cookies_clear(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let (key, jar) = current_jar(&state);
    jar.clear();
    state.network.save_jar(&key);
    Ok(())
}

#[cfg(test)]
mod tests {
    use ironcall_core::model::{Body, MultipartKind, MultipartPart};

    use super::*;
    use crate::test_support::fixture;

    fn ws_of(state: &AppState) -> Arc<std::sync::Mutex<Workspace>> {
        state.workspaces.current().unwrap()
    }

    #[test]
    fn folder_auth_and_default_headers_are_inherited() {
        let f = fixture();
        let state = &f.state;
        let ws = ws_of(state);
        let request_id = {
            let mut ws = lock(&ws);
            let root = ws.tree().id;
            let folder = ws.create_folder(root, "Users", None).unwrap().node.id;
            let request = ws.create_request(folder, "Get user", None).unwrap().node.id;

            let mut collection = ws.load_collection().unwrap();
            collection.file.headers = vec![
                KeyValue::new("X-Client", "ironcall"),
                KeyValue::new("X-Env", "coll"),
            ];
            ws.save_collection(&collection.file, collection.hash.as_deref(), false)
                .unwrap();

            let mut folder_file = ws.load_folder(folder).unwrap();
            folder_file.file.auth = Auth::Bearer {
                token: "folder-token".into(),
                extra: Default::default(),
            };
            folder_file.file.headers = vec![KeyValue::new("X-Env", "folder")];
            ws.save_folder(&folder_file.file, folder_file.hash.as_deref(), false)
                .unwrap();

            let mut req = ws.load_request(request).unwrap();
            req.file.url = "https://api.example.com/users".into();
            ws.save_request(&req.file, req.hash.as_deref(), false).unwrap();
            request
        };
        let request = lock(&ws).load_request(request_id).unwrap().file;

        let prepared = prepare(state, &request);
        assert_eq!(prepared.ctx.auth_origin.as_deref(), Some("Folder › Users"));
        let preview = state
            .sender
            .preview(&prepared.resolved.request, &prepared.ctx, None, &[], false)
            .unwrap();
        let find = |name: &str| {
            preview
                .headers
                .iter()
                .filter(|h| h.name == name && h.effective)
                .collect::<Vec<_>>()
        };
        assert_eq!(find("Authorization")[0].value, "Bearer folder-token");
        assert_eq!(find("Authorization")[0].origin.as_deref(), Some("Folder › Users"));
        assert_eq!(find("X-Client")[0].value, "ironcall");
        // The folder's default header overrides the collection's.
        assert_eq!(find("X-Env").len(), 1);
        assert_eq!(find("X-Env")[0].value, "folder");
    }

    #[test]
    fn uploads_outside_the_workspace_need_a_pick_this_session() {
        let f = fixture();
        let state = &f.state;
        let inside = f.root.join("payload.bin");
        std::fs::write(&inside, b"inside").unwrap();
        let outside_dir = tempfile::tempdir().unwrap();
        let outside = outside_dir.path().join("secret.txt");
        std::fs::write(&outside, b"outside").unwrap();

        let mut request = RequestFile::new("Upload");
        request.method = "POST".into();
        request.url = "https://api.example.com/upload".into();
        request.body = Body::Multipart {
            parts: vec![
                MultipartPart {
                    name: "a".into(),
                    kind: MultipartKind::File,
                    value: None,
                    path: Some("payload.bin".into()),
                    content_type: None,
                    enabled: true,
                    extra: Default::default(),
                },
                MultipartPart {
                    name: "b".into(),
                    kind: MultipartKind::File,
                    value: None,
                    path: Some(outside.display().to_string()),
                    content_type: None,
                    enabled: true,
                    extra: Default::default(),
                },
            ],
            extra: Default::default(),
        };

        let status = check_body_files(state, &prepare(state, &request));
        assert_eq!(status.len(), 2);
        assert!(status[0].exists && status[0].allowed && !status[0].outside_workspace);
        assert!(status[1].exists && status[1].outside_workspace);
        assert!(
            !status[1].allowed,
            "a workspace can't make the app upload arbitrary local files"
        );

        state.path_guard.allow_file(&outside).unwrap();
        let status = check_body_files(state, &prepare(state, &request));
        assert!(status[1].allowed, "picked this session");
    }

    #[test]
    fn proxy_urls_with_credentials_are_detected() {
        assert!(proxy_url_has_credentials("http://bob:pw@proxy:3128"));
        assert!(proxy_url_has_credentials("bob@proxy:3128"));
        assert!(!proxy_url_has_credentials("http://proxy.corp:3128"));
        assert!(!proxy_url_has_credentials("http://proxy:3128/path@x"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_in_the_workspace_pointing_outside_are_not_allowed() {
        let f = fixture();
        let state = &f.state;
        let elsewhere = tempfile::tempdir().unwrap();
        let secret = elsewhere.path().join("id_rsa");
        std::fs::write(&secret, "PRIVATE KEY").unwrap();
        std::os::unix::fs::symlink(&secret, f.root.join("payload.bin")).unwrap();
        std::fs::write(f.root.join("real.bin"), "ok").unwrap();

        let mut request = RequestFile::new("upload");
        request.url = "https://example.com/".into();
        for (path, expect_allowed) in [("payload.bin", false), ("real.bin", true)] {
            request.body = Body::BinaryFile {
                path: path.into(),
                content_type: None,
                extra: Default::default(),
            };
            let status = check_body_files(state, &prepare(state, &request));
            assert_eq!(status[0].allowed, expect_allowed, "{path}");
            assert_eq!(status[0].outside_workspace, !expect_allowed, "{path}");
        }
    }

    #[test]
    fn cookie_jars_persist_per_workspace() {
        let f = fixture();
        let state = &f.state;
        let (key, jar) = current_jar(state);
        jar.set(CookieInfo {
            domain: "api.example.com".into(),
            path: "/".into(),
            name: "session".into(),
            value: "abc".into(),
            expires: Some(SystemTime::now() + Duration::from_secs(3600)),
            secure: false,
            http_only: true,
            same_site: None,
            host_only: true,
        })
        .unwrap();
        state.network.save_jar(&key);

        // A new process (fresh Network) loads it from disk.
        let reloaded = Network::new(&f.data).jar(&key);
        let cookies = reloaded.list();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "session");
        assert_ne!(key, store_settings::NO_WORKSPACE);
    }
}
