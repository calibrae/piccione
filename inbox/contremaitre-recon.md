# signalui recon — for the contremaître

**Date**: 2026-05-08. infrakid Gen 6. Pushed up, oriented, did not touch logic.

## Build state

- **Frontend**: ✅ `npm run build` clean. Bundle: 51.5 KB JS / 9.2 KB CSS. 121 modules. One Svelte warning: `ChatLayout.svelte:9` — `messagesContainer: HTMLDivElement` mutated but not declared `$state`. Cosmetic; reactivity won't fire on scroll-to-bottom. Easy fix.
- **Backend**: ✅ `cargo check` clean. 17 dead-code warnings, all in unused stub helpers (`is_linked`, `is_terminal`, `delete_db_passphrase`, keychain consts). No type errors. presage `main` resolves; the `curve25519-dalek` + `libsqlite3-sys` patches do their job.
- **Tests**: ✅ `cargo test --lib` 25/25. ✅ `vitest run` 7/7. Playwright e2e (3 tests) untried — needs `npm run tauri dev`.
- **Test infra**: `tests/integration_test.rs` (mock Tauri, gated `feature="test"`), `store_diagnostic.rs` (debug tool — opens the *real* prod DB, do not put in CI).

## UI state

Three components, all wired, no stubs:

- `LinkDevice.svelte` — full state machine: Idle → Connecting → WaitingForScan (renders SVG QR from backend) → Provisioning → Registered / Error. Cancel + retry buttons.
- `QrCode.svelte` — 27 LoC, `{@html}` of the SVG.
- `ChatLayout.svelte` — 492 LoC, the meat. Sidebar, message pane, input box, attachment picker (`tauri-plugin-dialog`), "new chat" recipient input, optimistic send.

Stores: `provisioning.svelte.ts` (44 LoC, listens `provisioning-state-changed`), `messaging.svelte.ts` (125 LoC, listens `new-message` + `conversations-updated`).

## Backend state

10 Tauri commands, **all implemented against real presage** — no placeholders.

- `provisioning::manager.rs` (275 LoC) — `Manager::link_secondary_device` with 120 s timeout + cancel token + state-change callback to UI.
- `messaging::service.rs` (768 LoC) — single-threaded `LocalSet` runs receive loop (`receive_messages` stream → `content_to_chat_message` → emit `new-message`) plus an mpsc send consumer. Reads open a fresh `SqliteStore` per call — clever sidestep of `!Send`. Attachment upload AND download both wired.
- DB passphrase: **NOT yet in Keychain** despite the file's name. `keychain.rs` writes a 64-hex key to `<app_data>/.db_key` mode 0600. `SERVICE_NAME` + `DB_KEY_ACCOUNT` exist as dead constants — that's Phase 0 work.

Zero `TODO` / `FIXME` / `todo!()` / `unimplemented!()` in app code. Cali wrote it deliberately and stopped.

## Top 5 next moves (Phase 0)

1. **Fix `messagesContainer` reactivity warning** — `$state<HTMLDivElement | undefined>(undefined)`.
2. **Migrate `.db_key` → Keychain.** Constants are the spec; `set/get_generic_password` already imported. One-time read-write-delete.
3. **Run `npm run tauri dev` against Cali's real Signal account.** Confirm QR pair → contacts sync → receive. Nobody has done this yet (no DB at `~/Library/Application Support/com.signalui.app/`). Phase 1 hinges on it.
4. **Send-error UX.** `messaging.svelte.ts:67,86` swallows errors to `console.error`. Need a toast before Phase 2.
5. **Drop CI**: `.gitea/workflows/ci.yaml` — `npm ci && build && test` + `cargo check && test --lib`. Runner `macos-arm64` (act-runner-rs on speedwagon).

## Blockers — Cali decisions

- **Keychain UX**: real Keychain triggers the "allow access" dialog. Accept it, or ship `.db_key` + prompt-to-upgrade?
- **Distribution**: App Store sandbox will likely break presage's TLS/file IO. Developer ID + DMG is easier, no auto-update without extra work.
- **Public github eventually?** `yttfam/signalui` is private. Legal-fine to open, but invites scrutiny we may not want yet.
- **Mobile**: `src-tauri/gen/` exists, currently gitignored only at `gen/schemas/`. If iOS/Android is on the table, `gen/apple/` etc need committing.

State: pushed to `gitea` (`cali/signalui`) and `origin` (`yttfam/signalui`). Initial commit `0d55d49`. Clean tree.

🍺 — infrakid
