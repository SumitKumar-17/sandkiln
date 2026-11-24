# sandkiln-mcp

A [Model Context Protocol](https://modelcontextprotocol.io) server that lets
AI agents run code in [sandkiln](https://github.com/SumitKumar-17/sandkiln)
sandboxes: hardware-isolated Firecracker microVMs. It speaks MCP over stdio and
talks to a `sandkilnd` daemon over HTTP, so you need a daemon running somewhere
reachable (see the self-hosting docs; there is no hosted service).

## Install

```bash
npm install -g sandkiln-mcp   # installs the `sandkiln-mcp` command
```

Or run it without installing: `npx -y sandkiln-mcp`.

## Configure an MCP client

Add the server to your client's MCP configuration (the file name and location
depend on the client):

```json
{
  "mcpServers": {
    "sandkiln": {
      "command": "npx",
      "args": ["-y", "sandkiln-mcp"],
      "env": {
        "SANDKILN_DAEMON_URL": "http://127.0.0.1:7777",
        "SANDKILN_AUTH_TOKEN": "omit this line for an unauthenticated local daemon"
      }
    }
  }
}
```

| Variable | Default | Meaning |
| --- | --- | --- |
| `SANDKILN_DAEMON_URL` | `http://127.0.0.1:7777` | Daemon address. |
| `SANDKILN_AUTH_TOKEN` | unset | Bearer token, if the daemon has one. Never echoed in any tool result. |
| `SANDKILN_MCP_MAX_OUTPUT_BYTES` | `32768` | Cap on text returned by one tool call. |
| `SANDKILN_MCP_MAX_SANDBOXES` | `8` | Most live sandboxes one session may hold. |
| `SANDKILN_MCP_KEEP_ON_EXIT` | `0` | `1`: snapshot leftover sandboxes at exit instead of destroying them. |

## Tools

| Tool | What it does |
| --- | --- |
| `sandbox_create` | Boot a sandbox (image, vcpu, memory, name, tags, env, egress mode and allowed CIDRs). |
| `sandbox_exec` | Run a command; returns stdout, stderr, exit code. Output is truncated head and tail. |
| `sandbox_write_file` | Write a text or base64 file (4 MiB limit). |
| `sandbox_read_file` | Read a file; text as UTF-8, binary as base64; bounded by `max_bytes`. |
| `sandbox_list_dir` | List a directory (at most 500 entries). |
| `sandbox_snapshot` | Freeze a sandbox into a snapshot; the sandbox id is retired. |
| `snapshot_list` | List held snapshots and their live forks. |
| `sandbox_resume` | Boot from a snapshot, consuming it. |
| `sandbox_fork` | Boot from a snapshot without consuming it (one live fork at a time). |
| `sandbox_stop` | Stop a sandbox; destroys by default (`keep=false`), `keep=true` snapshots first. |
| `sandbox_list` | List live sandboxes, flagging the ones this session created. |
| `daemon_status` | Version, uptime, counts, free tap devices, disk headroom, warnings. |

Resources: `sandkiln://sandboxes` (live sandboxes) and `sandkiln://status`
(daemon health).

Failures come back as `isError` results with `{"error": {"kind", "message",
"status", "hint", "details"}}`; `kind` is one of `invalid_request`, `unauthorized`,
`not_found`, `conflict`, `rate_limited`, `daemon_error`, `daemon_unreachable`,
`timeout`, `limit_reached`, `internal`. A command that exits nonzero is a normal
result, not an error.

## Safety notes

- Code runs in a microVM, not on your machine. Egress is open by default; pass
  `egress: "deny_all"` to `sandbox_create` for code that must stay offline.
- Sandboxes this session created are destroyed when the client disconnects
  (stdin closes) or the server gets `SIGTERM`/`SIGINT`. A `SIGKILL` of the
  server cannot be intercepted, so those sandboxes remain until the daemon's
  idle reaper or an operator removes them.
- Snapshots are durable by design and outlive the session.
- `sandbox_exec` has a timeout enforced inside the sandbox (default 60 s, max
  600 s, and never above the daemon's `exec_timeout_secs`). On expiry the
  command and its child processes are killed and the call returns a `timeout`
  error whose `details` hold the output captured so far, so nothing keeps
  running. Against a daemon or guest agent that predates this, the limit is
  not enforced in the guest.
- Any tool can act on any sandbox id on the daemon, not only the session's own.
  Point the server at a daemon whose contents you are willing to expose to the
  agent, and set `SANDKILN_AUTH_TOKEN` when the daemon is reachable by others.
- The server log goes to stderr only; stdout carries the protocol.

## Development

```bash
npm run build -w sandkiln       # the SDK, a workspace dependency, first
npm run typecheck -w sandkiln-mcp
npm test -w sandkiln-mcp        # unit tests, no daemon needed
SANDKILN_DAEMON_URL=http://127.0.0.1:7777 node packages/mcp/test/live-smoke.mjs
```

See [`AGENTS.md`](AGENTS.md) for the file map.
