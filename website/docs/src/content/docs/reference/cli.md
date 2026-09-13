---
title: CLI (kiln)
description: Every kiln subcommand and flag.
---

```bash
npm install -g sandkiln-cli   # installs the `kiln` command
```

Global options, available on every subcommand: `--base-url <url>` (default: `SANDKILN_DAEMON_URL` or `http://127.0.0.1:7777`), `--token <token>` (default: `SANDKILN_AUTH_TOKEN`).

## A complete session

```bash
export SANDKILN_DAEMON_URL=http://127.0.0.1:7777
export SANDKILN_AUTH_TOKEN=...   # omit entirely for an unauthenticated local daemon

# Named, so you can find it again tomorrow without tracking an id yourself.
kiln sandbox get-or-create --name build-worker --vcpu 2 --mem 1024

kiln sandbox exec build-worker npm install
kiln sandbox exec build-worker npm test

kiln sandbox write build-worker /tmp/config.json ./local-config.json
kiln sandbox read build-worker /tmp/result.json

kiln sandbox preview build-worker 3000

kiln sandbox ls --tag env=ci

# Preserves state as a snapshot -- resolve it by the same name tomorrow
# with another `get-or-create`, no snapshot id to remember.
kiln sandbox rm build-worker
```

## `kiln sandbox`

| Command | What it does |
|---|---|
| `create` | Boot a new sandbox. `--name <name>`, `--tag <key=value>` (repeatable), `--vcpu <count>`, `--mem <mib>`, `--image <id>`, `--rate-bandwidth <bytes-per-sec>`, `--rate-ops <ops-per-sec>`, `--drive <id[:ro]>` (repeatable — attach an existing persistent drive, `:ro` for read-only). A plain create matching a configured pool's image/resources (and no `--drive`/rate flags) resumes a warm snapshot automatically — see `kiln pool` below. |
| `get-or-create` | Resolve `--name` (required) to a sandbox in one call: live as-is, resumed if stopped, or created fresh. Prints the id and whether it was freshly created. `--tag`, `--vcpu`, `--mem`, `--rate-bandwidth`, `--rate-ops`, `--drive` (all used only if a fresh sandbox is created). |
| `by-name <name>` | Resolve a name to a live sandbox's id. |
| `ls` | List sandboxes. `--tag <key=value>` (repeatable) filters. |
| `rm <id>` | Stop a sandbox. Preserves state as a snapshot by default; `--destroy` fully destroys it instead — no snapshot, nothing left to resume. |
| `exec <id> <command> [args...]` | Run a command inside a sandbox. Exits with the command's own exit code. |
| `read <id> <path>` | Read a file from a sandbox and print it to stdout. |
| `write <id> <path> <local-file>` | Write a local file into a sandbox at the given path. |
| `chmod <id> <path> <mode>` | Change a file's permission bits (octal, e.g. `644`). |
| `chown <id> <path> <uid> <gid>` | Change a file's owner/group. |
| `mkdir <id> <path>` | Create a directory. |
| `rename <id> <from> <to>` | Rename/move a file or directory. |
| `cp <id> <from> <to>` | Copy a file within the sandbox. |
| `symlink <id> <target> <link-path>` | Create a symlink. |
| `readlink <id> <path>` | Print a symlink's target. |
| `truncate <id> <path> <size>` | Truncate (or extend) a file to an exact byte size. |
| `ls-dir <id> <path>` | List a directory's entries with real metadata (type, mode, size, mtime). |
| `pty <id>` | Open a live, interactive shell session inside a sandbox over a WebSocket — distinct from `exec`'s request/response shape. Raw terminal mode: keystrokes, including Ctrl+C, pass straight through to the remote shell. Exit the remote shell (or Ctrl+D) to end the session. |
| `preview <id> <port>` | Print the URL to reach a server listening on `<port>` inside a sandbox. `--path <path>` (default `/`). |
| `snapshot <id>` | Save a sandbox's full state to disk and stop it. Prints the resulting snapshot id. |
| `snapshots` | List snapshots. `--source <sandbox-id>` narrows to the one (if any) taken from that original sandbox id. |
| `resume <snapshot-id>` | Boot a new sandbox from a snapshot, consuming it. Prints the new sandbox id. |
| `fork <snapshot-id>` | Boot a new sandbox from a snapshot without consuming it. Prints the new sandbox id. |

## `kiln image`

| Command | What it does |
|---|---|
| `create <id> <path>` | Register an already-built ext4 rootfs at `<path>` (a path on the daemon's own host, not a file upload) under `<id>`. Prints a warning that the guest agent can't be verified without root — see [Custom & managed images](../../concepts/images/). |
| `ls` | List registered images. |
| `rm <id>` | Delete a registered image. Refused while any sandbox, in-flight boot, or snapshot still references it. |

## `kiln drive`

| Command | What it does |
|---|---|
| `create <size-mib>` | Create a new empty persistent drive of `<size-mib>` MiB. Prints its id — attach it with `sandbox create --drive <id[:ro]>`. See [Drives](../../concepts/drives/). |
| `ls` | List persistent drives, including every current holder. |
| `rm <id>` | Delete a drive and its backing file. Refused while any sandbox or held snapshot still attaches it. |

## `kiln pool`

Configures pre-warmed pools — ready-to-resume snapshots a matching `kiln sandbox create` claims automatically instead of cold-booting. No separate "claim" command; see [Startup latency & the pre-warmed pool](../../architecture/startup-latency/).

| Command | What it does |
|---|---|
| `create <id>` | Configure a pool under `<id>`. `--image <id>`, `--vcpu <count>`, `--mem <mib>` (all daemon defaults if omitted — a matching create must resolve to the exact same values to claim from this pool), `--warm-count <n>` (default `0`, how many resumable snapshots to keep ready). |
| `ls` | List configured pools, including how many are currently warm and ready. |
| `rm <id>` | Remove a pool's configuration and destroy whatever it currently has warm. A sandbox already claimed from it is unaffected. |

Every subcommand's own action handler catches errors and reports them on stderr with a non-zero exit — nothing escapes as a raw stack trace.
