//! Encrypted secrets file: `.ironcall/secrets.enc` (PLAN.md §6.1).
//!
//! Layout: a header (magic, version, Argon2id parameters, salt, nonce) followed by the
//! XChaCha20-Poly1305 ciphertext of a JSON map `{storage_id: value}`. The header up to
//! the salt is authenticated as associated data, so tampering with the KDF parameters
//! or salt fails decryption like a wrong passphrase. Every write re-encrypts the whole
//! map with a fresh nonce. The passphrase-derived key and the decrypted map are held
//! only while unlocked, and are zeroized on lock and drop.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroizing;

use crate::fsutil::write_private_atomic;
use crate::{Error, Result, SecretKey, SecretStore};

const MAGIC: &[u8; 4] = b"ICSE";
const VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
/// magic + version + m_cost + t_cost + p_cost + salt.
const AAD_LEN: usize = 4 + 1 + 4 + 4 + 4 + SALT_LEN;
const HEADER_LEN: usize = AAD_LEN + NONCE_LEN;

/// Argon2id cost parameters, stored in the file header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    /// Memory in KiB.
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl Default for KdfParams {
    /// 64 MiB, 3 passes, 1 lane.
    fn default() -> Self {
        Self {
            m_cost: 64 * 1024,
            t_cost: 3,
            p_cost: 1,
        }
    }
}

impl KdfParams {
    /// Cheap parameters for tests. Never use for real files.
    pub fn insecure_for_tests() -> Self {
        Self {
            m_cost: 8,
            t_cost: 1,
            p_cost: 1,
        }
    }
}

/// Values keyed by storage ID. `SecretString` zeroizes each value on drop.
type SecretMap = BTreeMap<String, SecretString>;

struct Unlocked {
    key: Zeroizing<[u8; 32]>,
    salt: [u8; SALT_LEN],
    params: KdfParams,
    map: SecretMap,
}

/// Secrets in a passphrase-encrypted file. Starts locked.
pub struct EncryptedFileStore {
    path: PathBuf,
    params: KdfParams,
    state: Mutex<Option<Unlocked>>,
}

impl std::fmt::Debug for EncryptedFileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncryptedFileStore")
            .field("path", &self.path)
            .field("unlocked", &self.is_unlocked())
            .finish()
    }
}

impl EncryptedFileStore {
    /// A locked store for the file at `path` (which may not exist yet).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            params: KdfParams::default(),
            state: Mutex::new(None),
        }
    }

    /// KDF parameters used when creating a new file (existing files use their own).
    pub fn with_kdf_params(mut self, params: KdfParams) -> Self {
        self.params = params;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.path.is_file()
    }

    pub fn is_unlocked(&self) -> bool {
        self.lock_state().is_some()
    }

    /// Creates a new, empty file protected by `passphrase` and leaves it unlocked.
    /// Refuses to overwrite an existing file.
    pub fn create(&self, passphrase: &SecretString) -> Result<()> {
        if self.exists() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} already exists", self.path.display()),
            )));
        }
        let mut salt = [0u8; SALT_LEN];
        getrandom::fill(&mut salt).map_err(|e| Error::Backend(e.to_string()))?;
        let unlocked = Unlocked {
            key: derive_key(passphrase, &salt, self.params)?,
            salt,
            params: self.params,
            map: SecretMap::new(),
        };
        write_file(&self.path, &unlocked)?;
        *self.lock_state() = Some(unlocked);
        Ok(())
    }

    /// Decrypts the file with `passphrase` and keeps it unlocked for this session.
    pub fn unlock(&self, passphrase: &SecretString) -> Result<()> {
        let bytes = std::fs::read(&self.path)?;
        let Header {
            params,
            salt,
            nonce,
            ciphertext,
        } = parse_header(&bytes)?;
        let key = derive_key(passphrase, &salt, params)?;
        let cipher = XChaCha20Poly1305::new(&Key::from(*key));
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    &XNonce::from(nonce),
                    Payload {
                        msg: ciphertext,
                        aad: &bytes[..AAD_LEN],
                    },
                )
                .map_err(|_| Error::WrongPassphrase)?,
        );
        let raw: BTreeMap<String, String> = serde_json::from_slice(&plaintext)
            .map_err(|_| Error::Backend("secrets file content is invalid".into()))?;
        let map = raw.into_iter().map(|(k, v)| (k, SecretString::from(v))).collect();
        *self.lock_state() = Some(Unlocked {
            key,
            salt,
            params,
            map,
        });
        Ok(())
    }

    /// Forgets the key and the decrypted values.
    pub fn lock(&self) {
        *self.lock_state() = None;
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, Option<Unlocked>> {
        self.state.lock().expect("encrypted store lock poisoned")
    }

    fn update(&self, f: impl FnOnce(&mut SecretMap)) -> Result<()> {
        let mut state = self.lock_state();
        let unlocked = state.as_mut().ok_or(Error::Locked)?;
        let before = unlocked.map.clone();
        f(&mut unlocked.map);
        if let Err(e) = write_file(&self.path, unlocked) {
            unlocked.map = before;
            return Err(e);
        }
        Ok(())
    }
}

