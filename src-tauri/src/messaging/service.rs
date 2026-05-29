use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use presage::libsignal_service::content::ContentBody;
use presage::libsignal_service::prelude::Content;
use presage::libsignal_service::proto::{
    data_message::{Delete, Reaction},
    sync_message, DataMessage, EditMessage, ReceiptMessage, SyncMessage, TypingMessage,
};
use presage::libsignal_service::protocol::ServiceId;
use presage::manager::Registered;
use presage::model::identity::OnNewIdentity;
use presage::store::{ContentsStore, StateStore, Store, Thread};
use presage::Manager;
use presage_store_sqlite::SqliteStore;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::messaging::parse::{
    content_to_chat_message, derive_inbound_events, parse_thread, pick_sender_name,
};
use crate::messaging::types::{
    AttachmentInfo, ChatMessage, Conversation, DeleteEvent, EditEvent, InboundEvent, ReactionEvent,
    ReceiptEvent, ReceiptKind, TypingAction, TypingEvent,
};

/// Things the dedicated messaging thread can be asked to send.
enum SendRequest {
    /// Send a regular DataMessage (text + optional attachments).
    Text {
        conversation_id: String,
        body: String,
        file_paths: Vec<String>,
        quote: Option<crate::messaging::types::QuoteInput>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Send a ReceiptMessage (DELIVERY / READ / VIEWED) back to the sender of
    /// one or more incoming messages, identified by their original timestamps.
    Receipt {
        recipient_uuid: Uuid,
        kind: ReceiptKind,
        timestamps: Vec<u64>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Send a CallMessage (voice-call signaling) to a recipient. Enqueued by
    /// the calling subsystem's `SignalingSender` bridge. Fire-and-forget —
    /// RingRTC retransmits ICE on its own schedule, so a dropped send isn't
    /// fatal and we don't make the call thread wait on a reply.
    CallMessage {
        recipient_uuid: Uuid,
        call_message: presage::libsignal_service::proto::CallMessage,
    },
    /// Send (or remove) an emoji reaction to a message in a conversation.
    Reaction {
        conversation_id: String,
        target_author_uuid: String,
        target_timestamp: u64,
        emoji: String,
        remove: bool,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Delete-for-everyone: retract a previously-sent message.
    Delete {
        conversation_id: String,
        target_timestamp: u64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// List the account's linked devices (read-only).
    ListDevices {
        reply: oneshot::Sender<Result<Vec<crate::messaging::types::DeviceDto>, String>>,
    },
    /// Fetch + cache a contact's profile; returns the resolved display name.
    FetchProfile {
        uuid: Uuid,
        reply: oneshot::Sender<Result<Option<String>, String>>,
    },
    /// Pin or unpin a message in a conversation.
    Pin {
        conversation_id: String,
        target_author_uuid: String,
        target_timestamp: u64,
        pinned: bool,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Cast a vote on a poll message.
    PollVote {
        conversation_id: String,
        target_author_uuid: String,
        target_timestamp: u64,
        option_indexes: Vec<u32>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Whether message-backup / Link & Sync is possible (AEP persisted +
    /// BackupKey derivable).
    BackupStatus {
        reply: oneshot::Sender<bool>,
    },
    /// Decrypt + summarize an encrypted transfer archive at `path` (preview
    /// before import). Behind the `backups` feature.
    #[cfg(feature = "backups")]
    PreviewBackup {
        path: String,
        reply: oneshot::Sender<Result<crate::backups::BackupSummary, String>>,
    },
    /// Import contacts from an encrypted transfer archive into the store.
    #[cfg(feature = "backups")]
    ImportBackup {
        path: String,
        reply: oneshot::Sender<Result<usize, String>>,
    },
    /// Compute the safety number (identity fingerprint) for a 1:1 contact.
    SafetyNumber {
        uuid: Uuid,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// Update our own profile (display name + optional about).
    UpdateProfile {
        given_name: String,
        family_name: Option<String>,
        about: Option<String>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Typing indicator (start/stop) for a 1:1 conversation. Fire-and-forget.
    Typing {
        conversation_id: String,
        started: bool,
    },
}

/// Core messaging service.
///
/// Architecture:
/// - A single dedicated thread runs a tokio LocalSet (presage futures are !Send)
/// - The receive loop runs as a spawned local task on that thread
/// - Sends go through an mpsc channel to the same thread
/// - Read queries (conversations, messages) use a separate store clone
#[derive(Clone)]
pub struct MessagingService {
    read_store: Arc<Mutex<Option<SqliteStore>>>,
    db_path: Arc<PathBuf>,
    db_passphrase: Arc<Mutex<Option<String>>>,
    self_aci: Arc<Mutex<Option<String>>>,
    send_tx: Arc<Mutex<Option<mpsc::UnboundedSender<SendRequest>>>>,
    /// Whether outbound receipts (DELIVERY + READ) are emitted. Gated by
    /// the user's `read_receipts` setting; the receive loop and the
    /// `mark_conversation_read` command both consult this before
    /// enqueueing a Receipt SendRequest.
    pub read_receipts_enabled: Arc<std::sync::atomic::AtomicBool>,
    /// This linked device's Signal device id, surfaced for the calling
    /// subsystem (RingRTC signaling needs `local_device_id`). 0 until the
    /// manager loads — read lazily by the call thread, which can't place or
    /// receive a call before the manager is up anyway.
    pub self_device_id: Arc<std::sync::atomic::AtomicU32>,
    /// The calling subsystem, once spawned (lib.rs sets this after the Tauri
    /// AppHandle exists). The receive loop routes inbound `CallMessage`
    /// envelopes here. `None` until set — call messages arriving before then
    /// are dropped, which is fine: there can't be a call in flight yet.
    call_controller: Arc<Mutex<Option<crate::calling::manager::CallController>>>,
}

impl MessagingService {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            read_store: Arc::new(Mutex::new(None)),
            db_path: Arc::new(db_path),
            db_passphrase: Arc::new(Mutex::new(None)),
            self_aci: Arc::new(Mutex::new(None)),
            send_tx: Arc::new(Mutex::new(None)),
            // Default to "on" — the user can disable via Settings.
            // AppState::new overwrites this immediately after construction
            // with the value persisted in settings.json.
            read_receipts_enabled: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            self_device_id: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            call_controller: Arc::new(Mutex::new(None)),
        }
    }

    /// Hand the messaging service a `CallController` so the receive loop can
    /// route inbound CallMessage envelopes to it. Called once from lib.rs
    /// setup after the calling thread is spawned.
    pub async fn set_call_controller(
        &self,
        controller: crate::calling::manager::CallController,
    ) {
        *self.call_controller.lock().await = Some(controller);
    }

    /// Enqueue a CallMessage send. Fire-and-forget — used by the calling
    /// subsystem's signaling bridge.
    pub async fn send_call_message(
        &self,
        recipient_uuid: Uuid,
        call_message: presage::libsignal_service::proto::CallMessage,
    ) {
        let tx_guard = self.send_tx.lock().await;
        if let Some(tx) = tx_guard.as_ref() {
            let _ = tx.send(SendRequest::CallMessage {
                recipient_uuid,
                call_message,
            });
        }
    }

    /// Try to load an existing registered manager and start messaging.
    /// Returns true if successful (device was previously linked).
    pub async fn try_load_and_start<F>(&self, passphrase: &str, on_event: F) -> bool
    where
        F: Fn(InboundEvent) + Send + Sync + 'static,
    {
        let store = match self.open_store(passphrase).await {
            Ok(s) => s,
            Err(e) => {
                debug!("could not open store: {}", e);
                return false;
            }
        };

        let read_store = store.clone();

        match Manager::load_registered(store).await {
            Ok(mgr) => {
                let aci = mgr
                    .registration_data()
                    .service_ids
                    .aci()
                    .service_id_string();
                info!("loaded registered manager, aci={}", aci);
                if let Some(dev) = mgr.registration_data().device_id {
                    self.self_device_id
                        .store(u32::from(dev), std::sync::atomic::Ordering::Relaxed);
                }
                *self.self_aci.lock().await = Some(aci);
                *self.read_store.lock().await = Some(read_store);
                *self.db_passphrase.lock().await = Some(passphrase.to_string());

                self.start_messaging_thread(mgr, on_event).await;
                true
            }
            Err(e) => {
                info!("not yet registered: {}", e);
                // Only delete the DB if it's truly empty/corrupt (< 4KB),
                // not if it has real data from a previous session
                if self.db_path.exists() {
                    let size = std::fs::metadata(self.db_path.as_ref())
                        .map(|m| m.len())
                        .unwrap_or(0);
                    if size < 4096 {
                        let _ = std::fs::remove_file(self.db_path.as_ref());
                        info!("removed empty/corrupt database file ({}B)", size);
                    } else {
                        info!("keeping existing database ({}B) — may need re-link", size);
                    }
                }
                false
            }
        }
    }

    /// Set up after provisioning — store the manager and start messaging.
    /// Spawns a new thread for messaging.
    pub async fn start_after_provisioning<F>(
        &self,
        mgr: Manager<SqliteStore, Registered>,
        on_event: F,
    ) where
        F: Fn(InboundEvent) + Send + Sync + 'static,
    {
        let aci = mgr
            .registration_data()
            .service_ids
            .aci()
            .service_id_string();
        let read_store = mgr.store().clone();
        info!("starting messaging after provisioning, aci={}", aci);
        if let Some(dev) = mgr.registration_data().device_id {
            self.self_device_id
                .store(u32::from(dev), std::sync::atomic::Ordering::Relaxed);
        }
        *self.self_aci.lock().await = Some(aci);
        *self.read_store.lock().await = Some(read_store);

        self.start_messaging_thread(mgr, on_event).await;
    }

    /// Set up after provisioning on the CURRENT LocalSet (no new thread).
    /// Call this when you're already on a LocalSet-capable thread.
    pub async fn start_after_provisioning_local(
        &self,
        mgr: Manager<SqliteStore, Registered>,
        passphrase: &str,
        on_event: impl Fn(InboundEvent) + Send + Sync + 'static,
    ) {
        let aci = mgr
            .registration_data()
            .service_ids
            .aci()
            .service_id_string();
        info!("starting messaging locally after provisioning, aci={}", aci);
        if let Some(dev) = mgr.registration_data().device_id {
            self.self_device_id
                .store(u32::from(dev), std::sync::atomic::Ordering::Relaxed);
        }
        *self.self_aci.lock().await = Some(aci);
        *self.read_store.lock().await = Some(mgr.store().clone());
        *self.db_passphrase.lock().await = Some(passphrase.to_string());

        let ctx = self.spawn_send_channel().await;
        tokio::task::spawn_local(run_messaging(mgr, Arc::new(on_event), ctx));
    }

    /// Spawn the dedicated messaging thread with receive loop + send handler.
    /// Used by the cold-load path, which is not already on a LocalSet thread.
    async fn start_messaging_thread<F>(&self, mgr: Manager<SqliteStore, Registered>, on_event: F)
    where
        F: Fn(InboundEvent) + Send + Sync + 'static,
    {
        let ctx = self.spawn_send_channel().await;
        let on_event = Arc::new(on_event);

        std::thread::Builder::new()
            .name("signalui-messaging".to_string())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("messaging runtime");
                rt.block_on(async move {
                    tokio::task::LocalSet::new()
                        .run_until(run_messaging(mgr, on_event, ctx))
                        .await;
                });
            })
            .expect("failed to spawn messaging thread");
    }

    /// Create the send-request mpsc channel, publish the sender on
    /// `self.send_tx`, and bundle everything `run_messaging` needs that comes
    /// from `&self` into a [`MessagingContext`]. Keeps the two `start_*`
    /// entry points down to a couple of lines each.
    async fn spawn_send_channel(&self) -> MessagingContext {
        let (send_tx, send_rx) = mpsc::unbounded_channel::<SendRequest>();
        *self.send_tx.lock().await = Some(send_tx.clone());
        MessagingContext {
            send_tx,
            send_rx,
            self_aci: self.self_aci.clone(),
            receipts_enabled: self.read_receipts_enabled.clone(),
            attachments_dir: self
                .db_path
                .parent()
                .unwrap_or(std::path::Path::new("/tmp"))
                .join("attachments"),
            call_controller: self.call_controller.clone(),
        }
    }

    pub async fn self_id(&self) -> Option<String> {
        self.self_aci.lock().await.clone()
    }

    /// Send a text message via the channel to the messaging thread.
    pub async fn send_message(&self, conversation_id: &str, body: &str) -> Result<(), String> {
        self.send_message_with_attachments(conversation_id, body, vec![], None)
            .await
    }

    /// Send a message with optional attachments and an optional reply quote.
    pub async fn send_message_with_attachments(
        &self,
        conversation_id: &str,
        body: &str,
        file_paths: Vec<String>,
        quote: Option<crate::messaging::types::QuoteInput>,
    ) -> Result<(), String> {
        let tx_guard = self.send_tx.lock().await;
        let tx = tx_guard.as_ref().ok_or("messaging not started")?;

        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SendRequest::Text {
            conversation_id: conversation_id.to_string(),
            body: body.to_string(),
            file_paths,
            quote,
            reply: reply_tx,
        })
        .map_err(|_| "send channel closed".to_string())?;

        drop(tx_guard);

        reply_rx
            .await
            .map_err(|_| "send reply dropped".to_string())?
    }

    /// Send (or remove) an emoji reaction to a target message.
    pub async fn send_reaction(
        &self,
        conversation_id: &str,
        target_author_uuid: &str,
        target_timestamp: u64,
        emoji: &str,
        remove: bool,
    ) -> Result<(), String> {
        let tx_guard = self.send_tx.lock().await;
        let tx = tx_guard.as_ref().ok_or("messaging not started")?;
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SendRequest::Reaction {
            conversation_id: conversation_id.to_string(),
            target_author_uuid: target_author_uuid.to_string(),
            target_timestamp,
            emoji: emoji.to_string(),
            remove,
            reply: reply_tx,
        })
        .map_err(|_| "send channel closed".to_string())?;
        drop(tx_guard);
        reply_rx.await.map_err(|_| "send reply dropped".to_string())?
    }

    /// Send a typing start/stop indicator (1:1 only). Fire-and-forget so it
    /// never blocks the keystroke path; a dropped indicator is harmless.
    pub async fn send_typing(&self, conversation_id: &str, started: bool) {
        let tx_guard = self.send_tx.lock().await;
        if let Some(tx) = tx_guard.as_ref() {
            let _ = tx.send(SendRequest::Typing {
                conversation_id: conversation_id.to_string(),
                started,
            });
        }
    }

    /// Pin or unpin a message.
    pub async fn set_pin(
        &self,
        conversation_id: &str,
        target_author_uuid: &str,
        target_timestamp: u64,
        pinned: bool,
    ) -> Result<(), String> {
        let tx_guard = self.send_tx.lock().await;
        let tx = tx_guard.as_ref().ok_or("messaging not started")?;
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SendRequest::Pin {
            conversation_id: conversation_id.to_string(),
            target_author_uuid: target_author_uuid.to_string(),
            target_timestamp,
            pinned,
            reply: reply_tx,
        })
        .map_err(|_| "send channel closed".to_string())?;
        drop(tx_guard);
        reply_rx.await.map_err(|_| "pin reply dropped".to_string())?
    }

    /// Cast a vote on a poll.
    pub async fn vote_poll(
        &self,
        conversation_id: &str,
        target_author_uuid: &str,
        target_timestamp: u64,
        option_indexes: Vec<u32>,
    ) -> Result<(), String> {
        let tx_guard = self.send_tx.lock().await;
        let tx = tx_guard.as_ref().ok_or("messaging not started")?;
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SendRequest::PollVote {
            conversation_id: conversation_id.to_string(),
            target_author_uuid: target_author_uuid.to_string(),
            target_timestamp,
            option_indexes,
            reply: reply_tx,
        })
        .map_err(|_| "send channel closed".to_string())?;
        drop(tx_guard);
        reply_rx.await.map_err(|_| "vote reply dropped".to_string())?
    }

    /// Decrypt + summarize an encrypted transfer archive (preview before
    /// import). Runs on the messaging thread (store/manager access).
    #[cfg(feature = "backups")]
    pub async fn preview_backup(
        &self,
        path: &str,
    ) -> Result<crate::backups::BackupSummary, String> {
        let tx_guard = self.send_tx.lock().await;
        let tx = tx_guard.as_ref().ok_or("messaging not started")?;
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SendRequest::PreviewBackup { path: path.to_string(), reply: reply_tx })
            .map_err(|_| "send channel closed".to_string())?;
        drop(tx_guard);
        reply_rx.await.map_err(|_| "preview reply dropped".to_string())?
    }

