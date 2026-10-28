//! The collection runner (PLAN.md Phase 7): sends a list of requests in order, for a
//! number of iterations, checking each response and chaining extracted values.
//!
//! Sending is delegated to an [`Executor`], so this module needs no HTTP stack or async
//! runtime. A request's extractions are applied only after it has fully finished, so a
//! cancelled run never leaves values from requests that didn't complete.

use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use futures_util::stream;
use serde::{Deserialize, Serialize};

use crate::ItemId;
use crate::assertions::AssertionResult;
use crate::extract::{Extracted, ExtractedView};
use crate::runtime::RuntimeVars;

/// How many requests run at once in parallel mode.
pub const PARALLEL_LIMIT: usize = 6;

/// One request to run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RunItem {
    pub id: ItemId,
    pub name: String,
    /// Folder breadcrumb, e.g. `Users / Admin`. Empty at the top level.
    pub folder: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RunOptions {
    /// At least 1.
    pub iterations: u32,
    /// Pause between requests (sequential) or between iterations (parallel).
    #[cfg_attr(feature = "specta", specta(type = specta_typescript::Number))]
    pub delay_ms: u64,
    pub stop_on_failure: bool,
    /// Send each iteration's requests at once (up to [`PARALLEL_LIMIT`]) instead of in order.
    /// Chaining between requests of one iteration isn't reliable in this mode.
    pub parallel: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            iterations: 1,
            delay_ms: 0,
            stop_on_failure: false,
            parallel: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    /// Every assertion passed and every extraction succeeded.
    Passed,
    /// A response came back, but an assertion or extraction failed.
    Failed,
    /// No response: connection error, timeout, invalid request…
    Error,
    Cancelled,
}

/// The result of one request in one iteration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ItemResult {
    pub item: RunItem,
    /// 1-based.
    pub iteration: u32,
    /// Position in the run's item list.
    #[cfg_attr(feature = "specta", specta(type = specta_typescript::Number))]
    pub index: usize,
    pub outcome: Outcome,
    pub status: Option<u16>,
    pub duration_ms: Option<f64>,
    pub error: Option<String>,
    pub assertions: Vec<AssertionResult>,
    pub extracted: Vec<ExtractedView>,
    /// History entry for drill-down.
    pub history_id: Option<String>,
}

/// What the executor reports for a request that got a response.
#[derive(Debug)]
pub struct Executed {
    pub status: u16,
    pub duration_ms: f64,
    pub history_id: Option<String>,
    pub assertions: Vec<AssertionResult>,
    pub extracted: Vec<Extracted>,
}

#[derive(Debug)]
pub enum ExecError {
    Cancelled,
    /// No response. `history_id` if the attempt was recorded.
    Failed {
        message: String,
        history_id: Option<String>,
    },
}

/// Sends requests for the runner.
pub trait Executor: Sync {
    /// Loads, resolves (with `runtime` as the runtime scope) and sends one request, then
    /// checks the response (see [`crate::checks::check_response`]).
    fn execute(
        &self,
        item: &RunItem,
        runtime: RuntimeVars,
    ) -> impl Future<Output = Result<Executed, ExecError>> + Send;
    /// Waits, returning early if the run is cancelled.
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;
    fn cancelled(&self) -> bool;
}

