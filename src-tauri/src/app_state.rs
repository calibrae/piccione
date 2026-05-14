use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};
use zeroize::Zeroizing;

use crate::messaging::service::MessagingService;
use crate::provisioning::manager::ProvisioningManager;
use crate::settings::Settings;
use crate::store::keychain;

/// Shared application state managed by Tauri.
pub struct AppState {
    pub provisioning: ProvisioningManager,
    pub messaging: MessagingService,
    pub db_passphrase: Zeroizing<String>,
    /// User-facing app settings (read receipts, theme, …). Loaded from
    /// `<data_dir>/settings.json` at startup; mutated via the
    /// `set_settings` Tauri command which also writes back to disk.
    pub settings: Arc<Mutex<Settings>>,
    /// Notified once the startup thread has finished its `try_load_and_start`
    /// pass — success or failure. Front-end commands like `get_link_status`
    /// await this (with a timeout) to avoid a race where the WebView fires
    /// `get_link_status` before the cold-loaded manager has populated
    /// `self_aci`, and falsely thinks the device is unpaired.
    pub startup_complete: Arc<Notify>,
    /// The voice-calling subsystem. Spawned in lib.rs `setup` once the Tauri
    /// AppHandle exists (it needs an event-emit closure). `OnceLock` because
    /// it's written exactly once and read by the calling Tauri commands.
    pub calling: Arc<std::sync::OnceLock<crate::calling::manager::CallController>>,
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

        let settings = Settings::load(&data_dir);

        let messaging = MessagingService::new(db_path.clone());
        // Sync the persisted read-receipts toggle into the runtime atomic
        // so the receive loop sees the right value before any envelopes arrive.
        messaging
            .read_receipts_enabled
            .store(settings.read_receipts, std::sync::atomic::Ordering::Relaxed);

        Self {
            provisioning: ProvisioningManager::new(db_path),
            messaging,
            db_passphrase,
            startup_complete: Arc::new(Notify::new()),
            settings: Arc::new(Mutex::new(settings)),
            calling: Arc::new(std::sync::OnceLock::new()),
        }
    }

    pub fn db_passphrase_str(&self) -> &str {
        self.db_passphrase.as_str()
    }
}
