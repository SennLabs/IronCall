//! Environment files: `environments/*.yaml` at the workspace root (PLAN.md §4.2).
//!
//! Environments follow the same rules as requests: stable IDs, file names fixed at
//! creation, hash-checked atomic saves, own-write tracking, duplicate-ID repair and
//! never writing a file that can't be parsed. Secret variables are declared here
//! (`secret: true`) but their values are never written (PLAN.md §6).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ironcall_core::ItemId;
use ironcall_core::model::{EnvironmentFile, Extra, Vars, schema};

use super::{
    ENVIRONMENTS_DIR, Loaded, REQUEST_EXT, Reassignment, Result, SUPPORTED_VERSION, SaveOutcome, Workspace,
    WorkspaceError, clean_name, ensure_no_secret_values, parse, slash_path, to_yaml,
};
use crate::atomic::{self, TEMP_PREFIX, content_hash};
use crate::{slug, yaml};

/// One environment as listed in the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvironmentSummary {
    /// Stable for readable files; session-only for files that can't be read.
    pub id: ItemId,
    pub name: String,
    /// Relative to the workspace root, `/`-separated.
    pub path: String,
    /// Set if the file can't be read. Such environments can't be activated or edited.
    pub error: Option<String>,
    pub read_only: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct EnvEntry {
    pub(super) summary: EnvironmentSummary,
    abs: PathBuf,
    hash: String,
    mtime: Option<SystemTime>,
}

/// Parse result cached by (mtime, len), like request files.
#[derive(Clone, Debug)]
pub(super) struct EnvCached {
    mtime: Option<SystemTime>,
    len: u64,
    hash: String,
    parsed: std::result::Result<(ItemId, String, bool), String>,
}

impl Workspace {
    /// Environments sorted by name (then path).
    pub fn environments(&self) -> Vec<EnvironmentSummary> {
        self.environments.iter().map(|e| e.summary.clone()).collect()
    }

    fn env_dir(&self) -> PathBuf {
        self.root.join(ENVIRONMENTS_DIR)
    }

    fn env_entry(&self, id: ItemId) -> Result<&EnvEntry> {
        self.environments
            .iter()
            .find(|e| e.summary.id == id)
            .ok_or(WorkspaceError::NotFound(id))
    }

    /// Absolute path of an environment file.
    pub fn environment_path(&self, id: ItemId) -> Option<&Path> {
        self.env_entry(id).ok().map(|e| e.abs.as_path())
    }

    /// Rescans `environments/`. Returns whether anything changed, plus any duplicate-ID
    /// repairs.
    pub(super) fn scan_environments(&mut self) -> Result<(bool, Vec<Reassignment>)> {
        let dir = self.env_dir();
        let mut files = Vec::new();
        if dir.is_dir() {
            for entry in std::fs::read_dir(&dir)?.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || name.starts_with(TEMP_PREFIX) || !name.ends_with(REQUEST_EXT) {
                    continue;
                }
                // Symlinks are skipped, as in the tree (see `walk`).
                if entry.file_type().is_ok_and(|t| t.is_symlink()) {
                    continue;
                }
                let path = entry.path();
                if path.is_file() {
                    files.push(path);
                }
            }
        }
        files.sort();
        self.env_cache.retain(|p, _| files.contains(p));

