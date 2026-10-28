//! Secret storage for IronCall: the [`SecretStore`] trait and its keychain,
//! encrypted-file and plaintext backends (PLAN.md §6.1).
//!
//! Values are [`SecretString`]s everywhere, and no error or `Debug` output ever
//! contains one. Keys identify a secret by workspace, scope and variable name
//! (PLAN.md §4.5); they are not secret.

mod encrypted;
mod fsutil;
mod keychain;
mod plaintext;

use std::fmt;

use ironcall_core::ItemId;
pub use secrecy::{ExposeSecret, SecretString};

pub use encrypted::{EncryptedFileStore, KdfParams};
pub use keychain::{CredentialBackend, KeychainStore, KeyringBackend, MemoryBackend};
pub use plaintext::PlaintextFileStore;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the encrypted secrets file is locked; unlock it with its passphrase")]
    Locked,
    #[error("the OS keychain is not available: {0}")]
    Unavailable(String),
    #[error("wrong passphrase (or the secrets file is damaged)")]
    WrongPassphrase,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("secret store error: {0}")]
    Backend(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Scope strings for [`SecretKey::scope`].
pub mod scope {
    use ironcall_core::ItemId;

    pub const GLOBAL: &str = "global";
    pub const COLLECTION: &str = "collection";

    pub fn environment(id: ItemId) -> String {
        format!("env:{id}")
    }

    pub fn folder(id: ItemId) -> String {
        format!("folder:{id}")
    }
}

/// Identifies one secret value: (workspace, scope, variable name).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SecretKey {
    /// `None` for global variables.
    pub workspace: Option<ItemId>,
    /// e.g. `collection`, `env:<id>`, `folder:<id>`, `global` (see [`scope`]).
    pub scope: String,
    pub name: String,
}

impl SecretKey {
    pub fn new(workspace: Option<ItemId>, scope: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            workspace,
            scope: scope.into(),
            name: name.into(),
        }
    }

    pub fn global(name: impl Into<String>) -> Self {
        Self::new(None, scope::GLOBAL, name)
    }

    /// Stable string form, used as the keychain account and the file-store map key:
    /// `ironcall/<workspace id | global>/<scope>/<name>`.
    pub fn storage_id(&self) -> String {
        let workspace = self
            .workspace
            .map_or_else(|| "global".to_string(), |id| id.to_string());
        format!("ironcall/{workspace}/{}/{}", self.scope, self.name)
    }
}

impl fmt::Display for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.storage_id())
    }
}

/// A place secret values are kept.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &SecretKey) -> Result<Option<SecretString>>;
    fn set(&self, key: &SecretKey, value: &SecretString) -> Result<()>;
    /// Deleting a key with no value is not an error.
    fn delete(&self, key: &SecretKey) -> Result<()>;
}

/// Result of [`migrate`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MigrationReport {
    /// Keys whose value was copied (and then removed from the source).
    pub copied: usize,
    /// Keys with no value in the source.
    pub skipped: usize,
}

/// Moves the values of `keys` from `from` to `to` (PLAN.md §6.1 backend migration).
///
/// Every value is copied and verified by reading it back before anything is deleted
/// from `from`. If any copy fails, values already written to `to` are restored to what
/// `to` held before, `from` is left untouched, and the error is returned.
pub fn migrate(from: &dyn SecretStore, to: &dyn SecretStore, keys: &[SecretKey]) -> Result<MigrationReport> {
    let mut report = MigrationReport::default();
    // (key, previous value in `to`), for rollback.
    let mut written: Vec<(&SecretKey, Option<SecretString>)> = Vec::new();

    if let Err(e) = copy_all(from, to, keys, &mut written, &mut report) {
        for (key, previous) in written.into_iter().rev() {
            let _ = match previous {
                Some(v) => to.set(key, &v),
                None => to.delete(key),
            };
        }
        return Err(e);
    }

    for key in keys {
        from.delete(key)?;
    }
    Ok(report)
}

