//! Orderly daemon shutdown.
//!
//! Without it, a restart (or a plain `SIGTERM`) left every sandbox's
//! Firecracker process running with nothing owning it: leaked memory, leaked
//! tap devices, and sandboxes lost for good. Snapshots are already durable and
//! are reconciled at startup, so shutdown does what "persistent by default"
//! promises: it snapshots each live sandbox, and only destroys the ones that
//! structurally cannot be snapshotted (jailed ones).

use crate::http::routes::sandbox::{stop_sandbox_by_id, StopError};
use crate::state::AppState;
use futures_util::future::join_all;
use std::sync::Arc;
use std::time::Duration;

/// How long in-flight HTTP requests get to finish after the signal, and then
/// how long draining sandboxes may take, before the daemon exits anyway.
pub const REQUEST_GRACE: Duration = Duration::from_secs(30);
pub const DRAIN_DEADLINE: Duration = Duration::from_secs(60);

/// Resolves on `SIGINT` or `SIGTERM`.
pub async fn signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = term.recv() => tracing::info!("SIGTERM received, shutting down"),
        _ = int.recv() => tracing::info!("SIGINT received, shutting down"),
    }
}

/// Snapshots (or, failing that, destroys) every live sandbox, concurrently.
pub async fn drain_sandboxes(state: Arc<AppState>) {
    let ids: Vec<String> = state.sandboxes.lock().unwrap().keys().cloned().collect();
    if ids.is_empty() {
        return;
    }
    tracing::info!(count = ids.len(), "snapshotting live sandboxes before exit");
    let work = join_all(ids.into_iter().map(|id| {
        let state = state.clone();
        async move {
            let outcome = match stop_sandbox_by_id(state.clone(), id.clone(), true).await {
                Err(StopError::CannotPreserve(_)) => stop_sandbox_by_id(state, id.clone(), false).await.map(|_| "destroyed"),
                other => other.map(|_| "snapshotted"),
            };
            match outcome {
                Ok(what) => tracing::info!(sandbox_id = %id, what, "sandbox handled at shutdown"),
                Err(StopError::NotFound) => {}
                Err(_) => tracing::warn!(sandbox_id = %id, "failed to stop sandbox at shutdown"),
            }
        }
    }));
    if tokio::time::timeout(DRAIN_DEADLINE, work).await.is_err() {
        tracing::warn!(deadline_secs = DRAIN_DEADLINE.as_secs(), "drain deadline reached; exiting with sandboxes still stopping");
    }
}
