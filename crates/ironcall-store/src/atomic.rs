//! Atomic file writes and the own-writes registry (PLAN.md §4.4, §3.3).
//!
//! Every write goes to a temp file in the same directory, is flushed to disk and then
//! renamed over the target, so a crash never leaves a half-written workspace file.
//! Writes, renames and deletions IronCall makes are recorded in [`OwnWrites`] so the
//! file watcher can tell them apart from external changes (git pull, editors).

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Prefix of temp files created by [`write_atomic`]; the watcher ignores them.
pub const TEMP_PREFIX: &str = ".ironcall-tmp-";

/// How long an own write is remembered. Debounced watcher events arrive well within this.
const OWN_WRITE_TTL: Duration = Duration::from_secs(5);

/// Hex BLAKE3 hash of file contents, used for conflict detection and own-write matching.
pub fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Writes `bytes` to `path` atomically: temp file in the same directory, fsync, rename.
///
/// On Windows the rename is retried briefly if the target is locked (antivirus scanners
/// and editors often hold files open for a moment).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, false)
}

/// Like [`write_atomic`], but the file is readable only by the current user (0600 on
/// unix). For files holding sensitive data such as cookies.
pub fn write_private_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, true)
}

fn write_atomic_with(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_string_lossy();
    let tmp = dir.join(format!(
        "{TEMP_PREFIX}{}-{}-{file_name}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private;
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        rename_with_retry(&tmp, path)?;
        sync_dir(dir);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// `fs::rename`, retried for up to about a second on Windows sharing violations.
pub fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if cfg!(windows) && e.kind() == io::ErrorKind::PermissionDenied && attempt < 20 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Flushes a directory entry change to disk (Unix). Best effort.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[derive(Debug, Clone)]
enum OwnChange {
    /// We wrote this content to the path.
    Wrote(String),
    /// We removed (or renamed away) the path.
    Removed,
    /// We changed something under this directory (move, copy, rename, delete).
    Tree,
}

/// Recently made changes by IronCall itself, so the watcher can ignore them.
#[derive(Debug, Default)]
pub struct OwnWrites {
    entries: Mutex<HashMap<PathBuf, (OwnChange, Instant)>>,
}

impl OwnWrites {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that we wrote `bytes` to `path`.
    pub fn record_write(&self, path: &Path, bytes: &[u8]) {
        self.insert(path, OwnChange::Wrote(content_hash(bytes)));
    }

    /// Records that we removed or renamed away `path`.
    pub fn record_removed(&self, path: &Path) {
        self.insert(path, OwnChange::Removed);
    }

    /// Records that we changed everything under directory `path` (move, copy, delete).
    pub fn record_tree(&self, path: &Path) {
        self.insert(path, OwnChange::Tree);
    }

    /// Whether the current state of `path` is explained by one of our own changes.
    pub fn is_own(&self, path: &Path) -> bool {
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("own-writes lock poisoned");
        entries.retain(|_, (_, at)| now.duration_since(*at) < OWN_WRITE_TTL);

        if entries
            .iter()
            .any(|(p, (change, _))| matches!(change, OwnChange::Tree) && path.starts_with(p))
        {
            return true;
        }
        match entries.get(path) {
            Some((OwnChange::Removed, _)) => !path.exists(),
            Some((OwnChange::Wrote(hash), _)) => std::fs::read(path).is_ok_and(|b| content_hash(&b) == *hash),
            _ => false,
        }
    }

    fn insert(&self, path: &Path, change: OwnChange) {
        self.entries
            .lock()
            .expect("own-writes lock poisoned")
            .insert(path.to_path_buf(), (change, Instant::now()));
    }
}

/// Writes atomically and records the write as our own.
pub fn write_own(own: &OwnWrites, path: &Path, bytes: &[u8]) -> io::Result<()> {
    own.record_write(path, bytes);
    write_atomic(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_replace_content_and_leave_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.yaml");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["a.yaml"]);
    }

    #[test]
    fn hash_is_stable_and_content_sensitive() {
        assert_eq!(content_hash(b"x"), content_hash(b"x"));
        assert_ne!(content_hash(b"x"), content_hash(b"y"));
    }

    #[test]
    fn own_writes_match_only_our_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.yaml");
        let own = OwnWrites::new();
        write_own(&own, &path, b"ours").unwrap();
        assert!(own.is_own(&path));
        std::fs::write(&path, b"theirs").unwrap();
        assert!(!own.is_own(&path));
    }

    #[test]
    fn removed_and_tree_changes() {
        let dir = tempfile::tempdir().unwrap();
        let own = OwnWrites::new();
        let gone = dir.path().join("gone.yaml");
        own.record_removed(&gone);
        assert!(own.is_own(&gone));
        std::fs::write(&gone, b"back").unwrap();
        assert!(!own.is_own(&gone), "recreated externally");

        let sub = dir.path().join("sub");
        own.record_tree(&sub);
        assert!(own.is_own(&sub.join("deep/x.yaml")));
        assert!(!own.is_own(&dir.path().join("other.yaml")));
    }
}
