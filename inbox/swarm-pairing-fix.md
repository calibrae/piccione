# swarm-pairing-fix — phase report

Branch: `feat/pairing-fix` (off main 26c2593). Tests: 50 → 55. No regressions.

## Phase 1 — research
Findings in `inbox/presage-linking-research.md`. Punchline: presage's
`link_secondary_device` runs the whole linking protocol in one linear
future — `link_device` opens a websocket, sends the URL via mpsc, waits
for the protocol message, sends `NewDeviceRegistration`, returns. Persistence
(`set_aci_identity_key_pair`, `set_pni_identity_key_pair`,
`save_registration_data`) happens **inside** `link_secondary_device` before
it returns the `Manager`. Caller does not need a follow-up persist call. The
canonical caller (`presage-cli`) uses **multi-thread runtime + LocalSet** and
**no timeout**.

The error string Cali saw — `failed to provision device: no provisioning
message received` — is `presage::Error::ProvisioningError(ProvisioningError::MissingMessage)`,
emitted by `libsignal_service::provisioning::link_device` when the
provisioning websocket's stream returns `None` before the second
`ProvisioningStep::Message` arrives. That happens when the WS is dropped
mid-flight — e.g. by an outer cancellation.

## Phase 2 — bug
The old `pair_once.rs` wrapped the linking future in
`tokio::time::timeout(120s, future::join(pair_fut, qr_fut))`. 120s is too
tight for a wrist-scan + identity exchange when Cali is AFH; cancellation
drops the websocket inside `link_device`, killing it after the URL was sent
but before phase 2. Secondary issue: `current_thread` runtime + an unused
`LocalSet` (no `spawn_local` happens), no post-link verification.

## Phase 3 — rewrite
- New module `src/pair_flow.rs` (pub) factors the timeout/QR/join dance into
  a `run_pair(timeout, link_fn, on_qr)` that's testable.
- `pair_once.rs` now uses **multi_thread runtime + LocalSet** (matches
  presage-cli), 300s outer timeout, and `run_pair`.
- After link returns Ok, drops the manager and the LocalSet, then
  **re-opens** the SqliteStore from scratch and calls
  `Manager::load_registered`. Only on success do we print `PAIR_OK`.
  Anything else prints `ERROR <msg>` and exits 1.

## Phase 4 — helpers
- `src/bin/is_paired.rs` → `is-paired`: opens store, `Manager::load_registered`,
  prints `PAIRED` / `NOT_PAIRED`. `ERROR` only on store I/O failure.
- `src/bin/list_devices.rs` → `list-devices`: loads registered manager,
  `manager.devices()`, prints `DEVICE <id> <created> <*|->name` per device,
  trailing `OK <count>`.

## Phase 5 — tests
`pair_flow` ships 5 unit tests covering timeout, link error pass-through,
success+QR, QR-render failure, and link-completes-before-QR-send. Total
suite: 50 → 55 passing. `Manager::link_secondary_device` is stubbed at the
`run_pair` boundary via a `FnOnce(oneshot::Sender<Url>) -> Future<…>` so we
never need a real Signal account.

## Phase 6 — receive loop (verification only)
`messaging/service.rs::try_load_and_start` already does the right thing on a
registered store: open via `SqliteStore::open_with_passphrase`, call
`Manager::load_registered`, then start the receive loop. After `pair-once`
verifies persistence by re-opening and `load_registered`-ing the store
itself, the next launch of the Tauri app will hit the same code path and
succeed. The misleading log line `keeping existing database (… B) — may
need re-link` is genuinely misleading when the load failure is actually a
passphrase mismatch (Keychain vs file-based `.db_key`). **Out of scope to
fix here**, but flagged: today the file-based fallback in `pair_once` and
the Keychain-based passphrase used by the Tauri app at runtime can diverge
silently, and any subsequent `pair-once` run will hit `code 26 file is not
a database` against the real signalui.db. Recommend converging both paths
on `signalui_lib::store::keychain::get_or_create_db_passphrase` in a
follow-up swimlane.

## Remaining blockers
None for the pairing flow itself. Live verification against a real Signal
account requires Cali to scan; pair-once will print `QR_READY <path>` and
then either `PAIR_OK` or `ERROR <reason>` from a deterministic place.
