//! Pure, side-effect-free parsing of Signal `Content` envelopes into the
//! UI-bound event + view-model types. Everything here is synchronous and
//! deterministic — no network, no store, no `Manager`. That keeps it all
//! unit-testable in isolation (see the `tests` module at the bottom) and is
//! why it lives in its own file away from the async machinery in
//! `service.rs`.

use presage::libsignal_service::content::ContentBody;
use presage::libsignal_service::prelude::Content;
use presage::libsignal_service::proto::{
    data_message::{Delete, Reaction},
    sync_message, DataMessage, EditMessage, ReceiptMessage, SyncMessage, TypingMessage,
};
use presage::libsignal_service::protocol::ServiceId;
use presage::store::Thread;

use crate::messaging::types::{
    AttachmentInfo, ChatMessage, DeleteEvent, EditEvent, InboundEvent, ReactionEvent, ReceiptEvent,
    ReceiptKind, TypingAction, TypingEvent,
};

/// Determine which conversation an incoming message belongs to.
/// Uses presage's Thread::try_from which handles groups, sync messages, and edits.
fn resolve_conversation_id(content: &Content) -> String {
    match Thread::try_from(content) {
        Ok(Thread::Contact(sid)) => sid.raw_uuid().to_string(),
        Ok(Thread::Group(key)) => hex::encode(key),
        Err(_) => {
            // Fallback: use sender ID
            content.metadata.sender.raw_uuid().to_string()
        }
    }
}

pub(crate) fn parse_thread(conversation_id: &str) -> Result<presage::store::Thread, String> {
    if let Some(service_id) = ServiceId::parse_from_service_id_string(conversation_id) {
        Ok(presage::store::Thread::Contact(service_id))
    } else if let Ok(uuid) = conversation_id.parse::<uuid::Uuid>() {
        Ok(presage::store::Thread::Contact(ServiceId::Aci(
            presage::libsignal_service::protocol::Aci::from(uuid),
        )))
    } else if let Ok(bytes) = hex::decode(conversation_id) {
        if bytes.len() == 32 {
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            Ok(presage::store::Thread::Group(key))
        } else {
            Err(format!("invalid group key length: {}", bytes.len()))
        }
    } else {
        Err(format!("invalid conversation id: {}", conversation_id))
    }
}

pub(crate) fn extract_attachments(dm: &DataMessage) -> Vec<AttachmentInfo> {
    use prost::Message;

    dm.attachments
        .iter()
        .enumerate()
        .map(|(i, ptr)| {
            let id = match &ptr.attachment_identifier {
                Some(presage::proto::attachment_pointer::AttachmentIdentifier::CdnId(id)) => {
                    id.to_string()
                }
                Some(presage::proto::attachment_pointer::AttachmentIdentifier::CdnKey(key)) => {
                    key.clone()
                }
                None => format!("unknown-{}", i),
            };

            // Serialize the pointer so we can download later
            let mut buf = Vec::new();
            let _ = ptr.encode(&mut buf);

            AttachmentInfo {
                id,
                file_name: ptr
                    .file_name
                    .clone()
                    .unwrap_or_else(|| format!("file-{}", i)),
                mime_type: ptr
                    .content_type
                    .clone()
                    .unwrap_or_else(|| "application/octet-stream".to_string()),
                size: ptr.size.unwrap_or(0) as u64,
                local_path: None,
                pointer_data: Some(buf),
            }
        })
        .collect()
}

/// Build a UI `QuotedMessage` from a `DataMessage.quote`. Author name is the
/// raw uuid here; `enrich_sender_name`-style resolution can upgrade it later.
fn extract_quote(dm: &DataMessage) -> Option<crate::messaging::types::QuotedMessage> {
    let q = dm.quote.as_ref()?;
    let author_id = q.author_aci.clone().unwrap_or_default();
    Some(crate::messaging::types::QuotedMessage {
        id: q.id.unwrap_or(0),
        author_id: author_id.clone(),
        author_name: author_id,
        text: q.text.clone().unwrap_or_default(),
    })
}

