# sandkiln agent runner

A minimal AI-agent runner: an agent-generated script runs in an isolated sandbox and the runner reads back the result file it wrote. Uses the `sandkiln` PyPI package.

## What it does

1. Creates a sandbox with `Sandbox.create()`.
2. Writes `AGENT_SCRIPT` (a hardcoded stand-in for LLM output) with `write_file()`.
3. Runs it with `run_command()` and prints stdout, stderr and the exit code.
4. Reads the result file with `read_file()`, then calls `stop()`.

The whole program is `main.py`.

## Requirements

A running `sandkilnd` daemon ([`SELF_HOSTING.md`](../../SELF_HOSTING.md)). There is no hosted service.

## Run it

```
cd examples/agent-runner
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
python3 main.py
```

## Configuration

- `SANDKILN_DAEMON_URL`: daemon base URL (default `http://127.0.0.1:7777`).
- `SANDKILN_AUTH_TOKEN`: only needed if the daemon has a token.
