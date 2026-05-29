// mimalloc as the global allocator — see Cargo.toml for why.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod app_state;
mod calling;
mod backups;
mod commands;
mod messaging;
pub mod pair_flow;
mod provisioning;
mod settings;
pub mod store;

use app_state::AppState;
use tauri::{Emitter, Manager};
use tracing_subscriber::{fmt, EnvFilter};

use crate::messaging::types::InboundEvent;

/// Build a callback that fans out `InboundEvent`s to Tauri events.
///
/// The frontend listens for one event name per modifier kind; the receive
/// loop is intentionally agnostic about transport, so all the dispatch
/// lives here.
fn make_on_event(app: tauri::AppHandle) -> impl Fn(InboundEvent) + Send + Sync + 'static {
    move |event| match event {
        InboundEvent::Message {
            conversation_id,
            message,
        } => {
            let _ = app.emit(
                "new-message",
                serde_json::json!({
                    "conversation_id": conversation_id,
                    "message": message,
                }),
            );
            let _ = app.emit("conversations-updated", ());
        }
        InboundEvent::Receipt(payload) => {
            let _ = app.emit("read-receipt", payload);
        }
        InboundEvent::Typing(payload) => {
            let _ = app.emit("typing-indicator", payload);
        }
        InboundEvent::Reaction(payload) => {
            let _ = app.emit("reaction", payload);
        }
        InboundEvent::PollVote(payload) => {
            let _ = app.emit("poll-vote", payload);
        }
        InboundEvent::Pin(payload) => {
            let _ = app.emit("pin", payload);
        }
        InboundEvent::Edited(payload) => {
            let _ = app.emit("message-edited", payload);
            // The conversation list summary is derived from the latest
            // DataMessage body — refresh so an edit to the most-recent
            // message is reflected in the sidebar.
            let _ = app.emit("conversations-updated", ());
        }
        InboundEvent::Deleted(payload) => {
            let _ = app.emit("message-deleted", payload);
            let _ = app.emit("conversations-updated", ());
        }
    }
}

/// Backwards-compat alias for the provisioning module which spelt the helper
/// `make_on_message`. Renaming the symbol is a separate swimlane's call.
pub(crate) fn make_on_message(
    app: tauri::AppHandle,
) -> impl Fn(InboundEvent) + Send + Sync + 'static {
    make_on_event(app)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("signalui=info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to resolve app data dir");
            std::fs::create_dir_all(&data_dir).expect("failed to create app data dir");

            let state = AppState::new(data_dir);

            // Spawn the voice-calling subsystem. It needs:
            //  - an event-emit closure (RingRTC callbacks -> Tauri events)
            //  - a send closure (RingRTC outbound signaling -> presage)
            //  - this device's Signal device id, shared as an atomic the
            //    messaging service fills in once the manager loads.
            {
                let emit_handle = app.handle().clone();
                let messaging = state.messaging.clone();
                let controller = crate::calling::manager::CallController::spawn(
                    state.messaging.self_device_id.clone(),
                    move |ev| {
                        let _ = emit_handle.emit("call-event", ev);
                    },
                    move |recipient, call_message| {
                        // Fire-and-forget onto presage's send channel. The
                        // messaging service method is async; hop onto a
                        // throwaway runtime block since RingRTC's callback
                        // thread isn't a tokio context.
                        let messaging = messaging.clone();
                        tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("call-send runtime")
                            .block_on(async move {
                                messaging
                                    .send_call_message(recipient, call_message)
                                    .await;
                            });
                    },
                );
                let _ = state.calling.set(controller.clone());
                // Hand the messaging receive loop the controller so inbound
                // CallMessages get routed to it.
                let messaging = state.messaging.clone();
                std::thread::spawn(move || {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("call-wire runtime")
                        .block_on(async move {
                            messaging.set_call_controller(controller).await;
                        });
                });
            }

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
                        local
                            .run_until(async move {
                                let state = app_handle.state::<AppState>();
                                let passphrase = state.db_passphrase_str();
                                let on_event = make_on_event(app_handle.clone());
                                state
                                    .messaging
                                    .try_load_and_start(passphrase, on_event)
                                    .await;
                                // Unblock get_link_status waiters — startup pass is done
                                // (regardless of whether a registered manager was found).
                                state.startup_complete.notify_waiters();
                            })
                            .await;
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
            commands::messaging::mark_conversation_read,
            commands::messaging::get_conversation_media,
            commands::messaging::save_pasted_image,
            commands::messaging::send_reaction,
            commands::messaging::delete_for_everyone,
            commands::messaging::list_devices,
            commands::messaging::send_typing,
            commands::messaging::search_messages,
            commands::messaging::fetch_profile,
            commands::messaging::set_profile,
            commands::messaging::get_safety_number,
            commands::messaging::vote_poll,
            commands::messaging::set_pin,
            commands::account::sign_out,
            commands::settings::get_settings,
            commands::settings::set_settings,
            commands::calling::start_call,
            commands::calling::accept_call,
            commands::calling::decline_call,
            commands::calling::end_call,
            commands::calling::get_call_state,
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
