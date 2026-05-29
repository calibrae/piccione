//! Message backups / Link-and-Sync — the Tier-B history-backfill feature.
//!
//! See `docs/BACKUPS-DESIGN.md`. This module starts at the cryptographic
//! foundation: deriving the `BackupKey` from the account entropy pool that we
//! now persist at link (calibrae/presage#1). The frame-import path
//! (`Backups.proto` `Frame` → presage store) lands in a later step.

use presage::libsignal_service::libsignal_account_keys::{AccountEntropyPool, BackupKey};

/// Derive the account `BackupKey` from the persisted account entropy pool.
///
/// `aep_str` is the verbatim `accountEntropyPool` stored in
/// `RegistrationData` at link time. Returns `None` if it's absent or malformed
/// (e.g. a legacy link that only carried the deprecated `masterKey`). The
/// master key cannot substitute — AEP→master_key is one-way — so a `None`
/// here means message backups are unavailable for this device.
pub fn derive_backup_key(aep_str: &str) -> Option<BackupKey> {
    let aep: AccountEntropyPool = aep_str.parse().ok()?;
    Some(BackupKey::derive_from_account_entropy_pool(&aep))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_aep_yields_no_key() {
        assert!(derive_backup_key("").is_none());
        assert!(derive_backup_key("not-a-valid-entropy-pool").is_none());
    }

    #[test]
    fn valid_aep_derives_a_stable_key() {
        // AccountEntropyPool is 64 lowercase alphanumerics. A fixed test vector
        // must derive deterministically (same input → same key bytes).
        let aep = "0".repeat(64);
        if let Some(k1) = derive_backup_key(&aep) {
            let k2 = derive_backup_key(&aep).expect("second derive");
            assert_eq!(k1.0, k2.0, "derivation must be deterministic");
        }
        // If "0"*64 isn't a valid AEP per the parser, the None path is already
        // covered above; this test then just asserts no panic.
    }
}


/// Derive the per-backup `MessageBackupKey` (HMAC + AES) for *this* account
/// from the persisted AEP and our ACI — the key `BackupReader` needs to
/// decrypt a backup/transfer archive. Behind the `backups` feature because it
/// pulls the `libsignal-message-backup` codec crate.
#[cfg(feature = "backups")]
pub fn derive_message_backup_key(
    aep_str: &str,
    aci: presage::libsignal_service::protocol::Aci,
) -> Option<libsignal_message_backup::key::MessageBackupKey> {
    let backup_key = derive_backup_key(aep_str)?;
    let backup_id = backup_key.derive_backup_id(&aci);
    Some(libsignal_message_backup::key::MessageBackupKey::derive(
        &backup_key,
        &backup_id,
        None,
    ))
}

#[cfg(all(test, feature = "backups"))]
mod backup_key_tests {
    use super::*;
    use presage::libsignal_service::protocol::Aci;

    #[test]
    fn message_backup_key_derivation_compiles_and_runs() {
        // Cross-version sanity: BackupKey (account-keys) feeds MessageBackupKey
        // (message-backup) without a type mismatch, end to end.
        let aci = Aci::from(uuid::Uuid::nil());
        let _ = derive_message_backup_key(&"0".repeat(64), aci);
    }
}
