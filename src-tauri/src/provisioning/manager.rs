use std::path::PathBuf;
use std::sync::Arc;

use futures::channel::oneshot;
use futures::future;
use presage::libsignal_service::configuration::SignalServers;
use presage::model::identity::OnNewIdentity;
use presage::Manager;
use presage_store_sqlite::SqliteStore;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use presage::manager::Registered;

use crate::provisioning::error::ProvisioningError;
use crate::provisioning::qr::generate_qr_svg;
use crate::provisioning::state::ProvisioningState;

const PROVISIONING_TIMEOUT_SECS: u64 = 120;

/// Manages the device provisioning lifecycle.
///
/// Wraps presage's `Manager::link_secondary_device` with state tracking,
/// timeout, and cancellation support.
#[derive(Clone)]
pub struct ProvisioningManager {
    state: Arc<Mutex<ProvisioningState>>,
    cancel_token: CancellationToken,
    db_path: Arc<PathBuf>,
}

impl ProvisioningManager {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            state: Arc::new(Mutex::new(ProvisioningState::Idle)),
            cancel_token: CancellationToken::new(),
            db_path: Arc::new(db_path),
        }
    }

    pub async fn current_state(&self) -> ProvisioningState {
        self.state.lock().await.clone()
    }

    /// Start the device linking flow.
    ///
    /// Calls `on_state_change` every time the provisioning state transitions.
    /// Returns once provisioning completes, fails, or is cancelled.
    pub async fn start_linking<F>(
        &self,
        device_name: &str,
        db_passphrase: &str,
        on_state_change: F,
    ) -> Result<Manager<SqliteStore, Registered>, ProvisioningError>
    where
        F: Fn(ProvisioningState) + Send + Sync + 'static,
    {
        // Prevent concurrent provisioning attempts
        {
            let current = self.state.lock().await;
            if current.is_active() {
                return Err(ProvisioningError::AlreadyInProgress);
            }
        }

        // Create a child token for this attempt so cancel() works fresh each time
        let cancel_token = self.cancel_token.child_token();
        let state = self.state.clone();
        let on_state_change: Arc<dyn Fn(ProvisioningState) + Send + Sync> =
            Arc::new(on_state_change);

        // Transition to Connecting
        Self::set_state(&state, ProvisioningState::Connecting, &on_state_change).await;

        // Open the SQLite store with encryption
        let db_url = format!(
            "sqlite:{}?mode=rwc",
            self.db_path.to_string_lossy()
        );
        let store = SqliteStore::open_with_passphrase(
            &db_url,
            Some(db_passphrase),
            OnNewIdentity::Trust,
        )
        .await
        .map_err(|e| ProvisioningError::StoreError(e.to_string()))?;

        let device_name = device_name.to_string();

        // Create the provisioning URL channel
        let (provisioning_link_tx, provisioning_link_rx) = oneshot::channel();

        // Run provisioning with timeout and cancellation
        let link_future = async {
            let (manager_result, _) = future::join(
                Manager::link_secondary_device(
                    store,
                    SignalServers::Production,
                    device_name.clone(),
                    provisioning_link_tx,
                ),
                async {
                    match provisioning_link_rx.await {
                        Ok(url) => {
                            let url_str = url.to_string();
                            info!("provisioning URL received, generating QR code");

                            match generate_qr_svg(&url_str) {
                                Ok(svg) => {
                                    Self::set_state(
                                        &state,
                                        ProvisioningState::WaitingForScan {
                                            qr_code_svg: svg,
                                        },
                                        &on_state_change,
                                    )
                                    .await;
                                }
                                Err(e) => {
                                    error!("failed to generate QR code: {}", e);
                                    Self::set_state(
                                        &state,
                                        ProvisioningState::Error {
                                            message: e.to_string(),
                                        },
                                        &on_state_change,
                                    )
                                    .await;
                                }
                            }
                        }
                        Err(_) => {
                            warn!("provisioning link channel cancelled");
                        }
                    }
                },
            )
            .await;

            // If we get here, the user scanned and provisioning completed
            Self::set_state(
                &state,
                ProvisioningState::Provisioning,
                &on_state_change,
            )
            .await;

            match manager_result {
                Ok(manager) => {
                    info!("device linked successfully");
                    Self::set_state(
                        &state,
                        ProvisioningState::Registered {
                            device_name: device_name.clone(),
                        },
                        &on_state_change,
                    )
                    .await;
                    Ok(manager)
                }
                Err(e) => {
                    let msg = format!("{}", e);
                    error!("provisioning failed: {}", msg);
                    Self::set_state(
                        &state,
                        ProvisioningState::Error {
                            message: msg.clone(),
                        },
                        &on_state_change,
                    )
                    .await;
                    Err(ProvisioningError::ProvisioningFailed(msg))
                }
            }
        };

        // Race: provisioning vs timeout vs cancellation
        tokio::select! {
            result = link_future => result,
            _ = tokio::time::sleep(Duration::from_secs(PROVISIONING_TIMEOUT_SECS)) => {
                let state = self.state.clone();
                Self::set_state(
                    &state,
                    ProvisioningState::Error {
                        message: format!("timed out after {}s", PROVISIONING_TIMEOUT_SECS),
                    },
                    &on_state_change,
                ).await;
                Err(ProvisioningError::Timeout(PROVISIONING_TIMEOUT_SECS))
            }
            _ = cancel_token.cancelled() => {
                let state = self.state.clone();
                Self::set_state(
                    &state,
                    ProvisioningState::Error {
                        message: "cancelled by user".to_string(),
                    },
                    &on_state_change,
                ).await;
                Err(ProvisioningError::Cancelled)
            }
        }
    }

    /// Cancel an in-progress provisioning attempt.
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// Check if the device has been successfully linked.
    pub async fn is_linked(&self) -> bool {
        matches!(*self.state.lock().await, ProvisioningState::Registered { .. })
    }

    async fn set_state(
        state: &Arc<Mutex<ProvisioningState>>,
        new_state: ProvisioningState,
        on_change: &Arc<dyn Fn(ProvisioningState) + Send + Sync>,
    ) {
        let mut current = state.lock().await;
        *current = new_state.clone();
        on_change(new_state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn new_manager_starts_idle() {
        let mgr = ProvisioningManager::new(PathBuf::from("/tmp/test-signalui.db"));
        let state = mgr.current_state().await;
        assert!(matches!(state, ProvisioningState::Idle));
    }

    #[tokio::test]
    async fn cancel_sets_token() {
        let mgr = ProvisioningManager::new(PathBuf::from("/tmp/test-signalui.db"));
        assert!(!mgr.cancel_token.is_cancelled());
        mgr.cancel();
        assert!(mgr.cancel_token.is_cancelled());
    }

    #[tokio::test]
    async fn not_linked_initially() {
        let mgr = ProvisioningManager::new(PathBuf::from("/tmp/test-signalui.db"));
        assert!(!mgr.is_linked().await);
    }

    #[tokio::test]
    async fn set_state_calls_callback() {
        let state = Arc::new(Mutex::new(ProvisioningState::Idle));
        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = count.clone();

        let callback: Arc<dyn Fn(ProvisioningState) + Send + Sync> =
            Arc::new(move |_| {
                count_clone.fetch_add(1, Ordering::SeqCst);
            });

        ProvisioningManager::set_state(
            &state,
            ProvisioningState::Connecting,
            &callback,
        )
        .await;

        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(matches!(*state.lock().await, ProvisioningState::Connecting));
    }
}
