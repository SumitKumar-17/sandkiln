---
title: "Bug hunt: the vsock timeout that could hang a stop forever"
description: A real bug found by actually running a full snapshot-then-stop cycle.
---

Stopping a VM sends an `exec sync` command to the guest agent before killing the Firecracker process — added because a raw `SIGKILL` can leave writes to an attached drive sitting unflushed in the guest's page cache, silently losing data on anything the caller expected to persist. That was correct on its own. It became a problem the moment snapshot/resume existed: Firecracker requires a VM to be paused before it will accept a snapshot request, and pausing a VM halts its vCPUs outright. A halted vCPU can't run the guest agent, which means it can't answer *any* vsock request — including the sync call that stop always sends first.

The vsock client had no read or write timeout on its socket at all, so that one call blocked forever at the kernel level, with nothing in user space to interrupt it. An existing 5-second deadline in the retry logic one layer up didn't help — it only bounds the time *between* connection attempts, useful while waiting for the agent to come up after boot, not a single attempt already in progress that never returns. The result, found by actually running a full snapshot-then-stop cycle rather than in review: pause, then stop, then hang, indefinitely, every time.

**The fix** was a 3-second read and write timeout set directly on the vsock stream, applied unconditionally to every call the client makes — not a special case for the snapshot path.

That's the point: any unresponsive guest — paused, crash-looped, kernel-panicked — and any future call site that talks over this same client, would have hit the identical indefinite hang without it. Fixing it once at the transport layer means every current and future caller inherits a bounded worst case for free, instead of each new code path needing to remember to guard against a problem that's really about the transport, not about snapshots specifically. Verified live: a full snapshot-to-resume cycle now completes in roughly 7 bounded seconds instead of never, with resume itself measured at 39ms.
