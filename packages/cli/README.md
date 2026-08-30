# sandkiln-cli

Command-line interface for [sandkiln](https://github.com/SumitKumar-17/sandkiln)
— a compute primitive for safely running untrusted or AI-generated code
in hardware-isolated Firecracker microVMs. This package installs the
`kiln` command (the npm package is `sandkiln-cli`; `kiln` itself is
already taken by an unrelated package on npm).

It talks to a `sandkilnd` daemon over HTTP — you need one running
somewhere reachable (see the main repo's `SELF_HOSTING.md`; there is no
hosted service).

## Install

```
npm install -g sandkiln-cli
```

## Usage

```
kiln --base-url http://127.0.0.1:7777 --token $SANDKILN_AUTH_TOKEN sandbox create
kiln sandbox ls
kiln sandbox exec <id> python3 analyze.py
kiln sandbox read <id> /tmp/result.txt
kiln sandbox write <id> /tmp/input.json ./local-input.json
kiln sandbox preview <id> 3000
kiln sandbox snapshot <id>
kiln sandbox resume <snapshot-id>
kiln sandbox fork <snapshot-id>
kiln sandbox rm <id>
```

`--base-url`/`--token` default to the `SANDKILN_DAEMON_URL`/
`SANDKILN_AUTH_TOKEN` environment variables when omitted, same as the
JS/TS and Python SDKs.

Run `kiln --help` or `kiln sandbox --help` for the full command reference.
