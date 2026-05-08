// Headless one-shot Signal device pairing.
//
// Writes a pairing QR PNG to /tmp/signalui-pair.png, waits up to 120 s for
// the user to scan it, persists the resulting registered identity to the
// same SqliteStore the Tauri app uses, then exits.
//
// Stdout markers (for the harness):
//   QR_READY <path>    — QR PNG written, ready to forward
//   PAIR_OK            — device successfully linked, identity persisted
//   ERROR <msg>        — anything went wrong (binary also exits non-zero)

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::channel::oneshot;
use futures::future;
use presage::libsignal_service::configuration::SignalServers;
use presage::model::identity::OnNewIdentity;
use presage::Manager;
use presage_store_sqlite::SqliteStore;
use qrcode::QrCode;
use rand::RngCore;

const DEVICE_NAME: &str = "signalui-pair-once";
const QR_PATH: &str = "/tmp/signalui-pair.png";
const TIMEOUT_SECS: u64 = 120;
const APP_BUNDLE_ID: &str = "com.signalui.app";

fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME not set");
    PathBuf::from(home)
        .join("Library/Application Support")
        .join(APP_BUNDLE_ID)
}

/// Same convention as `signalui_lib::store::keychain::get_or_create_db_passphrase`:
/// a 64-hex-char key in `<data_dir>/.db_key`, file mode 0600.
fn get_or_create_db_passphrase(data_dir: &Path) -> std::io::Result<String> {
    let key_file = data_dir.join(".db_key");
    if key_file.exists() {
        let key = std::fs::read_to_string(&key_file)?;
        Ok(key.trim().to_string())
    } else {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let hex = hex::encode(bytes);
        std::fs::write(&key_file, &hex)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                &key_file,
                std::fs::Permissions::from_mode(0o600),
            );
        }
        Ok(hex)
    }
}

fn render_qr_png(url: &str, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let code = QrCode::new(url.as_bytes())?;
    let img = code
        .render::<image::Luma<u8>>()
        .min_dimensions(600, 600)
        .quiet_zone(true)
        .build();
    img.save(path)?;
    Ok(())
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("pair_once=info,presage=info")),
        )
        .init();

    let data = data_dir();
    if let Err(e) = std::fs::create_dir_all(&data) {
        eprintln!("ERROR failed to create data dir {:?}: {}", data, e);
        std::process::exit(1);
    }

    let passphrase = match get_or_create_db_passphrase(&data) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ERROR failed to load passphrase: {}", e);
            std::process::exit(1);
        }
    };

    let db_path = data.join("signalui.db");
    eprintln!("data_dir = {}", data.display());
    eprintln!("db_path  = {}", db_path.display());

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("ERROR failed to build tokio runtime: {}", e);
            std::process::exit(1);
        }
    };

    let outcome: Result<(), String> = rt.block_on(async move {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let db_url = format!("sqlite:{}?mode=rwc", db_path.to_string_lossy());
                let store = SqliteStore::open_with_passphrase(
                    &db_url,
                    Some(&passphrase),
                    OnNewIdentity::Trust,
                )
                .await
                .map_err(|e| format!("open store: {}", e))?;

                let (tx, rx) = oneshot::channel();

                let pair_fut = Manager::link_secondary_device(
                    store,
                    SignalServers::Production,
                    DEVICE_NAME.to_string(),
                    tx,
                );

                let qr_fut = async move {
                    match rx.await {
                        Ok(url) => {
                            let url_str = url.to_string();
                            eprintln!("provisioning URL received");
                            match render_qr_png(&url_str, QR_PATH) {
                                Ok(()) => {
                                    println!("QR_READY {}", QR_PATH);
                                    Ok(())
                                }
                                Err(e) => Err(format!("QR render: {}", e)),
                            }
                        }
                        Err(_) => Err("provisioning URL channel cancelled".to_string()),
                    }
                };

                let joined = tokio::time::timeout(
                    Duration::from_secs(TIMEOUT_SECS),
                    future::join(pair_fut, qr_fut),
                )
                .await
                .map_err(|_| format!("timed out after {}s", TIMEOUT_SECS))?;

                let (mgr_result, qr_result) = joined;
                qr_result?;
                let _mgr = mgr_result.map_err(|e| format!("link_secondary_device: {}", e))?;
                println!("PAIR_OK");
                Ok::<(), String>(())
            })
            .await
    });

    match outcome {
        Ok(()) => {
            eprintln!("done");
        }
        Err(e) => {
            eprintln!("ERROR {}", e);
            std::process::exit(1);
        }
    }
}