    /// Import contacts from an encrypted transfer archive; returns the count
    /// written. (Groups + messages follow — [LIVE-TEST] with a real archive.)
    #[cfg(feature = "backups")]
    pub async fn import_backup(&self, path: &str) -> Result<usize, String> {
        let tx_guard = self.send_tx.lock().await;
        let tx = tx_guard.as_ref().ok_or("messaging not started")?;
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SendRequest::ImportBackup { path: path.to_string(), reply: reply_tx })
            .map_err(|_| "send channel closed".to_string())?;
        drop(tx_guard);
        reply_rx.await.map_err(|_| "import reply dropped".to_string())?
    }

    /// Whether this device can derive a BackupKey (AEP persisted at link).
    /// Gates the future Link & Sync history-import affordance.
    pub async fn backup_available(&self) -> bool {
        let tx_guard = self.send_tx.lock().await;
        let Some(tx) = tx_guard.as_ref() else { return false };
        let (reply_tx, reply_rx) = oneshot::channel();
        if tx.send(SendRequest::BackupStatus { reply: reply_tx }).is_err() {
            return false;
        }
        drop(tx_guard);
        reply_rx.await.unwrap_or(false)
    }

    /// Compute the safety number for a contact — the 60-digit identity
    /// fingerprint Signal shows for out-of-band verification. Uses libsignal's
    /// own Fingerprint with the exact inputs Signal-Desktop uses (iterations
    /// 5200, version 2, 16-byte ACI identifiers), so the number matches the
    /// official client.
    pub async fn safety_number(&self, uuid: Uuid) -> Result<String, String> {
        let tx_guard = self.send_tx.lock().await;
        let tx = tx_guard.as_ref().ok_or("messaging not started")?;
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SendRequest::SafetyNumber { uuid, reply: reply_tx })
            .map_err(|_| "send channel closed".to_string())?;
        drop(tx_guard);
        reply_rx.await.map_err(|_| "safety number reply dropped".to_string())?
    }

    /// Set our own profile display name (and optional about line).
    pub async fn set_profile(
        &self,
        given_name: String,
        family_name: Option<String>,
        about: Option<String>,
    ) -> Result<(), String> {
        let tx_guard = self.send_tx.lock().await;
        let tx = tx_guard.as_ref().ok_or("messaging not started")?;
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SendRequest::UpdateProfile {
            given_name,
            family_name,
            about,
            reply: reply_tx,
        })
        .map_err(|_| "send channel closed".to_string())?;
        drop(tx_guard);
        reply_rx.await.map_err(|_| "update profile reply dropped".to_string())?
    }

    /// Fetch a contact's profile from the service (network), cache it in the
    /// store, and return the resolved display name. `Ok(None)` if we have no
    /// profile key for them. A wrong result here is a cosmetic name issue,
    /// not a security signal.
    pub async fn fetch_profile(&self, uuid: Uuid) -> Result<Option<String>, String> {
        let tx_guard = self.send_tx.lock().await;
        let tx = tx_guard.as_ref().ok_or("messaging not started")?;
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SendRequest::FetchProfile { uuid, reply: reply_tx })
            .map_err(|_| "send channel closed".to_string())?;
        drop(tx_guard);
        reply_rx.await.map_err(|_| "fetch profile reply dropped".to_string())?
    }

    /// List the account's linked devices (read-only — unlinking requires the
    /// primary phone).
    pub async fn list_devices(&self) -> Result<Vec<crate::messaging::types::DeviceDto>, String> {
        let tx_guard = self.send_tx.lock().await;
        let tx = tx_guard.as_ref().ok_or("messaging not started")?;
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SendRequest::ListDevices { reply: reply_tx })
            .map_err(|_| "send channel closed".to_string())?;
        drop(tx_guard);
        reply_rx.await.map_err(|_| "list devices reply dropped".to_string())?
    }

    /// Delete-for-everyone a message you sent.
    pub async fn send_delete(
        &self,
        conversation_id: &str,
        target_timestamp: u64,
    ) -> Result<(), String> {
        let tx_guard = self.send_tx.lock().await;
        let tx = tx_guard.as_ref().ok_or("messaging not started")?;
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SendRequest::Delete {
            conversation_id: conversation_id.to_string(),
            target_timestamp,
            reply: reply_tx,
        })
        .map_err(|_| "send channel closed".to_string())?;
        drop(tx_guard);
        reply_rx.await.map_err(|_| "send reply dropped".to_string())?
    }

    /// Send a receipt (DELIVERY / READ / VIEWED) back to a recipient for one
    /// or more inbound messages.
    pub async fn send_receipt(
        &self,
        recipient_uuid: Uuid,
        kind: ReceiptKind,
        timestamps: Vec<u64>,
    ) -> Result<(), String> {
        if timestamps.is_empty() {
            return Ok(());
        }
        let tx_guard = self.send_tx.lock().await;
        let tx = tx_guard.as_ref().ok_or("messaging not started")?;

        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SendRequest::Receipt {
            recipient_uuid,
            kind,
            timestamps,
            reply: reply_tx,
        })
        .map_err(|_| "send channel closed".to_string())?;
        drop(tx_guard);

        reply_rx
            .await
            .map_err(|_| "send reply dropped".to_string())?
    }

    /// Open a fresh store connection for reads.
    /// This ensures we see the latest data written by the messaging thread.
    async fn fresh_read_store(&self) -> Result<SqliteStore, String> {
        let passphrase_guard = self.db_passphrase.lock().await;
        let passphrase = passphrase_guard.as_ref().ok_or("no passphrase")?;
        self.open_store(passphrase).await
    }

    /// Load conversations from the store (opens fresh connection to see latest data).
    pub async fn get_conversations(&self) -> Result<Vec<Conversation>, String> {
        let store = self.fresh_read_store().await?;

        let mut conversations = Vec::new();
        let self_aci = self.self_aci.lock().await.clone();
        // Avatars cache lives next to the DB: <app_data>/avatars/.
        let avatars_dir = self
            .db_path
            .parent()
            .map(|p| p.join("avatars"))
            .unwrap_or_else(|| std::path::PathBuf::from("avatars"));

        let contacts = store
            .contacts()
            .await
            .map_err(|e| format!("failed to load contacts: {}", e))?;

        let mut contact_count = 0;
        let contacts: Vec<_> = contacts.flatten().collect();
        info!("contacts query returned {} results", contacts.len());

        for contact in contacts {
            contact_count += 1;
            let uuid_str = contact.uuid.to_string();
            let name = if Some(&uuid_str) == self_aci.as_ref() {
                "Note to Self".to_string()
            } else if !contact.name.is_empty() {
                contact.name.clone()
            } else {
                contact
                    .phone_number
                    .as_ref()
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| uuid_str.clone())
            };

            let service_id = ServiceId::Aci(presage::libsignal_service::protocol::Aci::from(
                contact.uuid,
            ));
            let thread = presage::store::Thread::Contact(service_id);
            let (last_message, last_timestamp) = get_last_message_info(&store, &thread).await;

            // Contact avatars arrive with the contact sync — purely local bytes.
            let avatar_path = contact.avatar.as_ref().and_then(|a| {
                cache_avatar(&avatars_dir, &format!("c-{uuid_str}"), &a.content_type, &a.reader)
            });

            conversations.push(Conversation {
                id: uuid_str,
                name,
                last_message,
                last_timestamp,
                is_group: false,
                avatar_path,
            });
        }

        let groups = store
            .groups()
            .await
            .map_err(|e| format!("failed to load groups: {}", e))?;

        for group_result in groups.flatten() {
            let (master_key, group) = group_result;
            let id = hex::encode(master_key);
            let thread = presage::store::Thread::Group(master_key);
            let (last_message, last_timestamp) = get_last_message_info(&store, &thread).await;

            // Group avatars are cached locally by presage after group sync.
            let avatar_path = match store.group_avatar(master_key).await {
                Ok(Some(bytes)) => cache_avatar(&avatars_dir, &format!("g-{id}"), "image/jpeg", &bytes),
                _ => None,
            };

            conversations.push(Conversation {
                id,
                name: group.title,
                last_message,
                last_timestamp,
                is_group: true,
                avatar_path,
            });
        }

        conversations.sort_by(|a, b| b.last_timestamp.cmp(&a.last_timestamp));
        Ok(conversations)
    }

    /// Full-text-ish search across every conversation. Case-insensitive
    /// substring match on message bodies. Read-only; bounded to `limit` hits.
    pub async fn search_messages(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::messaging::types::SearchHit>, String> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let store = self.fresh_read_store().await?;
        let self_aci = self.self_aci.lock().await.clone();

        // Enumerate (thread, id, name, is_group) for contacts + groups.
        let mut threads: Vec<(presage::store::Thread, String, String, bool)> = Vec::new();
        if let Ok(contacts) = store.contacts().await {
            for contact in contacts.flatten() {
                let uuid_str = contact.uuid.to_string();
                let name = if Some(&uuid_str) == self_aci.as_ref() {
                    "Note to Self".to_string()
                } else if !contact.name.is_empty() {
                    contact.name.clone()
                } else {
                    uuid_str.clone()
                };
                let service_id = ServiceId::Aci(
                    presage::libsignal_service::protocol::Aci::from(contact.uuid),
                );
                threads.push((
                    presage::store::Thread::Contact(service_id),
                    uuid_str,
                    name,
                    false,
                ));
            }
        }
        if let Ok(groups) = store.groups().await {
            for (master_key, group) in groups.flatten() {
                threads.push((
                    presage::store::Thread::Group(master_key),
                    hex::encode(master_key),
                    group.title,
                    true,
                ));
            }
        }

        let mut hits: Vec<crate::messaging::types::SearchHit> = Vec::new();
        for (thread, conv_id, conv_name, is_group) in threads {
            let Ok(iter) = store.messages(&thread, ..).await else {
                continue;
            };
            for msg_result in iter {
                let Ok(content) = msg_result else { continue };
                let Some(mut chat_msg) = content_to_chat_message(&content, &self_aci) else {
                    continue;
                };
                let Some(body) = chat_msg.body.clone() else { continue };
                if !body.to_lowercase().contains(&q) {
                    continue;
                }
                enrich_sender_name(&mut chat_msg, &store, &self_aci).await;
                hits.push(crate::messaging::types::SearchHit {
                    conversation_id: conv_id.clone(),
                    conversation_name: conv_name.clone(),
                    is_group,
                    timestamp: chat_msg.timestamp,
                    sender_name: chat_msg.sender_name.clone(),
                    snippet: body,
                });
                if hits.len() >= limit {
                    break;
                }
            }
            if hits.len() >= limit {
                break;
            }
        }
        hits.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        Ok(hits)
    }

    /// Load every attachment ever sent or received in a conversation, sorted
    /// newest-first. Used by the media-browser modal.
    ///
    /// Walks `store.messages(thread, ..)` (same iterator as [`get_messages`])
    /// but flattens each message's `attachments` into the result. Outgoing
    /// messages (sync envelopes) and incoming messages are both included.
    /// Messages with no attachments are skipped.
    pub async fn get_conversation_media(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<crate::messaging::types::MediaItem>, String> {
        let store = self.fresh_read_store().await?;
        let thread = parse_thread(conversation_id)?;
        let self_aci = self.self_aci.lock().await.clone();

        let messages_iter = store
            .messages(&thread, ..)
            .await
            .map_err(|e| format!("failed to load messages: {}", e))?;

        let mut items: Vec<crate::messaging::types::MediaItem> = Vec::new();
        for msg_result in messages_iter {
            let Ok(content) = msg_result else { continue };
            let Some(mut chat_msg) = content_to_chat_message(&content, &self_aci) else {
                continue;
            };
            if chat_msg.attachments.is_empty() {
                continue;
            }
            enrich_sender_name(&mut chat_msg, &store, &self_aci).await;
            for att in chat_msg.attachments.drain(..) {
                items.push(crate::messaging::types::MediaItem {
                    timestamp: chat_msg.timestamp,
                    sender_id: chat_msg.sender_id.clone(),
                    sender_name: chat_msg.sender_name.clone(),
                    is_outgoing: chat_msg.is_outgoing,
                    attachment: att,
                });
            }
        }
        items.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        Ok(items)
    }

    /// Load messages from the store (opens fresh connection).
    pub async fn get_messages(&self, conversation_id: &str) -> Result<Vec<ChatMessage>, String> {
        let store = self.fresh_read_store().await?;

        let thread = parse_thread(conversation_id)?;
        let self_aci = self.self_aci.lock().await.clone();

        let messages_iter = store
            .messages(&thread, ..)
            .await
            .map_err(|e| format!("failed to load messages: {}", e))?;

        let mut messages = Vec::new();
        for msg_result in messages_iter {
            if let Ok(content) = msg_result {
                if let Some(mut chat_msg) = content_to_chat_message(&content, &self_aci) {
                    enrich_sender_name(&mut chat_msg, &store, &self_aci).await;
                    messages.push(chat_msg);
                }
            }
        }

        messages.sort_by_key(|m| m.timestamp);
        Ok(messages)
    }

    async fn open_store(&self, passphrase: &str) -> Result<SqliteStore, String> {
        let db_url = format!("sqlite:{}?mode=rwc", self.db_path.to_string_lossy());
        SqliteStore::open_with_passphrase(&db_url, Some(passphrase), OnNewIdentity::Trust)
            .await
            .map_err(|e| format!("store: {}", e))
    }
}

