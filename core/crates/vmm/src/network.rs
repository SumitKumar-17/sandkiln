//! Per-sandbox networking: every running VM gets a tap device leased from a
//! pre-created pool, attached to a shared bridge, with a static IP. One
//! bridge means the NAT/DNS setup (`scripts/dev-tools/setup-tap-network.sh`,
//! `scripts/host-setup/start-dns-proxy.sh`) targets one interface and one
//! gateway IP, no per-VM wildcarding needed.
//!
//! A network identity has two halves with different lifetimes, so it is two
//! leases. The **IP** (address, MAC, host octet) is frozen into a guest's
//! memory image and must follow a snapshot; an [`IpLease`] is what a held
//! snapshot owns. The **tap** is only a host-side port the VM is plugged into
//! while it runs; a [`TapLease`] is taken at boot or resume and returned at
//! snapshot or stop, and a restored VM can be pointed at any free one
//! (`ResumeConfig::tap_override`). A running VM's [`Lease`] is the pair.
//!
//! The pool exists because creating a *new* tap device is a `TUNSETIFF`
//! ioctl that doesn't work under ambient `CAP_NET_ADMIN` in practice, only
//! full root — unlike the netlink attach/detach calls this module makes.
//! `scripts/host-setup/create-tap-pool.sh` creates devices once (needs
//! root); this module only attaches/detaches existing ones.

use std::collections::VecDeque;
use std::io;
use std::net::Ipv4Addr;
use std::process::Command;
use std::sync::Mutex;
use std::time::Instant;

use crate::vm::NetworkConfig;

/// Host octets 2..=254 are leasable (1 is the gateway, 0 and 255 are reserved).
const HOST_OCTET_COUNT: usize = 253;

/// Which shared pool ran dry. Carried inside the `io::Error` so a caller can
/// tell "retry later" from a real failure (`PoolExhausted::in`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolExhausted {
    Taps,
    Ips,
}

impl std::fmt::Display for PoolExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PoolExhausted::Taps => write!(f, "no free tap devices left in the pool (every tap is in use by a running sandbox)"),
            PoolExhausted::Ips => write!(f, "no free IPs left in the sandbox subnet"),
        }
    }
}

impl std::error::Error for PoolExhausted {}

impl PoolExhausted {
    /// The pool an error says is exhausted, if it is a pool-exhaustion error.
    pub fn of(error: &io::Error) -> Option<PoolExhausted> {
        error.get_ref()?.downcast_ref::<PoolExhausted>().copied()
    }

    fn into_error(self) -> io::Error {
        io::Error::other(self)
    }
}

/// The guest-visible, frozen half of a network identity. Baked into the guest
/// by boot-time kernel args, so it survives every snapshot and resume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestAddr {
    pub guest_ip: Ipv4Addr,
    pub gateway_ip: Ipv4Addr,
    pub guest_mac: String,
}

impl GuestAddr {
    /// The boot/resume configuration for this address plugged into `tap_device`.
    pub fn with_tap(&self, tap_device: &str) -> NetworkConfig {
        NetworkConfig {
            tap_device: tap_device.to_string(),
            guest_ip: self.guest_ip,
            gateway_ip: self.gateway_ip,
            guest_mac: self.guest_mac.clone(),
        }
    }
}

/// Ownership of one guest IP/MAC/host-octet, with no tap. What a held
/// snapshot owns: it costs nothing but an address from the /24.
pub struct IpLease {
    pub addr: GuestAddr,
    host_octet: u8,
}

impl IpLease {
    /// For persisting an identity (e.g. a `Snapshot` written to disk).
    pub fn host_octet(&self) -> u8 {
        self.host_octet
    }

    /// Joins the address to a tap for a VM that is about to run.
    pub fn with_tap(self, tap: TapLease) -> Lease {
        let config = self.addr.with_tap(tap.device());
        Lease { config, host_octet: self.host_octet }
    }
}

/// Ownership of one pooled tap device, attached to the bridge. Return it with
/// `NetworkManager::release_tap`; dropping it leaks the tap for the daemon's
/// lifetime.
pub struct TapLease {
    device: String,
}

