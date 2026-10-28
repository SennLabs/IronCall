//! Body viewing commands (PLAN.md §3.1): open a body file, page its rows, search it.
//! Bodies never travel whole over IPC.

use std::path::PathBuf;
use std::time::Instant;

use ironcall_store::body::{self, BodyId};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use crate::AppState;

#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BodyInfo {
    pub id: BodyId,
    #[specta(type = Number)]
    pub row_count: usize,
    #[specta(type = Number)]
    pub byte_len: u64,
    pub index_ms: u32,
}

#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BodyRow {
    #[specta(type = Number)]
    pub line: u64,
    pub continuation: bool,
    pub text: String,
}

#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BodyHit {
    #[specta(type = Number)]
    pub row: usize,
    #[specta(type = Number)]
    pub offset: u64,
    #[specta(type = Number)]
    pub column: usize,
}

/// Runs blocking file work off the async runtime's worker threads.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Indexes a body file. The path must be inside an allowed location.
#[tauri::command]
#[specta::specta]
pub async fn open_body(state: tauri::State<'_, AppState>, path: String) -> Result<BodyInfo, String> {
    let path = state
        .path_guard
        .check(&PathBuf::from(path))
        .map_err(|e| e.to_string())?;
    let bodies = state.bodies.clone();
    blocking(move || {
        let start = Instant::now();
        let (id, body) = bodies.open(&path)?;
        Ok(BodyInfo {
            id,
            row_count: body.index().row_count(),
            byte_len: body.index().byte_len(),
            index_ms: start.elapsed().as_millis() as u32,
        })
    })
    .await
}

/// Returns up to `count` rows starting at row `from`.
#[tauri::command]
#[specta::specta]
pub async fn get_body_lines(
    state: tauri::State<'_, AppState>,
    id: BodyId,
    from: u32,
    count: u32,
) -> Result<Vec<BodyRow>, String> {
    let body = state.bodies.get(id).ok_or("unknown body")?;
    blocking(move || {
        let rows = body.rows(from as usize, count as usize)?;
        Ok(rows
            .into_iter()
            .map(|r| BodyRow {
                line: r.line,
                continuation: r.continuation,
                text: r.text,
            })
            .collect())
    })
    .await
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SearchDirection {
    /// The first match starting at or after `offset`.
    Forward,
    /// The last match ending before `offset`.
    Backward,
}

#[derive(Debug, Serialize, Type)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum SearchResult {
    Found {
        hit: BodyHit,
    },
    NotFound,
    /// A newer search on the same body started before this one finished.
    Cancelled,
}

/// Finds the next or previous match of `query` relative to byte `offset`. Starting a new
/// search on a body cancels any search still running on it. Case-insensitive matching
/// folds Unicode case (not just ASCII).
#[tauri::command]
#[specta::specta]
pub async fn search_body(
    state: tauri::State<'_, AppState>,
    id: BodyId,
    query: String,
    offset: f64,
    direction: SearchDirection,
    ignore_case: bool,
) -> Result<SearchResult, String> {
    let body = state.bodies.get(id).ok_or("unknown body")?;
    let cancel = state.searches.start(id);
    blocking(move || {
        let offset = offset.max(0.0) as u64;
        let outcome = match direction {
            SearchDirection::Forward => body.find_next(&query, offset, ignore_case, &cancel)?,
            SearchDirection::Backward => body.find_prev(&query, offset, ignore_case, &cancel)?,
        };
        Ok(match outcome {
            body::SearchOutcome::Found(h) => SearchResult::Found {
                hit: BodyHit {
                    row: h.row,
                    offset: h.offset,
                    column: h.column,
                },
            },
            body::SearchOutcome::NotFound => SearchResult::NotFound,
            body::SearchOutcome::Cancelled => SearchResult::Cancelled,
        })
    })
    .await
}

/// One cancel flag per body: starting a search cancels the previous one on that body.
#[derive(Default)]
pub struct Searches(std::sync::Mutex<std::collections::HashMap<BodyId, std::sync::Arc<AtomicBool>>>);

impl Searches {
    fn start(&self, id: BodyId) -> std::sync::Arc<AtomicBool> {
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        if let Some(previous) = self
            .0
            .lock()
            .expect("searches lock poisoned")
            .insert(id, flag.clone())
        {
            previous.store(true, Ordering::Relaxed);
        }
        flag
    }
}

#[tauri::command]
#[specta::specta]
pub fn close_body(state: tauri::State<'_, AppState>, id: BodyId) {
    // Also deletes the spooled files if this body is a response.
    state.sender.close_body(id);
}
