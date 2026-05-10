//! `signalui-cli` — unified headless companion to the SignalUI Tauri app.
//!
//! Replaces the three single-purpose bins (`pair-once`, `is-paired`,
//! `list-devices`) with one signed binary. macOS Keychain ACLs are scoped
//! per code-signing identity, so collapsing into one bin means Cali sees a
//! single "Always Allow" prompt for headless access, not three.
//!
//! Subcommands:
//!   pair      — render a QR PNG, wait for scan, persist registration
//!   paired    — print PAIRED / NOT_PAIRED for the local store
//!   devices   — list linked-device slots on the registered Signal account
//!
//! Stdout markers (preserved verbatim from the legacy bins):
//!   QR_READY <path>           (pair)
//!   PAIR_OK                   (pair, success)
//!   PAIRED | NOT_PAIRED       (paired)
//!   DEVICE <id> <ts> [*-]<n>  (devices)
//!   OK <count>                (devices)
//!   ERROR <msg>               (any subcommand failure → exit 1)

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use presage::libsignal_service::configuration::SignalServers;
use presage::model::identity::OnNewIdentity;
use presage::Manager;
use presage_store_sqlite::SqliteStore;
use qrcode::QrCode;
use signalui_lib::pair_flow::{run_pair, PairOutcome, QrResult};
use signalui_lib::store::keychain::resolve_db_passphrase_for_cli;

const APP_BUNDLE_ID: &str = "com.signalui.app";
const PAIR_DEVICE_NAME: &str = "signalui-pair-once";
const PAIR_QR_PATH: &str = "/tmp/signalui-pair.png";
const PAIR_TIMEOUT_SECS: u64 = 300;

#[derive(Parser, Debug)]
#[command(
    name = "signalui-cli",
    about = "Headless companion to SignalUI (pair / paired / devices)",
    version
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Render a Signal pairing QR, wait up to 300 s for a scan, persist registration.
    Pair,
    /// Print PAIRED or NOT_PAIRED based on the local store.
    Paired,
    /// List linked-device slots on the registered Signal account.
    Devices,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new(
                        "signalui_cli=info,presage=info,libsignal_service=warn",
                    )
                }),
        )
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Pair => cmd_pair(),
        Cmd::Paired => cmd_paired(),
        Cmd::Devices => cmd_devices(),
    }
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

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

fn ensure_data_dir() -> PathBuf {
    let data = data_dir();
    if let Err(e) = std::fs::create_dir_all(&data) {
        fail(format!("create data dir {:?}: {e}", data));
    }
    data
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

// ---------------------------------------------------------------------------
// pair
// ---------------------------------------------------------------------------

fn cmd_pair() {
    let data = ensure_data_dir();
    let passphrase = match resolve_db_passphrase_for_cli(&data) {
        Ok(p) => p,
        Err(e) => fail(format!("failed to load passphrase: {e}")),
    };
    let passphrase: String = passphrase.as_str().to_string();

    let db_path = data.join("signalui.db");
    eprintln!("data_dir = {}", data.display());
    eprintln!("db_path  = {}", db_path.display());
    let db_url = format!("sqlite:{}?mode=rwc", db_path.to_string_lossy());

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
    {
        Ok(rt) => rt,
        Err(e) => fail(format!("failed to build tokio runtime: {e}")),
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
                .map_err(|e| format!("open store: {e}"))?;

                let result = run_pair(
                    Duration::from_secs(PAIR_TIMEOUT_SECS),
                    |qr_tx| async move {
                        Manager::link_secondary_device(
                            store,
                            SignalServers::Production,
                            PAIR_DEVICE_NAME.to_string(),
                            qr_tx,
                        )
                        .await
                        .map_err(|e| e.to_string())
                    },
                    |url| {
                        let url_str = url.to_string();
                        eprintln!("provisioning URL received");
                        match render_qr_png(&url_str, PAIR_QR_PATH) {
                            Ok(()) => {
                                println!("QR_READY {PAIR_QR_PATH}");
                                QrResult::Rendered
                            }
                            Err(e) => QrResult::Failed(format!("QR render: {e}")),
                        }
                    },
                )
                .await;

                match result {
                    PairOutcome::Success(_mgr) => Ok(()),
                    PairOutcome::Timeout(s) => Err(format!("timed out after {s}s")),
                    PairOutcome::Failed(m) => Err(format!("link_secondary_device: {m}")),
                }
            })
            .await;

        drop(local);
        pair_result?;

        // Verify by re-opening the store from scratch and reading the
        // registration back. Same call the Tauri app makes at boot.
        let verify_local = tokio::task::LocalSet::new();
        verify_local
            .run_until(async move {
                let verify_store = SqliteStore::open_with_passphrase(
                    &db_url,
                    Some(&passphrase),
                    OnNewIdentity::Trust,
                )
                .await
                .map_err(|e| format!("verify open store: {e}"))?;

                let mgr = Manager::load_registered(verify_store)
                    .await
                    .map_err(|e| format!("verify load_registered: {e}"))?;

                let aci = mgr
                    .registration_data()
                    .service_ids
                    .aci()
                    .service_id_string();
                eprintln!("verified registration, aci={aci}");
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

// ---------------------------------------------------------------------------
// paired
// ---------------------------------------------------------------------------

fn cmd_paired() {
    let data = ensure_data_dir();
    let passphrase = match resolve_db_passphrase_for_cli(&data) {
        Ok(p) => p,
        Err(e) => fail(format!("passphrase: {e}")),
    };
    let db_path = data.join("signalui.db");
    if !db_path.exists() {
        // No DB at all → definitely not paired. Not an error.
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
                    Some(passphrase.as_str()),
                    OnNewIdentity::Trust,
                )
                .await
                .map_err(|e| format!("open store: {e}"))?;

                match Manager::load_registered(store).await {
                    Ok(_) => Ok(true),
                    Err(e) => {
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

// ---------------------------------------------------------------------------
// devices
// ---------------------------------------------------------------------------

fn cmd_devices() {
    let data = ensure_data_dir();
    let passphrase = match resolve_db_passphrase_for_cli(&data) {
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

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        // clap's own sanity check: panics if the derive expansion is malformed.
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_pair_subcommand() {
        let cli = Cli::try_parse_from(["signalui-cli", "pair"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Pair));
    }

    #[test]
    fn parses_paired_subcommand() {
        let cli = Cli::try_parse_from(["signalui-cli", "paired"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Paired));
    }

    #[test]
    fn parses_devices_subcommand() {
        let cli = Cli::try_parse_from(["signalui-cli", "devices"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Devices));
    }

    #[test]
    fn rejects_unknown_subcommand() {
        let err = Cli::try_parse_from(["signalui-cli", "explode"]).unwrap_err();
        assert!(err.to_string().contains("unrecognized") || err.to_string().contains("invalid"));
    }

    #[test]
    fn requires_a_subcommand() {
        // Bare invocation should error rather than default to anything destructive.
        assert!(Cli::try_parse_from(["signalui-cli"]).is_err());
    }
}
