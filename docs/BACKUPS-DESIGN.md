# Design: Backups & Link-and-Sync history backfill (Tier B #1)

The flagship gap: when you link a new official Signal client, recent **history
transfers**. Piccione currently syncs only *forward* (new `SyncMessage.Sent`
transcripts + storage-service). This doc designs the backfill.

## Key finding — the Rust building blocks already exist

The libsignal checkout already in our dependency tree
(`libsignal-2a193a9867decbc4`) ships the crates this needs. This is **wiring,
not writing a codec from scratch**:

| Crate | Gives us |
|---|---|
| `libsignal-message-backup` | `BackupReader` (`new_encrypted_compressed`, `read_all`, `validate_all`), `export` (writer), `frame`, `key`, generated `proto` for `Backups.proto` |
| `libsignal-account-keys` | `AccountEntropyPool`, `BackupKey::derive_from_account_entropy_pool(aep)`, `derive_*` (ec key, media id, encryption keys) |
| `zkgroup` | `api::backups::auth_credential` — backup auth credential for the CDN |
| `libsignal-net` | `svrb.rs` — SVR-B storage/retrieval of the backup key |

We **already receive the `AccountEntropyPool`** at link time: it arrives in the
provisioning `SyncMessage.Keys { accountEntropyPool }` (confirmed in the proto
survey). So the secondary can derive the same `BackupKey` the primary used.

## Two distinct features (don't conflate)

### A. Link & Sync (the one Cali wants) — transfer-archive flow
Newest, simplest, no SVR-B / long-term CDN involved.

1. On link, the **secondary** sends the primary a sync request for a transfer
   archive (`SyncMessage` → `transferArchive` request path).
2. The **primary** exports a backup (`message-backup::export`) of recent
   history, encrypts it with a key derived from the shared AEP, uploads it to a
   **short-lived transfer CDN slot**.
3. The primary sends the secondary a `SyncMessage.TransferArchive { cdn, key }`.
4. The secondary downloads, `BackupReader::new_encrypted_compressed(...)`,
   `read_all()`, and imports `Frame`s into the presage SQLite store.

**Piccione is the secondary**, so we implement **steps 1 + 4** (request +
import). Steps 2-3 are the primary's job (the user's phone) — we don't build
them. This is the tractable half.

### B. Full remote backups (export/restore via SVR-B) — larger
Periodic full-account backup to Signal's backup CDN, key escrowed in SVR-B,
restore on reinstall. Needs backup-auth credentials + SVR-B + media tier.
**Defer** — Link & Sync (A) delivers the visible win without it.

## `Backups.proto` frame model (what we import)

A backup is a stream of `Frame`s after a `BackupInfo` header:
`AccountData`, `Recipient` (Contact/Group/Self/DistributionList/CallLink),
`Chat`, `ChatItem` (the messages — Incoming/Outgoing/Directionless +
SendStatus), `StickerPack`, `AdHocCall`. Import maps these onto presage's
store: `Recipient`→contacts/groups, `ChatItem`→`save_message`
(reconstruct a `Content`/`DataMessage` per item), pinned/expire metadata onto
the relevant rows.

## Integration points in Piccione

- **New crate deps:** `libsignal-message-backup`, `libsignal-account-keys`
  (git-pin to the same rev as our existing `libsignal-*`).
- **Messaging-thread query** (reuse the established oneshot pattern):
  `SendRequest::RequestTransferArchive` and an import routine that runs on the
  presage thread (store writes are `!Send`).
- **AEP access:** `mgr.registration_data()` — confirm the AEP is persisted at
  link; if only the master key is kept, add AEP persistence in the link path.
- **Import writer:** map `Frame`→presage `save_message`/`save_contact`/
  `save_group`. The trickiest part: faithfully rebuilding `ChatItem` →
  `Content` so `get_messages` renders them identically to live messages.
- **UI:** a one-shot "Importer l'historique" affordance post-link + progress.

## Risks / unknowns to resolve in the build session

1. **AEP availability** — is it in presage's persisted `registration_data`, or
   do we need to capture it during provisioning? (Blocking — verify first.)
2. **Transfer-archive sync protobufs** — confirm the exact `SyncMessage`
   request/response fields against Signal-Android/Desktop (the proto may need
   regenerating in libsignal-service-rs).
3. **ChatItem → Content fidelity** — edits, reactions, quotes, attachments
   pointers must round-trip; some may import as plain rows initially.
4. **Version skew** — `BackupInfo.version` / `BackupKey` VERSION const must
   match the primary's.

## Estimate

Link & Sync import (feature A): ~1–2 focused weeks given the crates exist —
most effort is the `Frame`→store mapping + the sync-request protobuf. Full
remote backups (B): +2–3 weeks (SVR-B, backup-auth, media). Recommend shipping
A first.

## Status

Design only. No code. Next: a spike that (1) confirms AEP persistence, (2)
adds the two libsignal crates, (3) reads a test backup file with `BackupReader`
and logs the frames — before touching the store-import path.
