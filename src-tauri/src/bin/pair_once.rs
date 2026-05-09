//! Headless one-shot Signal device pairing.
//!
//! Renders a pairing QR PNG to /tmp/signalui-pair.png, waits up to
//! `PAIR_TIMEOUT_SECS` for the user to scan it, persists the resulting
//! registered identity to the same SqliteStore the Tauri app uses, then
//! **verifies** persistence by re-opening the store with a fresh handle and
//! calling `Manager::load_registered`.
//!
//! Stdout markers (for the harness):
//!   QR_READY <path>    — QR PNG written, ready to forward
//!   PAIR_OK            — device successfully linked AND verified
//!   ERROR <msg>        — anything went wrong (binary also exits non-zero)
//!
//! Behaviour notes:
//! * Multi-thread runtime + `LocalSet` matches presage-cli, which is the
//!   canonical caller of `link_secondary_device`.
//! * Outer watchdog is 300s (vs. the old 120s). presage-cli has none, but a
//!   binary should not hang forever.
//! * After `link_secondary_device` returns Ok, we drop the manager, re-open
//!   the SqliteStore from scratch, and call `Manager::load_registered` to
//!   prove the registration row + identity keys are committed and visible to
//!   the next process. Only then do we print PAIR_OK.

use std::path::PathBuf;
use std::time::Duration;

use presage::libsignal_service::configuration::SignalServers;
use presage::model::identity::OnNewIdentity;
use presage::Manager;
use presage_store_sqlite::SqliteStore;
use qrcode::QrCode;
use signalui_lib::pair_flow::{run_pair, PairOutcome, QrResult};
use signalui_lib::store::keychain::resolve_db_passphrase_for_cli;

const DEVICE_NAME: &str = "signalui-pair-once";
const QR_PATH: &str = "/tmp/signalui-pair.png";
const PAIR_TIMEOUT_SECS: u64 = 300;
const APP_BUNDLE_ID: &str = "com.signalui.app";

fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME not set");
    PathBuf::from(home)
        .join("Library/Application Support")
        .join(APP_BUNDLE_ID)
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

fn fail(msg: impl AsRef<str>) -> ! {
    let m = msg.as_ref();
    println!("ERROR {m}");
    eprintln!("ERROR {m}");
    std::process::exit(1);
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new(
                        "pair_once=info,presage=info,libsignal_service=warn",
                    )
                }),
        )
        .init();

    let data = data_dir();
    if let Err(e) = std::fs::create_dir_all(&data) {
        fail(format!("failed to create data dir {:?}: {}", data, e));
    }

    let passphrase = match resolve_db_passphrase_for_cli(&data) {
        Ok(p) => p,
        Err(e) => fail(format!("failed to load passphrase: {}", e)),
    };
    // Convert from Zeroizing<String> to plain String for cheap clones across
    // the two LocalSet scopes below; the underlying secret only lives in
    // memory while this short-lived process runs.
    let passphrase: String = passphrase.as_str().to_string();

    let db_path = data.join("signalui.db");
    eprintln!("data_dir = {}", data.display());
    eprintln!("db_path  = {}", db_path.display());
    let db_url = format!("sqlite:{}?mode=rwc", db_path.to_string_lossy());

    // Multi-thread runtime so anything internal to presage / libsignal-service
    // that wants to spawn a Send task can — matches presage-cli.
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
    {
        Ok(rt) => rt,
        Err(e) => fail(format!("failed to build tokio runtime: {}", e)),
    };

    let outcome: Result<(), String> = rt.block_on(async move {
        let local = tokio::task::LocalSet::new();
        let passphrase_for_pair = passphrase.clone();
        let db_url_for_pair = db_url.clone();
        let pair_result = local
            .run_until(async move {
                let store = SqliteStore::open_with_passphrase(
                    &db_url_for_pair,
                    Some(&passphrase_for_pair),
                    OnNewIdentity::Trust,
                )
                .await
                .map_err(|e| format!("open store: {}", e))?;

                let result = run_pair(
                    Duration::from_secs(PAIR_TIMEOUT_SECS),
                    |qr_tx| async move {
                        Manager::link_secondary_device(
                            store,
                            SignalServers::Production,
                            DEVICE_NAME.to_string(),
                            qr_tx,
                        )
                        .await
                        .map_err(|e| e.to_string())
                    },
                    |url| {
                        let url_str = url.to_string();
                        eprintln!("provisioning URL received");
                        match render_qr_png(&url_str, QR_PATH) {
                            Ok(()) => {
                                println!("QR_READY {QR_PATH}");
                                QrResult::Rendered
                            }
                            Err(e) => QrResult::Failed(format!("QR render: {e}")),
                        }
                    },
                )
                .await;

                match result {
                    PairOutcome::Success(_mgr) => {
                        // Drop manager (and its store handle) at end of this
                        // scope; verification re-opens fresh.
                        Ok(())
                    }
                    PairOutcome::Timeout(s) => Err(format!("timed out after {s}s")),
                    PairOutcome::Failed(m) => {
                        Err(format!("link_secondary_device: {m}"))
                    }
                }
            })
            .await;

        // Drop the LocalSet (and therefore any lingering store handle) before
        // verifying. SQLite WAL mode commits the identity rows synchronously,
        // but re-opening from scratch is the cheapest assertion that the
        // *next* process will see the registration.
        drop(local);
        pair_result?;

        // Verify persistence: open a fresh store and load the registered
        // manager. This is the same call the Tauri app makes at startup.
        let verify_local = tokio::task::LocalSet::new();
        verify_local
            .run_until(async move {
                let verify_store = SqliteStore::open_with_passphrase(
                    &db_url,
                    Some(&passphrase),
                    OnNewIdentity::Trust,
                )
                .await
                .map_err(|e| format!("verify open store: {}", e))?;

                let mgr = Manager::load_registered(verify_store)
                    .await
                    .map_err(|e| format!("verify load_registered: {}", e))?;

                let aci = mgr
                    .registration_data()
                    .service_ids
                    .aci()
                    .service_id_string();
                eprintln!("verified registration, aci={}", aci);
                Ok::<(), String>(())
            })
            .await
    });

    match outcome {
        Ok(()) => {
            println!("PAIR_OK");
            eprintln!("done");
        }
        Err(e) => fail(e),
    }
}