        let old = std::mem::take(&mut self.environments);
        let mut entries = Vec::with_capacity(files.len());
        for abs in files {
            // Gone since the listing (a checkout in progress): skip it this time.
            let Ok(cached) = self.env_cached(&abs) else {
                continue;
            };
            let rel = abs.strip_prefix(&self.root).unwrap_or(&abs).to_path_buf();
            let (id, name, error, read_only) = match &cached.parsed {
                Ok((id, name, read_only)) => (*id, name.clone(), None, *read_only),
                Err(e) => {
                    // Keep the same session ID for the same broken file between scans.
                    let id = old
                        .iter()
                        .find(|o| o.abs == abs && o.summary.error.is_some())
                        .map_or_else(ItemId::new, |o| o.summary.id);
                    let stem = abs.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                    (id, stem, Some(e.clone()), false)
                }
            };
            entries.push(EnvEntry {
                summary: EnvironmentSummary {
                    id,
                    name,
                    path: slash_path(&rel),
                    error,
                    read_only,
                },
                abs,
                hash: cached.hash.clone(),
                mtime: cached.mtime,
            });
        }
        let reassigned = self.dedupe_env_ids(&mut entries)?;
        entries.sort_by(|a, b| {
            (a.summary.name.to_lowercase(), &a.summary.path)
                .cmp(&(b.summary.name.to_lowercase(), &b.summary.path))
        });
        let changed = entries != old;
        self.environments = entries;
        Ok((changed, reassigned))
    }

    fn env_cached(&mut self, path: &Path) -> Result<EnvCached> {
        let meta = std::fs::metadata(path)?;
        let mtime = meta.modified().ok();
        if let Some(c) = self.env_cache.get(path)
            && c.mtime == mtime
            && c.len == meta.len()
            && mtime.is_some()
        {
            return Ok(c.clone());
        }
        let bytes = std::fs::read(path)?;
        let parsed = parse::<EnvironmentFile>(&bytes).and_then(|e| {
            match yaml::schema_version(&e.schema, "environment") {
                Some(v) => Ok((e.id, e.name, v > SUPPORTED_VERSION)),
                None => Err(format!("not an environment file (schema `{}`)", e.schema)),
            }
        });
        let cached = EnvCached {
            mtime,
            len: bytes.len() as u64,
            hash: content_hash(&bytes),
            parsed,
        };
        self.env_cache.insert(path.to_path_buf(), cached.clone());
        Ok(cached)
    }

    /// Gives environment files that share an ID (e.g. copied on disk) new IDs, keeping
    /// the oldest by modification time.
    fn dedupe_env_ids(&mut self, entries: &mut [EnvEntry]) -> Result<Vec<Reassignment>> {
        let mut by_id: HashMap<ItemId, Vec<usize>> = HashMap::new();
        for (i, e) in entries.iter().enumerate() {
            if e.summary.error.is_none() {
                by_id.entry(e.summary.id).or_default().push(i);
            }
        }
        let mut reassigned = Vec::new();
        for (old, mut list) in by_id {
            if list.len() < 2 {
                continue;
            }
            list.sort_by_key(|&i| {
                (
                    super::copy_age(&entries[i].abs),
                    super::name_len(&entries[i].abs),
                    entries[i].summary.path.clone(),
                )
            });
            for &i in &list[1..] {
                let entry = &mut entries[i];
                if entry.summary.read_only {
                    continue;
                }
                let bytes = std::fs::read(&entry.abs)?;
                let mut file: EnvironmentFile = parse(&bytes).map_err(WorkspaceError::Invalid)?;
                let new = ItemId::new();
                file.id = new;
                let text = to_yaml(&file)?;
                atomic::write_own(&self.own, &entry.abs, text.as_bytes())?;
                self.env_cache.remove(&entry.abs);
                entry.summary.id = new;
                entry.hash = content_hash(text.as_bytes());
                tracing::info!(path = %entry.summary.path, %old, %new, "reassigned duplicate environment ID");
                reassigned.push(Reassignment {
                    path: entry.summary.path.clone(),
                    old,
                    new,
                });
            }
        }
        reassigned.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(reassigned)
    }

    pub fn load_environment(&self, id: ItemId) -> Result<Loaded<EnvironmentFile>> {
        let entry = self.env_entry(id)?;
        if let Some(e) = &entry.summary.error {
            return Err(WorkspaceError::Invalid(format!("{}: {e}", entry.summary.path)));
        }
        let bytes = std::fs::read(&entry.abs)?;
        let file =
            parse(&bytes).map_err(|e| WorkspaceError::Invalid(format!("{}: {e}", entry.summary.path)))?;
        Ok(Loaded {
            file,
            hash: Some(content_hash(&bytes)),
            read_only: entry.summary.read_only,
        })
    }

    /// Variables of a readable environment.
    pub fn environment_vars(&self, id: ItemId) -> Result<Vars> {
        Ok(self.load_environment(id)?.file.vars)
    }

    /// Saves an environment. Refuses with `Conflict` if the file on disk doesn't match
    /// `base_hash` (unless `force`), and errors if a secret variable carries a value.
    pub fn save_environment(
        &mut self,
        file: &EnvironmentFile,
        base_hash: Option<&str>,
        force: bool,
    ) -> Result<SaveOutcome<EnvironmentFile>> {
        let entry = self.env_entry(file.id)?.clone();
        if entry.summary.error.is_some() {
            return Err(WorkspaceError::Invalid(format!(
                "fix {} before editing it",
                entry.summary.path
            )));
        }
        if entry.summary.read_only {
            return Err(WorkspaceError::ReadOnly(entry.summary.path.clone()));
        }
        ensure_no_secret_values(&file.vars)?;
        let text = to_yaml(file)?;
        match self.check_and_write(&entry.abs, text.as_bytes(), base_hash, force)? {
            Some(hash) => {
                // The file is saved; a failed rescan is only logged (the watcher retries).
                if let Err(e) = self.rescan(std::slice::from_ref(&entry.abs)) {
                    tracing::warn!(error = %e, "rescan after save failed");
                }
                Ok(SaveOutcome::Saved { hash })
            }
            None => Ok(SaveOutcome::Conflict {
                disk: self.load_environment(file.id).ok(),
            }),
        }
    }

    /// Creates `environments/<slug>.yaml` with a new ID.
    pub fn create_environment(&mut self, name: &str) -> Result<EnvironmentSummary> {
        let file = EnvironmentFile {
            schema: schema::ENVIRONMENT.to_string(),
            id: ItemId::new(),
            name: clean_name(name)?,
            vars: Vars::default(),
            extra: Extra::default(),
        };
        self.write_new_environment(&file)
    }

    /// Copies an environment with a new ID, named "<name> copy". Secret values are not
    /// copied here; the app copies them in the secret store.
    pub fn duplicate_environment(&mut self, id: ItemId) -> Result<EnvironmentSummary> {
        let mut file = self.load_environment(id)?.file;
        file.id = ItemId::new();
        file.name = format!("{} copy", file.name);
        self.write_new_environment(&file)
    }

    fn write_new_environment(&mut self, file: &EnvironmentFile) -> Result<EnvironmentSummary> {
        ensure_no_secret_values(&file.vars)?;
        let dir = self.env_dir();
        std::fs::create_dir_all(&dir)?;
        let unique = slug::unique_name(&dir, &slug::slugify(&file.name), REQUEST_EXT, None)?;
        if let Some(warning) = &unique.warning {
            tracing::warn!(%warning, "environment path is long");
        }
        let path = dir.join(&unique.name);
        atomic::write_own(&self.own, &path, to_yaml(file)?.as_bytes())?;
        self.rescan(&[path])?;
        Ok(self.env_entry(file.id)?.summary.clone())
    }

    /// Changes the display name only; the file name stays (PLAN.md §4.3).
    pub fn rename_environment(&mut self, id: ItemId, name: &str) -> Result<()> {
        let name = clean_name(name)?;
        let loaded = self.load_environment(id)?;
        if loaded.read_only {
            return Err(WorkspaceError::ReadOnly(self.env_entry(id)?.summary.path.clone()));
        }
        let mut file = loaded.file;
        file.name = name;
        self.save_environment(&file, None, true)?;
        Ok(())
    }

    /// Moves an environment file (readable or not) to the trash.
    pub fn delete_environment(&mut self, id: ItemId) -> Result<()> {
        let abs = self.env_entry(id)?.abs.clone();
        self.own.record_removed(&abs);
        self.trash.trash(&abs)?;
        self.rescan(&[abs])?;
        Ok(())
    }
}
