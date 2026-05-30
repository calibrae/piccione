# Handoff — Piccione (for the next agent)

State as of 2026-05-30 ~08:05 CEST. Read this first, then `docs/FEATURE-MATRIX.md`,
`docs/GROUPS-DESIGN.md`, `docs/BACKUPS-DESIGN.md`, and palazzo (room: piccione).

## Where things stand
- **rc.6 published** (all 3 platforms): https://github.com/calibrae/piccione/releases/tag/v0.3.0-rc.6
- **~35 of Signal-Desktop's ~210 features** — the whole DataMessage send/receive
  surface + UX/identity/search/notifications. Matrix: `docs/FEATURE-MATRIX.md`.
- **Backups / Link & Sync import**: DONE + unit-validated (contacts + 1:1/group
  text + quotes). Only the archive **fetch** is missing (live-infra, see below).
- main is green, 0 open PRs.

## ACTIVE TASK — group-v2 management (create/modify/leave)
Full design + progress: `docs/GROUPS-DESIGN.md`. Status:
1. ✅ **HTTP write verbs** — `create_group`(PUT)/`modify_group`(PATCH) on
   `push_service`. **calibrae/libsignal-service-rs#1 MERGED to `storage-service`.**
2. ⛔ **NEXT BLOCKER: self `ExpiringProfileKeyCredential` fetch.**
   `GroupOperations::encrypt_group_with_credentials(title, "", None, self_credential,
   candidates, rng)` (already exists in libsignal-service-rs/src/groups_v2/operations.rs)
   needs OUR OWN profile-key credential to add ourselves as admin. Only
   `retrieve_profile` exists. Need: a versioned-profile credential request —
   zkgroup `ProfileKeyCredentialRequestContext` + `GET /v1/profile/{aci}/{version}/{credentialRequest}`,
   returning an `ExpiringProfileKeyCredentialResponse` → receive into the credential.
   Do this in the calibrae/libsignal-service-rs fork (branch `groups-v2-write` is
   merged; cut a new branch).
3. ⏭️ Then: `GroupsManager::create_group(secret_params, title, self_cred, candidates)`
   wiring (builders all exist) → presage `Manager::create_group_v2`/`leave_group`
   wrappers (calibrae/presage fork) → piccione `create_group` command + "New group"
   UI. v1 = create (members invited as PENDING — no per-member credential needed) + leave.
4. Validation = `[LIVE-TEST]` (hits Signal's live group server; can't unit-test).

## Repos / forks (CRITICAL)
- piccione: `~/Developer/perso/signalui` (this repo). Remotes: `gitea` + `origin`
  (github.com/calibrae/piccione). Work on `origin`/calibrae.
- **Forks** (piccione `src-tauri/Cargo.toml` `[patch]` points at local paths for
  dev; CI rewrites to the `calibrae/*` `storage-service` branch):
  - presage: `~/Developer/perso/presage` (calibrae/presage, branch `storage-service`)
  - libsignal-service-rs: `~/Developer/perso/libsignal-service-rs` (calibrae, branch `storage-service`)
- **2 fork PRs merged this session:** presage#1 (persist accountEntropyPool, for
  Backups), libsignal-service-rs#1 (group write verbs).
- Self-merge fork PRs with `gh pr merge N --repo calibrae/REPO --merge --admin`.

## Build / test
- backend: `cd src-tauri && cargo build --lib` · `cargo test --lib` (93 tests)
- **Windows guard:** `cargo check --lib --no-default-features` must pass (voice +
  backups features off; commands have `#[cfg(not(feature=...))]` stubs).
- frontend: `npm run build` · `npx svelte-check` · `npm test -- --run` (34 tests)
- **When adding a `listen()` in messaging.svelte.ts, bump the count assertions in
  `src/__tests__/messagingStore.test.ts`.** Component tests mock `get_settings` +
  stub `matchMedia`. Avoid nested `<button>`.

## Implementation pattern (proven across ~30 PRs)
Send feature: `SendRequest::X` variant + `do_send_X` + handler arm in
`messaging/service.rs` → public method → Tauri command in `commands/messaging.rs`
→ register in `lib.rs` `invoke_handler` → store method (invoke + optimistic) →
ChatLayout UI. Receive: parse in `messaging/parse.rs` → `InboundEvent` → emit in
`lib.rs` → store listener → render. 1:1 AND group (both Thread arms). Keep
`--no-default-features` compiling.

## Backups codec facts (if resuming that)
- `backups` feature (default-on). `libsignal-message-backup` tag v0.91.0,
  `features=["test-util"]` (exposes proto, no extra deps). Its protos are
  **rust-protobuf 3.x** (aliased `protobuf3`, `Message::parse_from_bytes`), NOT prost.
- Archive FETCH = WebAPI long-poll `GET /v1/devices/transfer_archive` (needs a
  presage `PushService` method + a live primary). See `docs/BACKUPS-DESIGN.md`.

## Orchestration (overnight autonomous mode)
- CronCreate is NOT enabled here; grytti/prompto/palazzo MCP NOT callable as tools.
- Use `run_in_background` Bash for: a sleep heartbeat (re-invokes you), self-merging
  CI watchers (poll `gh pr checks` → `gh pr merge` on green), long builds. Never
  foreground-block. palazzo via `curl -X POST http://10.10.0.3:6335/ingest` (NDJSON,
  field is `text` not `content`).

## Cali / style
NO AI attribution in commits. Ship first, iterate. Don't ship blind crypto-adjacent
code you can't validate — build it compile-verified, mark `[LIVE-TEST]`, like voice
calls + Backups. He values honest gap-assessment over feature-theatre.
