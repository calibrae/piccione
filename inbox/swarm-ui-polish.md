# swarm-ui-polish — phase report

Branch: `feat/ui-polish` (from `main` @ 89bb3c4)

## Files changed
- `src/lib/components/ChatLayout.svelte` — `messagesContainer` now `$state<HTMLDivElement | undefined>(undefined)`; `$effect` reads it as a tracked dep, scroll-to-bottom now fires on every list growth. Attachments redesigned: images render at `max-width:280px / max-height:280px`, click opens a full-screen lightbox (Escape/click-outside to close); non-image attachments render as a clean `EXT • filename • size • Ouvrir` row. Added `data-testid` hooks. Wrapped fire-and-forget `sendMessageWithAttachments` in `.catch(()=>{})` so the toast handles UX (no unhandled rejection).
- `src/lib/stores/messaging.svelte.ts` — replaced both `console.error` swallows on send paths with `toastStore.error("Échec de l'envoi", retry)` whose retry callback re-invokes the same send (closure over args).
- `src/App.svelte` — mount `<ToastContainer />` at root.
- `src/lib/stores/toasts.svelte.ts` — new. Hand-rolled rune store: `push/dismiss/clear/error/info/success`, ttl auto-dismiss (6s errors / 4s default, 0 disables), id-based timer cleanup.
- `src/lib/components/ToastContainer.svelte` — new. Renders bottom-right stack, "Réessayer" button when toast carries a retry, dismiss `×` button. Errors get `role=alert`. CSS-only animation, no deps.

## Tests
- 7 baseline still pass.
- `src/__tests__/toasts.test.ts` — 7 tests: push/dismiss/clear, ttl auto-dismiss, ttl=0 sticky, retry plumbed.
- `src/__tests__/ChatLayout.test.ts` — 3 tests: image-mime renders `<img>`, non-image renders file row with EXT label, scroll-to-bottom triggers on `new-message` event growth (verified via stubbed scrollTop setter).
- Total **17/17 passing**, `npm run build` clean.

## Mocks introduced
- `@tauri-apps/api/core` invoke + `convertFileSrc` (returns `tauri://localhost/<encoded>`).
- `@tauri-apps/api/event` listen captures callbacks in a Map so tests can dispatch `new-message` synthetically.
- `@tauri-apps/plugin-dialog` open stubbed to no-op.
- `vi.useFakeTimers` for ttl tests.
- jsdom `HTMLElement.prototype.scrollTop`/`scrollHeight` overrides + immediate `requestAnimationFrame` shim for the scroll test.

## Smells flagged (not fixed — out of scope)
- `messaging.svelte.ts` listens at module load with no unsubscribe; on hot reload listeners pile up. Should be moved into an init function called from App's `onMount`.
- `loadConversations` is polled every 5 s in `ChatLayout.onMount`; the backend already emits `conversations-updated`. The poll is redundant and burns ~12 IPC/min.
- `send_to_recipient` in the New Message form still uses local `sendError` text instead of the toast store — left intentionally to avoid scope creep, but the swimlane that owns "new chat UX" should unify it.
- `pointer_data` field on `AttachmentInfo` is unused on the frontend; rust-backend swimlane should confirm contract.
- `convertFileSrc` is called twice per image (button onclick + img src). Cheap, but a `$derived` would be cleaner.

## Expected merge order with siblings
This branch only edits **`src/App.svelte`, `src/lib/components/ChatLayout.svelte`, `src/lib/stores/messaging.svelte.ts`**, plus adds new files under `src/lib/{components,stores}/` and `src/__tests__/`. Conflict surface:

1. **Merge first** any swim that touches `messaging.svelte.ts` core wiring (e.g. attachment download / receive-loop refactor) — small, mechanical conflicts in the catch blocks.
2. **Merge after** `ui-polish`: any swim that adds new toast call-sites (e.g. provisioning errors), since `toastStore` will already exist.
3. Swims that only touch `src-tauri/` are independent — merge in any order.

No backend (`src-tauri/`) changes here.
