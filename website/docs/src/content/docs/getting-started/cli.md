---
title: "First sandbox: CLI"
description: Boot and run your first sandbox with kiln.
---

```bash
npm install -g sandkiln-cli   # installs the `kiln` command
```

```bash
kiln --base-url http://127.0.0.1:7777 --token $SANDKILN_AUTH_TOKEN sandbox create
kiln sandbox ls
kiln sandbox exec <id> python3 analyze.py
kiln sandbox rm <id>   # preserves state as a snapshot by default
```

`--base-url`/`--token` default to the `SANDKILN_DAEMON_URL`/`SANDKILN_AUTH_TOKEN` environment variables when omitted, same as both SDKs — see [Auth](../concepts/auth/). Full command reference: [CLI (kiln)](../reference/cli/).

## Next

- Give the sandbox a name so you can find it again later: [Named sandboxes & persistent stop](../concepts/named-sandboxes/).
- Boot from your own image instead of the daemon's default: [Boot from a custom image](../guides/custom-image/).
