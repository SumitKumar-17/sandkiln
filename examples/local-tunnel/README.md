# sandkiln local tunnel

Expose a service on this machine to code inside a sandbox. The reverse of dev-server preview.

## What it does

1. Starts a Node HTTP server on `127.0.0.1:9999` as a stand-in for a local service.
2. Creates a sandbox and opens `sandbox.tunnel(8080, { localPort: 9999 })`. Anything inside that connects to `127.0.0.1:8080` now reaches the local server.
3. Runs `curl http://127.0.0.1:8080/` inside the sandbox with `runCommand()` and checks the response matches what the local server sent.
4. Closes the tunnel and destroys the sandbox.

The whole program is `index.js`.

## Requirements

A running `sandkilnd` daemon ([`SELF_HOSTING.md`](../../SELF_HOSTING.md)). There is no hosted service.

## Run it

```
cd examples/local-tunnel
npm install
node index.js
# ends with: Match -- the sandbox really did reach a service running on this machine, not inside any VM.
```

## Configuration

- `SANDKILN_DAEMON_URL`: daemon base URL (default `http://127.0.0.1:7777`).
- `SANDKILN_AUTH_TOKEN`: only needed if the daemon has a token.

## Notes

- [Preview](../dev-server-preview/) reaches a server inside a sandbox from outside, one proxied request at a time. A tunnel goes the other way and holds a WebSocket open for its whole life, since the daemon relays every byte as soon as the guest connects.
