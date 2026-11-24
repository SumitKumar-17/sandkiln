# AGENTS.md — sandkiln-mcp (MCP server)

Read root `AGENTS.md` first.

## What this package is

`sandkiln-mcp`: a Model Context Protocol server (stdio transport) that
exposes sandkiln sandboxes as tools for AI agents. Like `kiln`, it is a thin
layer over `packages/sdk`: it adds argument schemas, output bounding, error
shaping and session cleanup, never daemon logic. New behavior goes in
`core/crates/daemon` and the SDK first.

## Files

- **`src/index.ts`** — entry only: parse config, build the server, connect
  `StdioServerTransport`, and run an idempotent `shutdown` on stdin close,
  `SIGTERM`, `SIGINT`. stdout is the protocol, so diagnostics use `console.error`.
- **`src/server.ts`** — wiring: builds the `McpServer`, the `SandboxSession`
  (its stopper treats a 404 as already stopped) and the `ToolContext`, and
  calls each `register*`.
- **`src/config.ts`** — `parseConfig(env)`: `SANDKILN_DAEMON_URL`,
  `SANDKILN_AUTH_TOKEN`, `SANDKILN_MCP_MAX_OUTPUT_BYTES`,
  `SANDKILN_MCP_MAX_SANDBOXES`, `SANDKILN_MCP_KEEP_ON_EXIT`. Pure.
- **`src/session.ts`** — `SandboxSession`: the owner of every sandbox this
  process created (create, resume, fork add; stop and snapshot remove), the
  per-session cap, and `cleanup(keep)`. One failed stop never blocks the rest.
- **`src/errors.ts`** — `ToolError` kinds, `ToolFailure` for this package's own
  guards, `toToolError` mapping `SandkilnApiError` status codes and fetch
  failures to a stable kind plus a hint.
- **`src/truncate.ts`** — `truncateText` (head and tail, UTF-8 safe) and
  `decodeFile` (text or base64, bounded). Everything returned to the model
  goes through these.
- **`src/exec-timeout.ts`** — `execTimeoutFailure`: a `SandkilnExecTimeoutError` (the command was killed in the guest at its limit) as a `timeout` tool error carrying the bounded partial output.
- **`src/resources.ts`** — `sandkiln://sandboxes`, `sandkiln://status`.
- **`src/tools/register.ts`** — `defineTool` (success and `isError` shaping,
  the only place errors are converted), `ToolContext`, `attach`.
- **`src/tools/{sandboxes,exec,files,snapshots,status}.ts`** — one file per
  tool group; each tool is schema, description, annotations and a short handler.

## Testing

`node:test`, no added dependency. `tsup` builds the pure helpers as their own
banner-less entries (`dist/config.js`, `truncate.js`, `errors.js`,
`session.js`, `exec-timeout.js`) so tests import them without starting the stdio
server, which `index.ts` does as a side effect. Add a new pure helper to
`tsup.config.ts`' second entry list. `npm test -w sandkiln-mcp` builds first.

`test/live-smoke.mjs` is not a unit test: it drives the built server over
stdio with the official MCP client against a real daemon (create, exec, files,
snapshot, resume, errors, cleanup on disconnect). Run it on the dev box after
any change to a tool; typecheck and unit tests do not prove a tool works.

## Building and verifying

```
npm run build -w sandkiln          # SDK dist first, a workspace dependency
npm run typecheck -w sandkiln-mcp
npm run build -w sandkiln-mcp
npm run test -w sandkiln-mcp
```

"Module 'sandkiln' has no exported member" means the SDK `dist/` is stale.

## Non-obvious things

- `@modelcontextprotocol/sdk` and `zod` are pinned to exact versions. Use
  `registerTool`/`registerResource`; the older `tool()`/`resource()` are deprecated.
- Tool arguments are snake_case (the MCP convention models see); the SDK is camelCase.
- `sandbox_snapshot` retires the sandbox id, so the session forgets it.
  `sandbox_resume` consumes the snapshot, `sandbox_fork` does not.
- `sandbox_exec`'s timeout is the daemon's per-request `timeout_secs`: the guest
  kills the command's process group, so a timeout leaves nothing running. There
  is no client-side timer any more.
- A `SIGKILL` of the server cannot clean up; that is documented, not fixable here.
- Not yet wired to a publish workflow (no `publish-mcp.yml`); CI builds and tests it.
