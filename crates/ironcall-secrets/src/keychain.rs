//! OS keychain store: Windows Credential Manager or Linux Secret Service, through
//! `keyring-core` (PLAN.md §6.1).
//!
//! Windows caps each credential blob at 2,560 bytes, so values are stored as UTF-8
//! bytes and anything over [`CHUNK_SIZE`] is split across `<account>#1..n` entries. The
//! main entry holds either the value inline (`v` + bytes) or the chunk count (`c` + n).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use keyring_core::{CredentialStore, Error as KeyringError};
use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroizing;

use crate::{Error, Result, SecretKey, SecretStore};

/// Largest value stored inline, and the size of each chunk. With the one-byte tag the
/// main entry stays well under Windows' 2,560-byte blob limit.
pub const CHUNK_SIZE: usize = 2000;

/// Keychain service name for every IronCall entry.
pub const SERVICE: &str = "ironcall";

const INLINE: u8 = b'v';
const CHUNKED: u8 = b'c';

/// Raw byte storage under (service, account). Implemented by the real keychain and by
/// [`MemoryBackend`] for tests.
pub trait CredentialBackend: Send + Sync {
    fn get(&self, service: &str, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>>;
    fn set(&self, service: &str, account: &str, bytes: &[u8]) -> Result<()>;
    /// Not an error if there is no entry.
    fn delete(&self, service: &str, account: &str) -> Result<()>;
}

/// Secrets in the OS keychain.
pub struct KeychainStore {
    backend: Arc<dyn CredentialBackend>,
}

impl std::fmt::Debug for KeychainStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeychainStore").finish_non_exhaustive()
    }
}

impl KeychainStore {
    pub fn with_backend(backend: Arc<dyn CredentialBackend>) -> Self {
        Self { backend }
    }

    /// Connects to the platform keychain. Fails with [`Error::Unavailable`] if there is
    /// none (e.g. no Secret Service running) or it doesn't answer within `timeout`.
    pub fn open(timeout: Duration) -> Result<Self> {
        let backend = with_timeout(timeout, KeyringBackend::platform)?;
        Ok(Self::with_backend(Arc::new(backend)))
    }

    /// True if the platform keychain can store, read and delete an entry within
    /// `timeout` (a hung D-Bus must not block the app).
    pub fn probe(timeout: Duration) -> bool {
        with_timeout(timeout, || {
            let store = Self::with_backend(Arc::new(KeyringBackend::platform()?));
            store.probe_once()
        })
        .is_ok()
    }

    fn probe_once(&self) -> Result<()> {
        let account = "ironcall/probe";
        self.backend.set(SERVICE, account, b"probe")?;
        let read = self.backend.get(SERVICE, account)?;
        self.backend.delete(SERVICE, account)?;
        if read.as_deref().map(Vec::as_slice) == Some(b"probe".as_slice()) {
            Ok(())
        } else {
            Err(Error::Unavailable(
                "keychain did not return the probe value".into(),
            ))
        }
    }

    fn chunk_account(account: &str, i: usize) -> String {
        format!("{account}#{i}")
    }

    /// Number of chunks the existing value uses (0 if inline or absent).
    fn existing_chunks(&self, account: &str) -> Result<usize> {
        Ok(match self.backend.get(SERVICE, account)? {
            Some(main) if main.first() == Some(&CHUNKED) => parse_count(&main[1..])?,
            _ => 0,
        })
    }
}

impl SecretStore for KeychainStore {
    fn get(&self, key: &SecretKey) -> Result<Option<SecretString>> {
        let account = key.storage_id();
        let Some(main) = self.backend.get(SERVICE, &account)? else {
            return Ok(None);
        };
        let bytes: Zeroizing<Vec<u8>> = match main.first() {
            Some(&INLINE) => Zeroizing::new(main[1..].to_vec()),
            Some(&CHUNKED) => {
                let n = parse_count(&main[1..])?;
                let mut all = Zeroizing::new(Vec::with_capacity(n * CHUNK_SIZE));
                for i in 1..=n {
                    let chunk = self
                        .backend
                        .get(SERVICE, &Self::chunk_account(&account, i))?
                        .ok_or_else(|| Error::Backend(format!("{key}: chunk {i} of {n} is missing")))?;
                    all.extend_from_slice(&chunk);
                }
                all
            }
            _ => {
                return Err(Error::Backend(format!(
                    "{key}: stored value has an unknown format"
                )));
            }
        };
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| Error::Backend(format!("{key}: stored value is not valid UTF-8")))?;
        Ok(Some(SecretString::from(text.to_string())))
    }

    fn set(&self, key: &SecretKey, value: &SecretString) -> Result<()> {
        let account = key.storage_id();
        let old_chunks = self.existing_chunks(&account)?;
        let bytes = value.expose_secret().as_bytes();

        let new_chunks = if bytes.len() <= CHUNK_SIZE {
            let mut main = Zeroizing::new(Vec::with_capacity(bytes.len() + 1));
            main.push(INLINE);
            main.extend_from_slice(bytes);
            self.backend.set(SERVICE, &account, &main)?;
            0
        } else {
            // Chunks first, then the manifest, so a reader never sees a manifest that
            // points at chunks that aren't written yet.
            let chunks: Vec<&[u8]> = bytes.chunks(CHUNK_SIZE).collect();
            for (i, chunk) in chunks.iter().enumerate() {
                self.backend
                    .set(SERVICE, &Self::chunk_account(&account, i + 1), chunk)?;
            }
            let mut main = vec![CHUNKED];
            main.extend_from_slice(chunks.len().to_string().as_bytes());
            self.backend.set(SERVICE, &account, &main)?;
            chunks.len()
        };

        for i in new_chunks + 1..=old_chunks {
            self.backend.delete(SERVICE, &Self::chunk_account(&account, i))?;
        }
        Ok(())
    }

    fn delete(&self, key: &SecretKey) -> Result<()> {
        let account = key.storage_id();
        let chunks = self.existing_chunks(&account)?;
        self.backend.delete(SERVICE, &account)?;
        for i in 1..=chunks {
            self.backend.delete(SERVICE, &Self::chunk_account(&account, i))?;
        }
        Ok(())
    }
}

