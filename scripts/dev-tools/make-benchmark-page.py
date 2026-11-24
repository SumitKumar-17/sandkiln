#!/usr/bin/env python3
"""Writes docs-site/performance/current-numbers.md and the README's speed table from the recorded benchmark results.

Every figure on the page is read from a file under
examples/benchmark-suite/results/ (the newest one for each scenario), so the
page can only show recorded runs and nothing is typed by hand. The README
block between the `bench:start` and `bench:end` markers is regenerated the same
way. `--check` fails if either is stale (run by docs-check).

Usage: scripts/dev-tools/make-benchmark-page.py [--check]
"""
import json
import statistics
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[2]
results = root / "examples/benchmark-suite/results"
out = root / "docs-site/performance/current-numbers.md"

# (heading, intro, label that selects the newest file, label to exclude or None, metrics shown in order)
SCENARIOS = [
    (
        "Create to first command, warm pool, one create every 4 seconds",
        "The default warm pool keeps instances ready, and the pause between creates lets it refill: the steady state of a real workload.",
        "warm-pool-on-paced", None,
        ["create_call", "first_command_after_create", "create_to_first_command"],
    ),
    (
        "Create to first command, warm pool, one create every 1.5 seconds",
        "Shorter gaps: the pool refills as soon as a claim drains it.",
        "warm-pool-on-pause1500-gaps", None,
        ["create_to_first_command"],
    ),
    (
        "Create to first command, warm pool, back to back",
        "Twenty creates in a row outrun a pool of two, so most of them boot cold.",
        "warm-pool-on-burst", None,
        ["create_to_first_command"],
    ),
    (
        "Create to first command, cold (pool disabled)",
        "Every create boots a microVM.",
        "warm-pool-off-paced", None,
        ["create_call", "first_command_after_create", "create_to_first_command"],
    ),
    (
        "Snapshots, resume, fork, commands and parallel creates",
        "Measured with the default pool's replenishment idle.",
        "-i9-13900K.json", "warm-pool",
        ["exec_roundtrip", "snapshot_take", "resume_call", "resume_first_command", "fork_call", "fork_first_command",
         "parallel_create_to_first_command_c8", "parallel_throughput_c8"],
    ),
]
LABELS = {
    "create_call": "create call returns",
    "first_command_after_create": "first command after create returns",
    "create_to_first_command": "create to first command",
    "exec_roundtrip": "run a command in a running sandbox",
    "snapshot_take": "take a snapshot",
    "resume_call": "resume a snapshot",
    "resume_first_command": "first command after a resume",
    "fork_call": "fork a snapshot",
    "fork_first_command": "first command after a fork",
    "parallel_create_to_first_command_c8": "create to first command, 8 at once",
    "parallel_throughput_c8": "creates per second, 8 at once",
}


def newest(label: str, exclude):
    files = sorted(
        p for p in results.glob("*.json") if label in p.name and not (exclude and exclude in p.name)
    )
    if not files:
        raise SystemExit(f"no benchmark result matching {label!r}")
    return files[-1]


def pct(samples, p):
    s = sorted(samples)
    k = (len(s) - 1) * p / 100
    lo = int(k)
    hi = min(lo + 1, len(s) - 1)
    return s[lo] + (s[hi] - s[lo]) * (k - lo)


def ms(v: float) -> str:
    return f"{v:.2f} ms" if v < 10 else f"{v:.1f} ms" if v < 100 else f"{v:.0f} ms"


