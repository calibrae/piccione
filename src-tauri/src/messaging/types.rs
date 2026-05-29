use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct Conversation {
    pub id: String,
    pub name: String,
    pub last_message: Option<String>,
    pub last_timestamp: u64,
    pub is_group: bool,
    /// Absolute path to a cached avatar image, if one is known locally.
    /// The frontend loads it via `convertFileSrc`. `None` → render initials.
    pub avatar_path: Option<String>,
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

/// Single attachment ready for the media-browser grid.
///
/// Distinct from [`ChatMessage`] because the media browser is a flat list
/// across the whole thread — it doesn't carry the message body. Each
/// [`MediaItem`] is one attachment; a chat message with three images
/// produces three media items.
#[derive(Debug, Clone, Serialize)]
pub struct MediaItem {
    pub timestamp: u64,
    pub sender_id: String,
    pub sender_name: String,
    pub is_outgoing: bool,
    pub attachment: AttachmentInfo,
}

/// A reply target as shown in the UI — the snippet of the message being
/// quoted. Built from `DataMessage.quote`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QuotedMessage {
    /// Sent-timestamp of the original message (its stable id).
    pub id: u64,
    pub author_id: String,
    pub author_name: String,
    pub text: String,
}

/// A reply the user is composing — fed back into the send path to populate
/// `DataMessage.quote`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct QuoteInput {
    pub id: u64,
    pub author_uuid: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub timestamp: u64,
    pub sender_id: String,
    pub sender_name: String,
    pub body: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
    pub is_outgoing: bool,
    /// Set when this message replies to another (DataMessage.quote).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<QuotedMessage>,
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

/// Read receipt kind, mirrors the on-wire ReceiptMessage.Type enum.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReceiptKind {
    Delivered,
    Read,
    Viewed,
}

impl ReceiptKind {
    /// Map the proto enum (DELIVERY=0, READ=1, VIEWED=2) to our public form.
    /// Anything we don't recognise is treated as `Delivered`.
    pub fn from_proto(value: i32) -> Self {
        match value {
            1 => ReceiptKind::Read,
            2 => ReceiptKind::Viewed,
            _ => ReceiptKind::Delivered,
        }
    }
}

/// Typing indicator action.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TypingAction {
    Started,
    Stopped,
}

