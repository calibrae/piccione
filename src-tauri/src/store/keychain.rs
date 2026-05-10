//! Database passphrase storage.
//!
//! Strategy:
//!   1. Prefer the Data Protection Keychain (`kSecUseDataProtectionKeychain`).
//!      This is the modern macOS API (10.15+) that stores secrets in a
//!      per-user, process-isolated bag — **no ACL prompts** for any binary
//!      running as the same user, regardless of codesigning identity. It's
//!      what iOS and SwiftUI apps use, and it's the right call for signalui.
//!   2. Migrate transparently from the old ACL keychain (SystemKeychain) if an
//!      entry is found there. The old entry is deleted after migration so the
//!      user stops seeing prompts.
//!   3. If Keychain is empty but a legacy `.db_key` file exists, migrate it
//!      into the DP Keychain. The file is renamed to `.db_key.bak` instead of
//!      deleted, so a Keychain corruption does not lock Cali out of his
//!      messages while the feature is still young.
//!   4. If neither exists, generate a fresh 64-hex passphrase, store it in
//!      the DP Keychain.
//!
//! The whole module is generic over a tiny [`KeychainBackend`] trait so the
//! tests can swap in an in-memory HashMap and stay deterministic.

use security_framework::base::Error as SfError;
use security_framework::passwords::{
    delete_generic_password, delete_generic_password_options, generic_password,
    get_generic_password, set_generic_password, set_generic_password_options, PasswordOptions,
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

// ---------------------------------------------------------------------------
// Data Protection Keychain (modern, no ACL prompts)
// ---------------------------------------------------------------------------

/// Data Protection Keychain backend — `kSecUseDataProtectionKeychain = true`.
///
/// Any process running as the same user can read/write without prompts.
/// This is the iOS-style keychain, available on macOS 10.15+.
pub struct DataProtectionKeychain;

impl KeychainBackend for DataProtectionKeychain {
    fn get(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>, KeychainError> {
        let mut opts = PasswordOptions::new_generic_password(service, account);
        opts.use_protected_keychain();
        match generic_password(opts) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(KeychainError::AccessFailed(e.to_string())),
        }
    }

    fn set(&self, service: &str, account: &str, password: &[u8]) -> Result<(), KeychainError> {
        let mut opts = PasswordOptions::new_generic_password(service, account);
        opts.use_protected_keychain();
        // If the item already exists, update it.
        match set_generic_password_options(password, opts) {
            Ok(()) => Ok(()),
            Err(e) => Err(KeychainError::StoreFailed(e.to_string())),
        }
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError> {
        let mut opts = PasswordOptions::new_generic_password(service, account);
        opts.use_protected_keychain();
        match delete_generic_password_options(opts) {
            Ok(()) => Ok(()),
            Err(e) if is_not_found(&e) => Ok(()),
            Err(e) => Err(KeychainError::DeleteFailed(e.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// Legacy ACL Keychain (old API — triggers prompts, kept for migration only)
// ---------------------------------------------------------------------------

/// Real macOS ACL Keychain backed by `security-framework` legacy API.
///
/// Still used for **reading** during one-time migration. Write path switched
/// to [`DataProtectionKeychain`]. Do not use this for new items.
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

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Production entry point.
///
/// Uses [`DataProtectionKeychain`] (no ACL prompts). On first call after an
/// upgrade from the old ACL keychain, migrates the existing entry over and
/// deletes the old one so the user never sees another prompt.
pub fn get_or_create_db_passphrase(
    data_dir: &std::path::Path,
) -> Result<Zeroizing<String>, KeychainError> {
    let dp = DataProtectionKeychain;
    let sys = SystemKeychain;
    get_or_create_db_passphrase_impl(&dp, &sys, data_dir)
}

/// Implementation used by production (two real backends) and by tests (two
/// MemoryKeychain instances standing in for dp and sys).
pub fn get_or_create_db_passphrase_impl<D: KeychainBackend, S: KeychainBackend>(
    dp: &D,
    sys: &S,
    data_dir: &std::path::Path,
) -> Result<Zeroizing<String>, KeychainError> {
    let key_file = data_dir.join(".db_key");

    // 1. DP keychain hit — fastest, preferred path, no prompts.
    if let Some(bytes) = dp.get(SERVICE_NAME, DB_KEY_ACCOUNT)? {
        let s = String::from_utf8(bytes).map_err(|_| KeychainError::InvalidData)?;
        tracing::debug!("loaded database encryption key from data-protection keychain");
        return Ok(Zeroizing::new(s.trim().to_string()));
    }

    // 2. Migrate from old ACL keychain if present.
    //    This fires once, on the first run after upgrading to the DP backend.
    //    After migration the user will never see another ACL prompt.
    if let Some(bytes) = sys.get(SERVICE_NAME, DB_KEY_ACCOUNT)? {
        let s = String::from_utf8(bytes).map_err(|_| KeychainError::InvalidData)?;
        let trimmed = s.trim().to_string();
        dp.set(SERVICE_NAME, DB_KEY_ACCOUNT, trimmed.as_bytes())?;
        // Delete old ACL entry — stops future prompts from other binaries.
        if let Err(e) = sys.delete(SERVICE_NAME, DB_KEY_ACCOUNT) {
            tracing::warn!("could not delete old ACL keychain entry: {}", e);
        } else {
            tracing::info!("migrated ACL keychain entry to data-protection keychain");
        }
        return Ok(Zeroizing::new(trimmed));
    }

    // 3. Migrate from legacy .db_key file if present.
    if key_file.exists() {
        let raw = std::fs::read_to_string(&key_file)
            .map_err(|e| KeychainError::AccessFailed(format!("read .db_key: {}", e)))?;
        let trimmed = raw.trim().to_string();
        dp.set(SERVICE_NAME, DB_KEY_ACCOUNT, trimmed.as_bytes())?;

        let bak = data_dir.join(".db_key.bak");
        if let Err(e) = std::fs::rename(&key_file, &bak) {
            tracing::warn!("could not rename .db_key -> .db_key.bak: {}", e);
        } else {
            tracing::info!("migrated .db_key into data-protection keychain (file kept at .db_key.bak)");
        }
        return Ok(Zeroizing::new(trimmed));
    }

    // 4. Cold start — generate a fresh passphrase, store it in the DP keychain.
    let passphrase = generate_passphrase();
    dp.set(SERVICE_NAME, DB_KEY_ACCOUNT, passphrase.as_bytes())?;
    tracing::info!("created fresh database encryption key in data-protection keychain");
    Ok(passphrase)
}

/// Implementation generic over a single [`KeychainBackend`] for simple tests.
///
/// Uses `keychain` for both dp and sys slots — fine for tests that don't need
/// migration logic, matches the old `get_or_create_db_passphrase_with` signature.
pub fn get_or_create_db_passphrase_with<K: KeychainBackend>(
    keychain: &K,
    data_dir: &std::path::Path,
) -> Result<Zeroizing<String>, KeychainError> {
    // Pass the same backend for both dp and sys slots.
    // Migration path (sys→dp) becomes a no-op because both slots point to the
    // same store — reading from "sys" after "dp" miss returns None (same store,
    // same miss), so tests that don't seed a "sys" entry stay on the happy path.
    get_or_create_db_passphrase_impl(keychain, keychain, data_dir)
}

/// Delete the database encryption key from both keychains.
///
/// Used by the "sign out / unpair" flow. After the call the app must be
/// restarted: the running messaging service still holds the cached
/// passphrase in `AppState::db_passphrase`.
#[allow(dead_code)]
pub fn delete_db_passphrase() -> Result<(), KeychainError> {
    // Clean both keychains — belt-and-braces for users mid-migration.
    let _ = SystemKeychain.delete(SERVICE_NAME, DB_KEY_ACCOUNT);
    DataProtectionKeychain.delete(SERVICE_NAME, DB_KEY_ACCOUNT)
}

/// Backend-generic variant for unit tests.
pub fn delete_db_passphrase_with<K: KeychainBackend>(keychain: &K) -> Result<(), KeychainError> {
    keychain.delete(SERVICE_NAME, DB_KEY_ACCOUNT)
}

// ---------------------------------------------------------------------------
// CLI helper
// ---------------------------------------------------------------------------

/// CLI-friendly wrapper around [`get_or_create_db_passphrase`].
///
/// With the Data Protection Keychain the timeout scaffolding is largely
/// vestigial — DP reads never prompt so they return in microseconds. We keep
/// the fallback chain intact in case the DP keychain is unavailable (rare
/// edge cases: locked user session, sandboxing, etc.).
pub fn resolve_db_passphrase_for_cli(
    data_dir: &std::path::Path,
) -> Result<Zeroizing<String>, KeychainError> {
    resolve_db_passphrase_for_cli_with_timeout(data_dir, std::time::Duration::from_secs(5))
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

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
    fn migrates_from_acl_to_dp_keychain() {
        let dir = tempdir();
        let dp = MemoryKeychain::default();
        let sys = MemoryKeychain::default();

        // Seed old ACL keychain, DP keychain empty.
        sys.set(SERVICE_NAME, DB_KEY_ACCOUNT, b"old-acl-key").unwrap();

        let p = get_or_create_db_passphrase_impl(&dp, &sys, dir.path()).unwrap();
        assert_eq!(p.as_str(), "old-acl-key");

        // Must now be in DP keychain.
        assert_eq!(
            dp.get(SERVICE_NAME, DB_KEY_ACCOUNT).unwrap().as_deref(),
            Some(b"old-acl-key".as_ref())
        );
        // Old ACL entry must be gone.
        assert!(sys.get(SERVICE_NAME, DB_KEY_ACCOUNT).unwrap().is_none());
    }

    #[test]
    fn dp_takes_precedence_over_acl() {
        let dir = tempdir();
        let dp = MemoryKeychain::default();
        let sys = MemoryKeychain::default();

        dp.set(SERVICE_NAME, DB_KEY_ACCOUNT, b"dp-key").unwrap();
        sys.set(SERVICE_NAME, DB_KEY_ACCOUNT, b"old-acl-key").unwrap();

        let p = get_or_create_db_passphrase_impl(&dp, &sys, dir.path()).unwrap();
        assert_eq!(p.as_str(), "dp-key");
        // ACL entry untouched (no migration needed when DP already populated).
        assert!(sys.get(SERVICE_NAME, DB_KEY_ACCOUNT).unwrap().is_some());
    }

    #[test]
    fn cli_resolver_falls_back_to_db_key_bak_when_no_keychain_match() {
        let dir = tempdir();
        std::fs::write(dir.path().join(".db_key.bak"), "from-bak-mirror\n").unwrap();
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
        assert_eq!(p.as_str(), "from-bak");
    }

    #[test]
    fn ignores_db_key_bak_post_migration_leftover() {
        let dir = tempdir();
        let kc = MemoryKeychain::default();
        std::fs::write(dir.path().join(".db_key.bak"), "stale-passphrase\n").unwrap();

        let p = get_or_create_db_passphrase_with(&kc, dir.path()).unwrap();

        assert_ne!(p.as_str(), "stale-passphrase");
        assert_eq!(p.len(), 64);
        assert!(p.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(
            kc.get(SERVICE_NAME, DB_KEY_ACCOUNT).unwrap().as_deref(),
            Some(p.as_bytes())
        );
        assert!(dir.path().join(".db_key.bak").exists());
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
        delete_db_passphrase_with(&kc).unwrap();
        delete_db_passphrase_with(&kc).unwrap();
        assert_eq!(kc.len(), 0);
    }

    #[test]
    fn delete_after_create_round_trip() {
        let dir = tempdir();
        let kc = MemoryKeychain::default();

        let _ = get_or_create_db_passphrase_with(&kc, dir.path()).unwrap();
        assert_eq!(kc.len(), 1);
        delete_db_passphrase_with(&kc).unwrap();
        assert_eq!(kc.len(), 0);

        let new_pass = get_or_create_db_passphrase_with(&kc, dir.path()).unwrap();
        assert_eq!(new_pass.len(), 64);
    }

    /// Ad-hoc tempdir helper — avoids pulling in the `tempfile` crate just for tests.
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