/// Everything `run_messaging` needs that originates from the
/// `MessagingService` struct — bundled so the two `start_*` entry points
/// don't each have to thread six clones through by hand.
struct MessagingContext {
    send_tx: mpsc::UnboundedSender<SendRequest>,
    send_rx: mpsc::UnboundedReceiver<SendRequest>,
    self_aci: Arc<Mutex<Option<String>>>,
    receipts_enabled: Arc<std::sync::atomic::AtomicBool>,
    attachments_dir: PathBuf,
    call_controller: Arc<Mutex<Option<crate::calling::manager::CallController>>>,
}

/// The core messaging work, shared by both `start_*` entry points: sync
/// contacts from Storage Service once, spawn the receive loop, then own the
/// send-handler loop on the calling task.
///
/// MUST be polled from within a `LocalSet` — presage's futures are `!Send`.
async fn run_messaging(
    mgr: Manager<SqliteStore, Registered>,
    on_event: Arc<dyn Fn(InboundEvent) + Send + Sync>,
    ctx: MessagingContext,
) {
    let MessagingContext {
        send_tx,
        mut send_rx,
        self_aci,
        receipts_enabled,
        attachments_dir,
        call_controller,
    } = ctx;

    let mut mgr_send = mgr.clone();
    let mgr_download = mgr.clone();
    let mgr_recv = mgr;
    let _ = std::fs::create_dir_all(&attachments_dir);

    // Storage Service is the only contact-sync path. We deliberately do NOT
    // call `request_contacts()`: modern primaries answer the legacy sync with
    // an empty stub, and presage's `Received::Contacts` handler clears the
    // contacts table before iterating — which would wipe everything
    // `sync_storage` just populated.
    match mgr_send.sync_storage().await {
        Ok(n) => info!("storage service sync: saved {} contacts", n),
        Err(e) => warn!("storage service sync failed: {}", e),
    }

    // Receive loop runs as its own local task; the send handler owns this one.
    tokio::task::spawn_local(receive_loop(
        mgr_recv,
        mgr_download,
        self_aci,
        on_event,
        send_tx,
        receipts_enabled,
        attachments_dir,
        call_controller,
    ));

    info!("send handler ready");
    while let Some(req) = send_rx.recv().await {
        handle_send_request(&mut mgr_send, req).await;
    }
}

