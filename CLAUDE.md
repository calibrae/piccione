# SignalUI

Native macOS Signal client. Tauri 2 + Svelte 5 + Rust + presage. Not Electron.

## Goal

Apple already runs Signal-Desktop on Electron — that's a Chromium VM to display a chat list. The qbytti-equivalent move is to ship a single small native app that:

- starts in <1s, idle RAM <100 MB
- pairs as a secondary device against an existing Signal account (no phone-number registration in v0.1)
- lists conversations, sends and receives **text** messages
- stores everything in an encrypted local SQLite, key in the macOS Keychain (eventually — see Phase 0)

**v0.1 scope**: pair via QR, conversation list, read/send text, attachments wired but secondary. **NOT in v0.1**: groups beyond display, voice/video calls, sticker packs, story support, contact management UI, multi-account.

## Stack — locked

| Layer | Choice | Why |
|---|---|---|
| Shell | Tauri 2 (`com.signalui.app`) | Native WebKit, no Chromium, ~10 MB binaries |
| UI | Svelte 5 + TypeScript + Vite 6 | Runes (`$state` / `$derived`), small bundle |
| Signal protocol | `presage` (whisperfish/presage, `main`) | Only pure-Rust Signal client lib worth using |
| Storage | `presage-store-sqlite` (SQLCipher via `libsqlite3-sys` fork) | Encrypted at rest |
| Keychain | `security-framework` 3 | macOS-native secret storage |
| Crypto pin | `curve25519-dalek` ← signalapp's `signal-curve25519-4.1.3` fork | Required by libsignal |
| QR | `qrcode` 0.14 (svg feature) | Render the linking QR straight to inline SVG |
| E2E | Playwright | UI flow only, no real Signal account |
| Unit | vitest (frontend) + cargo test (backend) | |

Identifier `com.signalui.app`. **When shipping**: register the bundle ID in App Store Connect and notarise with the existing `Developer ID Application: Nico Bousquet (XJQQCN392F)`. Don't act on this yet.

## Architecture

```
src/                     Svelte 5 frontend
  App.svelte             root: loading → LinkDevice or ChatLayout
  lib/
    components/          QrCode, LinkDevice, ChatLayout (492 LoC, the meat)
    stores/              provisioning.svelte.ts, messaging.svelte.ts (runes)
    types.ts             ProvisioningState, Conversation, ChatMessage
src-tauri/
  src/
    lib.rs               tauri::Builder, registers commands, spawns startup thread
    app_state.rs         AppState { ProvisioningManager, MessagingService, db_passphrase }
    commands/            #[tauri::command] wrappers (provisioning + messaging)
    provisioning/        manager.rs (link_secondary_device + state machine), qr.rs, state.rs
    messaging/           service.rs (768 LoC — receive loop, send queue, contacts/groups/messages reads)
    store/keychain.rs    file-based key for now (Keychain integration TBD)
  tests/                 integration_test (mock Tauri), store_diagnostic (real DB dump)
```

### Critical invariants

- **presage is `!Send`**: messaging + provisioning run on a dedicated thread with `LocalSet`, 8 MB stack. Don't move them onto the default tokio runtime.
- **Single-thread send/recv**: the messaging thread owns the `Manager`. Sends arrive via `mpsc::UnboundedSender<SendRequest>`. Reads use a separate `SqliteStore` clone opened on demand (`fresh_read_store`) so the UI never blocks the recv loop.
- **DB passphrase is captured ONCE at startup** in `AppState::new`. If keychain fails, falls back to empty string and re-tries — don't break this without thinking.
- **`OnNewIdentity::Trust`** everywhere — TOFU. Acceptable for v0.1, revisit before public release.
- **Tests must not touch the real `~/Library/Application Support/com.signalui.app/`** db. `store_diagnostic.rs` does, intentionally — it's a debug tool, not CI.

## Where to crib

- whisperfish/presage repo, `presage-cli/` and `examples/` for canonical `link_secondary_device` + receive-loop wiring.
- whisperfish itself for groups/attachments/UX patterns (but they're on Qt, don't copy structure).
- Tauri 2 docs `https://tauri.app/start/` for Svelte 5 integration; `tauri.conf.json` CSP currently locks `connect-src ipc:` only — relax if we need direct Signal CDN downloads from the WebView (probably not, attachments stream through Rust).

## Phase plan

### Phase 0 — green build + skeleton (where we are)
- [x] `npm run build` clean
- [x] `cargo check` clean (17 dead-code warnings — fine)
- [x] `vitest` 7/7 pass, `cargo test --lib` 25/25 pass
- [ ] Move db passphrase from `.db_key` file → real Keychain entry (`SERVICE_NAME` const is already there, dead)
- [ ] Fix the `messagesContainer` non-`$state` warning in `ChatLayout.svelte`
- [ ] Decide on logging (currently `tracing` to stdout — fine for dev)

### Phase 1 — pair flow against a real account
- Plug a test Signal account on the iPhone, run `npm run tauri dev`, confirm: Connecting → WaitingForScan (QR renders) → Provisioning → Registered.
- Confirm contacts sync (we already call `request_contacts()`).
- Confirm conversation list populates after first run, restart, reload.

### Phase 2 — message send/receive
- Verify text receive (Note to Self is the simplest test), outgoing optimistic update, sync messages from other linked devices.
- Verify text send, both 1:1 and group. Send-error UX (currently swallowed in console).
- Attachments: send works in code (`upload_attachments`), receive too (`get_attachment` + local file). Test images, then PDFs. Render in `ChatLayout.svelte`.

### Phase 3 — ship
- Tauri bundle for macOS, codesign with Developer ID, notarise. App Store submission optional and harder (sandbox would break presage's TLS at minimum — leave it for later).
- Auto-update via Tauri's updater plugin (not yet wired).

## Tauri commands (current surface)

| Command | Returns | Notes |
|---|---|---|
| `start_provisioning(deviceName)` | `()` | Spawns dedicated thread, runs link flow + then keeps the LocalSet alive forever |
| `cancel_provisioning` | `()` | Cancels the cancel token |
| `get_link_status` | `bool` | Backed by `messaging.self_id().is_some()` |
| `get_provisioning_state` | `ProvisioningState` | For polling/recovery |
| `get_conversations` | `Vec<Conversation>` | Opens a fresh store, returns contacts ∪ groups, sorted by last_timestamp |
| `get_messages(conversationId)` | `Vec<ChatMessage>` | Filters DataMessage + sync sent |
| `send_message(conversationId, body)` | `()` | Through the mpsc send channel |
| `send_to_recipient(recipientId, body)` | `()` | Same as send_message; for "new chat" UX |
| `send_message_with_attachments(conversationId, body, filePaths)` | `()` | Reads files synchronously, uploads via presage |
| `get_self_id` | `Option<String>` | ACI as service-id string |

Events emitted to the WebView: `provisioning-state-changed`, `new-message`, `conversations-updated`.

## Style

- Match the rest of the colony: ship first, iterate, no AI co-author lines, no fluff in commits.
- Keep `service.rs` from growing past ~1k LoC — split out attachment download + thread parsing when it's time.
- Vault: `secret/infra/gitea` for the gitea token. No Signal credentials are stored anywhere on disk except the encrypted SQLite.

## Repos

- `gitea`: `git@git.calii.net:cali/signalui.git` (private, primary)
- `origin`: `https://github.com/yttfam/signalui.git` (private — Signal protocol code, do **not** flip to public lightly)
