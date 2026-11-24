# sandkiln code playground

Run arbitrary code in an isolated sandbox and print the result, using the `sandkiln` npm package.

## What it does

1. Reads code from a file argument or stdin.
2. Creates a sandbox and writes the code in with `writeFile()`.
3. Runs it with `runCommand()` as `python3`, `node` or `bash`, chosen by file extension or `--lang py|js|sh`.
4. Prints stdout, stderr and the exit code, then calls `stop()`.

The whole program is `index.js`.

## Requirements

A running `sandkilnd` daemon ([`SELF_HOSTING.md`](../../SELF_HOSTING.md)). There is no hosted service.

## Run it

```
cd examples/code-playground
npm install
node index.js ./hello.py
# or: echo 'print("hi")' | node index.js --lang py
```

## Configuration

- `SANDKILN_DAEMON_URL`: daemon base URL (default `http://127.0.0.1:7777`).
- `SANDKILN_AUTH_TOKEN`: only needed if the daemon has a token.
