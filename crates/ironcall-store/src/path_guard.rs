//! Restricts file access to allowed roots (PLAN.md §3.1, §9).
//!
//! Tauri capability scopes only restrict JS-side plugins, so every Rust command that
//! touches the file system must check its paths here first.

use std::path::{Component, Path, PathBuf};
use std::sync::RwLock;

#[derive(Debug, thiserror::Error)]
pub enum PathGuardError {
    #[error("path is outside the allowed locations: {0}")]
    OutsideAllowedRoots(PathBuf),
    #[error("path must be absolute: {0}")]
    NotAbsolute(PathBuf),
    #[error("cannot resolve path {path}: {source}")]
    Resolve {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Set of directories (open workspace, app data dir) and individual files (picked by
/// the user in a dialog) that commands may access.
#[derive(Debug, Default)]
pub struct PathGuard {
    roots: RwLock<Vec<PathBuf>>,
    files: RwLock<Vec<PathBuf>>,
}

impl PathGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allows access to everything under `dir`. The directory must exist.
    pub fn allow_dir(&self, dir: &Path) -> Result<(), PathGuardError> {
        let dir = canonical(dir)?;
        let mut roots = self.roots.write().expect("path guard lock poisoned");
        if !roots.contains(&dir) {
            roots.push(dir);
        }
        Ok(())
    }

    /// Stops allowing access under `dir` (e.g. when a workspace is closed).
    pub fn revoke_dir(&self, dir: &Path) {
        if let Ok(dir) = canonical(dir) {
            self.roots
                .write()
                .expect("path guard lock poisoned")
                .retain(|r| r != &dir);
        }
    }

    /// Allows access to one file the user picked explicitly. The file must exist.
    pub fn allow_file(&self, file: &Path) -> Result<(), PathGuardError> {
        let file = canonical(file)?;
        let mut files = self.files.write().expect("path guard lock poisoned");
        if !files.contains(&file) {
            files.push(file);
        }
        Ok(())
    }

    /// Whether `path` is a file the user explicitly picked this session (via
    /// [`PathGuard::allow_file`]). Allowed directories don't count.
    pub fn is_allowed_file(&self, path: &Path) -> bool {
        let Ok(resolved) = canonical(path) else {
            return false;
        };
        self.files
            .read()
            .expect("path guard lock poisoned")
            .contains(&resolved)
    }

    /// Checks that `path` is inside an allowed location and returns its resolved form.
    ///
    /// The path may not exist yet (e.g. a file about to be created), but its parent
    /// directory must. Symlinks are resolved before checking, so a link inside a
    /// workspace pointing outside it is rejected.
    pub fn check(&self, path: &Path) -> Result<PathBuf, PathGuardError> {
        if !path.is_absolute() {
            return Err(PathGuardError::NotAbsolute(path.to_path_buf()));
        }
        let resolved = resolve(path)?;

        let files = self.files.read().expect("path guard lock poisoned");
        if files.contains(&resolved) {
            return Ok(resolved);
        }
        let roots = self.roots.read().expect("path guard lock poisoned");
        if roots.iter().any(|root| resolved.starts_with(root)) {
            return Ok(resolved);
        }
        Err(PathGuardError::OutsideAllowedRoots(path.to_path_buf()))
    }
}

fn canonical(path: &Path) -> Result<PathBuf, PathGuardError> {
    dunce_canonicalize(path).map_err(|source| PathGuardError::Resolve {
        path: path.to_path_buf(),
        source,
    })
}

/// Resolves an existing path, or a not-yet-existing file whose parent exists.
fn resolve(path: &Path) -> Result<PathBuf, PathGuardError> {
    if path.exists() {
        return canonical(path);
    }
    let file_name = match path.components().next_back() {
        Some(Component::Normal(name)) => name.to_owned(),
        _ => return Err(PathGuardError::OutsideAllowedRoots(path.to_path_buf())),
    };
    let parent = path
        .parent()
        .ok_or_else(|| PathGuardError::OutsideAllowedRoots(path.to_path_buf()))?;
    Ok(canonical(parent)?.join(file_name))
}

/// `std::fs::canonicalize`, minus the `\\?\` prefix Windows adds, so paths compare
/// consistently with the ones users and dialogs produce.
pub fn dunce_canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        if let Some(s) = canonical.to_str()
            && let Some(stripped) = s.strip_prefix(r"\\?\")
            && !stripped.starts_with("UNC\\")
        {
            return Ok(PathBuf::from(stripped));
        }
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, tempfile::TempDir, PathGuard) {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let guard = PathGuard::new();
        guard.allow_dir(workspace.path()).unwrap();
        (workspace, outside, guard)
    }

    #[test]
    fn allows_existing_and_new_files_inside_root() {
        let (ws, _outside, guard) = setup();
        let existing = ws.path().join("a.yaml");
        std::fs::write(&existing, "x").unwrap();
        assert!(guard.check(&existing).is_ok());
        assert!(guard.check(&ws.path().join("new.yaml")).is_ok());
    }

    #[test]
    fn rejects_paths_outside_roots() {
        let (_ws, outside, guard) = setup();
        let err = guard.check(&outside.path().join("x.yaml")).unwrap_err();
        assert!(matches!(err, PathGuardError::OutsideAllowedRoots(_)));
    }

    #[test]
    fn rejects_parent_dir_escape() {
        let (ws, outside, guard) = setup();
        let sneaky = ws
            .path()
            .join("..")
            .join(outside.path().file_name().unwrap())
            .join("x.yaml");
        assert!(guard.check(&sneaky).is_err());
    }

    #[test]
    fn rejects_relative_paths() {
        let (_ws, _outside, guard) = setup();
        assert!(matches!(
            guard.check(Path::new("a.yaml")),
            Err(PathGuardError::NotAbsolute(_))
        ));
    }

    #[test]
    fn allows_explicitly_picked_file_only() {
        let (_ws, outside, guard) = setup();
        let picked = outside.path().join("upload.bin");
        let sibling = outside.path().join("other.bin");
        std::fs::write(&picked, "x").unwrap();
        std::fs::write(&sibling, "x").unwrap();
        guard.allow_file(&picked).unwrap();
        assert!(guard.check(&picked).is_ok());
        assert!(guard.check(&sibling).is_err());
    }

    #[test]
    fn is_allowed_file_only_counts_picked_files() {
        let (ws, outside, guard) = setup();
        let inside = ws.path().join("a.bin");
        let picked = outside.path().join("b.bin");
        std::fs::write(&inside, "x").unwrap();
        std::fs::write(&picked, "x").unwrap();
        assert!(!guard.is_allowed_file(&inside), "directory access isn't a pick");
        assert!(!guard.is_allowed_file(&picked));
        guard.allow_file(&picked).unwrap();
        assert!(guard.is_allowed_file(&picked));
    }

    #[test]
    fn revoked_dir_is_rejected() {
        let (ws, _outside, guard) = setup();
        guard.revoke_dir(ws.path());
        assert!(guard.check(&ws.path().join("a.yaml")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_pointing_outside() {
        let (ws, outside, guard) = setup();
        let target = outside.path().join("secret.txt");
        std::fs::write(&target, "x").unwrap();
        let link = ws.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(guard.check(&link).is_err());
    }
}