impl TapLease {
    pub fn device(&self) -> &str {
        &self.device
    }
}

/// A running VM's whole network identity: an IP and the tap it is plugged
/// into. Split with `NetworkManager::detach` when the VM stops but the guest
/// (a snapshot) lives on.
pub struct Lease {
    pub config: NetworkConfig,
    host_octet: u8,
}

impl Lease {
    /// For persisting a lease's full identity.
    pub fn host_octet(&self) -> u8 {
        self.host_octet
    }
}

pub struct NetworkManager {
    bridge_name: String,
    gateway_ip: Ipv4Addr,
    prefix_len: u8,
    uplink: String,
    free_hosts: Mutex<VecDeque<u8>>,
    free_taps: Mutex<VecDeque<String>>,
    tap_pool_size: usize,
}

impl NetworkManager {
    /// `gateway_ip`/24 defines the shared subnet; host octets 2..254 are
    /// handed out to VMs (1 is the gateway itself). `tap_pool` must match
    /// what `create-tap-pool.sh` was run with.
    pub fn new(
        bridge_name: impl Into<String>,
        gateway_ip: Ipv4Addr,
        uplink: impl Into<String>,
        tap_pool: impl IntoIterator<Item = String>,
    ) -> Self {
        let taps: VecDeque<String> = tap_pool.into_iter().collect();
        Self {
            bridge_name: bridge_name.into(),
            gateway_ip,
            prefix_len: 24,
            uplink: uplink.into(),
            free_hosts: Mutex::new((2..=254u8).collect()),
            tap_pool_size: taps.len(),
            free_taps: Mutex::new(taps),
        }
    }

    /// The uplink interface this manager NATs sandbox traffic out
    /// through — exposed so `crate::egress::apply`/`remove` can scope a
    /// per-sandbox policy's rules to it, matching the existing
    /// bridge-wide `FORWARD` rule's own `-o <uplink>` scoping.
    pub fn uplink(&self) -> &str {
        &self.uplink
    }

    /// Idempotent: creates the bridge and NAT rules if they don't already
    /// exist, and verifies every pooled tap device is actually present.
    /// Call once at daemon startup before leasing any tap devices.
    pub fn ensure_ready(&self) -> io::Result<()> {
        if !link_exists(&self.bridge_name)? {
            run("ip", &["link", "add", &self.bridge_name, "type", "bridge"])?;
        }
        run("ip", &["addr", "replace", &format!("{}/{}", self.gateway_ip, self.prefix_len), "dev", &self.bridge_name])?;
        run("ip", &["link", "set", &self.bridge_name, "up"])?;
        run("sysctl", &["-w", "net.ipv4.ip_forward=1"])?;

        ensure_iptables_rule(&["-t", "nat", "-A", "POSTROUTING", "-o", &self.uplink, "-j", "MASQUERADE"])?;
        ensure_iptables_rule(&["-A", "FORWARD", "-i", &self.bridge_name, "-o", &self.uplink, "-j", "ACCEPT"])?;
        ensure_iptables_rule(&[
            "-A", "FORWARD", "-i", &self.uplink, "-o", &self.bridge_name,
            "-m", "state", "--state", "RELATED,ESTABLISHED", "-j", "ACCEPT",
        ])?;

        let missing: Vec<String> = {
            let taps = self.free_taps.lock().unwrap();
            let mut missing = Vec::new();
            for tap in taps.iter() {
                if !link_exists(tap)? {
                    missing.push(tap.clone());
                }
            }
            missing
        };
        if !missing.is_empty() {
            return Err(io::Error::other(format!(
                "tap devices missing: {missing:?} — run scripts/host-setup/create-tap-pool.sh first"
            )));
        }
        Ok(())
    }

    /// Leases an IP and a tap device for one freshly booted VM. The returned
    /// `Lease` must be released via `release()` (VM destroyed) or split with
    /// `detach()` (VM snapshotted), or both are leaked for the daemon's lifetime.
    pub fn lease(&self) -> io::Result<Lease> {
        let host_octet = {
            let mut free = self.free_hosts.lock().unwrap();
            free.pop_front().ok_or_else(|| PoolExhausted::Ips.into_error())?
        };
        match self.lease_tap() {
            Ok(tap) => Ok(IpLease { addr: self.addr_for(host_octet), host_octet }.with_tap(tap)),
            Err(e) => {
                self.free_hosts.lock().unwrap().push_back(host_octet);
                Err(e)
            }
        }
    }

