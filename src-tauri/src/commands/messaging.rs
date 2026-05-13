use tauri::{AppHandle, Manager};
use tracing::{info, error};

use crate::app_state::AppState;
use crate::messaging::types::{ChatMessage, Conversation};

#[tauri::command]
pub async fn get_conversations(app: AppHandle) -> Result<Vec<Conversation>, String> {
    info!("get_conversations called");
    let state = app.state::<AppState>();
    let result = state.messaging.get_conversations().await;
    match &result {
        Ok(convos) => info!("returning {} conversations", convos.len()),
        Err(e) => error!("get_conversations error: {}", e),
    }
    result
}

#[tauri::command]
pub async fn get_messages(
    app: AppHandle,
    conversation_id: String,
) -> Result<Vec<ChatMessage>, String> {
    info!("get_messages called for {}", conversation_id);
    let state = app.state::<AppState>();
    state.messaging.get_messages(&conversation_id).await
}

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    conversation_id: String,
    body: String,
) -> Result<(), String> {
    info!("send_message called: {} -> {}", conversation_id, body.len());
    let state = app.state::<AppState>();
    let result = state.messaging.send_message(&conversation_id, &body).await;
    match &result {
        Ok(()) => info!("send_message succeeded"),
        Err(e) => error!("send_message error: {}", e),
    }
    result
}

#[tauri::command]
pub async fn get_self_id(app: AppHandle) -> Result<Option<String>, String> {
    info!("get_self_id called");
    let state = app.state::<AppState>();
    let id = state.messaging.self_id().await;
    info!("self_id: {:?}", id);
    Ok(id)
}

#[tauri::command]
pub async fn send_to_recipient(
    app: AppHandle,
    recipient_id: String,
    body: String,
) -> Result<(), String> {
    info!("send_to_recipient called: {} -> {}", recipient_id, body.len());
    let state = app.state::<AppState>();
    let result = state.messaging.send_message(&recipient_id, &body).await;
    match &result {
        Ok(()) => info!("send_to_recipient succeeded"),
        Err(e) => error!("send_to_recipient error: {}", e),
    }
    result
}

/// Send a message with file attachments.
#[tauri::command]
pub async fn send_message_with_attachments(
    app: AppHandle,
    conversation_id: String,
    body: String,
    file_paths: Vec<String>,
) -> Result<(), String> {
    info!("send_message_with_attachments: {} files to {}", file_paths.len(), conversation_id);
    let state = app.state::<AppState>();
    let result = state
        .messaging
        .send_message_with_attachments(&conversation_id, &body, file_paths)
        .await;
    match &result {
        Ok(()) => info!("send with attachments succeeded"),
        Err(e) => error!("send with attachments error: {}", e),
    }
    result
}

/// Send a READ receipt to the given conversation for the supplied message
/// timestamps. Called by the front-end when the user opens / focuses a
/// conversation so the sender's client can show "read" indicators.
#[tauri::command]
pub async fn mark_conversation_read(
    app: AppHandle,
    conversation_id: String,
    message_timestamps: Vec<String>,
) -> Result<(), String> {
    use uuid::Uuid;
    let state = app.state::<crate::AppState>();

    // Settings gate: if the user has disabled read receipts entirely,
    // silently no-op. The mark-on-open behaviour stays for our own UI
    // (incoming-message timestamps are still tracked locally), but no
    // outbound envelope is sent.
    if !state
        .messaging
        .read_receipts_enabled
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return Ok(());
    }

    // Resolve recipient UUID. Conversations come in as ACI strings for 1:1.
    // Group threads have a hex-encoded master key — receipts there work
    // differently (per-member) and aren't handled by this command yet.
    let recipient = Uuid::parse_str(&conversation_id)
        .map_err(|_| "mark_conversation_read: not a 1:1 conversation".to_string())?;

    let timestamps: Vec<u64> = message_timestamps
        .iter()
        .filter_map(|s| s.parse::<u64>().ok())
        .collect();
    if timestamps.is_empty() {
        return Ok(());
    }

    state
        .messaging
        .send_receipt(
            recipient,
            crate::messaging::types::ReceiptKind::Read,
            timestamps,
        )
        .await
}

/// Return every attachment exchanged in a conversation, newest first.
#[tauri::command]
pub async fn get_conversation_media(
    app: AppHandle,
    conversation_id: String,
) -> Result<Vec<crate::messaging::types::MediaItem>, String> {
    use tracing::info;
    info!("get_conversation_media called for {}", conversation_id);
    let state = app.state::<crate::AppState>();
    state
        .messaging
        .get_conversation_media(&conversation_id)
        .await
}
