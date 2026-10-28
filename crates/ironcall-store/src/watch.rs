//! Watches an open workspace for external changes (PLAN.md Phase 2).
//!
//! Events are debounced (about 200ms), then filtered: IronCall's own writes (see
//! [`OwnWrites`]), temp files, `.ironcall/` and other dot-directories (such as `.git`)
//! are ignored. What remains is handed to a callback as a list of changed paths; the
//! caller then runs [`Workspace::rescan`](crate::workspace::Workspace::rescan) with them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::atomic::{OwnWrites, TEMP_PREFIX};

pub const DEBOUNCE: Duration = Duration::from_millis(200);

/// Keeps the watch running until dropped.
pub struct WorkspaceWatcher {
    _debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
}

impl std::fmt::Debug for WorkspaceWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceWatcher").finish_non_exhaustive()
    }
}

impl WorkspaceWatcher {
    /// Starts watching `root`. `on_change` runs on the watcher's thread with the
    /// external changes of each debounce window (never an empty list).
    pub fn start(
        root: &Path,
        own: Arc<OwnWrites>,
        on_change: impl Fn(Vec<PathBuf>) + Send + 'static,
    ) -> notify::Result<Self> {
        let root = std::fs::canonicalize(root)?;
        let filter_root = root.clone();
        let mut debouncer = new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
            let events = match result {
                Ok(events) => events,
                Err(errors) => {
                    for e in errors {
                        tracing::warn!(error = %e, "workspace watcher error");
                    }
                    return;
                }
            };
            let mut paths: Vec<PathBuf> = events
                .iter()
                .filter(|e| !e.kind.is_access())
                .flat_map(|e| e.paths.iter().cloned())
                .filter(|p| is_relevant(&filter_root, p) && !own.is_own(p))
                .collect();
            paths.sort();
            paths.dedup();
            if !paths.is_empty() {
                on_change(paths);
            }
        })?;
        debouncer.watch(&root, RecursiveMode::Recursive)?;
        Ok(Self {
            _debouncer: debouncer,
        })
    }
}

/// False for temp files and anything under a dot-directory (`.ironcall`, `.git`, …).
fn is_relevant(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    !rel.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s.starts_with('.') || s.starts_with(TEMP_PREFIX)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_dirs_and_temp_files_are_ignored() {
        let root = Path::new("/ws");
        assert!(is_relevant(root, Path::new("/ws/users/a.yaml")));
        assert!(!is_relevant(root, Path::new("/ws/.ironcall/local.yaml")));
        assert!(!is_relevant(root, Path::new("/ws/.git/index")));
        assert!(!is_relevant(
            root,
            Path::new("/ws/users/.ironcall-tmp-1-2-a.yaml")
        ));
        assert!(!is_relevant(root, Path::new("/elsewhere/a.yaml")));
    }
}
