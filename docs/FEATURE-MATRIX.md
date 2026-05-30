# Feature matrix — Piccione vs Signal-Desktop

Baseline: Signal-Desktop v8.15.0-alpha. Status of `main` as of 2026-05-30.

**Legend:** ✅ done · ⚠️ partial · 🔜 built, needs live test · ❌ not yet

## Core messaging

| Signal-Desktop | Piccione | Notes |
|---|---|---|
| Link as secondary device (QR) | ✅ | |
| 1:1 + group text send/receive | ✅ | |
| Multi-device sync (sent transcripts) | ✅ | |
| Attachments (send/receive, images/files) | ✅ | inline image render, file rows |
| Audio/video playback | ✅ | inline players |
| Voice message recording | ✅ | mic → attachment |
| Paste image into composer | ✅ | |
| Contacts sync | ✅ | |
| Encrypted local store (SQLCipher) | ✅ | |

## Message features

| Signal-Desktop | Piccione | Notes |
|---|---|---|
| Reactions (send + render) | ✅ | chips + quick-emoji picker |
| Quote / reply | ✅ | + tap-to-jump |
| Edit message (send/receive) | ✅ | render incoming + send edits (EditMessage) |
| Delete-for-everyone | ✅ | send + honor incoming |
| Delete-for-me (local) | ❌ | |
| Rich text (bold/italic/strike/mono/spoiler) | ✅ | render + compose-send (bodyRanges) |
| @mentions | ⚠️ | render pills ✅; compose ❌ (needs rich input) |
| Link previews | ⚠️ | render incoming ✅; outgoing fetch ❌ |
| Stickers | ⚠️ | render incoming ✅; send / pack mgmt ❌ |
| Shared contact cards | ✅ | render |
| Polls | ✅ | render + vote + tally; create ❌ |
| Pinned messages | ✅ | pin/unpin + bar |
| View-once media | ❌ | deferred (honor logic) |
| Disappearing messages | ❌ | deferred (honor/anchor logic) |
| GIF / Giphy | ❌ | |
| Payments (MobileCoin) | ❌ | out of scope |

## Conversation & UX

| Signal-Desktop | Piccione | Notes |
|---|---|---|
| Avatars (contact + group) | ✅ | |
| Real profile names | ✅ | + profile fetch fallback |
| Typing indicators | ✅ | send + render |
| Read / delivery receipts | ✅ | |
| Unread badges + dock badge | ✅ | |
| Notifications (desktop) | ✅ | |
| Mute conversation | ✅ | |
| Block / unblock | ⚠️ | local enforcement ✅; storage-service sync ❌ (presage gap) |
| Pin / archive conversation | ✅ | local |
| Conversation search | ✅ | |
| In-chat + global message search | ✅ | |
| Date separators, jumbomoji, jump-to-latest | ✅ | |
| Drafts, multiline composer | ✅ | |
| Themes (light/dark/auto) | ✅ | |
| Settings panel | ✅ | |

## Identity / account / groups

| Signal-Desktop | Piccione | Notes |
|---|---|---|
| Safety numbers (verify) | ✅ | matches official client's number |
| Linked devices view | ✅ | read-only (unlink on phone) |
| Edit own profile (name/about) | ✅ | |
| Group send/receive/display | ✅ | |
| Group create / rename / membership | ❌ | needs zkgroup group-v2 ops |
| Group invite links | ❌ | |
| Message requests gating | ❌ | |

## Voice / video calls

| Signal-Desktop | Piccione | Notes |
|---|---|---|
| 1:1 voice call (RingRTC) | 🔜 | built (macOS/Linux); on-LAN only, needs live ICE/identity test |
| Video calls | ❌ | |
| Group calls | ❌ | |

## History on link (Link & Sync)

| Signal-Desktop | Piccione | Notes |
|---|---|---|
| Link & Sync — import (decode → store) | 🔜 | full codec built + unit-validated: contacts + 1:1/group text + quotes |
| Link & Sync — archive fetch | ❌ | WebAPI long-poll (`GET /v1/devices/transfer_archive`); needs presage `PushService` method + live primary |
| Encrypted remote backups (SVR-B) | ❌ | |
| Stories | ❌ | subsystem |

## Tally

**~33 of Signal-Desktop's ~210 capabilities** — but the *core* of a real client:
the entire `DataMessage` send/receive surface, full UX / identity / search /
notifications, and the import half of the history-on-link codec.

The gaps cluster into four buckets:
- **honor-logic** — view-once, disappearing messages
- **rich-input** — compose @mentions (needs group-member list in the composer; edit-send is done)
- **upstream-codec** — group-v2 (zkgroup), archive-fetch (WebAPI + live primary), stories (subsystem)
- **niche / out-of-scope** — payments, GIFs

See `docs/BACKUPS-DESIGN.md` for the Link & Sync internals and
`docs/PARITY.md` for the wave-by-wave history.
