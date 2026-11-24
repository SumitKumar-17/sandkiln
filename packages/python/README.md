# sandkiln

Python client for [sandkiln](https://github.com/SumitKumar-17/sandkiln), which
runs untrusted or AI-generated code in hardware-isolated Firecracker microVMs.
It talks to a `sandkilnd` daemon you run yourself; there is no hosted service
(see [self-hosting](https://sandkiln.mintlify.site/getting-started/self-hosting)).
No runtime dependencies beyond the standard library.

## Install

```
pip install sandkiln
```

## Usage

```python
from sandkiln import Sandbox

sandbox = Sandbox.create(tags={"env": "ci"})

result = sandbox.run_command("python3", ["analyze.py"])
print(result.stdout, result.exit_code)

sandbox.write_file("/tmp/config.json", '{"ok": true}')
data = sandbox.read_file("/tmp/config.json")

sandbox.stop()  # keeps a resumable snapshot by default
```

`AsyncSandbox`, `AsyncDrive`, `AsyncImage` and `AsyncPool` are asyncio mirrors
with the same method names; they are separate classes so a sync call can never
silently block an event loop. `Drive`, `Image` and `Pool` cover persistent
drives, registered images and pre-warmed pools.

## Configuration

- Daemon URL: `base_url` argument or `SANDKILN_DAEMON_URL` (default `http://127.0.0.1:7777`).
- Auth: `auth_token` argument or `SANDKILN_AUTH_TOKEN`. Omit for an unauthenticated local daemon.

## Docs

Full method reference: [Python SDK docs](https://sandkiln.mintlify.site/api-reference/python-sdk/overview).
Streamed exec and `pty` sessions have no wrapper here yet; use the JS/TS SDK or
the `kiln` CLI. See the [roadmap](https://github.com/SumitKumar-17/sandkiln/blob/main/ROADMAP.md).

## License

MIT
