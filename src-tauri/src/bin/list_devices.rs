//! `list-devices` — enumerate linked devices on the registered Signal account.
//!
//! Loads the registered manager (must already be paired) and calls
//! `Manager::devices()`. Useful for spotting ghost slots accumulated from
//! failed pairings.
//!
//! Stdout per device: `DEVICE <id> <created_unix_ms> <name>`
//! Trailing line:     `OK <count>`
//!
//! Exits 1 with `ERROR …` if the store can't be opened or the device isn't
//! paired.

use std::path::PathBuf;

use presage::model::identity::OnNewIdentity;
use presage::Manager;
use presage_store_sqlite::SqliteStore;
use signalui_lib::store::keychain::get_or_create_db_passphrase;

const APP_BUNDLE_ID: &str = "com.signalui.app";

fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME not set");
    PathBuf::from(home)
        .join("Library/Application Support")
        .join(APP_BUNDLE_ID)
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
                    tracing_subscriber::EnvFilter::new("list_devices=info,presage=warn")
                }),
        )
        .init();

    let data = data_dir();
    if let Err(e) = std::fs::create_dir_all(&data) {
        fail(format!("create data dir: {e}"));
    }
    let passphrase = match get_or_create_db_passphrase(&data) {
        Ok(p) => p,
        Err(e) => fail(format!("passphrase: {e}")),
    };
    let db_path = data.join("signalui.db");
    if !db_path.exists() {
        fail("not paired (no signalui.db on disk)");
    }
    let db_url = format!("sqlite:{}?mode=rwc", db_path.to_string_lossy());

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
    {
        Ok(rt) => rt,
        Err(e) => fail(format!("runtime: {e}")),
    };

    let result: Result<usize, String> = rt.block_on(async move {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let store = SqliteStore::open_with_passphrase(
                    &db_url,
                    Some(passphrase.as_str()),
                    OnNewIdentity::Trust,
                )
                .await
                .map_err(|e| format!("open store: {e}"))?;

                let manager = Manager::load_registered(store)
                    .await
                    .map_err(|e| format!("load_registered: {e}"))?;

                let devices = manager
                    .devices()
                    .await
                    .map_err(|e| format!("fetch devices: {e}"))?;
                let current_id = manager.device_id();
                let count = devices.len();
                for device in devices {
                    let name = device
                        .name
                        .unwrap_or_else(|| "(no device name)".to_string());
                    let marker = if device.id == current_id { "*" } else { "-" };
                    println!(
                        "DEVICE {} {} {}{}",
                        device.id,
                        device.created_at,
                        marker,
                        name
                    );
                }
                Ok(count)
            })
            .await
    });

    match result {
        Ok(count) => println!("OK {count}"),
        Err(e) => fail(e),
    }
}
