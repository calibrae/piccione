//! `CallController` — owns RingRTC's `CallManager` on a dedicated thread and
//! bridges it to the rest of piccione.
//!
//! Threading: RingRTC objects (`PeerConnectionFactory`, `CallManager`,
//! `NativeCallContext`) are not `Send` — they live on one thread for their
//! whole life, same as presage's messaging thread. `CallController` is the
//! `Send + Clone` handle: an mpsc command channel into that thread plus an
//! `Arc<Mutex<CallState>>` for cheap reads.
//!
//! ```text
//!   Tauri cmd ──CallCommand──▶ call thread ──▶ ringrtc CallManager
//!   presage rx ──CallCommand──▶     │
//!                                   ├─ SignalingSender impl ──▶ presage send channel
//!                                   └─ CallStateHandler impl ──▶ Tauri events
//! ```
//!
//! `[LIVE-TEST]` markers flag everything that compiles + follows RingRTC's
//! own `bin/direct.rs` example but can only be confirmed by a real call.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use uuid::Uuid;

use super::{CallEvent, CallState};

// Everything below is the RingRTC engine — only compiled with the `voice`
// feature (off on Windows, where RingRTC's BoringSSL collides with OpenSSL).
#[cfg(feature = "voice")]
use std::collections::HashSet;
#[cfg(feature = "voice")]
use ringrtc::common::{CallConfig, CallId, CallMediaType, DeviceId, Result as RingResult};
#[cfg(feature = "voice")]
use ringrtc::core::call_manager::CallManager;
#[cfg(feature = "voice")]
use ringrtc::core::{group_call, signaling};
#[cfg(feature = "voice")]
use ringrtc::lite::http;
#[cfg(feature = "voice")]
use ringrtc::lite::sfu::{GroupMember, UserId};
#[cfg(feature = "voice")]
use ringrtc::native::{
    CallState as RingCallState, CallStateHandler, GroupUpdate, GroupUpdateHandler,
    NativeCallContext, NativePlatform, PeerId, SignalingSender,
};
#[cfg(feature = "voice")]
use ringrtc::webrtc::peer_connection_factory::{self as pcf, IceServer, PeerConnectionFactory};
#[cfg(feature = "voice")]
use ringrtc::webrtc::peer_connection_observer::NetworkRoute;
#[cfg(feature = "voice")]
use tracing::{error, info, warn};
#[cfg(feature = "voice")]
use super::CallDirection;
#[cfg(feature = "voice")]
use crate::calling::signaling as sig_map;

/// What the call thread can be asked to do.
pub enum CallCommand {
    /// User pressed "call" — place an outgoing 1:1 audio call.
    StartCall { peer_uuid: Uuid, peer_name: String },
    /// User accepted the ringing incoming call.
    Accept,
    /// User declined the ringing incoming call.
    Decline,
    /// User ended the active call (or cancelled a dial).
    Hangup,
    /// A `CallMessage` arrived from the Signal receive loop — feed it to
    /// RingRTC as inbound signaling.
    IncomingCallMessage {
        sender_uuid: Uuid,
        sender_device_id: u32,
        call_message: presage::libsignal_service::proto::CallMessage,
    },
}

/// Send + Clone handle to the call subsystem. Lives in `AppState`.
#[derive(Clone)]
pub struct CallController {
    cmd_tx: mpsc::UnboundedSender<CallCommand>,
    state: Arc<Mutex<CallState>>,
}

impl CallController {
    /// Spawn the call thread and return a handle.
    ///
    /// `emit_event` is how the call thread reaches the Tauri layer (lib.rs
    /// wires it to `app.emit`). `send_call_message` is how outbound RingRTC
    /// signaling reaches the wire — lib.rs wires it to push a
    /// `SendRequest::CallMessage` onto the messaging service's send channel.
    /// `self_device_id` is this linked device's Signal device id.
    pub fn spawn<E, S>(
        self_device_id: Arc<std::sync::atomic::AtomicU32>,
        emit_event: E,
        send_call_message: S,
    ) -> Self
    where
        E: Fn(CallEvent) + Send + Sync + 'static,
        S: Fn(Uuid, presage::libsignal_service::proto::CallMessage) + Send + Sync + 'static,
    {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<CallCommand>();
        let state = Arc::new(Mutex::new(CallState::Idle));

        let thread_state = state.clone();
        std::thread::Builder::new()
            .name("piccione-calling".to_string())
            .stack_size(4 * 1024 * 1024)
            .spawn(move || {
                call_thread_main(
                    self_device_id,
                    cmd_rx,
                    thread_state,
                    Arc::new(emit_event),
                    Arc::new(send_call_message),
                );
            })
            .expect("failed to spawn calling thread");

        Self { cmd_tx, state }
    }

