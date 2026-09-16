---
title: TAP devices and bridge networking
description: What a TAP device and a Linux bridge actually are, why sandkiln pre-creates a fixed pool of them, and a real sandbox's live network assignment.
---

## What it is

A **TAP device** is a virtual network interface that looks, to the kernel and to any userspace program, exactly like a real Ethernet card. Instead of a physical wire on the other end, whatever process opened `/dev/net/tun` to create it can read and write raw Ethernet frames directly. Firecracker uses one TAP device per microVM: from the guest's point of view, its virtio-net device is just talking to a normal network link, and Firecracker itself is the process on the other end shuttling those frames to and from the TAP device on the host.

A **Linux bridge** is a virtual network switch. Any number of interfaces (TAP devices, physical NICs) can be attached to one bridge, and the kernel forwards Ethernet frames between them the way a real network switch would, based on MAC addresses. Attaching several sandboxes' TAP devices to one bridge is what lets them all reach the outside network through a single, shared gateway IP and NAT setup, rather than each needing its own.

## Why sandkiln uses it here

The straightforward design would create a fresh TAP device for every sandbox at boot time and destroy it on teardown. sandkiln doesn't do that, for a concrete, load-bearing reason: creating a *new* TAP device is a `TUNSETIFF` ioctl on `/dev/net/tun`, and that specific ioctl does not work under this daemon's ambient `CAP_NET_ADMIN` capability in practice. It wants a genuinely privileged (root) process, unlike the netlink operations (attaching a TAP to a bridge, bringing a link up, creating the bridge itself) that ambient `CAP_NET_ADMIN` does cover. Since the daemon deliberately never runs as root (see [the jailer and privilege model](../jailer-privilege-model/) for the same reasoning applied one layer down, to the VM process itself), it can't create TAP devices on demand at all.

The fix is a pool: a one-time setup script (`scripts/host-setup/create-tap-pool.sh`), run once as real root, pre-creates a fixed number of persistent TAP devices up front. From then on, the daemon only ever *attaches* or *detaches* an already-existing device from that pool, both plain netlink operations covered by ambient `CAP_NET_ADMIN`. One shared bridge on top of that pool means the NAT and DNS-proxy setup only ever has to target one interface and one gateway IP, rather than wildcarding across however many TAP devices might exist at any moment.

## Key terms

- **TAP device.** A virtual Ethernet interface backed by a userspace process instead of physical hardware.
- **`TUNSETIFF`.** The specific ioctl on `/dev/net/tun` that creates a new TAP (or TUN) device, the one operation in this whole networking setup that needs full root, not just `CAP_NET_ADMIN`.
- **Ambient capability.** A Linux capability granted to a process (and everything it execs) without that process needing to run as root at all. `CAP_NET_ADMIN` here covers netlink-based network administration, not device creation.
- **Bridge.** A virtual switch; multiple interfaces attached to one bridge can reach each other and, via NAT, the outside network.
- **Lease.** sandkiln's own term (`sandkiln_vmm::network::Lease`) for one sandbox's currently-held TAP device plus its assigned IP, held for as long as that sandbox (or a snapshot descended from it) is using that network identity.

## How it works in sandkiln

`sandkiln_vmm::network::NetworkManager` owns two free pools: leftover TAP device names and leftover host IP octets. `Lease::reserve` pops one of each, attaches the TAP to the shared bridge (three separate `ip`/`bridge` subprocess calls, each timed individually and measured at roughly 1.4ms each, ~4.3ms total, small enough that batching them or reimplementing via direct netlink calls isn't planned), and hands back a `Lease` carrying that sandbox's tap device name, guest IP, gateway IP, and MAC address. Releasing a lease (on full sandbox teardown) detaches the tap and returns both the device name and the IP octet to their respective free pools for the next sandbox to reuse.

A snapshotted-and-resumed sandbox is a special case worth naming explicitly: the guest's IP and MAC are finalized via kernel boot arguments at the *original* boot and become part of the snapshotted memory image itself, so whatever resumes that snapshot has to reattach the exact same TAP device the original sandbox held, not a fresh one from the pool. `Vm::resume` and the daemon's snapshot bookkeeping account for this by keeping the lease tied to the snapshot, not releasing it back to the pool until the snapshot itself is deleted or consumed.

The fixed pool size is a real, explicit ceiling. It caps how many sandboxes can be networked concurrently until the pool is grown, and growing it is a deliberate, root-requiring step run outside the daemon's own runtime (re-running `create-tap-pool.sh` with a larger count), not something the daemon can do for itself while running. That's treated as the correct shape for this constraint, not a workaround waiting to be routed around.

## See it in action

The bridge itself, and the pool of persistent TAP devices sitting idle (state `DOWN`, not yet attached to any live sandbox) before any sandbox exists:

```
$ ip addr show sktapbr0
4: sktapbr0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 ...
    inet 172.16.0.1/24 scope global sktapbr0

$ ip link show | grep sktap | head -3
5: sktap0: <BROADCAST,MULTICAST> mtu 1500 ... state DOWN ...
6: sktap1: <BROADCAST,MULTICAST> mtu 1500 ... state DOWN ...
7: sktap2: <BROADCAST,MULTICAST> mtu 1500 ... state DOWN ...
```

Creating a real sandbox, then checking which TAP device it was actually leased and attached to:

```
$ curl -s -X POST http://127.0.0.1:7777/sandboxes -H 'content-type: application/json' -d '{}'
{"id":"354dbc49-b764-45a9-a90a-68903ddbade0"}

$ ip link show | grep sktap | grep -v DOWN
4: sktapbr0: <BROADCAST,MULTICAST,UP,LOWER_UP> ...
9: sktap4: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc fq_codel master sktapbr0 state UP ...
```

`sktap4` went from `DOWN` and unattached to `UP` with `master sktapbr0`: leased from the pool and attached to the bridge for this one sandbox, exactly as described above. Destroying the sandbox returns it to the free pool and back to `DOWN`.
