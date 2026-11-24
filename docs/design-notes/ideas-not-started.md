# Ideas explicitly not started

Kept honest rather than silent. This is a personal project with no fixed
deadline, so none of these is "next"; each is closer to a separate product
surface than a small extension.

## Rootfs/CoW crate with explicit dirty tracking (nearest-term)

Prompted by a public write-up of another sandbox platform's VM filesystem
work: not reused code, just the observation that they hit the same problem
and solved it with an explicit mechanism instead of a filesystem feature
that fails silently. Their approach (lazily fetching sparse ext4 blocks from
remote blob storage across a fleet) solves a distribution problem a single
self-hosted daemon does not have.

What would transfer: one crate that owns rootfs provisioning (today a couple
of free functions split across the daemon's sandbox routes and
`vmm::image`) and tracks per-sandbox writes against the base image with an
explicit dirty bitmap over a sparse file, the shape Firecracker's diff
snapshots use for memory. "Did this clone share blocks with its parent"
becomes a checkable fact, and the same bookkeeping would make a cheaper
rootfs snapshot possible. Cost: a new crate, an on-disk format and a
migration for every existing snapshot. It strengthens the path-based
snapshot/resume/fork model rather than replacing it. The configurable clone
destination (finding 4) was the smaller step in front of it and is done.

## Desktop/GUI automation

A managed desktop (windowing environment plus browser, over VNC or a
browser remote-desktop client), a screenshot API, a keyboard/mouse input
API, and reconnecting to a running desktop sandbox by id. Use cases:
browser-based agents and computer-use evals. Needs its own base image
(desktop environment, VNC server, input-injection tooling).

## Git-native sandbox filesystem

Version-controlled sandbox state: workspaces as private branches mounted as
POSIX directories, a snapshot doubling as a commit, merging by moving a
pointer, read-only mounts shared across sandboxes (following a moving head
or pinned to a snapshot). A substantial new subsystem (a git server and
object store) layered beside snapshot/resume/fork, not replacing them.

## Multi-node and regions

Everything assumes one daemon on one box. Multiple daemons, each owning its
bridge, tap pool and rootfs storage, with a routing layer that knows where
each sandbox lives (no migration; a sandbox is tied to its node). A "region"
identifier a caller can request, even if early on it only means which
physical box. Deliberately last: it multiplies the surface of networking,
images and storage, so it lands after those are solid on one node.

## Ecosystem and integrations

- Example integrations with agent frameworks and coding-agent tools.
- A plugin or adapter surface, once more than one real integration exists
  to generalize from, not before.
- Out of scope: a durable-workflow layer (queues, timers, retries, fan-out
  across sandboxes). It is an abstraction on top of the primitive and belongs
  in a separate library built against this API.

## Not planned

- GPU access in a sandbox (no Firecracker device model).
- OCI/Docker image conversion into a rootfs: a custom image today must
  already be a bootable ext4, built with `images/build-universal-image.sh`
  or handed in directly.
