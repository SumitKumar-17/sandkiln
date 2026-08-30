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
    /// Carried over from the source sandbox's `attached_drives` — a
    /// drive's data lives inside the snapshotted memory/rootfs state the
    /// same way the network config does, so it must stay marked attached
    /// while this snapshot exists, or a caller could attach it to a
    /// second sandbox and corrupt it via two VMs writing to one file.
    pub attached_drives: Vec<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// Id of the live sandbox currently forked from this snapshot without
    /// consuming it (`POST /snapshots/:id/fork`), if any. `Vm::resume`'s
    /// `/snapshot/load` reopens the exact rootfs file this snapshot
    /// records, and — if the source sandbox was networked — the exact tap
    /// device, since the guest's IP/MAC were finalized at the source
    /// sandbox's *original* boot and are frozen into the snapshotted
    /// memory image (see `sandkiln_vmm::vm::Vm::resume`'s doc comment).
    /// A second live descendant sharing either at once means two
    /// Firecracker processes writing one rootfs file, or two guests
    /// presenting the same boot-time IP/MAC on the bridge simultaneously —
    /// real corruption and a real network collision, not hypothetical
    /// ones. This field is the lock that rules both out: set while a fork
    /// is being resumed and cleared only once that descendant's `Vm` has
    /// actually been killed (`routes_sandbox::stop_sandbox_by_id`), and
    /// checked by `fork_snapshot`, `resume_snapshot`, `delete_snapshot`,
    /// and `snapshot_sandbox` alike before any of them touch this
    /// snapshot's shared resources.
    pub forked_into: Option<String>,
}
