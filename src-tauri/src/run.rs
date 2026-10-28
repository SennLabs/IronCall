//! Collection runs and runtime variables (PLAN.md Phase 7).
//!
//! The runner itself is `ironcall_core::runner`; this module lists the requests to run,
//! sends them through the normal send path, reports progress as [`RunProgress`] events
//! and keeps each workspace's session runtime variables (values from `extract`).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use indexmap::IndexMap;
use ironcall_core::ItemId;
use ironcall_core::extract::Extracted;
use ironcall_core::runner::{self, ExecError, Executed, Executor, Progress, RunItem, RunOptions, RunReport};
use ironcall_core::runtime::{RuntimeVarView, RuntimeVars};
use ironcall_http::CancellationToken;
use ironcall_store::workspace::{Node, NodeKind};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

use crate::AppState;
use crate::send::SendErrorKind;
use crate::vars::{EnvChoice, Scope, VarsError};

/// How many finished runs keep their report for "Copy results as Markdown".
const KEEP_REPORTS: usize = 10;

/// Key for state that belongs to no workspace.
const NO_WORKSPACE: &str = "";

#[derive(Default)]
pub struct Runs {
    /// Runtime variables per workspace ID, for normal sends.
    session: Mutex<HashMap<String, RuntimeVars>>,
    active: Mutex<HashMap<String, Active>>,
    /// Finished runs: run ID → (title, report), oldest first.
    reports: Mutex<IndexMap<String, (String, RunReport)>>,
}

struct Active {
    token: CancellationToken,
    sends: HashSet<String>,
}

fn workspace_key(state: &AppState) -> String {
    state
        .workspaces
        .current()
        .map(|ws| ws.lock().expect("workspace lock poisoned").id().to_string())
        .unwrap_or_else(|_| NO_WORKSPACE.to_string())
}

impl Runs {
    pub fn new() -> Self {
        Self::default()
    }

    /// The open workspace's session runtime variables.
    pub(crate) fn session_runtime(&self, state: &AppState) -> RuntimeVars {
        let key = workspace_key(state);
        self.session
            .lock()
            .expect("runs lock")
            .get(&key)
            .cloned()
            .unwrap_or_default()
    }

    fn with_session<T>(&self, state: &AppState, f: impl FnOnce(&mut RuntimeVars) -> T) -> T {
        let key = workspace_key(state);
        f(self.session.lock().expect("runs lock").entry(key).or_default())
    }

    pub(crate) fn apply_to_session(&self, state: &AppState, extracted: &[Extracted]) {
        self.with_session(state, |rt| rt.apply(extracted));
    }
}

/// Live progress of a collection run.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct RunProgress {
    pub run_id: String,
    pub progress: Progress,
}

/// What to run: the whole workspace, or one folder.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RunTarget {
    /// `None` for the whole workspace.
    pub folder: Option<ItemId>,
}

/// The requests under `target`, in tree order (unreadable files are skipped).
fn items(state: &AppState, target: &RunTarget) -> Result<(String, Vec<RunItem>), VarsError> {
    let ws = state
        .workspaces
        .current()
        .map_err(|_| VarsError::new("no workspace is open"))?;
    let ws = ws.lock().expect("workspace lock poisoned");
    let root = match target.folder {
        None => ws.tree(),
        Some(id) => ws
            .tree()
            .find(id)
            .filter(|n| matches!(n.kind, NodeKind::Folder | NodeKind::Collection))
            .ok_or_else(|| VarsError::new("that folder no longer exists"))?,
    };
    let mut out = Vec::new();
    collect(root, &mut Vec::new(), &mut out);
    Ok((root.name.clone(), out))
}

fn collect(node: &Node, crumbs: &mut Vec<String>, out: &mut Vec<RunItem>) {
    for child in &node.children {
        match child.kind {
            NodeKind::Request => out.push(RunItem {
                id: child.id,
                name: child.name.clone(),
                folder: crumbs.join(" / "),
            }),
            NodeKind::Folder => {
                crumbs.push(child.name.clone());
                collect(child, crumbs, out);
                crumbs.pop();
            }
            NodeKind::Collection | NodeKind::Error => {}
        }
    }
}