    /// Leases and attaches one tap, with no IP. For a VM that already has its
    /// address: a restored snapshot (`IpLease::with_tap`) or a fork.
    pub fn lease_tap(&self) -> io::Result<TapLease> {
        let device = {
            let mut free = self.free_taps.lock().unwrap();
            free.pop_front().ok_or_else(|| PoolExhausted::Taps.into_error())?
        };
        if let Err(e) = self.attach_tap(&device) {
            self.free_taps.lock().unwrap().push_back(device);
            return Err(e);
        }
        Ok(TapLease { device })
    }

    /// Detaches the tap from the bridge and returns it to the pool.
    pub fn release_tap(&self, tap: TapLease) {
        let _ = run("ip", &["link", "set", &tap.device, "nomaster"]);
        let _ = run("ip", &["link", "set", &tap.device, "down"]);
        self.free_taps.lock().unwrap().push_back(tap.device);
    }

    /// Returns an IP to the pool.
    pub fn release_ip(&self, ip: IpLease) {
        self.free_hosts.lock().unwrap().push_back(ip.host_octet);
    }

    /// Releases both halves: the VM is gone and nothing will resume it.
    pub fn release(&self, lease: Lease) {
        let (ip, tap) = self.split(lease);
        self.release_tap(tap);
        self.release_ip(ip);
    }

    /// The VM has stopped but its guest identity lives on in a snapshot: the
    /// tap goes back to the pool, the IP stays owned. The caller must already
    /// have stopped the VM (a tap with an open Firecracker fd is not free).
    pub fn detach(&self, lease: Lease) -> IpLease {
        let (ip, tap) = self.split(lease);
        self.release_tap(tap);
        ip
    }

    fn split(&self, lease: Lease) -> (IpLease, TapLease) {
        let addr = GuestAddr { guest_ip: lease.config.guest_ip, gateway_ip: lease.config.gateway_ip, guest_mac: lease.config.guest_mac };
        (IpLease { addr, host_octet: lease.host_octet }, TapLease { device: lease.config.tap_device })
    }

    /// Reconstructs the `IpLease` of an identity already held by something
    /// this fresh `NetworkManager` has no record of handing out — a held
    /// snapshot reconciled from disk at startup. Removes the octet from the
    /// free pool; logs a warning rather than panicking if it was already
    /// absent (pool/config drift, not fatal to startup). No tap is involved:
    /// a snapshot's saved tap name is meaningless until it is resumed.
    pub fn reserve_ip(&self, addr: GuestAddr, host_octet: u8) -> IpLease {
        if !remove_first(&self.free_hosts, |h| *h == host_octet) {
            tracing::warn!(
                host_octet,
                "reserved host octet was not present in the free pool (already reserved, \
                 leased, or outside the configured host range) — proceeding anyway"
            );
        }
        IpLease { addr, host_octet }
    }

    /// Claims an identity that must still be free, atomically with the pool:
    /// `None` means someone else holds it. For a retired checkpoint's restore,
    /// which can race a live sandbox, another restore of the same lineage, or
    /// a fresh create being handed the same octet. Two checkpoints of one
    /// lineage share an IP, so exactly one of two concurrent restores wins.
    pub fn try_reserve_ip(&self, addr: GuestAddr, host_octet: u8) -> Option<IpLease> {
        remove_first(&self.free_hosts, |h| *h == host_octet).then_some(IpLease { addr, host_octet })
    }

    fn addr_for(&self, host_octet: u8) -> GuestAddr {
        GuestAddr {
            guest_ip: octets_with_last(self.gateway_ip, host_octet),
            gateway_ip: self.gateway_ip,
            guest_mac: format!("AA:FC:00:00:{:02X}:{:02X}", host_octet, host_octet),
        }
    }

    pub fn tap_pool_size(&self) -> usize {
        self.tap_pool_size
    }