impl SecretStore for EncryptedFileStore {
    fn get(&self, key: &SecretKey) -> Result<Option<SecretString>> {
        let state = self.lock_state();
        let unlocked = state.as_ref().ok_or(Error::Locked)?;
        Ok(unlocked.map.get(&key.storage_id()).cloned())
    }

    fn set(&self, key: &SecretKey, value: &SecretString) -> Result<()> {
        self.update(|map| {
            map.insert(key.storage_id(), value.clone());
        })
    }

    fn delete(&self, key: &SecretKey) -> Result<()> {
        if self.get(key)?.is_none() {
            return Ok(());
        }
        self.update(|map| {
            map.remove(&key.storage_id());
        })
    }
}

fn derive_key(passphrase: &SecretString, salt: &[u8], params: KdfParams) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(params.m_cost, params.t_cost, params.p_cost, Some(32))
        .map_err(|e| Error::Backend(format!("invalid key derivation parameters: {e}")))?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.expose_secret().as_bytes(), salt, key.as_mut())
        .map_err(|e| Error::Backend(format!("key derivation failed: {e}")))?;
    Ok(key)
}

fn write_file(path: &Path, unlocked: &Unlocked) -> Result<()> {
    let plain: BTreeMap<&str, &str> = unlocked
        .map
        .iter()
        .map(|(k, v)| (k.as_str(), v.expose_secret()))
        .collect();
    let json = Zeroizing::new(serde_json::to_vec(&plain).map_err(|e| Error::Backend(e.to_string()))?);

    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|e| Error::Backend(e.to_string()))?;

    let mut out = Vec::with_capacity(HEADER_LEN + json.len() + 16);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&unlocked.params.m_cost.to_le_bytes());
    out.extend_from_slice(&unlocked.params.t_cost.to_le_bytes());
    out.extend_from_slice(&unlocked.params.p_cost.to_le_bytes());
    out.extend_from_slice(&unlocked.salt);
    debug_assert_eq!(out.len(), AAD_LEN);
    let cipher = XChaCha20Poly1305::new(&Key::from(*unlocked.key));
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &json,
                aad: &out[..AAD_LEN],
            },
        )
        .map_err(|_| Error::Backend("encryption failed".into()))?;
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    write_private_atomic(path, &out)?;
    Ok(())
}

struct Header<'a> {
    params: KdfParams,
    salt: [u8; SALT_LEN],
    nonce: [u8; NONCE_LEN],
    ciphertext: &'a [u8],
}