/// Progress, reported as the run goes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Progress {
    Started {
        iteration: u32,
        #[cfg_attr(feature = "specta", specta(type = specta_typescript::Number))]
        index: usize,
    },
    Finished {
        result: Box<ItemResult>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RunReport {
    pub results: Vec<ItemResult>,
    #[cfg_attr(feature = "specta", specta(type = specta_typescript::Number))]
    pub passed: usize,
    #[cfg_attr(feature = "specta", specta(type = specta_typescript::Number))]
    pub failed: usize,
    #[cfg_attr(feature = "specta", specta(type = specta_typescript::Number))]
    pub errors: usize,
    /// The user cancelled the run.
    pub cancelled: bool,
    /// `stopOnFailure` stopped the run.
    pub stopped_early: bool,
    pub total_ms: f64,
}

/// Runs `items` starting from the `runtime` scope, and returns the report plus the
/// values the run extracted (only from requests that finished). Merge those into the
/// caller's scope; the starting values aren't returned, so a run never overwrites
/// changes made to the scope while it ran.
pub async fn run<E: Executor>(
    exec: &E,
    items: &[RunItem],
    options: &RunOptions,
    runtime: RuntimeVars,
    on_progress: impl Fn(Progress) + Sync,
) -> (RunReport, RuntimeVars) {
    let started = Instant::now();
    let runtime = Mutex::new(runtime);
    let extracted_by_run = Mutex::new(RuntimeVars::new());
    let results = Mutex::new(Vec::new());
    let stop = Mutex::new(false);
    let iterations = options.iterations.max(1);
    let delay = Duration::from_millis(options.delay_ms);

    let one = |iteration: u32, index: usize| {
        let (runtime, extracted_by_run, results, stop, on_progress) =
            (&runtime, &extracted_by_run, &results, &stop, &on_progress);
        async move {
            on_progress(Progress::Started { iteration, index });
            let item = &items[index];
            let snapshot = runtime.lock().expect("runtime lock").clone();
            let result = match exec.execute(item, snapshot).await {
                Ok(done) => {
                    let failed = !(done.assertions.iter().all(|a| a.passed)
                        && done.extracted.iter().all(|e| e.value.is_ok()));
                    // Only now, with the response fully received and checked.
                    runtime.lock().expect("runtime lock").apply(&done.extracted);
                    extracted_by_run
                        .lock()
                        .expect("runtime lock")
                        .apply(&done.extracted);
                    ItemResult {
                        item: item.clone(),
                        iteration,
                        index,
                        outcome: if failed { Outcome::Failed } else { Outcome::Passed },
                        status: Some(done.status),
                        duration_ms: Some(done.duration_ms),
                        error: None,
                        assertions: done.assertions,
                        extracted: done.extracted.iter().map(Extracted::view).collect(),
                        history_id: done.history_id,
                    }
                }
                Err(e) => {
                    let (outcome, error, history_id) = match e {
                        ExecError::Cancelled => (Outcome::Cancelled, None, None),
                        ExecError::Failed { message, history_id } => {
                            (Outcome::Error, Some(message), history_id)
                        }
                    };
                    ItemResult {
                        item: item.clone(),
                        iteration,
                        index,
                        outcome,
                        status: None,
                        duration_ms: None,
                        error,
                        assertions: Vec::new(),
                        extracted: Vec::new(),
                        history_id,
                    }
                }
            };
            if options.stop_on_failure && matches!(result.outcome, Outcome::Failed | Outcome::Error) {
                *stop.lock().expect("stop lock") = true;
            }
            on_progress(Progress::Finished {
                result: Box::new(result.clone()),
            });
            results.lock().expect("results lock").push(result);
        }
    };
    let halted = || exec.cancelled() || *stop.lock().expect("stop lock");

    let mut first = true;
    'run: for iteration in 1..=iterations {
        if options.parallel {
            if !first && !delay.is_zero() {
                exec.sleep(delay).await;
            }
            first = false;
            if halted() {
                break;
            }
            stream::iter(0..items.len())
                .filter(|_| std::future::ready(!halted()))
                .map(|index| one(iteration, index))
                .buffer_unordered(PARALLEL_LIMIT)
                .collect::<()>()
                .await;
        } else {
            for index in 0..items.len() {
                if !first && !delay.is_zero() {
                    exec.sleep(delay).await;
                }
                first = false;
                if halted() {
                    break 'run;
                }
                one(iteration, index).await;
            }
        }
        if halted() {
            break;
        }
    }

    let mut results = results.into_inner().expect("results lock");
    // Parallel runs finish out of order; report in run order.
    results.sort_by_key(|r| (r.iteration, r.index));
    let count = |o: Outcome| results.iter().filter(|r| r.outcome == o).count();
    let report = RunReport {
        passed: count(Outcome::Passed),
        failed: count(Outcome::Failed),
        errors: count(Outcome::Error),
        cancelled: exec.cancelled(),
        stopped_early: *stop.lock().expect("stop lock") && !exec.cancelled(),
        total_ms: started.elapsed().as_secs_f64() * 1000.0,
        results,
    };
    drop(runtime);
    (report, extracted_by_run.into_inner().expect("runtime lock"))
}