    pub fn free_tap_count(&self) -> usize {
        self.free_taps.lock().unwrap().len()
    }

    pub fn ip_pool_size(&self) -> usize {
        HOST_OCTET_COUNT
    }

    pub fn free_ip_count(&self) -> usize {
        self.free_hosts.lock().unwrap().len()
    }

    /// Snapshot of currently-free tap devices — lets cross-crate callers
    /// (daemon tests verifying a reconciled snapshot removed its tap from
    /// the live pool) check pool state without reaching into private fields.
    pub fn free_tap_devices(&self) -> Vec<String> {
        self.free_taps.lock().unwrap().iter().cloned().collect()
    }

    /// Each step timed individually (all three are `fork`+`exec`, not a
    /// syscall) so profiling can tell process-spawn overhead from netlink
    /// work. Debug-level only; the daemon records the enclosing lease as a
    /// `/metrics` phase.
    fn attach_tap(&self, tap_device: &str) -> io::Result<()> {
        let started = Instant::now();
        run("ip", &["link", "set", tap_device, "up"])?;
        let link_up = started.elapsed();

        let before_master = Instant::now();
        run("ip", &["link", "set", tap_device, "master", &self.bridge_name])?;
        let set_master = before_master.elapsed();

        // Isolated ports can still reach the bridge (routing out through
        // the uplink keeps working) but can't forward frames to each
        // other — stops sandbox-to-sandbox traffic at L2.
        let before_isolate = Instant::now();
        run("bridge", &["link", "set", "dev", tap_device, "isolated", "on"])?;
        let isolate = before_isolate.elapsed();

        tracing::debug!(
            tap_device,
            link_up_us = link_up.as_micros(),
            set_master_us = set_master.as_micros(),
            isolate_us = isolate.as_micros(),
            total_us = started.elapsed().as_micros(),
            "attached tap device"
        );
        Ok(())
    }
}

/// Parses `ip route show default` to find the interface sandbox traffic
/// should be NATed out through, for setups that don't pin it explicitly.
pub fn detect_default_iface() -> io::Result<String> {
    let output = Command::new("ip").args(["route", "show", "default"]).output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .split_whitespace()
        .zip(stdout.split_whitespace().skip(1))
        .find(|(word, _)| *word == "dev")
        .map(|(_, iface)| iface.to_string())
        .ok_or_else(|| io::Error::other("no default route found — pass SANDKILN_UPLINK_IFACE explicitly"))
}

/// Removes the first element matching `pred`, reporting whether anything
/// was removed — `reserve` uses that to decide whether to warn.
fn remove_first<T>(pool: &Mutex<VecDeque<T>>, pred: impl Fn(&T) -> bool) -> bool {
    let mut pool = pool.lock().unwrap();
    match pool.iter().position(pred) {
        Some(idx) => {
            pool.remove(idx);
            true
        }
        None => false,
    }
}

fn octets_with_last(base: Ipv4Addr, last: u8) -> Ipv4Addr {
    let [a, b, c, _] = base.octets();
    Ipv4Addr::new(a, b, c, last)
}

fn link_exists(name: &str) -> io::Result<bool> {
    Ok(Command::new("ip").args(["link", "show", name]).output()?.status.success())
}

/// iptables has no idempotent "add if missing" — check first via `-C`,
/// then add. Mirrors the same pattern `scripts/dev-tools/setup-tap-network.sh` uses.
fn ensure_iptables_rule(args: &[&str]) -> io::Result<()> {
    let check_args: Vec<&str> = args.iter().map(|&a| if a == "-A" { "-C" } else { a }).collect();
    if Command::new("iptables").args(&check_args).output()?.status.success() {
        return Ok(());
    }
    run("iptables", args)
}

