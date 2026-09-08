---
title: CLI (kiln)
description: Every kiln subcommand and flag.
---

```bash
npm install -g sandkiln-cli   # installs the `kiln` command
```

Global options, available on every subcommand: `--base-url <url>` (default: `SANDKILN_DAEMON_URL` or `http://127.0.0.1:7777`), `--token <token>` (default: `SANDKILN_AUTH_TOKEN`).

## `kiln sandbox`

| Command | What it does |
|---|---|
| `create` | Boot a new sandbox. `--name <name>`, `--tag <key=value>` (repeatable), `--vcpu <count>`, `--mem <mib>`, `--image <id>`. |
| `get-or-create` | Resolve `--name` (required) to a sandbox in one call: live as-is, resumed if stopped, or created fresh. Prints the id and whether it was freshly created. `--tag`, `--vcpu`, `--mem` (used only if a fresh sandbox is created). |
| `by-name <name>` | Resolve a name to a live sandbox's id. |
| `ls` | List sandboxes. `--tag <key=value>` (repeatable) filters. |
| `rm <id>` | Stop a sandbox. Preserves state as a snapshot by default; `--destroy` fully destroys it instead — no snapshot, nothing left to resume. |
| `exec <id> <command> [args...]` | Run a command inside a sandbox. Exits with the command's own exit code. |
| `read <id> <path>` | Read a file from a sandbox and print it to stdout. |
| `write <id> <path> <local-file>` | Write a local file into a sandbox at the given path. |
| `preview <id> <port>` | Print the URL to reach a server listening on `<port>` inside a sandbox. `--path <path>` (default `/`). |
| `snapshot <id>` | Save a sandbox's full state to disk and stop it. Prints the resulting snapshot id. |
| `snapshots` | List snapshots. `--source <sandbox-id>` narrows to the one (if any) taken from that original sandbox id. |
| `resume <snapshot-id>` | Boot a new sandbox from a snapshot, consuming it. Prints the new sandbox id. |
| `fork <snapshot-id>` | Boot a new sandbox from a snapshot without consuming it. Prints the new sandbox id. |

## `kiln image`

| Command | What it does |
|---|---|
| `create <id> <path>` | Register an already-built ext4 rootfs at `<path>` (a path on the daemon's own host, not a file upload) under `<id>`. Prints a warning that the guest agent can't be verified without root — see [Custom & managed images](/docs/concepts/images/). |
| `ls` | List registered images. |
| `rm <id>` | Delete a registered image. Refused while any sandbox, in-flight boot, or snapshot still references it. |

Every subcommand's own action handler catches errors and reports them on stderr with a non-zero exit — nothing escapes as a raw stack trace.