/// Reconnecting receive loop: pull the message stream, dispatch each
/// `Received` variant, and on stream end / error sleep 5s and reconnect.
/// Never returns under normal operation.
#[allow(clippy::too_many_arguments)]
async fn receive_loop(
    mut mgr_recv: Manager<SqliteStore, Registered>,
    mgr_download: Manager<SqliteStore, Registered>,
    self_aci: Arc<Mutex<Option<String>>>,
    on_event: Arc<dyn Fn(InboundEvent) + Send + Sync>,
    send_tx: mpsc::UnboundedSender<SendRequest>,
    receipts_enabled: Arc<std::sync::atomic::AtomicBool>,
    attachments_dir: PathBuf,
    call_controller: Arc<Mutex<Option<crate::calling::manager::CallController>>>,
) {
    use presage::libsignal_service::content::ContentBody;
    use presage::model::messages::Received;
    loop {
        info!("starting receive loop");
        match mgr_recv.receive_messages().await {
            Ok(stream) => {
                let self_aci_val = self_aci.lock().await.clone();
                futures::pin_mut!(stream);
                while let Some(received) = stream.next().await {
                    match received {
                        Received::QueueEmpty => info!("message queue synced"),
                        Received::Contacts => info!("contacts synced"),
                        Received::Content(content) => {
                            // Voice-call signaling routes to the calling
                            // subsystem, not the WebView. It's not a
                            // DataMessage so process_content / the delivery
                            // receipt path are no-ops for it anyway.
                            if let ContentBody::CallMessage(cm) = &content.body {
                                if let Some(cc) =
                                    call_controller.lock().await.as_ref()
                                {
                                    cc.on_call_message(
                                        content.metadata.sender.raw_uuid(),
                                        content.metadata.sender_device.into(),
                                        cm.clone(),
                                    );
                                }
                            }
                            auto_delivery_receipt(
                                &content,
                                &self_aci_val,
                                &send_tx,
                                &receipts_enabled,
                            );
                            process_content(
                                &content,
                                &self_aci_val,
                                &mgr_download,
                                &attachments_dir,
                                on_event.as_ref(),
                            )
                            .await;
                        }
                    }
                }
                warn!("receive stream ended, reconnecting...");
            }
            Err(e) => error!("receive error: {}, retrying in 5s", e),
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

async fn process_content(
    content: &Content,
    self_aci: &Option<String>,
    mgr_download: &Manager<SqliteStore, Registered>,
    att_dir: &std::path::Path,
    on_event: &(dyn Fn(InboundEvent) + Send + Sync),
) {
    for mut ev in derive_inbound_events(content, self_aci) {
        if let InboundEvent::Message {
            ref mut message, ..
        } = ev
        {
            download_attachments(mgr_download, message, att_dir).await;
            enrich_sender_name(message, mgr_download.store(), self_aci).await;
        }
        on_event(ev);
    }
}

/// Fire a DELIVERY receipt for a freshly-received DataMessage.
///
/// Skips: own messages (sync from another linked device), non-DataMessage content
/// (typing indicators, receipts themselves, sync messages), and any envelope
/// where the sender UUID can't be resolved. Failure to enqueue the receipt is
/// not propagated — the receive loop must keep going.
fn auto_delivery_receipt(
    content: &Content,
    self_aci: &Option<String>,
    send_tx: &mpsc::UnboundedSender<SendRequest>,
    enabled: &std::sync::atomic::AtomicBool,
) {
    use presage::libsignal_service::content::ContentBody;

    if !enabled.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    // Only acknowledge real DataMessages. Sync / typing / receipt envelopes
    // don't need delivery receipts and would create a feedback loop with
    // peers that auto-ack receipts of their own.
    let timestamp = match &content.body {
        ContentBody::DataMessage(dm) => dm.timestamp.unwrap_or(content.metadata.timestamp),
        ContentBody::EditMessage(em) => em
            .target_sent_timestamp
            .unwrap_or(content.metadata.timestamp),
        _ => return,
    };

    let sender_uuid = content.metadata.sender.raw_uuid();
    let sender_str = sender_uuid.to_string();

    // Don't ack ourselves — sync envelopes from our other linked devices.
    if Some(&sender_str) == self_aci.as_ref() {
        return;
    }

    let (reply_tx, _reply_rx) = oneshot::channel();
    let req = SendRequest::Receipt {
        recipient_uuid: sender_uuid,
        kind: ReceiptKind::Delivered,
        timestamps: vec![timestamp],
        reply: reply_tx,
    };
    if send_tx.send(req).is_err() {
        warn!("auto delivery receipt: send channel closed");
    }
}

/// Dispatch a send-thread request: text/attachment message vs receipt message.
async fn handle_send_request(mgr: &mut Manager<SqliteStore, Registered>, req: SendRequest) {
    match req {
        SendRequest::Text {
            conversation_id,
            body,
            file_paths,
            quote,
            reply,
        } => {
            info!("processing send to {}", conversation_id);
            let result = do_send(mgr, &conversation_id, &body, &file_paths, quote).await;
            if let Err(ref e) = result {
                error!("send failed: {}", e);
            } else {
                info!("message sent successfully");
            }
            let _ = reply.send(result);
        }
        SendRequest::Receipt {
            recipient_uuid,
            kind,
            timestamps,
            reply,
        } => {
            info!(
                "processing {:?} receipt to {} for {} message(s)",
                kind,
                recipient_uuid,
                timestamps.len()
            );
            let result = do_send_receipt(mgr, recipient_uuid, kind, timestamps).await;
            if let Err(ref e) = result {
                error!("receipt send failed: {}", e);
            } else {
                info!("receipt sent successfully");
            }
            let _ = reply.send(result);
        }
        SendRequest::CallMessage {
            recipient_uuid,
            call_message,
        } => {
            if let Err(e) = do_send_call_message(mgr, recipient_uuid, call_message).await {
                error!("call message send failed: {}", e);
            }
        }
        SendRequest::Reaction {
            conversation_id,
            target_author_uuid,
            target_timestamp,
            emoji,
            remove,
            reply,
        } => {
            let result = do_send_reaction(
                mgr,
                &conversation_id,
                &target_author_uuid,
                target_timestamp,
                &emoji,
                remove,
            )
            .await;
            if let Err(ref e) = result {
                error!("reaction send failed: {}", e);
            }
            let _ = reply.send(result);
        }
        SendRequest::Delete {
            conversation_id,
            target_timestamp,
            reply,
        } => {
            let result = do_send_delete(mgr, &conversation_id, target_timestamp).await;
            if let Err(ref e) = result {
                error!("delete send failed: {}", e);
            }
            let _ = reply.send(result);
        }
        SendRequest::Typing {
            conversation_id,
            started,
        } => {
            if let Err(e) = do_send_typing(mgr, &conversation_id, started).await {
                error!("typing send failed: {}", e);
            }
        }
        SendRequest::ListDevices { reply } => {
            let current = mgr.registration_data().device_id;
            let result = mgr
                .devices()
                .await
                .map_err(|e| format!("failed to list devices: {e}"))
                .map(|devices| {
                    devices
                        .into_iter()
                        .map(|d| {
                            let id = u32::from(d.id);
                            crate::messaging::types::DeviceDto {
                                id,
                                name: d.name,
                                created_at: d.created_at.timestamp_millis(),
                                last_seen: d.last_seen.timestamp_millis(),
                                is_current: Some(id) == current,
                            }
                        })
                        .collect()
                });
            let _ = reply.send(result);
        }
        SendRequest::FetchProfile { uuid, reply } => {
            use presage::libsignal_service::protocol::Aci;
            let service_id = ServiceId::Aci(Aci::from(uuid));
            let result = match mgr.store().profile_key(&service_id).await {
                Ok(Some(key)) => match mgr.retrieve_profile_by_uuid(Aci::from(uuid), key).await {
                    Ok(profile) => Ok(profile.name.map(|n| n.to_string()).filter(|s| !s.trim().is_empty())),
                    Err(e) => Err(format!("failed to fetch profile: {e}")),
                },
                Ok(None) => Ok(None),
                Err(e) => Err(format!("no profile key: {e}")),
            };
            let _ = reply.send(result);
        }
        SendRequest::Pin {
            conversation_id,
            target_author_uuid,
            target_timestamp,
            pinned,
            reply,
        } => {
            let result = do_send_pin(mgr, &conversation_id, &target_author_uuid, target_timestamp, pinned).await;
            if let Err(ref e) = result {
                error!("pin send failed: {}", e);
            }
            let _ = reply.send(result);
        }
        SendRequest::PollVote {
            conversation_id,
            target_author_uuid,
            target_timestamp,
            option_indexes,
            reply,
        } => {
            let result = do_send_poll_vote(
                mgr,
                &conversation_id,
                &target_author_uuid,
                target_timestamp,
                option_indexes,
            )
            .await;
            if let Err(ref e) = result {
                error!("poll vote send failed: {}", e);
            }
            let _ = reply.send(result);
        }
        SendRequest::BackupStatus { reply } => {
            let ready = mgr
                .registration_data()
                .account_entropy_pool()
                .and_then(crate::backups::derive_backup_key)
                .is_some();
            let _ = reply.send(ready);
        }
        #[cfg(feature = "backups")]
        SendRequest::PreviewBackup { path, reply } => {
            let result = preview_backup_impl(mgr, &path).await;
            let _ = reply.send(result);
        }
        #[cfg(feature = "backups")]
        SendRequest::ImportBackup { path, reply } => {
            let result = import_backup_impl(mgr, &path).await;
            let _ = reply.send(result);
        }
        SendRequest::SafetyNumber { uuid, reply } => {
            let result = do_safety_number(mgr, uuid).await;
            let _ = reply.send(result);
        }
        SendRequest::UpdateProfile {
            given_name,
            family_name,
            about,
            reply,
        } => {
            use presage::libsignal_service::profile_name::ProfileName;
            let name = ProfileName {
                given_name,
                family_name: family_name.filter(|f| !f.trim().is_empty()),
            };
            let result = mgr
                .update_profile(name, about.filter(|a| !a.trim().is_empty()), None)
                .await
                .map_err(|e| format!("failed to update profile: {e}"));
            let _ = reply.send(result);
        }
    }
}

/// Send a CallMessage (voice-call signaling) envelope to a recipient.
async fn do_send_call_message(
    mgr: &mut Manager<SqliteStore, Registered>,
    recipient_uuid: Uuid,
    call_message: presage::libsignal_service::proto::CallMessage,
) -> Result<(), String> {
    use presage::libsignal_service::content::ContentBody;
    use presage::libsignal_service::protocol::{Aci, ServiceId};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_millis() as u64;
    let recipient = ServiceId::Aci(Aci::from(recipient_uuid));
    mgr.send_message(recipient, ContentBody::CallMessage(call_message), timestamp)
        .await
        .map_err(|e| format!("send_call_message: {}", e))
}

/// Send a single ReceiptMessage envelope back to a recipient.
async fn do_send_receipt(
    mgr: &mut Manager<SqliteStore, Registered>,
    recipient_uuid: Uuid,
    kind: ReceiptKind,
    timestamps: Vec<u64>,
) -> Result<(), String> {
    use presage::libsignal_service::content::ContentBody;
    use presage::libsignal_service::proto::receipt_message;
    use presage::libsignal_service::protocol::{Aci, ServiceId};

    let proto_kind = match kind {
        ReceiptKind::Delivered => receipt_message::Type::Delivery,
        ReceiptKind::Read => receipt_message::Type::Read,
        ReceiptKind::Viewed => receipt_message::Type::Viewed,
    };

    let receipt = ReceiptMessage {
        r#type: Some(proto_kind as i32),
        timestamp: timestamps,
    };

    // Receipts are stamped with their own envelope timestamp; the inner
    // `timestamp` repeated field references the message(s) being acknowledged.
    let envelope_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_millis() as u64;

    let recipient = ServiceId::Aci(Aci::from(recipient_uuid));
    mgr.send_message(recipient, ContentBody::ReceiptMessage(receipt), envelope_ts)
        .await
        .map_err(|e| format!("send_receipt: {}", e))
}

/// Send a `DataMessage.reaction` (add or remove) to the target message's
/// thread. `target_author_uuid` is the ACI of the message being reacted to.
/// Send a `DataMessage.delete` (delete-for-everyone) for a message we sent.
async fn do_send_delete(
    mgr: &mut Manager<SqliteStore, Registered>,
    conversation_id: &str,
    target_timestamp: u64,
) -> Result<(), String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_millis() as u64;
    let thread = parse_thread(conversation_id)?;
    let data_message = DataMessage {
        timestamp: Some(timestamp),
        delete: Some(Delete {
            target_sent_timestamp: Some(target_timestamp),
        }),
        ..Default::default()
    };
    match thread {
        presage::store::Thread::Contact(service_id) => mgr
            .send_message(service_id, data_message, timestamp)
            .await
            .map_err(|e| format!("failed to send delete: {e}")),
        presage::store::Thread::Group(master_key) => mgr
            .send_message_to_group(&master_key, data_message, timestamp)
            .await
            .map_err(|e| format!("failed to send group delete: {e}")),
    }
}

/// Send a `TypingMessage` (start/stop) to a 1:1 conversation. Groups are
/// skipped for now (they need the group_id set on the message).
/// Compute a contact's safety number. Mirrors Signal-Desktop's
/// `generateSafetyNumber` exactly: libsignal `Fingerprint` with
/// ITERATION_COUNT=5200, SERVICE_ID_VERSION=2, and the 16-byte ACI UUIDs as
/// the local/remote identifiers — so the result matches the official client.
#[cfg(feature = "backups")]
async fn import_backup_impl(
    mgr: &mut Manager<SqliteStore, Registered>,
    path: &str,
) -> Result<usize, String> {
    use presage::libsignal_service::protocol::Aci;
    use presage::store::ContentsStore;
    let reg = mgr.registration_data();
    let aep = reg
        .account_entropy_pool()
        .ok_or("no account entropy pool (relink needed for backups)")?;
    let aci = Aci::from(reg.service_ids.aci);
    let key = crate::backups::derive_message_backup_key(aep, aci)
        .ok_or("could not derive backup key")?;
    let bytes = std::fs::read(path).map_err(|e| format!("read archive: {e}"))?;
    let contacts = crate::backups::extract_contacts(&bytes, &key).await?;

    // SqliteStore is a cheap clone over the same pool; writes persist.
    let mut store = mgr.store().clone();
    let mut n = 0usize;
    for c in &contacts {
        store
            .save_contact(c)
            .await
            .map_err(|e| format!("save contact: {e}"))?;
        n += 1;
    }
    Ok(n)
}

#[cfg(feature = "backups")]
async fn preview_backup_impl(
    mgr: &mut Manager<SqliteStore, Registered>,
    path: &str,
) -> Result<crate::backups::BackupSummary, String> {
    use presage::libsignal_service::protocol::Aci;
    let reg = mgr.registration_data();
    let aep = reg
        .account_entropy_pool()
        .ok_or("no account entropy pool (relink needed for backups)")?;
    let aci = Aci::from(reg.service_ids.aci);
    let key = crate::backups::derive_message_backup_key(aep, aci)
        .ok_or("could not derive backup key")?;
    let bytes = std::fs::read(path).map_err(|e| format!("read archive: {e}"))?;
    crate::backups::summarize_backup(&bytes, &key).await
}

async fn do_safety_number(
    mgr: &mut Manager<SqliteStore, Registered>,
    their_uuid: Uuid,
) -> Result<String, String> {
    use presage::libsignal_service::protocol::{
        DeviceId, Fingerprint, IdentityKeyStore, ProtocolAddress,
    };

    const ITERATIONS: u32 = 5200;
    const SERVICE_ID_VERSION: u32 = 2;

    let our_aci = mgr
        .registration_data()
        .service_ids
        .aci;
    let store = mgr.store().aci_protocol_store();
    let our_key_pair = store
        .get_identity_key_pair()
        .await
        .map_err(|e| format!("no local identity key: {e}"))?;
    let our_identity_key = our_key_pair.identity_key();

    // get_identity keys by address name only; device id is irrelevant.
    let device = DeviceId::new(1).expect("device id 1 is valid");
    let their_addr = ProtocolAddress::new(their_uuid.to_string(), device);
    let their_identity_key = store
        .get_identity(&their_addr)
        .await
        .map_err(|e| format!("identity lookup failed: {e}"))?
        .ok_or_else(|| {
            "no identity on file for this contact yet — exchange a message first"
                .to_string()
        })?;

    let fingerprint = Fingerprint::new(
        SERVICE_ID_VERSION,
        ITERATIONS,
        our_aci.as_bytes(),
        our_identity_key,
        their_uuid.as_bytes(),
        &their_identity_key,
    )
    .map_err(|e| format!("fingerprint error: {e}"))?;

    let digits = fingerprint
        .display_string()
        .map_err(|e| format!("display error: {e}"))?;

    // Group into space-separated 5-digit blocks, like the official client.
    let blocks: Vec<String> = digits
        .as_bytes()
        .chunks(5)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect();
    Ok(blocks.join(" "))
}

/// Send a `DataMessage.pollVote` referencing a poll message.
/// Send a `DataMessage.pinMessage`/`unpinMessage` for a target message.
async fn do_send_pin(
    mgr: &mut Manager<SqliteStore, Registered>,
    conversation_id: &str,
    target_author_uuid: &str,
    target_timestamp: u64,
    pinned: bool,
) -> Result<(), String> {
    use presage::libsignal_service::proto::data_message::{PinMessage, UnpinMessage};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_millis() as u64;
    let thread = parse_thread(conversation_id)?;
    let author_bytes = target_author_uuid
        .parse::<Uuid>()
        .map(|u| u.as_bytes().to_vec())
        .unwrap_or_default();
    let data_message = if pinned {
        DataMessage {
            timestamp: Some(timestamp),
            pin_message: Some(PinMessage {
                target_author_aci_binary: Some(author_bytes),
                target_sent_timestamp: Some(target_timestamp),
                pin_duration: None,
            }),
            ..Default::default()
        }
    } else {
        DataMessage {
            timestamp: Some(timestamp),
            unpin_message: Some(UnpinMessage {
                target_author_aci_binary: Some(author_bytes),
                target_sent_timestamp: Some(target_timestamp),
            }),
            ..Default::default()
        }
    };
    match thread {
        presage::store::Thread::Contact(service_id) => mgr
            .send_message(service_id, data_message, timestamp)
            .await
            .map_err(|e| format!("failed to send pin: {e}")),
        presage::store::Thread::Group(master_key) => mgr
            .send_message_to_group(&master_key, data_message, timestamp)
            .await
            .map_err(|e| format!("failed to send group pin: {e}")),
    }
}

async fn do_send_poll_vote(
    mgr: &mut Manager<SqliteStore, Registered>,
    conversation_id: &str,
    target_author_uuid: &str,
    target_timestamp: u64,
    option_indexes: Vec<u32>,
) -> Result<(), String> {
    use presage::libsignal_service::proto::data_message::PollVote;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_millis() as u64;
    let thread = parse_thread(conversation_id)?;
    let author_bytes = target_author_uuid
        .parse::<Uuid>()
        .map(|u| u.as_bytes().to_vec())
        .unwrap_or_default();
    let vote_count = option_indexes.len() as u32;
    let poll_vote = PollVote {
        target_author_aci_binary: Some(author_bytes),
        target_sent_timestamp: Some(target_timestamp),
        option_indexes,
        vote_count: Some(vote_count),
    };
    let data_message = DataMessage {
        timestamp: Some(timestamp),
        poll_vote: Some(poll_vote),
        ..Default::default()
    };
    match thread {
        presage::store::Thread::Contact(service_id) => mgr
            .send_message(service_id, data_message, timestamp)
            .await
            .map_err(|e| format!("failed to send vote: {e}")),
        presage::store::Thread::Group(master_key) => mgr
            .send_message_to_group(&master_key, data_message, timestamp)
            .await
            .map_err(|e| format!("failed to send group vote: {e}")),
    }
}

async fn do_send_typing(
    mgr: &mut Manager<SqliteStore, Registered>,
    conversation_id: &str,
    started: bool,
) -> Result<(), String> {
    use presage::libsignal_service::proto::typing_message::Action;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_millis() as u64;
    match parse_thread(conversation_id)? {
        presage::store::Thread::Contact(service_id) => {
            let typing = TypingMessage {
                timestamp: Some(timestamp),
                action: Some(if started { Action::Started } else { Action::Stopped } as i32),
                group_id: None,
            };
            mgr.send_message(service_id, typing, timestamp)
                .await
                .map_err(|e| format!("failed to send typing: {e}"))
        }
        presage::store::Thread::Group(_) => Ok(()),
    }
}

async fn do_send_reaction(
    mgr: &mut Manager<SqliteStore, Registered>,
    conversation_id: &str,
    target_author_uuid: &str,
    target_timestamp: u64,
    emoji: &str,
    remove: bool,
) -> Result<(), String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_millis() as u64;
    let thread = parse_thread(conversation_id)?;

    let reaction = Reaction {
        emoji: Some(emoji.to_string()),
        remove: Some(remove),
        target_author_aci: Some(target_author_uuid.to_string()),
        target_sent_timestamp: Some(target_timestamp),
        ..Default::default()
    };
    let data_message = DataMessage {
        timestamp: Some(timestamp),
        reaction: Some(reaction),
        ..Default::default()
    };

    match thread {
        presage::store::Thread::Contact(service_id) => mgr
            .send_message(service_id, data_message, timestamp)
            .await
            .map_err(|e| format!("failed to send reaction: {e}")),
        presage::store::Thread::Group(master_key) => mgr
            .send_message_to_group(&master_key, data_message, timestamp)
            .await
            .map_err(|e| format!("failed to send group reaction: {e}")),
    }
}

async fn do_send(
    mgr: &mut Manager<SqliteStore, Registered>,
    conversation_id: &str,
    body: &str,
    file_paths: &[String],
    quote: Option<crate::messaging::types::QuoteInput>,
) -> Result<(), String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_millis() as u64;

    let thread = parse_thread(conversation_id)?;

    // Upload attachments if any
    let mut attachment_pointers = Vec::new();
    if !file_paths.is_empty() {
        use presage::libsignal_service::sender::AttachmentSpec;

        let mut specs_and_data = Vec::new();
        for path in file_paths {
            let data =
                std::fs::read(path).map_err(|e| format!("failed to read {}: {}", path, e))?;
            let mime = mime_guess::from_path(path)
                .first()
                .map(|m| m.to_string())
                .unwrap_or_else(|| "application/octet-stream".to_string());
            let file_name = std::path::Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string());

            specs_and_data.push((
                AttachmentSpec {
                    content_type: mime,
                    length: data.len(),
                    file_name,
                    preview: None,
                    voice_note: None,
                    borderless: None,
                    width: None,
                    height: None,
                    caption: None,
                    blur_hash: None,
                },
                data,
            ));
        }

        let results = mgr
            .upload_attachments(specs_and_data)
            .await
            .map_err(|e| format!("failed to upload attachments: {}", e))?;

        for result in results {
            match result {
                Ok(ptr) => attachment_pointers.push(ptr),
                Err(e) => return Err(format!("attachment upload failed: {}", e)),
            }
        }
    }

    let body_opt = if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    };

    // Build DataMessage.quote from the reply target, if any.
    let quote_proto = quote.map(|q| {
        use presage::libsignal_service::proto::data_message::Quote;
        Quote {
            id: Some(q.id),
            author_aci: Some(q.author_uuid),
            text: Some(q.text),
            ..Default::default()
        }
    });

    match thread {
        presage::store::Thread::Contact(service_id) => {
            let data_message = DataMessage {
                body: body_opt,
                timestamp: Some(timestamp),
                attachments: attachment_pointers,
                quote: quote_proto,
                ..Default::default()
            };
            mgr.send_message(service_id, data_message, timestamp)
                .await
                .map_err(|e| format!("failed to send: {}", e))?;
        }
        presage::store::Thread::Group(master_key) => {
            let data_message = DataMessage {
                body: body_opt,
                timestamp: Some(timestamp),
                attachments: attachment_pointers,
                quote: quote_proto,
                ..Default::default()
            };
            mgr.send_message_to_group(&master_key, data_message, timestamp)
                .await
                .map_err(|e| format!("failed to send to group: {}", e))?;
        }
    }

    Ok(())
}

