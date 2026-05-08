/// Diagnostic test: opens the actual SignalUI database and dumps what's in it.
/// Run with: cargo test --test store_diagnostic -- --nocapture
use presage::model::identity::OnNewIdentity;
use presage::store::{ContentsStore, StateStore};
use presage_store_sqlite::SqliteStore;

#[tokio::test]
async fn dump_store_contents() {
    // Read the passphrase
    let home = std::env::var("HOME").unwrap();
    let data_dir = std::path::PathBuf::from(home)
        .join("Library/Application Support/com.signalui.app");

    // Try keychain first, then file
    let passphrase = match std::fs::read_to_string(data_dir.join(".db_key")) {
        Ok(key) => key.trim().to_string(),
        Err(_) => {
            // Try keychain
            match security_framework::passwords::get_generic_password(
                "com.signalui.app",
                "signalui-db-encryption-key",
            ) {
                Ok(bytes) => String::from_utf8(bytes.to_vec()).unwrap(),
                Err(e) => {
                    eprintln!("Cannot access passphrase: {}", e);
                    eprintln!("Skipping diagnostic — no access to DB key");
                    return;
                }
            }
        }
    };

    let db_path = data_dir.join("signalui.db");
    if !db_path.exists() {
        eprintln!("No database found at {:?}", db_path);
        return;
    }

    let db_url = format!("sqlite:{}?mode=rwc", db_path.to_string_lossy());
    let store = SqliteStore::open_with_passphrase(&db_url, Some(&passphrase), OnNewIdentity::Trust)
        .await
        .expect("failed to open store");

    // Check registration
    match store.load_registration_data().await {
        Ok(Some(reg)) => {
            println!("=== REGISTRATION ===");
            println!("  ACI: {}", reg.service_ids.aci().service_id_string());
            println!("  Phone: {}", reg.phone_number);
            println!("  Device ID: {:?}", reg.device_id);
        }
        Ok(None) => println!("NOT REGISTERED"),
        Err(e) => println!("Registration error: {}", e),
    }

    // Dump contacts
    println!("\n=== CONTACTS ===");
    match store.contacts().await {
        Ok(contacts) => {
            let mut count = 0;
            for contact in contacts {
                match contact {
                    Ok(c) => {
                        count += 1;
                        println!(
                            "  [{}] {} (phone: {:?}, inbox_pos: {})",
                            c.uuid, c.name, c.phone_number, c.inbox_position
                        );
                    }
                    Err(e) => println!("  ERROR: {}", e),
                }
            }
            println!("  Total: {} contacts", count);
        }
        Err(e) => println!("  Error loading contacts: {}", e),
    }

    // Dump groups
    println!("\n=== GROUPS ===");
    match store.groups().await {
        Ok(groups) => {
            let mut count = 0;
            for group in groups {
                match group {
                    Ok((key, g)) => {
                        count += 1;
                        println!(
                            "  [{}] {} ({} members, rev {})",
                            hex::encode(key),
                            g.title,
                            g.members.len(),
                            g.revision
                        );
                    }
                    Err(e) => println!("  ERROR: {}", e),
                }
            }
            println!("  Total: {} groups", count);
        }
        Err(e) => println!("  Error loading groups: {}", e),
    }

    // Count messages per thread
    println!("\n=== MESSAGES ===");
    // Check Note to Self
    if let Ok(Some(reg)) = store.load_registration_data().await {
        let self_thread = presage::store::Thread::Contact(
            presage::libsignal_service::protocol::ServiceId::Aci(reg.service_ids.aci()),
        );
        match store.messages(&self_thread, ..).await {
            Ok(msgs) => {
                let count = msgs.count();
                println!("  Note to Self: {} messages", count);
            }
            Err(e) => println!("  Note to Self error: {}", e),
        }
    }

    println!("\nDiagnostic complete.");
}
