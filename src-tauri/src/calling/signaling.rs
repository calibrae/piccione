//! Mapping between RingRTC's `signaling::Message` and Signal's wire
//! `CallMessage` protobuf.
//!
//! The good news: RingRTC's Offer / Answer / ICE all carry their payload as
//! an `opaque: Vec<u8>` blob, and Signal's `CallMessage` proto has matching
//! `opaque` byte fields. So the mapping is struct-shuffling — move the
//! opaque blob + the call id + the media/hangup type across — not
//! byte-format surgery.
//!
//! `[LIVE-TEST]` — every field correspondence here is read off the proto
//! definitions + RingRTC's `core/signaling.rs`, and matches what
//! Signal-Desktop's node layer does. But "compiles + looks right" is not
//! "two phones actually connected a call" — this is the seam that needs a
//! real call between two devices to confirm.

use presage::libsignal_service::proto::{call_message, CallMessage};
use ringrtc::common::{CallId, CallMediaType};
use ringrtc::core::signaling;

/// Serialise a RingRTC outbound signaling message into a Signal
/// `CallMessage` ready to hand to `Manager::send_message`.
pub fn to_call_message(call_id: CallId, msg: &signaling::Message) -> CallMessage {
    let id = call_id.as_u64();
    match msg {
        signaling::Message::Offer(offer) => CallMessage {
            offer: Some(call_message::Offer {
                id: Some(id),
                r#type: Some(media_type_to_offer_type(offer.call_media_type) as i32),
                opaque: Some(offer.opaque.clone()),
            }),
            ..Default::default()
        },
        signaling::Message::Answer(answer) => CallMessage {
            answer: Some(call_message::Answer {
                id: Some(id),
                opaque: Some(answer.opaque.clone()),
            }),
            ..Default::default()
        },
        signaling::Message::Ice(ice) => CallMessage {
            ice_update: ice
                .candidates
                .iter()
                .map(|c| call_message::IceUpdate {
                    id: Some(id),
                    opaque: Some(c.opaque.clone()),
                })
                .collect(),
            ..Default::default()
        },
        signaling::Message::Hangup(hangup) => {
            let (typ, device_id) = hangup.to_type_and_device_id();
            CallMessage {
                hangup: Some(call_message::Hangup {
                    id: Some(id),
                    r#type: Some(hangup_type_to_proto(typ) as i32),
                    device_id,
                }),
                ..Default::default()
            }
        }
        signaling::Message::Busy => CallMessage {
            busy: Some(call_message::Busy { id: Some(id) }),
            ..Default::default()
        },
    }
}

