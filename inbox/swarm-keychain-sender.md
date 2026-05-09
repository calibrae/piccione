# Swarm: keychain-sender — phase report

Branch: `feat/keychain-sender` (worktree `/private/tmp/signalui-wt-keychain-sender`).

## Files changed
- `src-tauri/src/store/keychain.rs` — rewritten. New `KeychainBackend` trait, `SystemKeychain` for prod, generic `get_or_create_db_passphrase_with(&backend, data_dir)`, public `get_or_create_db_passphrase(data_dir)` signature unchanged so `AppState` keeps compiling. Order: keychain-hit → migrate legacy `.db_key` → generate. Legacy file renamed to `.db_key.bak` (recovery hatch), not deleted.
- `src-tauri/src/messaging/service.rs` — adds `pick_sender_name` (pure), `resolve_sender_name` (async, hits `store.contact_by_id`), `enrich_sender_name` (mutates `ChatMessage`). Wired into both receive loops (via `mgr_download.store()`) and `get_messages`. Outgoing/sync skip enrichment — already "You".

## Tests added
+10 lib tests. 25 → 35, all green (`cargo test --lib`).
- 5 keychain: memory round-trip, cold-start, keychain-hit short-circuit, `.db_key` migration → `.db_key.bak`, idempotent re-call.
- 5 sender: profile preferred, phone fallback, UUID-prefix `~xxxxxxxx`, blank contact, short-UUID no panic.

## Mocks
- `tests::MemoryKeychain` — `Mutex<HashMap>` behind `KeychainBackend`. Real keychain isolated from `cargo test`.
- `tests::make_contact()` builds a `presage::model::contacts::Contact` inline. Pure `pick_sender_name` covers all three resolution paths without a stub store.
- Tiny `TmpDir` helper, avoids pulling `tempfile` for two tests.

## Smells flagged (out of scope)
- `app_state.rs` swallows keychain failure into empty passphrase → locked keychain silently corrupts DB on first write. Worth surfacing.
- `delete_db_passphrase()` now public-but-unused — for "Sign out" UX.
- `messaging/service.rs:391` dead `contact_count` warning — janitor's lane.
- PNI senders parsed as Aci — best-effort, no test account hits this yet.
- Phase 0 still has `messagesContainer` non-`$state` warning — UI lane.

## Merge order
No expected conflicts with siblings:
- **ui-polish** is Svelte-only.
- **logging** touches `lib.rs` startup + `Cargo.toml` features only.
This branch only adds new code in `keychain.rs` + appends helpers/tests in `service.rs`. Rebase-clean off `main@89bb3c4`.

Suggested order: **logging** → **keychain-sender** → **ui-polish**.

Push: gitea + origin. No merge performed.
