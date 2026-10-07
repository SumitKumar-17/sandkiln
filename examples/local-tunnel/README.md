# sandkiln local tunnel

A minimal reference example of the local tunnel feature: expose a service
running on *this* machine to code running inside a sandbox — the exact
reverse of dev-server preview, which exposes a port inside the sandbox
outward.

**Uses `../../packages/sdk` directly, not a published npm version** —
`Sandbox.tunnel()` is new and hasn't shipped in a release yet. Switch
`package.json`'s `sandkiln` dependency back to a version range once it
has (same convention this directory already follows elsewhere; check
`CHANGELOG.md` for whether it's landed).

## What it does

1. Starts a plain Node HTTP server on `127.0.0.1:9999` — a stand-in for a
   real local service (a dev server, a database admin UI, an internal
   API) that only exists on this machine, not inside any sandbox.
2. Creates a sandbox with `Sandbox.create()`.
3. Opens a tunnel with `sandbox.tunnel(8080, { localPort: 9999 })` —
   after this, anything *inside* the sandbox that connects to
   `127.0.0.1:8080` actually reaches the real server from step 1.
4. Runs `curl http://127.0.0.1:8080/` **inside** the sandbox via
   `runCommand()` and prints what came back.
5. Confirms it's byte-for-byte what the local server actually sent —
   proof the sandbox reached a real service on this machine, not
   something running inside the VM.
6. Closes the tunnel and destroys the sandbox.

See `index.js` — it's the whole program.

## Requirements

A running `sandkilnd` daemon — see [`SELF_HOSTING.md`](../../SELF_HOSTING.md)
at the repo root for how to stand one up. There is no hosted service.

## Run it

```
cd examples/local-tunnel
npm install
node index.js
```

Expected output ends with:

```
Match -- the sandbox really did reach a service running on this machine, not inside any VM.
```

## Configuration

- `SANDKILN_DAEMON_URL` — base URL of the daemon. Defaults to
  `http://127.0.0.1:7777`.
- `SANDKILN_AUTH_TOKEN` — auth token, only needed if the daemon was
  started with one.

## How this is different from preview

[Dev-server preview](../dev-server-preview/) (`previewUrl()`) is for
reaching a server that's already running *inside* a sandbox, from
outside it (typically a browser). This is the opposite direction: a
sandbox reaching a server that's running *outside* it, on the machine
running this SDK. Preview needs no persistent connection (the daemon
proxies each request lazily); a tunnel holds a WebSocket open for its
whole life, since the daemon has to relay every byte the guest sends the
moment it accepts a connection, not just answer discrete requests.