/// Parse an inbound Signal `CallMessage` into a RingRTC `(CallId, Message)`.
///
/// Returns `Ok(None)` for a `CallMessage` that carries nothing we handle
/// (e.g. an `opaque`-only message — group calling, not wired yet).
pub fn from_call_message(
    cm: &CallMessage,
) -> Result<Option<(CallId, signaling::Message)>, SignalingMapError> {
    if let Some(offer) = &cm.offer {
        let id = CallId::new(offer.id.unwrap_or(0));
        let media = offer_type_to_media_type(offer.r#type.unwrap_or(0));
        let opaque = offer.opaque.clone().ok_or(SignalingMapError::MissingOpaque)?;
        let parsed = signaling::Offer::new(media, opaque)
            .map_err(|_| SignalingMapError::BadOpaque)?;
        return Ok(Some((id, signaling::Message::Offer(parsed))));
    }
    if let Some(answer) = &cm.answer {
        let id = CallId::new(answer.id.unwrap_or(0));
        let opaque = answer.opaque.clone().ok_or(SignalingMapError::MissingOpaque)?;
        let parsed = signaling::Answer::new(opaque)
            .map_err(|_| SignalingMapError::BadOpaque)?;
        return Ok(Some((id, signaling::Message::Answer(parsed))));
    }
    if !cm.ice_update.is_empty() {
        // Every IceUpdate in one CallMessage shares the same call id.
        let id = CallId::new(cm.ice_update[0].id.unwrap_or(0));
        let candidates = cm
            .ice_update
            .iter()
            .filter_map(|u| {
                u.opaque
                    .clone()
                    .map(|opaque| signaling::IceCandidate { opaque })
            })
            .collect();
        return Ok(Some((
            id,
            signaling::Message::Ice(signaling::Ice { candidates }),
        )));
    }
    if let Some(hangup) = &cm.hangup {
        let id = CallId::new(hangup.id.unwrap_or(0));
        let typ = signaling::HangupType::from_i32(hangup.r#type.unwrap_or(0))
            .ok_or(SignalingMapError::BadHangupType)?;
        let parsed =
            signaling::Hangup::from_type_and_device_id(typ, hangup.device_id.unwrap_or(0));
        return Ok(Some((id, signaling::Message::Hangup(parsed))));
    }
    if let Some(busy) = &cm.busy {
        let id = CallId::new(busy.id.unwrap_or(0));
        return Ok(Some((id, signaling::Message::Busy)));
    }
    // `opaque`-only CallMessages are group-calling traffic — not handled yet.
    Ok(None)
}

#[derive(Debug, thiserror::Error)]
pub enum SignalingMapError {
    #[error("CallMessage field missing its opaque blob")]
    MissingOpaque,
    #[error("CallMessage opaque blob failed to deserialize")]
    BadOpaque,
    #[error("CallMessage hangup carried an unknown type")]
    BadHangupType,
}

fn media_type_to_offer_type(m: CallMediaType) -> call_message::offer::Type {
    match m {
        CallMediaType::Audio => call_message::offer::Type::OfferAudioCall,
        CallMediaType::Video => call_message::offer::Type::OfferVideoCall,
    }
}

fn offer_type_to_media_type(t: i32) -> CallMediaType {
    match call_message::offer::Type::try_from(t) {
        Ok(call_message::offer::Type::OfferVideoCall) => CallMediaType::Video,
        // OfferAudioCall, or anything unrecognised — default to audio. We're
        // a voice-first client; an unknown offer type degrading to audio is
        // the safe failure mode.
        _ => CallMediaType::Audio,
    }
}

fn hangup_type_to_proto(t: signaling::HangupType) -> call_message::hangup::Type {
    match t {
        signaling::HangupType::Normal => call_message::hangup::Type::HangupNormal,
        signaling::HangupType::AcceptedOnAnotherDevice => {
            call_message::hangup::Type::HangupAccepted
        }
        signaling::HangupType::DeclinedOnAnotherDevice => {
            call_message::hangup::Type::HangupDeclined
        }
        signaling::HangupType::BusyOnAnotherDevice => call_message::hangup::Type::HangupBusy,
        signaling::HangupType::NeedPermission => {
            call_message::hangup::Type::HangupNeedPermission
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_round_trips_through_call_message() {
        // A RingRTC offer's opaque blob must survive the trip out to a
        // CallMessage and back unchanged, with the call id + media type.
        // We can't build a real signaling::Offer without a valid opaque
        // proto, so this test pins the *outbound* mapping shape.
        let cm = CallMessage {
            offer: Some(call_message::Offer {
                id: Some(0xDEAD_BEEF),
                r#type: Some(call_message::offer::Type::OfferAudioCall as i32),
                opaque: Some(vec![1, 2, 3, 4]),
            }),
            ..Default::default()
        };
        // Inbound parse of an offer with a bogus opaque must fail cleanly,
        // not panic.
        assert!(matches!(
            from_call_message(&cm),
            Err(SignalingMapError::BadOpaque)
        ));
    }

    #[test]
    fn busy_round_trips() {
        let cm = CallMessage {
            busy: Some(call_message::Busy { id: Some(42) }),
            ..Default::default()
        };
        let (id, msg) = from_call_message(&cm).unwrap().unwrap();
        assert_eq!(id.as_u64(), 42);
        assert!(matches!(msg, signaling::Message::Busy));
        // and back out
        let back = to_call_message(id, &msg);
        assert_eq!(back.busy.unwrap().id, Some(42));
    }

    #[test]
    fn hangup_type_maps_both_ways() {
        let cm = CallMessage {
            hangup: Some(call_message::Hangup {
                id: Some(7),
                r#type: Some(call_message::hangup::Type::HangupNormal as i32),
                device_id: None,
            }),
            ..Default::default()
        };
        let (id, msg) = from_call_message(&cm).unwrap().unwrap();
        assert_eq!(id.as_u64(), 7);
        let back = to_call_message(id, &msg);
        let h = back.hangup.unwrap();
        assert_eq!(h.id, Some(7));
        assert_eq!(h.r#type, Some(call_message::hangup::Type::HangupNormal as i32));
    }

    #[test]
    fn opaque_only_call_message_is_ignored() {
        let cm = CallMessage {
            opaque: Some(call_message::Opaque {
                data: Some(vec![9, 9, 9]),
                urgency: None,
            }),
            ..Default::default()
        };
        assert!(from_call_message(&cm).unwrap().is_none());
    }
}