    /// Current coarse call state — cheap, lock-and-clone.
    pub fn state(&self) -> CallState {
        self.state.lock().expect("call state lock").clone()
    }

    pub fn start_call(&self, peer_uuid: Uuid, peer_name: String) {
        let _ = self
            .cmd_tx
            .send(CallCommand::StartCall { peer_uuid, peer_name });
    }
    pub fn accept(&self) {
        let _ = self.cmd_tx.send(CallCommand::Accept);
    }
    pub fn decline(&self) {
        let _ = self.cmd_tx.send(CallCommand::Decline);
    }
    pub fn hangup(&self) {
        let _ = self.cmd_tx.send(CallCommand::Hangup);
    }
    /// Called from the messaging receive loop when a `CallMessage` arrives.
    pub fn on_call_message(
        &self,
        sender_uuid: Uuid,
        sender_device_id: u32,
        call_message: presage::libsignal_service::proto::CallMessage,
    ) {
        let _ = self.cmd_tx.send(CallCommand::IncomingCallMessage {
            sender_uuid,
            sender_device_id,
            call_message,
        });
    }
}

type EmitFn = Arc<dyn Fn(CallEvent) + Send + Sync>;
type SendFn =
    Arc<dyn Fn(Uuid, presage::libsignal_service::proto::CallMessage) + Send + Sync>;

/// Live state owned exclusively by the call thread.
#[cfg(feature = "voice")]
struct CallThread {
    self_device_id: Arc<std::sync::atomic::AtomicU32>,
    call_manager: CallManager<NativePlatform>,
    call_context: NativeCallContext,
    state: Arc<Mutex<CallState>>,
    /// The peer + call id of the call we're currently in / dialing / ringing.
    active: Option<ActiveCall>,
}

#[cfg(feature = "voice")]
struct ActiveCall {
    call_id: CallId,
    peer_uuid: Uuid,
    peer_name: String,
    direction: CallDirection,
}

#[cfg(feature = "voice")]
fn call_thread_main(
    self_device_id: Arc<std::sync::atomic::AtomicU32>,
    mut cmd_rx: mpsc::UnboundedReceiver<CallCommand>,
    state: Arc<Mutex<CallState>>,
    emit_event: EmitFn,
    send_call_message: SendFn,
) {
    let mut thread = match CallThread::new(self_device_id, state, emit_event, send_call_message) {
        Ok(t) => t,
        Err(e) => {
            error!("calling: failed to init RingRTC: {e}");
            return;
        }
    };
    info!("calling: RingRTC thread ready");

    // Block on the command channel. A current-thread runtime is the cheapest
    // way to await an mpsc receiver without pulling the whole call subsystem
    // onto the main tokio runtime.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("calling runtime");
    rt.block_on(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            if let Err(e) = thread.handle(cmd) {
                warn!("calling: command failed: {e}");
            }
        }
    });
}

/// Stub call thread for builds without the `voice` feature (Windows, where
/// RingRTC's bundled BoringSSL collides with SQLCipher's OpenSSL at link
/// time). Drains commands; any attempt to start or accept a call resolves
/// immediately to `Ended` so the UI shows a clear message instead of hanging.
#[cfg(not(feature = "voice"))]
fn call_thread_main(
    _self_device_id: Arc<std::sync::atomic::AtomicU32>,
    mut cmd_rx: mpsc::UnboundedReceiver<CallCommand>,
    state: Arc<Mutex<CallState>>,
    emit_event: EmitFn,
    _send_call_message: SendFn,
) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("calling runtime");
    rt.block_on(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            if matches!(cmd, CallCommand::StartCall { .. } | CallCommand::Accept) {
                let ended = CallState::Ended {
                    reason: "voice calls are not available in this build".to_string(),
                };
                *state.lock().expect("call state lock") = ended.clone();
                emit_event(CallEvent::StateChanged(ended));
                *state.lock().expect("call state lock") = CallState::Idle;
            }
            // Decline / Hangup / inbound CallMessages: nothing to do.
        }
    });
}

