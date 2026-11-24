# sandkiln dev server preview

A minimal reference example of live dev-server preview: start a server
inside an isolated sandkiln sandbox and reach it from a normal browser,
using the published `sandkiln` npm package's `Sandbox.previewUrl()` — not
a toy snippet.

## What it does

1. Creates a sandbox with `Sandbox.create()`.
2. Writes a small Python HTTP server into the sandbox with
   `sandbox.writeFile()` — a stand-in for a real dev server (`npm run
   dev`, `vite`, `python manage.py runserver`, ...).
3. Starts it in the background with `sandbox.runCommand()`.
4. Builds the URL to reach it with `sandbox.previewUrl(port)` — this does
   no network call itself; the daemon's `/sandboxes/:id/preview/:port`
   route proxies each request to the sandbox's own port lazily, on demand.
5. Prints that URL. Open it in a browser to see the running server.
6. On `Ctrl+C`, stops the sandbox with `sandbox.stop()`.

See `index.js` — it's the whole program.

## Requirements

A running `sandkilnd` daemon reachable from this machine *and* from
whatever browser opens the printed preview URL — see
[`SELF_HOSTING.md`](../../SELF_HOSTING.md) at the repo root for how to
stand one up. There is no hosted service.

## Run it

```
cd examples/dev-server-preview
npm install
node index.js
```

Then open the printed URL in a browser.

## Configuration

- `SANDKILN_DAEMON_URL` — base URL of the daemon. Defaults to
  `http://127.0.0.1:7777`. This also has to be the URL the printed
  preview link resolves to, since a browser hits the daemon directly.
- `SANDKILN_AUTH_TOKEN` — auth token, only needed if the daemon was
  started with one. When set, `previewUrl()` appends it to the printed
  link as a `?token=` query parameter automatically, since a browser
  navigating to the link can't attach an `Authorization` header.
