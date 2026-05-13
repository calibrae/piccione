//! Tauri commands for reading and updating user settings.
//!
//! Settings live in `<data_dir>/settings.json`. The in-memory copy in
//! `AppState::settings` is the source of truth at runtime; disk is only
//! written on `set_settings`. All getters / setters here go through the
//! `Mutex` on `AppState`.

use tauri::{AppHandle, Manager};
use tracing::{info, warn};

use crate::app_state::AppState;
use crate::settings::Settings;

#[tauri::command]
pub async fn get_settings(app: AppHandle) -> Result<Settings, String> {
    let state = app.state::<AppState>();
    let s = state.settings.lock().await.clone();
    Ok(s)
}

#[tauri::command]
pub async fn set_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let state = app.state::<AppState>();
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("data dir: {}", e))?;

    // Persist first, then update in-memory. If disk write fails, the
    // running app keeps the old value — preserves the "what you see is
    // what's saved" invariant.
    if let Err(e) = settings.save(&data_dir) {
        warn!("settings save failed: {}", e);
        return Err(format!("save: {}", e));
    }
    info!(
        read_receipts = settings.read_receipts,
        typing_indicators = settings.typing_indicators,
        theme = ?settings.theme,
        "settings updated"
    );

    // Push the read-receipts flag into the messaging service so the
    // receive loop and mark_conversation_read see the change without a
    // restart. AtomicBool keeps the inter-thread sync cheap.
    state
        .messaging
        .read_receipts_enabled
        .store(settings.read_receipts, std::sync::atomic::Ordering::Relaxed);

    *state.settings.lock().await = settings;
    Ok(())
}
