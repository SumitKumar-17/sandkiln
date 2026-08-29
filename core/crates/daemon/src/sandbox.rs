use sandkiln_vmm::network::Lease;
use sandkiln_vmm::vm::Vm;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

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
}