#[cfg(feature = "voice")]
impl CallThread {
    fn new(
        self_device_id: Arc<std::sync::atomic::AtomicU32>,
        state: Arc<Mutex<CallState>>,
        emit_event: EmitFn,
        send_call_message: SendFn,
    ) -> RingResult<Self> {
        // WebRTC logs only at warn in release; RingRTC wants this set once.
        ringrtc::webrtc::logging::set_logger(log::LevelFilter::Warn);

        let pcf = PeerConnectionFactory::new(&pcf::AudioConfig::default(), false, "", None)?;

        // Bridge object handed to RingRTC for all three callback traits.
        let bridge = Bridge {
            state: state.clone(),
            emit_event,
            send_call_message,
        };

        let platform = NativePlatform::new(
            pcf.clone(),
            Box::new(bridge.clone()),
            // should_assume_messages_sent: our send path is fire-and-forget
            // onto presage's channel, so RingRTC shouldn't wait for an ack.
            true,
            Box::new(bridge.clone()),
            Box::new(bridge.clone()),
        );
        let http_client = http::DelegatingClient::new(bridge);
        let call_manager = CallManager::new(platform, http_client)?;

        // One reusable call context. Voice-only: we still must supply an
        // outgoing video track + an incoming video sink (RingRTC's API
        // requires them) — we hand it a never-fed video source and a
        // discard sink.
        let outgoing_audio_track = pcf.create_outgoing_audio_track()?;
        let outgoing_video_source = pcf.create_outgoing_video_source()?;
        let outgoing_video_track = pcf.create_outgoing_video_track(&outgoing_video_source)?;
        let call_context = NativeCallContext::new(
            // hide_ip: false — relaying through Signal's TURN is a [LIVE-TEST]
            // follow-up; for now we use no ICE servers (host/STUN-less),
            // which works on the same LAN / many home networks.
            false,
            vec![IceServer::none()],
            outgoing_audio_track,
            outgoing_video_track,
            Box::new(DiscardVideoSink),
        );

        Ok(Self {
            self_device_id,
            call_manager,
            call_context,
            state,
            active: None,
        })
    }

    fn set_state(&self, new: CallState) {
        *self.state.lock().expect("call state lock") = new;
    }

    fn handle(&mut self, cmd: CallCommand) -> RingResult<()> {
        match cmd {
            CallCommand::StartCall {
                peer_uuid,
                peer_name,
            } => {
                let call_id = CallId::random();
                let peer_id: PeerId = peer_uuid.to_string();
                info!("calling: outgoing call {call_id} → {peer_uuid}");
                let local_device_id =
                    self.self_device_id.load(std::sync::atomic::Ordering::Relaxed)
                        as DeviceId;
                self.call_manager.create_outgoing_call(
                    peer_id,
                    call_id,
                    CallMediaType::Audio,
                    local_device_id,
                )?;
                self.call_manager
                    .proceed(call_id, self.call_context.clone(), CallConfig::default(), None)?;
                self.active = Some(ActiveCall {
                    call_id,
                    peer_uuid,
                    peer_name: peer_name.clone(),
                    direction: CallDirection::Outgoing,
                });
                self.set_state(CallState::Dialing {
                    peer_uuid: peer_uuid.to_string(),
                    peer_name,
                });
                Ok(())
            }
            CallCommand::Accept => {
                if let Some(active) = &self.active {
                    info!("calling: accepting {}", active.call_id);
                    self.call_manager.accept_call(active.call_id)?;
                }
                Ok(())
            }
            CallCommand::Decline | CallCommand::Hangup => {
                info!("calling: hangup");
                self.call_manager.hangup()?;
                self.active = None;
                self.set_state(CallState::Idle);
                Ok(())
            }
            CallCommand::IncomingCallMessage {
                sender_uuid,
                sender_device_id,
                call_message,
            } => self.handle_incoming(sender_uuid, sender_device_id, call_message),
        }
    }