async fn download_attachments(
    mgr: &Manager<SqliteStore, Registered>,
    chat_msg: &mut ChatMessage,
    att_dir: &std::path::Path,
) {
    use prost::Message;

    for att in &mut chat_msg.attachments {
        if att.local_path.is_some() || att.pointer_data.is_none() {
            continue;
        }

        let pointer_bytes = att.pointer_data.as_ref().unwrap();
        let pointer = match presage::proto::AttachmentPointer::decode(pointer_bytes.as_slice()) {
            Ok(p) => p,
            Err(e) => {
                warn!("failed to decode attachment pointer: {}", e);
                continue;
            }
        };

        match mgr.get_attachment(&pointer).await {
            Ok(data) => {
                let ext = mime_guess::get_mime_extensions_str(&att.mime_type)
                    .and_then(|exts| exts.first())
                    .unwrap_or(&"bin");
                // SECURITY: att.file_name is sender-controlled (straight off
                // the AttachmentPointer wire) — never let it touch the path.
                // Derive the on-disk name from the server CDN id only, and
                // hard-filter that to an alphanumeric/-/_ allowlist so a
                // crafted CdnKey can't traverse or absolute-path out of
                // att_dir either. file_name stays as display-only metadata
                // on AttachmentInfo for the UI.
                let safe_id: String = att
                    .id
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .take(128)
                    .collect();
                let safe_id = if safe_id.is_empty() {
                    "attachment".to_string()
                } else {
                    safe_id
                };
                let filename = format!("{}.{}", safe_id, ext);
                let path = att_dir.join(&filename);
                // Belt-and-braces: confirm the resolved path is still inside
                // att_dir before writing.
                if path.parent() != Some(att_dir) {
                    error!("attachment path escaped att_dir, refusing: {:?}", path);
                    continue;
                }
                if let Err(e) = std::fs::write(&path, &data) {
                    error!("failed to save attachment: {}", e);
                    continue;
                }
                info!("downloaded attachment: {} ({}B)", filename, data.len());
                att.local_path = Some(path.to_string_lossy().to_string());
            }
            Err(e) => {
                warn!("failed to download attachment {}: {}", att.id, e);
            }
        }
    }
}

