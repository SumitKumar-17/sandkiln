---
title: iptables and per-sandbox egress policy
description: What an iptables chain actually is, why sandkiln gives every sandbox its own, and a real allow_cidrs entry opening a blocked address back up.
---

## What it is

`iptables` is the traditional Linux command-line tool for configuring the kernel's netfilter packet-filtering rules (this project uses the legacy, `nf_tables`-backed `iptables`, not the newer native `nft` syntax, though both drive the same underlying kernel subsystem). A **chain** is a named, ordered list of **rules**. The kernel walks a chain top to bottom for every matching packet and stops at the first rule that matches, applying that rule's **target**: commonly `ACCEPT` (let it through), `DROP` (silently discard, no response sent back at all), or `REJECT` (discard, but tell the sender via an ICMP/TCP reset). `FORWARD` is one of the built-in chains the kernel always consults for traffic being routed *through* a host rather than delivered to or sent from it, exactly the traffic pattern for a sandbox's outbound packets crossing the host's bridge to reach the outside network.

## Why sandkiln uses it here

A sandbox running untrusted or AI-generated code is exactly the kind of workload where "what can this thing talk to on the network" is a real security question, not a nice-to-have. sandkiln answers it per sandbox: an optional `egress` policy on `POST /sandboxes` that can block everything by default and open specific destinations back up, or allow everything by default and block specific ones. iptables is the natural mechanism for this because the daemon already relies on the kernel's own bridge/NAT/forwarding setup for sandbox networking (see [TAP devices and bridge networking](../tap-bridge-networking/)). A per-sandbox policy is just more rules layered on top of infrastructure that already exists, not a new enforcement mechanism.

## Key terms

- **Chain.** A named, ordered list of rules the kernel evaluates in sequence for matching packets.
- **Target.** What a matched rule actually does: `ACCEPT`, `DROP`, `REJECT`, or jump to another chain.
- **`FORWARD`.** The built-in chain for traffic being routed through the host, not to or from it.
- **CIDR.** A compact way to write an IP address range (`10.0.0.0/8` means every address starting `10.`), used here for both `allow_cidrs` and `deny_cidrs`.
- **First-match-wins evaluation.** The kernel stops at the first rule in a chain that matches a packet; rule *order* inside a chain is what determines behavior on overlap, not any explicit priority field.

## How it works in sandkiln

`sandkiln_vmm::egress` gives every sandbox that requests a policy its own dedicated chain, named from its tap device (`SK-EG-<tap_device>`), a short, already-unique-per-lease identifier since `sandkiln_vmm::network` never reuses a leased tap device while it's in use. Everything the policy needs lives inside that one chain, in a fixed order: every `deny_cidrs` rule first, then every `allow_cidrs` rule, then the mode's own base default (`ACCEPT` for `allow_all`, `DROP` for `deny_all`). Because iptables evaluates top to bottom and stops at the first match, putting deny rules first means **deny always wins on overlap**, purely because of where those rules sit in the list, not because of any special-casing in the code.

The *only* thing this design touches in the shared, bridge-wide `FORWARD` chain is one `-I FORWARD 1` jump rule per sandbox, matched by that sandbox's own unique guest IP, routing its outbound traffic into its own chain before the bridge's existing catch-all `ACCEPT` rule ever gets a chance to short-circuit it. Every rule is additionally scoped to `-o <uplink>` (the host's real outbound interface), matching the existing bridge-wide rule's own scoping, which has a genuinely useful side effect: gateway-bound traffic (DNS queries to the bridge's own IP) never transits the uplink interface at all, so it's structurally exempt from any egress policy without needing an explicit allowlist entry. A `deny_all` sandbox with an empty `allow_cidrs` can still resolve names; it just can't reach anything past the gateway that isn't explicitly allowed.

Applying a policy used to mean one `iptables` subprocess spawn per rule: the chain create/flush, each `deny_cidrs` entry, each `allow_cidrs` entry, the default verdict, every one of them a real fork+exec paying the same process-startup cost regardless of how little work it actually does. Measured live on a real box, a 6-CIDR policy under that shape cost **~8.3ms average per create**. It's now loaded as a single `iptables-restore --noflush` call instead, with only the two rules that touch the *shared* `FORWARD` chain left as individual `iptables` calls (a `-C` existence check, and an `-I` only when it's actually missing). `--noflush` matters specifically because a full-table restore without it would wipe every other sandbox's own chain along with the one being populated. The same policy shape now measures **~3.1ms average**, a real ~63% cut, made visible going forward via a dedicated `egress_apply` phase in the daemon's own `/metrics` output.

A policy is tied to a sandbox's network *lease*, not to the VM process itself. `apply()` runs again on every resume or fork that reuses a snapshot's retained lease, not just on a fresh boot, and it's written to be idempotent (flushing and repopulating an already-existing chain rather than erroring) specifically so a resume can call it unconditionally without first checking whether the chain survived. It genuinely might have: iptables state lives in the kernel, independent of the daemon process, so a plain daemon restart never touches it. Only a real host reboot wipes it, at which point re-applying from scratch is exactly correct too.

## See it in action

Creating a sandbox with `deny_all` plus one explicitly allowed address, then proving the policy's actual effect from inside the guest, not just that the create call succeeded:

```
$ curl -s -X POST http://127.0.0.1:7777/sandboxes -H 'content-type: application/json' \
    -d '{"egress":{"mode":"deny_all","allow_cidrs":["10.5.31.2/32"]}}'
{"id":"354dbc49-b764-45a9-a90a-68903ddbade0"}
```

Pinging the one allowed address from inside that sandbox:

```
$ curl -s -X POST http://127.0.0.1:7777/sandboxes/354dbc49.../exec \
    -H 'content-type: application/json' -d '{"command":"ping","args":["-c1","-W2","10.5.31.2"]}'

{"stdout":"PING 10.5.31.2 (10.5.31.2) 56(84) bytes of data.\n64 bytes from 10.5.31.2: icmp_seq=1 ttl=253 time=1.05 ms\n\n--- 10.5.31.2 ping statistics ---\n1 packets transmitted, 1 received, 0% packet loss, time 0ms\nrtt min/avg/max/mdev = 1.045/1.045/1.045/0.000 ms\n","stderr":"","exit_code":0}
```

Pinging a *different* address, never listed in `allow_cidrs`, from the same sandbox: blocked, not merely unreachable for some unrelated reason.

```
$ curl -s -X POST http://127.0.0.1:7777/sandboxes/354dbc49.../exec \
    -H 'content-type: application/json' -d '{"command":"ping","args":["-c1","-W2","10.5.31.1"]}'

{"stdout":"PING 10.5.31.1 (10.5.31.1) 56(84) bytes of data.\n\n--- 10.5.31.1 ping statistics ---\n1 packets transmitted, 0 received, 100% packet loss, time 0ms\n\n","stderr":"","exit_code":1}
```

100% packet loss, `exit_code: 1`. The `deny_all` default is doing exactly what it says for everything not on the allowlist, while the one address in `allow_cidrs` gets through cleanly. For a full runnable version of this exact check, including the `allow_all`-with-no-policy baseline case, see `examples/egress-policy` in the repo.

A day-to-day, user-facing guide for setting an egress policy at create time doesn't exist yet as its own docs page. This page covers the internal mechanism, not the how-to.
