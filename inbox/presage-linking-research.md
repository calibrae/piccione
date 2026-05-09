# presage `link_secondary_device` — actual semantics

Source: `~/Developer/perso/_research/presage/presage/src/manager/linking.rs`
       + `~/.cargo/git/checkouts/libsignal-service-rs-7e457f3dfb9f3190/3d07d8d/src/provisioning/mod.rs` (`link_device`)
       + `~/Developer/perso/_research/presage/presage-cli/src/main.rs` (canonical caller)

## Two phases over one mpsc(1)

`Manager::link_secondary_device(store, servers, name, qr_oneshot)`:

1. **Clears registration** in the store *immediately* (so old creds are gone the moment we start).
2. Generates a 24-char password + 52-byte signaling key.
3. Builds a `PushService` and an `mpsc::channel(1)` (`tx → link_device`, `rx ← inner async block`).
4. Drives `future::join(link_device(tx), inner_async(rx, qr_oneshot))`:
   - `link_device` opens a websocket to `/v1/websocket/provisioning/`, reads two `ProvisioningStep`s from the stream, and sends two messages through `tx`:
     - `Url(tsdevice://…)`
     - `NewDeviceRegistration { phone_number, service_ids, device_id, registration_id, pni_registration_id, aci/pni keypairs, profile_key }`
     - Both `tx.send().await.expect(...)` — *will panic* if rx is dropped early.
   - `inner_async` reads first message, forwards URL via the **caller-supplied `oneshot::Sender<Url>`** (this is what we render as a QR), then reads the second and returns the registration data.
5. After join: `wait_for_qrcode_scan?` propagates `link_device` errors first. If WS dropped before the user scanned, `link_device` returns `ProvisioningError::MissingMessage` which presage wraps as `Error::ProvisioningError(...)` and displays as **`"failed to provision device: no provisioning message received"`** — same string we see, but it's the libsignal MissingMessage, *not* presage's `NoProvisioningMessageReceived`.
6. On success: persists ACI + PNI identity keypairs (`set_aci_identity_key_pair`, `set_pni_identity_key_pair`) and `save_registration_data(...)` **inside `link_secondary_device` itself**. Returns `Manager<S, Registered>`. Persistence is done before return — caller does not need to invoke anything else.

## What it does NOT do

- It does not run any background task. `link_secondary_device` is a single linear future. Nothing keeps the websocket alive past the function.
- It does not retry. WS dies → MissingMessage → done.
- It does not need an outer `LocalSet` at the presage level — `link_device` doesn't `tokio::spawn`. presage-cli uses `LocalSet` only because *other* Manager methods (receive loop, sender) are `!Send`.

## Canonical caller (presage-cli)

```rust
#[tokio::main(flavor = "multi_thread")]   // multi-thread runtime
async fn main() {
    let store = SqliteStore::open_with_passphrase(...).await?;
    let local = tokio::task::LocalSet::new();
    local.run_until(run(args, store)).await
}

// in run(): NO timeout wrapper, NO double LocalSet
let (tx, rx) = oneshot::channel();
let manager = future::join(
    Manager::link_secondary_device(store, servers, name, tx),
    async move { match rx.await { Ok(url) => qr2term::print_qr(url.to_string())..., Err(_) => ... } },
).await;
match manager { (Ok(m), _) => m.whoami().await?, (Err(e), _) => println!("{e:?}") }
```

## Verification primitive

`Manager::load_registered(store).await` re-opens the store and checks that the registration row + identity keys are there. It is the right cheap assertion that the link actually persisted to disk — call it on a *fresh* `SqliteStore::open_with_passphrase(...)` after dropping the linking manager to be sure WAL is committed/visible.
