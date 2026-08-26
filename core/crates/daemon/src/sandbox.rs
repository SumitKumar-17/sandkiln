use sandkiln_vmm::vm::Vm;
use std::path::PathBuf;
use std::time::SystemTime;

pub struct Sandbox {
    pub id: String,
    pub vm: Vm,
    /// This sandbox's own copy of the base rootfs image, removed on stop.
    pub rootfs_path: PathBuf,
    pub created_at: SystemTime,
}
