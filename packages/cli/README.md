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
kiln sandbox create --name web-server --image node-lts-custom
kiln sandbox get-or-create --name web-server
kiln sandbox by-name web-server
kiln sandbox ls
kiln sandbox exec <id> python3 analyze.py
kiln sandbox exec-stream <id> -- npm run build   # background it, follow output live
kiln sandbox logs <id> <session-id>              # reattach any number of times
kiln sandbox pty <id>                            # a live, interactive shell
kiln sandbox read <id> /tmp/result.txt
kiln sandbox write <id> /tmp/input.json ./local-input.json
kiln sandbox chmod <id> /tmp/script.sh 755
kiln sandbox mkdir <id> /tmp/workdir
kiln sandbox ls-dir <id> /tmp
kiln sandbox preview <id> 3000
kiln sandbox tunnel <id> 8080 --local-port 3000  # the reverse of preview
kiln sandbox mount <id> my-bucket https://s3.example.com /mnt/bucket --access-key ... --secret-key ...
kiln sandbox mounts <id>
kiln sandbox unmount <id> <mount-id>
kiln sandbox snapshot <id>
kiln sandbox snapshots --source <id>
kiln sandbox resume <snapshot-id>
kiln sandbox fork <snapshot-id>
kiln sandbox rm <id>              # preserves state as a snapshot by default
kiln sandbox rm <id> --destroy    # opts into full destruction instead

kiln image create <id> <path>     # register an already-built ext4 rootfs
kiln image ls
kiln image rm <id>

kiln drive create 512             # a new empty persistent drive, in MiB
kiln drive ls
kiln drive rm <id>

kiln pool create <id> --vcpu 2 --mem 1024 --warm-count 3
kiln pool ls
kiln pool rm <id>
```

`sandbox` also has `chown`/`rename`/`cp`/`symlink`/`readlink`/`truncate`
for the rest of the filesystem surface — run `kiln sandbox --help` for
every subcommand and flag.

A sandbox can turn into a snapshot on its own, not just via `kiln sandbox
snapshot` or `kiln sandbox rm` — if the daemon operator has
`SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS` configured, an idle sandbox is
paused and snapshotted automatically. Run `kiln sandbox snapshots
--source <id>` to check whether a sandbox id that dropped out of `kiln
sandbox ls` turned into a snapshot, and find its id.

Sandboxes can carry a caller-given `--name`, unique among live sandboxes
and held snapshots. `kiln sandbox get-or-create --name <name>` resolves
to a live sandbox as-is, resumes a stopped (snapshotted) one, or creates
a fresh one — whichever applies — in a single race-safe call.

`--base-url`/`--token` default to the `SANDKILN_DAEMON_URL`/
`SANDKILN_AUTH_TOKEN` environment variables when omitted, same as the
JS/TS and Python SDKs.

Run `kiln --help` or `kiln sandbox --help` for the full command reference.
