use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct Conversation {
    pub id: String,
    pub name: String,
    pub last_message: Option<String>,
    pub last_timestamp: u64,
    pub is_group: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttachmentInfo {
    pub id: String,
    pub file_name: String,
    pub mime_type: String,
    pub size: u64,
    /// Local file path after download (None if not yet downloaded)
    pub local_path: Option<String>,
    /// Serialized AttachmentPointer for downloading (internal, not displayed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pointer_data: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub timestamp: u64,
    pub sender_id: String,
    pub sender_name: String,
    pub body: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
    pub is_outgoing: bool,
}

/// Request to download an attachment (sent through the send channel)
#[derive(Debug)]
pub struct AttachmentDownloadRequest {
    pub attachment_json: String,
    pub reply: tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>,
}

/// Request to send a message with attachments
#[derive(Debug, Deserialize)]
pub struct SendWithAttachmentsRequest {
    pub conversation_id: String,
    pub body: String,
    pub file_paths: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_serializes() {
        let conv = Conversation {
            id: "abc-123".to_string(),
            name: "Alice".to_string(),
            last_message: Some("Hello".to_string()),
            last_timestamp: 1700000000000,
            is_group: false,
        };
        let json = serde_json::to_value(&conv).unwrap();
        assert_eq!(json["name"], "Alice");
        assert_eq!(json["is_group"], false);
    }

    #[test]
    fn message_serializes() {
        let msg = ChatMessage {
            timestamp: 1700000000000,
            sender_id: "uuid-123".to_string(),
            sender_name: "Bob".to_string(),
            body: Some("Hi there".to_string()),
            attachments: vec![],
            is_outgoing: false,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["body"], "Hi there");
        assert_eq!(json["is_outgoing"], false);
        assert_eq!(json["attachments"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn attachment_info_serializes() {
        let att = AttachmentInfo {
            id: "cdn-123".to_string(),
            file_name: "photo.jpg".to_string(),
            mime_type: "image/jpeg".to_string(),
            size: 12345,
            local_path: Some("/tmp/photo.jpg".to_string()),
            pointer_data: None,
        };
        let json = serde_json::to_value(&att).unwrap();
        assert_eq!(json["mime_type"], "image/jpeg");
        assert_eq!(json["local_path"], "/tmp/photo.jpg");
    }
}
