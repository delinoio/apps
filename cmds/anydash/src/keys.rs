// SPDX-License-Identifier: Apache-2.0
//! Data-key sources, AES-256-GCM sealing and atomic key rotation.
use crate::{
    error::{Failure, Result},
    pb::{ErrorReason, KeySource},
    settings::{Dirs, Settings},
    store::Store,
};
use base64::Engine as _;
use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
pub const SERVICE: &str = "io.delino.anydash";
const MAIN: &str = "data-key";
const PENDING: &str = "data-key-pending";
/// The OS secure store is unavailable (no keychain, no session bus or a locked keyring).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unavailable;
type StoreResult<T> = std::result::Result<T, Unavailable>;
/// Read-only or read-write access to an OS secure store.
pub trait SecureStore: Send + Sync {
    fn get(&self, service: &str, account: &str) -> StoreResult<Option<String>>;
    fn set(&self, service: &str, account: &str, value: &str) -> StoreResult<()>;
    fn delete(&self, service: &str, account: &str) -> StoreResult<()>;
}
/// The platform store: macOS Keychain, Windows Credential Manager or Linux Secret Service.
pub struct OsStore;
fn entry(service: &str, account: &str) -> StoreResult<keyring::Entry> {
    #[cfg(windows)]
    let entry = keyring::Entry::new_with_target(&format!("{service}/{account}"), service, account);
    #[cfg(not(windows))]
    let entry = keyring::Entry::new(service, account);
    entry.map_err(|_| Unavailable)
}
impl SecureStore for OsStore {
    fn get(&self, service: &str, account: &str) -> StoreResult<Option<String>> {
        match entry(service, account)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(Unavailable),
        }
    }
    fn set(&self, service: &str, account: &str, value: &str) -> StoreResult<()> {
        entry(service, account)?
            .set_password(value)
            .map_err(|_| Unavailable)
    }
    fn delete(&self, service: &str, account: &str) -> StoreResult<()> {
        match entry(service, account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(Unavailable),
        }
    }
}
/// An in-memory store for tests and for platforms without a secure store.
#[derive(Default)]
pub struct MemoryStore {
    pub values: Mutex<BTreeMap<(String, String), String>>,
    pub unavailable: bool,
}
impl SecureStore for MemoryStore {
    fn get(&self, service: &str, account: &str) -> StoreResult<Option<String>> {
        if self.unavailable {
            return Err(Unavailable);
        }
        Ok(self
            .values
            .lock()
            .map_err(|_| Unavailable)?
            .get(&(service.to_owned(), account.to_owned()))
            .cloned())
    }
    fn set(&self, service: &str, account: &str, value: &str) -> StoreResult<()> {
        if self.unavailable {
            return Err(Unavailable);
        }
        self.values
            .lock()
            .map_err(|_| Unavailable)?
            .insert((service.to_owned(), account.to_owned()), value.to_owned());
        Ok(())
    }
    fn delete(&self, service: &str, account: &str) -> StoreResult<()> {
        if self.unavailable {
            return Err(Unavailable);
        }
        self.values
            .lock()
            .map_err(|_| Unavailable)?
            .remove(&(service.to_owned(), account.to_owned()));
        Ok(())
    }
}
#[derive(Clone)]
pub struct DataKey {
    bytes: [u8; 32],
    pub id: String,
}
impl std::fmt::Debug for DataKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DataKey({})", self.id)
    }
}
fn unavailable(message: &str) -> Failure {
    Failure::local(ErrorReason::KeyUnavailable, message)
}
impl DataKey {
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self {
            id: hex::encode(&Sha256::digest(bytes)[..8]),
            bytes,
        }
    }
    pub fn generate() -> Result<Self> {
        let mut bytes = [0; 32];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| Failure::internal("Random number generation failed."))?;
        Ok(Self::from_bytes(bytes))
    }
    /// Parse base64url (padded or unpadded) text holding exactly 32 bytes.
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim().trim_end_matches('=');
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(text)
            .ok()
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .ok_or_else(|| unavailable("The encryption key must be base64url of 32 bytes."))?;
        Ok(Self::from_bytes(bytes))
    }
    #[must_use]
    pub fn encode(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.bytes)
    }
    fn cipher(&self) -> Result<LessSafeKey> {
        UnboundKey::new(&AES_256_GCM, &self.bytes)
            .map(LessSafeKey::new)
            .map_err(|_| Failure::internal("Encryption is unavailable."))
    }
    /// Encrypt with a random 96-bit nonce; `aad` binds the ciphertext to its secret name.
    pub fn seal(&self, aad: &str, plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        let mut nonce = [0; 12];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| Failure::internal("Random number generation failed."))?;
        let mut buffer = plaintext.to_vec();
        self.cipher()?
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad.as_bytes()),
                &mut buffer,
            )
            .map_err(|_| Failure::internal("Encryption failed."))?;
        Ok((nonce.to_vec(), buffer))
    }
    pub fn open(&self, aad: &str, nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
        let nonce = <[u8; 12]>::try_from(nonce)
            .map_err(|_| unavailable("A stored secret is unreadable."))?;
        let mut buffer = ciphertext.to_vec();
        let plain = self
            .cipher()?
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad.as_bytes()),
                &mut buffer,
            )
            .map_err(|_| unavailable("A stored secret cannot be decrypted with this key."))?;
        Ok(plain.to_vec())
    }
}
/// Fault injection points for rotation tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotateFault {
    /// Fail while re-encrypting, before the transaction commits.
    BeforeCommit,
    /// Stop after the commit and before promotion, as if the process crashed.
    AfterCommit,
}
pub struct Keys {
    pub source: KeySource,
    pub key: DataKey,
    file: PathBuf,
    os: Arc<dyn SecureStore>,
}
impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keys({:?}, {})", self.source, self.key.id)
    }
}
fn key_file(dirs: &Dirs) -> PathBuf {
    dirs.config.join("encryption.key")
}
#[cfg(unix)]
fn group_or_other_readable(path: &Path) -> Result<bool> {
    use std::os::unix::fs::PermissionsExt as _;
    Ok(std::fs::metadata(path)?.permissions().mode() & 0o077 != 0)
}
#[cfg(not(unix))]
fn group_or_other_readable(_: &Path) -> Result<bool> {
    // Windows profile directories are private to the user by default ACLs.
    Ok(false)
}
fn read_key_file(path: &Path) -> Result<DataKey> {
    if group_or_other_readable(path)? {
        return Err(unavailable(
            "The key file is readable by group or others; restrict it to mode 0600.",
        ));
    }
    DataKey::parse(&std::fs::read_to_string(path)?)
}
/// Write a key file with mode 0600 through a temporary file and an atomic rename.
fn write_key_file(path: &Path, key: &DataKey) -> Result<()> {
    crate::store::private_dir(path.parent().unwrap_or_else(|| Path::new(".")))?;
    let temp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&temp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&temp)?;
    std::io::Write::write_all(&mut file, key.encode().as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temp, path)?;
    Ok(())
}
impl Keys {
    /// Resolve the data key. `ANYDASH_KEY_SOURCE=os|env|file` forces one source. Otherwise an
    /// existing key is used in the order OS store, environment, key file; a new key is created
    /// in the OS store when it is available and in the key file otherwise.
    pub fn load(settings: &Settings, dirs: &Dirs, os: &Arc<dyn SecureStore>) -> Result<Self> {
        let file = key_file(dirs);
        let forced = match settings.var("ANYDASH_KEY_SOURCE") {
            None => None,
            Some("os") => Some(KeySource::OsSecureStore),
            Some("env") => Some(KeySource::Environment),
            Some("file") => Some(KeySource::KeyFile),
            Some(_) => return Err(unavailable("ANYDASH_KEY_SOURCE must be os, env or file.")),
        };
        let env = settings.var("ANYDASH_ENCRYPTION_KEY");
        let make = |source, key| Self {
            source,
            key,
            file: file.clone(),
            os: os.clone(),
        };
        let os_value = if matches!(forced, None | Some(KeySource::OsSecureStore)) {
            os.get(SERVICE, MAIN)
        } else {
            Err(Unavailable)
        };
        match forced {
            Some(KeySource::OsSecureStore) => {
                let key = match os_value {
                    Ok(Some(v)) => DataKey::parse(&v)?,
                    Ok(None) => {
                        let key = DataKey::generate()?;
                        os.set(SERVICE, MAIN, &key.encode())
                            .map_err(|Unavailable| {
                                unavailable("The OS secure store is unavailable.")
                            })?;
                        key
                    }
                    Err(Unavailable) => {
                        return Err(unavailable("The OS secure store is unavailable."));
                    }
                };
                return Ok(make(KeySource::OsSecureStore, key));
            }
            Some(KeySource::Environment) => {
                let key = DataKey::parse(
                    env.ok_or_else(|| unavailable("ANYDASH_ENCRYPTION_KEY is not set."))?,
                )?;
                return Ok(make(KeySource::Environment, key));
            }
            Some(_) => {
                let key = if file.exists() {
                    read_key_file(&file)?
                } else {
                    let key = DataKey::generate()?;
                    write_key_file(&file, &key)?;
                    key
                };
                return Ok(make(KeySource::KeyFile, key));
            }
            None => {}
        }
        if let Ok(Some(v)) = &os_value {
            return Ok(make(KeySource::OsSecureStore, DataKey::parse(v)?));
        }
        if let Some(env) = env {
            return Ok(make(KeySource::Environment, DataKey::parse(env)?));
        }
        if file.exists() {
            return Ok(make(KeySource::KeyFile, read_key_file(&file)?));
        }
        let key = DataKey::generate()?;
        if os_value.is_ok() && os.set(SERVICE, MAIN, &key.encode()).is_ok() {
            return Ok(make(KeySource::OsSecureStore, key));
        }
        write_key_file(&file, &key)?;
        Ok(make(KeySource::KeyFile, key))
    }
    fn store_pending(&self, key: &DataKey) -> Result<()> {
        match self.source {
            KeySource::OsSecureStore => self
                .os
                .set(SERVICE, PENDING, &key.encode())
                .map_err(|Unavailable| unavailable("The OS secure store is unavailable.")),
            KeySource::KeyFile => write_key_file(&self.pending_path(), key),
            _ => Ok(()),
        }
    }
    fn pending_path(&self) -> PathBuf {
        self.file.with_extension("key.pending")
    }
    fn delete_pending(&self) {
        match self.source {
            KeySource::OsSecureStore => {
                let _ = self.os.delete(SERVICE, PENDING);
            }
            KeySource::KeyFile => {
                let _ = std::fs::remove_file(self.pending_path());
            }
            _ => {}
        }
    }
    fn load_pending(&self) -> Option<DataKey> {
        match self.source {
            KeySource::OsSecureStore => self
                .os
                .get(SERVICE, PENDING)
                .ok()
                .flatten()
                .and_then(|v| DataKey::parse(&v).ok()),
            KeySource::KeyFile => read_key_file(&self.pending_path()).ok(),
            _ => None,
        }
    }
    /// Make the pending key the main key and delete the old one. Repeating a completed or
    /// partly completed promotion is harmless.
    fn promote(&mut self, key: DataKey, store: &Store) -> Result<()> {
        match self.source {
            KeySource::OsSecureStore => {
                self.os
                    .set(SERVICE, MAIN, &key.encode())
                    .map_err(|Unavailable| unavailable("The OS secure store is unavailable."))?;
                let _ = self.os.delete(SERVICE, PENDING);
            }
            KeySource::KeyFile => {
                write_key_file(&self.file, &key)?;
                let _ = std::fs::remove_file(self.pending_path());
            }
            _ => {}
        }
        self.key = key;
        store.put_meta("pending_key_id", "")?;
        Ok(())
    }
    /// Complete a promotion that a crash interrupted between commit and promotion, and check
    /// that the key matches the stored secrets.
    pub fn recover(&mut self, store: &Store) -> Result<()> {
        let pending = store.meta("pending_key_id")?;
        if !pending.is_empty() {
            if self.key.id == pending {
                store.put_meta("pending_key_id", "")?;
            } else if let Some(key) = self.load_pending().filter(|k| k.id == pending) {
                tracing::info!(event = "key_rotation_recovered", key_id = %key.id);
                self.promote(key, store)?;
            }
        }
        let recorded = store.meta("key_id")?;
        if recorded.is_empty() || store.secret_count()? == 0 {
            store.put_meta("key_id", &self.key.id)?;
        } else if recorded != self.key.id {
            return Err(unavailable(
                "The encryption key does not match the stored secrets; supply the current key.",
            ));
        }
        Ok(())
    }
    /// Rotate the data key: store the new key as pending, re-encrypt every secret in one
    /// transaction that records the new key ID, then promote. Failure before the commit deletes
    /// the pending key and keeps the old key and ciphertext. Failure of the promotion after the
    /// commit keeps the new key in memory and as the pending key; the next rotation or start
    /// completes the promotion.
    pub fn rotate(
        &mut self,
        store: &mut Store,
        new_key: Option<DataKey>,
        fault: Option<RotateFault>,
    ) -> Result<u32> {
        let failed = |message: &str| Failure::local(ErrorReason::KeyRotationFailed, message);
        // A committed rotation whose promotion failed leaves the secrets under the pending key,
        // which is then the only stored copy of the current key. Complete that promotion before
        // a new pending key could overwrite or delete it.
        if store.meta("pending_key_id")? == self.key.id {
            let current = self.key.clone();
            self.promote(current, store).map_err(|_| {
                failed(
                    "The previous key rotation could not be completed; the current key stays as the pending key. Retry when the key store is available.",
                )
            })?;
            tracing::info!(event = "key_rotation_recovered", key_id = %self.key.id);
        }
        let new = match (self.source, new_key) {
            (_, Some(key)) => key,
            (KeySource::Environment, None) => {
                return Err(Failure::invalid(
                    "new-key-env",
                    "The key comes from ANYDASH_ENCRYPTION_KEY; supply the new key with --new-key-env.",
                ));
            }
            (_, None) => DataKey::generate().map_err(|_| failed("Key rotation failed."))?,
        };
        if new.id == self.key.id {
            return Err(failed("The new key equals the current key."));
        }
        if self.store_pending(&new).is_err() {
            self.delete_pending();
            return Err(failed("Key rotation failed; the old key remains in place."));
        }
        let Ok(count) = store.reencrypt(&self.key, &new, fault == Some(RotateFault::BeforeCommit))
        else {
            self.delete_pending();
            return Err(failed("Key rotation failed; the old key remains in place."));
        };
        if fault == Some(RotateFault::AfterCommit) {
            return Ok(count);
        }
        tracing::info!(event = "key_rotated", key_id = %new.id, secret_count = count);
        // The secrets and the recorded key ID use the new key from here on, so this process uses
        // it even if the promotion fails; the pending key stays stored until a promotion
        // succeeds.
        self.key = new.clone();
        if self.promote(new, store).is_err() {
            return Err(failed(
                "The secrets were re-encrypted with the new key, but it could not be stored as the main key; it stays as the pending key, and the next rotation or start completes the promotion.",
            ));
        }
        Ok(count)
    }
}