lines = [
    "---",
    'title: "Current numbers"',
    'description: "Recorded benchmark results: create, first command, resume, fork, snapshot and parallel creates, with the conditions each was measured under."',
    "---",
    "",
    "These figures are medians from recorded runs of the [benchmark suite](/engineering/32-a-benchmark-you-can-rerun), read from the result files in the repository when this page is generated. They come from one shared development machine, so treat them as directional, and rerun the suite on your own hardware with `scripts/dev.sh bench-suite`. What each number does and does not include is in [what a start-latency number means](/engineering/29-what-a-start-latency-number-means).",
    "",
]
env_done = False
for heading, intro, label, exclude, metrics in SCENARIOS:
    path = newest(label, exclude)
    run = json.loads(path.read_text())
    env = run["environment"]
    lines += [f"## {heading}", "", intro, ""]
    lines += ["| Measurement | Median | p95 | Min | Max | Runs |", "| --- | ---: | ---: | ---: | ---: | ---: |"]
    for m in metrics:
        entry = run["metrics"][m]
        s = entry["samples"]
        if entry.get("unit") == "ms" or entry.get("unit") is None:
            lines.append(f"| {LABELS[m]} | {ms(statistics.median(s))} | {ms(pct(s, 95))} | {ms(min(s))} | {ms(max(s))} | {len(s)} |")
        else:
            lines.append(f"| {LABELS[m]} | {statistics.median(s):.1f} {entry['unit']} | {pct(s, 95):.1f} | {min(s):.1f} | {max(s):.1f} | {len(s)} |")
    lines += ["", f"Run `{path.name}`, {env['date'][:10]}, daemon {env['daemonVersion']}, guest kernel {env['guestKernel']}.", ""]
    if not env_done:
        host = f"{env['hostCpu']}, host kernel {env['hostKernel']}"
        env_done = True
lines += ["## Machine", "", f"{host}. One shared box that also runs other work, so absolute figures vary run to run.", ""]
text = "\n".join(lines)

# README block: the headline figures, same files.
def med(label, exclude, metric):
    run = json.loads(newest(label, exclude).read_text())
    return statistics.median(run["metrics"][metric]["samples"])

readme_rows = [
    ("warm", med("warm-pool-on-paced", None, "create_to_first_command")),
    ("cold", med("warm-pool-off-paced", None, "create_to_first_command")),
    ("resume", med("-i9-13900K.json", "warm-pool", "resume_call")),
    ("1st cmd", med("-i9-13900K.json", "warm-pool", "resume_first_command")),
    ("cmd", med("-i9-13900K.json", "warm-pool", "exec_roundtrip")),
]
# A Mermaid chart: GitHub draws it in colour from the code, so it is text in the
# repo (no image to regenerate or let go stale). Values are in the bar labels
# because the axis is linear and the smallest bars are slivers.
labels = ", ".join(f'"{name} {v:.3g}"' for name, v in readme_rows)
values = ", ".join(f"{v:.1f}" for _, v in readme_rows)
top = int(max(v for _, v in readme_rows) // 50 * 50 + 50)
block = "\n".join(
    [
        "<!-- bench:start (generated by scripts/dev-tools/make-benchmark-page.py) -->",
        "```mermaid",
        '%%{init: {"theme": "base", "themeVariables": {"xyChart": {"plotColorPalette": "#6f84d6"}}}}%%',
        "xychart-beta",
        '    title "Median latency in milliseconds, lower is better"',
        f"    x-axis [{labels}]",
        f'    y-axis "ms" 0 --> {top}',
        f"    bar [{values}]",
        "```",
        "",
        "*warm* and *cold*: create to first command with a warm instance ready, and with a cold boot. *resume*: the resume call. *1st cmd*: the first command after a resume. *cmd*: a command in a running sandbox.",
        "<!-- bench:end -->",
    ]
)
readme = root / "README.md"
old = readme.read_text()
start, end = old.index("<!-- bench:start"), old.index("<!-- bench:end -->") + len("<!-- bench:end -->")
new_readme = old[:start] + block + old[end:]

if "--check" in sys.argv:
    stale = [p for p, want in ((out, text), (readme, new_readme)) if not p.exists() or p.read_text() != want]
    for p in stale:
        print(f"{p.relative_to(root)} is stale: run scripts/dev-tools/make-benchmark-page.py")
    sys.exit(1 if stale else 0)
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(text)
readme.write_text(new_readme)
print(f"wrote {out.relative_to(root)} and the README speed table")
