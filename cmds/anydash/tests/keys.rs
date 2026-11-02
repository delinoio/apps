// SPDX-License-Identifier: Apache-2.0
//! Key supply from the OS store, a 0600 key file and an environment variable, and atomic
//! rotation with rollback.
mod support;
use anydash::{
    keys::{DataKey, Keys, MemoryStore, RotateFault, SERVICE, SecureStore, Unavailable},
    pb::{ErrorReason, KeySource},
    store::Store,
};
use std::{path::Path, sync::Arc};
fn open(home: &Path) -> Store {
    Store::open(&home.join("anydash/data/collector.db")).unwrap()
}
fn os() -> Arc<dyn SecureStore> {
    Arc::new(MemoryStore::default())
}
#[test]
fn os_secure_store_is_the_default_source() {
    let home = tempfile::tempdir().unwrap();
    let settings = support::settings(home.path(), &[("ANYDASH_KEY_SOURCE", "")]);
    let memory = Arc::new(MemoryStore::default());
    let os: Arc<dyn SecureStore> = memory.clone();
    let keys = Keys::load(&settings, &settings.dirs(), &os).unwrap();
    assert_eq!(keys.source, KeySource::OsSecureStore);
    assert!(memory.get(SERVICE, "data-key").unwrap().is_some());
    assert!(!settings.dirs().config.join("encryption.key").exists());
    // Reloading uses the stored key.
    assert_eq!(
        Keys::load(&settings, &settings.dirs(), &os).unwrap().key.id,
        keys.key.id
    );
    // Without a usable OS store the key file is created with mode 0600, apart from the data.
    let other = tempfile::tempdir().unwrap();
    let settings = support::settings(other.path(), &[("ANYDASH_KEY_SOURCE", "")]);
    let unavailable: Arc<dyn SecureStore> = Arc::new(MemoryStore {
        unavailable: true,
        ..MemoryStore::default()
    });
    let keys = Keys::load(&settings, &settings.dirs(), &unavailable).unwrap();
    assert_eq!(keys.source, KeySource::KeyFile);
    let file = settings.dirs().config.join("encryption.key");
    assert_ne!(file.parent(), Some(settings.dirs().data.as_path()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[cfg(unix)]
#[test]
fn key_file_readable_by_others_is_refused() {
    use std::os::unix::fs::PermissionsExt as _;
    let home = tempfile::tempdir().unwrap();
    let settings = support::settings(home.path(), &[]);
    Keys::load(&settings, &settings.dirs(), &os()).unwrap();
    let file = settings.dirs().config.join("encryption.key");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let error = Keys::load(&settings, &settings.dirs(), &os()).unwrap_err();
    assert_eq!(error.reason, ErrorReason::KeyUnavailable);
}
#[test]
fn environment_key_and_invalid_values() {
    let home = tempfile::tempdir().unwrap();
    let key = DataKey::generate().unwrap();
    let settings = support::settings(
        home.path(),
        &[
            ("ANYDASH_KEY_SOURCE", "env"),
            ("ANYDASH_ENCRYPTION_KEY", &key.encode()),
        ],
    );
    let keys = Keys::load(&settings, &settings.dirs(), &os()).unwrap();
    assert_eq!(
        (keys.source, keys.key.id),
        (KeySource::Environment, key.id.clone())
    );
    // The environment variable is used without forcing when no OS key exists.
    let settings = support::settings(
        home.path(),
        &[
            ("ANYDASH_KEY_SOURCE", ""),
            ("ANYDASH_ENCRYPTION_KEY", &key.encode()),
        ],
    );
    let unavailable: Arc<dyn SecureStore> = Arc::new(MemoryStore {
        unavailable: true,
        ..MemoryStore::default()
    });
    assert_eq!(
        Keys::load(&settings, &settings.dirs(), &unavailable)
            .unwrap()
            .source,
        KeySource::Environment
    );
    for bad in [
        ("ANYDASH_ENCRYPTION_KEY", "short"),
        ("ANYDASH_KEY_SOURCE", "vault"),
    ] {
        let settings = support::settings(home.path(), &[("ANYDASH_KEY_SOURCE", "env"), bad]);
        assert_eq!(
            Keys::load(&settings, &settings.dirs(), &os())
                .unwrap_err()
                .reason,
            ErrorReason::KeyUnavailable
        );
    }
}
fn seeded(home: &Path, extra: &[(&str, &str)], os: &Arc<dyn SecureStore>) -> (Keys, Store) {
    let settings = support::settings(home, extra);
    let store = open(home);
    let mut keys = Keys::load(&settings, &settings.dirs(), os).unwrap();
    keys.recover(&store).unwrap();
    for name in ["device_credentials", "connection:a", "connection:b"] {
        store.put_secret(&keys.key, name, name.as_bytes()).unwrap();
    }
    (keys, store)
}
#[test]
fn rotation_reencrypts_atomically_for_file_and_os_sources() {
    for source in ["file", "os"] {
        let home = tempfile::tempdir().unwrap();
        let os = os();
        let (mut keys, mut store) = seeded(home.path(), &[("ANYDASH_KEY_SOURCE", source)], &os);
        let old = keys.key.id.clone();
        assert_eq!(keys.rotate(&mut store, None, None).unwrap(), 3);
        assert_ne!(keys.key.id, old);
        assert_eq!(store.meta("key_id").unwrap(), keys.key.id);
        assert_eq!(
            store.secret(&keys.key, "connection:a").unwrap().unwrap(),
            b"connection:a"
        );
        // The promoted key is what a restart loads, and the pending key is gone.
        let settings = support::settings(home.path(), &[("ANYDASH_KEY_SOURCE", source)]);
        let mut reloaded = Keys::load(&settings, &settings.dirs(), &os).unwrap();
        reloaded.recover(&store).unwrap();
        assert_eq!(reloaded.key.id, keys.key.id);
        assert!(
            !settings
                .dirs()
                .config
                .join("encryption.key.pending")
                .exists()
        );
        assert!(os.get(SERVICE, "data-key-pending").unwrap().is_none());
    }
}
#[test]
fn rotation_failure_rolls_back_and_crash_recovery_promotes() {
    let home = tempfile::tempdir().unwrap();
    let os = os();
    let (mut keys, mut store) = seeded(home.path(), &[], &os);
    let old = keys.key.clone();
    let error = keys
        .rotate(&mut store, None, Some(RotateFault::BeforeCommit))
        .unwrap_err();
    assert_eq!(error.reason, ErrorReason::KeyRotationFailed);
    assert_eq!(keys.key.id, old.id);
    assert_eq!(store.meta("key_id").unwrap(), old.id);
    assert_eq!(
        store.secret(&old, "device_credentials").unwrap().unwrap(),
        b"device_credentials"
    );
    let settings = support::settings(home.path(), &[]);
    assert!(
        !settings
            .dirs()
            .config
            .join("encryption.key.pending")
            .exists()
    );
    // A crash between commit and promotion is completed at the next start.
    keys.rotate(&mut store, None, Some(RotateFault::AfterCommit))
        .unwrap();
    assert_eq!(
        keys.key.id, old.id,
        "the in-memory key was not promoted yet"
    );
    let mut restarted = Keys::load(&settings, &settings.dirs(), &os).unwrap();
    assert_eq!(restarted.key.id, old.id);
    restarted.recover(&store).unwrap();
    assert_ne!(restarted.key.id, old.id);
    assert_eq!(
        store
            .secret(&restarted.key, "connection:b")
            .unwrap()
            .unwrap(),
        b"connection:b"
    );
    assert_eq!(store.meta("pending_key_id").unwrap().len(), 0);
}
/// An OS store whose main-key writes fail while `fail_main` is set.
#[derive(Default)]
struct FlakyStore {
    inner: MemoryStore,
    fail_main: std::sync::atomic::AtomicBool,
}
impl SecureStore for FlakyStore {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, Unavailable> {
        self.inner.get(service, account)
    }
    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), Unavailable> {
        if account == "data-key" && self.fail_main.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Unavailable);
        }
        self.inner.set(service, account, value)
    }
    fn delete(&self, service: &str, account: &str) -> Result<(), Unavailable> {
        self.inner.delete(service, account)
    }
}
#[test]
fn failed_promotion_keeps_the_committed_key_until_it_is_promoted() {
    use std::sync::atomic::Ordering;
    let home = tempfile::tempdir().unwrap();
    let flaky = Arc::new(FlakyStore::default());
    let os: Arc<dyn SecureStore> = flaky.clone();
    let env = [("ANYDASH_KEY_SOURCE", "os")];
    let (mut keys, mut store) = seeded(home.path(), &env, &os);
    let old = keys.key.clone();
    // The re-encryption commits, then the OS store rejects the main-key write.
    flaky.fail_main.store(true, Ordering::SeqCst);
    let error = keys.rotate(&mut store, None, None).unwrap_err();
    assert_eq!(error.reason, ErrorReason::KeyRotationFailed);
    let committed = keys.key.clone();
    assert_ne!(committed.id, old.id, "the process uses the committed key");
    assert_eq!(store.meta("key_id").unwrap(), committed.id);
    assert_eq!(
        store.secret(&keys.key, "connection:a").unwrap().unwrap(),
        b"connection:a"
    );
    let pending = || {
        os.get(SERVICE, "data-key-pending")
            .unwrap()
            .map(|v| DataKey::parse(&v).unwrap().id)
    };
    assert_eq!(pending(), Some(committed.id.clone()));
    // Another rotation first completes the promotion; while that still fails, it is refused
    // and the pending key, the only stored copy of the current key, is kept.
    let error = keys.rotate(&mut store, None, None).unwrap_err();
    assert_eq!(error.reason, ErrorReason::KeyRotationFailed);
    assert_eq!(keys.key.id, committed.id);
    assert_eq!(pending(), Some(committed.id.clone()));
    assert_eq!(store.meta("key_id").unwrap(), committed.id);
    // A restart promotes the pending key.
    flaky.fail_main.store(false, Ordering::SeqCst);
    let settings = support::settings(home.path(), &env);
    let mut restarted = Keys::load(&settings, &settings.dirs(), &os).unwrap();
    assert_eq!(restarted.key.id, old.id);
    restarted.recover(&store).unwrap();
    assert_eq!(restarted.key.id, committed.id);
    assert_eq!(pending(), None);
    // Without a restart, the next rotation completes the promotion and rotates again.
    let other = tempfile::tempdir().unwrap();
    let (mut keys, mut store) = seeded(other.path(), &env, &os);
    flaky.fail_main.store(true, Ordering::SeqCst);
    keys.rotate(&mut store, None, None).unwrap_err();
    let committed = keys.key.clone();
    flaky.fail_main.store(false, Ordering::SeqCst);
    assert_eq!(keys.rotate(&mut store, None, None).unwrap(), 3);
    assert_ne!(keys.key.id, committed.id);
    assert_eq!(pending(), None);
    assert_eq!(
        os.get(SERVICE, "data-key")
            .unwrap()
            .map(|v| DataKey::parse(&v).unwrap().id),
        Some(keys.key.id.clone())
    );
    assert_eq!(
        store.secret(&keys.key, "connection:b").unwrap().unwrap(),
        b"connection:b"
    );
}
#[test]
fn environment_rotation_requires_the_new_key() {
    let home = tempfile::tempdir().unwrap();
    let os = os();
    let key = DataKey::generate().unwrap();
    let env = [
        ("ANYDASH_KEY_SOURCE", "env"),
        ("ANYDASH_ENCRYPTION_KEY", key.encode().leak() as &str),
    ];
    let (mut keys, mut store) = seeded(home.path(), &env, &os);
    assert_eq!(
        keys.rotate(&mut store, None, None).unwrap_err().reason,
        ErrorReason::InvalidArgument
    );
    let new = DataKey::generate().unwrap();
    assert_eq!(keys.rotate(&mut store, Some(new.clone()), None).unwrap(), 3);
    // The old environment key no longer matches the stored secrets until the user updates it.
    let settings = support::settings(home.path(), &env);
    let mut stale = Keys::load(&settings, &settings.dirs(), &os).unwrap();
    assert_eq!(
        stale.recover(&store).unwrap_err().reason,
        ErrorReason::KeyUnavailable
    );
    let updated = support::settings(
        home.path(),
        &[
            ("ANYDASH_KEY_SOURCE", "env"),
            ("ANYDASH_ENCRYPTION_KEY", &new.encode()),
        ],
    );
    let mut current = Keys::load(&updated, &updated.dirs(), &os).unwrap();
    current.recover(&store).unwrap();
    assert_eq!(
        store.secret(&current.key, "connection:a").unwrap().unwrap(),
        b"connection:a"
    );
}
#[test]
fn ciphertext_is_bound_to_key_and_name() {
    let key = DataKey::generate().unwrap();
    let (nonce, sealed) = key.seal("a", b"secret").unwrap();
    assert_eq!(key.open("a", &nonce, &sealed).unwrap(), b"secret");
    assert!(key.open("b", &nonce, &sealed).is_err());
    assert!(
        DataKey::generate()
            .unwrap()
            .open("a", &nonce, &sealed)
            .is_err()
    );
    assert_eq!(key.id.len(), 16);
}