fn copy_all<'k>(
    from: &dyn SecretStore,
    to: &dyn SecretStore,
    keys: &'k [SecretKey],
    written: &mut Vec<(&'k SecretKey, Option<SecretString>)>,
    report: &mut MigrationReport,
) -> Result<()> {
    for key in keys {
        let Some(value) = from.get(key)? else {
            report.skipped += 1;
            continue;
        };
        written.push((key, to.get(key)?));
        to.set(key, &value)?;
        let check = to.get(key)?;
        if check.as_ref().map(ExposeSecret::expose_secret) != Some(value.expose_secret()) {
            return Err(Error::Backend(format!(
                "{key}: value did not read back correctly"
            )));
        }
        report.copied += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    /// In-memory store whose `set` can be made to fail on a given key.
    #[derive(Default)]
    pub(crate) struct MapStore {
        map: Mutex<HashMap<String, String>>,
        fail_on: Option<String>,
    }

    impl SecretStore for MapStore {
        fn get(&self, key: &SecretKey) -> Result<Option<SecretString>> {
            Ok(self
                .map
                .lock()
                .unwrap()
                .get(&key.storage_id())
                .cloned()
                .map(SecretString::from))
        }
        fn set(&self, key: &SecretKey, value: &SecretString) -> Result<()> {
            if self.fail_on.as_deref() == Some(key.name.as_str()) {
                return Err(Error::Backend("injected failure".into()));
            }
            self.map
                .lock()
                .unwrap()
                .insert(key.storage_id(), value.expose_secret().to_string());
            Ok(())
        }
        fn delete(&self, key: &SecretKey) -> Result<()> {
            self.map.lock().unwrap().remove(&key.storage_id());
            Ok(())
        }
    }

    fn key(name: &str) -> SecretKey {
        SecretKey::new(Some(ItemId::new()), scope::COLLECTION, name)
    }

    #[test]
    fn storage_id_is_stable() {
        let ws: ItemId = "01J9Z3M0A1B2C3D4E5F6G7H8J9".parse().unwrap();
        let env: ItemId = "01J9Z3K7T2V8Q4M6N0P5R1S3W7".parse().unwrap();
        assert_eq!(
            SecretKey::new(Some(ws), scope::environment(env), "apiKey").storage_id(),
            "ironcall/01J9Z3M0A1B2C3D4E5F6G7H8J9/env:01J9Z3K7T2V8Q4M6N0P5R1S3W7/apiKey"
        );
        assert_eq!(
            SecretKey::global("token").storage_id(),
            "ironcall/global/global/token"
        );
    }

    #[test]
    fn migrate_moves_values_and_skips_missing() {
        let from = MapStore::default();
        let to = MapStore::default();
        let (a, b, c) = (key("a"), key("b"), key("c"));
        from.set(&a, &"one".to_string().into()).unwrap();
        from.set(&b, &"two".to_string().into()).unwrap();

        let report = migrate(&from, &to, &[a.clone(), b.clone(), c]).unwrap();
        assert_eq!(
            report,
            MigrationReport {
                copied: 2,
                skipped: 1
            }
        );
        assert_eq!(to.get(&a).unwrap().unwrap().expose_secret(), "one");
        assert_eq!(to.get(&b).unwrap().unwrap().expose_secret(), "two");
        assert!(from.get(&a).unwrap().is_none());
        assert!(from.get(&b).unwrap().is_none());
    }

    #[test]
    fn failed_migration_leaves_source_intact_and_rolls_back_target() {
        let from = MapStore::default();
        let to = MapStore {
            fail_on: Some("b".into()),
            ..MapStore::default()
        };
        let (a, b) = (key("a"), key("b"));
        from.set(&a, &"one".to_string().into()).unwrap();
        from.set(&b, &"two".to_string().into()).unwrap();

        assert!(migrate(&from, &to, &[a.clone(), b.clone()]).is_err());
        assert_eq!(from.get(&a).unwrap().unwrap().expose_secret(), "one");
        assert_eq!(from.get(&b).unwrap().unwrap().expose_secret(), "two");
        assert!(to.get(&a).unwrap().is_none(), "partial copy rolled back");
    }

    #[test]
    fn errors_and_debug_never_show_values() {
        let value = SecretString::from("hunter2".to_string());
        assert!(!format!("{value:?}").contains("hunter2"));
        for err in [
            Error::Locked,
            Error::WrongPassphrase,
            Error::Unavailable("no Secret Service".into()),
            Error::Backend("x".into()),
        ] {
            assert!(!err.to_string().contains("hunter2"));
        }
    }
}
