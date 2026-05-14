# Voice calls — implementation status

Branch: `voice-calls`. Voice only, no video (per Cali). Cross-platform
(macOS / Linux / Windows) — no macOS-only code anywhere.

## What's done & proven

**Phase 0 — feasibility (GREEN)**
RingRTC v2.69.0 builds as a plain Cargo dep on macOS + Linux + Windows via
`PREBUILT_WEBRTC=1` (downloads a prebuilt WebRTC blob from
`build-artifacts.signal.org` — no depot_tools, no Chromium source build).
Spike repo `calibrae/ringrtc-spike` — CI green Linux + Windows.

**Phase 1 — presage CallMessage (free)**
presage already delivers and sends `ContentBody::CallMessage`. Zero work.

**Phase 2 — backend (compiles, 85 lib tests pass, committed)**
- `calling/signaling.rs` — maps Signal `CallMessage` protobuf ↔ RingRTC
  `signaling::Message`. Opaque-blob shuffling, not byte surgery. 4 unit tests.
- `calling/manager.rs` — `CallController` (Send/Clone handle) + `CallThread`
  owning RingRTC `CallManager` on a dedicated thread. `Bridge` implements
  `SignalingSender` / `CallStateHandler` / `GroupUpdateHandler` / `http::Delegate`.
- `messaging/service.rs` — `SendRequest::CallMessage` variant; receive loop
  routes inbound `CallMessage` to the controller; `self_device_id` wired from
  registration data in all 3 start paths.
- `commands/calling.rs` — `start_call` / `accept_call` / `decline_call` /
  `end_call` / `get_call_state` Tauri commands.
- `lib.rs` — spawns `CallController` in `setup()`, emits `call-event` to the
  WebView, feeds it into the messaging service.

**Phase 3 — UI (builds clean)**
- `stores/calling.svelte.ts` — mirrors the Rust `CallState` / `CallEvent`
  serde shapes; one `call-event` IPC subscription; `startCall` / `accept` /
  `decline` / `end` / `refresh`.
- `components/CallOverlay.svelte` — single state-driven full-screen overlay:
  ringing → accept/decline, dialing → cancel, connected → duration timer +
  hang up, ended → brief "call ended" then auto-dismiss.
- `ChatLayout.svelte` — 📞 button in the chat header (1:1 only, disabled while
  a call is active).
- `App.svelte` — `<CallOverlay />` mounted at root; `callingStore.refresh()`
  on mount to recover state after a reload mid-call.

**Phase 4 — CI (done)**
`ci.yml` + `release.yml`: `PREBUILT_WEBRTC=1` env, plus `libpulse-dev`
`libasound2-dev` `cmake` on Linux and `coreutils` (grealpath) + `cmake` on
macOS — RingRTC's `native` feature pulls `cubeb`.

## What needs a live 2-device call to verify — the `[LIVE-TEST]` seams

These compile and are wired, but cannot be proven without two real Signal
accounts on two devices placing an actual call. Search the code for
`[LIVE-TEST]`:

1. **Signaling proto round-trip** — `calling/signaling.rs`. The opaque-blob
   mapping is structurally correct but never exercised against a real peer's
   offer/answer/ICE.
2. **Identity keys** — `manager.rs` currently passes `sender_uuid.as_bytes()`
   as a stand-in for the remote identity key into RingRTC. Needs the real
   identity key from the presage store before audio will actually negotiate.
3. **TURN/ICE servers** — `IceServer::none()`, no relay. Direct-path calls on
   the same LAN may connect; anything behind symmetric NAT will not until we
   fetch Signal's TURN credentials (`/v1/calling/relays`).
4. **`http::Delegate` stub** — RingRTC wants an HTTP client for its own
   server chatter; current impl is a stub. May need real wiring for group
   calls / TURN fetch (not for basic 1:1 on-LAN).

## Next steps (when Cali's back)

1. Live-test on LAN: two accounts, place a call, watch the `[LIVE-TEST]` #1/#2
   seams. Expect to wire the real identity key first.
2. Fetch TURN creds (`[LIVE-TEST]` #3) for calls across NAT.
3. Mute button + audio device selection (cubeb exposes both) — not yet in UI.
4. Decide: keep or delete `calibrae/ringrtc-spike`.
