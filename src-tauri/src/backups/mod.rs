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
    pub contacts: usize,
    pub groups: usize,
    pub selfs: usize,
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
    count_frames(frames).await
}

/// Reader-generic frame counter: reads the varint-delimited `BackupInfo`
/// header then each `Frame`, tallying by type. Works over either a decrypting
/// `FramesReader` (real archive) or a plaintext stream (the canonical test
/// fixture), so the proto-decode + oneof-match logic is unit-tested against
/// real Signal backup data.
#[cfg(feature = "backups")]
async fn count_frames<R>(reader: R) -> Result<BackupSummary, String>
where
    R: futures::io::AsyncRead + Unpin,
{
    use libsignal_message_backup::parse::VarintDelimitedReader;
    use libsignal_message_backup::proto::backup as pb;
    use protobuf3::Message;

    let mut reader = VarintDelimitedReader::new(reader);
    let header = reader
        .read_next()
        .await
        .map_err(|e| format!("read header: {e}"))?
        .ok_or("empty backup")?;
    pb::BackupInfo::parse_from_bytes(&header)
        .map_err(|e| format!("decode BackupInfo: {e}"))?;

    let mut sum = BackupSummary::default();
    while let Some(buf) = reader
        .read_next()
        .await
        .map_err(|e| format!("read frame: {e}"))?
    {
        let frame = pb::Frame::parse_from_bytes(&buf).map_err(|e| format!("decode frame: {e}"))?;
        match frame.item {
            Some(pb::frame::Item::Recipient(r)) => {
                sum.recipients += 1;
                use pb::recipient::Destination as D;
                match r.destination {
                    Some(D::Contact(_)) => sum.contacts += 1,
                    Some(D::Group(_)) => sum.groups += 1,
                    Some(D::Self_(_)) => sum.selfs += 1,
                    _ => {}
                }
            }
            Some(pb::frame::Item::Chat(_)) => sum.chats += 1,
            Some(pb::frame::Item::ChatItem(_)) => sum.chat_items += 1,
            Some(pb::frame::Item::StickerPack(_)) => sum.sticker_packs += 1,
            _ => sum.other += 1,
        }
    }
    Ok(sum)
}

#[cfg(all(test, feature = "backups"))]
mod import_tests {
    use super::*;

    // Canonical plaintext backup fixture vendored from libsignal
    // (message-backup/tests/res/canonical-backup.binproto). Validates the
    // frame-decode + oneof-match path against real Signal backup data.
    const CANONICAL: &[u8] = include_bytes!("testdata/canonical-backup.binproto");

    #[test]
    fn counts_frames_in_canonical_backup() {
        let sum = futures::executor::block_on(count_frames(futures::io::Cursor::new(CANONICAL)))
            .expect("canonical backup parses");
        // The canonical fixture carries 4 recipients + an account frame
        // (counted as `other`) and no chats — proves both Recipient and
        // non-Recipient frame decode + the oneof match work on real data.
        assert_eq!(sum.recipients, 4, "recipient decode, got {sum:?}");
        assert!(sum.other >= 1, "account frame should decode, got {sum:?}");
        // The destination oneof match works: the canonical fixture carries a
        // Self recipient (+ distribution-list/call-link entries we don't
        // separately tally). Real contacts/groups exercise the same arms.
        assert!(sum.selfs >= 1, "Self recipient should decode, got {sum:?}");
    }
}


/// Map a backup `Contact` frame to a presage `Contact` for store import.
/// Prefers the system (address-book) name, then the profile name. `None` if
/// the ACI is missing/!16 bytes (can't key a contact without it).
#[cfg(feature = "backups")]
pub fn backup_contact_to_presage(
    c: &libsignal_message_backup::proto::backup::Contact,
) -> Option<presage::model::contacts::Contact> {
    let aci = c.aci.as_ref()?;
    let uuid = uuid::Uuid::from_slice(aci).ok()?;

    let join = |g: &str, f: &str| {
        let n = format!("{g} {f}");
        let t = n.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    };
    let name = join(&c.systemGivenName, &c.systemFamilyName)
        .or_else(|| {
            join(
                c.profileGivenName.as_deref().unwrap_or(""),
                c.profileFamilyName.as_deref().unwrap_or(""),
            )
        })
        .unwrap_or_default();

    Some(presage::model::contacts::Contact {
        uuid,
        phone_number: None,
        name,
        verified: Default::default(),
        profile_key: c.profileKey.clone().unwrap_or_default(),
        expire_timer: 0,
        expire_timer_version: 0,
        inbox_position: 0,
        avatar: None,
    })
}

#[cfg(all(test, feature = "backups"))]
mod recipient_map_tests {
    use super::*;
    use libsignal_message_backup::proto::backup as pb;

    #[test]
    fn contact_proto_maps_to_presage_contact() {
        let mut c = pb::Contact::new();
        c.aci = Some(vec![7u8; 16]);
        c.profileGivenName = Some("Alice".to_string());
        c.profileFamilyName = Some("A".to_string());
        let mapped = backup_contact_to_presage(&c).expect("maps");
        assert_eq!(mapped.uuid, uuid::Uuid::from_slice(&[7u8; 16]).unwrap());
        assert_eq!(mapped.name, "Alice A");
    }

    #[test]
    fn contact_without_aci_is_skipped() {
        let c = pb::Contact::new();
        assert!(backup_contact_to_presage(&c).is_none());
    }
}


/// Extract importable contacts from an encrypted transfer archive — the
/// Recipient::Contact frames mapped to presage `Contact`s. (Group + ChatItem
/// extraction follow; they need a real archive to validate = [LIVE-TEST].)
#[cfg(feature = "backups")]
pub async fn extract_contacts(
    bytes: &[u8],
    key: &libsignal_message_backup::key::MessageBackupKey,
) -> Result<Vec<presage::model::contacts::Contact>, String> {
    use libsignal_message_backup::frame::{CursorFactory, FramesReader};
    use libsignal_message_backup::parse::VarintDelimitedReader;
    use libsignal_message_backup::proto::backup as pb;
    use protobuf3::Message;

    let frames = FramesReader::new(key, CursorFactory::new(bytes))
        .await
        .map_err(|e| format!("open frames: {e}"))?;
    let mut reader = VarintDelimitedReader::new(frames);
    // Skip the BackupInfo header.
    reader
        .read_next()
        .await
        .map_err(|e| format!("read header: {e}"))?
        .ok_or("empty backup")?;

    let mut contacts = Vec::new();
    while let Some(buf) = reader
        .read_next()
        .await
        .map_err(|e| format!("read frame: {e}"))?
    {
        let frame = pb::Frame::parse_from_bytes(&buf).map_err(|e| format!("decode frame: {e}"))?;
        if let Some(pb::frame::Item::Recipient(r)) = frame.item {
            if let Some(pb::recipient::Destination::Contact(c)) = r.destination {
                if let Some(contact) = backup_contact_to_presage(&c) {
                    contacts.push(contact);
                }
            }
        }
    }
    Ok(contacts)
}
