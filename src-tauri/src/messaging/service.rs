use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use presage::libsignal_service::content::ContentBody;
use presage::libsignal_service::prelude::Content;
use presage::libsignal_service::proto::DataMessage;
use presage::libsignal_service::protocol::ServiceId;
use presage::manager::Registered;
use presage::model::identity::OnNewIdentity;
use presage::store::{ContentsStore, StateStore, Store, Thread};
use presage::Manager;
use presage_store_sqlite::SqliteStore;
use tokio::sync::{mpsc, Mutex, oneshot};
use tracing::{debug, error, info, warn};

use crate::messaging::types::{AttachmentInfo, ChatMessage, Conversation};

struct SendRequest {
    conversation_id: String,
    body: String,
    file_paths: Vec<String>,
    reply: oneshot::Sender<Result<(), String>>,
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
}

impl MessagingService {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            read_store: Arc::new(Mutex::new(None)),
            db_path: Arc::new(db_path),
            db_passphrase: Arc::new(Mutex::new(None)),
            self_aci: Arc::new(Mutex::new(None)),
            send_tx: Arc::new(Mutex::new(None)),
        }
    }

    /// Try to load an existing registered manager and start messaging.
    /// Returns true if successful (device was previously linked).
    pub async fn try_load_and_start<F>(
        &self,
        passphrase: &str,
        on_message: F,
    ) -> bool
    where
        F: Fn(String, ChatMessage) + Send + Sync + 'static,
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
                *self.self_aci.lock().await = Some(aci);
                *self.read_store.lock().await = Some(read_store);
                *self.db_passphrase.lock().await = Some(passphrase.to_string());

                self.start_messaging_thread(mgr, on_message).await;
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
        on_message: F,
    ) where
        F: Fn(String, ChatMessage) + Send + Sync + 'static,
    {
        let aci = mgr
            .registration_data()
            .service_ids
            .aci()
            .service_id_string();
        let read_store = mgr.store().clone();
        info!("starting messaging after provisioning, aci={}", aci);
        *self.self_aci.lock().await = Some(aci);
        *self.read_store.lock().await = Some(read_store);

        self.start_messaging_thread(mgr, on_message).await;
    }

    /// Set up after provisioning on the CURRENT LocalSet (no new thread).
    /// Call this when you're already on a LocalSet-capable thread.
    pub async fn start_after_provisioning_local(
        &self,
        mgr: Manager<SqliteStore, Registered>,
        passphrase: &str,
        on_message: impl Fn(String, ChatMessage) + Send + Sync + 'static,
    ) {
        let aci = mgr
            .registration_data()
            .service_ids
            .aci()
            .service_id_string();
        let read_store = mgr.store().clone();
        info!("starting messaging locally after provisioning, aci={}", aci);
        *self.self_aci.lock().await = Some(aci);
        *self.read_store.lock().await = Some(read_store);
        *self.db_passphrase.lock().await = Some(passphrase.to_string());

        let (send_tx, mut send_rx) = mpsc::unbounded_channel::<SendRequest>();
        *self.send_tx.lock().await = Some(send_tx);

        let self_aci = self.self_aci.clone();
        let on_message = Arc::new(on_message);

        let mut mgr_send = mgr.clone();
        let mgr_download = mgr.clone();
        let mut mgr_recv = mgr;
        let attachments_dir = self.db_path.parent()
            .unwrap_or(std::path::Path::new("/tmp"))
            .join("attachments");
        let _ = std::fs::create_dir_all(&attachments_dir);

        // Request contacts sync
        if let Err(e) = mgr_send.request_contacts().await {
            warn!("failed to request contacts: {}", e);
        }

        // Spawn receive loop locally
        let self_aci_recv = self_aci.clone();
        let on_message_recv = on_message.clone();
        let att_dir = attachments_dir.clone();
        tokio::task::spawn_local(async move {
            loop {
                info!("starting receive loop");
                match mgr_recv.receive_messages().await {
                    Ok(stream) => {
                        let self_aci_val = self_aci_recv.lock().await.clone();
                        futures::pin_mut!(stream);
                        while let Some(received) = stream.next().await {
                            match received {
                                presage::model::messages::Received::QueueEmpty => {
                                    info!("message queue synced");
                                }
                                presage::model::messages::Received::Contacts => {
                                    info!("contacts synced");
                                }
                                presage::model::messages::Received::Content(content) => {
                                    let mut chat_msg = match content_to_chat_message(&content, &self_aci_val) {
                                        Some(m) => m,
                                        None => continue,
                                    };

                                    // Download attachments
                                    download_attachments(&mgr_download, &mut chat_msg, &att_dir).await;

                                    // Resolve sender's display name from the contact store.
                                    enrich_sender_name(&mut chat_msg, mgr_download.store(), &self_aci_val).await;

                                    let conv_id = resolve_conversation_id(&content);
                                    on_message_recv(conv_id, chat_msg);
                                }
                            }
                        }
                        warn!("receive stream ended, reconnecting...");
                    }
                    Err(e) => {
                        error!("receive error: {}, retrying in 5s", e);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });

        // Spawn send handler locally
        tokio::task::spawn_local(async move {
            info!("send handler ready");
            while let Some(req) = send_rx.recv().await {
                info!("processing send to {}", req.conversation_id);
                let result = do_send(&mut mgr_send, &req.conversation_id, &req.body, &req.file_paths).await;
                if let Err(ref e) = result {
                    error!("send failed: {}", e);
                } else {
                    info!("message sent successfully");
                }
                let _ = req.reply.send(result);
            }
        });
    }

    /// Spawn the dedicated messaging thread with receive loop + send handler.
    async fn start_messaging_thread<F>(
        &self,
        mgr: Manager<SqliteStore, Registered>,
        on_message: F,
    ) where
        F: Fn(String, ChatMessage) + Send + Sync + 'static,
    {
        let (send_tx, mut send_rx) = mpsc::unbounded_channel::<SendRequest>();
        *self.send_tx.lock().await = Some(send_tx);

        let self_aci = self.self_aci.clone();
        let on_message = Arc::new(on_message);
        let db_path = self.db_path.clone();

        std::thread::Builder::new()
            .name("signalui-messaging".to_string())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("messaging runtime");

                rt.block_on(async move {
                    let local = tokio::task::LocalSet::new();
                    local.run_until(async move {
                        // Clone for receiving, keep original for sending
                        let mut mgr_send = mgr.clone();
                        let mgr_download = mgr.clone();
                        let mut mgr_recv = mgr;
                        let att_dir = db_path.parent()
                            .unwrap_or(std::path::Path::new("/tmp"))
                            .join("attachments");
                        let _ = std::fs::create_dir_all(&att_dir);

                        // Request contacts sync
                        if let Err(e) = mgr_send.request_contacts().await {
                            warn!("failed to request contacts: {}", e);
                        }

                        // Spawn receive loop as a local task
                        let self_aci_recv = self_aci.clone();
                        let on_message_recv = on_message.clone();
                        tokio::task::spawn_local(async move {
                            loop {
                                info!("starting receive loop");
                                match mgr_recv.receive_messages().await {
                                    Ok(stream) => {
                                        let self_aci_val = self_aci_recv.lock().await.clone();
                                        futures::pin_mut!(stream);

                                        while let Some(received) = stream.next().await {
                                            match received {
                                                presage::model::messages::Received::QueueEmpty => {
                                                    info!("message queue synced");
                                                }
                                                presage::model::messages::Received::Contacts => {
                                                    info!("contacts synced");
                                                }
                                                presage::model::messages::Received::Content(content) => {
                                                    let mut chat_msg = match content_to_chat_message(&content, &self_aci_val) {
                                                        Some(m) => m,
                                                        None => continue,
                                                    };
                                                    download_attachments(&mgr_download, &mut chat_msg, &att_dir).await;
                                                    enrich_sender_name(&mut chat_msg, mgr_download.store(), &self_aci_val).await;
                                                    let conv_id = resolve_conversation_id(&content);
                                                    on_message_recv(conv_id, chat_msg);
                                                }
                                            }
                                        }
                                        warn!("receive stream ended, reconnecting...");
                                    }
                                    Err(e) => {
                                        error!("receive error: {}, retrying in 5s", e);
                                    }
                                }
                                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                            }
                        });

                        // Process sends on this task (same LocalSet, same thread)
                        info!("send handler ready");
                        while let Some(req) = send_rx.recv().await {
                            info!("processing send to {}", req.conversation_id);
                            let result = do_send(&mut mgr_send, &req.conversation_id, &req.body, &req.file_paths).await;
                            if let Err(ref e) = result {
                                error!("send failed: {}", e);
                            } else {
                                info!("message sent successfully");
                            }
                            let _ = req.reply.send(result);
                        }
                    }).await;
                });
            })
            .expect("failed to spawn messaging thread");
    }

    pub async fn self_id(&self) -> Option<String> {
        self.self_aci.lock().await.clone()
    }

    /// Send a text message via the channel to the messaging thread.
    pub async fn send_message(
        &self,
        conversation_id: &str,
        body: &str,
    ) -> Result<(), String> {
        self.send_message_with_attachments(conversation_id, body, vec![]).await
    }

    /// Send a message with optional attachments via the channel.
    pub async fn send_message_with_attachments(
        &self,
        conversation_id: &str,
        body: &str,
        file_paths: Vec<String>,
    ) -> Result<(), String> {
        let tx_guard = self.send_tx.lock().await;
        let tx = tx_guard.as_ref().ok_or("messaging not started")?;

        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SendRequest {
            conversation_id: conversation_id.to_string(),
            body: body.to_string(),
            file_paths,
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

            let service_id = ServiceId::Aci(
                presage::libsignal_service::protocol::Aci::from(contact.uuid),
            );
            let thread = presage::store::Thread::Contact(service_id);
            let (last_message, last_timestamp) = get_last_message_info(&store, &thread).await;

            conversations.push(Conversation {
                id: uuid_str,
                name,
                last_message,
                last_timestamp,
                is_group: false,
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

            conversations.push(Conversation {
                id,
                name: group.title,
                last_message,
                last_timestamp,
                is_group: true,
            });
        }

        conversations.sort_by(|a, b| b.last_timestamp.cmp(&a.last_timestamp));
        Ok(conversations)
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

async fn do_send(
    mgr: &mut Manager<SqliteStore, Registered>,
    conversation_id: &str,
    body: &str,
    file_paths: &[String],
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
            let data = std::fs::read(path)
                .map_err(|e| format!("failed to read {}: {}", path, e))?;
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

    let body_opt = if body.is_empty() { None } else { Some(body.to_string()) };

    match thread {
        presage::store::Thread::Contact(service_id) => {
            let data_message = DataMessage {
                body: body_opt,
                timestamp: Some(timestamp),
                attachments: attachment_pointers,
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
                ..Default::default()
            };
            mgr.send_message_to_group(&master_key, data_message, timestamp)
                .await
                .map_err(|e| format!("failed to send to group: {}", e))?;
        }
    }

    Ok(())
}

/// Determine which conversation an incoming message belongs to.
/// Uses presage's Thread::try_from which handles groups, sync messages, and edits.
fn resolve_conversation_id(content: &Content) -> String {
    match Thread::try_from(content) {
        Ok(Thread::Contact(sid)) => sid.raw_uuid().to_string(),
        Ok(Thread::Group(key)) => hex::encode(key),
        Err(_) => {
            // Fallback: use sender ID
            content.metadata.sender.raw_uuid().to_string()
        }
    }
}

fn parse_thread(conversation_id: &str) -> Result<presage::store::Thread, String> {
    if let Some(service_id) = ServiceId::parse_from_service_id_string(conversation_id) {
        Ok(presage::store::Thread::Contact(service_id))
    } else if let Ok(uuid) = conversation_id.parse::<uuid::Uuid>() {
        Ok(presage::store::Thread::Contact(ServiceId::Aci(
            presage::libsignal_service::protocol::Aci::from(uuid),
        )))
    } else if let Ok(bytes) = hex::decode(conversation_id) {
        if bytes.len() == 32 {
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            Ok(presage::store::Thread::Group(key))
        } else {
            Err(format!("invalid group key length: {}", bytes.len()))
        }
    } else {
        Err(format!("invalid conversation id: {}", conversation_id))
    }
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
                let filename = format!("{}_{}.{}", att.id, att.file_name, ext);
                let path = att_dir.join(&filename);
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

fn extract_attachments(dm: &DataMessage) -> Vec<AttachmentInfo> {
    use prost::Message;

    dm.attachments
        .iter()
        .enumerate()
        .map(|(i, ptr)| {
            let id = match &ptr.attachment_identifier {
                Some(presage::proto::attachment_pointer::AttachmentIdentifier::CdnId(id)) => {
                    id.to_string()
                }
                Some(presage::proto::attachment_pointer::AttachmentIdentifier::CdnKey(key)) => {
                    key.clone()
                }
                None => format!("unknown-{}", i),
            };

            // Serialize the pointer so we can download later
            let mut buf = Vec::new();
            let _ = ptr.encode(&mut buf);

            AttachmentInfo {
                id,
                file_name: ptr.file_name.clone().unwrap_or_else(|| format!("file-{}", i)),
                mime_type: ptr.content_type.clone().unwrap_or_else(|| "application/octet-stream".to_string()),
                size: ptr.size.unwrap_or(0) as u64,
                local_path: None,
                pointer_data: Some(buf),
            }
        })
        .collect()
}

fn content_to_chat_message(content: &Content, self_aci: &Option<String>) -> Option<ChatMessage> {
    let sender_id = content.metadata.sender.raw_uuid().to_string();
    let is_outgoing = Some(&sender_id) == self_aci.as_ref();

    match &content.body {
        ContentBody::DataMessage(dm) => {
            let body = dm.body.clone();
            let attachments = extract_attachments(dm);
            // Skip messages with no text and no attachments
            if body.is_none() && attachments.is_empty() {
                return None;
            }
            Some(ChatMessage {
                timestamp: dm.timestamp.unwrap_or(0),
                sender_id: sender_id.clone(),
                sender_name: sender_id,
                body,
                attachments,
                is_outgoing,
            })
        }
        ContentBody::SynchronizeMessage(sync) => {
            if let Some(sent) = &sync.sent {
                if let Some(dm) = &sent.message {
                    let body = dm.body.clone();
                    let attachments = extract_attachments(dm);
                    if body.is_none() && attachments.is_empty() {
                        return None;
                    }
                    return Some(ChatMessage {
                        timestamp: dm.timestamp.unwrap_or(0),
                        sender_id: self_aci.clone().unwrap_or_default(),
                        sender_name: "You".to_string(),
                        body,
                        attachments,
                        is_outgoing: true,
                    });
                }
            }
            None
        }
        _ => None,
    }
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

/// Pick a human-readable name for a sender given an optional cached contact.
/// Order: profile name → phone number → "~" + first 8 chars of UUID (Signal's
/// "unknown contact" UX). Pure function so it can be unit-tested without a store.
fn pick_sender_name(contact: Option<&presage::model::contacts::Contact>, sender_uuid_str: &str) -> String {
    if let Some(c) = contact {
        if !c.name.is_empty() {
            return c.name.clone();
        }
        if let Some(phone) = &c.phone_number {
            return phone.to_string();
        }
    }
    let prefix: String = sender_uuid_str.chars().take(8).collect();
    format!("~{}", prefix)
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

#[cfg(test)]
mod tests {
    use super::*;
    use presage::libsignal_service::prelude::Uuid;
    use presage::model::contacts::Contact;

    #[test]
    fn parse_thread_uuid() {
        let result = parse_thread("01234567-89ab-cdef-0123-456789abcdef");
        assert!(result.is_ok());
    }

    #[test]
    fn parse_thread_group_hex() {
        let key_hex = "a".repeat(64);
        let result = parse_thread(&key_hex);
        assert!(result.is_ok());
    }

    #[test]
    fn parse_thread_invalid() {
        let result = parse_thread("not-a-valid-id");
        assert!(result.is_err());
    }

    fn make_contact(name: &str, phone: Option<&str>) -> Contact {
        use presage::libsignal_service::prelude::phonenumber::PhoneNumber;
        Contact {
            uuid: Uuid::nil(),
            phone_number: phone.and_then(|p| p.parse::<PhoneNumber>().ok()),
            name: name.to_string(),
            verified: Default::default(),
            profile_key: vec![],
            expire_timer: 0,
            expire_timer_version: 2,
            inbox_position: 0,
            avatar: None,
        }
    }

    #[test]
    fn sender_name_prefers_profile_name() {
        let c = make_contact("Alice", Some("+33600000000"));
        let name = pick_sender_name(Some(&c), "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(name, "Alice");
    }

    #[test]
    fn sender_name_falls_back_to_phone() {
        let c = make_contact("", Some("+33612345678"));
        let name = pick_sender_name(Some(&c), "01234567-89ab-cdef-0123-456789abcdef");
        // PhoneNumber Display formats as "+33 6 12 34 56 78" — accept any non-empty,
        // non-uuid form, and require it contain the country code digits.
        assert!(name.contains("33"), "expected formatted phone, got {:?}", name);
        assert!(!name.starts_with('~'));
    }

    #[test]
    fn sender_name_fallback_to_uuid_prefix_when_no_contact() {
        let name = pick_sender_name(None, "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(name, "~01234567");
    }

    #[test]
    fn sender_name_fallback_when_contact_is_blank() {
        let c = make_contact("", None);
        let name = pick_sender_name(Some(&c), "deadbeef-cafe-1234-5678-9abcdef01234");
        assert_eq!(name, "~deadbeef");
    }

    #[test]
    fn sender_name_short_uuid_does_not_panic() {
        let name = pick_sender_name(None, "abc");
        assert_eq!(name, "~abc");
    }
}
