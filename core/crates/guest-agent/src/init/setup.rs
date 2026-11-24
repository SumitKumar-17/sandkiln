//! One-shot guest preparation before the agent starts. Every step is best
//! effort and logged: a guest with a missing optional mount is still a
//! working sandbox, and PID 1 must never exit.

use std::ffi::CString;
use std::io;

/// One pseudo-filesystem mount. Order matters: a parent precedes children.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct MountSpec {
    pub source: &'static str,
    pub target: &'static str,
    pub fstype: &'static str,
    pub flags: libc::c_ulong,
    pub data: &'static str,
}

const NOSUID_NODEV: libc::c_ulong = libc::MS_NOSUID | libc::MS_NODEV;
const NOSUID_NODEV_NOEXEC: libc::c_ulong = NOSUID_NODEV | libc::MS_NOEXEC;

/// The set systemd mounted for the previous default image, minus the
/// systemd-private ones (`/var/lib/systemd`, credentials, binfmt autofs) and
/// the debug ones (bpf, pstore, debugfs, hugetlbfs, mqueue) nothing relies
/// on. `/dev` itself is devtmpfs mounted by the kernel before init.
pub(super) const MOUNTS: &[MountSpec] = &[
    MountSpec { source: "proc", target: "/proc", fstype: "proc", flags: NOSUID_NODEV_NOEXEC, data: "" },
    MountSpec { source: "sysfs", target: "/sys", fstype: "sysfs", flags: NOSUID_NODEV_NOEXEC, data: "" },
    // ptmxmode=666 so a plain /dev/ptmx works for any user, as a PTY needs.
    MountSpec { source: "devpts", target: "/dev/pts", fstype: "devpts", flags: libc::MS_NOSUID | libc::MS_NOEXEC, data: "gid=5,mode=620,ptmxmode=666" },
    MountSpec { source: "tmpfs", target: "/dev/shm", fstype: "tmpfs", flags: NOSUID_NODEV, data: "" },
    MountSpec { source: "tmpfs", target: "/run", fstype: "tmpfs", flags: NOSUID_NODEV, data: "mode=755" },
    MountSpec { source: "tmpfs", target: "/tmp", fstype: "tmpfs", flags: NOSUID_NODEV, data: "" },
    MountSpec { source: "cgroup2", target: "/sys/fs/cgroup", fstype: "cgroup2", flags: NOSUID_NODEV_NOEXEC, data: "nsdelegate,memory_recursiveprot" },
];

/// `/dev` symlinks systemd created that userland expects.
const DEV_LINKS: &[(&str, &str)] =
    &[("/proc/self/fd", "/dev/fd"), ("/proc/self/fd/0", "/dev/stdin"), ("/proc/self/fd/1", "/dev/stdout"), ("/proc/self/fd/2", "/dev/stderr")];

/// systemd's service default for `LimitNOFILE` (soft stays at 1024).
const NOFILE_HARD: libc::rlim_t = 524_288;

pub(super) fn prepare() {
    ensure_stdio();
    for spec in MOUNTS {
        if let Err(e) = mount(spec) {
            eprintln!("init: mount {}: {e}", spec.target);
        }
    }
    for (target, link) in DEV_LINKS {
        // Already present (devtmpfs may carry them) is fine.
        let _ = std::os::unix::fs::symlink(target, link);
    }
    let _ = std::fs::create_dir_all("/run/lock");
    set_hostname();
    if let Err(e) = loopback_up() {
        eprintln!("init: loopback up: {e}");
    }
    super::environment::apply();
    raise_nofile_limit();
    // Deliver Ctrl-Alt-Del as SIGINT to us instead of an instant reset, so
    // the supervisor can sync first.
    // SAFETY: reboot(2) with RB_DISABLE_CAD takes no pointers.
    unsafe { libc::reboot(libc::LINUX_REBOOT_CMD_CAD_OFF) };
}

