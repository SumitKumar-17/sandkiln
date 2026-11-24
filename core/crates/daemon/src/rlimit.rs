//! Process resource limits.
//!
//! Each live sandbox holds several descriptors (API and vsock sockets,
//! console log, tunnel listener), so a default soft `RLIMIT_NOFILE` of 1024
//! caps a busy host at a few hundred VMs and then fails creates with
//! "Too many open files". The hard limit is usually far higher.

use std::io;

/// Raises the soft open-file limit to the hard limit; returns the new soft
/// limit. Call before the Tokio runtime starts.
pub fn raise_nofile_to_hard() -> io::Result<u64> {
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: `lim` is a valid out-pointer for the call's duration.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // Some kernels report an "infinite" hard limit that setrlimit rejects
    // above nr_open, so clamp to a large finite value.
    let target = lim.rlim_max.min(1 << 20);
    if lim.rlim_cur < target {
        lim.rlim_cur = target;
        // SAFETY: `lim` is a valid in-pointer for the call's duration.
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(lim.rlim_cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_limit_never_ends_up_below_where_it_started() {
        let mut before = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut before) };
        let after = raise_nofile_to_hard().unwrap();
        assert!(after >= before.rlim_cur);
    }
}
