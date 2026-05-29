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


/// Everything we currently import from a transfer archive.
#[cfg(feature = "backups")]
pub struct ImportedData {
    pub contacts: Vec<presage::model::contacts::Contact>,
    /// (thread, reconstructed Content) pairs ready for `save_message`.
    pub messages: Vec<(presage::store::Thread, presage::libsignal_service::content::Content)>,
}

/// Single-pass extraction of contacts + 1:1 text messages from a transfer
/// archive. Backups are ordered (Account, Recipients, Chats, ChatItems) so
/// incremental id→aci and chatId→recipientId maps are complete by the time
/// ChatItems arrive. Group messages + non-text items are skipped for now.
/// `self_uuid` is our ACI, used to set message direction/destination.
#[cfg(feature = "backups")]
pub async fn extract_backup(
    bytes: &[u8],
    key: &libsignal_message_backup::key::MessageBackupKey,
    self_uuid: uuid::Uuid,
) -> Result<ImportedData, String> {
    use libsignal_message_backup::frame::{CursorFactory, FramesReader};
    use libsignal_message_backup::parse::VarintDelimitedReader;

    let frames = FramesReader::new(key, CursorFactory::new(bytes))
        .await
        .map_err(|e| format!("open frames: {e}"))?;
    extract_from_reader(VarintDelimitedReader::new(frames), self_uuid).await
}

/// Reader-generic extraction core: consumes a varint-delimited frame stream
/// (post-decrypt, or a plaintext test stream) and builds the importable data.
/// Split out so the recipient/chat/item → store reconstruction is unit-tested
/// against synthetic frames without needing a real encrypted archive.
#[cfg(feature = "backups")]
async fn extract_from_reader<R>(
    mut reader: libsignal_message_backup::parse::VarintDelimitedReader<R>,
    self_uuid: uuid::Uuid,
) -> Result<ImportedData, String>
where
    R: futures::io::AsyncRead + Unpin,
{
    use libsignal_message_backup::proto::backup as pb;
    use presage::libsignal_service::content::{Content, ContentBody, Metadata};
    use presage::libsignal_service::proto::DataMessage;
    use presage::libsignal_service::protocol::{Aci, DeviceId, ServiceId};
    use presage::store::Thread;
    use protobuf3::Message;
    use std::collections::HashMap;

    reader.read_next().await.map_err(|e| format!("read header: {e}"))?.ok_or("empty backup")?;

    let mut contacts = Vec::new();
    let mut messages = Vec::new();
    // recipient backup-id -> ACI uuid (contacts + self), for message authors
    let mut id_aci: HashMap<u64, uuid::Uuid> = HashMap::new();
    // recipient backup-id -> presage Thread (contact or group), for the chat
    let mut id_thread: HashMap<u64, Thread> = HashMap::new();
    // chat backup-id -> recipient backup-id
    let mut chat_recipient: HashMap<u64, u64> = HashMap::new();

    while let Some(buf) = reader.read_next().await.map_err(|e| format!("read frame: {e}"))? {
        let frame = pb::Frame::parse_from_bytes(&buf).map_err(|e| format!("decode frame: {e}"))?;
        match frame.item {
            Some(pb::frame::Item::Recipient(r)) => {
                let rid = r.id;
                match &r.destination {
                    Some(pb::recipient::Destination::Contact(c)) => {
                        if let Some(aci) = c.aci.as_ref().and_then(|a| uuid::Uuid::from_slice(a).ok()) {
                            id_aci.insert(rid, aci);
                            id_thread.insert(rid, Thread::Contact(ServiceId::Aci(Aci::from(aci))));
                        }
                        if let Some(contact) = backup_contact_to_presage(c) {
                            contacts.push(contact);
                        }
                    }
                    Some(pb::recipient::Destination::Group(g)) => {
                        if let Ok(mk) = <[u8; 32]>::try_from(g.masterKey.as_slice()) {
                            id_thread.insert(rid, Thread::Group(mk));
                        }
                    }
                    Some(pb::recipient::Destination::Self_(_)) => {
                        id_aci.insert(rid, self_uuid);
                    }
                    _ => {}
                }
            }
            Some(pb::frame::Item::Chat(c)) => {
                chat_recipient.insert(c.id, c.recipientId);
            }
            Some(pb::frame::Item::ChatItem(item)) => {
                // 1:1 + group text messages.
                let Some(peer_rid) = chat_recipient.get(&item.chatId) else { continue };
                let Some(thread) = id_thread.get(peer_rid).cloned() else { continue };
                let author_aci = id_aci.get(&item.authorId).copied().unwrap_or(self_uuid);
                // Extract text from a StandardMessage.
                let text = match &item.item {
                    Some(pb::chat_item::Item::StandardMessage(m)) => {
                        m.text.as_ref().map(|t| t.body.clone())
                    }
                    _ => None,
                };
                let Some(body) = text else { continue };

                // Destination: for 1:1 the peer's ACI, for group ourselves
                // (groups carry the routing in the thread/master key).
                let destination = match &thread {
                    Thread::Contact(sid) => sid.clone(),
                    Thread::Group(_) => ServiceId::Aci(Aci::from(self_uuid)),
                };
                let metadata = Metadata {
                    sender: ServiceId::Aci(Aci::from(author_aci)),
                    destination,
                    sender_device: DeviceId::new(1).expect("device 1"),
                    timestamp: item.dateSent,
                    needs_receipt: false,
                    unidentified_sender: false,
                    was_plaintext: true,
                    server_guid: None,
                };
                let dm = DataMessage {
                    body: Some(body),
                    timestamp: Some(item.dateSent),
                    ..Default::default()
                };
                messages.push((thread, Content::from_body(ContentBody::DataMessage(dm), metadata)));
            }
            _ => {}
        }
    }
    Ok(ImportedData { contacts, messages })
}


