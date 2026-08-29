use sandkiln_vmm::network::Lease;
use sandkiln_vmm::vm::Vm;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Instant, SystemTime};

pub struct Sandbox {
    pub id: String,
    pub vm: Vm,
    pub network: Lease,
    /// This sandbox's own copy of the base rootfs image, removed on stop.
    pub rootfs_path: PathBuf,
    /// Ids of persistent drives attached at creation (see
    /// `sandkiln_vmm::drive::DriveStore`) — not touched on stop, unlike
    /// `rootfs_path`, since drives are meant to outlive this sandbox.
    /// Removing this sandbox from `AppState::sandboxes` is what "detaches"
    /// them: they become eligible for attaching to a later sandbox again.
    pub attached_drives: Vec<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// Updated on every real interaction (exec/read-file/write-file — see
    /// `routes_exec::call_agent`) and read by `idle_reaper` to decide
    /// whether to stop this sandbox. A `Mutex` rather than a plain field
    /// because `Sandbox` is read through `AppState::sandboxes`, a shared
    /// map behind one lock — individual sandboxes aren't otherwise
    /// mutable through it.
    pub last_activity: Mutex<Instant>,
}
