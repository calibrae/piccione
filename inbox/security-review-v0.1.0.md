# Adversarial security review — Piccione v0.1.0

Self-conducted adversarial pass (no separate agent available in-session;
done by trying to break the code, attacker-mindset, mapped to OWASP 2021).
Scope: `src-tauri/` Rust + `src/` Svelte. Date: 2026-05-13, commit 841be9b.

## Verdict

One **HIGH** (path traversal, fix below), one **MEDIUM** dependency item
(already fixed), the rest LOW/INFO or accepted-by-design. No `unsafe`, no
SQL injection, no XSS sink reachable by remote content, no secrets in logs.
Not bad for a 3-day build — but the path-traversal must be fixed before
any wider distribution.

---

## HIGH — A03 Injection: path traversal in attachment writes  (FIXED in commit below)

`messaging/service.rs` `download_attachments()`:

```rust
let filename = format!("{}_{}.{}", att.id, att.file_name, ext);
let path = att_dir.join(&filename);
std::fs::write(&path, &data)
```

`att.file_name` is `AttachmentPointer.file_name` straight off the wire —
**100% sender-controlled** (see `parse.rs::extract_attachments`). A
malicious sender sets `file_name` to `../../../../../../Users/cali/.zshrc`
and the write lands outside `att_dir`. `Path::join` resolves `..`
components; the `<id>_` prefix and `.<ext>` suffix do not neutralise a
`../` in the middle. `att.id` from a `CdnKey` is also sender-influenced
and unvalidated — a leading `/` would make `join` discard the base
entirely.

**Impact**: arbitrary file write within the user's permissions, triggered
by receiving a message. Overwrite shell rc files, LaunchAgents, etc.

**Fix**: never derive the on-disk name from sender input. Use only the
server CDN id, hard-sanitised:

```rust
let safe_id: String = att.id.chars()
    .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
    .collect();
let filename = format!("{safe_id}.{ext}");
```

Keep `att.file_name` as display-only metadata (it already rides in
`AttachmentInfo` for the UI). Belt-and-braces: after `join`, assert the
canonicalised path still starts with `att_dir`.

---

## MEDIUM — A06 Vulnerable components (FIXED in commit c7dd6ac)

`cargo audit` flagged 4. Three were `rustls-webpki` — in the TLS cert
validation path that verifies Signal's servers:

- RUSTSEC-2026-0104  reachable panic in CRL parsing (DoS)
- RUSTSEC-2026-0098  name constraints wrongly accepted for URI names
- RUSTSEC-2026-0099  name constraints accepted for wildcard-name certs

All fixed by bumping `rustls-webpki` 0.103.10 → 0.103.13. **Done.**

The 4th — RUSTSEC-2023-0071, `rsa` Marvin Attack timing sidechannel — has
no upstream fix, but `rsa` is pulled **only** by `sqlx-mysql`. Piccione
uses SQLite exclusively; the vulnerable RSA decrypt is never on an
executed path. **Follow-up**: trim sqlx's default backend features in
`presage-store-sqlite` so `sqlx-mysql` (and `rsa`) leave the tree
entirely. Tracked, not blocking.

`npm audit` (frontend): 0 vulnerabilities.

Also-ran (not vulnerabilities, "unmaintained" advisories): `atk`, `gdk*`
(GTK3 bindings — Tauri's Linux dep, out of our hands), `bincode`,
`fxhash`, `atomic-polyfill`. No action.

---

## LOW / INFO

### A02 — key sits beside the lock
`.db_key` (SQLCipher passphrase, 256-bit, 0600) lives in the same
directory as `signalui.db`. On a running system an attacker who can read
the data dir gets both. SQLCipher-at-rest therefore only defends against
*offline disk theft where the dir isn't also copied* — i.e. very little.
This is the documented v0.x tradeoff (`inbox/memory-accounting.md`,
`keychain.rs` module docs). **Real fix per-platform**: macOS Keychain /
Windows DPAPI / Linux libsecret — deferred because the dev-build keychain
UX was hostile (see git history). Acceptable for a single-user build;
must be revisited before "install it for your friends".

### A03 — `{@html}` sink in `QrCode.svelte`
`{@html svg}` renders the pairing QR. `svg` is generated Rust-side by the
`qrcode` crate from the provisioning URL — locally produced, not remote
content, and the URL rides as the QR *matrix*, not as embedded SVG text.
Not currently exploitable. **Invariant to document & never break**: the
value passed to `{@html}` must always be locally generated. Add a code
comment to that effect.

### A01 — Tauri command surface
14 `#[tauri::command]`s, all trusted-WebView-only. No `shell` / `fs` /
`process` plugins exposed; only `tauri-plugin-dialog`. `send_message_with_
attachments` takes `file_paths: Vec<String>` from the WebView and reads
them — an XSS in our own Svelte would turn that into arbitrary-file
exfiltration. We have no `{@html}` reachable by message content and no
`innerHTML`, so no XSS vector today. Standard Tauri trust model; fine
while the frontend stays ours.

### A06/DoS — `unwrap()` after guard
`service.rs` `download_attachments`: `att.pointer_data.as_ref().unwrap()`
is guarded by an early `continue` on `pointer_data.is_none()` — safe, but
fragile. Prefer `let Some(..) = .. else { continue }`. Cosmetic.

## Checked and clean

- **No `unsafe`** anywhere in `src-tauri/src/`.
- **SQL injection** — none. All queries go through sqlx `query!` /
  `query_as!` compile-time-checked macros in presage-store-sqlite. The
  one `format!`-built string is `sqlite:{db_path}?mode=rwc` where
  `db_path` is our own `app_data_dir()` join — not attacker-reachable.
- **Secrets in logs** — none. Passphrase is never logged (only "created
  fresh database encryption key in .db_key file"). ACI is logged — it's
  an account identifier, not a secret. settings.json has no secrets.
- **Deserialization** — `serde_json::from_str::<Settings>` is a bounded
  type, defaults-on-error, no panic. proto decode is bounded. The one
  `copy_from_slice` into a fixed array is length-guarded (`if bytes.len()
  == 32`).
- **Panics as DoS** — `panic!`/`unwrap` in `parse.rs` are all in
  `#[cfg(test)]`. `expect("time went backwards")` only fires if the
  system clock predates 1970. `expect()` on runtime/thread spawn is
  startup-only.
- **CSP** — tight: `default-src 'self'`, no `script-src 'unsafe-inline'`,
  `connect-src` limited to `ipc:`. `style-src 'unsafe-inline'` is needed
  for Svelte scoped styles, low risk.

## Action items

1. ~~[HIGH] Sanitise attachment filenames~~ — **DONE**: on-disk name is
   now the hard-filtered CDN id only, with a post-join containment check.
2. **[follow-up]** Trim `sqlx-mysql`/`rsa` from the tree via
   presage-store-sqlite sqlx features.
3. **[follow-up]** Per-platform secret storage (Keychain/DPAPI/libsecret)
   before multi-user distribution.
4. **[nit]** Code comment on the `{@html}` invariant in QrCode.svelte.
5. **[nit]** `let Some(..) else continue` instead of guarded `unwrap()`.
