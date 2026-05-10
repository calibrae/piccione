use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Notify;
use zeroize::Zeroizing;

use crate::messaging::service::MessagingService;
use crate::provisioning::manager::ProvisioningManager;
use crate::store::keychain;

/// Shared application state managed by Tauri.
pub struct AppState {
    pub provisioning: ProvisioningManager,
    pub messaging: MessagingService,
    pub db_passphrase: Zeroizing<String>,
    /// Notified once the startup thread has finished its `try_load_and_start`
    /// pass — success or failure. Front-end commands like `get_link_status`
    /// await this (with a timeout) to avoid a race where the WebView fires
    /// `get_link_status` before the cold-loaded manager has populated
    /// `self_aci`, and falsely thinks the device is unpaired.
    pub startup_complete: Arc<Notify>,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        let db_path = data_dir.join("signalui.db");

        // Get or create the DB passphrase ONCE at startup
        // If Keychain access fails (e.g. locked keychain), start with empty passphrase
        // and handle it gracefully during provisioning/loading
        let db_passphrase = match keychain::get_or_create_db_passphrase(&data_dir) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("keychain access failed, will retry later: {}", e);
                zeroize::Zeroizing::new(String::new())
            }
        };

        Self {
            provisioning: ProvisioningManager::new(db_path.clone()),
            messaging: MessagingService::new(db_path),
            db_passphrase,
            startup_complete: Arc::new(Notify::new()),
        }
    }

    pub fn db_passphrase_str(&self) -> &str {
        self.db_passphrase.as_str()
    }
}
