#!/usr/bin/env python3
"""Renders docs/images/benchmarks.svg from the committed benchmark results.

Every bar is a median read from a file under examples/benchmark-suite/results/,
so the chart can only show recorded runs. Re-run the suite, commit the new
result files, update RESULTS below if their names change, run this script.
No dependencies.
"""
import json
import sys
import math
import statistics
from html import escape
from pathlib import Path

root = Path(__file__).resolve().parents[2]
results = root / "examples/benchmark-suite/results"
RUNS = {
    "warm": json.loads((results / "2026-11-24-warm-pool-on-paced-i9-13900K.json").read_text()),
    "cold": json.loads((results / "2026-11-24-warm-pool-off-paced-i9-13900K.json").read_text()),
    "base": json.loads((results / "2026-11-24-i9-13900K.json").read_text()),
}

def median(run: str, metric: str) -> float:
    return statistics.median(RUNS[run]["metrics"][metric]["samples"])

# (label, value in ms, highlighted)
BARS = [
    ("create to first command, warm pool", median("warm", "create_to_first_command"), True),
    ("create to first command, cold", median("cold", "create_to_first_command"), False),
    ("resume a snapshot", median("base", "resume_call"), False),
    ("first command after a resume", median("base", "resume_first_command"), False),
    ("command in a running sandbox", median("base", "exec_roundtrip"), False),
]

GROUND, RULE, INK, MUTED, DIM, LINK = "#151515", "#3a3835", "#ece9e3", "#b9b5ad", "#8f8b84", "#8fa3e8"
FONT = "'JetBrains Mono', ui-monospace, 'SF Mono', Menlo, Consolas, monospace"
W, PAD, LABEL_W, VALUE_W = 760, 32, 250, 80
ROW, TOP = 40, 70
TRACK_X, TRACK_W = PAD + LABEL_W, W - PAD * 2 - LABEL_W - VALUE_W
LO, HI = 1.0, 1000.0  # log axis, ms
H = TOP + len(BARS) * ROW + 56

def x_of(ms: float) -> float:
    return TRACK_X + TRACK_W * (math.log10(max(ms, LO)) - math.log10(LO)) / (math.log10(HI) - math.log10(LO))

def fmt(ms: float) -> str:
    return f"{ms:.1f} ms" if ms < 100 else f"{ms:.0f} ms"

parts = [
    f'<rect width="{W}" height="{H}" fill="{GROUND}"/>',
    f'<rect x="0.5" y="0.5" width="{W - 1}" height="{H - 1}" fill="none" stroke="{RULE}"/>',
    f'<text x="{PAD}" y="34" font-size="13" fill="{DIM}">measured latency, medians, log scale (lower is better)</text>',
]
for tick in (1, 10, 100, 1000):
    x = x_of(tick)
    parts.append(f'<line x1="{x:.1f}" y1="{TOP - 14}" x2="{x:.1f}" y2="{TOP + len(BARS) * ROW - 8}" stroke="{RULE}"/>')
    parts.append(f'<text x="{x:.1f}" y="{TOP + len(BARS) * ROW + 12}" font-size="12" fill="{DIM}" text-anchor="middle">{tick} ms</text>')
for i, (label, ms, hot) in enumerate(BARS):
    y = TOP + i * ROW
    color = LINK if hot else MUTED
    parts.append(f'<text x="{PAD}" y="{y + 12}" font-size="13" fill="{INK if hot else MUTED}">{escape(label)}</text>')
    parts.append(f'<rect x="{TRACK_X}" y="{y}" width="{x_of(ms) - TRACK_X:.1f}" height="16" fill="{color}"/>')
    parts.append(f'<text x="{W - PAD}" y="{y + 13}" font-size="13" fill="{INK}" text-anchor="end">{fmt(ms)}</text>')
parts.append(f'<text x="{PAD}" y="{H - 14}" font-size="11" fill="{DIM}">one shared i9-13900K box, n=20 per metric; sources: examples/benchmark-suite/results/</text>')

svg = (
    f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" role="img" '
    f'aria-label="Bar chart on a log scale: create to first command takes {fmt(BARS[0][1])} with a warm pool and {fmt(BARS[1][1])} cold">\n'
    f'<g font-family="{FONT}">\n' + "\n".join(parts) + "\n</g>\n</svg>\n"
)
targets = (root / "docs/images/benchmarks.svg",)
if "--check" in sys.argv:
    stale = [o for o in targets if not o.exists() or o.read_text() != svg]
    for o in stale:
        print(f"{o.relative_to(root)} is stale: run scripts/dev-tools/make-benchmark-image.py")
    sys.exit(1 if stale else 0)
for out in targets:
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(svg)
    print(f"wrote {out.relative_to(root)}")
for label, ms, _ in BARS:
    print(f"  {label}: {fmt(ms)}")
