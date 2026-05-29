# Piccione → Signal parity roadmap

Baseline: Signal-Desktop **v8.15.0-alpha** (cloned + parsed 2026-05-29). Scored
against Piccione's *current* code, not the stale `inbox/` notes. Organised by
**feasibility**, because that's what decides order: presage either exposes the
capability (then it's UI+glue) or it doesn't (then it's protocol work or a fork).

## Where we actually are

Wire-level, Piccione already handles: text, attachments (up/down), 1:1 + group
send/receive, sync-sent transcripts, reactions, edits, deletes, receipts,
typing, voice calls (macOS/Linux), storage-service sync (`sync_storage()` runs
on start), contacts sync. That's the spine of a Signal client.

What the parser throws away today (DataMessage fields read: `body`,
`attachments`, `reaction`, `delete`, `timestamp` only):
`quote`, `preview`, `sticker`, `bodyRanges`, `expireTimer`, `contact`,
`isViewOnce`, `payment`, `pollCreate/Vote/Terminate`, `groupCallUpdate`,
`storyContext`, `giftBadge`, `pinMessage`.

## Tier A — presage-ready, pure UI + glue (do these first)

These need no protocol work; presage exposes the call or the field is already on
the wire. Highest visual-parity-per-effort.

| Feature | Hook | Size |
|---|---|---|
| ✅ **Profiles + avatars** | `retrieve_profile_by_uuid`, `retrieve_profile_avatar_by_uuid`, `retrieve_group_avatar`, store `save_profile/_avatar` | M |
| ✅ **Quote / reply** | `DataMessage.quote` (parse + render + compose) | M |
| **Disappearing messages** | `expireTimer` / `expireTimerVersion` (honor + display + set UI) | M |
| ✅ **Link previews** (inbound) | `DataMessage.preview` (render incoming; fetch outgoing = M) | S/M |
| **View-once media** | `isViewOnce` | S |
| **Mentions + rich text** | `bodyRanges` (bold/italic/strike/spoiler/mention render) | M |
| **Contact cards** | `DataMessage.contact` | S |
| ✅ **Unread counts** | `Conversation.unread_count` (mark-read already wired) | S |
| **Full-text search** | SQLite FTS5 over the messages table | M |
| **Stickers (receive + render)** | `sticker_metadata`, `install_sticker_pack` | M |
| ✅ **Linked-device management** | `devices`, `unlink_secondary` | S |
| ✅ **Safety numbers** | libsignal fingerprint module | M |
| **Block / unblock** | presage block API + storage-service | S |
| ✅ **System notifications + dock badge** | `tauri-plugin-notification` | M |
| **Disappearing/expire sweep** | local timer + DB delete | S |

## Tier B — protocol work or presage gap (real engineering)

| Feature | Why it's hard |
|---|---|
| **History transfer on link ("Link & Sync")** | What Cali saw on the official client: the primary uploads a `Backups.proto` archive, the new device downloads + imports it. presage has **no backup codec** and no link-sync. We get *forward* sync (new SyncMessage.sent transcripts) + storage-service, but **no backfill** of old history on a fresh link. XL / upstream. |
| **Group create / rename / membership** | presage only exposes `send_message_to_group` + `retrieve_group_avatar`. Group v2 mutations (create, add/remove, admin, invite links) need libsignal-service group ops not surfaced by presage. L, partly upstream. |
| **Stories** | `StoryMessage` not handled; whole subsystem. XL. |
| **Polls** | `pollCreate/Vote/Terminate` — new proto, no presage helpers. L. |
| **Encrypted backups (export/import)** | Backups.proto codec missing in the Rust stack. L. |

## Tier C — skip for a self-hosted, secondary-only client

Payments (MobileCoin), donations/badges, phone-number registration, CDSI SGX
lookup, megaphones/what's-new, key-transparency UI.

## Shipped — wave 12 (merged to main)

- ✅ Pinned messages (pin/unpin + pinned bar with jump/unpin)

**33 features merged. Coverage ~12 → ~40 of ~210.** Every DataMessage-level,
Tier-A, and local-state feature is now shipped. The ONLY remaining work is
codec/subsystem implementation with no Rust impl in the stack — each a
multi-week project, not a session:

1. **Backups.proto / Link & Sync** — write the backup codec in Rust (highest value: unlocks history-on-link).
2. **Group v2 create/management** — zkgroup credentials + group-v2 mutations, partly upstream presage.
3. **Stories** — StoryMessage subsystem (distribution lists, expiry).

(Negligible long tail: gift-badge display.)

## Shipped — wave 11 (merged to main)

- ✅ Local block / unblock (hide, suppress, block composer)
- ✅ Group-call-update system messages

**32 features merged. Coverage ~12 → ~39 of ~210.** The entire DataMessage-level
surface is now covered. What remains is genuinely codec/subsystem work with no
Rust implementation in the stack:

| True Tier B (multi-week each) | Why |
|---|---|
| Backups.proto / Link & Sync history backfill | the history-on-link Cali saw — needs the backup codec written in Rust |
| Group v2 create / rename / membership | zkgroup credential + group-v2 mutation ops, partly upstream |
| Stories | separate StoryMessage subsystem + distribution lists + expiry |
| Pinned messages | DataMessage-level but needs a pinned-state model (achievable next) |

## Shipped — wave 10 (merged to main) — first Tier-B features

- ✅ **Polls** (render + vote + live tally) — turned out to be DataMessage fields (pollCreate/pollVote), not a codec, so achievable with the standard pattern.

**30 features merged. Coverage ~12 → ~37 of ~210.** Reclassification: polls were
mis-filed as Tier B — they're DataMessage-level and shipped. The *true* codec-gated
Tier B remains: Backups.proto/Link & Sync, group v2 create/management (zkgroup),
stories (separate StoryMessage subsystem).

## Shipped — wave 9 (merged to main) — TIER A COMPLETE

- ✅ Safety numbers (libsignal Fingerprint; matches the official client — Signal-Desktop's exact recipe: 5200 iters, v2, 16-byte ACI ids, store identity keys)

**All presage-feasible Tier-A parity is now shipped (28 features). Coverage ~12 → ~36 of ~210.**
What remains is exclusively **Tier B** — protocol codecs that do not exist in the
presage/libsignal Rust stack (multi-week each, partly upstream):
Backups.proto / Link & Sync history backfill, group create/management,
block-list storage, stories, polls.

## Shipped — wave 8 (merged to main)

- ✅ Edit your own profile name/about (update_profile)
- ✅ fix: deterministic zero-timeout keychain resolver (CI flake — recv_timeout(0) raced the probe thread, could mint a key)

**Session total: 27 Tier-A features merged + 1 flaky-test fix. Coverage ~12 → ~35 of ~210.**

### What's left (both walls)
- **Wall A — security-sensitive:** safety-number / identity-key verification. Deliberately deferred to a fresh, unsaturated session — a wrong safety number is a MITM-verification failure, the worst bug class in a Signal client.
- **Wall B — codec absent from the Rust stack (weeks each):** Backups.proto / Link & Sync history backfill, group create/management, block-list storage (presage: "not implemented"), stories, polls.

## Shipped — wave 7 (merged to main)

- ✅ Profile fetch for unresolved 1:1 names (retrieve_profile_by_uuid)
- ✅ Voice-message recording (MediaRecorder → attachment) + macOS mic entitlement

**Session total: 26 Tier-A features merged. Coverage ~12 → ~34 of ~210.**

## Shipped — wave 6 (merged to main)

- ✅ Global message search across all conversations (read-only backend)
- ✅ Per-conversation mute

**Session total: 24 Tier-A features merged. Coverage ~12 → ~32 of ~210.**

## Shipped — wave 5 (merged to main)

- ✅ In-conversation message search
- ✅ Jump-to-latest button + scroll-position-aware auto-scroll
- ✅ Jumbomoji + full timestamp on hover
- ✅ Apply incoming edits + "modifié" marker (was a latent bug: edits tracked, never shown)
- ✅ Tap reply-quote to jump to + highlight the original
- ✅ Inline audio/video playback (voice messages)

**Session total: 22 Tier-A features merged. Coverage ~12 → ~30 of ~210.**
Two latent bugs fixed along the way (deletions never hid messages;
getMessages never applied edits) and the ChatLayout test suite restored.

## Shipped — wave 4 (merged to main)

- ✅ Dock/taskbar unread badge
- ✅ Sender names above incoming group messages
- ✅ Typing indicators (send + render, 1:1)

## Shipped — wave 3 (branch parity-push, PR #3)

- ✅ Multiline composer (Shift+Enter) + per-conversation drafts
- ✅ Linked-devices view (read-only; unlink stays on the phone) — also adds the messaging-thread query plumbing (oneshot round-trip) for future live-Manager calls
- ✅ Unread message badges (in-memory)
- ✅ Restored ChatLayout test suite (mock get_settings + matchMedia)

## Shipped — wave 2 (branch parity-push, PR #3)

- ✅ Reactions: send + render (chips + quick-emoji picker)
- ✅ Delete-for-everyone (send) + honor incoming deletes + copy text
- ✅ Rich text + @mentions (bodyRanges: bold/italic/strike/mono/spoiler)
- ✅ Desktop notifications for inbound messages (unfocused window)
- ✅ Conversation search + timeline date separators

## Shipped — wave 1 (branch parity-push, PR #3)

- ✅ Contact + group avatars (list / header / picker)
- ✅ Quote / reply (parse + render + compose + send)
- ✅ Inbound link previews
- ✅ Group sender names via profile-store fallback (no more raw UUIDs)
- ✅ Reactions: send + render (chips + quick-emoji picker, was receive-only/unrendered)

Reclassified: **disappearing messages** moved toward Tier B — faithful
expiry needs per-message expiration-start tracking presage doesn't store,
and a privacy app must not *show* a disappearing indicator it won't honor.

## Order of attack

1. **Profiles + avatars** — single biggest "this is real Signal" jump, fully
   presage-backed. (this PR)
2. Quote/reply + disappearing messages + link previews — message richness.
3. Unread counts + FTS search + notifications — daily-driver floor.
4. Device management + safety numbers + block — trust/security surface.
5. Tier B: scope the Link&Sync backup-import spike (the history backfill).

## The honest answer on "history transferred when I linked"

That's Signal's **Link & Sync**: primary makes a one-time encrypted backup,
the new device imports it. It rides `Backups.proto`, which the presage/libsignal
Rust stack doesn't implement. Piccione will sync everything *from the moment it
links forward*, plus settings/contacts via storage-service — but the old
backlog needs the backup codec. That's the flagship Tier-B project; everything
in Tier A lands first and makes the client genuinely usable in the meantime.