pub(crate) fn run(program: &str, args: &[&str]) -> io::Result<()> {
    let output = Command::new(program).args(args).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "{program} {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octets_with_last_keeps_network_prefix() {
        let base: Ipv4Addr = "172.16.0.1".parse().unwrap();
        assert_eq!(octets_with_last(base, 2), "172.16.0.2".parse::<Ipv4Addr>().unwrap());
        assert_eq!(octets_with_last(base, 254), "172.16.0.254".parse::<Ipv4Addr>().unwrap());
    }

    #[test]
    fn new_pool_has_hosts_2_through_254_and_the_given_taps() {
        let mgr = NetworkManager::new(
            "test-br0",
            "10.0.0.1".parse().unwrap(),
            "eth-test",
            ["tapA".to_string(), "tapB".to_string()],
        );
        assert_eq!(mgr.free_hosts.lock().unwrap().len(), 253); // 2..=254
        assert_eq!(mgr.free_taps.lock().unwrap().len(), 2);
    }

    /// A failed lease (nonexistent tap) must not leak its IP or tap name.
    #[test]
    fn failed_lease_returns_both_ip_and_tap_to_the_pool() {
        let mgr = NetworkManager::new(
            "test-br0-nonexistent",
            "10.0.0.1".parse().unwrap(),
            "eth-test",
            ["tap-does-not-exist".to_string()],
        );

        let result = mgr.lease();
        assert!(result.is_err(), "expected lease() to fail attaching a nonexistent tap");
        assert_eq!(mgr.free_hosts.lock().unwrap().len(), 253, "host octet must be returned to the pool on failure");
        assert_eq!(mgr.free_taps.lock().unwrap().len(), 1, "tap name must be returned to the pool on failure");
    }

    #[test]
    fn lease_fails_with_no_free_taps_without_touching_the_ip_pool() {
        let mgr = NetworkManager::new("test-br0", "10.0.0.1".parse().unwrap(), "eth-test", std::iter::empty());
        let result = mgr.lease();
        assert!(result.is_err());
        assert_eq!(mgr.free_hosts.lock().unwrap().len(), 253, "no tap was available, so no IP should be consumed either");
    }

    fn addr(host: u8) -> GuestAddr {
        GuestAddr {
            guest_ip: format!("10.0.0.{host}").parse().unwrap(),
            gateway_ip: "10.0.0.1".parse().unwrap(),
            guest_mac: format!("AA:FC:00:00:{host:02X}:{host:02X}"),
        }
    }

    fn manager(taps: &[&str]) -> NetworkManager {
        NetworkManager::new("test-br0", "10.0.0.1".parse().unwrap(), "eth-test", taps.iter().map(|t| t.to_string()))
    }

    /// Stands in for a tap that `lease_tap` would have attached: tests cannot
    /// run `ip`, so a `TapLease` is built directly.
    fn fake_tap(name: &str) -> TapLease {
        TapLease { device: name.to_string() }
    }

    #[test]
    fn pool_sizes_report_the_configured_totals_and_the_free_counts() {
        let mgr = manager(&["tapA", "tapB"]);
        assert_eq!((mgr.tap_pool_size(), mgr.free_tap_count()), (2, 2));
        assert_eq!((mgr.ip_pool_size(), mgr.free_ip_count()), (253, 253));
    }

    /// Core of the exhaustion fix: a reserved IP must leave the pool and use no tap.
    #[test]
    fn reserve_ip_removes_only_the_host_octet_and_never_a_tap() {
        let mgr = manager(&["tapA", "tapB"]);
        let ip = mgr.reserve_ip(addr(5), 5);
        assert_eq!(ip.host_octet(), 5);
        assert!(!mgr.free_hosts.lock().unwrap().contains(&5), "reserved host octet must leave the free pool");
        assert_eq!(mgr.free_tap_count(), 2, "holding an IP must not consume a tap");
    }

    #[test]
    fn reserve_ip_of_an_already_absent_octet_does_not_panic_or_touch_unrelated_entries() {
        let mgr = manager(&["tapB"]);
        let _ip = mgr.reserve_ip(addr(5), 255); // outside 2..=254
        assert_eq!(mgr.free_ip_count(), 253);
        assert_eq!(mgr.free_tap_count(), 1);
    }

    #[test]
    fn try_reserve_ip_gives_exactly_one_of_two_claimants_the_identity() {
        let mgr = manager(&[]);
        let first = mgr.try_reserve_ip(addr(7), 7);
        assert!(first.is_some());
        assert!(mgr.try_reserve_ip(addr(7), 7).is_none(), "a second claim of the same octet must lose");
        mgr.release_ip(first.unwrap());
        assert!(mgr.try_reserve_ip(addr(7), 7).is_some(), "releasing must make it claimable again");
    }

    #[test]
    fn try_reserve_ip_refuses_an_octet_already_leased_to_a_running_vm() {
        let mgr = manager(&[]);
        // Drain the front octet the way lease() would hand it out.
        let taken = mgr.free_hosts.lock().unwrap().pop_front().unwrap();
        assert!(mgr.try_reserve_ip(addr(taken), taken).is_none());
    }

    #[test]
    fn many_ip_leases_hold_no_taps_and_resume_takes_one_tap_each() {
        // 40 snapshots on a 2-tap pool: their IPs coexist, their taps do not exist.
        let mgr = manager(&["tapA", "tapB"]);
        let held: Vec<IpLease> = (2..42u8).map(|h| mgr.reserve_ip(addr(h), h)).collect();
        assert_eq!(held.len(), 40);
        assert_eq!(mgr.free_tap_count(), 2, "forty held identities, zero taps consumed");
        assert_eq!(mgr.free_ip_count(), 253 - 40);
    }

    #[test]
    fn with_tap_builds_the_boot_config_from_the_frozen_address() {
        let lease = mgr_ip(5).with_tap(fake_tap("tapZ"));
        assert_eq!(lease.config.tap_device, "tapZ");
        assert_eq!(lease.config.guest_ip, "10.0.0.5".parse::<Ipv4Addr>().unwrap());
        assert_eq!(lease.config.guest_mac, "AA:FC:00:00:05:05");
        assert_eq!(lease.host_octet(), 5);
    }

    fn mgr_ip(host: u8) -> IpLease {
        IpLease { addr: addr(host), host_octet: host }
    }

    /// The address must come back unchanged through a stop/resume cycle, on a
    /// different tap if that is what is free.
    #[test]
    fn detach_returns_the_tap_but_keeps_the_ip_and_a_resume_can_use_another_tap() {
        let mgr = manager(&[]); // the fake lease below stands in for the pool's only tap
        let lease = mgr.reserve_ip(addr(9), 9).with_tap(fake_tap("tapA"));
        let ip = mgr.detach(lease);
        assert_eq!(mgr.free_tap_devices(), vec!["tapA".to_string()], "the tap is back in the pool");
        assert!(!mgr.free_hosts.lock().unwrap().contains(&9), "the IP stays owned: detach must not return it to the pool");
        assert_eq!(ip.addr, addr(9), "guest IP and MAC are frozen");
        let resumed = ip.with_tap(fake_tap("tapOther"));
        assert_eq!(resumed.config.tap_device, "tapOther");
        assert_eq!(resumed.config.guest_ip, addr(9).guest_ip);
    }

    #[test]
    fn release_returns_both_halves() {
        let mgr = manager(&[]);
        let octet = mgr.free_hosts.lock().unwrap().pop_front().unwrap();
        mgr.release(mgr_ip(octet).with_tap(fake_tap("tapA")));
        assert_eq!(mgr.free_tap_devices(), vec!["tapA".to_string()]);
        assert!(mgr.free_hosts.lock().unwrap().contains(&octet));
    }

    #[test]
    fn exhaustion_errors_are_distinguishable_from_other_failures() {
        let mgr = manager(&[]);
        let err = mgr.lease_tap().err().unwrap();
        assert_eq!(PoolExhausted::of(&err), Some(PoolExhausted::Taps));
        assert!(err.to_string().contains("no free tap devices"));
        assert_eq!(PoolExhausted::of(&io::Error::other("something else")), None);
        assert_eq!(PoolExhausted::of(&io::Error::from(io::ErrorKind::NotFound)), None);
    }

    #[test]
    fn an_exhausted_ip_pool_is_reported_as_such_and_takes_no_tap() {
        let mgr = manager(&["tapA"]);
        mgr.free_hosts.lock().unwrap().clear();
        let err = mgr.lease().err().unwrap();
        assert_eq!(PoolExhausted::of(&err), Some(PoolExhausted::Ips));
        assert_eq!(mgr.free_tap_count(), 1);
    }
}
