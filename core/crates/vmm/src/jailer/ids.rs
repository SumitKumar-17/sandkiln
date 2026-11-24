//! The uid pool that gives every concurrent jailed VM a distinct identity,
//! and the per-VM jailer instance id.

use std::collections::VecDeque;
use std::io;
use std::ops::RangeInclusive;
use std::sync::Mutex;

/// Hands out distinct uid/gid pairs to concurrent jailed VMs from a fixed
/// range, takes them back on release. Mirrors
/// `crate::network::NetworkManager`'s tap/IP pool — a bounded,
/// pre-declared id space rather than hashing a sandbox id into one,
/// because a collision here is a real security bug: two jailed VMs
/// sharing a uid means one guest's escaped process can
/// `kill`/`ptrace`/read the other's files.
///
/// Configure the range outside normal system/user uids (600000+,
/// mirroring `/etc/subuid` convention) so a jailed uid never collides
/// with a real host account. Only the uid is per VM; the gid is one shared
/// jail group (`JailLaunch::gid`), so the isolation between two sandboxes
/// rests on distinct uids and distinct chroots, not on distinct groups.
pub struct JailerIdPool {
    free: Mutex<VecDeque<u32>>,
    quarantined: Mutex<Vec<u32>>,
}

impl JailerIdPool {
    pub fn new(range: RangeInclusive<u32>) -> Self {
        Self { free: Mutex::new(range.collect()), quarantined: Mutex::new(Vec::new()) }
    }

    /// Number of ids currently available to lease — the daemon's max
    /// concurrent-jailed-sandbox ceiling at this instant.
    pub fn available(&self) -> usize {
        self.free.lock().unwrap().len()
    }

    pub fn lease(&self) -> io::Result<u32> {
        self.free.lock().unwrap().pop_front().ok_or_else(|| io::Error::other("no free jailer uid/gid left in the pool"))
    }

    pub fn release(&self, id: u32) {
        self.free.lock().unwrap().push_back(id);
    }

    /// Withholds `id` from reuse for the rest of this process's life. For a
    /// uid whose process could not be killed: handing the uid to the next
    /// VM would let a live, possibly guest-controlled, process share an
    /// identity with it.
    pub fn quarantine(&self, id: u32) {
        self.quarantined.lock().unwrap().push(id);
    }

    /// Ids withheld by [`Self::quarantine`].
    pub fn quarantined(&self) -> Vec<u32> {
        self.quarantined.lock().unwrap().clone()
    }
}

/// A stable, jailer-`--id`-safe identifier for one VM, from the same
/// monotonic counter `vm::Vm` uses for socket paths — keeps every
/// jailer/chroot/socket path traceable to one number in the logs.
pub fn jail_instance_id(vm_id: u64) -> String {
    format!("sandkiln-{vm_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jail_instance_id_is_stable_and_readable() {
        assert_eq!(jail_instance_id(42), "sandkiln-42");
    }

    #[test]
    fn new_pool_has_exactly_the_configured_range() {
        let pool = JailerIdPool::new(600000..=600009);
        assert_eq!(pool.available(), 10);
    }

    #[test]
    fn lease_and_release_round_trip_without_growing_or_shrinking_the_pool() {
        let pool = JailerIdPool::new(600000..=600001);
        let a = pool.lease().unwrap();
        let b = pool.lease().unwrap();
        assert_ne!(a, b, "two concurrent leases must never return the same id");
        assert_eq!(pool.available(), 0);
        assert!(pool.lease().is_err(), "pool must be exhausted after leasing every id in the range");

        pool.release(a);
        assert_eq!(pool.available(), 1);
        let c = pool.lease().unwrap();
        assert_eq!(c, a, "a released id becomes available for lease again");
    }

    #[test]
    fn leased_ids_are_always_within_the_configured_range() {
        let pool = JailerIdPool::new(700000..=700004);
        let mut leased = Vec::new();
        while let Ok(id) = pool.lease() {
            assert!((700000..=700004).contains(&id));
            leased.push(id);
        }
        assert_eq!(leased.len(), 5);
    }

    #[test]
    fn quarantined_ids_never_return_to_the_pool() {
        let pool = JailerIdPool::new(600000..=600001);
        let a = pool.lease().unwrap();
        pool.quarantine(a);
        assert_eq!(pool.quarantined(), vec![a]);
        let b = pool.lease().unwrap();
        assert_ne!(a, b);
        assert!(pool.lease().is_err(), "the quarantined id must stay out of circulation");
    }
}
