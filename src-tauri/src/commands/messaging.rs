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
