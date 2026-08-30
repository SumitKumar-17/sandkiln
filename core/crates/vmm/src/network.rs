//! Per-sandbox networking: every VM gets a tap device leased from a
//! pre-created pool, attached to a shared bridge, with a statically
//! assigned IP. One bridge means the NAT/DNS setup proven in
//! `scripts/setup-tap-network.sh` and `scripts/start-dns-proxy.sh` needs
//! no per-VM wildcarding — it already targets one interface and one
//! gateway IP, which is exactly what the bridge is.
//!
//! The pool exists because creating a *new* tap device is a TUNSETIFF
//! ioctl on `/dev/net/tun`, and — unlike the netlink operations here
//! (bridge/link management) — that specific ioctl did not work under this
//! process's ambient `CAP_NET_ADMIN` in practice, only under full root.
//! Persistent tap devices sidestep it: `scripts/create-tap-pool.sh`
//! creates them once (needs root), and this module only ever
//! attaches/detaches existing devices, which is a plain netlink call and
//! does work under ambient `CAP_NET_ADMIN` (see `scripts/grant-net-admin.sh`).

use std::collections::VecDeque;
use std::io;
use std::net::Ipv4Addr;
use std::process::Command;
use std::sync::Mutex;

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
    /// Exposed so a caller that needs to persist a lease's full identity
    /// (e.g. the daemon writing a `Snapshot`'s held lease to disk so it
    /// survives a restart) can round-trip it — `host_octet` itself stays
    /// private since nothing outside this module should construct a
    /// `Lease` except through `lease()` or `NetworkManager::reserve()`.
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
                "tap devices missing: {missing:?} — run scripts/create-tap-pool.sh first"
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

    /// Reconstructs a `Lease` for a tap device/host octet that's already
    /// held by something outside this `NetworkManager`'s own bookkeeping —
    /// specifically, a `Snapshot` reconciled from disk at daemon startup,
    /// which holds a real tap device (frozen into its saved memory image,
    /// see `sandkiln_vmm::vm::snapshot`'s `Vm::resume` doc comment) that
    /// this fresh `NetworkManager` instance has no record of ever handing
    /// out. Without this, the tap/host octet would sit in the free pool
    /// and a later live `lease()` call could hand the same tap device to
    /// a second, unrelated sandbox — two VMs fighting over one device.
    /// Removes both from the free pools (idempotent-ish: logs a warning
    /// rather than panicking if either was already absent, since that
    /// indicates pool/config drift worth knowing about but not fatal to
    /// startup) and returns the equivalent of a normal `lease()`.
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

    /// A snapshot of which tap devices are currently free. Useful for
    /// observability, and lets cross-crate callers (the daemon's own
    /// tests, verifying that reconciling a snapshot from disk actually
    /// removed its held tap from the live pool) check pool state without
    /// reaching into this module's private fields.
    pub fn free_tap_devices(&self) -> Vec<String> {
        self.free_taps.lock().unwrap().iter().cloned().collect()
    }

    fn attach_tap(&self, tap_device: &str) -> io::Result<()> {
        run("ip", &["link", "set", tap_device, "up"])?;
        run("ip", &["link", "set", tap_device, "master", &self.bridge_name])?;
        // Isolated bridge ports can still reach the bridge itself (so
        // routing out through the uplink keeps working) but can't forward
        // frames to each other — this is what actually stops one sandbox
        // from reaching another's IP on the shared bridge at L2.
        run("bridge", &["link", "set", "dev", tap_device, "isolated", "on"])?;
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

/// Removes and discards the first element matching `pred` from a pooled
/// `VecDeque`, reporting whether anything was actually removed. `reserve`
/// needs that boolean to decide whether the removal was a no-op (pool
/// drift) worth warning about.
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
/// then add. Mirrors the same pattern `scripts/setup-tap-network.sh` uses.
fn ensure_iptables_rule(args: &[&str]) -> io::Result<()> {
    let check_args: Vec<&str> = args.iter().map(|&a| if a == "-A" { "-C" } else { a }).collect();
    if Command::new("iptables").args(&check_args).output()?.status.success() {
        return Ok(());
    }
    run("iptables", args)
}

fn run(program: &str, args: &[&str]) -> io::Result<()> {
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

    /// A tap device that can't actually be attached (this one doesn't
    /// exist, and we're not asserting anything about root/permissions —
    /// `ip link set` on a nonexistent device fails cleanly either way)
    /// must not leak its IP or tap name out of the pool: a failed lease
    /// should be exactly as if it never happened.
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

    /// This is the core of the tap-double-lease fix: a snapshot reconciled
    /// from disk at startup calls `reserve` for the tap it holds, and a
    /// live `lease()` call afterward must not be handed that same device.
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

    /// Reserving something already outside the pool (stale config,
    /// duplicate reservation) must not panic or corrupt the pool — it's a
    /// startup-time warning, not a fatal error, since the daemon still
    /// needs to come up.
    #[test]
    fn reserve_of_an_already_absent_tap_does_not_panic_or_touch_unrelated_entries() {
        let mgr = NetworkManager::new("test-br0", "10.0.0.1".parse().unwrap(), "eth-test", ["tapB".to_string()]);

        // 255 is outside the pool's 2..=254 host-octet range, so it can
        // never have been present to begin with.
        let lease = mgr.reserve(test_config("tap-not-in-pool"), 255);
        assert_eq!(lease.config.tap_device, "tap-not-in-pool");
        assert_eq!(mgr.free_taps.lock().unwrap().len(), 1, "tapB must be untouched");
        assert_eq!(mgr.free_hosts.lock().unwrap().len(), 253, "no host octet should have been removed");
    }

    /// After a reserve, the reserved tap is unavailable to a subsequent
    /// live lease — the actual resource-ownership property this exists to
    /// guarantee, not just an isolated pool-bookkeeping detail.
    #[test]
    fn a_reserved_tap_cannot_then_be_leased_to_a_different_caller() {
        let mgr = NetworkManager::new("test-br0", "10.0.0.1".parse().unwrap(), "eth-test", ["only-tap".to_string()]);
        let _held = mgr.reserve(test_config("only-tap"), 2);

        let result = mgr.lease();
        assert!(result.is_err(), "the only tap device is already reserved, lease() must fail rather than double-hand it out");
    }
}
