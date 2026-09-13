//! Per-sandbox egress (outbound) network policy: IP/CIDR allow and deny
//! lists layered on top of `network.rs`'s shared bridge-wide NAT/forward
//! setup, which stays completely untouched for any sandbox that doesn't
//! opt in — this only ever adds rules ahead of that existing catch-all
//! `ACCEPT`, never modifies it.
//!
//! One dedicated iptables chain per sandbox (named from its tap device,
//! already a short, unique-per-lease identifier — `sandkiln_vmm::network`
//! never reuses one while it's leased) rather than juggling rule numbers
//! in the shared `FORWARD` chain: every rule this policy needs lives in
//! that one chain, in a fixed, always-correct order (deny rules, then
//! allow rules, then the base-mode default), and the *only* thing
//! touching `FORWARD` itself is one `-I FORWARD 1` jump rule routing this
//! sandbox's own outbound traffic (matched by its unique `guest_ip`, not
//! its tap — the existing bridge-wide rules already show traffic is
//! evaluated post-bridging, where interface matching means the bridge
//! itself, not the originating tap) into that chain before the general
//! `ACCEPT` ever gets a chance to short-circuit it.
//!
//! **Deny always wins on overlap** — not because of any special-casing,
//! just rule order: deny rules are always appended to the chain before
//! allow rules, and iptables evaluates a chain top-to-bottom, first
//! match wins.
//!
//! **Scoped to `-o <uplink>` only, matching the existing bridge-wide
//! rule's own scoping** — gateway-bound traffic (DNS to the bridge's own
//! IP) never transits the uplink at all, so it's structurally unaffected
//! by any egress policy here without needing an explicit exemption; a
//! `DenyAll` sandbox can still resolve names, it just can't reach
//! anything past the gateway that isn't explicitly allowed.
//!
//! **Deliberately not built in this first slice**: domain-level rules
//! (would need the shared DNS proxy to become source-IP-aware, a
//! separate, larger change — see `ROADMAP.md`'s "Firewall and egress
//! policy" section) and port-level matching (`-p tcp --dport`, a
//! straightforward future extension of the same rule shape, just not
//! part of this pass). IPv4 only, matching every other networking type
//! in this crate.
//!
//! **Lifecycle**: applied once a lease is actually in active use for a
//! live VM (a fresh boot, or a resume/fork reusing a snapshot's retained
//! lease) and removed only when that lease is finally released back to
//! `NetworkManager`'s free pool — not on a mere snapshot-and-stop, which
//! leaves the lease (and so the tap device and this chain) dormant but
//! intact, exactly like the tap device itself stays attached-but-unused
//! through that window. `apply` is idempotent (flushes and repopulates an
//! already-existing chain rather than erroring) specifically so a resume
//! can call it unconditionally without needing to know whether the chain
//! already survived from before — true after a plain daemon restart
//! (iptables state lives in the kernel, not the daemon process) and
//! harmless to repeat after a real host reboot (nothing survived to
//! flush).

use crate::network::run;
use serde::{Deserialize, Serialize};
use std::io;
use std::net::Ipv4Addr;
use std::process::Command;
use std::str::FromStr;

/// `Serialize`/`Deserialize` here (and on `EgressPolicy` below) are for
/// `crate::snapshot::SnapshotMeta` in the daemon crate — a sandbox's
/// egress policy is carried through snapshot/resume/fork exactly like
/// its tags/name/drives are, so it doesn't silently disappear (a real
/// security regression, not just a convenience gap) the moment a
/// protected sandbox is ever snapshotted and resumed.
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

/// One sandbox's egress policy — see this module's own doc comment for
/// the full design. `allow_cidrs`/`deny_cidrs` are already-validated
/// (`validate_cidr`) IPv4 CIDR strings, checked at the API boundary
/// (`routes_sandbox::CreateSandboxRequest`) so this module never has to
/// reject one itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressPolicy {
    pub mode: EgressMode,
    pub allow_cidrs: Vec<String>,
    pub deny_cidrs: Vec<String>,
}

/// Validates an IPv4 CIDR string (`"10.0.0.0/8"`) without needing a new
/// crate dependency — iptables itself already understands CIDR notation
/// natively, so this only ever exists to reject a malformed policy with
/// a clear error at request time instead of a cryptic iptables failure
/// bubbling up from deep inside a boot task.
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

/// Installs (or, if this sandbox's chain already exists from a prior
/// call — see this module's own doc comment on why that's expected, not
/// an error — re-installs) `policy` for the sandbox holding `guest_ip`
/// on `tap_device`, egressing via `uplink`. Idempotent: safe to call
/// unconditionally on every resume/fork, not just a fresh boot.
pub fn apply(guest_ip: Ipv4Addr, tap_device: &str, uplink: &str, policy: &EgressPolicy) -> io::Result<()> {
    let chain = chain_name(tap_device);

    if chain_exists(&chain)? {
        run("iptables", &["-F", &chain])?;
    } else {
        run("iptables", &["-N", &chain])?;
    }

    for cidr in &policy.deny_cidrs {
        run("iptables", &["-A", &chain, "-d", cidr, "-j", "DROP"])?;
    }
    for cidr in &policy.allow_cidrs {
        run("iptables", &["-A", &chain, "-d", cidr, "-j", "ACCEPT"])?;
    }
    let default_verdict = match policy.mode {
        EgressMode::AllowAll => "ACCEPT",
        EgressMode::DenyAll => "DROP",
    };
    run("iptables", &["-A", &chain, "-j", default_verdict])?;

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

/// Tears down everything `apply` set up for this sandbox — the jump rule
/// in `FORWARD` and the chain itself. Best-effort: called from a
/// sandbox's final teardown (lease release), where a missing rule/chain
/// (e.g. this sandbox never actually had a policy applied) isn't an
/// error worth failing the whole teardown over.
pub fn remove(guest_ip: Ipv4Addr, tap_device: &str, uplink: &str) {
    let chain = chain_name(tap_device);
    let guest_ip_str = guest_ip.to_string();
    let _ = Command::new("iptables").args(["-D", "FORWARD", "-s", &guest_ip_str, "-o", uplink, "-j", &chain]).status();
    let _ = Command::new("iptables").args(["-F", &chain]).status();
    let _ = Command::new("iptables").args(["-X", &chain]).status();
}

fn chain_exists(chain: &str) -> io::Result<bool> {
    Ok(Command::new("iptables").args(["-L", chain, "-n"]).output()?.status.success())
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