impl TypingAction {
    pub fn from_proto(value: i32) -> Self {
        match value {
            1 => TypingAction::Stopped,
            _ => TypingAction::Started,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReceiptEvent {
    pub chat_id: String,
    pub message_ids: Vec<String>,
    #[serde(rename = "type")]
    pub kind: ReceiptKind,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TypingEvent {
    pub chat_id: String,
    pub sender_id: String,
    pub action: TypingAction,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReactionEvent {
    pub chat_id: String,
    pub target_message_id: String,
    pub emoji: String,
    pub sender_id: String,
    pub remove: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EditEvent {
    pub chat_id: String,
    pub message_id: String,
    pub new_text: String,
    pub edited_at: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeleteEvent {
    pub chat_id: String,
    pub message_id: String,
}

/// Anything the receive loop wants to surface to the Tauri layer.
///
/// One incoming `Content` may produce zero or more events: a `DataMessage`
/// carrying both a body and a reaction is rare in practice, but the modeling
/// keeps that door open and decouples the wire format from the UI bus.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum InboundEvent {
    Message {
        conversation_id: String,
        message: ChatMessage,
    },
    Receipt(ReceiptEvent),
    Typing(TypingEvent),
    Reaction(ReactionEvent),
    Edited(EditEvent),
    Deleted(DeleteEvent),
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
            avatar_path: None,
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
            quote: None,
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

    #[test]
    fn receipt_kind_from_proto() {
        assert_eq!(ReceiptKind::from_proto(0), ReceiptKind::Delivered);
        assert_eq!(ReceiptKind::from_proto(1), ReceiptKind::Read);
        assert_eq!(ReceiptKind::from_proto(2), ReceiptKind::Viewed);
        // Anything outside the spec is conservatively flagged as delivered
        // rather than dropping the receipt entirely.
        assert_eq!(ReceiptKind::from_proto(99), ReceiptKind::Delivered);
    }

    #[test]
    fn typing_action_from_proto() {
        assert_eq!(TypingAction::from_proto(0), TypingAction::Started);
        assert_eq!(TypingAction::from_proto(1), TypingAction::Stopped);
        // Unknown actions default to Started so the UI shows the indicator.
        assert_eq!(TypingAction::from_proto(7), TypingAction::Started);
    }

    #[test]
    fn receipt_event_serializes_with_type_field() {
        let ev = ReceiptEvent {
            chat_id: "uuid-1".to_string(),
            message_ids: vec!["1700000000000".to_string()],
            kind: ReceiptKind::Read,
            timestamp: 1700000000001,
        };
        let json = serde_json::to_value(&ev).unwrap();
        // The frontend protocol calls this field `type`.
        assert_eq!(json["type"], "read");
        assert_eq!(json["chat_id"], "uuid-1");
        assert_eq!(json["message_ids"][0], "1700000000000");
    }

    #[test]
    fn reaction_event_serializes() {
        let ev = ReactionEvent {
            chat_id: "uuid-1".to_string(),
            target_message_id: "1700000000000".to_string(),
            emoji: "🔥".to_string(),
            sender_id: "uuid-2".to_string(),
            remove: false,
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["emoji"], "🔥");
        assert_eq!(json["remove"], false);
    }

    #[test]
    fn typing_event_serializes() {
        let ev = TypingEvent {
            chat_id: "chat-1".to_string(),
            sender_id: "uuid-2".to_string(),
            action: TypingAction::Started,
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["chat_id"], "chat-1");
        assert_eq!(json["sender_id"], "uuid-2");
        assert_eq!(json["action"], "started");
    }

    #[test]
    fn typing_action_serializes_lowercase() {
        let started = serde_json::to_value(TypingAction::Started).unwrap();
        let stopped = serde_json::to_value(TypingAction::Stopped).unwrap();
        assert_eq!(started, "started");
        assert_eq!(stopped, "stopped");
    }

    #[test]
    fn edit_event_serializes() {
        let ev = EditEvent {
            chat_id: "chat-1".to_string(),
            message_id: "1700000000000".to_string(),
            new_text: "fixed typo".to_string(),
            edited_at: 1700000000999,
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["chat_id"], "chat-1");
        assert_eq!(json["message_id"], "1700000000000");
        assert_eq!(json["new_text"], "fixed typo");
        assert_eq!(json["edited_at"], 1700000000999u64);
    }

    #[test]
    fn delete_event_serializes() {
        let ev = DeleteEvent {
            chat_id: "chat-1".to_string(),
            message_id: "1700000000000".to_string(),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["chat_id"], "chat-1");
        assert_eq!(json["message_id"], "1700000000000");
        // Lean payload — no extra fields slip in.
        assert_eq!(json.as_object().unwrap().len(), 2);
    }

    #[test]
    fn receipt_kind_serializes_lowercase() {
        assert_eq!(
            serde_json::to_value(ReceiptKind::Delivered).unwrap(),
            "delivered"
        );
        assert_eq!(serde_json::to_value(ReceiptKind::Read).unwrap(), "read");
        assert_eq!(serde_json::to_value(ReceiptKind::Viewed).unwrap(), "viewed");
    }

    #[test]
    fn inbound_event_message_tag_is_kebab() {
        let ev = InboundEvent::Message {
            conversation_id: "chat-1".to_string(),
            message: ChatMessage {
                timestamp: 1,
                sender_id: "u".to_string(),
                sender_name: "U".to_string(),
                body: Some("hi".to_string()),
                attachments: vec![],
                is_outgoing: false,
                quote: None,
            },
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["kind"], "message");
        assert_eq!(json["conversation_id"], "chat-1");
    }

    #[test]
    fn inbound_event_typing_tag_is_kebab() {
        let ev = InboundEvent::Typing(TypingEvent {
            chat_id: "c".to_string(),
            sender_id: "s".to_string(),
            action: TypingAction::Stopped,
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["kind"], "typing");
    }

    #[test]
    fn inbound_event_edited_tag_is_kebab() {
        let ev = InboundEvent::Edited(EditEvent {
            chat_id: "c".to_string(),
            message_id: "1".to_string(),
            new_text: "x".to_string(),
            edited_at: 2,
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["kind"], "edited");
    }

    #[test]
    fn inbound_event_deleted_tag_is_kebab() {
        let ev = InboundEvent::Deleted(DeleteEvent {
            chat_id: "c".to_string(),
            message_id: "1".to_string(),
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["kind"], "deleted");
    }
}
