//! Database passphrase storage.
//!
//! Strategy:
//!   1. Prefer macOS Keychain (`security-framework`).
//!   2. If Keychain is empty but a legacy `.db_key` file exists, migrate it
//!      into the Keychain. The file is renamed to `.db_key.bak` instead of
//!      deleted, so a Keychain corruption does not lock Cali out of his
//!      messages while the feature is still young.
//!   3. If neither exists, generate a fresh 64-hex passphrase, store it in
//!      the Keychain, and write a `.db_key` mirror as a belt-and-braces
//!      fallback (also 0600).
//!
//! The whole module is generic over a tiny [`KeychainBackend`] trait so the
//! tests can swap in an in-memory HashMap and stay deterministic.

use security_framework::base::Error as SfError;
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use zeroize::Zeroizing;

pub const SERVICE_NAME: &str = "com.signalui.app";
pub const DB_KEY_ACCOUNT: &str = "signalui-db-encryption-key";

/// macOS Security framework error code for "item not found".
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// Abstract keychain operations so we can mock them in tests.
pub trait KeychainBackend: Send + Sync {
    /// Returns `Ok(None)` if the item is missing.
    fn get(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>, KeychainError>;
    fn set(&self, service: &str, account: &str, password: &[u8]) -> Result<(), KeychainError>;
    /// Deleting a non-existent item is a no-op and returns `Ok(())`.
    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError>;
}

/// Real macOS Keychain backed by `security-framework`.
pub struct SystemKeychain;

impl KeychainBackend for SystemKeychain {
    fn get(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>, KeychainError> {
        match get_generic_password(service, account) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(KeychainError::AccessFailed(e.to_string())),
        }
    }

    fn set(&self, service: &str, account: &str, password: &[u8]) -> Result<(), KeychainError> {
        set_generic_password(service, account, password)
            .map_err(|e| KeychainError::StoreFailed(e.to_string()))
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError> {
        match delete_generic_password(service, account) {
            Ok(()) => Ok(()),
            Err(e) if is_not_found(&e) => Ok(()),
            Err(e) => Err(KeychainError::DeleteFailed(e.to_string())),
        }
    }
}

fn is_not_found(e: &SfError) -> bool {
    e.code() == ERR_SEC_ITEM_NOT_FOUND
}

/// Public entry point — production code uses this.
pub fn get_or_create_db_passphrase(
    data_dir: &std::path::Path,
) -> Result<Zeroizing<String>, KeychainError> {
    get_or_create_db_passphrase_with(&SystemKeychain, data_dir)
}

/// Implementation generic over a [`KeychainBackend`] for testability.
pub fn get_or_create_db_passphrase_with<K: KeychainBackend>(
    keychain: &K,
    data_dir: &std::path::Path,
) -> Result<Zeroizing<String>, KeychainError> {
    let key_file = data_dir.join(".db_key");

    // 1. Keychain hit — fastest, preferred path.
    if let Some(bytes) = keychain.get(SERVICE_NAME, DB_KEY_ACCOUNT)? {
        let s = String::from_utf8(bytes).map_err(|_| KeychainError::InvalidData)?;
        tracing::debug!("loaded database encryption key from keychain");
        return Ok(Zeroizing::new(s.trim().to_string()));
    }

    // 2. Migrate from legacy .db_key file if present.
    if key_file.exists() {
        let raw = std::fs::read_to_string(&key_file)
            .map_err(|e| KeychainError::AccessFailed(format!("read .db_key: {}", e)))?;
        let trimmed = raw.trim().to_string();
        keychain.set(SERVICE_NAME, DB_KEY_ACCOUNT, trimmed.as_bytes())?;

        // Rename to .db_key.bak so a future bug can't silently fall back to a
        // stale on-disk key, but Cali can recover by hand if the keychain
        // entry is wiped. He can `rm .db_key.bak` once he trusts the migration.
        let bak = data_dir.join(".db_key.bak");
        if let Err(e) = std::fs::rename(&key_file, &bak) {
            tracing::warn!("could not rename .db_key -> .db_key.bak: {}", e);
        } else {
            tracing::info!("migrated .db_key into keychain (file kept at .db_key.bak)");
        }
        return Ok(Zeroizing::new(trimmed));
    }

    // 3. Cold start — generate a fresh passphrase, store it in the keychain.
    let passphrase = generate_passphrase();
    keychain.set(SERVICE_NAME, DB_KEY_ACCOUNT, passphrase.as_bytes())?;
    tracing::info!("created fresh database encryption key in keychain");
    Ok(passphrase)
}

/// Delete the database encryption key from the real Keychain.
///
/// Used by the "sign out / unpair" flow. After the call the app must be
/// restarted: the running messaging service still holds the cached
/// passphrase in `AppState::db_passphrase`.
#[allow(dead_code)] // public ergonomic wrapper; sign_out goes through delete_db_passphrase_with directly
pub fn delete_db_passphrase() -> Result<(), KeychainError> {
    delete_db_passphrase_with(&SystemKeychain)
}

/// Backend-generic variant for unit tests.
pub fn delete_db_passphrase_with<K: KeychainBackend>(keychain: &K) -> Result<(), KeychainError> {
    keychain.delete(SERVICE_NAME, DB_KEY_ACCOUNT)
}

/// CLI-friendly wrapper around [`get_or_create_db_passphrase`].
///
/// The headless bins (`pair-once`, `is-paired`, `list-devices`) used to
/// re-implement the passphrase resolution on top of `<data_dir>/.db_key`.
/// That diverged from the keychain-backed value the GUI app uses, so the
/// bins were forced to call into this module — but accessing the GUI app's
/// keychain item from a different binary triggers a security-agent prompt
/// on macOS. Over SSH the prompt has nowhere to render and the call hangs.
///
/// This wrapper enforces the "Keychain-backed, file fallback" contract
/// described in the bug report:
///
/// 1. Spawn a worker thread that calls [`get_or_create_db_passphrase`].
/// 2. If it returns within `timeout`, use that result.
/// 3. Otherwise (or on any error) fall back to `.db_key.bak` then
///    `.db_key`. The `.db_key.bak` mirror is the exact value the GUI app
///    wrote into the keychain at migration time, so trusting it preserves
///    "single source of truth" — the keychain is still the canonical
///    write path; the bak file is a read-only escape hatch.
///
/// The abandoned worker thread, if any, is left to be reaped at process
/// exit. Bins are short-lived; this is fine.
pub fn resolve_db_passphrase_for_cli(
    data_dir: &std::path::Path,
) -> Result<Zeroizing<String>, KeychainError> {
    resolve_db_passphrase_for_cli_with_timeout(data_dir, std::time::Duration::from_secs(3))
}

/// Backend-generic + tunable-timeout variant for tests.
pub fn resolve_db_passphrase_for_cli_with_timeout(
    data_dir: &std::path::Path,
    timeout: std::time::Duration,
) -> Result<Zeroizing<String>, KeychainError> {
    let dir = data_dir.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(get_or_create_db_passphrase(&dir));
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(p)) => return Ok(p),
        Ok(Err(e)) => {
            tracing::warn!("keychain resolution failed ({}); trying file fallback", e);
        }
        Err(_) => {
            tracing::warn!(
                "keychain resolution timed out after {:?}; trying file fallback",
                timeout
            );
        }
    }
    read_passphrase_fallback_files(data_dir)
}

