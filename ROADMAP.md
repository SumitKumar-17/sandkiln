# Roadmap

This is a working plan, not a spec — it gets rewritten as we learn things.
Every milestone below ends with something concretely proven on real
hardware, not just code that compiles.

## What works today

- A Firecracker microVM boots under real KVM in ~11ms (was ~32ms until a
  fixed 20ms socket-wait sleep was found and fixed — see Benchmarking),
  with a guest agent running inside it that answers `exec` / `read_file` /
  `write_file` / `list_dir` over vsock.
- **Current sandbox launch latency, measured, not estimated**: a full
  `POST /sandboxes` create averages **~144ms** (was ~168ms before the same
  fix; the load-test table in Benchmarking predates both fixes and hasn't
  been cleanly re-run yet) — a real per-phase profiling pass found the
  rootfs clone is 74% of that, not the network lease this project had
  suspected (~4ms). See Benchmarking below for the full breakdown and the
  honest correction to an earlier, wrong conclusion about why a CoW
  filesystem test didn't change end-to-end latency.
- An HTTP daemon (`sandkilnd`) manages the full lifecycle — create, exec,
  list, stop — driving Firecracker directly from Rust rather than shelling
  out.
- Structured logging throughout: VM lifecycle events carry timing, HTTP
  requests are traced, everything is correlatable and filterable.
- Every sandbox gets real networking: its own tap device leased from a
  pool, attached to a shared bridge, its own IP, NAT'd outbound access,
  and DNS through a host-local proxy. Proven with two sandboxes running
  and reaching the internet concurrently through the daemon's HTTP API.