fn parse_count(bytes: &[u8]) -> Result<usize> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Error::Backend("stored chunk manifest is invalid".into()))
}

/// Runs `f` on a helper thread and gives up after `timeout`.
fn with_timeout<T: Send + 'static>(
    timeout: Duration,
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("ironcall-keychain".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .map_err(Error::Io)?;
    rx.recv_timeout(timeout)
        .map_err(|_| Error::Unavailable(format!("the keychain did not respond within {timeout:?}")))?
}

// ---------------------------------------------------------------------------------
// Real keychain

/// [`CredentialBackend`] over a `keyring-core` credential store.
pub struct KeyringBackend {
    store: Arc<CredentialStore>,
}

impl KeyringBackend {
    pub fn new(store: Arc<CredentialStore>) -> Self {
        Self { store }
    }

    /// The platform's store: Windows Credential Manager, Linux Secret Service.
    pub fn platform() -> Result<Self> {
        #[cfg(windows)]
        let store: Arc<CredentialStore> =
            windows_native_keyring_store::Store::new().map_err(map_unavailable)?;
        #[cfg(target_os = "linux")]
        let store: Arc<CredentialStore> =
            zbus_secret_service_keyring_store::Store::new().map_err(map_unavailable)?;
        #[cfg(not(any(windows, target_os = "linux")))]
        return Err(Error::Unavailable("no keychain support on this platform".into()));
        #[cfg(any(windows, target_os = "linux"))]
        Ok(Self::new(store))
    }

    fn entry(&self, service: &str, account: &str) -> Result<keyring_core::Entry> {
        self.store.build(service, account, None).map_err(map_error)
    }
}

impl CredentialBackend for KeyringBackend {
    fn get(&self, service: &str, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        match self.entry(service, account)?.get_secret() {
            Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(e) => Err(map_error(e)),
        }
    }

    fn set(&self, service: &str, account: &str, bytes: &[u8]) -> Result<()> {
        self.entry(service, account)?.set_secret(bytes).map_err(map_error)
    }

    fn delete(&self, service: &str, account: &str) -> Result<()> {
        match self.entry(service, account)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(map_error(e)),
        }
    }
}

#[cfg(any(windows, target_os = "linux"))]
fn map_unavailable(e: KeyringError) -> Error {
    match map_error(e) {
        Error::Backend(msg) => Error::Unavailable(msg),
        other => other,
    }
}

/// Maps keyring errors without ever including stored bytes (some variants carry them).
fn map_error(e: KeyringError) -> Error {
    match e {
        KeyringError::NoStorageAccess(p) => Error::Unavailable(p.to_string()),
        KeyringError::PlatformFailure(p) => Error::Backend(p.to_string()),
        KeyringError::BadEncoding(_) | KeyringError::BadDataFormat(..) => {
            Error::Backend("stored value has an unexpected format".into())
        }
        e @ (KeyringError::TooLong(..)
        | KeyringError::Invalid(..)
        | KeyringError::NoDefaultStore
        | KeyringError::NotSupportedByStore(_)
        | KeyringError::BadStoreFormat(_)) => Error::Backend(e.to_string()),
        _ => Error::Backend("keychain error".into()),
    }
}

// ---------------------------------------------------------------------------------
// In-memory backend for tests

/// An in-memory [`CredentialBackend`] that enforces a blob size limit like Windows'.
#[derive(Default)]
pub struct MemoryBackend {
    entries: Mutex<HashMap<(String, String), Vec<u8>>>,
    /// Maximum blob size; `None` for unlimited.
    pub max_blob: Option<usize>,
}

