use crate::config::Config;
use crate::metrics::Metrics;
use crate::routes_preview::PreviewClient;
use crate::sandbox::Sandbox;
use crate::snapshot::Snapshot;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use sandkiln_vmm::drive::DriveStore;
use sandkiln_vmm::jailer::JailerIdPool;
use sandkiln_vmm::network::NetworkManager;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

/// One drive attached to a sandbox or a held snapshot, and whether it was
/// attached read-only. `Sandbox::attached_drives` and
/// `Snapshot::attached_drives` both carry this rather than a bare drive
/// id, because whether a *new* attach may coexist with the existing ones
/// depends on both pieces of information — see `can_attach_read_only`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedDrive {
    pub drive_id: String,
    pub read_only: bool,
}

pub struct AppState {
    pub config: Config,
    pub network: NetworkManager,
    pub drives: DriveStore,
    /// `Some` exactly when `config.jailer` is `Some` — the pool of
    /// uid/gid pairs `routes_sandbox::create_sandbox` leases from for
    /// each jailed boot, released back on `stop_sandbox_by_id`. Built
    /// here rather than passed in separately since its range comes
    /// straight out of `config.jailer`.
    pub jailer_ids: Option<JailerIdPool>,
    pub sandboxes: Mutex<HashMap<String, Sandbox>>,
    pub snapshots: Mutex<HashMap<String, Snapshot>>,
    pub metrics: Metrics,
    /// Reused across every `/preview` proxy request rather than built
    /// per-request, so repeated hits on one dev server benefit from
    /// `hyper-util`'s connection pooling instead of a fresh TCP handshake
    /// (and, for a WebSocket-using dev server later, from a client
    /// already wired for keep-alive) every time.
    pub preview_client: PreviewClient,
}

impl AppState {
    /// `snapshots` is the result of `crate::snapshot::reconcile` run
    /// against the on-disk snapshot store before this is called — passed
    /// in rather than always starting empty so a daemon restart doesn't
    /// silently orphan every snapshot that was durable on disk (see
    /// `main.rs`).
    pub fn new(config: Config, network: NetworkManager, drives: DriveStore, snapshots: HashMap<String, Snapshot>) -> Self {
        let jailer_ids = config.jailer.as_ref().map(|j| JailerIdPool::new(j.uid_gid_range.clone()));
        Self {
            config,
            network,
            drives,
            jailer_ids,
            sandboxes: Mutex::new(HashMap::new()),
            snapshots: Mutex::new(snapshots),
            metrics: Metrics::new(),
            preview_client: build_preview_client(),
        }
    }

    /// Every current holder of `drive_id` — running sandboxes and held
    /// snapshots with it frozen into saved state (Firecracker bakes a
    /// drive's host path into the snapshot the same way it does network
    /// config, so a snapshotted drive is still "in use" even though no
    /// `Vm` is running) — each labeled and marked with whether that
    /// particular attachment is read-only. Empty means nothing holds it.
    ///
    /// Checked wherever an operation would conflict with the drive still
    /// being held: attaching it to another sandbox (via
    /// `can_attach_read_only`, since many simultaneous read-only holders
    /// are fine) or deleting it outright (never fine while this is
    /// non-empty, regardless of read-only status).
    pub fn drive_holders(&self, drive_id: &str) -> Vec<DriveHold> {
        let mut holders: Vec<DriveHold> = self
            .sandboxes
            .lock()
            .unwrap()
            .values()
            .filter_map(|s| {
                s.attached_drives
                    .iter()
                    .find(|d| d.drive_id == drive_id)
                    .map(|d| DriveHold { holder: format!("sandbox {}", s.id), read_only: d.read_only })
            })
            .collect();
        holders.extend(self.snapshots.lock().unwrap().values().filter_map(|s| {
            s.attached_drives
                .iter()
                .find(|d| d.drive_id == drive_id)
                .map(|d| DriveHold { holder: format!("snapshot {}", s.id), read_only: d.read_only })
        }));
        holders
    }
}

/// One thing currently holding a drive (a running sandbox or a held
/// snapshot), and whether it holds it read-only. See `AppState::drive_holders`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveHold {
    pub holder: String,
    pub read_only: bool,
}

/// The multi-holder rule at the heart of this feature: a drive may be
/// attached to arbitrarily many holders at once, but only if every
/// existing holder *and* the new attach being requested are all
/// read-only. A single read-write attachment — existing or requested —
/// needs exclusive, single-holder access, exactly like every attachment
/// did before read-only sharing existed. Pulled out of
/// `AppState::drive_holders`'s callers so it's directly testable without
/// `AppState`, a mutex, or axum.
pub fn can_attach_read_only(existing: &[bool], requesting_read_only: bool) -> bool {
    existing.is_empty() || (requesting_read_only && existing.iter().all(|ro| *ro))
}

/// Renders a list of `DriveHold`s into the human-readable form used in
/// `AppError::Conflict` messages and nowhere else — kept next to
/// `DriveHold` rather than duplicated at each call site.
pub fn describe_drive_holders(holders: &[DriveHold]) -> String {
    holders
        .iter()
        .map(|h| if h.read_only { format!("{} (read-only)", h.holder) } else { h.holder.clone() })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A short connect timeout is what actually turns "guest port isn't
/// listening" into a fast, clear error — a refused connection fails
/// immediately either way, but a black-holed one (SYN silently dropped,
/// e.g. a guest firewall rule) would otherwise hang until the request-level
/// timeout in `Config::preview_timeout`, which is tuned for slow dev-server
/// compiles, not connection setup.
fn build_preview_client() -> PreviewClient {
    let mut connector = HttpConnector::new();
    connector.set_connect_timeout(Some(Duration::from_secs(5)));
    hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(connector)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_attach_read_only_allows_the_first_attach_regardless_of_mode() {
        assert!(can_attach_read_only(&[], true));
        assert!(can_attach_read_only(&[], false));
    }

    #[test]
    fn can_attach_read_only_allows_stacking_more_read_only_holders() {
        assert!(can_attach_read_only(&[true], true));
        assert!(can_attach_read_only(&[true, true], true));
    }

    #[test]
    fn can_attach_read_only_rejects_a_read_write_request_while_anything_holds_it() {
        assert!(!can_attach_read_only(&[true], false));
        assert!(!can_attach_read_only(&[true, true], false));
    }

    #[test]
    fn can_attach_read_only_rejects_a_read_only_request_while_any_holder_is_read_write() {
        assert!(!can_attach_read_only(&[false], true));
        // Mixed existing holders: one read-only, one read-write — the
        // read-write one alone is enough to force exclusivity.
        assert!(!can_attach_read_only(&[true, false], true));
    }

    #[test]
    fn can_attach_read_only_rejects_read_write_onto_a_read_write_holder() {
        assert!(!can_attach_read_only(&[false], false));
    }

    #[test]
    fn describe_drive_holders_marks_read_only_holders_and_leaves_read_write_ones_bare() {
        let holders = vec![
            DriveHold { holder: "sandbox a".to_string(), read_only: true },
            DriveHold { holder: "sandbox b".to_string(), read_only: false },
        ];
        assert_eq!(describe_drive_holders(&holders), "sandbox a (read-only), sandbox b");
    }

    #[test]
    fn describe_drive_holders_of_an_empty_list_is_an_empty_string() {
        assert_eq!(describe_drive_holders(&[]), "");
    }
}
