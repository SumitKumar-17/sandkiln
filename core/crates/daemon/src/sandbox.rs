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
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
}
