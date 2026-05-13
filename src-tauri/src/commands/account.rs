//! Account-level Tauri commands.
//!
//! For v0.1 the only supported operation is "sign out / unpair", which wipes
//! the local DB encryption key from the macOS Keychain plus any on-disk
//! fallbacks (`.db_key`, `.db_key.bak`). The app must be restarted afterwards
//! to clear in-memory presage state and surface the LinkDevice screen.
//!
//! This is intentionally **destructive**: deleting the passphrase makes the
//! encrypted SQLite store unreadable. We do *not* delete the SQLite file
//! itself — the user can wipe `~/Library/Application Support/app.piccione/`
//! manually if they want a clean re-link, but losing the key alone is enough
//! to force a fresh provision on next start.

use tauri::{AppHandle, Manager};
use tracing::{info, warn};

use crate::store::keychain::{self, KeychainBackend, SystemKeychain};

/// Result of a sign-out call. Reports which side-effects actually happened so
/// the UI can render an honest confirmation toast.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct SignOutReport {
    pub keychain_cleared: bool,
    pub db_key_file_removed: bool,
    pub db_key_bak_removed: bool,
}

/// Wipe the DB encryption key. The app must be restarted to fully unpair.
#[tauri::command]
pub async fn sign_out(app: AppHandle) -> Result<SignOutReport, String> {
    info!("sign_out called");

    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {}", e))?;

    let report = sign_out_with(&SystemKeychain, &data_dir).map_err(|e| e.to_string())?;
    info!(
        "sign_out: keychain={} db_key={} db_key_bak={}",
        report.keychain_cleared, report.db_key_file_removed, report.db_key_bak_removed
    );
    Ok(report)
}

/// Backend-generic helper. Lives outside the `#[tauri::command]` so unit
/// tests can drive it with an in-memory keychain and a tempdir.
pub fn sign_out_with<K: KeychainBackend>(
    keychain_backend: &K,
    data_dir: &std::path::Path,
) -> Result<SignOutReport, keychain::KeychainError> {
    keychain::delete_db_passphrase_with(keychain_backend).map_err(|e| {
        warn!("delete_db_passphrase_with: {}", e);
        e
    })?;

    let db_key = data_dir.join(".db_key");
    let db_key_file_removed = remove_if_exists(&db_key);

    let db_key_bak = data_dir.join(".db_key.bak");
    let db_key_bak_removed = remove_if_exists(&db_key_bak);

    Ok(SignOutReport {
        keychain_cleared: true,
        db_key_file_removed,
        db_key_bak_removed,
    })
}

fn remove_if_exists(path: &std::path::Path) -> bool {
    if path.exists() {
        match std::fs::remove_file(path) {
            Ok(()) => true,
            Err(e) => {
                warn!("remove {:?}: {}", path, e);
                false
            }
        }
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::keychain::tests::{tempdir, MemoryKeychain};
    use crate::store::keychain::{DB_KEY_ACCOUNT, SERVICE_NAME};

    #[test]
    fn report_serializes_camel_case_friendly() {
        let r = SignOutReport {
            keychain_cleared: true,
            db_key_file_removed: false,
            db_key_bak_removed: true,
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["keychain_cleared"], true);
        assert_eq!(json["db_key_file_removed"], false);
        assert_eq!(json["db_key_bak_removed"], true);
    }

    #[test]
    fn remove_if_exists_handles_missing() {
        let dir = tempdir();
        let p = dir.path().join("nope");
        assert!(!remove_if_exists(&p));
        std::fs::write(&p, b"x").unwrap();
        assert!(remove_if_exists(&p));
        assert!(!p.exists());
    }

    #[test]
    fn sign_out_wipes_keychain_and_files() {
        let dir = tempdir();
        let kc = MemoryKeychain::default();
        kc.set(SERVICE_NAME, DB_KEY_ACCOUNT, b"some-hex").unwrap();
        std::fs::write(dir.path().join(".db_key"), "stale\n").unwrap();
        std::fs::write(dir.path().join(".db_key.bak"), "older\n").unwrap();

        let report = sign_out_with(&kc, dir.path()).unwrap();
        assert_eq!(
            report,
            SignOutReport {
                keychain_cleared: true,
                db_key_file_removed: true,
                db_key_bak_removed: true,
            }
        );

        // Keychain wiped.
        assert!(kc.get(SERVICE_NAME, DB_KEY_ACCOUNT).unwrap().is_none());
        // Both fallbacks gone.
        assert!(!dir.path().join(".db_key").exists());
        assert!(!dir.path().join(".db_key.bak").exists());
    }

    #[test]
    fn sign_out_is_idempotent() {
        let dir = tempdir();
        let kc = MemoryKeychain::default();

        // First call on an empty system: nothing to clean, but the call must
        // succeed and report `keychain_cleared = true` (the operation
        // succeeded; the fact the slot was already empty is a no-op).
        let report = sign_out_with(&kc, dir.path()).unwrap();
        assert!(report.keychain_cleared);
        assert!(!report.db_key_file_removed);
        assert!(!report.db_key_bak_removed);

        // Second call also fine.
        let report2 = sign_out_with(&kc, dir.path()).unwrap();
        assert_eq!(report, report2);
    }
}