/// Write avatar `bytes` to `<dir>/<key>.<ext>` once and return the absolute
/// path. `key` must be filename-safe (uuid or hex group id). Re-uses an
/// existing file (avatars are immutable enough for the session). Returns
/// `None` on any IO error — a missing avatar just falls back to initials.
fn cache_avatar(dir: &std::path::Path, key: &str, content_type: &str, bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let ext = match content_type {
        t if t.contains("png") => "png",
        t if t.contains("webp") => "webp",
        t if t.contains("gif") => "gif",
        _ => "jpg",
    };
    if std::fs::create_dir_all(dir).is_err() {
        return None;
    }
    let path = dir.join(format!("{key}.{ext}"));
    if !path.exists() && std::fs::write(&path, bytes).is_err() {
        return None;
    }
    Some(path.to_string_lossy().into_owned())
}

async fn get_last_message_info(
    store: &SqliteStore,
    thread: &presage::store::Thread,
) -> (Option<String>, u64) {
    match store.messages(thread, ..).await {
        Ok(iter) => {
            let mut last: Option<(String, u64)> = None;
            for msg in iter.flatten() {
                if let ContentBody::DataMessage(dm) = &msg.body {
                    if let Some(body) = &dm.body {
                        let ts = dm.timestamp.unwrap_or(0);
                        if last.as_ref().is_none_or(|(_, prev_ts)| ts > *prev_ts) {
                            last = Some((body.clone(), ts));
                        }
                    }
                }
            }
            match last {
                Some((body, ts)) => (Some(body), ts),
                None => (None, 0),
            }
        }
        Err(_) => (None, 0),
    }
}

