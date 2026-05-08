use std::path::PathBuf;

use zeroize::Zeroizing;

use crate::messaging::service::MessagingService;
use crate::provisioning::manager::ProvisioningManager;
use crate::store::keychain;

/// Shared application state managed by Tauri.
pub struct AppState {
    pub provisioning: ProvisioningManager,
    pub messaging: MessagingService,
    pub db_passphrase: Zeroizing<String>,
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
        }
    }

    pub fn db_passphrase_str(&self) -> &str {
        self.db_passphrase.as_str()
    }
}
