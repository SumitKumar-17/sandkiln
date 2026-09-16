---
title: "First sandbox: CLI"
description: Boot and run your first sandbox with kiln.
---

```bash
npm install -g sandkiln-cli   # installs the `kiln` command
export SANDKILN_DAEMON_URL=http://127.0.0.1:7777
```

## A full session

Real, captured output from a running daemon:

```
$ kiln sandbox create --tag env=demo
9f1ad482-e9ee-4a0f-9a7c-7826f77b9cfc

$ kiln sandbox ls --tag env=demo
9f1ad482-e9ee-4a0f-9a7c-7826f77b9cfc  2026-09-15T20:54:58.000Z  -  env=demo

$ kiln sandbox exec 9f1ad482-e9ee-4a0f-9a7c-7826f77b9cfc -- python3 -c "print(21 * 2)"
42

$ kiln sandbox rm 9f1ad482-e9ee-4a0f-9a7c-7826f77b9cfc
9f1ad482-e9ee-4a0f-9a7c-7826f77b9cfc stopped and preserved as snapshot 6e49a676-6322-4a42-8608-26b4483320c1
```

`--base-url`/`--token` flags default to the `SANDKILN_DAEMON_URL`/`SANDKILN_AUTH_TOKEN` environment variables when omitted, same as both SDKs — see [Auth](../../concepts/auth/).

## `--` before an argument that looks like a flag

`exec`'s command and args are passed straight to `commander`'s own option parser, so an argument starting with `-` (a `python3 -c` script, `grep -v`, ...) needs `--` in front of it or `kiln` tries to parse it as its own flag:

```
$ kiln sandbox exec <id> python3 -c "print(1)"
error: unknown option '-c'

$ kiln sandbox exec <id> -- python3 -c "print(1)"
1
```

## A failed command doesn't throw — it exits non-zero

`exec` runs the command and reports whatever it actually did, exit code included; `cat`-ing a missing file isn't a CLI error, it's a normal result with a non-zero exit:

```
$ kiln sandbox exec <id> -- false; echo "exit: $?"
exit: 1
```

## Long-running commands: `exec-stream` and `logs`

`exec` waits for the command to finish and returns everything at once — fine for a short script, not for a build or a server you want to watch live. `exec-stream` starts the command in the background and follows its output as it happens:

```
$ kiln sandbox exec-stream <id> -- sh -c 'for i in 1 2 3; do echo tick $i; sleep 1; done'
session b479098a-80e6-485f-9709-15efde8f1078 started
tick 1
tick 2
tick 3
[process exited with code 0]
```

Ctrl+C detaches without stopping the command. Reattach any time, whether it's still running or long finished, with `kiln sandbox logs <id> <session-id>` — replays everything captured so far, then live-tails anything new. `kiln sandbox logs <id>` with no session id lists every streamed session on that sandbox instead. Not carried across `resume`/`fork`, and doesn't survive a daemon restart.

## Next

- Give the sandbox a name so you can find it again later: [Named sandboxes & persistent stop](../../concepts/named-sandboxes/).
- Boot from your own image instead of the daemon's default: [Boot from a custom image](../../guides/custom-image/).
- Full command reference: [CLI (kiln)](../../reference/cli/).
