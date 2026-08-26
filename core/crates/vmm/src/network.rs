//! Per-sandbox networking: every VM gets its own tap device attached to a
//! shared bridge, with a statically-assigned IP from a pool. One bridge
//! means the NAT/DNS setup proven in `scripts/setup-tap-network.sh` and
//! `scripts/start-dns-proxy.sh` needs no per-VM wildcarding — it already
//! targets one interface and one gateway IP, which is exactly what the
//! bridge is.
//!
//! Requires `CAP_NET_ADMIN` on the running process (see
//! `scripts/grant-net-admin.sh`) — not root.

use std::collections::VecDeque;
use std::io;
use std::net::Ipv4Addr;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use crate::vm::NetworkConfig;

pub struct NetworkManager {
    bridge_name: String,
    gateway_ip: Ipv4Addr,
    prefix_len: u8,
    uplink: String,
    free_hosts: Mutex<VecDeque<u8>>,
    next_tap_id: AtomicU32,
}

pub struct Lease {
    pub config: NetworkConfig,
    host_octet: u8,
}

impl NetworkManager {
    /// `gateway_ip`/24 defines the shared subnet; host octets 2..254 are
    /// handed out to VMs (1 is the gateway itself).
    pub fn new(bridge_name: impl Into<String>, gateway_ip: Ipv4Addr, uplink: impl Into<String>) -> Self {
        Self {
            bridge_name: bridge_name.into(),
            gateway_ip,
            prefix_len: 24,
            uplink: uplink.into(),
            free_hosts: Mutex::new((2..=254u8).collect()),
            next_tap_id: AtomicU32::new(0),
        }
    }

    /// Idempotent: creates the bridge and NAT rules if they don't already
    /// exist. Call once at daemon startup before leasing any tap devices.
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
        Ok(())
    }

    /// Creates a tap device attached to the bridge and assigns it a free
    /// IP. The returned `Lease` must be released via `release()` once the
    /// VM it was handed to stops, or the IP is leaked for the process
    /// lifetime.
    pub fn lease(&self) -> io::Result<Lease> {
        let host_octet = {
            let mut free = self.free_hosts.lock().unwrap();
            free.pop_front().ok_or_else(|| io::Error::other("no free IPs left in the sandbox subnet"))?
        };

        let tap_id = self.next_tap_id.fetch_add(1, Ordering::Relaxed);
        let tap_device = format!("sktap{tap_id}");
        let guest_ip = octets_with_last(self.gateway_ip, host_octet);
        let guest_mac = format!("AA:FC:00:00:{:02X}:{:02X}", (tap_id >> 8) & 0xff, tap_id & 0xff);

        if let Err(e) = self.create_tap(&tap_device) {
            self.free_hosts.lock().unwrap().push_back(host_octet);
            return Err(e);
        }

        Ok(Lease {
            config: NetworkConfig { tap_device, guest_ip, gateway_ip: self.gateway_ip, guest_mac },
            host_octet,
        })
    }

    pub fn release(&self, lease: Lease) -> io::Result<()> {
        let _ = run("ip", &["link", "del", &lease.config.tap_device]);
        self.free_hosts.lock().unwrap().push_back(lease.host_octet);
        Ok(())
    }

    fn create_tap(&self, tap_device: &str) -> io::Result<()> {
        run("ip", &["tuntap", "add", tap_device, "mode", "tap"])?;
        run("ip", &["link", "set", tap_device, "master", &self.bridge_name])?;
        run("ip", &["link", "set", tap_device, "up"])?;
        Ok(())
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