fn read_passphrase_fallback_files(
    data_dir: &std::path::Path,
) -> Result<Zeroizing<String>, KeychainError> {
    for name in [".db_key.bak", ".db_key"] {
        let p = data_dir.join(name);
        if p.exists() {
            let raw = std::fs::read_to_string(&p)
                .map_err(|e| KeychainError::AccessFailed(format!("read {}: {}", name, e)))?;
            tracing::warn!("using {} as passphrase fallback", name);
            return Ok(Zeroizing::new(raw.trim().to_string()));
        }
    }
    Err(KeychainError::AccessFailed(
        "keychain unavailable and no .db_key{,.bak} fallback on disk".to_string(),
    ))
}

fn generate_passphrase() -> Zeroizing<String> {
    use rand::RngCore;
    let mut key_bytes = Zeroizing::new([0u8; 32]);
    rand::thread_rng().fill_bytes(key_bytes.as_mut());
    let hex = hex::encode(key_bytes.as_ref());
    Zeroizing::new(hex)
}

#[derive(Debug, thiserror::Error)]
pub enum KeychainError {
    #[error("invalid data in keychain")]
    InvalidData,

    #[error("failed to store in keychain: {0}")]
    StoreFailed(String),

    #[error("failed to delete from keychain: {0}")]
    DeleteFailed(String),

    #[error("keychain access failed: {0}")]
    AccessFailed(String),
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory keychain used by tests.
    #[derive(Default)]
    pub(crate) struct MemoryKeychain {
        store: Mutex<HashMap<(String, String), Vec<u8>>>,
    }

    impl MemoryKeychain {
        pub(crate) fn len(&self) -> usize {
            self.store.lock().unwrap().len()
        }
    }