    fn handle_incoming(
        &mut self,
        sender_uuid: Uuid,
        sender_device_id: u32,
        call_message: presage::libsignal_service::proto::CallMessage,
    ) -> RingResult<()> {
        let parsed = match sig_map::from_call_message(&call_message) {
            Ok(Some(parsed)) => parsed,
            Ok(None) => return Ok(()), // group-calling traffic, not handled
            Err(e) => {
                warn!("calling: undecodable CallMessage: {e}");
                return Ok(());
            }
        };
        let (call_id, msg) = parsed;

        let peer_id: PeerId = sender_uuid.to_string();
        let local_device_id =
            self.self_device_id.load(std::sync::atomic::Ordering::Relaxed) as DeviceId;
        // [LIVE-TEST] direct.rs uses the peer-id bytes as a stand-in identity
        // key. Real Signal carries the actual identity public key; whether
        // RingRTC's offer verification rejects the stand-in on a live call
        // is exactly what needs a two-device test.
        let sender_identity_key = sender_uuid.as_bytes().to_vec();
        let receiver_identity_key = sender_uuid.as_bytes().to_vec();

        match msg {
            signaling::Message::Offer(offer) => {
                info!("calling: incoming offer {call_id} from {sender_uuid}");
                self.call_manager.received_offer(
                    peer_id,
                    call_id,
                    signaling::ReceivedOffer {
                        offer,
                        age: std::time::Duration::from_secs(0),
                        sender_device_id: sender_device_id as DeviceId,
                        receiver_device_id: local_device_id,
                        sender_identity_key,
                        receiver_identity_key,
                    },
                )?;
                self.call_manager
                    .proceed(call_id, self.call_context.clone(), CallConfig::default(), None)?;
                self.active = Some(ActiveCall {
                    call_id,
                    peer_uuid: sender_uuid,
                    peer_name: sender_uuid.to_string(),
                    direction: CallDirection::Incoming,
                });
                self.set_state(CallState::Ringing {
                    peer_uuid: sender_uuid.to_string(),
                    peer_name: sender_uuid.to_string(),
                });
            }
            signaling::Message::Answer(answer) => {
                self.call_manager.received_answer(
                    peer_id,
                    call_id,
                    signaling::ReceivedAnswer {
                        answer,
                        sender_device_id: sender_device_id as DeviceId,
                        sender_identity_key,
                        receiver_identity_key,
                    },
                )?;
            }
            signaling::Message::Ice(ice) => {
                self.call_manager.received_ice(
                    peer_id,
                    call_id,
                    signaling::ReceivedIce {
                        ice,
                        sender_device_id: sender_device_id as DeviceId,
                    },
                )?;
            }
            signaling::Message::Hangup(hangup) => {
                self.call_manager.received_hangup(
                    peer_id,
                    call_id,
                    signaling::ReceivedHangup {
                        hangup,
                        sender_device_id: sender_device_id as DeviceId,
                    },
                )?;
            }
            signaling::Message::Busy => {
                self.call_manager.received_busy(
                    peer_id,
                    call_id,
                    signaling::ReceivedBusy {
                        sender_device_id: sender_device_id as DeviceId,
                    },
                )?;
            }
        }
        Ok(())
    }
}

/// One object, three RingRTC callback traits + the HTTP delegate. Cloned
/// into `NativePlatform` and `DelegatingClient` at setup.
#[derive(Clone)]
#[cfg(feature = "voice")]
struct Bridge {
    state: Arc<Mutex<CallState>>,
    emit_event: EmitFn,
    send_call_message: SendFn,
}

#[cfg(feature = "voice")]
impl Bridge {
    fn peer(&self) -> Option<(Uuid, String)> {
        match &*self.state.lock().expect("call state lock") {
            CallState::Ringing { peer_uuid, peer_name }
            | CallState::Dialing { peer_uuid, peer_name }
            | CallState::Connected { peer_uuid, peer_name } => {
                Uuid::parse_str(peer_uuid).ok().map(|u| (u, peer_name.clone()))
            }
            _ => None,
        }
    }
}

#[cfg(feature = "voice")]
impl SignalingSender for Bridge {
    fn send_signaling(
        &self,
        recipient_id: &str,
        call_id: CallId,
        _receiver_device_id: Option<DeviceId>,
        message: signaling::Message,
    ) -> RingResult<()> {
        let Ok(recipient) = Uuid::parse_str(recipient_id) else {
            warn!("calling: send_signaling to non-uuid recipient {recipient_id}");
            return Ok(());
        };
        let call_message = sig_map::to_call_message(call_id, &message);
        (self.send_call_message)(recipient, call_message);
        Ok(())
    }