/// Build UI `LinkPreview`s from `DataMessage.preview`. Image previews are
/// dropped for now; url/title/description render a card.
fn extract_previews(dm: &DataMessage) -> Vec<crate::messaging::types::LinkPreview> {
    dm.preview
        .iter()
        .filter_map(|p| {
            let url = p.url.clone()?;
            Some(crate::messaging::types::LinkPreview {
                url,
                title: p.title.clone().unwrap_or_default(),
                description: p.description.clone().unwrap_or_default(),
            })
        })
        .collect()
}

/// Map `DataMessage.bodyRanges` to UI `MsgRange`s (styles + mentions).
fn extract_body_ranges(dm: &DataMessage) -> Vec<crate::messaging::types::MsgRange> {
    use presage::libsignal_service::proto::body_range::{AssociatedValue, Style};
    dm.body_ranges
        .iter()
        .filter_map(|r| {
            let start = r.start?;
            let length = r.length?;
            let (style, mention_uuid) = match &r.associated_value {
                Some(AssociatedValue::Style(s)) => {
                    let name = match Style::try_from(*s).unwrap_or(Style::None) {
                        Style::Bold => "bold",
                        Style::Italic => "italic",
                        Style::Spoiler => "spoiler",
                        Style::Strikethrough => "strikethrough",
                        Style::Monospace => "monospace",
                        Style::None => return None,
                    };
                    (Some(name.to_string()), None)
                }
                Some(AssociatedValue::MentionAci(aci)) => (None, Some(aci.clone())),
                _ => return None,
            };
            Some(crate::messaging::types::MsgRange { start, length, style, mention_uuid })
        })
        .collect()
}

/// A non-text system event carried by a DataMessage (e.g. group call).
fn system_event(dm: &DataMessage) -> Option<String> {
    if dm.group_call_update.is_some() {
        return Some("group-call".to_string());
    }
    None
}

/// Extract a poll from DataMessage.pollCreate.
fn extract_poll(dm: &DataMessage) -> Option<crate::messaging::types::PollInfo> {
    let pc = dm.poll_create.as_ref()?;
    Some(crate::messaging::types::PollInfo {
        question: pc.question.clone().unwrap_or_default(),
        options: pc.options.clone(),
        allow_multiple: pc.allow_multiple.unwrap_or(false),
    })
}

pub(crate) fn content_to_chat_message(
    content: &Content,
    self_aci: &Option<String>,
) -> Option<ChatMessage> {
    let sender_id = content.metadata.sender.raw_uuid().to_string();
    let is_outgoing = Some(&sender_id) == self_aci.as_ref();

    match &content.body {
        ContentBody::DataMessage(dm) => {
            // Reaction / delete-only DataMessages are NOT chat messages.
            // They get fanned out as modifier events instead — keep them out
            // of the message list to avoid empty bubbles in the UI.
            if dm.reaction.is_some() || dm.delete.is_some() {
                return None;
            }
            let body = dm.body.clone();
            let attachments = extract_attachments(dm);
            let poll = extract_poll(dm);
            let system = system_event(dm);
            // Skip messages with no text, attachments, poll, or system event.
            if body.is_none() && attachments.is_empty() && poll.is_none() && system.is_none() {
                return None;
            }
            Some(ChatMessage {
                timestamp: dm.timestamp.unwrap_or(0),
                sender_id: sender_id.clone(),
                sender_name: sender_id,
                body,
                attachments,
                is_outgoing,
                quote: extract_quote(dm),
                previews: extract_previews(dm),
                body_ranges: extract_body_ranges(dm),
                poll: extract_poll(dm),
                system_event: system_event(dm),
            })
        }
        ContentBody::SynchronizeMessage(sync) => {
            if let Some(sent) = &sync.sent {
                if let Some(dm) = &sent.message {
                    if dm.reaction.is_some() || dm.delete.is_some() {
                        return None;
                    }
                    let body = dm.body.clone();
                    let attachments = extract_attachments(dm);
                    let poll = extract_poll(dm);
                    let system = system_event(dm);
                    if body.is_none() && attachments.is_empty() && poll.is_none() && system.is_none() {
                        return None;
                    }
                    return Some(ChatMessage {
                        timestamp: dm.timestamp.unwrap_or(0),
                        sender_id: self_aci.clone().unwrap_or_default(),
                        sender_name: "You".to_string(),
                        body,
                        attachments,
                        is_outgoing: true,
                        quote: extract_quote(dm),
                        previews: extract_previews(dm),
                        body_ranges: extract_body_ranges(dm),
                        poll: extract_poll(dm),
                        system_event: system_event(dm),
                    });
                }
            }
            None
        }
        _ => None,
    }
}