/// Sends a run's requests through the normal send path.
struct TauriExecutor<'a, R: tauri::Runtime> {
    state: &'a AppState,
    app: &'a tauri::AppHandle<R>,
    run_id: &'a str,
    token: CancellationToken,
    environment: EnvChoice,
    counter: AtomicUsize,
    /// Key of the workspace the run started in.
    workspace: String,
}

impl<R: tauri::Runtime> TauriExecutor<'_, R> {
    fn track(&self, send_id: &str, running: bool) {
        let mut active = self.state.runs.active.lock().expect("runs lock");
        if let Some(run) = active.get_mut(self.run_id) {
            if running {
                run.sends.insert(send_id.to_string());
            } else {
                run.sends.remove(send_id);
            }
        }
    }
}

impl<R: tauri::Runtime> Executor for TauriExecutor<'_, R> {
    async fn execute(&self, item: &RunItem, runtime: RuntimeVars) -> Result<Executed, ExecError> {
        if self.token.is_cancelled() {
            return Err(ExecError::Cancelled);
        }
        // A run belongs to the workspace it started in: if another one was opened, its
        // requests would resolve against the wrong variables. Stop instead.
        if workspace_key(self.state) != self.workspace {
            self.token.cancel();
            return Err(ExecError::Failed {
                message: "the workspace was closed during the run".into(),
                history_id: None,
            });
        }
        let request = {
            let ws = self.state.workspaces.current().map_err(|_| ExecError::Failed {
                message: "no workspace is open".into(),
                history_id: None,
            })?;
            let ws = ws.lock().expect("workspace lock poisoned");
            ws.load_request(item.id).map_err(|e| ExecError::Failed {
                message: format!("couldn't load the request: {e}"),
                history_id: None,
            })?
        };
        let send_id = format!("{}-{}", self.run_id, self.counter.fetch_add(1, Ordering::SeqCst));
        self.track(&send_id, true);
        // A cancel that landed between the check above and `track` would miss this send.
        if self.token.is_cancelled() {
            self.track(&send_id, false);
            return Err(ExecError::Cancelled);
        }
        let scope = Scope::Run {
            environment: self.environment.clone(),
            runtime,
        };
        let (result, checked) = crate::send::send_in(
            self.state,
            self.app,
            send_id.clone(),
            &request.file,
            &scope,
            Some(&self.token),
        )
        .await;
        self.track(&send_id, false);
        match result {
            Ok(summary) => {
                let checked = checked.unwrap_or_default();
                Ok(Executed {
                    status: summary.status,
                    duration_ms: summary.timings.total_ms,
                    history_id: summary.history_id,
                    assertions: checked.assertions,
                    extracted: checked.extracted,
                })
            }
            Err(e) if e.kind == SendErrorKind::Cancelled => Err(ExecError::Cancelled),
            Err(e) => Err(ExecError::Failed {
                message: e.message,
                history_id: e.history_id,
            }),
        }
    }

    async fn sleep(&self, duration: Duration) {
        tokio::select! {
            _ = tokio::time::sleep(duration) => {}
            _ = self.token.cancelled() => {}
        }
    }

    fn cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

/// The requests a run of `target` would send, in order.
#[tauri::command]
#[specta::specta]
pub fn run_items(state: tauri::State<'_, AppState>, target: RunTarget) -> Result<Vec<RunItem>, VarsError> {
    Ok(items(&state, &target)?.1)
}

/// Runs the requests under `target`. `runId` is chosen by the UI so it can cancel before
/// this returns; progress arrives as [`RunProgress`] events. The run starts from the
/// session's runtime variables, and what it extracts is kept in the session afterwards.
#[tauri::command]
#[specta::specta]
pub async fn run_collection(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    run_id: String,
    target: RunTarget,
    environment: EnvChoice,
    options: RunOptions,
) -> Result<RunReport, VarsError> {
    run_in(&state, &app, run_id, target, environment, options).await
}

