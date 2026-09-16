---
title: The jailer and privilege model
description: What chroot, cgroups, and setuid actually do, why Firecracker's jailer drops privilege instead of running privileged, and the real state of this on sandkiln today.
---

## What it is

Three separate Linux mechanisms work together here, each solving a different piece of "run this process with less power than the one that started it":

- **chroot(2)** changes what a process believes its own filesystem root (`/`) is. Once called, the process (and anything it execs afterward) can no longer see or open any path outside that new root, not because permissions forbid it, but because the kernel simply won't resolve a path outside the chroot at all.
- **cgroups (control groups), v2** are a kernel mechanism for capping how much of a resource (CPU time, memory, number of processes) a group of processes can consume in total, enforced by the kernel itself rather than trusted to the process's own behavior.
- **setuid-root** is a file permission bit that makes a binary run as the `root` user regardless of who executes it. It's how an otherwise-unprivileged user can run one specific, narrowly-scoped privileged operation without being root themselves: the classic example is `passwd`, which needs to write `/etc/shadow` but is run by ordinary users.

## Why sandkiln uses it here

The sandkiln daemon deliberately never runs as root. It runs as an ordinary user with exactly one Linux capability raised into its ambient set, `CAP_NET_ADMIN` (see [TAP devices and bridge networking](../tap-bridge-networking/) for what that capability actually covers). The reasoning is blast radius: this daemon's whole job is booting VMs to run code nobody vouched for, and if a bug in that path ever let an attacker influence what the daemon itself does, the gap between "can mess with network interfaces" and "can do anything root can" is the gap between an incident and a full host compromise.

Firecracker's own jailer exists for exactly the same reason, one layer down. It does the *privileged* setup a VM needs (creating the chroot, setting cgroup limits, creating device nodes for `/dev/kvm` and `/dev/net/tun` inside the jail), then drops every one of those privileges and execs the real `firecracker` binary as a dedicated, unprivileged uid/gid. The VM process itself never runs privileged; only the brief setup window before it does.

## Key terms

- **jailer.** Firecracker's own small, purpose-built binary that does privileged one-time setup then re-execs `firecracker` unprivileged. Ships as part of Firecracker, not something sandkiln wrote.
- **Chroot jail.** The directory a jailed process is confined to seeing as `/`. Every file a VM's config references (kernel image, rootfs, any extra drives) has to already exist inside this jail before the VM boots.
- **cgroup v2.** The current cgroups API, as opposed to the older, now-legacy cgroups v1, which sandkiln's jailer support targets exclusively.
- **uid/gid pair.** The dedicated, unprivileged user and group id jailer drops into before exec'ing `firecracker`. sandkiln allocates a distinct pair per VM from a pool so concurrently running jailed VMs never share one.
- **Hard link (vs. copy).** How a host file is made to appear inside the chroot: linking the same inode under a new path inside the jail, which is instant and uses no extra disk space, falling back to a real copy only when the jail and the source file are on different filesystems (hard links can't cross a filesystem boundary).

## How it works in sandkiln

`sandkiln-vmm::jailer` implements this as an alternative boot path to the daemon's default direct spawn. `Vm::boot` either runs `Command::new(firecracker_bin)` directly (today's default, `SANDKILN_JAILER_ENABLED` unset) or launches jailer instead, which itself execs `firecracker` after dropping privilege. `JailerIdPool` allocates the distinct uid/gid pairs, mirroring the same pool-based allocation pattern `sandkiln_vmm::network` uses for tap devices and IPs. `link_resource_into_jail` places one host file inside a given VM's chroot and reports back the in-jail path; every subsequent Firecracker API call for that VM (`/boot-source`'s kernel path, `/drives/*`'s path, `/vsock`'s socket path) has to use that in-jail path, not the real host one, since once `chroot()` is called the process can no longer resolve the host path at all.

There's a real, structural reason the daemon can't do any of this itself without jailer: chroot, setuid/setgid, and creating device nodes inside the jail all need privileges the daemon deliberately doesn't have. The standard way to hand an unprivileged process access to a narrow, specific privileged operation is exactly the setuid-root pattern described above. jailer is that small, purpose-built binary, not the whole daemon.

**Where this stands today, stated plainly.** jailer support is opt-in (`SANDKILN_JAILER_ENABLED`) and off by default, so every sandbox still boots via a direct Firecracker spawn unless explicitly turned on. It's built, and its own logic (chroot path-rewriting, cgroup limit calculation, uid/gid pooling) has unit tests, but it is **not yet proven working end to end on real hardware**. The one real-hardware attempt so far (2026-09-08) failed every `POST /sandboxes`: jailer itself hit `Operation not permitted` trying to `chown` a hard-linked file into the chroot, because the `jailer` binary needs to be made setuid-root as a separate, one-time manual step (`SELF_HOSTING.md`'s "Optional: jailer-based sandbox boot") that simply hadn't been done yet on that box. That's not a code bug in sandkiln's jailer integration. It's an unmet setup precondition, caught live rather than assumed, but it does mean jailer-based booting should be verified on your own hardware before relying on it for a genuinely adversarial workload, not treated as already proven. Snapshotting a jailed sandbox also isn't supported (`400`): jailer support covers the initial boot only, and `Vm::resume` always spawns directly regardless of whether the original sandbox was jailed.

## See it in action

Checking whether jailer mode is on for the currently running daemon, and what a plain (non-jailed) sandbox create looks like today, the default, verified-working path:

```
$ curl -s -i -X POST http://127.0.0.1:7777/sandboxes -H 'content-type: application/json' -d '{}'

HTTP/1.1 200 OK
content-type: application/json
x-request-id: <uuid>
content-length: 45
date: <date>

{"id":"<uuid>"}
```

The daemon's own startup log states which path is active for the whole process, not per request, since this is a boot-time configuration choice, not something a caller selects per sandbox:

```
jailer-based sandbox boot disabled (SANDKILN_JAILER_ENABLED not set) -- sandboxes boot via a direct Firecracker process spawn, see ROADMAP.md's Security hardening section
```

Turning jailer on (`SANDKILN_JAILER_ENABLED=1`) without first making the `jailer` binary setuid-root reproduces the real failure documented above: every create fails with a `500` and the guest console log shows jailer's own `Operation not permitted` trying to `chown` a file into the chroot. That failure is the concrete, live evidence behind "not yet proven on real hardware" above, not a hypothetical caveat.