fn mount(spec: &MountSpec) -> io::Result<()> {
    std::fs::create_dir_all(spec.target)?;
    let (source, target, fstype, data) = (cstr(spec.source)?, cstr(spec.target)?, cstr(spec.fstype)?, cstr(spec.data)?);
    // SAFETY: all four are valid NUL-terminated strings that outlive the call.
    let rc = unsafe { libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), spec.flags, data.as_ptr().cast()) };
    match io::Error::last_os_error().raw_os_error() {
        _ if rc == 0 => Ok(()),
        // Mounted already (a kernel that automounts it): the goal holds.
        Some(libc::EBUSY) => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

fn cstr(s: &str) -> io::Result<CString> {
    CString::new(s).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

/// The hostname to set from the contents of `/etc/hostname`: the first
/// non-comment, non-empty line, trimmed; `None` when there is none or it
/// is longer than the kernel's 64-byte limit.
pub(super) fn parse_hostname(contents: &str) -> Option<&str> {
    let name = contents.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#'))?;
    (name.len() <= 64).then_some(name)
}

fn set_hostname() {
    let Ok(contents) = std::fs::read_to_string("/etc/hostname") else { return };
    let Some(name) = parse_hostname(&contents) else { return };
    // SAFETY: pointer and length describe `name`'s bytes.
    if unsafe { libc::sethostname(name.as_ptr().cast(), name.len()) } != 0 {
        eprintln!("init: sethostname: {}", io::Error::last_os_error());
    }
}

/// Brings `lo` up; the kernel then assigns 127.0.0.1 itself. The tunnel and
/// preview features reach guest services through it.
fn loopback_up() -> io::Result<()> {
    // SAFETY: plain socket(2); the result is checked.
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: zeroed ifreq is a valid initial value; the name fits IFNAMSIZ.
    let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
    for (dst, src) in ifr.ifr_name.iter_mut().zip(b"lo\0") {
        *dst = *src as libc::c_char;
    }
    // SAFETY: `ifr` is a valid ifreq; the union field written is the flags
    // the preceding SIOCGIFFLAGS filled in.
    let result = unsafe {
        if libc::ioctl(fd, libc::SIOCGIFFLAGS as _, &mut ifr) != 0 {
            Err(io::Error::last_os_error())
        } else {
            ifr.ifr_ifru.ifru_flags |= libc::IFF_UP as libc::c_short;
            if libc::ioctl(fd, libc::SIOCSIFFLAGS as _, &ifr) != 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
        }
    };
    // SAFETY: `fd` was opened above and is closed once.
    unsafe { libc::close(fd) };
    result
}

fn raise_nofile_limit() {
    let limit = libc::rlimit { rlim_cur: 1024, rlim_max: NOFILE_HARD };
    // SAFETY: `limit` is a valid rlimit for the call's duration.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
        eprintln!("init: setrlimit NOFILE: {}", io::Error::last_os_error());
    }
}

/// The kernel gives init /dev/console as fds 0-2 when it exists; if it did
/// not, point any closed one at /dev/null so a later `open` never lands on
/// fd 0-2 by accident.
fn ensure_stdio() {
    for fd in 0..3 {
        // SAFETY: F_GETFD only queries the descriptor table.
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } != -1 {
            continue;
        }
        let path = CString::new("/dev/null").expect("no NUL");
        // SAFETY: valid C string; on success the returned fd is dup'd onto `fd`.
        unsafe {
            let opened = libc::open(path.as_ptr(), libc::O_RDWR);
            if opened >= 0 && opened != fd {
                libc::dup2(opened, fd);
                libc::close(opened);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn a_mount_target_is_never_mounted_before_its_parent() {
        for (i, spec) in MOUNTS.iter().enumerate() {
            for later in &MOUNTS[i + 1..] {
                assert!(
                    !spec.target.starts_with(&format!("{}/", later.target)),
                    "{} is mounted before its parent {}",
                    spec.target,
                    later.target
                );
            }
        }
    }

    #[test]
    fn mount_targets_are_unique_absolute_paths() {
        let targets: HashSet<_> = MOUNTS.iter().map(|m| m.target).collect();
        assert_eq!(targets.len(), MOUNTS.len());
        assert!(MOUNTS.iter().all(|m| m.target.starts_with('/')));
    }

    #[test]
    fn the_filesystems_pty_and_tmp_workloads_need_are_present() {
        for needed in ["/proc", "/sys", "/dev/pts", "/dev/shm", "/run", "/tmp"] {
            assert!(MOUNTS.iter().any(|m| m.target == needed), "{needed} missing");
        }
    }

    #[test]
    fn hostname_takes_the_first_real_line() {
        assert_eq!(parse_hostname("ubuntu-fc-uvm\n"), Some("ubuntu-fc-uvm"));
        assert_eq!(parse_hostname("# comment\n\n  box1  \nother"), Some("box1"));
    }

    #[test]
    fn hostname_rejects_empty_and_oversized() {
        assert_eq!(parse_hostname(""), None);
        assert_eq!(parse_hostname("# only a comment\n"), None);
        assert_eq!(parse_hostname(&"a".repeat(65)), None);
        assert_eq!(parse_hostname(&"a".repeat(64)).map(str::len), Some(64));
    }
}
