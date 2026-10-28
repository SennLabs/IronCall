//! cURL import/export and OpenAPI/Swagger import (PLAN.md Phase 6): the IPC contract.
//!
//! Imports never write anything until the user confirms: `openapi_preview` parses a
//! spec into a plan held in memory (by `planId`) and shows what would be created;
//! `openapi_import` then writes it through the normal workspace operations (slugs,
//! IDs, ordering, own-writes).

use std::sync::{Arc, Mutex};

use ironcall_core::ItemId;
use ironcall_core::model::{Auth, RequestFile, Variable, VariableSpec};
use ironcall_core::redact;
use ironcall_core::wire::{self, WireBody, WireRequest};
use ironcall_import::{curl, openapi};
use ironcall_secrets::SecretString;
use ironcall_store::workspace::{Workspace, WorkspaceError as StoreError};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::AppState;

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportWarning {
    pub message: String,
    /// Where it applies, e.g. a flag (`--cert`), a JSON pointer (`#/paths/~1users/get`)
    /// or an operation (`GET /users`).
    pub location: Option<String>,
}

// ---------------------------------------------------------------------------------
// cURL

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CurlParseResult {
    /// A new request (fresh ID, name derived from method and path). Open it in a tab
    /// or merge it into the current one.
    pub request: RequestFile,
    /// Flags that were ignored or only partly supported (never dropped silently).
    pub warnings: Vec<ImportWarning>,
}

/// Parses a cURL command (bash/POSIX quoting, or Chrome's "Copy as cURL (cmd)" with
/// `^` escapes; detected automatically).
#[tauri::command]
#[specta::specta]
pub fn curl_parse(command: String) -> Result<CurlParseResult, String> {
    let parsed = curl::parse(&command).map_err(|e| e.to_string())?;
    Ok(CurlParseResult {
        request: parsed.request,
        warnings: parsed
            .warnings
            .into_iter()
            .map(|w| warning(w.message, w.location))
            .collect(),
    })
}

