//! `is-paired` — quick yes/no for the SignalUI store.
//!
//! Opens the same SQLite store as `pair-once` / the Tauri app and tries to
//! load the registered manager. Useful for CI / status checks.
//!
//! Stdout: `PAIRED` or `NOT_PAIRED`. Exits 0 either way unless the store
//! itself is unreachable, in which case we print `ERROR …` and exit 1.

use std::path::{Path, PathBuf};

use presage::model::identity::OnNewIdentity;
use presage::Manager;
use presage_store_sqlite::SqliteStore;
use rand::RngCore;

const APP_BUNDLE_ID: &str = "com.signalui.app";

fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME not set");
    PathBuf::from(home)
        .join("Library/Application Support")
        .join(APP_BUNDLE_ID)
}

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
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
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
        // No DB at all → definitely not paired. This isn't an error.
        println!("NOT_PAIRED");
        return;
    }
    let db_url = format!("sqlite:{}?mode=rwc", db_path.to_string_lossy());

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1)
        .build()
    {
        Ok(rt) => rt,
        Err(e) => fail(format!("runtime: {e}")),
    };

    let result: Result<bool, String> = rt.block_on(async move {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let store = SqliteStore::open_with_passphrase(
                    &db_url,
                    Some(&passphrase),
                    OnNewIdentity::Trust,
                )
                .await
                .map_err(|e| format!("open store: {e}"))?;

                match Manager::load_registered(store).await {
                    Ok(_) => Ok(true),
                    Err(e) => {
                        // Differentiate "no registration" from a real error.
                        let msg = e.to_string();
                        if msg.contains("not yet registered") || msg.contains("missing key") {
                            Ok(false)
                        } else {
                            Err(format!("load_registered: {msg}"))
                        }
                    }
                }
            })
            .await
    });

    match result {
        Ok(true) => println!("PAIRED"),
        Ok(false) => println!("NOT_PAIRED"),
        Err(e) => fail(e),
    }
}
