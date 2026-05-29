import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// Mirrors src-tauri/src/calling/mod.rs CallState (serde tag = "state",
// kebab-case). The Rust enum is the source of truth.
export type CallState =
  | { state: "idle" }
  | { state: "ringing"; peer_uuid: string; peer_name: string }
  | { state: "dialing"; peer_uuid: string; peer_name: string }
  | { state: "connected"; peer_uuid: string; peer_name: string }
  | { state: "ended"; reason: string };

// Mirrors CallEvent (serde tag = "kind"). StateChanged flattens the inner
// CallState map, so a state-changed event also carries the `state` field.
type CallEvent =
  | ({ kind: "state-changed" } & CallState)
  | { kind: "remote-audio-state"; enabled: boolean };

function createCallingStore() {
  let call = $state<CallState>({ state: "idle" });
  let remoteMuted = $state(false);
  // Wall-clock seconds since the call reached "connected", for the timer.
  let connectedAt = $state<number | null>(null);

  // Single IPC subscription for the app's lifetime.
  listen<CallEvent>("call-event", (event) => {
    const e = event.payload;
    if (e.kind === "remote-audio-state") {
      remoteMuted = !e.enabled;
      return;
    }
    // state-changed — the rest of `e` IS a CallState.
    const { kind: _kind, ...next } = e;
    call = next as CallState;
    if (call.state === "connected" && connectedAt === null) {
      connectedAt = Date.now();
    }
    if (call.state === "idle" || call.state === "ended") {
      connectedAt = null;
      remoteMuted = false;
      // "ended" is transient — drop back to idle after a short beat so the
      // overlay shows "call ended" then disappears.
      if (call.state === "ended") {
        setTimeout(() => {
          if (call.state === "ended") call = { state: "idle" };
        }, 2500);
      }
    }
  });

  return {
    get call() {
      return call;
    },
    get remoteMuted() {
      return remoteMuted;
    },
    get connectedAt() {
      return connectedAt;
    },
    /** True whenever an overlay should be on screen. */
    get active() {
      return call.state !== "idle";
    },

    async startCall(recipientId: string, recipientName: string) {
      // Optimistic — the backend will emit the real state-changed shortly.
      call = { state: "dialing", peer_uuid: recipientId, peer_name: recipientName };
      try {
        await invoke("start_call", { recipientId, recipientName });
      } catch (e) {
        console.error("start_call failed:", e);
        call = { state: "idle" };
      }
    },
    async accept() {
      try {
        await invoke("accept_call");
      } catch (e) {
        console.error("accept_call failed:", e);
      }
    },
    async decline() {
      try {
        await invoke("decline_call");
      } catch (e) {
        console.error("decline_call failed:", e);
      }
      call = { state: "idle" };
    },
    async end() {
      try {
        await invoke("end_call");
      } catch (e) {
        console.error("end_call failed:", e);
      }
      call = { state: "idle" };
    },
    /** Poll the backend on load to recover from a reload mid-call. */
    async refresh() {
      try {
        call = await invoke<CallState>("get_call_state");
      } catch {
        call = { state: "idle" };
      }
    },
  };
}

export const callingStore = createCallingStore();
