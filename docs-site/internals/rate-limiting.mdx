---
title: "Internals: token-bucket rate limiting"
description: What a token bucket actually is, and why sandkiln exposes one simplified knob over Firecracker's own four independent limiters.
---

## What it is

A token bucket is a simple, widely-used algorithm for capping a rate of something (bytes, requests, operations) while still allowing short bursts. Picture a bucket that holds up to `size` tokens: every operation consumes one token (or `N` tokens, for `N` bytes transferred), and the bucket refills at a steady rate. If the bucket is full, a burst goes through immediately, limited only by `size`; once it's empty, further operations wait for tokens to trickle back in at the refill rate. It's the same shape used by countless network shapers and API rate limiters, not something specific to Firecracker or sandkiln.

## Why sandkiln uses it here

Firecracker implements token-bucket limiting natively as part of its device model. Every virtio-block device (the rootfs drive, and any extra drives) and the virtio-net device both accept an optional `rate_limiter` in their configuration, enforced by Firecracker itself before I/O ever reaches the host. sandkiln doesn't implement any limiting logic of its own: it just fills in Firecracker's own configuration correctly, which is a real advantage of running on real virtualization rather than a container's shared kernel, the limiter lives at the hypervisor boundary, not in an eBPF program or a cgroup the guest could potentially observe or route around.

## Key terms

- **`size`**: the bucket's maximum capacity, in whatever unit the bucket measures, bytes for a bandwidth bucket, operations for an ops bucket.
- **`refill_time`**: milliseconds for the bucket to go from empty back to full. sandkiln always sets this to `1000`, so `size` doubles as "the sustained rate per second" and there's only one number for a caller to reason about.
- **`one_time_burst`**: an initial allowance consumed before the steady refill rate applies, letting the very first burst be larger than `size` alone would allow. sandkiln never sets this (`None`) -- it's part of Firecracker's own `TokenBucket` schema, exposed at the `sandkiln_vmm` crate level (`sandkiln_vmm::vm::{RateLimiter, TokenBucket}`) for a future caller with a real need, but not surfaced through the daemon's own API today.
- **`rx_rate_limiter` / `tx_rate_limiter`**: Firecracker limits network ingress and egress independently -- two separate buckets, one per direction. sandkiln applies the same limiter to both rather than exposing four independent knobs (drive bandwidth, drive ops, network rx, network tx): a deliberately simpler sandbox-level surface, not a limitation of what the device model itself can do.

## How it works in sandkiln

A request's `rate_limit: { bandwidth_bytes_per_sec, ops_per_sec }` (at least one required if the field is present at all, `0` rejected outright rather than treated as "unlimited") is resolved into Firecracker's own `TokenBucket` shape by `routes_sandbox.rs::resolve_rate_limit`, then applied uniformly by `sandkiln_vmm::vm::boot::insert_rate_limiter` to four places in the boot configuration: the rootfs drive's `rate_limiter`, every extra drive's `rate_limiter`, and the network interface's `rx_rate_limiter` and `tx_rate_limiter`, each as its own independent `PUT` during `Vm::boot`'s configuration sequence. `None` (the field omitted entirely) means unlimited host I/O, today's unchanged default.

Because a warm-pool snapshot has no rate limit baked into its boot-time state (Firecracker's device configuration is fixed at boot, not something a snapshot can change on resume), a create that requests a custom `rate_limit` can never match a pre-warmed pool -- it always falls through to a normal cold create instead, regardless of whether a pool exists for that profile.

## See it in action

```
$ curl -si -X POST http://127.0.0.1:7777/sandboxes \
    -H 'content-type: application/json' \
    -d '{"rate_limit":{"bandwidth_bytes_per_sec":1000000}}'

HTTP/1.1 200 OK
content-type: application/json
x-request-id: becb52af-4567-41ae-9c88-d0ae6831bfb9
content-length: 45
date: Wed, 16 Sep 2026 05:24:15 GMT

{"id":"6c49ece5-418a-4366-85a7-fab6001518e2"}
```

Behind that one field, four separate `TokenBucket { size: 1000000, one_time_burst: None, refill_time: 1000 }` values were written into Firecracker's own boot configuration: the rootfs drive, the network's rx side, and the network's tx side all capped at one million bytes per second, refilling once a second. There's no separate endpoint to inspect a live sandbox's configured limiter; it's set once at boot and stays fixed for that VM's lifetime, matching Firecracker's own model.
