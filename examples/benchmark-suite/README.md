# sandkiln benchmark suite

A reproducible, client-side benchmark for any `sandkilnd` daemon, written in
TypeScript on the `sandkiln` npm package. It measures what a caller feels
(wall time seen by the SDK), prints a results table and an environment block,
writes a JSON file you can diff, and has a `compare` mode that flags
regressions. The published numbers in [`docs/benchmarks.md`](../../docs/benchmarks.md)
are measured one layer lower (inside the daemon and VMM), so the two are
complementary, not interchangeable; that page's "Reproduce these numbers"
section says which figure maps to which metric.

## What it measures

| Metric | What the timer covers |
| --- | --- |
| `create_call` | `Sandbox.create()` returning |
| `first_command_after_create` | first `runCommand("true")` after create returned (the guest agent may not be listening yet) |
| `create_to_first_command` | the two above summed: the metric a caller feels |
| `exec_roundtrip` | one `runCommand("true")` on an open, warmed sandbox, `--exec-samples` times |
| `snapshot_take` | `sandbox.snapshot()` on a warmed sandbox |
| `fork_call`, `fork_first_command` | `Sandbox.fork()`, then the first command on the fork |
| `resume_call`, `resume_first_command` | `Sandbox.resume()`, then the first command on the resumed sandbox |
| `parallel_create_to_first_command_cN` | per-sandbox create-to-first-command with `N` creates in flight at once |
| `parallel_round_wall_cN`, `parallel_throughput_cN` | wall time of a round of `N` creates and sandboxes ready per second |

Every sandbox is tagged `benchmark_run=<unique id>`. After each benchmark (and
on Ctrl-C) the suite deletes exactly the sandboxes, snapshots and retired
checkpoints carrying that tag, so it leaves nothing behind and never touches
anyone else's.

## Run it

Needs Node 22.18 or newer (TypeScript runs directly, no build step) and a
running daemon: see [Self-hosting](https://sandkiln.mintlify.site/self-hosting/manual-setup).

```
cd examples/benchmark-suite
npm install
npm start -- --iterations 20 --json results/mine.json --markdown results/mine.md
```

From the repo root, `scripts/dev.sh bench-suite [args]` does the same (and runs
`npm install` on first use). To run it on the dev box, use
`scripts/remote.sh run scripts/dev.sh bench-suite ...`.

| Option | Default | Meaning |
| --- | --- | --- |
| `--iterations N` | 20 | rounds per benchmark (sample count for most metrics) |
| `--concurrency C` | 8 | simultaneous creates in the parallel benchmark |
| `--exec-samples N` | 200 | exec round trips on one sandbox |
| `--only a,b` | all | run a subset: `create`, `exec`, `snapshot`, `parallel` (`--list`) |
| `--base-url`, `--token` | env `SANDKILN_DAEMON_URL`, `SANDKILN_AUTH_TOKEN`, then `http://127.0.0.1:7777` | which daemon |
| `--hardware LABEL` | none | hardware description stored in the result |
| `--json FILE`, `--markdown FILE` | none | write the result files |
| `--allow-busy` | off | run even when the daemon has live sandboxes |

The suite refuses to start when `/status` shows live sandboxes, because other
work competes for CPU, disk and taps and skews every number. Check
`GET /status` yourself and rerun if a result looks noisy.

## Reading the results

The table gives median, p95, p99, min, max and sample count `n`. Percentiles
use linear interpolation, so with small `n` p95 and p99 sit close to the max
and say little; with `n = 20`, treat the median and min as the stable figures.
The environment block records the daemon version (from `GET /status`), the
guest kernel (from inside a sandbox) and, only when the daemon is on this
machine, the host kernel and CPU. For a remote daemon pass `--hardware`.

### Comparing two runs

```
npm run compare -- results/before.json results/after.json --threshold 20 --min-delta-ms 0.5
```

Medians are compared. A latency is a regression when it is worse by more than
`--threshold` percent and by more than `--min-delta-ms` (so 200us to 300us on
exec is not flagged); a throughput metric regresses when it drops. The command
exits 1 on any regression, so it can gate a script. `scripts/bench-report.sh`
does the same job against the daemon's own `/metrics`; this one works against
any daemon you can reach over HTTP.

## Adding a benchmark

1. Create `src/benchmarks/<name>.ts` exporting a `Benchmark`
   (`name`, `description`, `run(ctx)`). Use `ctx.create()` (it tags the
   sandbox), `timed()` for wall time and `ctx.record(metric, value)` for each
   sample. Clean up in `finally` with `ctx.cleanup()` or `sandbox.stop({ keep: false })`.
2. Add it to the array in `src/benchmarks/index.ts`. Run order is array order.
3. Throughput-style metrics pass `"ops/s", true` to `ctx.record` so `compare`
   knows higher is better.

Pure logic (statistics, formatting, comparison, argument parsing) lives in
`src/stats.ts`, `src/format.ts`, `src/compare.ts` and `src/args.ts` and is
covered by `npm test` with no daemon. `npm run typecheck` runs `tsc`.

## Caveats

- Small samples on a shared machine. A sample result is a data point, not a
  guarantee; rerun before concluding anything, and expect `snapshot_take` to
  move the most because it synchronously writes the full guest memory.
- These are client-side times: they include the HTTP hop and the SDK, unlike
  the vmm-layer benchmark. `exec_roundtrip` here is an HTTP request per
  command; the vmm number reuses an open vsock connection.
- `resume` retains the checkpoint in history by default, as a real caller's
  does; the suite deletes it afterwards.
- Run on an idle daemon. Anything else creating sandboxes during a run is
  noise, and the suite only checks for it at the start.

## Sample results

[`results/`](results/) holds a committed run (JSON and Markdown) labelled with
its date and hardware.
