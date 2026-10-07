//! Per-sandbox egress (outbound) firewall, layered on `network.rs`'s
//! shared bridge-wide NAT/forward setup without modifying it.
//!
//! One iptables chain per sandbox (`SK-EG-<tap_device>`, tap names are
//! unique while leased) instead of juggling rule numbers in `FORWARD`:
//! every rule lives in that chain (deny, then allow, then the mode
//! default — first-match-wins makes deny always beat allow on overlap,
//! no special-casing needed), and the only thing touching `FORWARD`
//! itself is one `-I FORWARD 1` jump rule matched by `guest_ip` (not the
//! tap — traffic is evaluated post-bridging, where interface matching
//! means the bridge, not the tap).
//!
//! **Scoped to `-o <uplink>` only**, same as the bridge-wide rule — DNS
//! to the bridge's own gateway IP never transits the uplink, so it's
//! structurally exempt without an explicit allowlist entry; `DenyAll`
//! can still resolve names, just can't reach past the gateway.
//!
//! **Not built**: domain-level rules (needs the DNS proxy to become
//! source-IP-aware — see `ROADMAP.md`'s "Firewall and egress policy")
//! and port matching (`-p tcp --dport`, a straightforward future
//! extension). IPv4 only.
//!
//! **Lifecycle**: tied to the lease, not the VM — applied when a lease
//! goes live (boot, resume, fork), removed only when the lease is
//! released. A snapshot-and-stop leaves it dormant but intact, like the
//! tap device. `apply` is idempotent (flush-and-repopulate) so a resume
//! can call it unconditionally — needed after a daemon restart (iptables
//! state lives in the kernel) and harmless after a real reboot.

use crate::network::run;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::net::Ipv4Addr;
use std::process::{Command, Stdio};
use std::str::FromStr;

/// `Serialize`/`Deserialize` (here and on `EgressPolicy`) are for
/// `crate::snapshot::SnapshotMeta` — an egress policy is carried through
/// snapshot/resume/fork like tags/name/drives, so it can't silently
/// disappear (a security regression) when a protected sandbox resumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressMode {
    /// Today's default, unquestioned behavior for anything not covered
    /// by an explicit `deny_cidrs` entry: outbound is allowed.
    AllowAll,
    /// Outbound is blocked by default; only what `allow_cidrs` explicitly
    /// lists (and isn't also in `deny_cidrs`, which still wins) gets
    /// through.
    DenyAll,
}

/// One sandbox's egress policy. `allow_cidrs`/`deny_cidrs` are already
/// validated (`validate_cidr`) at the API boundary
/// (`routes_sandbox::CreateSandboxRequest`), not here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressPolicy {
    pub mode: EgressMode,
    pub allow_cidrs: Vec<String>,
    pub deny_cidrs: Vec<String>,
}

/// Validates an IPv4 CIDR string (`"10.0.0.0/8"`) — no new dependency,
/// iptables already understands CIDR; this just turns a malformed policy
/// into a clear error at request time instead of a cryptic iptables
/// failure deep inside a boot task.
pub fn validate_cidr(s: &str) -> Result<(), String> {
    let (addr, prefix) = s.split_once('/').ok_or_else(|| format!("'{s}' is not a CIDR (expected e.g. '10.0.0.0/8')"))?;
    Ipv4Addr::from_str(addr).map_err(|_| format!("'{s}' has an invalid IPv4 address"))?;
    let prefix: u8 = prefix.parse().map_err(|_| format!("'{s}' has a non-numeric prefix length"))?;
    if prefix > 32 {
        return Err(format!("'{s}' has a prefix length above 32"));
    }
    Ok(())
}

fn chain_name(tap_device: &str) -> String {
    format!("SK-EG-{tap_device}")
}

