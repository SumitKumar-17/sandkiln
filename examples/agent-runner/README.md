# sandkiln agent runner

A minimal reference example of the shape a real AI-agent sandbox runner
takes: an agent-generated script runs inside an isolated sandbox, and the
runner reads back a result file the script produced. Uses the published
`sandkiln` PyPI package — not a toy snippet.

## What it does

1. Creates a sandbox with `Sandbox.create()`.
2. Writes a script into the sandbox with `sandbox.write_file()`. Here
   that script is `AGENT_SCRIPT` in `main.py` — a hardcoded stand-in for
   what an LLM agent would generate at runtime, clearly labeled as such.
   A real runner substitutes the agent's actual output for that string.
3. Runs it with `sandbox.run_command()` and prints stdout/stderr/exit
   code.
4. Reads back the result file the script wrote with
   `sandbox.read_file()`.
5. Stops the sandbox with `sandbox.stop()`.

See `main.py` — it's the whole program.

## Requirements

A running `sandkilnd` daemon reachable from this machine. There is no
hosted service — see [`SELF_HOSTING.md`](../../SELF_HOSTING.md) at the
repo root for how to stand one up.

## Run it

```
cd examples/agent-runner
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
python3 main.py
```

## Configuration

- `SANDKILN_DAEMON_URL` — base URL of the daemon. Defaults to
  `http://127.0.0.1:7777`.
- `SANDKILN_AUTH_TOKEN` — auth token, only needed if the daemon was
  started with one.
