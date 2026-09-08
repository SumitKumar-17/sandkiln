---
title: "Engineering notebook: real bugs found building this"
description: What broke while building interactive terminals and pre-warmed pools, how each was actually found, and what's still genuinely unresolved.
---

`architecture/bug-hunt-vsock-timeout` tells one story in detail — a stop that could hang forever, found by actually running the failure case rather than in review. This page collects the others, from building interactive terminal access and pre-warmed pools. Same rule as everywhere else on this site: nothing here is stated unless it was actually observed running against real Firecracker/KVM hardware.

## The PTY session that hung for exactly 10 seconds

`kiln sandbox pty` opens a live, bidirectional shell session over a WebSocket. After the remote shell exited, the local CLI process didn't exit — it hung for a fixed 10 seconds every time, only ending because an external `timeout` command killed it.

The first hypothesis was wrong: it looked like a client-side cleanup problem, so the fix attempt was an explicit `process.exit(0)` on the Node side. It didn't help — the hang was identical with or without it, which was the signal that the problem wasn't on the client at all.

The real cause was on the guest side. A PTY session shovels bytes in both directions using two handles to the same vsock connection — one thread reads the shell's output and writes it to the host, the other reads host input and writes it to the shell. Both handles were `try_clone()`d from the same underlying socket. When the shell exited, the output thread noticed (its read returned EOF) and ended, dropping its handle — but a `try_clone()`d handle is a duplicate file descriptor pointing at the *same* kernel socket, and the kernel doesn't tear a socket down until every descriptor referencing it is closed. The other thread, still blocked reading host input that would never arrive, kept the connection open indefinitely.

**The fix**: when the output side detects the shell has exited, it explicitly calls `shutdown(Shutdown::Both)` on the connection — a real socket-level shutdown, not just dropping a handle, which unblocks any other thread sharing that socket immediately. The symmetric case matters just as much: if the *host* disconnects first (a lost connection, a closed browser tab) while the shell is still running, the guest now sends that shell's process group a `SIGHUP` — the same signal a real terminal sends on hangup — so a dropped connection doesn't leave an orphaned shell running forever. Verified with a real stress test that opens a session, disconnects without exiting the shell, and confirms zero orphaned processes survive.

## MMDS forgets who you are after a resume

Building pre-warmed pools meant a sandbox's real identity (its id, name, tags) is only known *after* it's already resumed from a warm snapshot — the snapshot itself was taken from an anonymous placeholder instance. The natural fix looked simple: call Firecracker's `PATCH /mmds` to update the guest-visible metadata to the caller's real identity once the resume completes.

It failed immediately, with a genuinely surprising error: `"MMDS data store is not initialized"` — on a VM that had MMDS fully configured before it was ever snapshotted. Firecracker's snapshot/restore mechanism, it turns out, does not consider MMDS's data store part of what gets restored, even though the VM was networked and MMDS was live at snapshot time.

**The fix**: redo the full initialization sequence post-resume — `PUT /mmds/config` followed by `PUT /mmds` with the real content — instead of assuming a bare `PATCH` would work against already-initialized state. Both calls are cheap and idempotent, so doing the full sequence every time a pool claim succeeds costs nothing extra and is correct whether the VM's MMDS was ever really initialized after restore or not.

## The failure rate that wasn't rare

This is the most significant thing this project has found so far, and it's still not fully explained.

Testing the pool feature meant resuming snapshots far more often, in quick succession, than any prior manual test ever had — and that volume surfaced something the existing test suite had never seen: a resumed guest kernel can panic during early boot. The daemon captures every VM's serial console output to a log file specifically so a guest crash before the vsock channel comes up isn't invisible from the host — reading that log showed a real kernel oops, a divide-by-zero trap inside the console driver's own early initialization path, followed by the whole Firecracker process exiting.

