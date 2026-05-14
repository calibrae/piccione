# Voice calls — autonomous implementation run

Phase 0 (RingRTC build spike) = DONE, GREEN on macOS+Linux+Windows.

## Architecture decided

```
Signal wire  ──CallMessage proto──┐
                                  ▼
        presage (calling branch): receive loop surfaces
        InboundEvent::Call, send path for CallMessage
                                  │
                                  ▼
   piccione src-tauri/src/calling/:
     - mod.rs        CallState, CallEvent, CallDirection
     - manager.rs    CallManager — owns ringrtc CallManager on its
                     actor thread; impls SignalingSender +
                     CallStateHandler + GroupUpdateHandler
     - signaling.rs  ringrtc signaling::Message  <->  Signal CallMessage
                     proto mapping  [LIVE-TEST seam]
                                  │
              Tauri commands ◄────┤──── Tauri events
   start_call/accept/decline/     │     incoming-call / call-state
   end/toggle_mute                ▼
                          src/ Svelte UI:
                          IncomingCall.svelte, InCall.svelte,
                          calling.svelte.ts store
```

## Honesty markers

- `[LIVE-TEST]` — code that compiles + looks correct against `direct.rs`
  / Signal-Desktop's node layer, but the wire mapping can only be
  confirmed by a real call between two devices. Cali + a friend.
- everything else: compiles + unit-tested where possible.

## Order (commit after each compiles)

1. presage `calling` branch — surface incoming CallMessage + send path
2. piccione `calling` module — state, manager, trait impls
3. Tauri commands + receive-loop routing
4. Svelte UI
5. CI build deps (coreutils / libpulse / libasound / cmake / PREBUILT_WEBRTC)
6. STATUS doc — what's proven vs what needs a live call
