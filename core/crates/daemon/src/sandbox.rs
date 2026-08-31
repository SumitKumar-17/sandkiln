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
    /// Id of the registered image (see `sandkiln_vmm::image::ImageStore`
    /// and `routes_images`) this sandbox's `rootfs_path` was cloned from,
    /// if it booted from one via `POST /sandboxes`'s `image_id` field.
    /// `None` means it booted from the daemon-wide `SANDKILN_BASE_ROOTFS`
    /// default instead — today's unchanged behavior. Checked by
    /// `AppState::image_holder` so `DELETE /images/:id` can refuse to
    /// remove an image a live sandbox was booted from.
    pub image_id: Option<String>,
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
    /// Caller-given identity, unique among live sandboxes and held
    /// snapshots at the moment it was claimed (see
    /// `AppState::name_holder`/`AppState::lock_name`). `None` for a
    /// sandbox created without one — naming is opt-in, not required.
    /// Carried forward onto the `Snapshot` record this sandbox becomes on
    /// stop (`routes_snapshot::snapshot_and_stop`) and back onto a new
    /// `Sandbox` on resume/fork, so the same name keeps resolving to
    /// whichever record currently represents this identity — see
    /// `ROADMAP.md`'s "Sandbox vs. session" note: the name identifies the
    /// persistent thing, not any one running instance of it.
    pub name: Option<String>,
}