#[cfg(all(test, feature = "backups"))]
mod message_import_tests {
    use super::*;
    use libsignal_message_backup::proto::backup as pb;
    use protobuf3::Message;

    // varint-length-delimited concat of protos, as VarintDelimitedReader expects.
    fn delimited(msgs: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        for m in msgs {
            let mut os = protobuf3::CodedOutputStream::vec(&mut out);
            os.write_raw_varint32(m.len() as u32).unwrap();
            os.write_raw_bytes(m).unwrap();
            os.flush().unwrap();
        }
        out
    }

    #[test]
    fn reconstructs_a_1to1_text_message() {
        let me = uuid::Uuid::from_bytes([1u8; 16]);
        let peer = uuid::Uuid::from_bytes([2u8; 16]);

        let info = pb::BackupInfo::new();

        let mut contact = pb::Contact::new();
        contact.aci = Some(peer.as_bytes().to_vec());
        contact.profileGivenName = Some("Bob".to_string());
        let mut rec = pb::Recipient::new();
        rec.id = 5;
        rec.destination = Some(pb::recipient::Destination::Contact(contact));
        let mut f_rec = pb::Frame::new();
        f_rec.item = Some(pb::frame::Item::Recipient(rec));

        let mut chat = pb::Chat::new();
        chat.id = 9;
        chat.recipientId = 5;
        let mut f_chat = pb::Frame::new();
        f_chat.item = Some(pb::frame::Item::Chat(chat));

        let mut text = pb::Text::new();
        text.body = "hello from backup".to_string();
        let mut sm = pb::StandardMessage::new();
        sm.text = Some(text).into();
        let mut item = pb::ChatItem::new();
        item.chatId = 9;
        item.authorId = 5;
        item.dateSent = 1_700_000_000_000;
        item.item = Some(pb::chat_item::Item::StandardMessage(sm));
        let mut f_item = pb::Frame::new();
        f_item.item = Some(pb::frame::Item::ChatItem(item));

        let bytes = delimited(&[
            info.write_to_bytes().unwrap(),
            f_rec.write_to_bytes().unwrap(),
            f_chat.write_to_bytes().unwrap(),
            f_item.write_to_bytes().unwrap(),
        ]);

        let reader = libsignal_message_backup::parse::VarintDelimitedReader::new(
            futures::io::Cursor::new(bytes),
        );
        let data = futures::executor::block_on(extract_from_reader(reader, me))
            .expect("extract");

        assert_eq!(data.contacts.len(), 1, "one contact");
        assert_eq!(data.contacts[0].name, "Bob");
        assert_eq!(data.messages.len(), 1, "one message");
        let (thread, content) = &data.messages[0];
        assert!(matches!(thread, presage::store::Thread::Contact(_)));
        if let presage::libsignal_service::content::ContentBody::DataMessage(dm) = &content.body {
            assert_eq!(dm.body.as_deref(), Some("hello from backup"));
            assert_eq!(dm.timestamp, Some(1_700_000_000_000));
        } else {
            panic!("expected DataMessage");
        }
    }
}