The first few times this happened during testing, it looked like a fluke. It wasn't. Measured directly and repeatedly, running clean, isolated, sequential single-claim tests with no other load on the box: the failure rate ranged from roughly **1 in 3 to 2 in 3** resumes. Not a one-off, not a rare edge case — closer to a coin flip. The project's own existing snapshot/resume integration check still passes on every run, for a simple reason: it only does one resume per run, and a 1-in-3 failure rate doesn't reliably show up in one attempt.

The root cause is still an open question. The working hypothesis is TSC (timestamp counter) or clock-source drift between the moment a snapshot is taken and the moment it's restored — early kernel boot code does timing-sensitive arithmetic that a corrupted or zero-valued frequency value could plausibly divide by zero in. That's an informed guess based on where the panic happens, not a confirmed diagnosis, and it's stated that way deliberately rather than dressed up as solved.

**What actually shipped** doesn't require solving the root cause first: every pool claim runs a real health check (a trivial `exec true`) against the resumed sandbox before it's ever handed back to a caller. A sandbox that fails the check is torn down immediately, and the request transparently falls back to a normal cold create — the caller gets a working sandbox either way, just slower on the unlucky path. A repeated stress test confirmed zero caller-visible failures across every run, with the daemon's own logs showing the fallback path firing at exactly the rate the direct measurements predicted.

## Paying for the same timeout twice

Once the health check above existed, its failure path was slow — about 10.3 seconds to recover from a bad resume, roughly double what the design intended. The health check itself times out (retrying briefly, then giving up) after about 5 seconds when a VM isn't responding. The investigation was straightforward once looked at directly: tearing down the broken sandbox afterward called the normal VM-stop path, which itself opens with an optimistic "sync the filesystem before killing the process" call — a real, previously-justified precaution against losing unflushed writes on a normal stop. Against a VM that had already failed its health check, that call was never going to succeed, and it paid its *own* independent 5-second timeout before giving up.

**The fix**: a `force_stop` path that skips the sync call entirely, used specifically when a VM is already known to be dead — there's nothing to flush on a sandbox that never got the chance to do any real work. This alone roughly halved the fallback's cost, from ~10.3 seconds to ~5.4 seconds.

## Injecting into the wrong file

Not a code bug — a tooling gap that caused a real mistake during this project's own development. Getting a guest-agent change into a running sandbox is a two-step process: build the agent binary for the guest's target, then inject it into a rootfs image file. The second step needs an exact path, and there was more than one plausibly-named `.ext4` file on the dev box for unrelated reasons. The wrong one got injected once — the daemon kept silently booting from the old, un-updated image, and the mismatch wasn't obvious until sandboxes didn't behave like the just-built code should have.

**The fix** was at the tooling level, not just "be more careful": a single `scripts/dev.sh inject-agent` command that resolves the daemon's own actual configured default rootfs path automatically, so this specific class of mistake — updating the wrong file because a path had to be remembered by hand — can't recur.

## Currently open

Honest status on what's still unresolved, not swept into a changelog and forgotten:

- **The guest-kernel-panic-on-resume root cause is unconfirmed.** TSC/clock-source drift is a hypothesis, not a diagnosis. Whether this is specific to this dev box's kernel/KVM/CPU combination, this project's own guest kernel build, or a broader Firecracker snapshot/restore characteristic is genuinely open, and worth real investigation before assuming it generalizes to other hardware.
- **Pool configuration is in-memory only** — it doesn't survive a daemon restart the way snapshot records do, and a restart can orphan an already-warm snapshot with no pool configuration left to claim or clean it up.
- **Jailer hardening is opt-in and still not proven on real hardware.** The first real-hardware attempt failed outright (every sandbox create returned `500`) because the `jailer` binary itself needs a one-time `setuid-root` step that hadn't been applied yet — root-caused via the same console-log capture used above, not guessed at. Not recommended as-is for a genuinely adversarial workload until it's actually verified end to end.
- See the Roadmap page on the main site and the repository's own `ROADMAP.md` for the full, current list of what's shipped, partial, and not started — this page covers what broke and got fixed, not the complete feature status.
