use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProvisioningError {
    #[error("provisioning timed out after {0} seconds")]
    Timeout(u64),

    #[error("provisioning was cancelled")]
    Cancelled,

    #[error("websocket connection failed: {0}")]
    ConnectionFailed(String),

    #[error("failed to generate QR code: {0}")]
    QrGenerationFailed(String),

    #[error("provisioning failed: {0}")]
    ProvisioningFailed(String),

    #[error("store error: {0}")]
    StoreError(String),

    #[error("already provisioning")]
    AlreadyInProgress,
}

impl serde::Serialize for ProvisioningError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_display_correctly() {
        assert_eq!(
            ProvisioningError::Timeout(120).to_string(),
            "provisioning timed out after 120 seconds"
        );
        assert_eq!(
            ProvisioningError::Cancelled.to_string(),
            "provisioning was cancelled"
        );
        assert_eq!(
            ProvisioningError::AlreadyInProgress.to_string(),
            "already provisioning"
        );
    }

    #[test]
    fn errors_serialize_as_string() {
        let err = ProvisioningError::Timeout(120);
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json, "provisioning timed out after 120 seconds");
    }
}