async fn run_in<R: tauri::Runtime>(
    state: &AppState,
    app: &tauri::AppHandle<R>,
    run_id: String,
    target: RunTarget,
    environment: EnvChoice,
    options: RunOptions,
) -> Result<RunReport, VarsError> {
    let (title, list) = items(state, &target)?;
    let token = CancellationToken::new();
    {
        let mut active = state.runs.active.lock().expect("runs lock");
        if active.contains_key(&run_id) {
            return Err(VarsError::new("a run with this ID is already running"));
        }
        active.insert(
            run_id.clone(),
            Active {
                token: token.clone(),
                sends: HashSet::new(),
            },
        );
    }
    let exec = TauriExecutor {
        state,
        app,
        run_id: &run_id,
        token,
        environment,
        counter: AtomicUsize::new(0),
        workspace: workspace_key(state),
    };
    let start = state.runs.session_runtime(state);
    let (report, runtime) = runner::run(&exec, &list, &options, start, |progress| {
        let event = RunProgress {
            run_id: run_id.clone(),
            progress,
        };
        if let Err(e) = event.emit(app) {
            tracing::warn!(error = %e, "couldn't emit run progress");
        }
    })
    .await;
    state.runs.active.lock().expect("runs lock").remove(&run_id);
    // Only what the run extracted, into the workspace it ran in: values cleared or set
    // by other sends while it ran stay as they are.
    state
        .runs
        .session
        .lock()
        .expect("runs lock")
        .entry(exec.workspace.clone())
        .or_default()
        .merge(&runtime);
    tracing::info!(
        requests = list.len(),
        passed = report.passed,
        failed = report.failed,
        errors = report.errors,
        cancelled = report.cancelled,
        "collection run finished"
    );
    let mut reports = state.runs.reports.lock().expect("runs lock");
    reports.insert(run_id, (title, report.clone()));
    while reports.len() > KEEP_REPORTS {
        reports.shift_remove_index(0);
    }
    Ok(report)
}

/// Cancels a run: in-flight requests are cancelled and nothing else starts.
#[tauri::command]
#[specta::specta]
pub fn run_cancel(state: tauri::State<'_, AppState>, run_id: String) {
    cancel(&state, &run_id);
}

fn cancel(state: &AppState, run_id: &str) {
    let sends: Vec<String> = {
        let active = state.runs.active.lock().expect("runs lock");
        let Some(run) = active.get(run_id) else {
            return;
        };
        run.token.cancel();
        run.sends.iter().cloned().collect()
    };
    for send_id in sends {
        state.sender.cancel(&send_id);
    }
}

/// A finished run's results as Markdown, for sharing.
#[tauri::command]
#[specta::specta]
pub fn run_report_markdown(state: tauri::State<'_, AppState>, run_id: String) -> Option<String> {
    let reports = state.runs.reports.lock().expect("runs lock");
    reports
        .get(&run_id)
        .map(|(title, report)| runner::markdown(report, title))
}

/// The session's runtime variables (values from `extract`). Sensitive values are masked
/// unless `reveal`.
#[tauri::command]
#[specta::specta]
pub fn runtime_vars(state: tauri::State<'_, AppState>, reveal: bool) -> Vec<RuntimeVarView> {
    state.runs.session_runtime(&state).views(reveal)
}