/// Best-effort group-id resolution for a `TypingMessage`.
/// `TypingMessage` carries its own `group_id` (NOT a master key — it's the
/// derived group ID). We surface it as hex so the frontend has a stable
/// identifier; if it's missing we fall back to the sender (1:1 chat).
fn typing_chat_id(tm: &TypingMessage, sender_uuid: &str) -> String {
    match tm.group_id.as_ref() {
        Some(gid) if !gid.is_empty() => hex::encode(gid),
        _ => sender_uuid.to_string(),
    }
}

/// Pure, fully-testable extraction of UI-bound events from a single `Content`.
///
/// One `Content` may yield multiple events (e.g. a `DataMessage` with both a
/// body AND a reaction is theoretically valid wire-format). The receive loop
/// is responsible for downloading attachments only on `Message` events.
pub fn derive_inbound_events(content: &Content, self_aci: &Option<String>) -> Vec<InboundEvent> {
    let sender_uuid = content.metadata.sender.raw_uuid().to_string();
    let chat_id = resolve_conversation_id(content);
    let mut out: Vec<InboundEvent> = Vec::new();

    match &content.body {
        ContentBody::DataMessage(dm) => {
            // Reactions, deletes, and bodies can in principle co-exist; emit
            // every modifier we find, then fall through to the chat message
            // factory which already de-dupes empty payloads.
            push_data_message_modifiers(&mut out, &chat_id, &sender_uuid, dm);
            if let Some(msg) = content_to_chat_message(content, self_aci) {
                out.push(InboundEvent::Message {
                    conversation_id: chat_id.clone(),
                    message: msg,
                });
            }
        }
        ContentBody::SynchronizeMessage(SyncMessage {
            sent: Some(sync_message::Sent {
                message: Some(dm), ..
            }),
            ..
        }) => {
            // For sync-from-other-device messages the *sender* of the modifier
            // is us (self_aci). The frontend renders that distinction.
            let actor = self_aci.clone().unwrap_or_else(|| sender_uuid.clone());
            push_data_message_modifiers(&mut out, &chat_id, &actor, dm);
            if let Some(msg) = content_to_chat_message(content, self_aci) {
                out.push(InboundEvent::Message {
                    conversation_id: chat_id.clone(),
                    message: msg,
                });
            }
        }
        ContentBody::SynchronizeMessage(SyncMessage {
            sent:
                Some(sync_message::Sent {
                    edit_message:
                        Some(EditMessage {
                            target_sent_timestamp: Some(ts),
                            data_message: Some(dm),
                        }),
                    ..
                }),
            ..
        }) => {
            // Edit issued by us from another linked device.
            if let Some(ev) = build_edit_event(&chat_id, *ts, dm) {
                out.push(InboundEvent::Edited(ev));
            }
        }
        ContentBody::EditMessage(EditMessage {
            target_sent_timestamp: Some(ts),
            data_message: Some(dm),
        }) => {
            if let Some(ev) = build_edit_event(&chat_id, *ts, dm) {
                out.push(InboundEvent::Edited(ev));
            }
        }
        ContentBody::ReceiptMessage(ReceiptMessage { r#type, timestamp }) => {
            if !timestamp.is_empty() {
                let kind = ReceiptKind::from_proto(r#type.unwrap_or(0));
                out.push(InboundEvent::Receipt(ReceiptEvent {
                    chat_id,
                    message_ids: timestamp.iter().map(|t| t.to_string()).collect(),
                    kind,
                    timestamp: content.metadata.timestamp,
                }));
            }
        }
        ContentBody::TypingMessage(tm) => {
            let chat_id = typing_chat_id(tm, &sender_uuid);
            let action = TypingAction::from_proto(tm.action.unwrap_or(0));
            out.push(InboundEvent::Typing(TypingEvent {
                chat_id,
                sender_id: sender_uuid,
                action,
            }));
        }
        _ => {}
    }

    out
}

pub(crate) fn push_data_message_modifiers(
    out: &mut Vec<InboundEvent>,
    chat_id: &str,
    sender_id: &str,
    dm: &DataMessage,
) {
    if let Some(Reaction {
        emoji,
        remove,
        target_sent_timestamp,
        ..
    }) = &dm.reaction
    {
        if let (Some(emoji), Some(ts)) = (emoji, target_sent_timestamp) {
            out.push(InboundEvent::Reaction(ReactionEvent {
                chat_id: chat_id.to_string(),
                target_message_id: ts.to_string(),
                emoji: emoji.clone(),
                sender_id: sender_id.to_string(),
                remove: remove.unwrap_or(false),
            }));
        }
    }

    if let Some(Delete {
        target_sent_timestamp: Some(ts),
    }) = &dm.delete
    {
        out.push(InboundEvent::Deleted(DeleteEvent {
            chat_id: chat_id.to_string(),
            message_id: ts.to_string(),
        }));
    }

    if let Some(pv) = &dm.poll_vote {
        if let Some(ts) = pv.target_sent_timestamp {
            out.push(InboundEvent::PollVote(crate::messaging::types::PollVoteEvent {
                chat_id: chat_id.to_string(),
                poll_id: ts.to_string(),
                voter_id: sender_id.to_string(),
                option_indexes: pv.option_indexes.clone(),
            }));
        }
    }
}

pub(crate) fn build_edit_event(
    chat_id: &str,
    target_ts: u64,
    dm: &DataMessage,
) -> Option<EditEvent> {
    let new_text = dm.body.clone()?;
    Some(EditEvent {
        chat_id: chat_id.to_string(),
        message_id: target_ts.to_string(),
        new_text,
        edited_at: dm.timestamp.unwrap_or(0),
    })
}

/// Pick a human-readable name for a sender given an optional cached contact.
/// Order: profile name → phone number → "~" + first 8 chars of UUID (Signal's
/// "unknown contact" UX). Pure function so it can be unit-tested without a store.
pub(crate) fn pick_sender_name(
    contact: Option<&presage::model::contacts::Contact>,
    sender_uuid_str: &str,
) -> String {
    if let Some(c) = contact {
        if !c.name.is_empty() {
            return c.name.clone();
        }
        if let Some(phone) = &c.phone_number {
            return phone.to_string();
        }
    }
    let prefix: String = sender_uuid_str.chars().take(8).collect();
    format!("~{}", prefix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use presage::libsignal_service::content::Metadata;
    use presage::libsignal_service::proto::{
        data_message::{Delete as PbDelete, Reaction as PbReaction},
        receipt_message, typing_message, ReceiptMessage, TypingMessage,
    };
    use presage::libsignal_service::protocol::{Aci, ServiceId};
    use presage::model::contacts::Contact;
    use uuid::Uuid;

    fn aci(uuid: Uuid) -> ServiceId {
        ServiceId::Aci(Aci::from(uuid))
    }

    fn metadata(sender: Uuid, ts: u64) -> Metadata {
        Metadata {
            sender: aci(sender),
            destination: aci(Uuid::nil()),
            sender_device: 1.try_into().unwrap(),
            timestamp: ts,
            needs_receipt: false,
            unidentified_sender: false,
            was_plaintext: false,
            server_guid: None,
        }
    }

    fn content_with_body(sender: Uuid, ts: u64, body: ContentBody) -> Content {
        Content {
            metadata: metadata(sender, ts),
            body,
        }
    }

    #[test]
    fn parse_thread_uuid() {
        let result = parse_thread("01234567-89ab-cdef-0123-456789abcdef");
        assert!(result.is_ok());
    }

    #[test]
    fn parse_thread_group_hex() {
        let key_hex = "a".repeat(64);
        let result = parse_thread(&key_hex);
        assert!(result.is_ok());
    }

    #[test]
    fn parse_thread_invalid() {
        let result = parse_thread("not-a-valid-id");
        assert!(result.is_err());
    }

    fn make_contact(name: &str, phone: Option<&str>) -> Contact {
        use presage::libsignal_service::prelude::phonenumber::PhoneNumber;
        Contact {
            uuid: Uuid::nil(),
            phone_number: phone.and_then(|p| p.parse::<PhoneNumber>().ok()),
            name: name.to_string(),
            verified: Default::default(),
            profile_key: vec![],
            expire_timer: 0,
            expire_timer_version: 2,
            inbox_position: 0,
            avatar: None,
        }
    }

    #[test]
    fn data_message_with_body_emits_message_event() {
        let sender = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);
        let dm = DataMessage {
            body: Some("hello world".to_string()),
            timestamp: Some(1700000000000),
            ..Default::default()
        };
        let content = content_with_body(sender, 1700000000000, ContentBody::DataMessage(dm));
        let events = derive_inbound_events(&content, &None);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], InboundEvent::Message { .. }));
    }

    #[test]
    fn reaction_message_emits_reaction_event() {
        let sender = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);
        let dm = DataMessage {
            timestamp: Some(1700000000000),
            reaction: Some(PbReaction {
                emoji: Some("🔥".to_string()),
                remove: Some(false),
                target_sent_timestamp: Some(1699999999000),
                target_author_aci: None,
                target_author_aci_binary: None,
            }),
            ..Default::default()
        };
        let content = content_with_body(sender, 1700000000000, ContentBody::DataMessage(dm));
        let events = derive_inbound_events(&content, &None);
        assert_eq!(events.len(), 1);
        match &events[0] {
            InboundEvent::Reaction(r) => {
                assert_eq!(r.emoji, "🔥");
                assert_eq!(r.target_message_id, "1699999999000");
                assert_eq!(r.sender_id, sender.to_string());
                assert_eq!(r.chat_id, sender.to_string());
                assert!(!r.remove);
            }
            other => panic!("expected Reaction, got {other:?}"),
        }
    }

    #[test]
    fn sender_name_prefers_profile_name() {
        let c = make_contact("Alice", Some("+33600000000"));
        let name = pick_sender_name(Some(&c), "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(name, "Alice");
    }

    #[test]
    fn sender_name_falls_back_to_phone() {
        let c = make_contact("", Some("+33612345678"));
        let name = pick_sender_name(Some(&c), "01234567-89ab-cdef-0123-456789abcdef");
        // PhoneNumber Display formats as "+33 6 12 34 56 78" — accept any non-empty,
        // non-uuid form, and require it contain the country code digits.
        assert!(
            name.contains("33"),
            "expected formatted phone, got {:?}",
            name
        );
        assert!(!name.starts_with('~'));
    }

    #[test]
    fn sender_name_fallback_to_uuid_prefix_when_no_contact() {
        let name = pick_sender_name(None, "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(name, "~01234567");
    }

    #[test]
    fn sender_name_fallback_when_contact_is_blank() {
        let c = make_contact("", None);
        let name = pick_sender_name(Some(&c), "deadbeef-cafe-1234-5678-9abcdef01234");
        assert_eq!(name, "~deadbeef");
    }

    #[test]
    fn sender_name_short_uuid_does_not_panic() {
        let name = pick_sender_name(None, "abc");
        assert_eq!(name, "~abc");
    }

    #[test]
    fn reaction_remove_flag_propagates() {
        let sender = Uuid::from_u128(0x3333_3333_3333_3333_3333_3333_3333_3333);
        let dm = DataMessage {
            timestamp: Some(1700000000000),
            reaction: Some(PbReaction {
                emoji: Some("👍".to_string()),
                remove: Some(true),
                target_sent_timestamp: Some(1699999999000),
                target_author_aci: None,
                target_author_aci_binary: None,
            }),
            ..Default::default()
        };
        let content = content_with_body(sender, 1700000000000, ContentBody::DataMessage(dm));
        let events = derive_inbound_events(&content, &None);
        match &events[0] {
            InboundEvent::Reaction(r) => assert!(r.remove),
            _ => panic!("expected Reaction"),
        }
    }

    #[test]
    fn delete_message_emits_delete_event() {
        let sender = Uuid::from_u128(0x4444_4444_4444_4444_4444_4444_4444_4444);
        let dm = DataMessage {
            timestamp: Some(1700000000000),
            delete: Some(PbDelete {
                target_sent_timestamp: Some(1699999999000),
            }),
            ..Default::default()
        };
        let content = content_with_body(sender, 1700000000000, ContentBody::DataMessage(dm));
        let events = derive_inbound_events(&content, &None);
        assert_eq!(events.len(), 1);
        match &events[0] {
            InboundEvent::Deleted(d) => {
                assert_eq!(d.message_id, "1699999999000");
                assert_eq!(d.chat_id, sender.to_string());
            }
            other => panic!("expected Deleted, got {other:?}"),
        }
    }

    #[test]
    fn edit_message_emits_edited_event() {
        let sender = Uuid::from_u128(0x5555_5555_5555_5555_5555_5555_5555_5555);
        let inner = DataMessage {
            body: Some("edited text".to_string()),
            timestamp: Some(1700000000500),
            ..Default::default()
        };
        let edit = EditMessage {
            target_sent_timestamp: Some(1700000000000),
            data_message: Some(inner),
        };
        let content = content_with_body(sender, 1700000000500, ContentBody::EditMessage(edit));
        let events = derive_inbound_events(&content, &None);
        assert_eq!(events.len(), 1);
        match &events[0] {
            InboundEvent::Edited(e) => {
                assert_eq!(e.message_id, "1700000000000");
                assert_eq!(e.new_text, "edited text");
                assert_eq!(e.edited_at, 1700000000500);
            }
            other => panic!("expected Edited, got {other:?}"),
        }
    }

    #[test]
    fn receipt_message_emits_receipt_event() {
        let sender = Uuid::from_u128(0x6666_6666_6666_6666_6666_6666_6666_6666);
        let receipt = ReceiptMessage {
            r#type: Some(receipt_message::Type::Read as i32),
            timestamp: vec![1700000000000, 1700000000100],
        };
        let content =
            content_with_body(sender, 1700000000200, ContentBody::ReceiptMessage(receipt));
        let events = derive_inbound_events(&content, &None);
        assert_eq!(events.len(), 1);
        match &events[0] {
            InboundEvent::Receipt(r) => {
                assert_eq!(r.kind, ReceiptKind::Read);
                assert_eq!(r.message_ids, vec!["1700000000000", "1700000000100"]);
                assert_eq!(r.chat_id, sender.to_string());
            }
            other => panic!("expected Receipt, got {other:?}"),
        }
    }

    #[test]
    fn empty_receipt_is_dropped() {
        // Defensive: a malformed receipt with no timestamps should be ignored
        // rather than emitting a noise event with an empty list.
        let sender = Uuid::from_u128(0x7777_7777_7777_7777_7777_7777_7777_7777);
        let receipt = ReceiptMessage {
            r#type: Some(0),
            timestamp: vec![],
        };
        let content =
            content_with_body(sender, 1700000000000, ContentBody::ReceiptMessage(receipt));
        let events = derive_inbound_events(&content, &None);
        assert!(events.is_empty());
    }

    #[test]
    fn typing_message_emits_typing_event() {
        let sender = Uuid::from_u128(0x8888_8888_8888_8888_8888_8888_8888_8888);
        let typing = TypingMessage {
            timestamp: Some(1700000000000),
            action: Some(typing_message::Action::Started as i32),
            group_id: None,
        };
        let content = content_with_body(sender, 1700000000000, ContentBody::TypingMessage(typing));
        let events = derive_inbound_events(&content, &None);
        assert_eq!(events.len(), 1);
        match &events[0] {
            InboundEvent::Typing(t) => {
                assert_eq!(t.action, TypingAction::Started);
                assert_eq!(t.sender_id, sender.to_string());
                assert_eq!(t.chat_id, sender.to_string());
            }
            other => panic!("expected Typing, got {other:?}"),
        }
    }

    #[test]
    fn typing_message_with_group_id_uses_hex_chat_id() {
        let sender = Uuid::from_u128(0x9999_9999_9999_9999_9999_9999_9999_9999);
        let group_id = vec![0xAB; 16];
        let typing = TypingMessage {
            timestamp: Some(1700000000000),
            action: Some(typing_message::Action::Stopped as i32),
            group_id: Some(group_id.clone()),
        };
        let content = content_with_body(sender, 1700000000000, ContentBody::TypingMessage(typing));
        let events = derive_inbound_events(&content, &None);
        match &events[0] {
            InboundEvent::Typing(t) => {
                assert_eq!(t.chat_id, hex::encode(&group_id));
                assert_eq!(t.action, TypingAction::Stopped);
            }
            _ => panic!("expected Typing"),
        }
    }

    #[test]
    fn null_message_yields_no_events() {
        // Sentinel: presage uses NullMessage to tombstone deletions in the
        // local store. We MUST NOT emit anything for those — they're internal.
        let sender = Uuid::from_u128(0xAAAA_AAAA_AAAA_AAAA_AAAA_AAAA_AAAA_AAAA);
        let content = content_with_body(
            sender,
            1700000000000,
            ContentBody::NullMessage(presage::libsignal_service::proto::NullMessage::default()),
        );
        let events = derive_inbound_events(&content, &None);
        assert!(events.is_empty());
    }

    #[test]
    fn data_message_with_only_reaction_does_not_emit_chat_message() {
        // Regression guard: the old `_ => None` arm in content_to_chat_message
        // accidentally produced empty bubbles when the only thing the message
        // carried was a reaction. We now suppress the chat-message slot for
        // reaction-only and delete-only DataMessages.
        let sender = Uuid::from_u128(0xBBBB_BBBB_BBBB_BBBB_BBBB_BBBB_BBBB_BBBB);
        let dm = DataMessage {
            timestamp: Some(1700000000000),
            reaction: Some(PbReaction {
                emoji: Some("❤️".to_string()),
                remove: Some(false),
                target_sent_timestamp: Some(1699999999000),
                target_author_aci: None,
                target_author_aci_binary: None,
            }),
            ..Default::default()
        };
        let content = content_with_body(sender, 1700000000000, ContentBody::DataMessage(dm));
        let events = derive_inbound_events(&content, &None);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], InboundEvent::Reaction(_)));
    }
}
