# sandkiln code playground

A minimal reference example: run arbitrary code inside an isolated
sandkiln sandbox and see the result. This is what "a sandbox as the
execution backend for code" looks like end to end, using the published
`sandkiln` npm package — not a toy snippet.

## What it does

1. Reads code from a file argument, or from stdin if no file is given.
2. Creates a sandbox with `Sandbox.create()`.
3. Writes the code into the sandbox with `sandbox.writeFile()`.
4. Runs it with `sandbox.runCommand()` — `python3`, `node`, or `bash`,
   picked from the file extension (or `--lang py|js|sh`).
5. Prints stdout, stderr, and the exit code.
6. Destroys the sandbox with `sandbox.stop({ keep: false })`. A plain
   `stop()` would snapshot it and keep the state, which a one-shot run has
   no use for.

See `index.js` — it's the whole program.

## Requirements

A running `sandkilnd` daemon reachable from this machine. There is no
hosted service — see [`SELF_HOSTING.md`](../../SELF_HOSTING.md) at the
repo root for how to stand one up.

The guest image decides which languages work: the default test image has
`python3` and `bash` but **no `node`**, so `--lang js` fails there with an
"unexpected agent response: No such file or directory" error. The
production image has both. `hello.py` and `--lang py|sh` run on any image
with Python and bash.

## Run it

```
cd examples/code-playground
npm install
node index.js ./hello.py
```

or pipe code in directly:

```
echo 'print("hi from sandkiln")' | node index.js --lang py
```

## Configuration

- `SANDKILN_DAEMON_URL` — base URL of the daemon. Defaults to
  `http://127.0.0.1:7777`.
- `SANDKILN_AUTH_TOKEN` — auth token, only needed if the daemon was
  started with one.
