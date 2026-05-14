//! Tauri commands for the voice-calling subsystem.
//!
//! All of these are thin: they forward to the `CallController` (which owns
//! RingRTC on its dedicated thread). The controller is `OnceLock`-set in
//! lib.rs `setup`; if a command fires before that (shouldn't happen — the
//! UI only shows call affordances once linked) it returns a clear error.

use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::app_state::AppState;
use crate::calling::CallState;

fn controller(
    app: &AppHandle,
) -> Result<crate::calling::manager::CallController, String> {
    // CallController is a cheap Clone (an mpsc Sender + an Arc<Mutex<…>>),
    // so hand back an owned clone rather than wrestle the State guard's
    // lifetime.
    app.state::<AppState>()
        .calling
        .get()
        .cloned()
        .ok_or_else(|| "calling subsystem not ready".to_string())
}

/// Place an outgoing 1:1 audio call to a contact.
#[tauri::command]
pub async fn start_call(
    app: AppHandle,
    recipient_id: String,
    recipient_name: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&recipient_id)
        .map_err(|_| format!("not a valid recipient uuid: {recipient_id}"))?;
    controller(&app)?.start_call(uuid, recipient_name);
    Ok(())
}

/// Accept the ringing incoming call.
#[tauri::command]
pub async fn accept_call(app: AppHandle) -> Result<(), String> {
    controller(&app)?.accept();
    Ok(())
}

/// Decline the ringing incoming call.
#[tauri::command]
pub async fn decline_call(app: AppHandle) -> Result<(), String> {
    controller(&app)?.decline();
    Ok(())
}

/// End the active call (or cancel an outgoing dial).
#[tauri::command]
pub async fn end_call(app: AppHandle) -> Result<(), String> {
    controller(&app)?.hangup();
    Ok(())
}

/// Current coarse call state — for the frontend to poll on load / recover.
#[tauri::command]
pub async fn get_call_state(app: AppHandle) -> Result<CallState, String> {
    Ok(controller(&app)?.state())
}
