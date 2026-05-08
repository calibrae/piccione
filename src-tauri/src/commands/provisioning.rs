use tauri::{AppHandle, Emitter, Manager};

use crate::app_state::AppState;
use crate::provisioning::state::ProvisioningState;

const PROVISIONING_EVENT: &str = "provisioning-state-changed";

#[tauri::command]
pub async fn start_provisioning(
    app: AppHandle,
    device_name: String,
) -> Result<(), String> {
    let app_state = app.state::<AppState>();
    let provisioning = app_state.provisioning.clone();
    let messaging = app_state.messaging.clone();
    let db_passphrase = app_state.db_passphrase.clone();

    let app_handle = app.clone();
    let on_state_change = move |new_state: ProvisioningState| {
        if let Err(e) = app_handle.emit(PROVISIONING_EVENT, &new_state) {
            tracing::error!("failed to emit provisioning event: {}", e);
        }
    };

    let app_for_messaging = app.clone();

    // Everything on one thread + LocalSet (presage is !Send)
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");

            rt.block_on(async move {
                let local = tokio::task::LocalSet::new();
                local
                    .run_until(async move {
                        // Link the device
                        let mgr = match provisioning
                            .start_linking(&device_name, &db_passphrase, on_state_change)
                            .await
                        {
                            Ok(m) => m,
                            Err(e) => {
                                tracing::error!("provisioning failed: {}", e);
                                return;
                            }
                        };

                        // Start messaging on this same LocalSet
                        let on_message = crate::make_on_message(app_for_messaging);
                        messaging
                            .start_after_provisioning_local(mgr, &db_passphrase, on_message)
                            .await;

                        // Keep the LocalSet alive forever (send handler + receive loop)
                        futures::future::pending::<()>().await;
                    })
                    .await;
            });
        })
        .expect("failed to spawn provisioning thread");

    Ok(())
}

#[tauri::command]
pub async fn cancel_provisioning(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    state.provisioning.cancel();
    Ok(())
}

#[tauri::command]
pub async fn get_link_status(app: AppHandle) -> Result<bool, String> {
    let state = app.state::<AppState>();
    Ok(state.messaging.self_id().await.is_some())
}

#[tauri::command]
pub async fn get_provisioning_state(app: AppHandle) -> Result<ProvisioningState, String> {
    let state = app.state::<AppState>();
    Ok(state.provisioning.current_state().await)
}