impl MemoryBackend {
    /// A backend with Windows' 2,560-byte blob limit.
    pub fn windows_like() -> Self {
        Self {
            max_blob: Some(2560),
            ..Self::default()
        }
    }

    pub fn len(&self) -> usize {
        self.entries.lock().expect("memory backend lock poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl CredentialBackend for MemoryBackend {
    fn get(&self, service: &str, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        Ok(self
            .entries
            .lock()
            .expect("memory backend lock poisoned")
            .get(&(service.to_string(), account.to_string()))
            .map(|v| Zeroizing::new(v.clone())))
    }

    fn set(&self, service: &str, account: &str, bytes: &[u8]) -> Result<()> {
        if self.max_blob.is_some_and(|max| bytes.len() > max) {
            return Err(Error::Backend(format!(
                "blob of {} bytes is over the limit",
                bytes.len()
            )));
        }
        self.entries
            .lock()
            .expect("memory backend lock poisoned")
            .insert((service.to_string(), account.to_string()), bytes.to_vec());
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> Result<()> {
        self.entries
            .lock()
            .expect("memory backend lock poisoned")
            .remove(&(service.to_string(), account.to_string()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope;

    fn store() -> (Arc<MemoryBackend>, KeychainStore) {
        let backend = Arc::new(MemoryBackend::windows_like());
        (backend.clone(), KeychainStore::with_backend(backend))
    }

    fn key() -> SecretKey {
        SecretKey::new(Some(ironcall_core::ItemId::new()), scope::COLLECTION, "token")
    }

    fn value(len: usize) -> SecretString {
        // Multibyte characters too, so chunk boundaries can split a code point.
        let s: String = "aé€"
            .chars()
            .cycle()
            .scan(0, |n, c| {
                *n += c.len_utf8();
                (*n <= len).then_some(c)
            })
            .collect();
        let pad = len - s.len();
        let s = s + &"x".repeat(pad);
        assert_eq!(s.len(), len);
        SecretString::from(s)
    }

    #[test]
    fn round_trips_every_size() {
        let (_backend, store) = store();
        let key = key();
        for len in [0, 1, 2000, 2001, 4096, 65536] {
            let v = value(len);
            store.set(&key, &v).unwrap();
            let got = store.get(&key).unwrap().unwrap();
            assert_eq!(got.expose_secret(), v.expose_secret(), "len {len}");
        }
    }

    #[test]
    fn large_values_are_chunked_under_the_blob_limit() {
        let (backend, store) = store();
        store.set(&key(), &value(65536)).unwrap();
        // 65,536 / 2,000 = 33 chunks, plus the manifest.
        assert_eq!(backend.len(), 34);
    }

    #[test]
    fn shrinking_a_value_removes_stale_chunks() {
        let (backend, store) = store();
        let key = key();
        store.set(&key, &value(10_000)).unwrap();
        assert_eq!(backend.len(), 6);
        store.set(&key, &value(4096)).unwrap();
        assert_eq!(backend.len(), 4);
        store.set(&key, &value(10)).unwrap();
        assert_eq!(backend.len(), 1);
        assert_eq!(
            store.get(&key).unwrap().unwrap().expose_secret(),
            value(10).expose_secret()
        );
    }

    #[test]
    fn delete_removes_all_chunks() {
        let (backend, store) = store();
        let key = key();
        store.set(&key, &value(9000)).unwrap();
        store.delete(&key).unwrap();
        assert!(backend.is_empty());
        assert!(store.get(&key).unwrap().is_none());
        store.delete(&key).unwrap();
    }

    #[test]
    fn missing_chunk_is_an_error_without_the_value() {
        let (backend, store) = store();
        let key = key();
        store
            .set(&key, &SecretString::from("s3cr3t".repeat(1000)))
            .unwrap();
        backend
            .delete(SERVICE, &format!("{}#2", key.storage_id()))
            .unwrap();
        let err = store.get(&key).unwrap_err().to_string();
        assert!(err.contains("chunk 2"), "{err}");
        assert!(!err.contains("s3cr3t"));
    }

    /// Run with `cargo test -p ironcall-secrets -- --ignored` on a machine with a keychain.
    #[test]
    #[ignore]
    fn real_keychain_round_trip() {
        let available = KeychainStore::probe(Duration::from_secs(3));
        eprintln!("keychain probe: {available}");
        if !available {
            return;
        }
        let store = KeychainStore::open(Duration::from_secs(3)).unwrap();
        let key = key();
        let v = value(4096);
        store.set(&key, &v).unwrap();
        assert_eq!(
            store.get(&key).unwrap().unwrap().expose_secret(),
            v.expose_secret()
        );
        store.delete(&key).unwrap();
        assert!(store.get(&key).unwrap().is_none());
    }
}
