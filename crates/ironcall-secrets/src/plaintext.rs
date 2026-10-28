//! Plaintext secrets file: `.ironcall/secrets.yaml` (PLAN.md §6.1).
//!
//! Only as safe as the disk; the UI warns before choosing it. The file is git-ignored
//! by `.ironcall/.gitignore`, written atomically and readable only by the user. It is
//! JSON (which is valid YAML) so values never go through YAML's implicit typing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::fsutil::write_private_atomic;
use crate::{Error, Result, SecretKey, SecretStore};

/// `SecretString` zeroizes each value when the map is dropped.
type SecretMap = BTreeMap<String, SecretString>;

#[derive(Deserialize)]
struct FileIn {
    #[serde(default)]
    secrets: SecretMap,
}

#[derive(Serialize)]
struct FileOut<'a> {
    version: u32,
    secrets: BTreeMap<&'a str, &'a str>,
}

pub struct PlaintextFileStore {
    path: PathBuf,
    /// Serialises read-modify-write cycles.
    write: Mutex<()>,
}

impl std::fmt::Debug for PlaintextFileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaintextFileStore")
            .field("path", &self.path)
            .finish()
    }
}

impl PlaintextFileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            write: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Result<SecretMap> {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => Zeroizing::new(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SecretMap::new()),
            Err(e) => return Err(e.into()),
        };
        // Only the line number: serde_json's message can quote part of the input.
        let content: FileIn = serde_json::from_slice(&bytes).map_err(|e| {
            Error::Backend(format!(
                "{} is not valid (line {})",
                self.path.display(),
                e.line()
            ))
        })?;
        Ok(content.secrets)
    }

    fn write(&self, secrets: &SecretMap) -> Result<()> {
        let content = FileOut {
            version: 1,
            secrets: secrets
                .iter()
                .map(|(k, v)| (k.as_str(), v.expose_secret()))
                .collect(),
        };
        let json =
            Zeroizing::new(serde_json::to_vec_pretty(&content).map_err(|e| Error::Backend(e.to_string()))?);
        write_private_atomic(&self.path, &json)?;
        Ok(())
    }
}

impl SecretStore for PlaintextFileStore {
    fn get(&self, key: &SecretKey) -> Result<Option<SecretString>> {
        Ok(self.read()?.get(&key.storage_id()).cloned())
    }

    fn set(&self, key: &SecretKey, value: &SecretString) -> Result<()> {
        let _guard = self.write.lock().expect("plaintext store lock poisoned");
        let mut secrets = self.read()?;
        secrets.insert(key.storage_id(), value.clone());
        self.write(&secrets)
    }

    fn delete(&self, key: &SecretKey) -> Result<()> {
        let _guard = self.write.lock().expect("plaintext store lock poisoned");
        let mut secrets = self.read()?;
        if secrets.remove(&key.storage_id()).is_some() {
            self.write(&secrets)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope;

    #[test]
    fn round_trip_persists_and_is_user_only() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".ironcall").join("secrets.yaml");
        let key = SecretKey::new(Some(ironcall_core::ItemId::new()), scope::COLLECTION, "token");
        let s = PlaintextFileStore::new(&path);
        assert!(s.get(&key).unwrap().is_none());

        s.set(&key, &SecretString::from("no".to_string())).unwrap();
        let reopened = PlaintextFileStore::new(&path);
        assert_eq!(
            reopened.get(&key).unwrap().unwrap().expose_secret(),
            "no",
            "no YAML typing"
        );

        reopened.delete(&key).unwrap();
        assert!(s.get(&key).unwrap().is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn invalid_file_error_has_no_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.yaml");
        std::fs::write(&path, "{\"version\": 1, \"secrets\": {\"k\": \"hunter2\"").unwrap();
        let err = PlaintextFileStore::new(&path)
            .get(&SecretKey::global("k"))
            .unwrap_err()
            .to_string();
        assert!(!err.contains("hunter2"), "{err}");
    }
}