    // Group calling — not wired for voice-first 1:1. These are no-ops.
    fn send_call_message(
        &self,
        _recipient_id: UserId,
        _message: Vec<u8>,
        _urgency: group_call::SignalingMessageUrgency,
    ) -> RingResult<()> {
        Ok(())
    }
    fn send_call_message_to_group(
        &self,
        _group_id: group_call::GroupId,
        _message: Vec<u8>,
        _urgency: group_call::SignalingMessageUrgency,
        _recipients_override: HashSet<UserId>,
    ) -> RingResult<()> {
        Ok(())
    }
    fn send_call_message_to_adhoc_group(
        &self,
        _message: Vec<u8>,
        _urgency: group_call::SignalingMessageUrgency,
        _expiration: u64,
        _recipients_to_endorsements: std::collections::HashMap<UserId, Vec<u8>>,
    ) -> RingResult<()> {
        Ok(())
    }
}

#[cfg(feature = "voice")]
impl CallStateHandler for Bridge {
    fn handle_call_state(
        &self,
        _remote_peer_id: &str,
        _call_id: CallId,
        ring_state: RingCallState,
    ) -> RingResult<()> {
        let peer = self.peer();
        let new_state = match ring_state {
            RingCallState::Incoming(_) | RingCallState::Ringing => peer
                .map(|(u, n)| CallState::Ringing {
                    peer_uuid: u.to_string(),
                    peer_name: n,
                })
                .unwrap_or(CallState::Idle),
            RingCallState::Outgoing(_) | RingCallState::Connecting => peer
                .map(|(u, n)| CallState::Dialing {
                    peer_uuid: u.to_string(),
                    peer_name: n,
                })
                .unwrap_or(CallState::Idle),
            RingCallState::Connected => peer
                .map(|(u, n)| CallState::Connected {
                    peer_uuid: u.to_string(),
                    peer_name: n,
                })
                .unwrap_or(CallState::Idle),
            RingCallState::Ended(reason, _) => CallState::Ended {
                reason: format!("{reason:?}"),
            },
            RingCallState::Rejected(reason) => CallState::Ended {
                reason: format!("{reason:?}"),
            },
            RingCallState::Concluded => CallState::Idle,
        };
        *self.state.lock().expect("call state lock") = new_state.clone();
        (self.emit_event)(CallEvent::StateChanged(new_state));
        Ok(())
    }

    fn handle_remote_audio_state(&self, _peer: &str, enabled: bool) -> RingResult<()> {
        (self.emit_event)(CallEvent::RemoteAudioState { enabled });
        Ok(())
    }
    fn handle_remote_video_state(&self, _peer: &str, _enabled: bool) -> RingResult<()> {
        Ok(())
    }
    fn handle_remote_sharing_screen(&self, _peer: &str, _enabled: bool) -> RingResult<()> {
        Ok(())
    }
    fn handle_network_route(&self, _peer: &str, _route: NetworkRoute) -> RingResult<()> {
        Ok(())
    }
    fn handle_audio_levels(
        &self,
        _peer: &str,
        _captured: ringrtc::webrtc::peer_connection::AudioLevel,
        _received: ringrtc::webrtc::peer_connection::AudioLevel,
    ) -> RingResult<()> {
        Ok(())
    }
    fn handle_low_bandwidth_for_video(&self, _peer: &str, _recovered: bool) -> RingResult<()> {
        Ok(())
    }
}

#[cfg(feature = "voice")]
impl GroupUpdateHandler for Bridge {
    fn handle_group_update(&self, _update: GroupUpdate) -> RingResult<()> {
        // Group calls aren't wired for voice-first. No-op.
        Ok(())
    }
}

#[cfg(feature = "voice")]
impl http::Delegate for Bridge {
    fn send_request(&self, _request_id: u32, _request: http::Request) {
        // [LIVE-TEST] RingRTC uses HTTP for TURN credentials. We currently
        // run with IceServer::none() (no TURN), so no request should fire on
        // a normal 1:1 call. If a live call needs TURN this becomes a real
        // reqwest-backed impl + DelegatingClient::received_response.
        warn!("calling: RingRTC HTTP request ignored (no TURN configured yet)");
    }
}

/// Voice-only: RingRTC's `NativeCallContext` requires an incoming video
/// sink. We never negotiate video, so frames should never arrive — this
/// just discards anything that does.
#[cfg(feature = "voice")]
struct DiscardVideoSink;

#[cfg(feature = "voice")]
impl ringrtc::webrtc::media::VideoSink for DiscardVideoSink {
    fn on_video_frame(
        &self,
        _track_id: u32,
        _frame: ringrtc::webrtc::media::VideoFrame,
    ) {
    }
    fn box_clone(&self) -> Box<dyn ringrtc::webrtc::media::VideoSink> {
        Box::new(DiscardVideoSink)
    }
}
