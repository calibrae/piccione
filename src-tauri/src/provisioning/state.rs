use serde::Serialize;

/// Represents the current state of the device provisioning flow.
///
/// State transitions:
/// Idle -> Connecting -> WaitingForScan -> Provisioning -> Registered
///                                                     \-> Error
/// Any state can transition to Error.
/// Idle and Error can transition to Connecting (retry).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ProvisioningState {
    Idle,
    Connecting,
    WaitingForScan {
        qr_code_svg: String,
    },
    Provisioning,
    Registered {
        device_name: String,
    },
    Error {
        message: String,
    },
}

impl ProvisioningState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Registered { .. } | Self::Error { .. })
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Connecting | Self::WaitingForScan { .. } | Self::Provisioning
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_serializes_with_type_tag() {
        let state = ProvisioningState::Idle;
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["type"], "Idle");
    }

    #[test]
    fn waiting_for_scan_includes_qr_data() {
        let state = ProvisioningState::WaitingForScan {
            qr_code_svg: "<svg>test</svg>".to_string(),
        };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["type"], "WaitingForScan");
        assert_eq!(json["qr_code_svg"], "<svg>test</svg>");
    }

    #[test]
    fn error_includes_message() {
        let state = ProvisioningState::Error {
            message: "connection failed".to_string(),
        };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["type"], "Error");
        assert_eq!(json["message"], "connection failed");
    }

    #[test]
    fn registered_includes_device_name() {
        let state = ProvisioningState::Registered {
            device_name: "SignalUI Desktop".to_string(),
        };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["type"], "Registered");
        assert_eq!(json["device_name"], "SignalUI Desktop");
    }

    #[test]
    fn terminal_states() {
        assert!(!ProvisioningState::Idle.is_terminal());
        assert!(!ProvisioningState::Connecting.is_terminal());
        assert!(!ProvisioningState::WaitingForScan {
            qr_code_svg: String::new()
        }
        .is_terminal());
        assert!(!ProvisioningState::Provisioning.is_terminal());
        assert!(ProvisioningState::Registered {
            device_name: String::new()
        }
        .is_terminal());
        assert!(ProvisioningState::Error {
            message: String::new()
        }
        .is_terminal());
    }

    #[test]
    fn active_states() {
        assert!(!ProvisioningState::Idle.is_active());
        assert!(ProvisioningState::Connecting.is_active());
        assert!(ProvisioningState::WaitingForScan {
            qr_code_svg: String::new()
        }
        .is_active());
        assert!(ProvisioningState::Provisioning.is_active());
        assert!(!ProvisioningState::Registered {
            device_name: String::new()
        }
        .is_active());
        assert!(!ProvisioningState::Error {
            message: String::new()
        }
        .is_active());
    }
}
