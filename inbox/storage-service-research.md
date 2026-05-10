# Signal Storage Service — research dossier

## TL;DR
- The contact-sync hole is **not in our code** and not in presage's wiring — it's in the libsignal-service-rs ⊋ presage stack: **neither implements Signal Storage Service reads.**
- Modern Signal moved real contact sync to the Storage Service. The legacy `SyncMessage::Contacts` we call via `request_contacts()` is a deprecated fallback that modern iPhone primaries respond to with an empty stub.
- We need to vendor `StorageService.proto`, implement the 5-endpoint API client + manifest/record crypto, and surface `Manager::sync_storage()` in presage. **3-4 evenings of work**, plus an upstream PR if we want.

## The Storage Service in 6 lines
- 6 HTTP endpoints under `/v1/storage/*` on Signal's regular service host.
- Auth via short-lived basic-auth from `GET /v1/storage/auth` (separate from primary websocket auth).
- One **manifest** per account — encrypted blob versioned by a `uint64`, contains a list of `(raw_uuid, record_type)` identifiers.
- Each **record** fetched separately via `PUT /v1/storage/read` — encrypted, decoded as a `StorageRecord` oneof.
- Record types we care about for v0.x: `CONTACT` (1), `GROUPV2` (3). Skip the rest until we need them.
- Writes are versioned + conflict-detected (HTTP 409 with new manifest if you're stale). Read-only is enough until the user starts editing contacts from signalui.

## Reference implementation
- Canonical: **Signal-Android** (`signalapp/Signal-Android`).
  - API client: `lib/libsignal-service/src/main/java/org/whispersystems/signalservice/api/storage/StorageServiceApi.kt`
  - Sync orchestrator: `app/src/main/java/org/thoughtcrime/securesms/jobs/StorageSyncJob.kt`
  - Crypto helpers: `core/models-jvm/src/main/java/org/signal/core/models/storageservice/StorageManifestKey.kt`
- Proto: vendored copy at `/tmp/StorageService.proto` (380 lines, fetched from Signal-Android `lib/libsignal-service/src/main/protowire/StorageService.proto`).
- Master key + StorageServiceKey derivation: **already in libsignal-service-rs** at `src/master_key.rs` (`StorageServiceKey`). Half the entry-key plumbing exists — we just don't have the API/crypto/proto on top.

## Crypto layers
1. **Storage auth** — short-lived basic-auth credentials from `/v1/storage/auth`. Username/password pair, sent as HTTP Basic in subsequent storage-service requests.
2. **Manifest** — encrypted with `StorageManifestKey` (derived from master_key with `b"Storage Service Encryption"` HKDF info string, already in `master_key.rs`). Inner format: `ManifestRecord` proto.
3. **Record IKM** — manifest carries a `recordIkm` blob. Each record's encryption key is derived per-record from `(recordIkm, identifier_raw_bytes)`.
4. **Records** — AES-CTR + HMAC. Decoded as `StorageRecord` proto.

## ContactRecord proto (what we want)
```
string  aci, e164, pni
bytes   profileKey, identityKey
string  givenName, familyName, username, systemGivenName, systemFamilyName, systemNickname
bool    blocked, whitelisted, archived, markedUnread, hidden
Name    nickname  // { given, family }
string  note
bytes   aciBinary (16), pniBinary (16)
```
Everything we need to populate the conversation list properly.

## What we need to build

| Layer | File / Crate | Effort |
|---|---|---|
| Proto vendor | `libsignal-service-rs/protobuf/StorageService.proto` + build.rs hook | 30 min |
| API client (5 endpoints) | `libsignal-service-rs/src/storage_service.rs` | 1 evening |
| Manifest decrypt | same file (uses existing `StorageManifestKey`) | 1 evening |
| Per-record key derivation + AES-CTR/HMAC | same file | 1 evening |
| `Manager::sync_storage()` | `presage/src/manager/registered.rs` | 0.5 evening |
| `save_contact()` integration | wire decoded ContactRecord into SqliteStore's existing `contacts` table | 0.5 evening |
| Wire into signalui | `start_after_provisioning_local` calls `sync_storage()` instead of (or alongside) `request_contacts()`, expose a "Sync contacts" Tauri command | 0.5 evening |
| Tests | mock manifest, mock records, round-trip encrypt/decrypt | 1 evening |

**Total: 4-5 evenings.** Mostly mechanical once the crypto is right. The ground truth is in Signal-Android Kotlin so the wire format is unambiguous — we're porting, not designing.

## Tactical stopgap: CDSI for phone lookup (1 evening)

Before Storage Service lands, the **immediate Marianne-unblocker** is enabling the `cdsi` feature already present in libsignal-service-rs:
- presage exposes `manager.lookup_username` (gated `#[cfg(feature = "cdsi")]`); underlying CDSI websocket directory client lives at `libsignal-service-rs/src/websocket/directory.rs`.
- Lets us: enter "+33 6 XX XX XX XX" → resolve to ACI → send message.
- Doesn't populate the sidebar contact list but kills the "where's Marianne" UX wall today.
- Toggle: add `cdsi` to presage's feature list in our Cargo.toml, expose `lookup_phone(phone_number) -> Option<Aci>` Tauri command, add a "Search by phone" mode to the new-message picker.

## Recommended order

1. **Tonight**: ship CDSI lookup (1 evening) — Cali can message Marianne.
2. **This week**: ship Storage Service client in libsignal-service-rs (3-4 evenings) — sidebar populates with real contacts.
3. **Optional upstream**: PR to `signalapp/libsignal-service-rs` (the lower layer) and `whisperfish/presage` (consumer). Whisperfish have been wanting Storage Service for years; this would be a real contribution to the Rust Signal ecosystem.

## Unknowns / risks
- The exact crypto for per-record key derivation needs triple-checking against Signal-Android. AES-CTR vs AES-GCM, HMAC truncation length, info-string for HKDF — small things that cost a debugging evening if wrong.
- Storage Service is over the regular Signal service host, but auth flow is distinct. presage's existing PushService might or might not expose the right plumbing for the storage auth call.
- The `recordIkm` field is relatively new (manifest schema evolves). Older accounts may not have it; older fallback is per-record HKDF off the StorageItemKey derived directly from the master key. Need both code paths.

## Files captured during research
- `/tmp/StorageService.proto` — 380 lines, ready to vendor.