/// Installs (or re-installs — idempotent, expected on resume/fork)
/// `policy` for the sandbox holding `guest_ip` on `tap_device`, egressing
/// via `uplink`.
///
/// Rules load as one `iptables-restore --noflush` call instead of one
/// `iptables` spawn per rule — cut a 6-CIDR policy from ~8.3ms to ~3.1ms
/// per create, since each spawn pays the same fork+exec cost regardless
/// of work done. `--noflush` leaves every other chain untouched; only the
/// two rules touching `FORWARD` itself stay individual `iptables` calls
/// (a `-C` check, an `-I` only when missing).
pub fn apply(guest_ip: Ipv4Addr, tap_device: &str, uplink: &str, policy: &EgressPolicy) -> io::Result<()> {
    let chain = chain_name(tap_device);

    let mut restore_input = format!("*filter\n:{chain} - [0:0]\n");
    for cidr in &policy.deny_cidrs {
        restore_input.push_str(&format!("-A {chain} -d {cidr} -j DROP\n"));
    }
    for cidr in &policy.allow_cidrs {
        restore_input.push_str(&format!("-A {chain} -d {cidr} -j ACCEPT\n"));
    }
    let default_verdict = match policy.mode {
        EgressMode::AllowAll => "ACCEPT",
        EgressMode::DenyAll => "DROP",
    };
    restore_input.push_str(&format!("-A {chain} -j {default_verdict}\nCOMMIT\n"));
    run_restore(&restore_input)?;

    let guest_ip_str = guest_ip.to_string();
    let jump_args = ["-s", guest_ip_str.as_str(), "-o", uplink, "-j", chain.as_str()];
    let mut check_args = vec!["-C", "FORWARD"];
    check_args.extend_from_slice(&jump_args);
    if !Command::new("iptables").args(&check_args).output()?.status.success() {
        let mut insert_args = vec!["-I", "FORWARD", "1"];
        insert_args.extend_from_slice(&jump_args);
        run("iptables", &insert_args)?;
    }

    Ok(())
}

/// Feeds `input` to `iptables-restore` over stdin. On failure, the error
/// carries stderr, same clarity as a single bad `iptables -A` call.
fn run_restore(input: &str) -> io::Result<()> {
    let mut child = Command::new("iptables-restore").arg("--noflush").stdin(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    child.stdin.take().expect("stdin was piped").write_all(input.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "iptables-restore failed: {}\n--- input ---\n{input}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

/// Tears down the `FORWARD` jump rule and the chain itself. Best-effort —
/// called at lease release, where a missing rule/chain (no policy was
/// ever applied) isn't worth failing the teardown over.
pub fn remove(guest_ip: Ipv4Addr, tap_device: &str, uplink: &str) {
    let chain = chain_name(tap_device);
    let guest_ip_str = guest_ip.to_string();
    let _ = Command::new("iptables").args(["-D", "FORWARD", "-s", &guest_ip_str, "-o", uplink, "-j", &chain]).status();
    let _ = Command::new("iptables").args(["-F", &chain]).status();
    let _ = Command::new("iptables").args(["-X", &chain]).status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_cidr_accepts_well_formed_ipv4_cidrs() {
        assert!(validate_cidr("10.0.0.0/8").is_ok());
        assert!(validate_cidr("192.168.1.1/32").is_ok());
        assert!(validate_cidr("0.0.0.0/0").is_ok());
    }

    #[test]
    fn validate_cidr_rejects_a_missing_prefix() {
        assert!(validate_cidr("10.0.0.0").is_err());
    }

    #[test]
    fn validate_cidr_rejects_an_invalid_address() {
        assert!(validate_cidr("999.0.0.0/8").is_err());
        assert!(validate_cidr("not-an-ip/8").is_err());
    }

    #[test]
    fn validate_cidr_rejects_a_non_numeric_or_out_of_range_prefix() {
        assert!(validate_cidr("10.0.0.0/thirty-two").is_err());
        assert!(validate_cidr("10.0.0.0/33").is_err());
    }

    #[test]
    fn chain_name_is_derived_from_the_tap_device_and_stays_short() {
        let name = chain_name("sktap31");
        assert_eq!(name, "SK-EG-sktap31");
        // Real iptables chain names are capped at 28 usable characters --
        // this project's tap names (`sktap<n>`, n up to a couple digits)
        // must never come close.
        assert!(name.len() <= 28, "chain name {name:?} is too long for iptables");
    }
}
