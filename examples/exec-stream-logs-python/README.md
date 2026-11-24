# sandkiln exec-stream logs (Python)

The Python counterpart to [`exec-stream-logs`](../exec-stream-logs/): start a
command running detached inside a sandbox, watch it live, then re-attach to
the same session *after it finished* and get the whole log back in
milliseconds. Uses `Sandbox.exec_stream()`, `.attach_logs()` and
`.list_exec_streams()`.

## What it does

1. Creates a sandbox with `Sandbox.create()`.
2. Starts a ~6-second, multi-line command with `sandbox.exec_stream()`,
   which returns a session id **immediately** (unlike `run_command()`).
3. Attaches with `sandbox.attach_logs(session_id)`, a `LogStream`, and prints
   each message as it arrives; the last is the daemon's
   `[process exited with code 0]` notice and sets `logs.exit_code`.
4. Lists the session with `sandbox.list_exec_streams()` (now with its exit
   code).
5. Re-attaches and calls `read_all()`: the same log, in milliseconds,
   because the daemon buffers output independently of any connection.
6. Stops the sandbox with `sandbox.stop(keep=False)`.

See `main.py`, it is the whole program.

## Requirements

A running `sandkilnd` daemon reachable from this machine (see
[`SELF_HOSTING.md`](../../SELF_HOSTING.md)). Python 3.9+ and `sandkiln` 0.6.0
or newer; there are no other dependencies, the WebSocket client is part of
the package.

## Run it

```
cd examples/exec-stream-logs-python
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
python3 main.py
```

`SANDKILN_DAEMON_URL` (default `http://127.0.0.1:7777`) and
`SANDKILN_AUTH_TOKEN` are read from the environment.
