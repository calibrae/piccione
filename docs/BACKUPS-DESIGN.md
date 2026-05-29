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
- **AEP persistence (prerequisite):** patch calibrae/presage to add an
  `account_entropy_pool` field to `RegistrationData` and populate it in
  `manager/linking.rs` (it's already in scope there at line 130, currently
  consumed and dropped). Without this the `BackupKey` cannot be derived.
- **Import writer:** map `Frame`→presage `save_message`/`save_contact`/
  `save_group`. The trickiest part: faithfully rebuilding `ChatItem` →
  `Content` so `get_messages` renders them identically to live messages.
- **UI:** a one-shot "Importer l'historique" affordance post-link + progress.

## Risks / unknowns to resolve in the build session

1. **AEP availability — RESOLVED (blocking, needs upstream/fork work).**
   presage *receives* `accountEntropyPool` at link (`manager/linking.rs:130`)
   but only uses it to derive the master key, then **drops it** —
   `RegistrationData` has no AEP field. Because AEP→master_key is one-way, the
   stored master key cannot reconstruct the `BackupKey`. **Therefore Link &
   Sync requires patching presage to persist the AEP in `RegistrationData` at
   link time** (a calibrae/presage fork change + ideally an upstream PR). This
   is the first concrete work item, and it gates everything else.
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

## Status — codec read path PROVEN (2026-05-30)

Implemented + merged to main:
- presage fork persists the AEP (calibrae/presage#1).
- `derive_backup_key` (AEP→BackupKey), `derive_message_backup_key`
  (→MessageBackupKey), `backup_available` command. (#27, #28)
- `validate_backup` (decrypt+decompress+validate) and `summarize_backup`
  (FramesReader→VarintDelimitedReader→Frame loop), behind a default-on
  `backups` feature. (#29)
- **Runtime-validated** against libsignal's canonical-backup.binproto: frame
  decode + the Recipient `destination` oneof both proven on real data (4
  recipients incl. a Self decode). (#30)

Dependency facts (load-bearing): message-backup pinned `tag=v0.91.0` (matches
libsignal-service) resolves clean; its `proto` module is public only under the
`test-util` feature (no extra deps); its generated protos are **rust-protobuf
3.x** (`protobuf3` alias, `Message::parse_from_bytes`), NOT prost;
`CompletedBackup` fields are private so import uses the raw frame loop.

## Remaining (all [LIVE-TEST] — need a real encrypted transfer archive)

1. **Frame→presage store mapping** — `Recipient::Contact`→`save_contact`,
   `::Group`→`save_group`; `ChatItem`→reconstruct `Content`/`DataMessage`→
   `save_message` (the hard part: edits/reactions/quotes/attachments fidelity).
   Compile-verifiable now; correctness needs a real archive.
2. **Transfer-archive request** — `SyncMessage` request to the primary +
   download; protobuf may need regenerating in libsignal-service-rs.
3. **Import UI** — post-link "Importer l'historique" + progress.

The decode foundation is done; the above is the import body, gated on a live
2-device link (Piccione = secondary) for end-to-end validation.