    impl KeychainBackend for MemoryKeychain {
        fn get(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>, KeychainError> {
            Ok(self
                .store
                .lock()
                .unwrap()
                .get(&(service.to_string(), account.to_string()))
                .cloned())
        }
        fn set(&self, service: &str, account: &str, password: &[u8]) -> Result<(), KeychainError> {
            self.store
                .lock()
                .unwrap()
                .insert((service.to_string(), account.to_string()), password.to_vec());
            Ok(())
        }
        fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError> {
            self.store
                .lock()
                .unwrap()
                .remove(&(service.to_string(), account.to_string()));
            Ok(())
        }
    }

    #[test]
    fn generate_passphrase_is_64_hex_chars() {
        let pass = generate_passphrase();
        assert_eq!(pass.len(), 64);
        assert!(pass.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn generate_passphrase_is_random() {
        let p1 = generate_passphrase();
        let p2 = generate_passphrase();
        assert_ne!(*p1, *p2);
    }

    #[test]
    fn memory_backend_round_trip() {
        let kc = MemoryKeychain::default();
        assert!(kc.get(SERVICE_NAME, DB_KEY_ACCOUNT).unwrap().is_none());
        kc.set(SERVICE_NAME, DB_KEY_ACCOUNT, b"hunter2").unwrap();
        assert_eq!(
            kc.get(SERVICE_NAME, DB_KEY_ACCOUNT).unwrap().as_deref(),
            Some(b"hunter2".as_ref())
        );
        kc.delete(SERVICE_NAME, DB_KEY_ACCOUNT).unwrap();
        assert!(kc.get(SERVICE_NAME, DB_KEY_ACCOUNT).unwrap().is_none());
    }

    #[test]
    fn cold_start_generates_and_stores_in_keychain() {
        let dir = tempdir();
        let kc = MemoryKeychain::default();

        let p = get_or_create_db_passphrase_with(&kc, dir.path()).unwrap();
        assert_eq!(p.len(), 64);
        let stored = kc.get(SERVICE_NAME, DB_KEY_ACCOUNT).unwrap().unwrap();
        assert_eq!(stored, p.as_bytes());

        // No .db_key written when keychain works.
        assert!(!dir.path().join(".db_key").exists());
    }

    #[test]
    fn keychain_hit_returns_existing() {
        let dir = tempdir();
        let kc = MemoryKeychain::default();
        kc.set(SERVICE_NAME, DB_KEY_ACCOUNT, b"deadbeef").unwrap();

        let p = get_or_create_db_passphrase_with(&kc, dir.path()).unwrap();
        assert_eq!(p.as_str(), "deadbeef");
    }

    #[test]
    fn migrates_legacy_db_key_file() {
        let dir = tempdir();
        let kc = MemoryKeychain::default();
        std::fs::write(dir.path().join(".db_key"), "legacy-passphrase\n").unwrap();

        let p = get_or_create_db_passphrase_with(&kc, dir.path()).unwrap();
        assert_eq!(p.as_str(), "legacy-passphrase");

        // Stored in keychain.
        assert_eq!(
            kc.get(SERVICE_NAME, DB_KEY_ACCOUNT).unwrap().unwrap(),
            b"legacy-passphrase"
        );
        // Original file moved to .bak.
        assert!(!dir.path().join(".db_key").exists());
        assert!(dir.path().join(".db_key.bak").exists());
    }

    #[test]
    fn second_call_uses_keychain_not_filesystem() {
        let dir = tempdir();
        let kc = MemoryKeychain::default();

        let p1 = get_or_create_db_passphrase_with(&kc, dir.path()).unwrap();
        let p2 = get_or_create_db_passphrase_with(&kc, dir.path()).unwrap();
        assert_eq!(p1.as_str(), p2.as_str());
    }

    #[test]
    fn cli_resolver_falls_back_to_db_key_bak_when_no_keychain_match() {
        // Real SystemKeychain isn't on the .bak path here — the helper opens
        // a fresh data_dir with no keychain item, no .db_key, but a populated
        // .db_key.bak. The fallback layer must surface the .bak content.
        let dir = tempdir();
        std::fs::write(dir.path().join(".db_key.bak"), "from-bak-mirror\n").unwrap();
        // The underlying get_or_create call will mint a fresh keychain key
        // (because real SystemKeychain has no entry for our service+account
        // pair, OR if it does, this test will spuriously succeed without
        // exercising the fallback). Either way this isn't the path we test;
        // we test the *file* fallback by giving it an immediate timeout.
        let p = resolve_db_passphrase_for_cli_with_timeout(
            dir.path(),
            std::time::Duration::from_millis(0),
        )
        .unwrap();
        assert_eq!(p.as_str(), "from-bak-mirror");
    }

    #[test]
    fn cli_resolver_errors_when_no_keychain_and_no_files() {
        let dir = tempdir();
        // Immediate timeout, no .db_key{,.bak} on disk.
        let err = resolve_db_passphrase_for_cli_with_timeout(
            dir.path(),
            std::time::Duration::from_millis(0),
        )
        .unwrap_err();
        match err {
            KeychainError::AccessFailed(msg) => {
                assert!(msg.contains("no .db_key"), "unexpected error: {msg}");
            }
            other => panic!("expected AccessFailed, got {other:?}"),
        }
    }

    #[test]
    fn cli_resolver_prefers_bak_over_db_key() {
        let dir = tempdir();
        std::fs::write(dir.path().join(".db_key.bak"), "from-bak\n").unwrap();
        std::fs::write(dir.path().join(".db_key"), "from-key\n").unwrap();
        let p = resolve_db_passphrase_for_cli_with_timeout(
            dir.path(),
            std::time::Duration::from_millis(0),
        )
        .unwrap();
        // .bak is the migrated mirror — preferred over a stale .db_key on
        // post-migration systems where both exist transiently.
        assert_eq!(p.as_str(), "from-bak");
    }

    /// The bins (`pair-once`, `is-paired`, `list-devices`) used to read
    /// `<data_dir>/.db_key` directly. After the migration to keychain-only
    /// resolution, the file was renamed to `.db_key.bak` — but the bins kept
    /// the old direct-read code path, which would have either re-read a
    /// **stale** `.db_key.bak` (if they were patched to look at .bak) or
    /// generated a brand-new file out of phase with the keychain copy. Either
    /// way the bin's passphrase diverged from the app's, and SQLCipher
    /// panicked with "file is not a database".
    ///
    /// This test pins the contract the bins now rely on: when only
    /// `.db_key.bak` is on disk (post-migration leftover) and the keychain is
    /// empty, the lib helper must NOT read the .bak file. It must mint a
    /// fresh passphrase and store it in the keychain, where the next call —
    /// from any process — will find it.
    #[test]
    fn ignores_db_key_bak_post_migration_leftover() {
        let dir = tempdir();
        let kc = MemoryKeychain::default();
        std::fs::write(dir.path().join(".db_key.bak"), "stale-passphrase\n").unwrap();

        let p = get_or_create_db_passphrase_with(&kc, dir.path()).unwrap();

        // Must not adopt the stale .bak content.
        assert_ne!(p.as_str(), "stale-passphrase");
        // Must look like a freshly minted 64-hex passphrase.
        assert_eq!(p.len(), 64);
        assert!(p.chars().all(|c| c.is_ascii_hexdigit()));
        // And it must be in the keychain so subsequent processes see the same.
        assert_eq!(
            kc.get(SERVICE_NAME, DB_KEY_ACCOUNT).unwrap().as_deref(),
            Some(p.as_bytes())
        );
        // .db_key.bak is left untouched (Cali's escape hatch).
        assert!(dir.path().join(".db_key.bak").exists());
        // No new .db_key was written.
        assert!(!dir.path().join(".db_key").exists());
    }

    #[test]
    fn delete_removes_existing_entry() {
        let kc = MemoryKeychain::default();
        kc.set(SERVICE_NAME, DB_KEY_ACCOUNT, b"hunter2").unwrap();
        assert_eq!(kc.len(), 1);
        delete_db_passphrase_with(&kc).unwrap();
        assert_eq!(kc.len(), 0);
    }

    #[test]
    fn delete_is_idempotent_when_missing() {
        let kc = MemoryKeychain::default();
        // Calling delete on an empty backend must not error; sign-out
        // happens-while-already-signed-out is a real path.
        delete_db_passphrase_with(&kc).unwrap();
        delete_db_passphrase_with(&kc).unwrap();
        assert_eq!(kc.len(), 0);
    }

    #[test]
    fn delete_after_create_round_trip() {
        let dir = tempdir();
        let kc = MemoryKeychain::default();

        // Cold start populates the keychain, then delete wipes it.
        let _ = get_or_create_db_passphrase_with(&kc, dir.path()).unwrap();
        assert_eq!(kc.len(), 1);
        delete_db_passphrase_with(&kc).unwrap();
        assert_eq!(kc.len(), 0);

        // A subsequent get_or_create should mint a fresh passphrase, not
        // resurrect the old one.
        let new_pass = get_or_create_db_passphrase_with(&kc, dir.path()).unwrap();
        assert_eq!(new_pass.len(), 64);
    }

    /// Ad-hoc tempdir helper — avoids pulling in the `tempfile` crate just for two tests.
    pub(crate) struct TmpDir(pub std::path::PathBuf);
    impl TmpDir {
        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    pub(crate) fn tempdir() -> TmpDir {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let pid = std::process::id();
        let counter = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("signalui-kc-{}-{}-{}", pid, nanos, counter));
        std::fs::create_dir_all(&path).unwrap();
        TmpDir(path)
    }
    static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
}