/// Cheap check for the URL bar's paste handler: does this text look like a cURL command?
#[tauri::command]
#[specta::specta]
pub fn curl_detect(text: String) -> bool {
    let t = text.trim_start();
    t.starts_with("curl ") || t.starts_with("curl.exe ") || t.starts_with("curl\t")
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CurlDialect {
    /// bash/zsh/sh, single-quoted.
    Posix,
    /// PowerShell (`curl.exe` with backtick continuations).
    PowerShell,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CurlExportMode {
    /// As authored: `{{variables}}` left in place (inherited auth and default headers
    /// are included, also unresolved). Safe to share.
    Variables,
    /// Variables resolved; secret values and sensitive headers masked as `••••`.
    /// The default.
    Masked,
    /// Fully resolved, secrets included. Only after an explicit choice in the UI.
    Resolved,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CurlExport {
    pub command: String,
    /// E.g. multipart files referenced by path, or settings curl can't express.
    pub warnings: Vec<ImportWarning>,
}

/// Builds a cURL command for a request.
#[tauri::command]
#[specta::specta]
pub async fn curl_export(
    state: tauri::State<'_, AppState>,
    request: RequestFile,
    dialect: CurlDialect,
    mode: CurlExportMode,
) -> Result<CurlExport, String> {
    export(&state, &request, dialect, mode)
}

fn export(
    state: &AppState,
    request: &RequestFile,
    dialect: CurlDialect,
    mode: CurlExportMode,
) -> Result<CurlExport, String> {
    let prepared = crate::network::prepare_with(state, request, mode != CurlExportMode::Variables);
    let mut ctx = prepared.ctx.clone();
    ctx.allow_unresolved = mode == CurlExportMode::Variables;
    // "With variables" is offered as safe to share: `{{references}}` stay, but literal
    // credentials typed into the request (or inherited auth) are masked.
    let request = if mode == CurlExportMode::Variables {
        ctx.auth = redact::redact_auth(&ctx.auth);
        redact::redact_request_file(&prepared.resolved.request, &prepared.sensitive_headers)
    } else {
        prepared.resolved.request.clone()
    };
    let mut wire = wire::build(&request, &ctx).map_err(|e| e.to_string())?;
    if mode == CurlExportMode::Masked {
        mask_wire(&mut wire, &prepared.resolved.secrets, &prepared.sensitive_headers);
    }
    let dialect = match dialect {
        CurlDialect::Posix => curl::Dialect::Posix,
        CurlDialect::PowerShell => curl::Dialect::PowerShell,
    };
    let export = curl::export(&wire, dialect, curl::ExportOptions::default());
    Ok(CurlExport {
        command: export.command,
        warnings: export
            .warnings
            .into_iter()
            .map(|w| warning(w.message, w.location))
            .collect(),
    })
}

// ---------------------------------------------------------------------------------
// OpenAPI / Swagger

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SpecVersion {
    Swagger2,
    OpenApi30,
    OpenApi31,
}

/// One folder or request in the import preview.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PlanNode {
    pub name: String,
    /// `None` for folders.
    pub method: Option<String>,
    /// For requests, e.g. `{{baseUrl}}/pets/:petId`.
    pub url: Option<String>,
    pub children: Vec<PlanNode>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OpenApiPreview {
    /// Pass to `openapi_import`. Valid until the next preview or app restart.
    pub plan_id: String,
    pub version: SpecVersion,
    pub title: String,
    pub description: Option<String>,
    /// Folders (from tags) and requests (from operations).
    pub tree: Vec<PlanNode>,
    pub request_count: u32,
    /// Server URLs found; the first becomes `baseUrl` in the new environment.
    pub servers: Vec<String>,
    /// Security schemes mapped to auth, e.g. "bearerAuth → Bearer on the collection".
    pub auth: Vec<String>,
    pub warnings: Vec<ImportWarning>,
}

/// Opens a native picker for an OpenAPI/Swagger file (YAML or JSON) and previews it.
/// `None` if cancelled.
#[tauri::command]
#[specta::specta]
pub async fn openapi_pick_and_preview(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Option<OpenApiPreview>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("OpenAPI / Swagger", &["yaml", "yml", "json"])
            .blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else {
        return Ok(None);
    };
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_SPEC_BYTES {
        return Err(format!("the file is larger than {} MB", MAX_SPEC_BYTES >> 20));
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    preview(&state, bytes).await.map(Some)
}

/// Previews a spec pasted as text.
#[tauri::command]
#[specta::specta]
pub async fn openapi_preview_text(
    state: tauri::State<'_, AppState>,
    text: String,
) -> Result<OpenApiPreview, String> {
    preview(&state, text.into_bytes()).await
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OpenApiImportOptions {
    pub plan_id: String,
    /// Folder (or the collection root) to import into.
    pub parent: ItemId,
    /// Put everything in a new folder named after the spec title (recommended), rather
    /// than directly into `parent`.
    pub wrap_in_folder: bool,
    /// Create an environment with `baseUrl` (and any server variables). Its name is the
    /// spec title.
    pub create_environment: bool,
    /// Make that environment active.
    pub activate_environment: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OpenApiImportResult {
    /// The top-level folder created (when `wrapInFolder`), else `None`.
    pub folder: Option<ItemId>,
    pub requests_created: u32,
    pub folders_created: u32,
    pub environment: Option<ItemId>,
    /// Problems while writing (e.g. a name that had to be changed).
    pub warnings: Vec<ImportWarning>,
}

#[tauri::command]
#[specta::specta]
pub async fn openapi_import(
    state: tauri::State<'_, AppState>,
    options: OpenApiImportOptions,
) -> Result<OpenApiImportResult, String> {
    let plan = state
        .imports
        .get(&options.plan_id)
        .ok_or("this import preview has expired; preview the spec again")?;
    let ws = state.workspaces.current().map_err(|e| e.message)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut ws = ws.lock().expect("workspace lock poisoned");
        apply_plan(&mut ws, &plan, &options).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------------------------
// Helpers

/// Import plans waiting for confirmation, by plan ID (the last few only).
#[derive(Default)]
pub struct Plans(Mutex<Vec<(String, Arc<openapi::Plan>)>>);

impl Plans {
    const KEEP: usize = 4;

    fn insert(&self, plan: openapi::Plan) -> String {
        let id = ulid::Ulid::generate().to_string();
        let mut plans = self.0.lock().expect("plans lock poisoned");
        plans.push((id.clone(), Arc::new(plan)));
        let excess = plans.len().saturating_sub(Self::KEEP);
        plans.drain(..excess);
        id
    }

    fn get(&self, id: &str) -> Option<Arc<openapi::Plan>> {
        let plans = self.0.lock().expect("plans lock poisoned");
        plans.iter().find(|(k, _)| k == id).map(|(_, p)| p.clone())
    }
}

fn warning(message: String, location: Option<String>) -> ImportWarning {
    ImportWarning { message, location }
}

/// Masks secret values and sensitive headers in a wire request (PLAN.md §6.3).
fn mask_wire(wire: &mut WireRequest, secrets: &[SecretString], sensitive: &[String]) {
    wire.url = redact::redact_url_masking(&wire.url, secrets);
    for (name, value) in &mut wire.headers {
        *value = redact::mask_values(redact::header_value(name, value, sensitive), secrets);
    }
    match &mut wire.body {
        WireBody::Bytes(bytes) => {
            if let Ok(text) = std::str::from_utf8(bytes) {
                *bytes = redact::mask_values(text, secrets).into_bytes();
            }
        }
        WireBody::Multipart(parts) => {
            for part in parts {
                if let wire::PartContent::Text(text) = &mut part.content {
                    *text = redact::mask_values(text, secrets);
                }
            }
        }
        WireBody::File { .. } => {}
    }
}

/// Largest spec accepted (GitHub's full description is about 13MB).
const MAX_SPEC_BYTES: u64 = 64 * 1024 * 1024;

async fn preview(state: &AppState, bytes: Vec<u8>) -> Result<OpenApiPreview, String> {
    let plan = tauri::async_runtime::spawn_blocking(move || openapi::plan(&bytes))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let preview = OpenApiPreview {
        plan_id: String::new(),
        version: match plan.version {
            openapi::SpecVersion::Swagger2 => SpecVersion::Swagger2,
            openapi::SpecVersion::OpenApi30 => SpecVersion::OpenApi30,
            openapi::SpecVersion::OpenApi31 => SpecVersion::OpenApi31,
        },
        title: plan.title.clone(),
        description: plan.description.clone(),
        tree: plan.tree.iter().map(plan_node).collect(),
        request_count: plan.request_count() as u32,
        servers: plan.servers.clone(),
        auth: plan.auth_summary.clone(),
        warnings: plan
            .warnings
            .iter()
            .map(|w| warning(w.message.clone(), w.location.clone()))
            .collect(),
    };
    let plan_id = state.imports.insert(plan);
    Ok(OpenApiPreview { plan_id, ..preview })
}

fn plan_node(item: &openapi::PlanItem) -> PlanNode {
    match item {
        openapi::PlanItem::Folder { name, items, .. } => PlanNode {
            name: name.clone(),
            method: None,
            url: None,
            children: items.iter().map(plan_node).collect(),
        },
        openapi::PlanItem::Request(r) => PlanNode {
            name: r.name.clone(),
            method: Some(r.method.clone()),
            url: Some(r.url.clone()),
            children: Vec::new(),
        },
    }
}

/// Writes an import plan into the workspace. Auth is only ever set on folders and
/// requests created here; existing folders and the collection are never changed.
fn apply_plan(
    ws: &mut Workspace,
    plan: &openapi::Plan,
    options: &OpenApiImportOptions,
) -> Result<OpenApiImportResult, StoreError> {
    let mut result = OpenApiImportResult {
        folder: None,
        requests_created: 0,
        folders_created: 0,
        environment: None,
        warnings: Vec::new(),
    };
    let root_auth = plan.root_auth.clone();
    let target = if options.wrap_in_folder {
        let folder = create_folder(
            ws,
            options.parent,
            &plan.title,
            plan.description.clone(),
            root_auth.clone(),
            &mut result,
        )?;
        result.folder = Some(folder);
        folder
    } else {
        options.parent
    };
    // Without a wrapper folder, the spec's auth goes on each top-level item created.
    let top_auth = if options.wrap_in_folder { None } else { root_auth };
    for item in &plan.tree {
        write_item(ws, target, item, top_auth.as_ref(), &mut result)?;
    }

    if options.create_environment && !plan.environment.is_empty() {
        let env = ws.create_environment(&plan.title)?;
        let mut loaded = ws.load_environment(env.id)?;
        for var in &plan.environment {
            let value = if var.secret {
                Variable::Detailed(VariableSpec {
                    secret: true,
                    ..VariableSpec::default()
                })
            } else {
                Variable::Plain(var.value.clone())
            };
            loaded.file.vars.insert(var.name.clone(), value);
        }
        ws.save_environment(&loaded.file, loaded.hash.as_deref(), true)?;
        if options.activate_environment {
            let mut local = ws.load_local();
            local.active_environment = Some(env.id);
            ws.save_local(&local)?;
        }
        result.environment = Some(env.id);
    }
    Ok(result)
}

fn create_folder(
    ws: &mut Workspace,
    parent: ItemId,
    name: &str,
    description: Option<String>,
    auth: Option<Auth>,
    result: &mut OpenApiImportResult,
) -> Result<ItemId, StoreError> {
    let created = ws.create_folder(parent, name, None)?;
    if let Some(w) = created.warning {
        result.warnings.push(warning(w, Some(name.to_string())));
    }
    let id = created.node.id;
    let description = description.filter(|d| !d.trim().is_empty());
    if description.is_some() || auth.is_some() {
        let mut folder = ws.load_folder(id)?;
        folder.file.description = description;
        if let Some(auth) = auth {
            folder.file.auth = auth;
        }
        ws.save_folder(&folder.file, folder.hash.as_deref(), true)?;
    }
    result.folders_created += 1;
    Ok(id)
}

fn write_item(
    ws: &mut Workspace,
    parent: ItemId,
    item: &openapi::PlanItem,
    auth: Option<&Auth>,
    result: &mut OpenApiImportResult,
) -> Result<(), StoreError> {
    match item {
        openapi::PlanItem::Folder {
            name,
            description,
            items,
        } => {
            let folder = create_folder(ws, parent, name, description.clone(), auth.cloned(), result)?;
            for child in items {
                write_item(ws, folder, child, None, result)?;
            }
        }
        openapi::PlanItem::Request(request) => {
            let created = ws.create_request(parent, &request.name, None)?;
            if let Some(w) = created.warning {
                result.warnings.push(warning(w, Some(request.name.clone())));
            }
            let base = ws.load_request(created.node.id)?.hash;
            let mut file = (**request).clone();
            file.id = created.node.id;
            if let Some(auth) = auth
                && file.auth.is_inherit()
            {
                file.auth = auth.clone();
            }
            ws.save_request(&file, base.as_deref(), true)?;
            result.requests_created += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ironcall_core::model::KeyValue;

    use super::*;
    use crate::test_support::fixture;

    fn fixture_spec(name: &str) -> Vec<u8> {
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/openapi/").to_string() + name)
            .unwrap()
    }

    #[test]
    fn openapi_plan_is_written_into_the_workspace() {
        let f = fixture();
        let state = &f.state;
        let plan = openapi::plan(&fixture_spec("edge-security.yaml")).unwrap();
        let expected_requests = plan.request_count() as u32;
        let ws = state.workspaces.current().unwrap();
        let mut ws = ws.lock().unwrap();
        let root = ws.tree().id;
        let collection_before = ws.load_collection().unwrap().hash;

        let result = apply_plan(
            &mut ws,
            &plan,
            &OpenApiImportOptions {
                plan_id: String::new(),
                parent: root,
                wrap_in_folder: true,
                create_environment: true,
                activate_environment: true,
            },
        )
        .unwrap();

        assert_eq!(result.requests_created, expected_requests);
        let folder = result.folder.unwrap();
        assert_eq!(ws.tree().find(folder).unwrap().name, plan.title);
        // The spec's global auth goes on the new folder; the collection is untouched.
        let folder_file = ws.load_folder(folder).unwrap().file;
        assert!(!folder_file.auth.is_inherit());
        assert_eq!(ws.load_collection().unwrap().hash, collection_before);

        // An environment with the spec's variables; secrets declared without values.
        let env = result.environment.unwrap();
        assert_eq!(ws.load_local().active_environment, Some(env));
        let vars = ws.environment_vars(env).unwrap();
        assert!(vars.values().any(|v| v.is_secret()));
        let env_file = std::fs::read_to_string(ws.environment_path(env).unwrap()).unwrap();
        assert!(!env_file.contains("value: '<"), "{env_file}");
    }

    #[test]
    fn petstore_v2_and_v3_import_without_errors() {
        for name in [
            "petstore-v2.json",
            "petstore-expanded-v3.0.yaml",
            "petstore-v3.1.yaml",
        ] {
            let f = fixture();
            let plan = openapi::plan(&fixture_spec(name)).unwrap();
            let ws = f.state.workspaces.current().unwrap();
            let mut ws = ws.lock().unwrap();
            let root = ws.tree().id;
            let result = apply_plan(
                &mut ws,
                &plan,
                &OpenApiImportOptions {
                    plan_id: String::new(),
                    parent: root,
                    wrap_in_folder: false,
                    create_environment: false,
                    activate_environment: false,
                },
            )
            .unwrap();
            assert!(result.requests_created > 0, "{name}");
            assert_eq!(
                ws.tree().request_count() as u32,
                result.requests_created,
                "{name}"
            );
        }
    }

    #[test]
    fn curl_export_modes() {
        let f = fixture();
        let state = &f.state;
        {
            let ws = state.workspaces.current().unwrap();
            let mut ws = ws.lock().unwrap();
            let mut c = ws.load_collection().unwrap();
            c.file
                .vars
                .insert("host".into(), Variable::Plain("https://api.example.com".into()));
            ws.save_collection(&c.file, c.hash.as_deref(), false).unwrap();
        }
        let mut req = RequestFile::new("x");
        req.url = "{{host}}/users".into();
        req.headers = vec![KeyValue::new("Authorization", "Bearer abc123def456")];

        let build = |req: &RequestFile, mode| export(state, req, CurlDialect::Posix, mode).unwrap().command;
        let vars = build(&req, CurlExportMode::Variables);
        assert!(vars.contains("{{host}}/users"), "{vars}");
        assert!(
            !vars.contains("abc123def456"),
            "\"safe to share\" masks literal credentials: {vars}"
        );

        // Literal auth credentials too; a {{reference}} stays.
        req.headers.clear();
        req.auth = ironcall_core::model::Auth::Bearer {
            token: "literal-token-999".into(),
            extra: Default::default(),
        };
        assert!(!build(&req, CurlExportMode::Variables).contains("literal-token-999"));
        req.auth = ironcall_core::model::Auth::Bearer {
            token: "{{token}}".into(),
            extra: Default::default(),
        };
        let with_ref = build(&req, CurlExportMode::Variables);
        assert!(with_ref.contains("{{token}}"), "{with_ref}");
        req.auth = ironcall_core::model::Auth::default();
        req.headers = vec![KeyValue::new("Authorization", "Bearer abc123def456")];
        let masked = build(&req, CurlExportMode::Masked);
        assert!(masked.contains("https://api.example.com/users"), "{masked}");
        assert!(
            !masked.contains("abc123def456"),
            "sensitive header masked: {masked}"
        );
        let resolved = build(&req, CurlExportMode::Resolved);
        assert!(resolved.contains("abc123def456"), "{resolved}");
        // Round trip: the resolved export parses back to the same URL.
        let parsed = curl::parse(&resolved).unwrap();
        assert_eq!(parsed.request.url, "https://api.example.com/users");
    }
}
