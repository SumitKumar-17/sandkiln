# sandkiln dev server preview

Start a server inside a sandbox and reach it from a normal browser with `Sandbox.previewUrl()`.

## What it does

1. Creates a sandbox and writes a small Python HTTP server into it (a stand-in for `npm run dev`, `vite` and the like).
2. Starts it in the background with `runCommand()`.
3. Builds the URL with `previewUrl(port)`. No network call happens here; the daemon's `/sandboxes/:id/preview/:port` route proxies requests on demand.
4. Prints the URL. Open it in a browser; `Ctrl+C` stops the sandbox.

The whole program is `index.js`.

## Requirements

A running `sandkilnd` daemon ([`SELF_HOSTING.md`](../../SELF_HOSTING.md)). There is no hosted service. The daemon must also be reachable from whatever browser opens the printed URL.

## Run it

```
cd examples/dev-server-preview
npm install
node index.js
```

## Configuration

- `SANDKILN_DAEMON_URL`: daemon base URL (default `http://127.0.0.1:7777`).
- `SANDKILN_AUTH_TOKEN`: only needed if the daemon has a token.

## Notes

- With an auth token set, `previewUrl()` appends it as `?token=` because a browser cannot attach an `Authorization` header.
