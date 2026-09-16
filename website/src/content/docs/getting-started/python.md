---
title: "First sandbox: Python"
description: Boot and run your first sandbox with the Python SDK.
---

```bash
pip install sandkiln
```

```python
from sandkiln import Sandbox

sandbox = Sandbox.create(tags={"env": "demo"})
print("created:", sandbox.id)
# created: a5d040b0-88a0-40a3-b09a-cf9ae45bd3cf

result = sandbox.run_command("python3", ["-c", "print(21 * 2)"])
print(result.stdout.strip(), result.exit_code)
# 42 0

sandbox.stop()  # preserved as a resumable snapshot by default
```

Same operations as the JS/TS SDK, Python-idiomatic naming (`run_command` not `runCommand`, snake_case fields), zero runtime dependencies (stdlib `urllib` only). One real gap, not just a naming difference: streamed background exec (`exec_stream`/attaching to its live log) and `pty` aren't implemented in this SDK yet — use the [CLI](../cli/#long-running-commands-exec-stream-and-logs) or the [daemon HTTP API](../../reference/http-api/) directly for those today.

## A failed command doesn't raise — it returns a non-zero exit code

`run_command` reports whatever the command actually did; `cat`-ing a missing file isn't a Python exception, it's a normal `ExecResult` with `exit_code: 1`:

```python
failed = sandbox.run_command("cat", ["/does/not/exist"])
print("exit code:", failed.exit_code, "stderr:", failed.stderr.strip())
# exit code: 1 stderr: cat: /does/not/exist: No such file or directory
```

`SandkilnApiError` is reserved for the request itself failing — a bad sandbox id, an unreachable daemon, a validation error. Calling any method after `stop()` (without `resume()`-ing it first) is the everyday way to see one:

```python
from sandkiln import SandkilnApiError

try:
    sandbox.run_command("echo", ["hi"])
except SandkilnApiError as err:
    print(err)
    # sandbox not found: a5d040b0-88a0-40a3-b09a-cf9ae45bd3cf (status 404)
```

## Next

- Give the sandbox a name so you can find it again later: [Named sandboxes & persistent stop](../../concepts/named-sandboxes/).
- Boot from your own image instead of the daemon's default: [Boot from a custom image](../../guides/custom-image/).
- Full method reference: [Python SDK](../../reference/python-sdk/).