/// Removes one runtime variable, or all of them if `name` is `None`.
#[tauri::command]
#[specta::specta]
pub fn runtime_vars_clear(state: tauri::State<'_, AppState>, name: Option<String>) {
    state.runs.with_session(&state, |rt| match name {
        Some(name) => rt.remove(&name),
        None => rt.clear(),
    });
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;

    use axum::Router;
    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use ironcall_core::model::{
        AssertOp, AssertTarget, Assertion, ExtractSource, Extraction, KeyValue, RequestFile,
    };
    use ironcall_secrets::ExposeSecret;
    use serde_json::json;

    use super::*;
    use crate::test_support::{Fixture, fixture};

    const TOKEN: &str = "tok-e2e-very-secret";

    #[derive(Default)]
    struct Server {
        users: Mutex<Vec<String>>,
    }

    fn authed(h: &HeaderMap) -> bool {
        h.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {TOKEN}"))
    }

    /// login → create → fetch → delete, with a slow endpoint for cancelling.
    async fn serve() -> SocketAddr {
        let app = Router::new()
            .route(
                "/login",
                post(|| async { axum::Json(json!({"access_token": TOKEN})) }),
            )
            .route(
                "/users",
                post(|State(s): State<Arc<Server>>, h: HeaderMap| async move {
                    if !authed(&h) {
                        return (StatusCode::UNAUTHORIZED, axum::Json(json!({})));
                    }
                    let id = format!("u{}", s.users.lock().unwrap().len() + 1);
                    s.users.lock().unwrap().push(id.clone());
                    (StatusCode::CREATED, axum::Json(json!({"id": id})))
                }),
            )
            .route(
                "/users/{id}",
                get(
                    |State(s): State<Arc<Server>>, Path(id): Path<String>, h: HeaderMap| async move {
                        let found = authed(&h) && s.users.lock().unwrap().contains(&id);
                        if found {
                            (StatusCode::OK, axum::Json(json!({"id": id})))
                        } else {
                            (StatusCode::NOT_FOUND, axum::Json(json!({})))
                        }
                    },
                )
                .delete(
                    |State(s): State<Arc<Server>>, Path(id): Path<String>| async move {
                        s.users.lock().unwrap().retain(|u| u != &id);
                        StatusCode::NO_CONTENT
                    },
                ),
            )
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    axum::Json(json!({"id": "never"}))
                }),
            )
            .with_state(Arc::new(Server::default()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    fn status(code: u16) -> Assertion {
        Assertion {
            target: AssertTarget::Status,
            path: None,
            name: None,
            op: AssertOp::Eq,
            value: Some(json!(code)),
            enabled: true,
            extra: Default::default(),
        }
    }

    fn take(var: &str, path: &str) -> Extraction {
        Extraction {
            var: var.into(),
            from: ExtractSource::Body,
            path: Some(path.into()),
            name: None,
            pattern: None,
            sensitive: false,
            extra: Default::default(),
        }
    }

    /// Adds requests to the workspace root, in order.
    fn add(f: &Fixture, requests: Vec<RequestFile>) {
        let ws = f.state.workspaces.current().unwrap();
        let mut ws = ws.lock().unwrap();
        for (i, mut r) in requests.into_iter().enumerate() {
            let root = ws.tree().id;
            let created = ws.create_request(root, &r.name, Some(i)).unwrap();
            let loaded = ws.load_request(created.node.id).unwrap();
            r.id = created.node.id;
            ws.save_request(&r, loaded.hash.as_deref(), false).unwrap();
        }
    }

    fn request(name: &str, method: &str, url: String) -> RequestFile {
        let mut r = RequestFile::new(name);
        r.method = method.into();
        r.url = url;
        r
    }

    fn crud(addr: SocketAddr, fetch_url: &str) -> Vec<RequestFile> {
        let base = format!("http://{addr}");
        let bearer = || vec![KeyValue::new("Authorization", "Bearer {{authToken}}")];
        let mut login = request("Login", "POST", format!("{base}/login"));
        login.assert = vec![status(200)];
        login.extract = vec![take("authToken", "$.access_token")];
        let mut create = request("Create user", "POST", format!("{base}/users"));
        create.headers = bearer();
        create.assert = vec![status(201)];
        create.extract = vec![take("userId", "$.id")];
        let mut fetch = request("Fetch user", "GET", format!("{base}{fetch_url}"));
        fetch.headers = bearer();
        fetch.assert = vec![
            status(200),
            Assertion {
                path: Some("$.id".into()),
                ..Assertion {
                    target: AssertTarget::Body,
                    value: Some(json!("{{userId}}")),
                    ..status(0)
                }
            },
        ];
        let mut delete = request("Delete user", "DELETE", format!("{base}/users/{{{{userId}}}}"));
        delete.headers = bearer();
        delete.assert = vec![status(204)];
        vec![login, create, fetch, delete]
    }

    fn whole() -> RunTarget {
        RunTarget { folder: None }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn chained_crud_run_passes_and_never_stores_the_token() {
        let addr = serve().await;
        let f = fixture();
        add(&f, crud(addr, "/users/{{userId}}"));
        let state = &f.state;

        let titles: Vec<_> = items(state, &whole())
            .unwrap()
            .1
            .into_iter()
            .map(|i| i.name)
            .collect();
        assert_eq!(titles, ["Login", "Create user", "Fetch user", "Delete user"]);

        let report = run_in(
            state,
            f.app.handle(),
            "r1".into(),
            whole(),
            EnvChoice::Active,
            RunOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            (report.passed, report.failed, report.errors),
            (4, 0, 0),
            "{report:#?}"
        );
        assert!(report.results.iter().all(|r| r.history_id.is_some()));
        assert_eq!(
            report.results[0].extracted[0].value.as_deref(),
            Some("••••"),
            "token masked"
        );

        // What the run extracted stays in the session for later sends.
        let session = state.runs.session_runtime(state);
        assert_eq!(session.get("userId").unwrap().expose_secret(), "u1");
        assert!(
            state
                .runs
                .session_runtime(state)
                .views(false)
                .iter()
                .all(|v| !v.value.contains(TOKEN))
        );

        let md = state
            .runs
            .reports
            .lock()
            .unwrap()
            .get("r1")
            .map(|(t, r)| runner::markdown(r, t))
            .unwrap();
        assert!(md.contains("**4 passed, 0 failed, 0 errors**"), "{md}");

        // The extracted token is sensitive: masked in history, logs and the workspace.
        let (rows, _) = state
            .history
            .list(&ironcall_store::history::Query {
                limit: 50,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 4);
        for row in rows {
            let all = format!("{:?}", state.history.get(&row.id).unwrap().unwrap());
            assert!(
                !all.contains(TOKEN),
                "{}: token leaked into history",
                row.request_name
            );
        }
        for dir in [&f.root, &f.data] {
            for entry in walk(dir) {
                if entry.to_string_lossy().contains(".sqlite") {
                    // History stores response bodies (the login body holds the token by
                    // design); its request and metadata columns are checked above.
                    continue;
                }
                let bytes = std::fs::read(&entry).unwrap_or_default();
                assert!(
                    !String::from_utf8_lossy(&bytes).contains(TOKEN)
                        || entry.to_string_lossy().contains("spool"),
                    "token in {}",
                    entry.display()
                );
            }
        }
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancelling_keeps_only_values_from_finished_requests() {
        let addr = serve().await;
        let f = fixture();
        let mut requests = crud(addr, "/slow");
        // Fetch now hits /slow, and extracts something that must never arrive.
        requests[2].extract = vec![take("fetched", "$.id")];
        add(&f, requests);
        let state = &f.state;

        let run = run_in(
            state,
            f.app.handle(),
            "r2".into(),
            whole(),
            EnvChoice::Active,
            RunOptions::default(),
        );
        let cancel_later = async {
            // Wait until the slow request is in flight, then cancel.
            for _ in 0..200 {
                tokio::time::sleep(Duration::from_millis(25)).await;
                let sends = state.runs.active.lock().unwrap().get("r2").map(|a| a.sends.len());
                if state.runs.session_runtime(state).is_empty()
                    && sends == Some(1)
                    && state
                        .history
                        .list(&Default::default())
                        .map(|(r, _)| r.len())
                        .unwrap_or(0)
                        >= 2
                {
                    break;
                }
            }
            cancel(state, "r2");
        };
        let (report, ()) = tokio::join!(run, cancel_later);
        let report = report.unwrap();
        assert!(report.cancelled, "{report:#?}");
        let outcomes: Vec<_> = report.results.iter().map(|r| r.outcome).collect();
        assert_eq!(
            outcomes,
            [
                runner::Outcome::Passed,
                runner::Outcome::Passed,
                runner::Outcome::Cancelled
            ],
            "delete never started"
        );
        let session = state.runs.session_runtime(state);
        assert!(session.get("userId").is_some());
        assert!(
            session.get("fetched").is_none(),
            "the cancelled request extracted nothing"
        );
        assert!(state.runs.active.lock().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_run_only_adds_what_it_extracted_to_the_session() {
        let addr = serve().await;
        let f = fixture();
        add(&f, crud(addr, "/slow"));
        let state = &f.state;
        state.runs.with_session(state, |rt| {
            rt.set("old", ironcall_secrets::SecretString::from("1"), false)
        });

        let run = run_in(
            state,
            f.app.handle(),
            "r3".into(),
            whole(),
            EnvChoice::Active,
            RunOptions::default(),
        );
        let meanwhile = async {
            // While the slow request is in flight, the user clears the runtime variables.
            for _ in 0..200 {
                tokio::time::sleep(Duration::from_millis(25)).await;
                if state
                    .history
                    .list(&Default::default())
                    .map(|(r, _)| r.len())
                    .unwrap_or(0)
                    >= 2
                {
                    break;
                }
            }
            state.runs.with_session(state, |rt| rt.clear());
            cancel(state, "r3");
        };
        let (report, ()) = tokio::join!(run, meanwhile);
        assert!(report.unwrap().cancelled);
        let session = state.runs.session_runtime(state);
        assert!(session.get("old").is_none(), "a cleared value isn't brought back");
        assert!(session.get("userId").is_some(), "what the run extracted is kept");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn single_sends_check_and_extract_into_the_session() {
        let addr = serve().await;
        let f = fixture();
        let state = &f.state;
        let login = crud(addr, "")[0].clone();
        let (result, checked) =
            crate::send::send_in(state, f.app.handle(), "s1".into(), &login, &Scope::Session, None).await;
        let summary = result.unwrap();
        let checks = summary.checks.unwrap();
        assert!(checks.assertions[0].passed);
        assert_eq!(checks.extracted[0].value.as_deref(), Some("••••"));
        state.runs.apply_to_session(state, &checked.unwrap().extracted);

        // A runtime variable wins over every other scope, and stays masked.
        let views = state.runs.session_runtime(state).views(false);
        assert_eq!(views[0].name, "authToken");
        assert_eq!(views[0].value, "••••");
        let probe = request("Probe", "GET", "{{authToken}}".into());
        let (resolver, _) = crate::vars::resolver_for(state, &probe);
        assert_eq!(resolver.resolve_str("{{authToken}}").text, TOKEN);

        // No assertions or extractions: no checks.
        let plain = request("Plain", "POST", format!("http://{addr}/login"));
        let (result, checked) =
            crate::send::send_in(state, f.app.handle(), "s2".into(), &plain, &Scope::Session, None).await;
        assert!(result.unwrap().checks.is_none() && checked.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn runs_use_the_chosen_environment_and_folder() {
        let addr = serve().await;
        let f = fixture();
        let state = &f.state;
        let env_id = {
            let ws = state.workspaces.current().unwrap();
            let mut ws = ws.lock().unwrap();
            let env = ws.create_environment("Local").unwrap();
            let mut file = ws.load_environment(env.id).unwrap();
            file.file.vars.insert(
                "base".into(),
                ironcall_core::model::Variable::Plain(format!("http://{addr}")),
            );
            ws.save_environment(&file.file, file.hash.as_deref(), false)
                .unwrap();
            let root = ws.tree().id;
            let folder = ws.create_folder(root, "Auth", None).unwrap().node.id;
            let created = ws.create_request(folder, "Login", None).unwrap();
            let loaded = ws.load_request(created.node.id).unwrap();
            let mut login = crud(addr, "")[0].clone();
            login.id = created.node.id;
            login.url = "{{base}}/login".into();
            ws.save_request(&login, loaded.hash.as_deref(), false).unwrap();
            (env.id, folder)
        };
        let (env, folder) = env_id;
        let target = RunTarget { folder: Some(folder) };
        let list = items(state, &target).unwrap().1;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].folder, "", "breadcrumbs are relative to the run's folder");
        assert_eq!(items(state, &whole()).unwrap().1[0].folder, "Auth");

        let without = run_in(
            state,
            f.app.handle(),
            "e1".into(),
            target.clone(),
            EnvChoice::None,
            RunOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(without.errors, 1, "no environment: {{base}} is unresolved");
        let with = run_in(
            state,
            f.app.handle(),
            "e2".into(),
            target,
            EnvChoice::Id { id: env },
            RunOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(with.passed, 1, "{with:#?}");
        assert!(
            items(
                state,
                &RunTarget {
                    folder: Some(ItemId::new())
                }
            )
            .is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_sample_workspace_passes_against_the_echo_server() {
        let echo = ironcall_http::echo::start().await.unwrap();
        ironcall_core::vars::set_local_echo(echo.base_url());
        let f = fixture();
        let dir = tempfile::tempdir().unwrap();
        let ws = ironcall_store::sample::create(dir.path()).unwrap();
        f.state
            .workspaces
            .install(f.app.handle(), &f.state.path_guard, ws)
            .unwrap();

        let report = run_in(
            &f.state,
            f.app.handle(),
            "sample".into(),
            whole(),
            EnvChoice::Active,
            RunOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            (report.passed, report.failed, report.errors),
            (7, 0, 0),
            "{report:#?}"
        );
        // The token was chained, and stays masked.
        let views = f.state.runs.session_runtime(&f.state).views(false);
        assert!(views.iter().any(|v| v.name == "authToken" && v.value == "••••"));
    }
}