/// The report as a Markdown table, for "Copy results as Markdown".
pub fn markdown(report: &RunReport, title: &str) -> String {
    let multi = report.results.iter().any(|r| r.iteration > 1);
    let mut out = format!("## Run: {}\n\n", cell(title));
    let mut summary = format!(
        "**{} passed, {} failed, {} errors** in {}",
        report.passed,
        report.failed,
        report.errors,
        duration(report.total_ms)
    );
    if report.cancelled {
        summary.push_str(" (cancelled)");
    } else if report.stopped_early {
        summary.push_str(" (stopped at the first failure)");
    }
    out.push_str(&summary);
    out.push_str("\n\n| # | Request | Status | Time | Result | Assertions |\n|---|---|---|---|---|---|\n");
    for r in &report.results {
        let num = if multi {
            format!("{}.{}", r.iteration, r.index + 1)
        } else {
            (r.index + 1).to_string()
        };
        let name = if r.item.folder.is_empty() {
            r.item.name.clone()
        } else {
            format!("{} / {}", r.item.folder, r.item.name)
        };
        let result = match r.outcome {
            Outcome::Passed => "✅ Passed",
            Outcome::Failed => "❌ Failed",
            Outcome::Error => "⚠️ Error",
            Outcome::Cancelled => "⏹ Cancelled",
        };
        let passed = r.assertions.iter().filter(|a| a.passed).count();
        let asserts = if r.assertions.is_empty() {
            "–".to_string()
        } else {
            format!("{passed}/{}", r.assertions.len())
        };
        out.push_str(&format!(
            "| {num} | {} | {} | {} | {result} | {asserts} |\n",
            cell(&name),
            r.status.map(|s| s.to_string()).unwrap_or_else(|| "–".into()),
            r.duration_ms.map(duration).unwrap_or_else(|| "–".into()),
        ));
    }
    let problems: Vec<String> = report
        .results
        .iter()
        .filter(|r| matches!(r.outcome, Outcome::Failed | Outcome::Error))
        .flat_map(|r| {
            let who = format!("{} (#{})", r.item.name, r.index + 1);
            let mut lines: Vec<String> = r
                .assertions
                .iter()
                .filter(|a| !a.passed)
                .map(|a| {
                    format!(
                        "- {who}: `{}`: {}",
                        a.label.replace('`', "'"),
                        a.message.clone().unwrap_or_default()
                    )
                })
                .collect();
            lines.extend(r.extracted.iter().filter_map(|e| {
                e.error
                    .as_ref()
                    .map(|err| format!("- {who}: extract `{}`: {err}", e.var))
            }));
            lines.extend(r.error.iter().map(|e| format!("- {who}: {e}")));
            lines
        })
        .collect();
    if !problems.is_empty() {
        out.push_str("\n**Failures**\n\n");
        out.push_str(&problems.join("\n"));
        out.push('\n');
    }
    out
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn duration(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else {
        format!("{} ms", ms.round())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AssertOp, AssertTarget, Assertion, ExtractSource, Extraction, RequestFile};
    use crate::response::{BodyText, ResponseData};
    use crate::vars::Resolver;
    use secrecy::{ExposeSecret, SecretString};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    type Handler = Box<dyn Fn(&str, &RuntimeVars) -> Result<(u16, String), String> + Sync>;

    /// A fake server: each request name maps to a handler that gets the runtime scope.
    struct Fake {
        requests: HashMap<String, RequestFile>,
        handler: Handler,
        cancel_at: Option<usize>,
        calls: AtomicUsize,
        cancelled: AtomicBool,
        in_flight: AtomicUsize,
        max_in_flight: AtomicUsize,
        log: Mutex<Vec<String>>,
        slept: Mutex<Vec<Duration>>,
    }

    impl Fake {
        fn new(
            handler: impl Fn(&str, &RuntimeVars) -> Result<(u16, String), String> + Sync + 'static,
        ) -> Self {
            Self {
                requests: HashMap::new(),
                handler: Box::new(handler),
                cancel_at: None,
                calls: AtomicUsize::new(0),
                cancelled: AtomicBool::new(false),
                in_flight: AtomicUsize::new(0),
                max_in_flight: AtomicUsize::new(0),
                log: Mutex::new(Vec::new()),
                slept: Mutex::new(Vec::new()),
            }
        }

        fn with(mut self, r: RequestFile) -> Self {
            self.requests.insert(r.name.clone(), r);
            self
        }

        fn items(&self, names: &[&str]) -> Vec<RunItem> {
            names
                .iter()
                .map(|n| RunItem {
                    id: self.requests.get(*n).map(|r| r.id).unwrap_or_default(),
                    name: n.to_string(),
                    folder: String::new(),
                })
                .collect()
        }
    }

    impl Executor for Fake {
        async fn execute(&self, item: &RunItem, runtime: RuntimeVars) -> Result<Executed, ExecError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight.fetch_max(now, Ordering::SeqCst);
            // Let other parallel requests start.
            for _ in 0..3 {
                yield_now().await;
            }
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            self.log.lock().unwrap().push(item.name.clone());
            if self.cancel_at == Some(n) {
                self.cancelled.store(true, Ordering::SeqCst);
                return Err(ExecError::Cancelled);
            }
            let request = self
                .requests
                .get(&item.name)
                .cloned()
                .unwrap_or_else(|| RequestFile::new(&item.name));
            let (status, body) =
                (self.handler)(&item.name, &runtime).map_err(|message| ExecError::Failed {
                    message,
                    history_id: Some(format!("h{n}")),
                })?;
            let response = ResponseData::new(status, vec![], 10.0, body.len() as u64, BodyText::Text(body));
            let resolver = Resolver::new(vec![runtime.layer()]);
            let checked = crate::checks::check_response(&request, &resolver, &response, &[]);
            Ok(Executed {
                status,
                duration_ms: 10.0,
                history_id: Some(format!("h{n}")),
                assertions: checked.assertions,
                extracted: checked.extracted,
            })
        }

        async fn sleep(&self, d: Duration) {
            self.slept.lock().unwrap().push(d);
        }

        fn cancelled(&self) -> bool {
            self.cancelled.load(Ordering::SeqCst)
        }
    }

    /// Yields once, so concurrent futures interleave (no runtime needed).
    fn yield_now() -> impl Future<Output = ()> {
        let mut yielded = false;
        std::future::poll_fn(move |cx| {
            if yielded {
                std::task::Poll::Ready(())
            } else {
                yielded = true;
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        })
    }

    /// Polls to completion. The fake executor never blocks, so a busy loop is enough.
    fn poll_to_end<F: Future>(f: F) -> F::Output {
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        let mut f = std::pin::pin!(f);
        loop {
            if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    fn req(name: &str, asserts: Vec<Assertion>, extract: Vec<Extraction>) -> RequestFile {
        let mut r = RequestFile::new(name);
        r.assert = asserts;
        r.extract = extract;
        r
    }

    fn status_is(code: u16) -> Assertion {
        Assertion {
            target: AssertTarget::Status,
            path: None,
            name: None,
            op: AssertOp::Eq,
            value: Some(serde_json::json!(code)),
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

    fn crud() -> Fake {
        Fake::new(|name, rt| {
            let get = |k: &str| rt.get(k).map(|v| v.expose_secret().to_string());
            Ok(match name {
                "login" => (200, r#"{"token":"t-1"}"#.into()),
                "create" if get("authToken").as_deref() == Some("t-1") => (201, r#"{"id":"u-9"}"#.into()),
                "fetch" if get("userId").as_deref() == Some("u-9") => (200, r#"{"id":"u-9"}"#.into()),
                "delete" if get("userId").as_deref() == Some("u-9") => (204, String::new()),
                _ => (401, "{}".into()),
            })
        })
        .with(req(
            "login",
            vec![status_is(200)],
            vec![take("authToken", "$.token")],
        ))
        .with(req("create", vec![status_is(201)], vec![take("userId", "$.id")]))
        .with(req("fetch", vec![status_is(200)], vec![]))
        .with(req("delete", vec![status_is(204)], vec![]))
    }

    #[test]
    fn chained_crud_flow_passes_and_keeps_extracted_values() {
        let fake = crud();
        let items = fake.items(&["login", "create", "fetch", "delete"]);
        let events = Mutex::new(Vec::new());
        let (report, runtime) = poll_to_end(run(
            &fake,
            &items,
            &RunOptions::default(),
            RuntimeVars::new(),
            |p| events.lock().unwrap().push(p),
        ));
        assert_eq!(
            (report.passed, report.failed, report.errors),
            (4, 0, 0),
            "{report:#?}"
        );
        assert!(!report.cancelled && !report.stopped_early);
        assert_eq!(runtime.get("userId").unwrap().expose_secret(), "u-9");
        assert_eq!(
            report.results[0].extracted[0].value.as_deref(),
            Some("••••"),
            "token is masked"
        );
        assert_eq!(report.results[1].history_id.as_deref(), Some("h1"));
        assert_eq!(
            events.lock().unwrap().len(),
            8,
            "a start and a finish per request"
        );
    }

    #[test]
    fn failures_and_stop_on_failure() {
        let fake = crud();
        // Without login, create gets 401 and fails its assertion; fetch then has no userId.
        let items = fake.items(&["create", "fetch", "delete"]);
        let (report, _) = poll_to_end(run(
            &fake,
            &items,
            &RunOptions::default(),
            RuntimeVars::new(),
            |_| {},
        ));
        assert_eq!(report.failed, 3);
        assert!(!report.stopped_early);
        let create = &report.results[0];
        assert_eq!(create.outcome, Outcome::Failed);
        assert_eq!(create.status, Some(401));
        assert!(create.extracted[0].error.is_some());

        let fake = crud();
        let options = RunOptions {
            stop_on_failure: true,
            iterations: 3,
            ..RunOptions::default()
        };
        let (report, _) = poll_to_end(run(&fake, &items, &options, RuntimeVars::new(), |_| {}));
        assert_eq!(report.results.len(), 1);
        assert!(report.stopped_early);
    }

    #[test]
    fn errors_are_reported_with_their_history_entry() {
        let fake = Fake::new(|_, _| Err("connection refused".into()));
        let items = fake.items(&["a"]);
        let (report, _) = poll_to_end(run(
            &fake,
            &items,
            &RunOptions::default(),
            RuntimeVars::new(),
            |_| {},
        ));
        assert_eq!(report.errors, 1);
        assert_eq!(report.results[0].outcome, Outcome::Error);
        assert_eq!(report.results[0].error.as_deref(), Some("connection refused"));
        assert_eq!(report.results[0].history_id.as_deref(), Some("h0"));
    }

    #[test]
    fn cancelling_leaves_no_values_from_unfinished_requests() {
        let mut fake = crud();
        fake.cancel_at = Some(1); // `create` is cancelled mid-flight
        let items = fake.items(&["login", "create", "fetch", "delete"]);
        let mut start = RuntimeVars::new();
        start.set("userId", SecretString::from("stale"), false);
        let (report, runtime) = poll_to_end(run(&fake, &items, &RunOptions::default(), start, |_| {}));
        assert!(report.cancelled);
        assert_eq!(report.results.len(), 2, "nothing starts after the cancel");
        assert_eq!(report.results[1].outcome, Outcome::Cancelled);
        assert_eq!(
            runtime.get("authToken").unwrap().expose_secret(),
            "t-1",
            "login finished"
        );
        assert!(
            runtime.get("userId").is_none(),
            "create didn't finish, and starting values aren't returned"
        );
        assert_eq!(*fake.log.lock().unwrap(), ["login", "create"]);
    }

    #[test]
    fn iterations_and_delays() {
        let fake = crud();
        let items = fake.items(&["login", "create"]);
        let options = RunOptions {
            iterations: 3,
            delay_ms: 50,
            ..RunOptions::default()
        };
        let (report, _) = poll_to_end(run(&fake, &items, &options, RuntimeVars::new(), |_| {}));
        assert_eq!(report.results.len(), 6);
        assert_eq!(report.results[5].iteration, 3);
        assert_eq!(
            fake.slept.lock().unwrap().len(),
            5,
            "between requests, not before the first"
        );
        let zero = crud();
        poll_to_end(run(
            &zero,
            &items,
            &RunOptions {
                iterations: 0,
                ..options.clone()
            },
            RuntimeVars::new(),
            |_| {},
        ));
        assert_eq!(zero.calls.load(Ordering::SeqCst), 2, "0 iterations means 1");
    }

    #[test]
    fn parallel_mode_overlaps_and_reports_in_order() {
        let fake = Fake::new(|_, _| Ok((200, "{}".into())));
        let names: Vec<String> = (0..10).map(|i| format!("r{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let items = fake.items(&refs);
        let options = RunOptions {
            parallel: true,
            iterations: 2,
            delay_ms: 5,
            ..RunOptions::default()
        };
        let (report, _) = poll_to_end(run(&fake, &items, &options, RuntimeVars::new(), |_| {}));
        assert_eq!(report.passed, 20);
        let max = fake.max_in_flight.load(Ordering::SeqCst);
        assert!(max > 1 && max <= PARALLEL_LIMIT, "max in flight {max}");
        let order: Vec<_> = report.results.iter().map(|r| (r.iteration, r.index)).collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted);
        assert_eq!(fake.slept.lock().unwrap().len(), 1, "delay between iterations");
    }

    #[test]
    fn markdown_report() {
        let fake = crud();
        let items = fake.items(&["create", "login"]);
        let (report, _) = poll_to_end(run(
            &fake,
            &items,
            &RunOptions::default(),
            RuntimeVars::new(),
            |_| {},
        ));
        let md = markdown(&report, "Users | smoke");
        assert!(md.starts_with("## Run: Users \\| smoke\n"), "{md}");
        assert!(md.contains("**1 passed, 1 failed, 0 errors**"), "{md}");
        assert!(
            md.contains("| 1 | create | 401 | 10 ms | ❌ Failed | 0/1 |"),
            "{md}"
        );
        assert!(
            md.contains("| 2 | login | 200 | 10 ms | ✅ Passed | 1/1 |"),
            "{md}"
        );
        assert!(
            md.contains("- create (#1): `status eq 201`: expected eq 201"),
            "{md}"
        );
        assert!(
            md.contains("- create (#1): extract `userId`: the JSONPath matched nothing"),
            "{md}"
        );
        assert!(!md.contains("t-1"));
    }
}
