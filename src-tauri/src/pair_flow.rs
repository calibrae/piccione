//! Headless pairing flow shared by `pair-once` and friends.
//!
//! Wraps `Manager::link_secondary_device` with:
//! - a long, configurable outer timeout (presage-cli has none; we want a
//!   watchdog so a stuck websocket can't hang the binary forever)
//! - a `link_fn` indirection so we can unit-test the timeout / QR / success
//!   paths without standing up a real Signal account
//!
//! See `inbox/presage-linking-research.md` for the upstream semantics.
use std::future::Future;
use std::time::Duration;
use std::pin::Pin;

use futures::channel::oneshot;
use futures::future;
use url::Url;

/// Outcome of a single pairing attempt.
#[derive(Debug)]
pub enum PairOutcome<M> {
    Success(M),
    /// Outer watchdog fired (presage's own timeout machinery is via
    /// `Error::Timeout` but we add a belt-and-braces watchdog too).
    Timeout(u64),
    /// Anything else: presage error string, QR render failure, etc.
    Failed(String),
}

/// The two events the QR handler may need to signal back to the caller.
pub enum QrResult {
    Rendered,
    Failed(String),
}

/// Run a pairing attempt.
///
/// `link_fn` is the boundary we stub in tests — in production it's a thin
/// wrapper around `Manager::link_secondary_device` that returns a `Result<M,
/// String>`. `on_qr` is invoked synchronously the moment we receive the
/// provisioning URL — in production it renders a PNG / prints to stdout.
///
/// The two futures are composed with `futures::future::join` (matching
/// presage-cli) and the whole join is wrapped in `tokio::time::timeout` so a
/// stuck websocket can't hang us.
pub async fn run_pair<M, LinkFn, LinkFut, QrFn>(
    timeout: Duration,
    link_fn: LinkFn,
    on_qr: QrFn,
) -> PairOutcome<M>
where
    LinkFn: FnOnce(oneshot::Sender<Url>) -> LinkFut,
    LinkFut: Future<Output = Result<M, String>>,
    QrFn: FnOnce(Url) -> QrResult,
{
    let (qr_tx, qr_rx) = oneshot::channel::<Url>();

    let pair_fut = link_fn(qr_tx);
    let qr_fut = async move {
        match qr_rx.await {
            Ok(url) => match on_qr(url) {
                QrResult::Rendered => Ok(()),
                QrResult::Failed(e) => Err(e),
            },
            Err(_) => Err("provisioning URL channel cancelled".to_string()),
        }
    };

    let joined: Pin<Box<dyn Future<Output = (Result<M, String>, Result<(), String>)>>> =
        Box::pin(future::join(pair_fut, qr_fut));

    match tokio::time::timeout(timeout, joined).await {
        Err(_) => PairOutcome::Timeout(timeout.as_secs()),
        Ok((Err(link_err), _)) => PairOutcome::Failed(link_err),
        Ok((Ok(_), Err(qr_err))) => {
            // The link succeeded but our QR side blew up — this is still a
            // failure mode worth surfacing because the user couldn't have
            // scanned anything sensible.
            PairOutcome::Failed(format!("qr handler: {qr_err}"))
        }
        Ok((Ok(mgr), Ok(()))) => PairOutcome::Success(mgr),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// A dummy "manager" so we can flow through the success path without
    /// pulling in a real presage Manager (which is parametric over !Send
    /// futures).
    #[derive(Debug, PartialEq, Eq)]
    struct StubManager(&'static str);

    fn dummy_url() -> Url {
        Url::parse("tsdevice:/?uuid=00000000-0000-0000-0000-000000000000&pub_key=AAAA").unwrap()
    }

    #[tokio::test]
    async fn timeout_fires_when_link_hangs() {
        // link_fn never resolves — simulating a websocket that sent the URL
        // but never gets the second message.
        let outcome = run_pair::<StubManager, _, _, _>(
            Duration::from_millis(50),
            |qr_tx| async move {
                let _ = qr_tx.send(dummy_url());
                // hang forever
                std::future::pending::<Result<StubManager, String>>().await
            },
            |_| QrResult::Rendered,
        )
        .await;

        match outcome {
            PairOutcome::Timeout(_) => {}
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn surfaces_link_error_verbatim() {
        let outcome = run_pair::<StubManager, _, _, _>(
            Duration::from_secs(5),
            |qr_tx| async move {
                let _ = qr_tx.send(dummy_url());
                Err::<StubManager, _>("failed to provision device: no provisioning message received".into())
            },
            |_| QrResult::Rendered,
        )
        .await;

        match outcome {
            PairOutcome::Failed(msg) => {
                assert!(
                    msg.contains("no provisioning message received"),
                    "got: {msg}"
                );
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn success_path_invokes_qr_handler() {
        let qr_seen = Arc::new(AtomicBool::new(false));
        let qr_seen_2 = qr_seen.clone();

        let outcome = run_pair::<StubManager, _, _, _>(
            Duration::from_secs(5),
            |qr_tx| async move {
                qr_tx.send(dummy_url()).expect("qr_tx open");
                Ok(StubManager("paired"))
            },
            move |_url| {
                qr_seen_2.store(true, Ordering::SeqCst);
                QrResult::Rendered
            },
        )
        .await;

        assert!(qr_seen.load(Ordering::SeqCst), "qr handler must have run");
        match outcome {
            PairOutcome::Success(StubManager("paired")) => {}
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn qr_render_failure_marks_attempt_failed() {
        let outcome = run_pair::<StubManager, _, _, _>(
            Duration::from_secs(5),
            |qr_tx| async move {
                qr_tx.send(dummy_url()).expect("qr_tx open");
                Ok(StubManager("paired"))
            },
            |_| QrResult::Failed("disk full".into()),
        )
        .await;

        match outcome {
            PairOutcome::Failed(msg) => assert!(msg.contains("disk full"), "got: {msg}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn link_completing_before_qr_send_is_failure() {
        // pathological: link_fn returns Ok without ever sending the URL.
        // qr_rx therefore returns Err and the QR handler reports failure.
        let outcome = run_pair::<StubManager, _, _, _>(
            Duration::from_secs(5),
            |_qr_tx| async move { Ok(StubManager("never-saw-qr")) },
            |_| QrResult::Rendered,
        )
        .await;

        match outcome {
            PairOutcome::Failed(msg) => {
                assert!(msg.contains("cancelled") || msg.contains("qr handler"), "got: {msg}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
