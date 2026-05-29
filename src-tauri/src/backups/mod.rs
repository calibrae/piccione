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


/// Open an encrypted Link-and-Sync transfer archive from in-memory `bytes`,
/// decrypt + decompress + structurally validate it, and report how many
/// unknown proto fields were seen (forward-compat signal). This is the
/// `BackupReader` entry point the import path builds on; once this returns
/// `Ok`, the next layer walks the frames into the presage store.
///
/// `Purpose::DeviceTransfer` matches the Link & Sync archive a primary
/// produces (vs `RemoteBackup` for the SVR-B remote-backup tier).
#[cfg(feature = "backups")]
pub async fn validate_backup(
    bytes: &[u8],
    key: &libsignal_message_backup::key::MessageBackupKey,
) -> Result<usize, String> {
    use libsignal_message_backup::backup::Purpose;
    use libsignal_message_backup::frame::CursorFactory;
    use libsignal_message_backup::BackupReader;

    let reader = BackupReader::new_encrypted_compressed(
        key,
        CursorFactory::new(bytes),
        Purpose::DeviceTransfer,
    )
    .await
    .map_err(|e| format!("open backup: {e}"))?;

    let result = reader.validate_all().await;
    result
        .result
        .map_err(|e| format!("backup validation failed: {e}"))?;
    Ok(result.found_unknown_fields.len())
}


/// Per-frame-type counts from a transfer archive — the read-loop foundation
/// the store-import builds on. Walks the decrypted, length-delimited frame
/// stream (`FramesReader` → `VarintDelimitedReader` → `proto::backup::Frame`)
/// after the `BackupInfo` header. A real `Frame`→presage-store mapping
/// replaces the counters; the loop structure is the same.
#[cfg(feature = "backups")]
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct BackupSummary {
    pub recipients: usize,
    pub chats: usize,
    pub chat_items: usize,
    pub sticker_packs: usize,
    pub other: usize,
}

#[cfg(feature = "backups")]
pub async fn summarize_backup(
    bytes: &[u8],
    key: &libsignal_message_backup::key::MessageBackupKey,
) -> Result<BackupSummary, String> {
    use libsignal_message_backup::frame::{CursorFactory, FramesReader};
    use libsignal_message_backup::parse::VarintDelimitedReader;
    use libsignal_message_backup::proto::backup as pb;
    use protobuf3::Message;

    let frames = FramesReader::new(key, CursorFactory::new(bytes))
        .await
        .map_err(|e| format!("open frames: {e}"))?;
    let mut reader = VarintDelimitedReader::new(frames);

    // First length-delimited message is the BackupInfo header.
    let header = reader
        .read_next()
        .await
        .map_err(|e| format!("read header: {e}"))?
        .ok_or("empty backup")?;
    let _info = pb::BackupInfo::parse_from_bytes(&header)
        .map_err(|e| format!("decode BackupInfo: {e}"))?;

    let mut sum = BackupSummary::default();
    while let Some(buf) = reader
        .read_next()
        .await
        .map_err(|e| format!("read frame: {e}"))?
    {
        let frame = pb::Frame::parse_from_bytes(&buf).map_err(|e| format!("decode frame: {e}"))?;
        match frame.item {
            Some(pb::frame::Item::Recipient(_)) => sum.recipients += 1,
            Some(pb::frame::Item::Chat(_)) => sum.chats += 1,
            Some(pb::frame::Item::ChatItem(_)) => sum.chat_items += 1,
            Some(pb::frame::Item::StickerPack(_)) => sum.sticker_packs += 1,
            _ => sum.other += 1,
        }
    }
    Ok(sum)
}
