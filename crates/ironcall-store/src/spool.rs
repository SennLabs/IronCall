//! Response body spool (PLAN.md §3.1, §6.3).
//!
//! Response bodies are written to files in the app data dir (never the system temp
//! dir), readable only by the current user, and removed when evicted, on startup and
//! on exit. Responses can contain sensitive data, so nothing here outlives the session.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// How many sends keep their spooled bodies before the oldest are deleted.
pub const DEFAULT_RETAIN: usize = 20;

#[derive(Debug)]
pub struct Spool {
    dir: PathBuf,
    retain: usize,
    /// Spooled entries, oldest first. Each entry owns all files sharing its stem.
    entries: Mutex<VecDeque<String>>,
}

impl Spool {
    /// Opens the spool in `dir`, creating it and deleting anything left from a
    /// previous run.
    pub fn open(dir: impl Into<PathBuf>, retain: usize) -> io::Result<Self> {
        let spool = Self {
            dir: dir.into(),
            retain: retain.max(1),
            entries: Mutex::new(VecDeque::new()),
        };
        std::fs::create_dir_all(&spool.dir)?;
        restrict_dir(&spool.dir)?;
        spool.clear()?;
        Ok(spool)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Starts a new entry. `stem` must be a safe file-name stem (e.g. a ULID send ID).
    ///
    /// Returns the entry plus the stems of the oldest entries beyond the retention limit.
    /// The caller must close anything still reading those (Windows can't delete open
    /// files) and then call [`Spool::remove_entry`] for each.
    pub fn create_entry(&self, stem: &str) -> io::Result<(SpoolEntry, Vec<String>)> {
        if stem.is_empty()
            || !stem
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid spool entry name",
            ));
        }
        let mut entries = self.entries.lock().expect("spool lock poisoned");
        entries.push_back(stem.to_string());
        let excess = entries.len().saturating_sub(self.retain);
        let evicted = entries.range(..excess).cloned().collect();
        let entry = SpoolEntry {
            dir: self.dir.clone(),
            stem: stem.to_string(),
        };
        Ok((entry, evicted))
    }

    /// Path of an existing entry's file with the given extension.
    pub fn path_of(&self, stem: &str, extension: &str) -> PathBuf {
        self.dir.join(format!("{stem}.{extension}"))
    }

    /// Deletes one entry's files now (e.g. when its response is closed).
    pub fn remove_entry(&self, stem: &str) {
        self.entries
            .lock()
            .expect("spool lock poisoned")
            .retain(|s| s != stem);
        self.remove_files(stem);
    }

    /// Deletes every file in the spool directory.
    pub fn clear(&self) -> io::Result<()> {
        self.entries.lock().expect("spool lock poisoned").clear();
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.is_file() {
                let _ = std::fs::remove_file(path);
            }
        }
        Ok(())
    }

    fn remove_files(&self, stem: &str) {
        let Ok(dir) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in dir.flatten() {
            let path = entry.path();
            if path.file_stem().and_then(|s| s.to_str()) == Some(stem) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        let _ = self.clear();
    }
}

/// Files for one spooled response. All files share the entry's stem.
#[derive(Debug, Clone)]
pub struct SpoolEntry {
    dir: PathBuf,
    stem: String,
}

impl SpoolEntry {
    pub fn stem(&self) -> &str {
        &self.stem
    }

    /// Path of the file with the given extension, e.g. `raw` or `txt`.
    pub fn path(&self, extension: &str) -> PathBuf {
        self.dir.join(format!("{}.{extension}", self.stem))
    }

    /// Creates (or truncates) the file with the given extension, readable only by the
    /// current user.
    pub fn create(&self, extension: &str) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(self.path(extension))
    }
}

/// Limits the spool directory to the current user (0700, or a user-only ACL on Windows).
fn restrict_dir(dir: &Path) -> io::Result<()> {
    // Best effort: the spool lives in the app data dir, which is private by default, so
    // a failure (e.g. no `icacls`) is logged rather than stopping the app.
    if let Err(e) = ironcall_core::perms::restrict_to_user(dir) {
        tracing::warn!(dir = %dir.display(), error = %e, "couldn't make the spool folder user-only");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn spool(retain: usize) -> (tempfile::TempDir, Spool) {
        let tmp = tempfile::tempdir().unwrap();
        let spool = Spool::open(tmp.path().join("spool"), retain).unwrap();
        (tmp, spool)
    }

    #[test]
    fn open_clears_leftovers() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("spool");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.raw"), "secret").unwrap();
        let _spool = Spool::open(&dir, 5).unwrap();
        assert!(!dir.join("old.raw").exists());
    }

    #[test]
    fn evicts_oldest_beyond_retention() {
        let (_tmp, spool) = spool(2);
        let mut evicted = Vec::new();
        for stem in ["a", "b", "c"] {
            let (entry, old) = spool.create_entry(stem).unwrap();
            entry.create("raw").unwrap().write_all(b"x").unwrap();
            entry.create("txt").unwrap().write_all(b"x").unwrap();
            for stem in old {
                spool.remove_entry(&stem);
                evicted.push(stem);
            }
        }
        assert_eq!(evicted, ["a"]);
        assert!(!spool.dir().join("a.raw").exists());
        assert!(!spool.dir().join("a.txt").exists());
        assert!(spool.dir().join("b.raw").exists());
        assert!(spool.dir().join("c.txt").exists());
    }

    #[test]
    fn remove_entry_deletes_its_files() {
        let (_tmp, spool) = spool(5);
        let (entry, _) = spool.create_entry("a").unwrap();
        entry.create("raw").unwrap();
        spool.remove_entry("a");
        assert!(!entry.path("raw").exists());
    }

    #[test]
    fn drop_clears_everything() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("spool");
        {
            let spool = Spool::open(&dir, 5).unwrap();
            spool.create_entry("a").unwrap().0.create("raw").unwrap();
        }
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn rejects_unsafe_names() {
        let (_tmp, spool) = spool(5);
        for bad in ["", "../x", "a/b", "a.b"] {
            assert!(spool.create_entry(bad).is_err(), "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn files_and_dir_are_user_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, spool) = spool(5);
        let (entry, _) = spool.create_entry("a").unwrap();
        entry.create("raw").unwrap();
        let file_mode = std::fs::metadata(entry.path("raw")).unwrap().permissions().mode() & 0o777;
        let dir_mode = std::fs::metadata(spool.dir()).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
    }
}
