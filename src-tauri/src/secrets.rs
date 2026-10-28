//! Picks and holds the secret stores (PLAN.md §6.1).
//!
//! The keychain is probed once in the background at startup, because a missing or hung
//! Secret Service must never block the UI. Encrypted-file stores stay unlocked for the
//! rest of the session once unlocked, keyed by workspace root.
//!
//! Each workspace's backend choice is kept per machine in the app data folder
//! (`secret-backends.json`, keyed by workspace folder), never in the workspace: a repo
//! could commit an `.ironcall/local.yaml` (and a `.gitignore` that lets it through)
//! selecting the plaintext file, so secrets typed in would land in the repo folder.
//!
//! Global secrets always use the keychain (they belong to no workspace, so there is no
//! `.ironcall/` to hold a file). If the keychain is unavailable, global secrets can't be
//! stored and the UI says so.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ironcall_secrets::{EncryptedFileStore, KeychainStore, PlaintextFileStore, SecretStore};
use ironcall_store::workspace::LOCAL_DIR;

use crate::vars::{SecretBackend, VarsError};

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

const PROBE_PENDING: u8 = 0;
const PROBE_UNAVAILABLE: u8 = 1;
const PROBE_AVAILABLE: u8 = 2;

pub struct Secrets {
    probe: AtomicU8,
    keychain: Mutex<Option<Arc<KeychainStore>>>,
    encrypted: Mutex<HashMap<PathBuf, Arc<EncryptedFileStore>>>,
    /// `secret-backends.json` in the app data folder.
    choices_file: PathBuf,
    choices: Mutex<()>,
}

impl Secrets {
    /// `data_dir` is the app data folder, which holds the per-machine backend choices.
    pub fn new(data_dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            probe: AtomicU8::new(PROBE_PENDING),
            keychain: Mutex::new(None),
            encrypted: Mutex::new(HashMap::new()),
            choices_file: data_dir.join("secret-backends.json"),
            choices: Mutex::new(()),
        })
    }

    /// The backend chosen on this machine for the workspace at `root` (default: the
    /// keychain).
    pub fn backend_for(&self, root: &Path) -> SecretBackend {
        let _lock = self.choices.lock().expect("backend choices lock poisoned");
        backend_from_setting(self.read_choices().get(&choice_key(root)).map(String::as_str))
    }

    /// Records the backend chosen for the workspace at `root`.
    pub fn set_backend_for(&self, root: &Path, backend: SecretBackend) -> Result<(), VarsError> {
        let _lock = self.choices.lock().expect("backend choices lock poisoned");
        let mut choices = self.read_choices();
        let key = choice_key(root);
        if backend == SecretBackend::Keychain {
            choices.remove(&key);
        } else {
            choices.insert(key, backend_setting(backend).to_string());
        }
        let mut text = serde_json::to_string_pretty(&choices).map_err(|e| VarsError::new(e.to_string()))?;
        text.push('\n');
        ironcall_store::atomic::write_private_atomic(&self.choices_file, text.as_bytes())
            .map_err(|e| VarsError::new(format!("couldn't save the secret storage choice: {e}")))
    }

    fn read_choices(&self) -> BTreeMap<String, String> {
        let Ok(text) = std::fs::read_to_string(&self.choices_file) else {
            return BTreeMap::new();
        };
        serde_json::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "ignoring unreadable secret backend choices");
            BTreeMap::new()
        })
    }

    /// Probes the keychain on a background thread.
    pub fn start_probe(self: &Arc<Self>) {
        let this = self.clone();
        std::thread::spawn(move || {
            let store = KeychainStore::probe(PROBE_TIMEOUT)
                .then(|| KeychainStore::open(PROBE_TIMEOUT).ok())
                .flatten();
            let available = store.is_some();
            *this.keychain.lock().expect("keychain lock poisoned") = store.map(Arc::new);
            this.probe.store(
                if available {
                    PROBE_AVAILABLE
                } else {
                    PROBE_UNAVAILABLE
                },
                Ordering::SeqCst,
            );
            tracing::info!(available, "keychain probe finished");
        });
    }

    pub fn keychain_available(&self) -> bool {
        self.probe.load(Ordering::SeqCst) == PROBE_AVAILABLE
    }

    fn keychain(&self) -> Result<Arc<dyn SecretStore>, VarsError> {
        if self.probe.load(Ordering::SeqCst) == PROBE_PENDING {
            return Err(VarsError::new(
                "still checking the OS keychain; try again in a moment",
            ));
        }
        self.keychain
            .lock()
            .expect("keychain lock poisoned")
            .clone()
            .map(|k| k as Arc<dyn SecretStore>)
            .ok_or_else(|| {
                VarsError::new(
                    "the OS keychain is not available on this system; choose the encrypted file in Secrets settings",
                )
            })
    }

    /// The encrypted-file store for a workspace (created lazily, kept for the session).
    pub fn encrypted(&self, root: &Path) -> Arc<EncryptedFileStore> {
        self.encrypted
            .lock()
            .expect("encrypted store lock poisoned")
            .entry(root.to_path_buf())
            .or_insert_with(|| Arc::new(EncryptedFileStore::new(root.join(LOCAL_DIR).join("secrets.enc"))))
            .clone()
    }

    fn plaintext(root: &Path) -> Arc<dyn SecretStore> {
        Arc::new(PlaintextFileStore::new(root.join(LOCAL_DIR).join("secrets.json")))
    }

    /// The store for a workspace's chosen backend.
    pub fn workspace_store(
        &self,
        backend: SecretBackend,
        root: &Path,
    ) -> Result<Arc<dyn SecretStore>, VarsError> {
        match backend {
            SecretBackend::Keychain => self.keychain(),
            SecretBackend::EncryptedFile => Ok(self.encrypted(root) as Arc<dyn SecretStore>),
            SecretBackend::PlaintextFile => Ok(Self::plaintext(root)),
        }
    }

    /// The store for global secrets.
    pub fn global_store(&self) -> Result<Arc<dyn SecretStore>, VarsError> {
        self.keychain()
    }
}

/// Workspaces are identified by their resolved folder, which a repo can't choose.
fn choice_key(root: &Path) -> String {
    ironcall_store::path_guard::dunce_canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf())
        .display()
        .to_string()
}

pub fn backend_from_setting(setting: Option<&str>) -> SecretBackend {
    match setting {
        Some("encryptedFile") => SecretBackend::EncryptedFile,
        Some("plaintextFile") => SecretBackend::PlaintextFile,
        _ => SecretBackend::Keychain,
    }
}

pub fn backend_setting(backend: SecretBackend) -> &'static str {
    match backend {
        SecretBackend::Keychain => "keychain",
        SecretBackend::EncryptedFile => "encryptedFile",
        SecretBackend::PlaintextFile => "plaintextFile",
    }
}
