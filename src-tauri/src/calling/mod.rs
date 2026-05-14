//! Voice calling — RingRTC integration.
//!
//! Layout:
//! - `signaling`  RingRTC `signaling::Message` ⇄ Signal `CallMessage` proto
//! - `manager`    `CallController` — owns RingRTC's `CallManager` on a
//!                dedicated thread, bridges its callbacks to Tauri events
//!                and its outbound signaling to presage's send channel
//!
//! Voice (1:1 audio) only for v0.x. Video needs a native overlay window
//! for frame rendering; group calls need the SFU path. Both deferred.

pub mod manager;
pub mod signaling;

use serde::Serialize;

/// Who started the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CallDirection {
    Incoming,
    Outgoing,
}

/// Coarse call lifecycle, mirrored to the frontend. Deliberately simpler
/// than RingRTC's internal `CallState` — the UI only needs to know which
/// screen to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum CallState {
    /// No call. Default.
    Idle,
    /// An incoming call is ringing — show the accept/decline screen.
    Ringing {
        peer_uuid: String,
        peer_name: String,
    },
    /// We placed a call, waiting for the other side — show "calling…".
    Dialing {
        peer_uuid: String,
        peer_name: String,
    },
    /// Media is flowing — show the in-call screen.
    Connected {
        peer_uuid: String,
        peer_name: String,
    },
    /// The call finished (any reason). Frontend shows a brief "call ended"
    /// then drops back to Idle.
    Ended {
        reason: String,
    },
}

impl CallState {
    pub fn is_active(&self) -> bool {
        !matches!(self, CallState::Idle | CallState::Ended { .. })
    }
}

/// Events the call thread emits toward the Tauri layer. `lib.rs`'s event
/// pump turns these into `call-state-changed` / `incoming-call` IPC events.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CallEvent {
    /// Call state transitioned — carries the new coarse state.
    StateChanged(CallState),
    /// Remote peer muted / unmuted their mic.
    RemoteAudioState { enabled: bool },
}
