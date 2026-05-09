# swarm-modifiers — phase report

Branch: `feat/modifiers` (worktree `/private/tmp/signalui-wt-modifiers`).

## What landed
Wired the 5 silently-dropped modifier message variants from presage's receive
loop into Tauri events.

### Files changed
- `src-tauri/src/messaging/types.rs` — added `ReceiptKind`, `TypingAction`,
  `ReceiptEvent`, `TypingEvent`, `ReactionEvent`, `EditEvent`, `DeleteEvent`,
  and the unified `InboundEvent` enum (Serialize-ready, frontend-shaped).
- `src-tauri/src/messaging/service.rs` — replaced the `_ => None` arm with a
  pure `derive_inbound_events(&Content, &Option<String>)` extractor; receive
  loops now iterate over the returned events and only run attachment download
  on `Message` variants. Callback signature changed from
  `Fn(String, ChatMessage)` to `Fn(InboundEvent)`. `content_to_chat_message`
  now suppresses reaction-only / delete-only `DataMessage`s so they don't
  render as empty bubbles.
- `src-tauri/src/lib.rs` — `make_on_event` dispatcher emits one Tauri event
  per `InboundEvent` variant (`new-message`, `read-receipt`,
  `typing-indicator`, `reaction`, `message-edited`, `message-deleted`).
  Edits/deletes also kick `conversations-updated` so the sidebar summary
  refreshes. `make_on_message` kept as a backward-compat alias for
  `commands/provisioning.rs`.
- `src/lib/stores/messaging.svelte.ts` — listens for the 5 new events and
  mirrors them into per-chat reactive maps (`receipts`, `typing`,
  `reactions`, `edits`, `deletions`). UI rendering is the next swarm.

## Tests
+15 new lib tests (25 → 40) — all `cargo test --lib` green. Mocks build
real `Content` envelopes via `Metadata::new` + raw `ContentBody` variants;
no Tauri runtime needed. Frontend build clean, vitest 7/7 still pass.

## Mocks introduced
`metadata(sender, ts)` + `content_with_body(sender, ts, body)` test helpers
synthesise presage `Content` directly from in-memory `DataMessage` /
`ReceiptMessage` / `TypingMessage` / `EditMessage` / `NullMessage` protos.

## Smells flagged (don't fix here)
- `ChatLayout.svelte:9` `messagesContainer` not `$state` — pre-existing
  Phase 0 todo, surfaces on every build. Belongs to UI swimlane.
- `presage` persists Edit + Delete already (`save_message` in
  `presage/manager/registered.rs`). Reactions are stored as part of the
  parent `DataMessage`. We do NOT duplicate persistence — we only emit
  events. Worth confirming with whichever swimlane owns history rendering.
- Sync-of-our-own modifier ack from another linked device is best-effort:
  reactions/deletes synced via `SyncMessage.sent.message` are routed to the
  same path; sync-edit through `SyncMessage.sent.edit_message` is wired but
  untested against a live tree.

## Merge order
Safe to merge first — only modifier-shaped enrichment, no overlap with
provisioning, attachments, history, or store work. If swarm-attachments or
swarm-history touch `service.rs` they should rebase on top of this.