/// Async resolution against the presage contact store. "You" for self,
/// otherwise delegates to [`pick_sender_name`].
async fn resolve_sender_name(
    store: &SqliteStore,
    sender_uuid_str: &str,
    self_aci: &Option<String>,
) -> String {
    if Some(sender_uuid_str) == self_aci.as_deref() {
        return "You".to_string();
    }
    let Ok(uuid) = sender_uuid_str.parse::<uuid::Uuid>() else {
        return pick_sender_name(None, sender_uuid_str);
    };
    let service_id = ServiceId::Aci(presage::libsignal_service::protocol::Aci::from(uuid));

    // 1. A saved contact with a name wins.
    if let Ok(Some(contact)) = store.contact_by_id(&service_id).await {
        if !contact.name.is_empty() {
            return contact.name.clone();
        }
    }

    // 2. Fall back to a synced/fetched profile name (covers group members who
    //    aren't saved contacts — the usual "raw UUID in a group" case). Purely
    //    a local store read; no network.
    if let Ok(Some(key)) = store.profile_key(&service_id).await {
        if let Ok(Some(profile)) = store.profile(uuid, key).await {
            if let Some(name) = profile.name {
                let joined = name.to_string();
                if !joined.trim().is_empty() {
                    return joined;
                }
            }
        }
    }

    // 3. Phone number, else a short ~uuid handle.
    match store.contact_by_id(&service_id).await {
        Ok(contact_opt) => pick_sender_name(contact_opt.as_ref(), sender_uuid_str),
        Err(_) => pick_sender_name(None, sender_uuid_str),
    }
}

/// Mutate the message in-place so its sender_name reflects the contact store.
/// Skip outgoing messages — they already say "You".
async fn enrich_sender_name(
    chat_msg: &mut ChatMessage,
    store: &SqliteStore,
    self_aci: &Option<String>,
) {
    if chat_msg.is_outgoing {
        return;
    }
    let resolved = resolve_sender_name(store, &chat_msg.sender_id, self_aci).await;
    chat_msg.sender_name = resolved;
}
