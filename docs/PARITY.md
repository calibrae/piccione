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
| **Unread counts** | `Conversation.unread_count` (mark-read already wired) | S |
| **Full-text search** | SQLite FTS5 over the messages table | M |
| **Stickers (receive + render)** | `sticker_metadata`, `install_sticker_pack` | M |
| **Linked-device management** | `devices`, `unlink_secondary` | S |
| **Safety numbers** | libsignal fingerprint module | M |
| **Block / unblock** | presage block API + storage-service | S |
| **System notifications + dock badge** | `tauri-plugin-notification` | M |
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