- A real JS/TS SDK — [`sandkiln` on npm](https://www.npmjs.com/package/sandkiln),
  published — `Sandbox.create()`, `runCommand()`, `stop()`, `Sandbox.list()`
  — verified end to end against the live daemon, not just typechecked in
  isolation.
- `criterion` benchmarks for boot time and exec latency, and a concurrent
  load-test script against the daemon's HTTP API.
- Snapshot/resume/fork, durable across a daemon restart, with a tiered
  idle lifecycle (auto-suspend, then archive) on top; a host-side reverse
  proxy for previewing a dev server running inside a sandbox; interactive
  PTY sessions over WebSocket; pre-warmed snapshot pools with a
  `max_count` ceiling and queueing; per-sandbox resource overrides with
  enforced ceilings; request-id correlation and a `/metrics` endpoint;
  opt-in Firecracker jailer hardening; full filesystem operations;
  persistent drives with read-only sharing; a guest-accessible metadata
  service; durable sandbox history; per-sandbox egress (outbound network)
  policy enforced via dedicated iptables chains; snapshot lineage
  (parent-pointer ancestry, queryable in both directions); time-travel
  restore (retired, non-consumed checkpoints, restorable repeatedly).
  All exposed through both SDKs and the CLI (interactive PTY: JS/TS SDK
  and CLI only; idle-lifecycle archiving: daemon-operator config only, no
  client surface; egress policy, snapshot lineage, and time-travel
  restore: daemon HTTP API only so far, no SDK/CLI surface yet — see
  their respective sections),
  live-verified via `scripts/integration-test.sh` (294 checks, 0 failing,
  with `SANDKILN_AUTH_TOKEN` set — see that script's own usage
  comment for what's skipped without one).

## Engineering principles

- **Modular, not monolithic.** Each concern is its own crate/package with a
  narrow public API — `vmm`, `guest-agent`, the daemon, the SDK, the CLI
  stay separable. No crate reaches into another's internals.
- **Benchmark the hot paths.** Boot time, exec round-trip latency, and
  snapshot/resume time are the metrics that actually matter for this
  product. See the Benchmarking section below.
- **Prove it, don't assume it.** Every milestone ends with something
  actually run on real hardware, not just unit tests.
- **No half-finished surfaces.** A feature either works end to end or it
  isn't claimed as done. Deliberately deferred work is called out
  explicitly, not left silently incomplete.

Execution model: development happens in this repo; anything that needs
KVM, a Linux toolchain, or real hardware (Rust builds, Firecracker,
rootfs/kernel builds, actually booting a microVM) runs on the remote dev
box over SSH.

## Networking — done

Every sandbox leases a tap device from a pre-created pool
(`scripts/host-setup/create-tap-pool.sh`) and attaches it to a shared bridge with a
statically assigned IP; the daemon runs unprivileged with `CAP_NET_ADMIN`
raised into its ambient set (`scripts/host-setup/grant-net-admin.sh`), not as root.
Verified: two sandboxes running concurrently, each with a distinct IP,
both resolving DNS and reaching the real internet through the daemon's
HTTP API.

Sandboxes are also isolated from each other on the shared bridge (Linux
bridge port isolation — a tap can reach the gateway/uplink but not another
sandbox's tap), verified: cross-sandbox ping fails, gateway ping and real
outbound HTTP both still work.

## Client SDKs

- **JS/TS (`sandkiln` npm package) — working, matches the daemon's full
  surface.** `Sandbox.create()` (tags, an auth token, and optional
  `vcpuCount`/`memSizeMib` overrides), `Sandbox.list()` (tag-filterable),
  `Sandbox.resume()`/`Sandbox.fork()` (static, boot from a snapshot),
  `runCommand()`, `readFile()`/`writeFile()`, `snapshot()`, `previewUrl()`,
  `stop()`, `execStream()`/`listExecStreams()`/`attachLogs()`,
  `mount()`/`listMounts()`/`unmount()`. ESM + CJS + full type definitions
  via `tsup`. Verified against a live, auth-enabled daemon end to end —
  not just typechecked, which is how `stop()` returning `200` instead of
  the documented `204` got caught and fixed. **Published**:
  [npmjs.com/package/sandkiln](https://www.npmjs.com/package/sandkiln)
  (0.10.0, with signed provenance from the CI build — includes
  everything in this bullet).
- **Done: full filesystem operations** — `chmod`/`chown`/`mkdir`
  (with `-p`-style `parents`)/`rename`/`copy`/`symlink`/`readlink`/
  `truncate`/directory listing with metadata (name, is-dir, is-symlink,
  size, permission bits, mtime), as new vsock protocol commands
  alongside `exec`/`read_file`/`write_file`. Also closes a
  previously-undocumented gap: `list_dir` already existed in the
  protocol and guest agent but was never wired up above that layer (no
  daemon route, no SDK, no CLI) — it's now exposed too, with richer
  metadata than its original bare-filename-list shape (nothing
  depended on that shape yet, so it was widened in place rather than
  adding a separate variant). Exposed as
  `sandbox.chmod/chown/mkdir/rename/copy/symlink/readlink/truncate/listDir`
  in both SDKs and `kiln sandbox chmod|chown|mkdir|rename|cp|symlink|
  readlink|truncate|ls-dir` in the CLI. No path validation on any of
  these (same as `read_file`/`write_file` already had none) — a
  deliberate consistency choice, not an oversight: the guest agent is a
  "dumb executor" by design (see its own `AGENTS.md`), and a path is
  scoped to whatever it resolves to inside that one microVM's own
  filesystem, not the real host. **Live-verified end to end** after
  rebuilding and re-injecting the guest agent into the daemon's actual
  base rootfs (`~/sandkiln-tools/images/ubuntu-22.04.ext4` —
  `SANDKILN_BASE_ROOTFS`'s real default, not the differently-named test
  image first (mistakenly) injected into): 21 new
  `scripts/integration-test.sh` checks, 193/193 passing, covering
  mkdir -p and its conflict case, chmod reflected in a later listing,
  rename/copy both verified by content (copy leaves the original
  intact, rename doesn't), symlink/readlink round-tripping the exact
  target string, a listing correctly marking a symlink vs. a directory,
  truncate verified byte-accurate by re-reading the file, chown, and
  the chmod-on-nonexistent-path/readlink-on-non-symlink error cases.
- **Python (`sandkiln` PyPI package) — working, mirrors the JS SDK
  exactly**, including `resume()`/`fork()`/`snapshot()`/`preview_url()`
  and resource overrides. Zero runtime dependencies (stdlib `urllib`,
  matching the JS SDK's own zero-dependency `fetch` approach). Verified
  live end to end, including `attach()` reconstructing a handle without a
  network call and correct 404 handling on a stopped sandbox. **Done:
  published to PyPI** (`pip install sandkiln`) via
  `.github/workflows/publish-python-sdk.yml`'s OIDC trusted-publishing
  flow — see `packages/python/AGENTS.md`'s Publishing section for how to
  ship the next version.
- Both talk to the daemon's HTTP API — no logic duplicated between them
  beyond what each language's idioms require.
- **Not yet built: an async Python client.** `packages/python`'s
  `Sandbox` is entirely synchronous (`urllib`-backed, blocking calls) —
  there's no `asyncio`-based counterpart today, so a caller already
  running an async event loop (an async web framework, an async agent
  runner) has to fall back to a thread pool to use this SDK without
  blocking it. A real gap for that use case specifically, not a
  correctness issue with the sync client itself.
- **Done: per-exec/per-create environment variables.** `POST /sandboxes`
  accepts an `env: {key: value}` map baked in for that sandbox's whole
  lifetime; `exec`/`exec-stream` each accept their own `env`, merged on
  top of the sandbox's create-time env with the per-call value winning on
  a key conflict — resolved daemon-side (`routes_exec::resolve_env`)
  before the request ever reaches the guest, so the guest agent stays a
  "dumb executor" with a single already-resolved `env` field on
  `Request::Exec`/`ExecStreamHandshake` rather than any notion of
  "sandbox-level" vs. "call-level." Persists through
  snapshot/resume/fork exactly like `tags` (unlike `egress`, there's no
  external resource to re-apply, so a fork gets the identical value a
  resume would, no ownership asymmetry). `Sandbox.create({env})`/
  `.runCommand(cmd, args, {env})`/`.execStream(cmd, args, {env})` in the
  JS/TS SDK, matching `env=` kwargs in the Python SDK's `create()`/
  `get_or_create()`/`run_command()` (Python has no `execStream()` at
  all, see the SDK bullet above), `--env key=value` (repeatable) on
  `kiln sandbox create|get-or-create|exec|exec-stream` in the CLI.
  Live-verified end to end on the real dev box across all three
  languages (Rust daemon via curl, the JS/TS SDK, the Python SDK) and
  the CLI: create-time env reaching an unmodified exec, a per-call
  override winning on a shared key while an unrelated create-time key
  survives untouched, a per-call key that didn't exist at create time
  still reaching the process, and the merged behavior working
  identically for `exec` and `exec-stream`. Also verified surviving
  snapshot → resume and snapshot → fork unchanged. New
  `scripts/integration-tests/24-env-vars.sh`, full suite passing
  (`cargo test --workspace` and `cargo clippy --workspace --all-targets`
  clean throughout).
- File operations, SSH/PTY-style interactive access, and image
  build/import from an existing container image were also checked
  against this same "is it actually built" standard: file
  operations are already covered in full (see the filesystem-ops bullet
  above); an interactive session already exists as `kiln sandbox pty`
  (see the CLI section below) over a WebSocket, so that's shipped, not a
  gap; converting an existing OCI/Docker image into a bootable rootfs is
  explicitly out of scope today (see `routes_images.rs`'s own module
  doc comment) — a custom image today has to already be a bootable
  ext4 rootfs, built via `images/build-universal-image.sh` or handed in
  directly, not derived from a container image on the fly.

## CLI (`kiln`) — working

- **Done:** `kiln sandbox create|ls|rm|exec|read|write|preview|snapshot|
  resume|fork` — a thin `commander`-based wrapper over the SDK, verified
  live end to end. `cp` (a single unified copy command) was simplified to
  explicit `read`/`write` subcommands instead — less magic than parsing a
  `sandbox:path` prefix syntax for a first version.
- **Done: `kiln sandbox exec-stream`/`kiln sandbox logs`** — see the
  "Dev servers and live preview" section's streamed background exec
  entry.
- Built for manual testing, agentic workflows, and debugging — mirrors the
  SDK surface, usable standalone without writing code.
- **Published**: [npmjs.com/package/sandkiln-cli](https://www.npmjs.com/package/sandkiln-cli)
  (0.2.0, signed provenance) — not as `kiln`, which is a pre-existing,
  unrelated package (`node-kiln`, owned by someone else since before this
  project existed). `npm install -g sandkiln-cli` still gives you the
  `kiln` command (npm's `bin` field maps them independently). Verified
  live: installed fresh from the registry, ran `kiln --help` successfully.

## Authentication and multi-tenancy

- **Done:** a single bearer token (`SANDKILN_AUTH_TOKEN`) gates every
  `/sandboxes*` route via daemon middleware; `/healthz` stays open. Off by
  default for local dev, with a startup warning so that's never silent.
  This is a self-hosted project, not tied to any platform's identity
  system, so a plain shared-secret token stands in for what a hosted
  platform would do with OIDC.
- Per-token scoping (which sandboxes a token can see/act on) once more than
  one caller shares a daemon instance.

## Base and custom images

- A **universal base image**: Ubuntu with current Node.js LTS, Python,
  common CLI tooling, and full root access inside the sandbox — the
  default every sandbox boots from unless told otherwise.
- A small catalog of **managed images** for common language runtimes.
- **Done (partial): custom/managed images.** `POST /images` registers an
  already-built ext4 rootfs from a host path into a daemon-managed
  directory (`SANDKILN_IMAGES_DIR`) under a caller-given id; `GET /images`
  lists them (`in_use_by`, always-`false` `guest_agent_verified` plus a
  `verification_hint`, since the unprivileged daemon can never loop-mount
  a candidate image to confirm the agent is baked in — use
  `scripts/preflight-check.sh --root-checks --rootfs-image <path>` out of
  band first); `DELETE /images/:id` refuses (409) while any live sandbox,
  in-flight boot, or held snapshot still references it. `POST /sandboxes`
  takes an optional `image_id` to boot from a registered image instead of
  `SANDKILN_BASE_ROOTFS`; carried through `snapshot`/`resume`/`fork` like
  `name`/`tags`. Exposed as `Image.register/list/delete` in both SDKs
  (plus `imageId`/`image_id` on `Sandbox.create`) and `kiln image
  ls|create|rm`, `kiln sandbox create --image`. **Not done:** OCI-image
  conversion — still accepts only an already-built ext4 file, not an OCI
  image reference — and there's no way yet to boot from an image by name
  through `get-or-create`.
- Image build tooling lives in `images/` — reproducible, scripted builds,
  not hand-built blobs.

## Persistence and snapshotting

- **Sandbox vs. session**: a sandbox is a persistent identity (name,
  config, filesystem state); a session is one running microVM instance of
  it. A sandbox resumed daily for a week is one sandbox, seven sessions —
  our current `Sandbox` type conflates the two (it dies with its VM) and
  needs to split before persistence can work at all.
- **Done: snapshot/resume.** Save a running microVM's full state (memory +
  disk) and resume it later, skipping boot and dependency installation
  entirely — `POST /sandboxes/:id/snapshot`, `POST /snapshots/:id/resume`,
  exposed as `Sandbox.snapshot()`/`Sandbox.resume()` in both SDKs and
  `kiln sandbox snapshot|resume`. Live-verified repeatedly this session,
  including with drives attached.
- **Done: snapshots durable across a daemon restart.** Snapshot metadata
  is written atomically to disk alongside its state/memory files and
  reconciled back into the daemon at startup — a snapshot taken before a
  daemon crash or restart is still listable and resumable afterward, with
  its held network tap device correctly reserved out of the pool before
  the daemon starts accepting new sandbox creates (preventing a
  double-lease race). Verified live: killed a daemon with a snapshot on
  disk, started a fresh instance, resumed it, data intact. Not durable
  across a host *reboot* by default — snapshot storage lives under
  `$TMPDIR`, see `SELF_HOSTING.md`'s persistent-state section.
- **Done: persistent-by-default sandboxes.** `DELETE /sandboxes/:id`
  auto-snapshots on stop by default (`stop_sandbox_by_id(..., keep: true)`
  via the shared `snapshot_and_stop`/`resume_snapshot_by_id` path in
  `routes_snapshot.rs`) instead of destroying — "stop and come back later"
  is now the default, not something the caller has to manage. An explicit
  `?keep=false` (CLI: `kiln sandbox rm --destroy`) opts back into a full
  destroy; a sandbox that structurally can't be preserved (jailed) falls
  back to destroy automatically rather than leaking. The idle reaper's
  destroy pass uses the same default.
- **Done: named sandboxes.** Create/resume by a caller-given name (unique
  among live sandboxes, `1-64` chars, `[A-Za-z0-9_-]`) instead of only an
  opaque id — `name` on `POST /sandboxes`, `GET /sandboxes/by-name/:name`
  (live only — 409 pointing at get-or-create if the name currently
  resolves to a held snapshot instead), and `POST /sandboxes/get-or-create`
  (return-if-live / resume-if-snapshotted / create-if-neither, race-safe
  under a per-name lock so two concurrent callers claiming the same new
  name can't both win). A name carries through `snapshot`/`resume`/`fork`
  when re-specified. Exposed as `Sandbox.getOrCreate()`/`Sandbox.byName()`
  in both SDKs (plus `name` on `create`/`resume`/`fork`) and
  `kiln sandbox get-or-create|get`, `--name` on `create`/`ls`/`resume`/
  `fork`.
- **Done: auto-suspend on idle.** `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`
  pauses and snapshots (not destroys) a sandbox that's gone quiet for a
  configurable window — the same pause+snapshot path the manual snapshot
  route uses, composed into the idle reaper rather than built as a new
  mechanism. Frees the VM/network resources it held while keeping state
  resumable; if it's also configured, `SANDKILN_IDLE_TIMEOUT_SECS` (plain
  destroy) must be strictly longer and acts as a backstop for a sandbox
  whose auto-suspend keeps failing, not a competing timer. Discoverability
  (a sandbox vanishing into a snapshot on its own): `GET /snapshots
  ?source_sandbox_id=<id>` finds what a given sandbox became —
  `Sandbox.listSnapshots()`/`list_snapshots()` in both SDKs,
  `kiln sandbox snapshots --source <id>`.
- **Done (partial): non-consuming snapshot fork.** `POST /snapshots/:id/fork`
  boots a new sandbox from a snapshot without consuming it, so the same
  prepared state can be resumed from repeatedly — `Sandbox.fork()` in both
  SDKs and `kiln sandbox fork`. **Not** true simultaneous parallel forking:
  Firecracker has no verified mechanism to give two live descendants of one
  snapshot independent rootfs backing files or independent guest IP/MAC
  (both are frozen into the snapshotted state), so at most one live fork of
  a given snapshot may exist at a time (`Snapshot::forked_into`, enforced
  for resume/fork/delete/snapshot alike — a second fork attempt while one
  is live is a `409`). Real parallel branches off one snapshot still needs
  either a verified per-fork drive-path override or a from-scratch
  live-memory-clone approach — genuinely open, see
  `core/crates/daemon/src/routes_snapshot.rs`'s module doc comment.
- **Done: time-travel restore.** `POST /snapshots/:id/resume` no longer
  deletes the checkpoint it consumes by default — it *retires* into
  `GET /snapshots/history`, restorable again later via `POST
  /snapshots/history/:id/restore`, as many times as wanted (`DELETE
  /snapshots/history/:id` to reclaim it outright). `?retain_history=false`
  opts back into the original, zero-retention behavior. Sequential, not
  branching: restoring an old checkpoint doesn't destroy or invalidate
  whatever came after it in that lineage (they stay in history too, like
  an old git commit's descendants surviving a checkout of an ancestor),
  but restoring refuses (`409`, naming the holder) while anything else
  sharing that checkpoint's frozen network identity is currently live or
  held — the same one-live-descendant-at-a-time rule `Snapshot::forked_into`
  already enforces for fork, generalized across a lineage's full history
  instead of just its single most recent snapshot. A restored sandbox
  owns everything outright (a fresh network lease, a private rootfs
  clone) and so, unlike a fork, stays snapshottable afterward, starting a
  new branch of history from that point.
  - **A real, pre-existing corruption bug found and fixed while building
    this**: `POST /snapshots/:id/fork` shared its source snapshot's
    rootfs *file* directly, not a private copy. Fine for two
    *simultaneous* forks (already ruled out by `forked_into`), not for
    two *sequential* ones: fork, mutate the shared file, stop the fork,
    then resume (not fork) the original snapshot directly — `Vm::resume`
    loads memory state describing the rootfs as it was *before* the fork
    ever ran, against a file that now has the fork's mutations layered on
    top. Reproduced live (write a marker, fork, overwrite the marker in
    the fork, stop the fork, resume the original directly — the marker
    came back mutated) and fixed the same way retirement's own safety
    property works: `routes_sandbox::clone_rootfs` gives every fork its
    own private copy now, closing the exact class of bug this whole
    feature exists to prevent.
  - **A second issue found live, this one operational rather than a
    correctness bug**: retaining every resume's checkpoint by default has
    a real, unbounded disk cost — a full guest-memory dump plus a private
    rootfs copy, forever, until deleted. Routine manual verification
    while building this alone filled a 468GB dev-box disk. `DELETE
    /snapshots/history/:id` (added specifically in response to this, not
    part of the original design) is the mitigation shipped so far;
    automatic expiry/retention policy is a deliberately deferred
    follow-up — see `SELF_HOSTING.md`'s "Time-travel restore and its disk
    cost" for operator-facing guidance in the meantime.
  - **Deliberately out of scope**: true parallel branching (two
    checkpoints from one lineage live at once) — blocked by the same
    frozen-network-identity constraint that already rules out true
    concurrent forking (see `routes_snapshot.rs`'s own module doc
    comment: the guest's IP/MAC can't be changed post-hoc without
    in-guest cooperation this project's guest agent doesn't have), not a
    scope choice. No SDK/CLI surface yet — daemon HTTP API only, same
    precedent as egress policy and snapshot lineage.
- **Done: tiered idle lifecycle, the archive tier — a first honestly-scoped
  slice.** Extends today's binary auto-suspend (running → snapshot) with
  a second, independent tier: `SANDKILN_ARCHIVE_TIMEOUT_SECS` moves a
  held snapshot's `state.snap`/`mem.bin` from `snapshots_root()` onto a
  separately configured `SANDKILN_ARCHIVE_DIR` once it's sat unresumed
  that long — `idle_reaper`'s new archive pass, applying to *any* held
  snapshot regardless of how it arose (auto-suspend or a manual
  `POST /sandboxes/:id/snapshot`), independent of whether auto-suspend or
  `idle_timeout` are even configured. `GET /snapshots` reports
  `archived_at_unix`. No SDK/CLI surface — daemon-operator config only,
  same as `idle_timeout`/`auto_suspend_timeout`. Live-verified: a real
  snapshot archived after a short configured timeout, its files
  confirmed moved, a daemon restart correctly reconciling it back with
  `archived: true` and no false-alarm warnings, and — critically — a
  full resume afterward proving the archived snapshot is still exactly
  as usable as a hot one (file content round-tripped through
  write-file → snapshot → archive → resume → read-file correctly).
  - **A real Firecracker constraint found the hard way, that reshaped
    this feature mid-build**: the original design moved all three of a
    snapshot's files (state, memory, *and* rootfs) into the archive
    directory. Live-testing a resume afterward failed outright —
    `"Error manipulating the backing file: No such file or directory ...
    /tmp/sandkiln-rootfs-<id>.ext4"` — because Firecracker bakes the
    rootfs backing file's absolute host path into `state.snap` itself at
    snapshot time, and `/snapshot/load` has no override for it (unlike
    `mem_backend.backend_path`, a genuine load-time parameter). The
    rootfs file has to stay exactly where it was created for as long as
    the snapshot might ever be resumed or forked. Fixed by having
    archiving only ever move `state.snap`/`mem.bin` and leave
    `rootfs_path` completely untouched — a real, if partial, win rather
    than the complete one originally hoped for (`mem.bin` alone is
    exactly the guest's configured RAM size, often comparable to or
    larger than the rootfs copy, so this still meaningfully reduces what
    an idle snapshot leaves on hot storage — just not all of it).
  - **A second real bug found via the same live-testing pass**: two
    existing snapshot-lifecycle code paths (`resume_snapshot_by_id`'s
    post-resume cleanup, `delete_snapshot_by_id`'s teardown) derived the
    directory to remove from a hardcoded `snapshot_dir(&id)` — always the
    *hot* root, regardless of where a snapshot's files actually were.
    For an archived snapshot this would have silently no-op'd (removing
    an already-vacated or nonexistent hot directory) while leaking the
    real, large archived files forever. Fixed by deriving the directory
    from the snapshot's own current `snapshot_path` instead — accurate
    whether hot or archived, and this was a latent bug even before
    archiving existed as a concept, just never exercised until now.
  - **Deliberately not built in this slice**: the delete-after-archive
    tier the original design sketched (archive past one timeout, delete
    past a longer one) — this ships archive only, explicitly deferred,
    matching the same "narrower first slice, come back for the rest"
    precedent `max_count` set for pre-warmed pools above. Also not the
    "remote storage" archive tier originally imagined (an S3-compatible
    store — remote storage mounts have since shipped, so the dependency
    is gone, but nothing wires archiving onto them yet)
    — `SANDKILN_ARCHIVE_DIR` is still a local filesystem path,
    just a separately configured one (the real, concrete win available
    today: pointing it at a real disk instead of `snapshots_root()`'s
    default location under `$TMPDIR`, often tmpfs).

## Drives and remote storage

- **Drives**: attachable persistent filesystem storage that outlives a
  single sandbox and can be reattached to a new one — for state that
  should survive well past any one VM's lifetime.
- **Done: read-only shared drives.** A drive attached read-only may be
  attached to arbitrarily many sandboxes at once — for data or a common
  base layer that doesn't need per-sandbox copies — while a read-write
  attachment (existing or requested) still needs exclusive, single-holder
  access, exactly like before this existed. `AppState::drive_holders()`
  tracks every current holder plus whether each holds it read-only;
  `can_attach_read_only()` is the pure rule deciding whether a new attach
  may coexist with what's already there. Covers snapshots holding a drive
  too, not just live sandboxes.
- **Done: drives in both SDKs and the CLI.** `Drive.create/list/delete`
  in the JS/TS and Python SDKs, `drives`/`DriveAttachment` on
  `Sandbox.create()`/`create()` in both, and `kiln drive create|ls|rm`
  plus `--drive <id[:ro]>` on `sandbox create`.
- **Done: remote storage mounts.** `POST/GET/DELETE
  /sandboxes/:id/mounts` mounts an S3-compatible bucket into a sandbox via
  `rclone mount`, running entirely inside the guest — no new wire
  protocol, just the existing `Mkdir`/`WriteFile`/`Chmod`/`Exec` guest
  requests chained together (see `routes_mounts.rs`'s module doc comment).
  Needs two things a stock setup doesn't have out of the box: a guest
  kernel built with `CONFIG_FUSE_FS` (Firecracker's own default/CI kernel
  configs don't enable it, and guest kernels can't load modules —
  `images/build-guest-kernel.sh`) and `rclone`/`fusermount3` baked into
  the rootfs the same way the guest agent is (`images/inject-rclone.sh`,
  `scripts/dev.sh inject-rclone`) — see `SELF_HOSTING.md`'s "Remote
  storage mounts (optional)" section. Credentials go into the guest as a
  `0600` rclone config file via `WriteFile`+`Chmod`, never as a command-line
  argument. No re-application on resume/fork/restore — a mount is a live
  guest-side FUSE process, captured by Firecracker's own snapshot
  mechanism along with the rest of guest memory.
- **Done: exposed in both SDKs and the CLI.** Was daemon-HTTP-API-only
  for a while (see this project's own `examples/remote-storage-mount`,
  which called the three routes with raw `fetch()` until this landed).
  `Sandbox.mount()`/`.listMounts()`/`.unmount()` (JS/TS),
  `mount()`/`list_mounts()`/`unmount()` (Python), `kiln sandbox
  mount|mounts|unmount` (CLI). Live-verified against a real daemon
  (`listMounts()` on a mount-free sandbox, and a mount attempt against an
  unreachable endpoint failing cleanly as a `SandkilnApiError` rather
  than hanging or crashing) — full success needs a real S3-compatible
  endpoint this dev box doesn't have configured, same caveat
  `scripts/integration-tests/22-mounts.sh` already carries.

## Firewall and egress policy

- **Done: per-sandbox IP/CIDR-based egress policy.** A request-level
  `egress: { mode, allow_cidrs, deny_cidrs }` on `POST /sandboxes` (and
  `POST /sandboxes/get-or-create`) — `mode` is `allow_all` (today's
  default-open behavior, minus whatever `deny_cidrs` subtracts from it)
  or `deny_all` (nothing outbound except what `allow_cidrs` opens back
  up). Enforced with one dedicated iptables chain per sandbox
  (`sandkiln_vmm::egress`), named from its tap device, with a single
  jump rule inserted ahead of the daemon's existing bridge-wide `FORWARD`
  `ACCEPT` rule so only that sandbox's own traffic is affected. Within a
  sandbox's chain, every `deny_cidrs` rule is appended before every
  `allow_cidrs` rule, before the base mode's own default — iptables'
  first-match-wins evaluation order means deny always beats allow on
  overlap, with no special-casing needed. Rules match only traffic
  actually leaving via the daemon's uplink interface (mirroring the
  existing bridge-wide rule's own scoping), which has a useful
  side-effect verified live: gateway-bound traffic (DNS to the bridge's
  own IP) never transits the uplink at all, so it's structurally exempt
  from any egress policy without an explicit allowlist entry — even a
  `deny_all` sandbox with an empty `allow_cidrs` can still resolve names,
  it just can't reach anything past the gateway that isn't explicitly
  allowed. A policy is tied to the sandbox's *lease*, not its VM: applied
  once when a lease becomes live (fresh boot, pool claim, resume, fork)
  and removed only when the lease is finally released (full destroy, or
  deleting a held snapshot) — a plain snapshot-and-stop leaves the chain
  dormant but intact, exactly like the tap device it's attached to.
  Persists correctly through snapshot → resume and → fork (re-applied
  each time from the snapshot's own retained policy, since the
  underlying iptables state doesn't survive a host reboot the way the
  snapshot file does); a re-apply failure on resume/fork is a loud
  warning, not fatal, since both are one-way/hard-to-redo operations and
  this remains a best-effort hardening layer, not a hard guarantee (the
  same framing this project already uses for jailer). All of this was
  verified live against the real dev box: `deny_all` blocks an
  unlisted LAN destination, an `allow_cidrs` entry opens it back up
  under `deny_all`, a `deny_cidrs` entry blocks one destination under
  `allow_all` while leaving others reachable, gateway-bound traffic
  stays reachable regardless of policy, and the policy survives
  snapshot/resume/fork and is fully torn down on destroy — see
  `scripts/integration-tests/19-egress.sh` for the host-agnostic subset
  of this (request validation, and that a policy doesn't break normal
  use or fail to survive snapshot/resume/fork) that runs in CI; the
  actual allow/deny/deny-wins-on-overlap behavior needs a second real,
  reachable LAN address to test against, which isn't guaranteed on every
  machine this suite runs on, so that part is manually verified only
  (same treatment as the daemon-restart case in the sandbox-history
  work above).
- **Deferred, deliberately**: domain-level rules (would need the shared
  DNS proxy — currently one `dnsmasq` instance with no per-source
  differentiation — to become source-IP-aware, a substantially bigger
  change than this first slice) and port-level matching (`-p tcp
  --dport`, a straightforward extension of the same rule shape, just not
  built yet). IPv4 only, matching every other networking type in this
  codebase.
- **Not yet exposed in the SDKs/CLI** — daemon HTTP API only so far. A
  deliberate scope cut for this first slice, same as pool `max_count`'s
  own SDK/CLI follow-up; worth doing in a pass of its own.
- **Fixed: `egress::apply`'s hot path.** Flagged by the same audit that
  found the `Vm::call` retry-loop bug (see the Benchmarking section) as
  a real, unmeasured cost: applying a policy spawned one `iptables`
  subprocess per rule (chain create/flush, each `deny_cidrs` entry, each
  `allow_cidrs` entry, the default verdict), each paying a full
  fork+exec regardless of how little work it does. A 6-CIDR policy (9
  spawns under the old shape) measured **~8.3ms average per create** —
  real, and easy to cut since none of that work needs separate
  processes. Now loaded as one `iptables-restore --noflush` call instead
  (`sandkiln-vmm::egress::apply`); the two rules that actually touch the
  shared `FORWARD` chain stay as individual `iptables` calls, already
  minimal (a `-C` existence check, and an `-I` only when it's missing).
  A new `egress_apply` `CreatePhase` metric (only recorded when a create
  actually requests a policy, so its count is expected to be far lower
  than the other phases') makes this cost visible going forward instead
  of hiding inside `setup`. Re-measured the same way afterward (same
  6-CIDR policy, `create_phase_duration_ms{phase="egress_apply"}`'s own
  delta over 17 fresh creates): **~3.1ms average, down from ~8.3ms — a
  real ~63% cut**, consistent with going from ~9-10 `iptables` spawns to
  ~3 (one `iptables-restore` plus the two `FORWARD` calls left as-is).
  Verified live: `cargo test --workspace` and `cargo clippy --workspace
  --all-targets` clean, full integration suite 300/300 including
  `19-egress.sh`'s allow/deny/deny-wins-on-overlap and
  survive-snapshot/resume/fork checks.
- Request-level matchers (path, method, query, header) with a rule that
  either forwards or transforms the request are a further-out stretch
  beyond all of the above, useful for a proxy sitting in front of a
  sandbox's own exposed port rather than the sandbox's own outbound
  egress.
- Consider a per-sandbox CA + TLS-terminating proxy for HTTPS
  inspection/transformation, mounted into the guest's trust store at
  boot — meaningfully more complex than IP/domain-level filtering, so
  it's a deliberate stretch goal, not a given.

## Security hardening

- **Isolation model, stated explicitly**: every sandbox is a real
  Firecracker microVM with its own kernel, not a shared-kernel
  container/namespace sandbox with syscall interception. This is the
  strongest isolation story available for running untrusted code and is
  worth saying outright (in docs and on the website) rather than leaving
  it implicit — it's the actual reason this project exists in this shape
  rather than as a thinner container wrapper.
- **Done (opt-in): Firecracker's jailer** — chroot, cgroup v2 resource
  limits, a dedicated unprivileged uid/gid per VM
  (`SANDKILN_JAILER_ENABLED`, see `SELF_HOSTING.md`'s "Optional:
  jailer-based sandbox boot"). Off by default; the daemon still boots
  every sandbox via a direct Firecracker spawn unless explicitly turned
  on. Builds and passes unit tests. **First real-hardware attempt this
  session**: every `POST /sandboxes` failed (`500`,
  `"<chroot>/root/api.sock" never appeared`) with
  `SANDKILN_JAILER_ENABLED=1` on the dev box — root-caused via the guest
  console log to jailer itself failing at
  `Failed to change owner for .../firecracker: Operation not permitted`.
  Not a code bug: `SELF_HOSTING.md` already documents that the `jailer`
  *binary* needs `setuid-root` (`sudo chown root:root` +
  `sudo chmod u+s` on it — a separate one-time step, not part of
  `setup.sh`, since it needs an interactive root password) — that step
  had simply never been applied on this box before this test, and
  applying it needs a real interactive terminal this session didn't
  have. **Still not verified on real hardware** — same caveat as
  before, now with a concrete repro and root cause instead of just
  "hasn't been tried yet." Verify before relying on it for a genuinely
  adversarial workload. Snapshotting a jailed sandbox isn't supported
  (`400`) — jailer support covers `Vm::boot` only, `Vm::resume` always
  spawns directly.
- **Done: enforced per-sandbox resource ceilings.** A `POST /sandboxes`
  request may now override `vcpu_count`/`mem_size_mib` per sandbox
  (defaulting to the daemon's configured values when omitted), checked
  against `SANDKILN_MAX_VCPU_COUNT`/`SANDKILN_MAX_MEM_SIZE_MIB` — `0` or
  above-ceiling is rejected with `400`, not silently clamped. Live-verified.
  seccomp filters and disk-size ceilings are still open.
- **Done:** automatic idle timeout (`SANDKILN_IDLE_TIMEOUT_SECS`) — a
  sandbox with no exec/read/write activity past the configured window is
  stopped automatically.
- **Done:** network isolation between sandboxes on the shared bridge (see
  the Networking section) — bridge port isolation, no sandbox-to-sandbox
  traffic by default.
- **Done: per-sandbox I/O rate limiting.** `POST /sandboxes` (and
  `get-or-create`) takes an optional `rate_limit: {bandwidth_bytes_per_sec?,
  ops_per_sec?}`, validated by the same "reject, don't silently no-op"
  rule as `vcpu_count`/`mem_size_mib` (at least one sub-field required if
  present at all, `0` rejected outright with `400`) and applied uniformly
  to the rootfs drive, every attached drive, and both directions of the
  network interface via Firecracker's own token-bucket rate limiter
  (`rate_limiter` on each drive, `rx_rate_limiter`/`tx_rate_limiter` on
  the NIC — confirmed against the real `firecracker_spec` swagger schema,
  not assumed). Each bucket refills to its full size once per second, a
  deliberately simple sandbox-level knob rather than exposing
  Firecracker's own independent-burst/independent-direction granularity —
  that's still available at the vmm-crate level (`VmConfig::rate_limit`,
  `sandkiln_vmm::vm::{RateLimiter, TokenBucket}`) if a future need for
  finer control shows up. `None` (the default) means unlimited host I/O,
  unchanged from before this existed. Exposed in both SDKs (`rateLimit`/
  `rate_limit_bandwidth_bytes_per_sec`+`rate_limit_ops_per_sec` on
  `create`/`getOrCreate`) and the CLI (`--rate-bandwidth`/`--rate-ops` on
  `kiln sandbox create`/`get-or-create`).

## Multi-agent isolation

- **Done: separate Linux users with private home directories, baked
  into the base image.** `images/setup-multi-agent-users.sh` creates
  per-agent users (private `$HOME`) plus a shared group for deliberate,
  controlled file sharing between agents in the same sandbox — run as
  part of `images/build-universal-image.sh`, not something a caller
  configures at create time.
- Runtime-configurable agent users (chosen per sandbox at `POST
  /sandboxes` time, rather than fixed at image-build time) is still
  open.

## System-privileged workloads

- Support workloads that need real system-level privileges inside the
  guest: container runtimes (Docker-in-VM via nested virtualization or a
  compatible runtime), VPN clients, FUSE filesystem drivers.
- This needs care in the base image (kernel config, cgroups setup inside
  the guest) more than in the host-side daemon.
- **GPU passthrough/access inside a sandbox is explicitly not planned.**
  Called out here deliberately rather than left silent — Firecracker
  itself has no GPU device model, and adding one would be a different
  project (a real hardware-passthrough VMM), not an extension of this
  one. If a workload needs a GPU, it doesn't belong in a sandkiln
  sandbox today.

## Dev servers and live preview

- **Done: host-side reverse proxy.** `GET/POST/... /sandboxes/:id/preview/:port[/path]`
  proxies a full HTTP request to a server listening on that port inside
  the sandbox, over the bridge network — `Sandbox.previewUrl()`/
  `preview_url()` in both SDKs, `kiln sandbox preview`, an
  `examples/dev-server-preview` reference. Preview routes accept the auth
  token as a `?token=` query parameter (not just a header), since the
  caller is typically a browser tab or `<iframe>` that can't set one.
  Live-verified, including a real `python3 -m http.server` proxied
  end to end, the 502/404/401 error paths, and previewing a sandbox
  forked from a snapshot (which borrows its network lease rather than
  owning one). WebSocket proxying (dev-server HMR/live-reload) is a real,
  explicitly-scoped-out follow-up, not silently broken — plain HTTP only
  for now.
- **Not yet built: the reverse direction (a local tunnel).** Everything
  above exposes a port *inside* the sandbox outward; there's nothing
  today that reaches the other way — a sandboxed process reaching a
  service running on the caller's own machine (a local database, a local
  API a coding agent needs to call during a test run) without that
  service already being reachable from the sandbox's own network
  namespace. Checked and confirmed absent, not just undocumented.
- **Alternative worth considering alongside the proxy**: today's
  `/preview/:port` is a daemon-proxied *path*, not a real routable
  domain. A dedicated public domain/subdomain per exposed port (the
  guest reachable at its own URL rather than through a `/preview/:port`
  path prefix) is a different, also-valid shape used elsewhere — would
  need real DNS/routing infrastructure this project doesn't have today,
  so it's a bigger lift than the existing proxy, not a drop-in
  replacement for it.
- Fast iterative file sync tuned for dev-server workflows — write many
  small files quickly, ideally with watch-mode support.
- **Done: interactive terminal access.** A real PTY inside the sandbox,
  exposed over a WebSocket — distinct from batch `exec` (request in,
  response out), this is a live, bidirectional shell session:
  `GET /sandboxes/:id/pty[?cols=&rows=]` upgrades to a WebSocket and
  proxies raw bytes to a shell forked (via `forkpty(2)`) inside the
  guest, over a second, dedicated vsock port (`PTY_PORT`, separate from
  the request/response `AGENT_PORT`) — `Sandbox.pty()` in the JS/TS SDK
  (native `WebSocket`, no new runtime dependency), `kiln sandbox pty
  <id>` in the CLI (raw terminal mode, keystrokes including Ctrl+C pass
  straight through), an `examples/interactive-terminal` reference.
  Live-verified end to end, including both hangup directions: the shell
  exiting first cleanly ends the WebSocket session, and the WebSocket
  closing first (a lost connection, a closed browser tab) sends the
  shell's process group `SIGHUP` so it actually terminates instead of
  running orphaned inside the guest. Also covered by
  `scripts/integration-test.sh` itself now (`17-pty.sh`, via a small
  Node WebSocket helper, `scripts/lib/pty-check.mjs`, since the rest of
  that harness is bash+curl with no native way to drive a WebSocket) —
  a real command's output round-tripping through a real shell, and the
  SIGHUP cleanup itself (open a session, disconnect without exiting the
  shell, `exec` a `ps` check confirming nothing orphaned survives).
  - Auth: like `/preview/:port`, this accepts the token as a `?token=`
    query parameter, not just a header — neither a browser's nor
    Node.js's native `WebSocket` constructor can set custom headers.
  - A per-sandbox concurrent-session cap (`MAX_PTY_SESSIONS_PER_SANDBOX
    = 64`) is enforced daemon-side.
  - No live terminal resize yet — `cols`/`rows` size the session once,
    at open time, via the initial `PtyHandshake`; a real follow-up would
    need a small control-message side-channel over the same WebSocket
    (e.g. a JSON resize message multiplexed alongside the raw byte
    stream) to change it mid-session.
- **Done: streamed background exec sessions (`kiln sandbox exec-stream`/
  `kiln sandbox logs`).** Distinct from both batch `exec` (blocks for one
  request, returns one stdout/stderr blob) and `pty` (a live interactive
  shell): this runs a command detached inside the guest and lets any
  number of callers attach to its output over time, each getting a
  **replay of everything captured so far, then a live tail** — the exact
  model this section used to call out as the shape worth copying once
  built. `POST /sandboxes/:id/exec-stream` starts it and returns a
  session id immediately; `GET /sandboxes/:id/exec-stream` lists sessions
  (running or finished); `GET /sandboxes/:id/exec-stream/:id/logs`
  (WebSocket) attaches. A third, dedicated vsock port
  (`EXEC_STREAM_PORT`, alongside `AGENT_PORT`/`PTY_PORT`) carries framed
  `ExecStreamEvent`s (stdout/stderr chunks, then one `Exit`) from a
  spawned child process with no controlling terminal — the daemon does
  the actual buffering (a bounded 1MiB ring buffer plus a
  `tokio::sync::broadcast` fan-out, in `crate::log_session::LogSession`),
  not the guest, since a session has to keep capturing output whether or
  not anyone is currently watching. `Sandbox.execStream()`/
  `.listExecStreams()`/`.attachLogs()` in the JS/TS SDK, `kiln sandbox
  exec-stream <id> -- <command> [args...]` (start + follow in one) and
  `kiln sandbox logs <id> [session-id]` (list, or attach/follow an
  existing one — including one already finished) in the CLI, both
  resolving to the remote command's own exit code.
  Live-verified end to end (start a multi-line, multi-second command;
  attach mid-stream and see replay-then-live-tail; reattach after it
  finishes and see a full instant replay; two concurrent attaches to the
  same session, one joining late, both get the complete ordered log) plus
  a new `scripts/integration-tests/23-exec-stream-logs.sh` (a small Node
  WebSocket helper, same pattern as `17-pty.sh`'s).
  - Not carried across resume/fork/restore, and doesn't survive a daemon
    restart — same in-memory-only scope as `Sandbox::pty_session_count`
    already has, not a new limitation. A session is daemon-process state
    tied to one specific `Sandbox` value, not the guest's own memory.
  - No kill/cancel endpoint yet (`DELETE .../exec-stream/:id` isn't
    implemented) — a deliberate v1 scope cut, not an oversight: the
    guest agent would need a way to signal a specific spawned child by
    id, which today's "one connection, one process, no server-side
    registry" design (see `sandkiln-guest-agent`'s `exec_stream.rs`)
    doesn't have a hook for yet.
  - No concurrent-session cap, unlike PTY's `MAX_PTY_SESSIONS_PER_SANDBOX`
    — each session gets its own independent vsock connection and child
    process, with no shared resource between sessions to contend over.

## Tags and sandbox metadata

- Key/value tags on sandboxes (environment, team, owner, whatever the
  caller wants) for filtering and listing.
- **Done: durable sandbox history (sqlite), scoped honestly** — new
  `sandkiln-store` crate (`rusqlite`, bundled), a `GET /sandboxes/history`
  route, and `AppState::history: HistoryStore`. **This is not, and
  cannot be, "the in-memory map becomes sqlite" in the sense of live
  sandboxes surviving a restart** — a live `Sandbox` owns a real
  Firecracker `Child` process with open API/vsock sockets, and there is
  no re-adoption mechanism anywhere in this project (jailer included) to
  reattach to one after the daemon that spawned it is gone. Confirmed
  directly: today a daemon restart of any kind (kill, crash,
  `systemctl restart`) orphans every live sandbox's process
  unconditionally — `scripts/sandkilnd-ctl.sh` already does its own
  best-effort orphan `pkill` for exactly this reason, and
  `SELF_HOSTING.md`'s troubleshooting section already documented the
  symptom before this existed. What sqlite actually buys, honestly: a
  durable record of *every sandbox that ever existed* — id, name, tags,
  image, created-at, and how it ended (`destroyed`, `snapshotted` with
  the resulting snapshot id, or `orphaned_by_restart`) — queryable
  (`?live_only=`, `?limit=`) across restarts, which the existing
  in-memory map and even `Snapshot`'s own flat-file durability (which
  only covers sandboxes that got that far) don't give you. At daemon
  startup, any record still marked live is unconditionally marked
  `orphaned_by_restart` — not a heuristic, since nothing can make it
  genuinely still live at that point. **Live-verified end to end**,
  including the actual restart case: created a sandbox, confirmed its
  live history entry, restarted the real daemon, confirmed it flipped to
  `orphaned_by_restart` with its tags intact, while separately-recorded
  destroyed/snapshotted entries from before the restart were untouched.
  15 new `scripts/integration-test.sh` checks (the create/destroy/
  snapshot recording path — the restart case itself needs a real daemon
  restart mid-test, verified manually instead), 209/209 passing overall.
- **Done: guest-accessible metadata service**, via Firecracker's own
  native MMDS (Microvm Metadata Service) rather than anything
  sandkiln-built — every sandbox with a name and/or tags automatically
  gets its own `{id, name, tags}` served at `http://169.254.169.254/`
  inside the guest, a link-local HTTP endpoint Firecracker's device
  model answers directly at the network layer. **No vsock/guest-agent
  involvement at all** — this needed zero protocol or guest-agent
  changes, and zero new daemon HTTP routes or SDK/CLI surface (nothing
  outside the guest ever calls this; it's automatic from whatever
  `tags`/`name` a caller already provides at create time). Configured
  V2 (token-gated: the guest must `PUT
  http://169.254.169.254/latest/api/token` for a session token before
  it can `GET` anything) rather than V1's plain unauthenticated GET,
  since a sandbox may run untrusted or AI-generated code that could
  otherwise SSRF an unauthenticated metadata endpoint. **Live-verified**
  by actually curling it from inside a real sandbox (5 new
  `scripts/integration-test.sh` checks, 194/194 passing) — including a
  real gotcha worth knowing: `GET /` with no `Accept` header returns an
  AWS-IMDS-style newline-separated list of top-level key names (`id`,
  `name`, `tags`), not the JSON value itself; `Accept: application/json`
  is required to get the actual content back. Not yet wired: live
  updates if tags ever become mutable after create (no such API exists
  yet — `PATCH /mmds` is available in Firecracker whenever that's
  needed) or exposing resource config (`vcpu_count`/`mem_size_mib`/etc.)
  alongside `id`/`name`/`tags` (out of scope for what this item asked
  for).

## Benchmarking

- **Done:** `criterion` benchmarks in `sandkiln-vmm` for boot time and exec
  latency, run against the real Firecracker binary (not mocked). Run with
  `SANDKILN_BENCH_FIRECRACKER_BIN=<path> SANDKILN_BENCH_KERNEL_PATH=<path>
  SANDKILN_BENCH_ROOTFS_PATH=<path> cargo bench -p sandkiln-vmm --bench
  vm_lifecycle`.
- **Done:** A scripted load test: concurrent sandbox creation/exec through
  the daemon's HTTP API. Run with `scripts/load-test.sh [concurrency]
  [iterations] [base-url]` against a running `sandkilnd` (defaults: 10
  workers, 20 iterations each, `http://127.0.0.1:7777`).
- **Done:** A full end-to-end integration test, `scripts/integration-test.sh
  [base-url]` — sandbox lifecycle, tags, drives (including persistence
  across sandboxes and conflict detection), snapshot/resume, auth,
  `/metrics`, and error cases, all in one repeatable run against a real
  daemon. Tracks and tears down everything it creates. See root
  `AGENTS.md`'s "Integration testing" section.
- **Real measured results** (dev box, single node, 8-tap pool). The
  numbers below predate the `wait_for_socket`/`Vm::call` retry fixes
  further down this section — re-run with `cargo bench -p sandkiln-vmm
  --bench vm_lifecycle` after those landed:
  - Cold boot (criterion): **10.5–10.9ms** (was 32.3–33.1ms before the
    `wait_for_socket` fix — see below).
  - Exec round-trip on an already-open vsock connection (criterion):
    **220–250µs**.
  - Load test, 4 concurrent workers × 5 cycles, 0 errors:
    **before** the fix below — 5.59 cycles/sec, `create` mean 369ms
    (p95 588ms).
    **after** — 5.38 cycles/sec, `create` mean **211ms** (p95 577ms),
    `exec` mean 366ms, `delete` mean 131ms. (Overall throughput is flat —
    `create` got cheaper but isn't the only phase in a cycle, and both
    runs are small samples on a shared, variable-load dev box — but the
    `create`-specific improvement is real and repeatable.)
  - **Finding, partially fixed**: `create`'s mean was far above the ~33ms
    cold-boot number — the gap was the ~300MB rootfs copy, done
    synchronously *after* the network lease. Fixed: the copy now runs
    concurrently with the lease (independent work, no reason to serialize
    them) and uses `cp --reflink=auto`, an instant copy-on-write clone on
    a filesystem that supports it (XFS, Btrfs). On this dev box's ext4,
    `--reflink` can't help — ext4 has no CoW.
  - **Follow-up, actually measured (not just assumed) live**: set up a
    real XFS loopback filesystem on the dev box and pointed
    `SANDKILN_BASE_ROOTFS` at it. Confirmed the CoW clone is real —
    cloning the same 300MiB rootfs four times added a measured ~4MiB of
    real disk usage total (`df`), not ~1.2GiB — and `cp --reflink=auto`
    itself dropped from ~110ms (ext4) to ~0ms (XFS). **But end-to-end
    `POST /sandboxes` latency was unchanged** (~160-170ms either way, five
    creates each) — because the copy already runs *concurrently* with the
    network lease (see above), shrinking it to ~0ms just means the join
    now waits on the lease side instead; it was never truly serial once
    that concurrency landed. This means the earlier "still needs a
    CoW-capable filesystem or a device-mapper layer to close the gap"
    framing was based on an assumption (the copy is *the* bottleneck)
    that turned out not to hold once actually measured on a real
    CoW filesystem. `scripts/preflight-check.sh` still reports whether
    rootfs storage is on a CoW-capable filesystem (real, still a correct
    thing to want — a disk-space-free clone is a genuine win on its own).
    **The reasoning in this entry for *why* XFS didn't help is itself
    wrong — see the profiling pass immediately below, which measured the
    network lease at ~4ms and so rules out "the join now waits on the
    lease side instead" as an explanation.** The entry is left standing
    rather than edited because the measurements in it (the CoW clone is
    real; end-to-end latency didn't move) are correct and reproducible —
    only the causal story attached to them was guesswork.
- **Done: a real per-phase profiling pass of the cold-create path**, the
  "honest next step" the entry above asked for. Instrumented every
  sub-step of `create_sandbox_cold` and `Vm::boot` (see "Instrumentation"
  at the end of this entry) and ran 20 sequential `POST /sandboxes`
  creates on the dev box, one at a time, each sandbox destroyed before
  the next, with nothing else running on the box.
  - **What we measured.** Means over 20 creates (ext4, 300MiB base
    rootfs, no jailer, 2 vCPU / 512MiB, `POST /sandboxes` with an empty
    body). Total **167.65ms** (min 161.41, max 179.10):

    | phase | mean | share |
    | --- | --- | --- |
    | rootfs clone (`cp --reflink=auto`) | 124.09ms | 74.0% |
    | `Vm::boot` total | 34.00ms | 20.3% |
    | — process spawn | 0.34ms | 0.2% |
    | — wait for the API socket | 20.11ms | 12.0% |
    | — the 7 configuration PUTs | 1.61ms | 1.0% |
    | — `InstanceStart` | 11.87ms | 7.1% |
    | history-store sqlite write | 9.24ms | 5.5% |
    | network lease (concurrent) | 4.36ms | ~0.1% of the critical path |

    Those add to 167.33ms of the measured 167.65ms — **0.32ms
    unaccounted**, so this is a complete breakdown, not a partial one.
  - **What we found — the two live hypotheses going in were both wrong,
    and in the same direction.** `NetworkManager::lease` is **~4.36ms**,
    not the ~130ms it was suspected of: its three `ip`/`bridge`
    subprocess calls measure 1.39ms / 1.46ms / 1.46ms, and because the
    lease runs concurrently with the rootfs clone it contributes only
    **~0.17ms** to the critical path (setup join 124.26ms vs. the clone's
    own 124.09ms). Firecracker's per-VM API configuration is **~1.61ms**
    across all seven PUTs — the slowest single one is
    `/network-interfaces/eth0` at 0.51ms. Batching the `ip`/`bridge`
    calls or replacing them with direct netlink would therefore buy at
    most a couple of milliseconds off a 167ms create; **a netlink crate
    is not worth adding for this, and isn't planned.**
  - **The rootfs copy *is* the dominant cost after all — 74% of a cold
    create** — which is what the entry above set out to disprove. Both
    entries are measured and both are correct; what was wrong was the
    *explanation* offered above for the XFS null result. The copy was
    never "hidden behind the lease", because the lease is ~4ms.
  - **Leading hypothesis for the XFS null result, explicitly not yet
    verified.** `clone_rootfs` copies *into* `std::env::temp_dir()`, and
    `cp --reflink=auto` silently falls back to a full byte copy when
    source and destination are on different filesystems. The XFS
    experiment moved only `SANDKILN_BASE_ROOTFS` onto the XFS loopback;
    the destination stayed on `/tmp` (ext4), so the daemon's actual clone
    could never have reflinked, even though a standalone `cp` *within*
    the XFS mount did. That fits every number in both entries, but it has
    **not** been re-tested: the XFS loopback no longer exists on the box
    and mounting one needs root. Treat it as the next thing to check, not
    as established.
  - **What's still open.** If that hypothesis holds, the fix is to give
    the per-sandbox rootfs copy a configurable destination directory so
    it can be placed on the same filesystem as the base image, instead of
    always landing in `std::env::temp_dir()`. That is a config-surface
    change that also touches snapshot/archive path assumptions, so it was
    deliberately **not** attempted in this pass — and on this ext4-only
    box its payoff can't be measured at all, which is exactly the
    situation that produced the wrong conclusion above. Also unexamined:
    the **9.24ms synchronous sqlite write** (`history.record_created`)
    sitting on the critical path, 5.5% of a create, for a write whose own
    doc comment already calls it best-effort. Moving it off the request
    path looks easy and worth ~9ms, but wasn't measured as a change here.
  - **What we fixed: the API-socket wait.** `wait_for_socket` polled for
    Firecracker's API socket with a fixed `sleep(20ms)`. It measured
    **20.11ms on every single boot** (min 20.04, max 20.18 across 20) —
    the signature of one quantized sleep rather than of real waiting.
    Replaced with `connect_api_with_retry`, which retries the *connect*
    (not a file-existence check) on a 200µs→5ms backoff. Retrying the
    connect also closes a real race the faster polling would otherwise
    have opened: the socket file appears at `bind()`, a moment before
    `listen()`, so a fast existence-poll can win that race and get
    `ECONNREFUSED`.
    - **Measured after, same methodology, 2×20 creates:** socket wait
      **20.11ms → 0.67ms / 0.69ms** (so Firecracker really is ready in
      well under 1ms, and ~19.4ms of every boot was dead sleep);
      `boot_duration_ms` **34.00ms → 11.22ms / 11.44ms**; end-to-end
      create **167.65ms → 144.59ms / 143.56ms**. A **~23ms (~14%)** cut
      to every cold create, and it applies to snapshot resume too, which
      used the same helper. `InstanceStart` also read lower afterwards
      (11.87ms → 8.75/9.13ms); that wasn't the target of the change and
      isn't confidently attributed to it.
    - Verified with the full `scripts/integration-test.sh` — snapshot,
      resume, and time-travel restore all pass. One run out of four
      failed a pool-claim MMDS assertion; the three runs after it were
      300/300 clean, and the code path that assertion covers
      (`Vm::update_metadata`) is untouched by this change, so this reads
      as the already-documented intermittent resume failure below rather
      than a regression — stated as a reading of the evidence, not as
      something proven.
  - **Instrumentation left behind**, so none of this has to be
    rediscovered by hand: `/metrics` gains
    `create_phase_duration_ms{phase="rootfs_clone"|"network_lease"|"setup"|"total"}`
    (`boot_duration_ms` stays its own metric, unchanged). Finer detail is
    debug-level `tracing` rather than metrics — nobody alerts on "one
    `ip` exec took 1.4ms", and `sandkiln-vmm` has no access to the
    daemon's `Metrics` anyway: `"cold create setup phases"` and
    `"cold create complete"` (`sandkilnd::routes_sandbox`),
    `"vm boot phase breakdown"` with per-endpoint PUT timings
    (`sandkiln_vmm::vm::boot`), and `"attached tap device"` with each
    subprocess call timed separately (`sandkiln_vmm::network`).
    `scripts/dev-tools/profile-cold-create.sh` drives the run. Note the
    log filter is the **binary** name: `RUST_LOG=sandkilnd=debug`, not
    `sandkiln_daemon=debug`, which silently matches nothing.
  - **Caveat, same as every number in this section:** a shared dev box,
    small samples, one create at a time. The phase *shares* are large and
    consistent enough to act on; the absolute totals move a few percent
    run to run.
- **Done: fixed `Vm::call`/`open_pty`/`open_exec_stream`'s own retry
  loop** — the same fixed-sleep bug class as `wait_for_socket` above,
  found by auditing the codebase for siblings after that fix landed.
  Each retried a failed vsock call on a flat `sleep(100ms)` rather than a
  backoff — 5x the cost per wasted retry that `wait_for_socket`'s 20ms
  sleep had. Replaced with the same `retry_with_backoff` helper
  `connect_api_with_retry` now shares (1ms→20ms backoff for the vsock
  case, since a guest agent's own startup is a slower, more variable
  race than Firecracker's bare API socket appearing) — one generic
  helper instead of three near-identical hand-rolled loops.
  **Live-measured, and the fix's real benefit turned out to be a much
  smaller part of a much bigger story**: A/B'd old vs. new code, timing
  a fresh sandbox's first real `exec` end to end. Both measured
  **~420-460ms** — indistinguishable within noise. This *is* still a
  real, correct fix (worst-case wasted overshoot per retry drops from
  ~100ms to ~20ms), but on this box the guest agent's own startup time
  (kernel finishing boot, systemd, the agent binary starting and binding
  vsock) so thoroughly dominates the ~420-460ms that shaving retry-loop
  overshoot doesn't show up as a measurable win by itself. That
  measurement is the real finding — see below.
  - **The real discovery: `POST /sandboxes` returning 200 does not mean
    the sandbox is actually ready to use, and nothing before this
    measured that gap.** Every exec *after* the first, on the same
    sandbox, measured **~3-5ms** — confirming the ~420-460ms is a
    one-time "is the guest agent listening yet" tax, currently paid
    silently by whichever caller happens to make the first real call,
    misattributed to "exec is slow" rather than being visible as its own
    thing. Every benchmark elsewhere in this document stops at
    Firecracker's `InstanceStart` succeeding or the daemon's own
    `create()` returning — none of them measure through to "the guest
    agent actually answered a real request," which is the number that
    actually matters for "how long until my code runs."
  - **A resumed sandbox skips almost all of it.** Same test against a
    snapshot resumed from an already-warmed sandbox (agent confirmed
    live before the snapshot was taken): first exec after resume
    measured **~4-18ms** — a **~25-100x** difference from a cold
    create's ~420-460ms, confirmed repeatedly, not a one-off. This is a
    dramatically bigger, more real version of the pre-warmed-pool win
    already documented below — that section's own "70-200ms vs.
    160-200ms" framing only ever compared `create()` returning, never
    "time until the sandbox can actually run something," which is where
    almost this entire gap actually lives.
  - **`scripts/bench-report.sh` now tracks this permanently** as
    `first_exec_client`, timed the same way a real caller would
    experience it (client-side, immediately after `create()` returns),
    alongside its existing daemon-`/metrics`-based phase breakdown — so
    this doesn't have to be rediscovered by hand again, the same
    motivation behind that script's own creation.
  - **What's still open**: whether the daemon should block `create()`
    until the guest agent actually answers once, so `POST /sandboxes`
    returning 200 genuinely means "ready," not just "Firecracker
    started it" — a real API-semantics question, not just a performance
    one, since today's fast `create()` number is honest about what it
    measures but easy to misread as "time until usable." Not changed
    yet — this needs a decision, not just a fix.
- **Done: snapshot/resume benchmarked** (`bench_snapshot_take`/
  `bench_resume` in `core/crates/vmm/benches/vm_lifecycle.rs`, alongside
  the existing `bench_cold_boot`/`bench_exec_roundtrip`). Real numbers,
  same dev box as above:
  - `snapshot_take` (pause + write memory/state to disk): **~322ms**
    (309.6–335.0ms) — expensive, dominated by writing the guest's full
    memory to disk synchronously; a real cost for auto-suspend and any
    future tiered-idle-lifecycle work, not free.
    **This dev box is a real, actively-used desktop** (an editor, a
    browser, a remote-desktop client all running concurrently), not a
    dedicated bench machine, and `snapshot_take`'s synchronous disk
    write is far more exposed to that than the other benchmarks here.
    Re-running it later measured 552.7ms (a 20-iteration bench-report.sh
    run) and, in isolation via `cargo bench -- snapshot_take`,
    560-915ms with criterion's own significance test reporting no
    detected change from its prior run (p=0.14) — genuinely wide
    variance under today's real background load, not a regression and
    not a data error. The ~322ms figure stands as a real measurement
    from a quieter moment; treat both as real, load-dependent readings
    rather than picking one as authoritative.
  - `resume_from_snapshot`: **~25.8ms** (25.5–26.2ms) — only **~19%
    faster than `cold_boot`'s ~31.9ms** (31.5–32.4ms), not the dramatic
    win the earlier framing below assumed. Both numbers are already
    small on this base image (lightweight kernel, minimal guest-agent
    init) — cold boot has little slack left for resume to undercut.
    **Both predate the `wait_for_socket` fix** (see further down this
    section) — re-measured afterward: `resume_from_snapshot`
    **6.5–7.7ms**, `cold_boot` **10.5–10.9ms**, still only a modest gap
    between them for the same reason as before. The real, much larger
    gap this framing was missing entirely — resume vs. cold create
    counted through to "the sandbox can actually run something," not
    just "boot finished" — is the `first_exec_client` finding later in
    this section.
  - **This changes what a pre-warmed pool actually buys**: the ~180ms
    gap between a ~32ms boot and the measured 211ms full-create (see
    "What works today" above) isn't in the boot/resume step at all —
    both are ~25–32ms either way — it's in the surrounding per-create
    setup (rootfs prep, network lease; since profiled, and it is almost
    entirely the rootfs prep — the lease is ~4ms). A pre-warmed pool's real value
    is pre-doing *that* setup ahead of a request, not shaving boot
    latency itself, which was never the bottleneck. Worth re-measuring
    once a pool exists, rather than assumed up front.
- **Done: pre-warmed snapshot pool, a first honestly-scoped slice.** The
  mechanism sandkiln already has (snapshot/resume, auto-suspend) is the
  same one production Firecracker users document as their main cold-start
  fix — restore an already-initialized VM instead of booting one from
  scratch. This closes the gap identified above: `POST /pools`
  (`{id, image_id?, vcpu_count?, mem_size_mib?, warm_count}`) configures a
  pool; a background replenisher (`sandkiln-daemon`'s `pool_replenisher`
  module, ticking every 2s) keeps `warm_count` resumable snapshots ready
  per pool; a plain `POST /sandboxes` (no `drives`, no `rate_limit` — see
  below) matching a pool's `image_id`/resolved `vcpu_count`/`mem_size_mib`
  claims a warm snapshot automatically instead of cold-booting — entirely
  transparent, no separate "create from pool" call. `Pool.create/list/delete`
  in the JS/TS SDK, `kiln pool create|ls|rm` in the CLI.
  Live-verified, including the actual latency win on real hardware — with
  an important, honestly-measured caveat (see the finding right below
  this): a **clean** claim (the resumed snapshot passes its health check)
  measured at roughly **70–200ms** across repeated runs vs. a cold
  create's own **~160–200ms** on the same box, comparing `create()`
  returning in each case — a real but modest win at this sample size
  (small-sample numbers on a shared, variable-load dev box, not a
  controlled benchmark — see the Benchmarking section for the
  underlying boot/resume/setup breakdown this is built on). That clean
  win is **not** the reliable common case on this dev box today, though —
  see the failure-rate finding immediately below, which affects a large
  enough fraction of claims that "pools make creates faster" needs that
  caveat attached, not stated as a clean, unconditional result.
  **This still understates the real win, found later**: `create()`
  returning isn't the same as "ready to use" — see the Benchmarking
  section's `first_exec_client` finding. A cold create's *first real
  exec* pays an extra, currently-unmeasured-here **~420-460ms** the
  guest agent needs to actually start listening; a pool claim, resuming
  an already-warm agent, pays almost none of it (~4-18ms). Counting
  through to "the sandbox actually ran something," not just "`create()`
  returned," the real gap this feature closes is closer to **25-100x**,
  not the modest 2x this bullet's own numbers suggest on their own.
  - **A real Firecracker/KVM finding from building this, not a sandkiln
    bug — and a significant one, not a rare edge case.** Resuming a
    snapshot has a failure mode where the restored guest kernel panics
    early in boot (confirmed via its captured console log — an
    early-boot divide-by-zero trap in the console driver, restored
    CPU/timer state interacting badly with timing-sensitive init code —
    plausibly TSC/clock-source drift between snapshot-time and
    restore-time, though that's an informed guess, not a confirmed root
    cause) and the whole Firecracker process exits shortly after.
    **Measured directly, repeatedly, on this dev box**: across several
    clean, isolated, sequential single-claim test runs (no concurrent
    load), the failure rate ranged from roughly **1 in 3 to 2 in 3**
    resumes — not a rare one-off. `08-snapshots.sh`'s own single
    resume-then-exec check still passes every run because one resume
    isn't enough attempts to reliably hit it; a pool's whole purpose is
    resuming far more often than a manual test ever would, which is
    exactly what surfaced a failure rate that would otherwise have stayed
    invisible. Whether this is specific to this dev box's kernel/KVM/CPU
    combination, this project's own guest kernel build, or a broader
    Firecracker snapshot/restore characteristic is genuinely open —
    worth real investigation before assuming it generalizes, but not
    worth blocking this feature on, given the fallback below makes it
    safe either way.
    Handled, not just noted: every claim runs a real post-resume health
    check (`exec true`) before being handed to the caller — a claim that
    fails it tears the broken sandbox down (via `Vm::force_stop`, which
    skips `stop()`'s own optimistic "sync before kill" call entirely,
    since a VM that just failed its health check was never going to
    respond to that either — found live too: without this, a fallback
    paid two independent ~5-second retry timeouts back to back, ~10.3s
    total, instead of one, ~5.4s) and transparently falls back to a
    normal cold create, so a caller never receives a dead sandbox id.
    Confirmed live via repeated stress tests (0 caller-visible failures
    across every run; the daemon log confirms the fallback path actually
    firing at the rate described above).
  - **A related, separately real finding**: Firecracker's snapshot/restore
    does not preserve MMDS's initialized state — `PATCH /mmds` alone fails
    with "MMDS data store is not initialized" after a resume, even though
    the VM was networked and had MMDS configured before being snapshotted.
    `sandkiln_vmm::vm::Vm::update_metadata` works around this by redoing
    the full `PUT /mmds/config` + `PUT /mmds` sequence rather than a bare
    `PATCH`, so a pool-claimed sandbox's guest-visible MMDS content
    correctly reflects the caller's real identity, not the warm-boot
    placeholder's.
  - **Scoped honestly, matching the shape sketched above with one
    deliberate cut**: pool identity is keyed by image + resolved
    vcpu/mem config (not just image); `warm_count` of `0` is valid
    (an inert pool); a request with `drives` or a custom `rate_limit`
    never matches a pool, since both are baked into a VM's state at boot
    time and a warm snapshot was booted with neither — falls through to
    cold create rather than silently ignoring them. Pool *configuration*
    is in-memory only, not durable across a daemon restart (a caller
    re-`POST`s after one) — and a daemon restart can orphan an
    already-warm snapshot that has no pool left to claim or clean it up,
    a known, not-yet-solved edge of that same limitation.
  - **Done: `max_count` ceiling with queueing.** `PoolConfig.max_count`
    (`Option<u32>`, unbounded when unset) caps the total number of *live*
    instances — warm + claimed, combined — a pool's profile may have at
    once; `POST /sandboxes` matching a pool at its ceiling with nothing
    warm **queues** (a `tokio::sync::Notify` per pool, woken on every
    warm-replenish or every live instance stopping) for up to 30 seconds
    before returning a real `503`, rather than either rejecting the
    request outright or silently exceeding the ceiling. `GET /pools` now
    also reports `max_count` and `claimed`. Live-verified end to end: a
    `max_count: 1` pool correctly blocks a second concurrent claim, wakes
    it the instant the first is stopped (not on any fixed poll interval),
    and returns a clean `503` with a clear message after the full 30s
    when nothing ever frees up.
    - **A real bug found in this feature by live-testing it, not
      assumed away**: the first version shipped without retrying a
      failed warm claim's pool resolution. Since a warm claim's
      post-resume health check fails at the rate documented above (up to
      ~2-in-3 resumes), and a failed claim's fallback used to fall
      straight through to a *plain, unattributed* cold create, a
      `max_count`-bounded pool under a bad run of resumes could silently
      end up running more live instances than its own configured
      ceiling — the accounting simply never saw the fallback create at
      all. Caught by watching `GET /pools`'s `claimed` count read `0`
      right after a claim that had, per the daemon's own log, actually
      hit the health-check-failure path — a real, live, "the number is
      wrong" discovery, not a design review catching it on paper. Fixed
      with a bounded (3-attempt) retry: a failed warm claim's reserved
      slot is released immediately (`PoolClaimGuard`'s `Drop`), and the
      very next loop iteration re-resolves the pool from scratch, which
      correctly finds room and attributes the resulting cold-create
      fallback to the pool instead of letting it slip through unbounded.
      A genuinely pathological run of 3 consecutive bad resumes still
      falls through unattributed at the end — an accepted, rare residual
      edge case, not a claim this closes completely.
- **Done: snapshot lineage.** `Snapshot.parent_snapshot_id` records the
  snapshot a new snapshot's own source sandbox was itself resumed/forked
  from, if any — `None` marks a lineage root (a cold-booted source
  sandbox). `GET /snapshots` exposes it on every `SnapshotSummary`
  (answers "what did this come from") and gained a second filter,
  `?parent_snapshot_id=<id>` (answers "what came from this," and unlike
  the existing `?source_sandbox_id=` filter can genuinely match more than
  one snapshot, since a snapshot can be forked, that fork snapshotted and
  torn down, then forked again into a sibling line over time) — together
  enough to walk a full lineage tree in either direction, one request at
  a time, without a dedicated tree-shaped endpoint. Persists through
  `meta.json` the same way `name`/`egress` do. **A real bug caught live,
  not on paper, while wiring this up**: the obvious first implementation
  sourced the new pointer from the already-existing
  `Sandbox::source_snapshot_id` field — which turned out to be *only*
  ever set on a forked sandbox, deliberately left `None` on resume so a
  resumed sandbox (which owns its rootfs outright) stays eligible to be
  snapshotted again by `check_snapshottable`'s own `ForkedFrom` guard.
  Since a forked sandbox is exactly the one kind of sandbox that guard
  then refuses to let be snapshotted at all, reusing that field would
  have made every *resumed* sandbox's lineage — the actually-common,
  observable case — silently dead-ended, while producing correct-looking
  code that compiled and passed every test written against it up to that
  point. Live-tested against the real daemon before this was caught: a
  resume-then-snapshot chain queried by `?parent_snapshot_id=` came back
  empty. Fixed with a second, separate `Sandbox::parent_snapshot_id`
  field — `Some` on both resume and fork (both genuinely descend from
  that snapshot), `None` only for a real cold boot — decoupled entirely
  from `source_snapshot_id`'s unrelated rootfs-sharing-guard purpose. 10
  new `scripts/integration-test.sh` checks, 267/267 passing overall.
  **Deliberately narrow**: lineage is a parent *pointer*, not a durable
  ancestry record — a deleted intermediate snapshot breaks the visible
  chain at that point, since `GET /snapshots` only ever reflects
  snapshots that currently exist. A fully durable ancestry table (surviving
  every deletion in between) would be a real extension, not a bug fix, if
  it's ever needed.
- **Fan-out cloning**: cloning one snapshot into *several* new live
  sandboxes at once (not just one) is a real, documented pattern
  elsewhere, but it directly conflicts with the one-live-fork-per-snapshot
  constraint above (`Snapshot::forked_into`) — that constraint exists
  because Firecracker has no verified way to give two live descendants
  of one snapshot independent rootfs backing files or guest IP/MAC (see
  the fork bullet above). Genuine fan-out would need that same unsolved
  problem solved first, not a new API layered on top of today's fork;
  tracked here as a restatement of the existing gap in fan-out terms,
  not a separate, independently-achievable item.
- These numbers are from one manual run on one shared dev box, not
  isolated hardware — treat them as directionally useful, not authoritative.
  Automating re-runs so regressions are visible over time is still open.

## Observability

- **Structured logging — working today.** `tracing` throughout, not just
  at the daemon's edge: HTTP requests/responses (method, path, status,
  latency) via `tower-http`'s `TraceLayer`, and VM lifecycle events (boot
  with timing, vsock call latency, stop) emitted from `sandkiln-vmm`
  itself so the library is useful standalone, not just under the daemon.
  Correlated by `vm_id`, filterable per-module via `RUST_LOG`.
- **Done: request-id correlation.** Every HTTP request gets an id (caller-
  supplied via `X-Request-Id`, or generated) established as the active
  `tracing::Span` before any handler runs, echoed back in the response,
  and propagated across `spawn_blocking` into every `sandkiln-vmm` call
  the request triggers (`tracing_util::spawn_blocking_in_current_span`) —
  so one id ties an HTTP request to the VM boot/call/stop log lines it
  caused. Live-verified.
- **Done: `/metrics` endpoint** (Prometheus text format, unauthenticated
  like `/healthz`): `sandboxes_created_total` (counter), `sandboxes_active`
  (gauge), boot duration and exec latency (histograms). Hand-rolled text
  exposition rather than a new dependency — see `metrics.rs`.
- **Done: JSON log output** (`SANDKILN_LOG_FORMAT=json`) for production log
  pipelines, alongside the default pretty terminal format.
- **Done: guest-side console capture.** A guest that fails before it can
  answer over vsock (kernel panic, agent crash) is no longer invisible —
  the spawned Firecracker process's stdout/stderr (the guest's
  `console=ttyS0` output) is captured to a per-VM log file, and a boot
  failure's error message includes that file's path.

## Multi-node and regions

Everything so far assumes one daemon on one box. A single machine has a
ceiling — on concurrent sandboxes, on blast radius if it goes down, on
being close to wherever the caller actually is:

- Multiple daemon instances, each owning its own bridge/tap pool/rootfs
  storage, with something in front that knows which sandboxes live where
  (a routing layer, not full clustering — a sandbox is tied to the node
  it booted on, not migrated between them).
- A place identifier ("region") a caller can request at creation time,
  even if early on that just means "which physical box," not literal
  geographic distribution.
- This is deliberately last among the infrastructure work — it multiplies
  the surface area of everything above it (networking, images, storage),
  so it should land once those are individually solid on one node.

## Ecosystem and integrations

The primitive is only as useful as what's built on top of it:

- Example integrations with agent frameworks and coding-agent tools —
  showing a sandbox as the execution backend for agent-generated code,
  not just a standalone API.
- **Done:** a minimal reference "code playground"
  (`examples/code-playground`, JS/TS) and a reference "AI agent runner"
  (`examples/agent-runner`, Python) as real, runnable example projects
  against each SDK's actual current API — see `examples/AGENTS.md`.
- Consider what a plugin/adapter surface would look like once there's
  more than one real integration to generalize from — not before.
- **Deliberately out of scope for this project**: a durable-workflow layer
  (queues, timers, retries, fan-out across many sandboxes) is a different
  abstraction *on top of* a sandbox primitive, not part of the primitive
  itself. Worth knowing that shape exists — it's what turns "run this
  command in an isolated VM" into "orchestrate a long-running agent
  workflow" — but it belongs in a separate project/library built against
  this one's API, not merged into the daemon.

## Ideas explicitly not started, kept honest rather than silent

Two capabilities other sandbox platforms document that sandkiln has
zero coverage of — each closer to a separate product surface than a
small extension of what exists, called out deliberately (same reasoning
as the GPU-passthrough note above) rather than left as a silent gap:

- **Desktop/GUI automation**: a managed desktop environment inside a
  sandbox (a windowing environment plus a browser, reachable over VNC or
  a browser-based remote-desktop client), with a screenshot-capture API
  and a full keyboard/mouse input-control API, plus reconnecting to an
  already-running desktop sandbox by id without killing the VM. Real,
  documented use cases: browser-based agent automation, computer-use
  agent evals. This is a meaningfully different product surface than
  "run untrusted code and read/write files" — it needs its own base
  image work (a desktop environment, a VNC server, input-injection
  tooling inside the guest) well beyond today's universal image.
- **Git-native sandbox filesystem**: version-controlled sandbox state as
  a first-class concept — workspaces as private branches mounted as
  POSIX directories, a snapshot doubling as a commit, merging by moving
  a pointer rather than copying data, and read-only mounts for sharing
  common tooling across many sandboxes either following a moving head or
  pinned to one snapshot. A substantial new subsystem (a real git
  server/object store this project doesn't have any of today), not a
  small extension of `snapshot`/`resume`/`fork` — those stay path-based
  and single-lineage; this would be a different persistence model
  layered alongside them, not a replacement.

Both are plausible future directions for this project specifically
because it's a personal project without a fixed roadmap deadline, not
because either is a small lift — treat this section as "worth doing
eventually if there's appetite," not "next."

## Documentation and examples

- **Done**: a full docs site (`website/docs/`, Astro + Starlight) —
  Getting Started, Core Concepts, Guides, Reference (daemon HTTP API,
  JS/TS SDK, Python SDK, CLI), and Architecture, covering both SDKs and
  the CLI with runnable examples throughout. Deployed three ways: as a
  `/docs` subpath of the main site (GitHub Pages and the merged Vercel
  build) and standalone at its own domain root
  (sandkiln-docs.vercel.app) — see `website/AGENTS.md`.
- Example projects: **done** — nine real, runnable reference projects
  against each SDK's published API, one per major feature surface: code
  playground, AI-agent sandbox runner, dev-server preview, interactive
  terminal, pre-warmed pool, streamed exec/logs, remote storage mount,
  snapshot/resume/fork lifecycle, and named/persistent sandboxes. See
  `examples/AGENTS.md` for what each one demonstrates and why.
