use sandkiln_vmm::network::Lease;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

/// A saved, stopped microVM: memory and device state on disk, ready to be
/// resumed into a new live sandbox. This is what a sandbox becomes
/// instead of being fully torn down — no `Vm`, no running process, but
/// not gone either.
pub struct Snapshot {
    pub id: String,
    /// The id of the sandbox this was taken from. Purely informational —
    /// that sandbox no longer exists by the time a `Snapshot` exists.
    pub source_sandbox_id: String,
    /// The state file written by `Vm::snapshot`.
    pub snapshot_path: PathBuf,
    /// The guest-memory file written by `Vm::snapshot`.
    pub mem_file_path: PathBuf,
    /// The rootfs image carried over unmodified from the sandbox that
    /// made this snapshot. Firecracker's snapshot only records this
    /// file's host path, not its contents, so it has to stay right where
    /// it was when the snapshot was taken — ownership passes to the new
    /// sandbox on resume, or the file is removed if this snapshot is
    /// deleted outright instead.
    pub rootfs_path: PathBuf,
    /// Held rather than released back to `NetworkManager`'s pool: the
    /// guest's network config (IP, MAC) was finalized via kernel boot
    /// args at the sandbox's original boot and is frozen into the
    /// snapshotted memory image, so whatever resumes this snapshot must
    /// reattach the exact same tap device rather than get a fresh lease.
    /// See `sandkiln_vmm::vm::Vm::resume`'s doc comment for why.
    pub network: Lease,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
}
