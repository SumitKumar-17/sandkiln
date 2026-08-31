use crate::state::AttachedDrive;
use sandkiln_vmm::network::Lease;
use sandkiln_vmm::vm::Vm;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Instant, SystemTime};

pub struct Sandbox {
    pub id: String,
    pub vm: Vm,
    /// `None` only for a sandbox forked from a snapshot
    /// (`source_snapshot_id.is_some()`): its network — if the snapshot's
    /// source sandbox had any — stays owned by that snapshot the whole
    /// time (see `Snapshot::forked_into`), never released by *this*
    /// sandbox's teardown. Every other sandbox (created fresh, or
    /// consuming-resumed via `/snapshots/:id/resume`) owns its lease
    /// outright.
    pub network: Option<Lease>,
    /// This sandbox's own copy of the base rootfs image. Removed on stop
    /// — unless `source_snapshot_id` is set, in which case this path is
    /// the snapshot's shared rootfs file, not this sandbox's own, and
    /// must survive so the snapshot can be forked/resumed again.
    pub rootfs_path: PathBuf,
    /// Persistent drives attached at creation (see
    /// `sandkiln_vmm::drive::DriveStore`), each with whether it was
    /// attached read-only — not touched on stop, unlike `rootfs_path`,
    /// since drives are meant to outlive this sandbox. Removing this
    /// sandbox from `AppState::sandboxes` is what "detaches" them: they
    /// become eligible for attaching to a later sandbox again, subject to
    /// `crate::state::can_attach_read_only`'s multi-holder rule.
    pub attached_drives: Vec<AttachedDrive>,
    /// The uid/gid leased from `AppState::jailer_ids` for this sandbox's
    /// VM, if it was booted jailed — `None` for a direct (unjailed) boot.
    /// Released back to the pool in `stop_sandbox_by_id`; `Vm::stop`
    /// itself only knows how to tear down the chroot directory, not this
    /// daemon-level allocation, so the two are released independently.
    pub jail_id: Option<u32>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// Updated on every real interaction (exec/read-file/write-file — see
    /// `routes_exec::call_agent`) and read by `idle_reaper` to decide
    /// whether to stop this sandbox. A `Mutex` rather than a plain field
    /// because `Sandbox` is read through `AppState::sandboxes`, a shared
    /// map behind one lock — individual sandboxes aren't otherwise
    /// mutable through it.
    pub last_activity: Mutex<Instant>,
    /// Set when this sandbox was created by forking a snapshot without
    /// consuming it, rather than by a normal create or a consuming
    /// `/resume` — see `Snapshot::forked_into`, which this is the other
    /// half of: `routes_sandbox::stop_sandbox_by_id` clears that lock on
    /// the referenced snapshot once this sandbox's `Vm` is fully stopped,
    /// and skips deleting `rootfs_path` / releasing `network` here since
    /// neither is owned by this sandbox.
    pub source_snapshot_id: Option<String>,
}
