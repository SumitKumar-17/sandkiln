//! Keeps a `tracing::Span`'s context intact across a
//! `tokio::task::spawn_blocking` thread boundary.
//!
//! `tracing`'s "current span" bookkeeping is thread-local, and
//! `tokio::task::spawn_blocking(f)` runs `f` on a fresh blocking-pool
//! thread that starts with none — a bare `spawn_blocking` silently drops
//! span context, so every `tracing::info!`/`debug!`/`warn!` call inside
//! `sandkiln-vmm`'s `Vm::boot`/`call`/`stop` (all invoked from inside
//! `spawn_blocking` — see `routes_sandbox`/`routes_exec`/
//! `routes_snapshot`/`routes_drives`) would otherwise log with no request
//! context at all, defeating request/trace correlation
//! (`request_id::correlate`) for exactly the operations that matter most.
//!
//! `tracing::Span::enter`'s own documentation ("In Asynchronous Code")
//! confirms spans don't propagate on their own and must be re-entered
//! explicitly; what it doesn't spell out is that `tracing::Span::current()`
//! and the `tracing::info!`/etc. macros both resolve *which subscriber to
//! talk to* via the thread-local "current default dispatch"
//! (`tracing::dispatcher::get_default`) — a completely separate piece of
//! thread-local state from the span itself. Re-entering the span alone is
//! not sufficient on a thread with no default dispatch configured (i.e. a
//! blocking-pool thread, unless a *global* default subscriber happens to
//! be installed process-wide via `set_global_default`, which `main.rs`
//! does, but this helper doesn't rely on that in case it's ever used
//! somewhere it isn't — see `spawn_blocking_in_current_span_carries_
//! span_context_and_events_into_the_new_thread` below, which exercises
//! this without a global subscriber, proving both pieces are actually
//! necessary and sufficient).

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

    /// Proves the propagation this module exists for actually works, by
    /// building a throwaway (not process-global) JSON subscriber, entering
    /// a span carrying a unique `request_id`, running an event through
    /// `spawn_blocking_in_current_span` on a real blocking-pool thread, and
    /// asserting that event's logged JSON carries that `request_id` in its
    /// span context — i.e. this is a real regression test for the thread
    /// boundary, not just an assertion that the code compiles.
    ///
    /// Deliberately uses `tracing::subscriber::set_default` (a thread-local
    /// override) rather than `set_global_default` (process-global, and
    /// settable only once per process — other tests in this binary would
    /// break it). That override only applies to the thread it's set on, so
    /// it's held across the whole `.await` below rather than dropped early
    /// — the default `#[tokio::test]` runtime is single-threaded, so the
    /// test body and everything it polls up to the `spawn_blocking` call
    /// itself all run on that one thread. This is exactly why
    /// `spawn_blocking_in_current_span` also captures and re-establishes
    /// the *dispatcher* (not just the span) inside the spawned closure: the
    /// blocking-pool thread `f` actually runs on is a genuinely different
    /// OS thread than the one this guard is scoped to, and has no
    /// dispatcher of its own at all, thread-local or global, in this test.
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
