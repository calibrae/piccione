# Overnight autonomous run — status for Cali

Window: 2026-05-29 ~23:36 → 2026-05-30 07:36 CEST. Goal: Signal feature parity.
Everything below is **merged to `main`, CI-green**. Binary: **rc.5** (see Releases).

## Parity features shipped earlier this session (~28–34, all merged)
avatars · quote/reply (+jump) · link previews · reactions · delete-for-everyone ·
rich text & @mentions · edits ("modifié") · typing (send+render) · receipts ·
desktop + dock notifications · unread badges · mute · block · group sender labels ·
conversation / in-chat / **global** search · date separators · jumbomoji ·
multiline composer + drafts · jump-to-latest · scroll-anchoring · audio/video
playback · **voice-message recording** · linked-devices view · **safety numbers**
(matches official client) · self-profile edit · polls (vote+tally) · pinned
messages · group-call-update + gift-badge system lines.

## The white whale — Backups / Link & Sync history import (NEW)
The "history transferred when I linked" feature. **Import side is complete and
unit-validated:**
- presage fork persists the `accountEntropyPool` (calibrae/presage#1).
- AEP → `BackupKey` → `MessageBackupKey`.
- decrypt (`FramesReader`) → frame loop → reconstruct presage `Content`s.
- imports **contacts + 1:1 text + group text** into the store
  (`save_contact`/`save_message`).
- `preview_backup` + `import_backup` commands; **Settings → "Sauvegarde"**
  file-picker import.
- **8 backups tests**: canonical Signal backup fixture (decode) + synthetic
  frames (1:1 & group message reconstruction — thread/body/timestamp).

### What it still needs (live infra / fork work — your help)
1. **Archive fetch** — Link & Sync pulls the archive via a **WebAPI/REST**
   handshake (not a sync message), needing a live primary + server +
   libsignal-service-rs endpoints. Until then, `import_backup` runs against an
   archive **file** you supply.
2. **2-device live test** — link Piccione as secondary, point the import at a
   real transfer archive, confirm contacts + text history land. This is the
   `[LIVE-TEST]` that validates the whole pipeline end-to-end.
3. Attachments / non-text items, and group **metadata** (names) — deferred.

## How to try the import now
Settings → Sauvegarde → "Importer une sauvegarde" → pick a decrypted-by-key
transfer archive file. (Key derives from the AEP captured at link; if it says
"indisponible", the device was linked before the AEP-persist fix — relink.)

## Orchestration notes (for the next agent)
CronCreate is NOT enabled in this context; grytti/prompto/palazzo MCP aren't
callable as functions — used `run_in_background` tasks + a sleep heartbeat as
the cron, self-merging CI watchers, and curl→palazzo `/ingest`. Full technical
detail + dependency findings are in palazzo (room: piccione) and
`docs/BACKUPS-DESIGN.md`.
