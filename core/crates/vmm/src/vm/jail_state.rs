//! Host-side handles of one jailed VM: what the daemon holds besides the
//! child process, and how it lets go of them.

use std::path::PathBuf;
use std::process::ChildStdin;

pub(super) struct JailState {
    /// `<chroot_base>/<supervisor name>/<id>`. Removed wholesale on teardown.
    pub instance_dir: PathBuf,
    /// The VM's cgroup. Jailer creates it and never removes it.
    pub cgroup_dir: PathBuf,
    /// Write end of the supervisor's stdin. The supervisor kills Firecracker
    /// when this closes, which is how the daemon stops a process it may not
    /// signal (another uid), and why a VM cannot outlive a crashed daemon.
    pub control: Option<ChildStdin>,
}

impl JailState {
    /// Closes the control pipe: the supervisor ends Firecracker and exits.
    /// Must come before waiting on the child.
    pub fn request_exit(&mut self) {
        drop(self.control.take());
    }

    /// Removes the jail directory tree and the cgroup once the process is
    /// gone. Failures are logged, not returned: the VM is already stopped,
    /// and what remains is a leak an operator should hear about.
    pub fn teardown(&self, vm_id: u64) {
        if let Err(e) = std::fs::remove_dir_all(&self.instance_dir) {
            tracing::warn!(
                vm_id,
                dir = %self.instance_dir.display(),
                error = %e,
                "could not remove the jail directory; if it is root-owned the supervisor never opened it to the jail group, remove it with sudo"
            );
        }
        match std::fs::remove_dir(&self.cgroup_dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                vm_id,
                cgroup = %self.cgroup_dir.display(),
                error = %e,
                "could not remove the VM's cgroup; the daemon must own the parent cgroup directory (scripts/host-setup/prepare-jailer.sh)"
            ),
        }
    }
}
