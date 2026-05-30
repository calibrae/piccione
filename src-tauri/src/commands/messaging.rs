use tauri::{AppHandle, Manager};
use tracing::{error, info};

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
    info!(
        "send_to_recipient called: {} -> {}",
        recipient_id,
        body.len()
    );
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
    quote: Option<crate::messaging::types::QuoteInput>,
    body_ranges: Option<Vec<crate::messaging::types::RangeInput>>,
) -> Result<(), String> {
    info!(
        "send_message_with_attachments: {} files to {}",
        file_paths.len(),
        conversation_id
    );
    let state = app.state::<AppState>();
    let result = state
        .messaging
        .send_message_with_attachments(&conversation_id, &body, file_paths, quote, body_ranges.unwrap_or_default())
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

/// Persist a pasted/dropped image (raw bytes from the WebView clipboard) to a
/// temp file and return its absolute path, so the frontend can feed it into
/// `send_message_with_attachments` exactly like a file-picker selection.
///
/// The WebView can't hand a real filesystem path for clipboard image data, so
/// we round-trip the bytes through the backend. Files land in the app cache
/// dir under `pasted/`; they're transient (the OS clears cache eventually) and
/// presage copies its own attachment on send.
#[tauri::command]
pub async fn save_pasted_image(
    app: AppHandle,
    bytes: Vec<u8>,
    extension: String,
) -> Result<String, String> {
    // Whitelist the extension to a short alphanumeric token — this value ends
    // up in a filename, so don't trust the WebView with path separators.
    let ext: String = extension
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(5)
        .collect::<String>()
        .to_lowercase();
    let ext = if ext.is_empty() { "png".to_string() } else { ext };

    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("no cache dir: {e}"))?
        .join("pasted");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir: {e}"))?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("paste-{ts}.{ext}"));
    std::fs::write(&path, &bytes).map_err(|e| format!("write: {e}"))?;

    Ok(path.to_string_lossy().into_owned())
}

/// Send or remove an emoji reaction to a message.
#[tauri::command]
pub async fn send_reaction(
    app: AppHandle,
    conversation_id: String,
    target_author_uuid: String,
    target_timestamp: u64,
    emoji: String,
    remove: bool,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    state
        .messaging
        .send_reaction(
            &conversation_id,
            &target_author_uuid,
            target_timestamp,
            &emoji,
            remove,
        )
        .await
}

/// Delete-for-everyone a message you previously sent.
#[tauri::command]
pub async fn delete_for_everyone(
    app: AppHandle,
    conversation_id: String,
    target_timestamp: u64,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    state
        .messaging
        .send_delete(&conversation_id, target_timestamp)
        .await
}

/// List the account's linked devices (read-only).
#[tauri::command]
pub async fn list_devices(
    app: AppHandle,
) -> Result<Vec<crate::messaging::types::DeviceDto>, String> {
    let state = app.state::<AppState>();
    state.messaging.list_devices().await
}

/// Send a typing start/stop indicator (1:1). Fire-and-forget.
#[tauri::command]
pub async fn send_typing(
    app: AppHandle,
    conversation_id: String,
    started: bool,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    state.messaging.send_typing(&conversation_id, started).await;
    Ok(())
}

/// Search message bodies across all conversations (read-only).
#[tauri::command]
pub async fn search_messages(
    app: AppHandle,
    query: String,
) -> Result<Vec<crate::messaging::types::SearchHit>, String> {
    let state = app.state::<AppState>();
    state.messaging.search_messages(&query, 200).await
}

/// Fetch + cache a contact's profile; returns the resolved display name.
#[tauri::command]
pub async fn fetch_profile(app: AppHandle, uuid: String) -> Result<Option<String>, String> {
    let parsed = uuid::Uuid::parse_str(&uuid).map_err(|_| "invalid uuid".to_string())?;
    let state = app.state::<AppState>();
    state.messaging.fetch_profile(parsed).await
}

/// Update our own profile display name (+ optional about).
#[tauri::command]
pub async fn set_profile(
    app: AppHandle,
    given_name: String,
    family_name: Option<String>,
    about: Option<String>,
) -> Result<(), String> {
    if given_name.trim().is_empty() {
        return Err("le prénom ne peut pas être vide".into());
    }
    let state = app.state::<AppState>();
    state.messaging.set_profile(given_name, family_name, about).await
}

/// Compute the safety number (identity fingerprint) for a 1:1 contact.
#[tauri::command]
pub async fn get_safety_number(app: AppHandle, uuid: String) -> Result<String, String> {
    let parsed = uuid::Uuid::parse_str(&uuid).map_err(|_| "invalid uuid".to_string())?;
    let state = app.state::<AppState>();
    state.messaging.safety_number(parsed).await
}

/// Cast a vote on a poll message.
#[tauri::command]
pub async fn vote_poll(
    app: AppHandle,
    conversation_id: String,
    target_author_uuid: String,
    target_timestamp: u64,
    option_indexes: Vec<u32>,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    state
        .messaging
        .vote_poll(&conversation_id, &target_author_uuid, target_timestamp, option_indexes)
        .await
}

/// Pin or unpin a message.
#[tauri::command]
pub async fn set_pin(
    app: AppHandle,
    conversation_id: String,
    target_author_uuid: String,
    target_timestamp: u64,
    pinned: bool,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    state
        .messaging
        .set_pin(&conversation_id, &target_author_uuid, target_timestamp, pinned)
        .await
}

/// Whether message-backup / Link & Sync is possible on this device.
#[tauri::command]
pub async fn backup_available(app: AppHandle) -> Result<bool, String> {
    let state = app.state::<AppState>();
    Ok(state.messaging.backup_available().await)
}

/// Decrypt + summarize an encrypted transfer archive before import.
#[cfg(feature = "backups")]
#[tauri::command]
pub async fn preview_backup(
    app: AppHandle,
    path: String,
) -> Result<crate::backups::BackupSummary, String> {
    let state = app.state::<AppState>();
    state.messaging.preview_backup(&path).await
}

/// Stub when the `backups` feature is off (e.g. Windows) so the command is
/// always registered.
#[cfg(not(feature = "backups"))]
#[tauri::command]
pub async fn preview_backup(_app: AppHandle, _path: String) -> Result<(), String> {
    Err("message backups are not available in this build".into())
}

/// Import contacts from an encrypted transfer archive.
#[cfg(feature = "backups")]
#[tauri::command]
pub async fn import_backup(app: AppHandle, path: String) -> Result<usize, String> {
    let state = app.state::<AppState>();
    state.messaging.import_backup(&path).await
}

#[cfg(not(feature = "backups"))]
#[tauri::command]
pub async fn import_backup(_app: AppHandle, _path: String) -> Result<usize, String> {
    Err("message backups are not available in this build".into())
}
