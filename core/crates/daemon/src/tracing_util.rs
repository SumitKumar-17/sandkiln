//! Keeps a `tracing::Span`'s context intact across a
//! `tokio::task::spawn_blocking` thread boundary.
//!
//! `tracing`'s "current span" is thread-local; a fresh blocking-pool
//! thread starts with none, so a bare `spawn_blocking` silently drops
//! span context — every event inside `sandkiln-vmm`'s `Vm::boot`/`call`/
//! `stop` (always called from inside `spawn_blocking`) would log with no
//! request correlation otherwise.
//!
//! Two separate pieces of thread-local state, both needed: the span
//! itself, and the "current default dispatch"
//! (`tracing::dispatcher::get_default`) the `tracing::info!`-style macros
//! use to resolve which subscriber to talk to. Re-entering the span alone
//! isn't enough on a thread with no default dispatch configured — this
//! helper captures and re-establishes both, rather than relying on
//! `main.rs`'s process-global default subscriber, so it still works
//! wherever it's used. The test below proves both are actually necessary
//! by exercising this with no global subscriber installed at all.

/// Runs `f` on a blocking-pool thread (via `tokio::task::spawn_blocking`)
/// inside the span and dispatcher that were active on the calling task,
/// so any `tracing` events `f` emits — directly or via a library it calls
/// into, like `sandkiln-vmm` — are correlated with whatever request
/// triggered it. `panic_message` is used as the `Result::expect` message
/// if `f` panics, matching this crate's existing per-call-site
/// `.expect("... task panicked")` convention.
pub async fn spawn_blocking_in_current_span<F, R>(panic_message: &'static str, f: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let span = tracing::Span::current();
    let dispatch = tracing::dispatcher::get_default(tracing::Dispatch::clone);
    tokio::task::spawn_blocking(move || tracing::dispatcher::with_default(&dispatch, || span.in_scope(f)))
        .await
        .expect(panic_message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::Instrument;

    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedBuf {
        type Writer = Self;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Real regression test, not just a compile check: a throwaway JSON
    /// subscriber, a span carrying a unique `request_id`, an event emitted
    /// via `spawn_blocking_in_current_span` on a real blocking-pool
    /// thread, then asserts the logged JSON carries that `request_id`.
    ///
    /// Uses `tracing::subscriber::set_default` (thread-local), not
    /// `set_global_default` (process-global, settable once — other tests
    /// would break it), held across the whole `.await` since the
    /// single-threaded `#[tokio::test]` runtime runs the test body on that
    /// same thread. The blocking-pool thread `f` actually runs on has no
    /// dispatcher of its own at all, thread-local or global — proving this
    /// module's dispatcher capture is necessary, not just the span.
    #[tokio::test]
    async fn spawn_blocking_in_current_span_carries_span_context_and_events_into_the_new_thread() {
        let buf = SharedBuf::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_writer(buf.clone())
            .with_current_span(true)
            .with_span_list(true)
            .finish();

        // A unique id per test run rather than a global default subscriber
        // (which can only ever be installed once per process, and other
        // tests in this binary log too) — searching the buffer for this
        // exact value is what makes the assertion below unambiguous.
        let request_id = uuid::Uuid::new_v4().to_string();

        let _guard = tracing::subscriber::set_default(subscriber);
        let span = tracing::info_span!("http_request", request_id = %request_id);
        let outcome = async move {
            spawn_blocking_in_current_span("test task panicked", || {
                tracing::info!(from = "blocking closure", "event emitted on the spawned thread");
                7
            })
            .instrument(span)
            .await
        }
        .await;
        drop(_guard);

        assert_eq!(outcome, 7);

        let logged = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(
            logged.contains(&request_id),
            "expected the blocking-thread event to carry request_id {request_id}, got: {logged}"
        );
        assert!(
            logged.contains("event emitted on the spawned thread"),
            "expected the blocking-thread event's message in the log, got: {logged}"
        );
    }

    #[tokio::test]
    async fn spawn_blocking_in_current_span_returns_the_closures_value() {
        let value = spawn_blocking_in_current_span("panic", || 1 + 1).await;
        assert_eq!(value, 2);
    }

    #[tokio::test]
    #[should_panic(expected = "boom")]
    async fn spawn_blocking_in_current_span_propagates_a_panic_via_the_given_message() {
        spawn_blocking_in_current_span("boom", || panic!("closure panicked")).await;
    }
}
