//! Global variables in `<app data>/globals.yaml` (PLAN.md §5): the lowest-precedence
//! scope, shared by every workspace on this machine. Like every other file, it never
//! holds secret values; those live in the secret store.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ironcall_core::model::{Extra, Vars};
use serde::{Deserialize, Serialize};

use crate::atomic::{self, content_hash};
use crate::workspace::{Loaded, Result, SaveOutcome, WorkspaceError, ensure_no_secret_values};
use crate::yaml;

pub const GLOBALS_SCHEMA: &str = "ironcall/globals@1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct GlobalsFile {
    schema: String,
    #[serde(default, skip_serializing_if = "Vars::is_empty")]
    vars: Vars,
    #[serde(flatten)]
    extra: Extra,
}

#[derive(Debug)]
pub struct GlobalsStore {
    path: PathBuf,
    /// Serialises check-and-write so two saves can't interleave.
    lock: Mutex<()>,
}

impl GlobalsStore {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The global variables and the file's hash. A missing file gives no variables and
    /// no hash; an unreadable file is an error (it is never overwritten silently).
    pub fn load(&self) -> Result<(Vars, Option<String>)> {
        match self.read()? {
            Some((file, hash)) => Ok((file.vars, Some(hash))),
            None => Ok((Vars::default(), None)),
        }
    }

    /// Saves the variables, keeping unknown keys already in the file. Refuses with
    /// `Conflict` if the file changed since `base_hash`, and errors if a secret
    /// variable carries a value.
    pub fn save(&self, vars: &Vars, base_hash: Option<&str>) -> Result<SaveOutcome<Vars>> {
        ensure_no_secret_values(vars)?;
        let _guard = self.lock.lock().expect("globals lock poisoned");
        let current = self.read()?;
        let current_hash = current.as_ref().map(|(_, h)| h.clone());
        if base_hash.is_some() && current_hash.as_deref() != base_hash {
            return Ok(SaveOutcome::Conflict {
                disk: current.map(|(file, hash)| Loaded {
                    file: file.vars,
                    hash: Some(hash),
                    read_only: false,
                }),
            });
        }
        let file = GlobalsFile {
            schema: GLOBALS_SCHEMA.to_string(),
            vars: vars.clone(),
            extra: current.map(|(f, _)| f.extra).unwrap_or_default(),
        };
        let text = yaml::to_string(&file).map_err(|source| WorkspaceError::Yaml {
            path: self.path.display().to_string(),
            source,
        })?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        atomic::write_atomic(&self.path, text.as_bytes())?;
        Ok(SaveOutcome::Saved {
            hash: content_hash(text.as_bytes()),
        })
    }

    fn read(&self) -> Result<Option<(GlobalsFile, String)>> {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let invalid = |e: String| WorkspaceError::Invalid(format!("{}: {e}", self.path.display()));
        let text =
            std::str::from_utf8(&bytes).map_err(|_| invalid("the file is not valid UTF-8 text".into()))?;
        let file: GlobalsFile = yaml::from_str(text).map_err(|e| invalid(e.to_string()))?;
        if yaml::schema_version(&file.schema, "globals").is_none() {
            return Err(invalid(format!("not a globals file (schema `{}`)", file.schema)));
        }
        Ok(Some((file, content_hash(&bytes))))
    }
}

#[cfg(test)]
mod tests {
    use ironcall_core::model::{Variable, VariableSpec};

    use super::*;

    fn store() -> (tempfile::TempDir, GlobalsStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store = GlobalsStore::open(tmp.path().join("globals.yaml"));
        (tmp, store)
    }

    #[test]
    fn missing_file_is_empty() {
        let (_tmp, store) = store();
        assert_eq!(store.load().unwrap(), (Vars::default(), None));
    }

    #[test]
    fn save_and_load_round_trip_with_conflict_check() {
        let (_tmp, store) = store();
        let mut vars = Vars::default();
        vars.insert("region".into(), Variable::Plain("ap-southeast-2".into()));
        let SaveOutcome::Saved { hash } = store.save(&vars, None).unwrap() else {
            panic!("expected saved");
        };
        let (loaded, loaded_hash) = store.load().unwrap();
        assert_eq!(loaded, vars);
        assert_eq!(loaded_hash.as_deref(), Some(hash.as_str()));

        // Someone else writes; our stale save conflicts.
        let mut other = vars.clone();
        other.insert("x".into(), Variable::Plain("1".into()));
        store.save(&other, Some(&hash)).unwrap();
        match store.save(&vars, Some(&hash)).unwrap() {
            SaveOutcome::Conflict { disk: Some(disk) } => assert_eq!(disk.file, other),
            other => panic!("expected conflict, got {other:?}"),
        }
    }

    #[test]
    fn refuses_secret_values() {
        let (_tmp, store) = store();
        let mut vars = Vars::default();
        vars.insert(
            "token".into(),
            Variable::Detailed(VariableSpec {
                value: Some("hunter2".into()),
                secret: true,
                ..VariableSpec::default()
            }),
        );
        assert!(store.save(&vars, None).is_err());
        assert!(!store.path().exists());
    }

    #[test]
    fn keeps_unknown_keys_and_rejects_wrong_schema() {
        let (_tmp, store) = store();
        std::fs::write(store.path(), "schema: 'ironcall/globals@1'\nfuture: 'kept'\n").unwrap();
        let (vars, hash) = store.load().unwrap();
        store.save(&vars, hash.as_deref()).unwrap();
        assert!(
            std::fs::read_to_string(store.path())
                .unwrap()
                .contains("future: 'kept'")
        );

        std::fs::write(store.path(), "schema: 'ironcall/request@1'\n").unwrap();
        assert!(store.load().is_err());
    }
}
