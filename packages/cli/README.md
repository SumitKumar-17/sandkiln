# sandkiln-cli

The `kiln` command for [sandkiln](https://github.com/SumitKumar-17/sandkiln),
which runs untrusted or AI-generated code in hardware-isolated Firecracker
microVMs. The npm package is `sandkiln-cli` (the bare name `kiln` belongs to an
unrelated package). It talks to a `sandkilnd` daemon you run yourself; there is
no hosted service (see [self-hosting](https://sandkiln.mintlify.site/getting-started/self-hosting)).

## Install

```
npm install -g sandkiln-cli
```

## Usage

```
kiln sandbox create --name web-server
kiln sandbox exec <id> python3 analyze.py
kiln sandbox exec-stream <id> -- npm run build   # background; follow with `kiln sandbox logs`
kiln sandbox pty <id>                            # interactive shell
kiln sandbox write <id> /tmp/input.json ./local-input.json
kiln sandbox snapshot <id>
kiln sandbox resume <snapshot-id>
kiln sandbox rm <id>              # keeps a snapshot; --destroy removes everything

kiln image create <id> <path>     # register an already-built ext4 rootfs
kiln drive create 512             # new persistent drive, in MiB
kiln pool create <id> --vcpu 2 --mem 1024 --warm-count 3
```

`--base-url` and `--token` default to `SANDKILN_DAEMON_URL` and
`SANDKILN_AUTH_TOKEN`. `kiln --help` and `kiln sandbox --help` list every
subcommand and flag.

## Docs

Full reference: [CLI docs](https://sandkiln.mintlify.site/api-reference/cli/overview).
