---
title: "First sandbox: Python"
description: Boot and run your first sandbox with the Python SDK.
---

Not yet published to PyPI — install it from the repository:

```bash
pip install ./packages/python
```

```python
from sandkiln import Sandbox

sandbox = Sandbox.create(tags={"env": "ci"})
result = sandbox.run_command("python3", ["analyze.py"])
print(result.stdout, result.exit_code)
sandbox.stop()  # preserved as a resumable snapshot by default
```

Mirrors the JS/TS SDK exactly — same operations, Python-idiomatic naming (`run_command` not `runCommand`, snake_case fields), zero runtime dependencies (stdlib `urllib` only). Full method reference: [Python SDK](../../reference/python-sdk/).

## Next

- Give the sandbox a name so you can find it again later: [Named sandboxes & persistent stop](../../concepts/named-sandboxes/).
- Boot from your own image instead of the daemon's default: [Boot from a custom image](../../guides/custom-image/).
