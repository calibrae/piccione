mod app_state;
mod commands;
mod messaging;
mod provisioning;
mod store;

use app_state::AppState;
use tauri::{Emitter, Manager};
use tracing_subscriber::{fmt, EnvFilter};

fn make_on_message(app: tauri::AppHandle) -> impl Fn(String, messaging::types::ChatMessage) + Send + Sync + 'static {
    move |conv_id, msg| {
        let _ = app.emit("new-message", serde_json::json!({
            "conversation_id": conv_id,
            "message": msg,
        }));
        let _ = app.emit("conversations-updated", ());
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("signalui=info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to resolve app data dir");
            std::fs::create_dir_all(&data_dir)
                .expect("failed to create app data dir");

            let state = AppState::new(data_dir);
            app.manage(state);

            // Try to load registered manager and start messaging
            let app_handle = app.handle().clone();
            std::thread::Builder::new()
                .stack_size(8 * 1024 * 1024)
                .spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("startup runtime");
                    rt.block_on(async move {
                        let local = tokio::task::LocalSet::new();
                        local.run_until(async move {
                            let state = app_handle.state::<AppState>();
                            let passphrase = state.db_passphrase_str();
                            let on_message = make_on_message(app_handle.clone());
                            state.messaging.try_load_and_start(passphrase, on_message).await;
                        }).await;
                    });
                })
                .expect("failed to spawn startup thread");

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::provisioning::start_provisioning,
            commands::provisioning::cancel_provisioning,
            commands::provisioning::get_link_status,
            commands::provisioning::get_provisioning_state,
            commands::messaging::get_conversations,
            commands::messaging::get_messages,
            commands::messaging::send_message,
            commands::messaging::get_self_id,
            commands::messaging::send_to_recipient,
            commands::messaging::send_message_with_attachments,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    #[test]
    fn sanity_check() {
        assert_eq!(2 + 2, 4);
    }
}