fn parse_header(bytes: &[u8]) -> Result<Header<'_>> {
    let invalid = || Error::Backend("not an IronCall secrets file".into());
    if bytes.len() < HEADER_LEN || &bytes[..4] != MAGIC {
        return Err(invalid());
    }
    if bytes[4] != VERSION {
        return Err(Error::Backend(format!(
            "secrets file version {} is not supported by this version of IronCall",
            bytes[4]
        )));
    }
    let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().expect("4 bytes"));
    let params = KdfParams {
        m_cost: u32_at(5),
        t_cost: u32_at(9),
        p_cost: u32_at(13),
    };
    // Refuse absurd costs from a tampered header rather than allocating gigabytes.
    if params.m_cost > 4 * 1024 * 1024 || params.t_cost > 64 || params.p_cost > 64 {
        return Err(invalid());
    }
    let salt = bytes[17..17 + SALT_LEN].try_into().expect("salt length");
    let nonce = bytes[AAD_LEN..HEADER_LEN].try_into().expect("nonce length");
    Ok(Header {
        params,
        salt,
        nonce,
        ciphertext: &bytes[HEADER_LEN..],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope;

    fn pass(s: &str) -> SecretString {
        SecretString::from(s.to_string())
    }

    fn setup() -> (tempfile::TempDir, PathBuf, SecretKey) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".ironcall").join("secrets.enc");
        let key = SecretKey::new(Some(ironcall_core::ItemId::new()), scope::COLLECTION, "apiKey");
        (tmp, path, key)
    }

    fn store(path: &Path) -> EncryptedFileStore {
        EncryptedFileStore::new(path).with_kdf_params(KdfParams::insecure_for_tests())
    }

    #[test]
    fn round_trip_and_persistence_across_reopen() {
        let (_tmp, path, key) = setup();
        let s = store(&path);
        s.create(&pass("correct horse")).unwrap();
        s.set(&key, &pass("hunter2")).unwrap();
        assert_eq!(s.get(&key).unwrap().unwrap().expose_secret(), "hunter2");

        let reopened = store(&path);
        assert!(!reopened.is_unlocked());
        assert!(matches!(reopened.get(&key), Err(Error::Locked)));
        reopened.unlock(&pass("correct horse")).unwrap();
        assert_eq!(reopened.get(&key).unwrap().unwrap().expose_secret(), "hunter2");

        reopened.delete(&key).unwrap();
        let again = store(&path);
        again.unlock(&pass("correct horse")).unwrap();
        assert!(again.get(&key).unwrap().is_none());
    }

    #[test]
    fn wrong_passphrase_is_rejected() {
        let (_tmp, path, _key) = setup();
        store(&path).create(&pass("right")).unwrap();
        assert!(matches!(
            store(&path).unlock(&pass("wrong")),
            Err(Error::WrongPassphrase)
        ));
    }

    #[test]
    fn tampering_is_detected() {
        let (_tmp, path, key) = setup();
        let s = store(&path);
        s.create(&pass("pw")).unwrap();
        s.set(&key, &pass("value")).unwrap();
        let original = std::fs::read(&path).unwrap();

        // Flip a ciphertext byte, then a salt byte (authenticated header).
        for at in [original.len() - 1, 20] {
            let mut bytes = original.clone();
            bytes[at] ^= 1;
            std::fs::write(&path, &bytes).unwrap();
            assert!(
                matches!(store(&path).unlock(&pass("pw")), Err(Error::WrongPassphrase)),
                "byte {at}"
            );
        }
    }

    #[test]
    fn file_contains_no_plaintext() {
        let (_tmp, path, key) = setup();
        let s = store(&path);
        s.create(&pass("pw")).unwrap();
        s.set(&key, &pass("super-secret-token-value")).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("super-secret-token-value"));
        assert!(!text.contains("apiKey"), "key names are encrypted too");
    }

    #[test]
    fn each_write_uses_a_fresh_nonce() {
        let (_tmp, path, key) = setup();
        let s = store(&path);
        s.create(&pass("pw")).unwrap();
        s.set(&key, &pass("v")).unwrap();
        let a = std::fs::read(&path).unwrap();
        s.set(&key, &pass("v")).unwrap();
        let b = std::fs::read(&path).unwrap();
        assert_ne!(a[AAD_LEN..HEADER_LEN], b[AAD_LEN..HEADER_LEN]);
    }

    #[test]
    fn create_refuses_to_overwrite_and_lock_forgets() {
        let (_tmp, path, key) = setup();
        let s = store(&path);
        s.create(&pass("pw")).unwrap();
        assert!(s.create(&pass("pw")).is_err());
        s.lock();
        assert!(matches!(s.set(&key, &pass("x")), Err(Error::Locked)));
    }

    #[cfg(unix)]
    #[test]
    fn file_is_user_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, path, _key) = setup();
        store(&path).create(&pass("pw")).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn debug_shows_no_values() {
        let (_tmp, path, key) = setup();
        let s = store(&path);
        s.create(&pass("pw")).unwrap();
        s.set(&key, &pass("hunter2")).unwrap();
        let debug = format!("{s:?}");
        assert!(!debug.contains("hunter2") && !debug.contains("pw\""), "{debug}");
    }
}
