# swarm-quickwins — phase report

Branch: `feat/quickwins` (worktree `/private/tmp/signalui-wt-quickwins`).
Picked 8 of the 10 candidate quick wins from the brief.

## Files changed

### Frontend
- `src/lib/stores/messaging.svelte.ts` — listeners no longer attach at module
  load. New `initListeners()` registers all 7 IPC subscriptions, returns a
  teardown. Idempotent + re-armable. New `sendToRecipient` routes failures
  through `toastStore.error` with retry, mirroring the other send paths.
- `src/App.svelte` — calls `initListeners` on `onMount`, tears down on
  `onDestroy`. Kills HMR / restart pile-ups.
- `src/lib/components/ChatLayout.svelte` — dropped the redundant 5 s
  `loadConversations` poll (`conversations-updated` already covers it).
  New-message form now uses the toast store + keeps the form open on failure.
  **All UI strings → French.**
- `src/lib/components/LinkDevice.svelte` — every English string → French.
- `scripts/check-port.mjs` (new) + `predev` hook in `package.json` — friendly
  stderr message + lsof/kill hint when port 1420 is busy.

### Backend
- `src-tauri/src/store/keychain.rs` — new `delete_db_passphrase_with<K>`
  parametric helper, `delete_db_passphrase()` kept as ergonomic wrapper.
  `MemoryKeychain` + `tempdir` promoted `pub(crate)` for reuse.
- `src-tauri/src/commands/account.rs` (new) — `sign_out` Tauri command +
  pure `sign_out_with`. Wipes keychain + `.db_key{,.bak}`, returns a
  serializable `SignOutReport`. Registered in `invoke_handler!`.
- `src-tauri/src/messaging/types.rs` — +9 modifier event payload-shape tests
  (TypingEvent/EditEvent/DeleteEvent serde, ReceiptKind lowercase, kebab-case
  `InboundEvent` tag for every variant).

### CI
- `.gitea/workflows/ci.yaml` — debug + release `cargo test --lib` pair (was
  release-only). Timeout 25 → 35 min.

## Tests

| Suite      | Before | After | Δ   |
|------------|--------|-------|-----|
| vitest     | 17     | 25    | +8  |
| cargo lib  | 50     | 66    | +16 |
| **Total**  | **67** | **91**| **+24** |

All passing. `npm run build` clean. `cargo check` clean.

New coverage: listener registration count + idempotency + teardown +
re-arm; `sendToRecipient` happy/reject; LinkDevice French copy & banned
English phrases; `check-port` free/busy exit codes; `sign_out_with`
keychain+files wipe + idempotent; `delete_db_passphrase_with` round-trips;
9 modifier payload-shape tests.

## Mocks

- `MemoryKeychain` + `tempdir` shared across `keychain.rs` and
  `commands/account.rs` test modules.
- Frontend: hoisted `listen` mock records subscribe/unsubscribe calls so the
  store's listener accounting is observable.

## Smells flagged (out of scope)

- `app_state.rs` still swallows keychain failure into empty passphrase →
  silent DB corruption on first write. Dedicated lane needed.
- 17 dead-code warnings remain in unused stub helpers — janitor pass.
- `sign_out` only wipes the key; presage `Manager` still holds registered
  state until restart. UI must surface that. Wiping the SQLite file is an
  irreversible follow-up decision.
- E2E not wired into CI yet (Tauri webdriver).

## Merge order

Rebase-clean off `main@26c2593`. Merge after any swimlane editing
`messaging.svelte.ts` / `ChatLayout.svelte` core wiring; merge before
anyone wanting to call `sign_out` from the UI (none in flight).
