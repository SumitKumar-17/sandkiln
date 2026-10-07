//! Per-sandbox networking: every VM gets a tap device leased from a
//! pre-created pool, attached to a shared bridge, with a static IP. One
//! bridge means the NAT/DNS setup (`scripts/dev-tools/setup-tap-network.sh`,
//! `scripts/host-setup/start-dns-proxy.sh`) targets one interface and one
//! gateway IP, no per-VM wildcarding needed.
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

pub struct NetworkManager {
    bridge_name: String,
    gateway_ip: Ipv4Addr,
    prefix_len: u8,
    uplink: String,
    free_hosts: Mutex<VecDeque<u8>>,
    free_taps: Mutex<VecDeque<String>>,
}

pub struct Lease {
    pub config: NetworkConfig,
    host_octet: u8,
}

impl Lease {
    /// For persisting a lease's full identity (e.g. a `Snapshot` written
    /// to disk) — stays private otherwise; only `lease()`/`reserve()`
    /// construct a `Lease`.
    pub fn host_octet(&self) -> u8 {
        self.host_octet
    }
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
        Self {
            bridge_name: bridge_name.into(),
            gateway_ip,
            prefix_len: 24,
            uplink: uplink.into(),
            free_hosts: Mutex::new((2..=254u8).collect()),
            free_taps: Mutex::new(tap_pool.into_iter().collect()),
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

    /// Leases a tap device and an IP for one VM. The returned `Lease`
    /// must be released via `release()` once the VM stops, or both are
    /// leaked for the daemon's lifetime.
    pub fn lease(&self) -> io::Result<Lease> {
        let host_octet = {
            let mut free = self.free_hosts.lock().unwrap();
            free.pop_front().ok_or_else(|| io::Error::other("no free IPs left in the sandbox subnet"))?
        };
        let tap_device = {
            let mut free = self.free_taps.lock().unwrap();
            match free.pop_front() {
                Some(tap) => tap,
                None => {
                    self.free_hosts.lock().unwrap().push_back(host_octet);
                    return Err(io::Error::other("no free tap devices left in the pool"));
                }
            }
        };

        if let Err(e) = self.attach_tap(&tap_device) {
            self.free_hosts.lock().unwrap().push_back(host_octet);
            self.free_taps.lock().unwrap().push_back(tap_device);
            return Err(e);
        }

        let guest_ip = octets_with_last(self.gateway_ip, host_octet);
        let guest_mac = format!("AA:FC:00:00:{:02X}:{:02X}", host_octet, host_octet);

        Ok(Lease { config: NetworkConfig { tap_device, guest_ip, gateway_ip: self.gateway_ip, guest_mac }, host_octet })
    }

    pub fn release(&self, lease: Lease) -> io::Result<()> {
        let _ = run("ip", &["link", "set", &lease.config.tap_device, "nomaster"]);
        let _ = run("ip", &["link", "set", &lease.config.tap_device, "down"]);
        self.free_hosts.lock().unwrap().push_back(lease.host_octet);
        self.free_taps.lock().unwrap().push_back(lease.config.tap_device);
        Ok(())
    }

    /// Reconstructs a `Lease` for a tap/host-octet already held by
    /// something this fresh `NetworkManager` has no record of handing
    /// out — a `Snapshot` reconciled from disk at startup, holding a tap
    /// frozen into its saved memory image. Without this, a later live
    /// `lease()` could hand the same tap to a second sandbox. Removes both
    /// from the free pools; logs a warning rather than panicking if either
    /// was already absent (pool/config drift, not fatal to startup).
    pub fn reserve(&self, config: NetworkConfig, host_octet: u8) -> Lease {
        let tap_was_free = remove_first(&self.free_taps, |t| t == &config.tap_device);
        let host_was_free = remove_first(&self.free_hosts, |h| *h == host_octet);
        if !tap_was_free {
            tracing::warn!(
                tap_device = %config.tap_device,
                "reserved tap device was not present in the free pool (already reserved, \
                 leased, or outside the configured tap pool) — proceeding anyway"
            );
        }
        if !host_was_free {
            tracing::warn!(
                host_octet,
                "reserved host octet was not present in the free pool (already reserved, \
                 leased, or outside the configured host range) — proceeding anyway"
            );
        }
        Lease { config, host_octet }
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

    fn test_config(tap: &str) -> NetworkConfig {
        NetworkConfig {
            tap_device: tap.to_string(),
            guest_ip: "10.0.0.5".parse().unwrap(),
            gateway_ip: "10.0.0.1".parse().unwrap(),
            guest_mac: "AA:FC:00:00:05:05".to_string(),
        }
    }

    /// Core of the tap-double-lease fix: a reserved tap must leave the pool.
    #[test]
    fn reserve_removes_tap_and_host_octet_from_the_free_pools() {
        let mgr = NetworkManager::new(
            "test-br0",
            "10.0.0.1".parse().unwrap(),
            "eth-test",
            ["tapA".to_string(), "tapB".to_string()],
        );

        let lease = mgr.reserve(test_config("tapA"), 5);
        assert_eq!(lease.config.tap_device, "tapA");
        assert_eq!(lease.host_octet(), 5);

        let free_taps = mgr.free_taps.lock().unwrap();
        assert!(!free_taps.contains(&"tapA".to_string()), "reserved tap must leave the free pool");
        assert!(free_taps.contains(&"tapB".to_string()), "unrelated tap must stay in the free pool");
        drop(free_taps);
        assert!(
            !mgr.free_hosts.lock().unwrap().contains(&5),
            "reserved host octet must leave the free pool"
        );
    }

    /// Reserving something already outside the pool must warn, not panic.
    #[test]
    fn reserve_of_an_already_absent_tap_does_not_panic_or_touch_unrelated_entries() {
        let mgr = NetworkManager::new("test-br0", "10.0.0.1".parse().unwrap(), "eth-test", ["tapB".to_string()]);

        // 255 is outside the 2..=254 host-octet range.
        let lease = mgr.reserve(test_config("tap-not-in-pool"), 255);
        assert_eq!(lease.config.tap_device, "tap-not-in-pool");
        assert_eq!(mgr.free_taps.lock().unwrap().len(), 1, "tapB must be untouched");
        assert_eq!(mgr.free_hosts.lock().unwrap().len(), 253, "no host octet should have been removed");
    }

    /// The actual resource-ownership guarantee: a reserved tap stays unleasable.
    #[test]
    fn a_reserved_tap_cannot_then_be_leased_to_a_different_caller() {
        let mgr = NetworkManager::new("test-br0", "10.0.0.1".parse().unwrap(), "eth-test", ["only-tap".to_string()]);
        let _held = mgr.reserve(test_config("only-tap"), 2);

        let result = mgr.lease();
        assert!(result.is_err(), "the only tap device is already reserved, lease() must fail rather than double-hand it out");
    }
}
