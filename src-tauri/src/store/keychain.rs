use security_framework::base::Error as SfError;
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use zeroize::Zeroizing;

const SERVICE_NAME: &str = "com.signalui.app";
const DB_KEY_ACCOUNT: &str = "signalui-db-encryption-key";

// macOS Security framework error code for "item not found"
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// Retrieve or generate the database encryption passphrase.
///
/// Uses a file-based key stored alongside the database.
/// File permissions are set to 0600 (owner-only).
/// For production builds, this should be migrated to Keychain.
pub fn get_or_create_db_passphrase(data_dir: &std::path::Path) -> Result<Zeroizing<String>, KeychainError> {
    let key_file = data_dir.join(".db_key");

    if key_file.exists() {
        let key = std::fs::read_to_string(&key_file)
            .map_err(|e| KeychainError::AccessFailed(format!("failed to read key file: {}", e)))?;
        tracing::debug!("loaded database encryption key from file");
        Ok(Zeroizing::new(key.trim().to_string()))
    } else {
        let passphrase = generate_passphrase();
        std::fs::write(&key_file, passphrase.as_bytes())
            .map_err(|e| KeychainError::StoreFailed(format!("failed to write key file: {}", e)))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600));
        }
        tracing::info!("created database encryption key in file");
        Ok(passphrase)
    }
}

/// Delete the database encryption key from the Keychain.
pub fn delete_db_passphrase() -> Result<(), KeychainError> {
    delete_generic_password(SERVICE_NAME, DB_KEY_ACCOUNT)
        .map_err(|e| KeychainError::DeleteFailed(e.to_string()))
}

fn generate_passphrase() -> Zeroizing<String> {
    use rand::RngCore;
    let mut key_bytes = Zeroizing::new([0u8; 32]);
    rand::thread_rng().fill_bytes(key_bytes.as_mut());
    let hex = hex::encode(key_bytes.as_ref());
    Zeroizing::new(hex)
}

#[derive(Debug, thiserror::Error)]
pub enum KeychainError {
    #[error("invalid data in keychain")]
    InvalidData,

    #[error("failed to store in keychain: {0}")]
    StoreFailed(String),

    #[error("failed to delete from keychain: {0}")]
    DeleteFailed(String),

    #[error("keychain access failed: {0}")]
    AccessFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_passphrase_is_64_hex_chars() {
        let pass = generate_passphrase();
        assert_eq!(pass.len(), 64);
        assert!(pass.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn generate_passphrase_is_random() {
        let p1 = generate_passphrase();
        let p2 = generate_passphrase();
        assert_ne!(*p1, *p2);
    }
}
