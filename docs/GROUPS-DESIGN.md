# Design: Group v2 management (create / modify / leave)

The remaining group gap. Piccione does group **messaging** end-to-end; what's
missing is **management** — create a group, rename, add/remove members, leave.

## Key finding — most of the hard part already exists

The write-side is **not** "implement zkgroup from scratch." libsignal-service-rs
`src/groups_v2/operations.rs` already has the ZK builders:

| Already in the stack | Does |
|---|---|
| `GroupSecretParams::derive_from_master_key` | group crypto context |
| `GroupOperations::encrypt_group_with_credentials(title, self_credential)` | **builds the encrypted `proto::Group` for create** (self as full member; others full if we have their credential, else **pending invite**) |
| `build_add_member_action` / `build_remove_member_action` / `build_add_pending_member_action` | **GroupChange.Actions for modify** |
| `create_member_presentation` | the ZK proof to add a member |
| `GroupsManager::get_authorization_for_today` | the group **auth credential** |
| `fetch_encrypted_group` + `decrypt_group` | read (done — we already display groups) |

So the encryption + action-building is **done**. The gaps are plumbing + one
credential fetch.

## The actual gaps (3)

1. **HTTP write verbs** — `push_service` has only `get_group` (`GET /v1/groups/`).
   Add `create_group` (`PUT /v1/groups/`, body = encrypted `proto::Group`) and
   `modify_group` (`PATCH /v1/groups/`, body = `GroupChange.Actions`), mirroring
   `get_group`. Returns `GroupChangeResponse`.
2. **Self profile-key credential** — `encrypt_group_with_credentials` needs our
   own `ExpiringProfileKeyCredential`. Only `retrieve_profile` exists today; add
   a versioned-profile credential request (server returns it for our own ACI +
   profile key). One fetch, cached. **Adding *other* full members** needs *their*
   credential too — defer by **inviting them as pending members** (v1), which
   needs only their encrypted service-id, no credential.
3. **presage Manager wrappers** — presage exposes only `send_message_to_group`.
   Add `create_group_v2(name, invite_acis)`, `update_group_*`, `leave_group`:
   generate a random `GroupMasterKey` → `GroupSecretParams` → auth credential →
   `encrypt_group_with_credentials` → `push_service.create_group` → `save_group`
   to the store → send the group-context update message.

## v1 scope (create + leave; defer full add-member)

- **Create group** — name + pick contacts (invited as pending). Sidesteps the
  per-member credential fetch entirely. This is how Signal invites work.
- **Leave group** — `build` a leave action (remove self) → `modify_group`.
- **Rename / avatar / add-full-member** — v2, once the credential-fetch path is
  in (needs `retrieve_profile` to request profile-key credentials).

## Work breakdown

| Repo | Change | Size |
|---|---|---|
| calibrae/libsignal-service-rs | `create_group`/`modify_group` HTTP on push_service; `GroupsManager::create_group(secret_params, title, self_cred, invites)` wiring the existing builders; self profile-key-credential request | M |
| calibrae/presage | `Manager::create_group_v2` / `leave_group` wrappers (master-key gen, store save, group-context message) | M |
| piccione | `create_group` / `leave_group` commands + a "New group" UI (name + contact multi-select) + leave button in group-info | S/M |

## Validation

Creating a group hits Signal's live group server — **cannot be unit-tested**.
Build it compile-verified end-to-end, mark `[LIVE-TEST]`, validate by creating a
real group from a linked Piccione (like the voice-call + Backups seams).

## Estimate

~1 week given the ZK builders exist: most effort is the two fork PRs
(libsignal-service-rs HTTP + presage wrappers) and the self-credential fetch.
Create + leave first; rename/add-full-member follow once credential-fetch lands.

## Status / progress

- ✅ **HTTP write verbs** — `create_group` (`PUT /v1/groups/`) + `modify_group`
  (`PATCH /v1/groups/`) added to `push_service`. **calibrae/libsignal-service-rs#1,
  merged to `storage-service`.** Piccione builds clean against it.
- ⏭️ **Next: `GroupsManager::create_group` wiring** — call
  `encrypt_group_with_credentials(title, "", None, self_credential, candidates, rng)`
  → `push_service.create_group`. Blocked on:
- ⛔ **Self `ExpiringProfileKeyCredential` fetch** — the one missing primitive.
  `encrypt_group_with_credentials` needs our own credential (to put ourselves in
  the group as admin). Only `retrieve_profile` exists; need a versioned-profile
  **credential request** (zkgroup `ProfileKeyCredentialRequestContext` +
  `GET /v1/profile/{aci}/{version}/{credentialRequest}`). This is the next
  sub-task; once it lands, `create_group` (members invited as pending) + `leave`
  are a short hop, then the presage wrapper + Piccione UI.

Cross-repo state: **2 fork PRs merged** — calibrae/presage#1 (AEP persist, for
Backups) + calibrae/libsignal-service-rs#1 (group write verbs).
