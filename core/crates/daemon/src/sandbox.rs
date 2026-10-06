use crate::state::{AttachedDrive, Mount};
use sandkiln_vmm::egress::EgressPolicy;
use sandkiln_vmm::network::Lease;
use sandkiln_vmm::vm::Vm;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

pub struct Sandbox {
    pub id: String,
    pub vm: Vm,
    /// `None` only for a fork (`source_snapshot_id.is_some()`): the
    /// source snapshot owns the lease for as long as the fork lives (see
    /// `Snapshot::forked_into`), so this sandbox's teardown never
    /// releases it. Every other sandbox owns its lease outright.
    pub network: Option<Lease>,
    /// Always this sandbox's own private rootfs clone, including a
    /// fork/restore (each gets a fresh clone via
    /// `routes_sandbox::clone_rootfs`, never the source's file directly)
    /// — so `destroy_sandbox_by_id` can always remove it unconditionally.
    /// **Not always true**: forking used to share the source snapshot's
    /// rootfs file directly, a real corruption bug (fork, mutate, stop,
    /// resume the original — its memory disagreed with the file on disk)
    /// found live building time-travel restore. See
    /// `routes_snapshot::fork_snapshot`.
    pub rootfs_path: PathBuf,
    /// Drives attached at creation, with read-only/write mode. Not
    /// touched on stop (drives outlive the sandbox) — removal from
    /// `AppState::sandboxes` is what "detaches" them, per
    /// `crate::state::can_attach_read_only`'s multi-holder rule.
    pub attached_drives: Vec<AttachedDrive>,
    /// Registered image this sandbox's rootfs was cloned from, if any
    /// (`None` = the `SANDKILN_BASE_ROOTFS` default). Checked by
    /// `AppState::image_holder` so `DELETE /images/:id` can refuse to
    /// remove an image a live sandbox depends on.
    pub image_id: Option<String>,
    /// Leased uid/gid if booted jailed, `None` otherwise — released back
    /// to `AppState::jailer_ids` in `stop_sandbox_by_id` (`Vm::stop` only
    /// tears down the chroot, not this daemon-level allocation).
    pub jail_id: Option<u32>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// Updated on every exec/read/write (`routes_exec::call_agent`),
    /// read by `idle_reaper`. A `Mutex` since `Sandbox` lives behind
    /// `AppState::sandboxes`'s one shared-map lock.
    pub last_activity: Mutex<Instant>,
    /// Set when forked from a snapshot without consuming it — the other
    /// half of `Snapshot::forked_into`: `stop_sandbox_by_id` clears that
    /// lock once this `Vm` stops, and skips releasing `network` (borrowed
    /// from the live snapshot, not owned). `rootfs_path` is still this
    /// sandbox's own clone to delete regardless.
    pub source_snapshot_id: Option<String>,
    /// Caller-given identity, unique among live sandboxes and held
    /// snapshots at claim time (`AppState::name_holder`/`lock_name`).
    /// Carried onto the `Snapshot` a stop produces and back onto the next
    /// `Sandbox` on resume/fork, so the name always resolves to whichever
    /// record currently represents it — see `ROADMAP.md`'s "Sandbox vs.
    /// session" note.
    pub name: Option<String>,
    /// Open `GET /sandboxes/:id/pty` sessions, checked/incremented under
    /// `AppState::sandboxes`'s lock (`routes_pty::MAX_PTY_SESSIONS_PER_SANDBOX`),
    /// decremented via an RAII guard on session end. `Arc` because the
    /// guard can outlive this `Sandbox`'s reachability through the map.
    pub pty_session_count: Arc<AtomicU32>,
    /// Set when this sandbox counts against a pool's `max_count` (warm
    /// claim or cold-create under headroom — see `crate::pool`). Released
    /// on stop by whichever path removes it. **Not** carried onto the
    /// resulting `Snapshot` — a later resume/fork is a fresh creation
    /// event, matched against whatever pool applies then.
    pub source_pool_id: Option<String>,
    /// Egress policy if set at create time (see `sandkiln_vmm::egress`).
    /// `None` for a fork, same ownership convention as `network`: the
    /// iptables chain is tied to the lease the snapshot owns — see
    /// `Snapshot::egress` for the copy actually (re-)applied on
    /// resume/fork.
    pub egress: Option<EgressPolicy>,
    /// Baked in at create time as the base layer under each
    /// exec/exec-stream call's own `env` (call wins on conflict — see
    /// `routes_exec::resolve_env`). Unlike `egress`, no external resource
    /// to re-apply, so it carries through resume **and** fork identically.
    pub env: HashMap<String, String>,
    /// Informational lineage pointer, carried onto `Snapshot::parent_snapshot_id`
    /// if this sandbox is later snapshotted. **A separate field from
    /// `source_snapshot_id`, not a reuse**: that field is `Some` only on
    /// a fork (to refuse re-snapshotting it) and `None` on resume (so a
    /// resumed sandbox stays snapshottable) — lineage needs the opposite
    /// shape, `Some` on both. Reusing the wrong field would have made
    /// every resumed sandbox's lineage a dead end; caught live, not on
    /// paper (`ROADMAP.md`'s "Snapshot lineage").
    pub parent_snapshot_id: Option<String>,
    /// Active remote-storage mounts (`crate::routes_mounts`). Never
    /// re-applied on resume/fork/restore — a mount is a live guest FUSE
    /// process, already captured whole by Firecracker's own snapshot.
    /// Exists purely so `GET /sandboxes/:id/mounts` can list without a
    /// live guest round-trip.
    pub mounts: Vec<Mount>,
    /// Streamed background exec sessions (`kiln logs -f`, see
    /// `crate::routes_logs`). Not carried across resume/fork/restore,
    /// same as `pty_session_count` — always empty on a fresh `Sandbox`.
    pub log_sessions: Mutex<HashMap<String, Arc<crate::log_session::LogSession>>>,
}
