#!/usr/bin/env python3
"""Renders docs/images/product-session.txt as the product page's session image.

The session file is a real capture (commands and their actual output, run
with the `kiln` CLI against a live daemon). Lines starting with `$ ` are
typed commands, `# ` lines are typed shell comments, anything else is output.
Re-capture the session, edit the file, run this script. No dependencies.

Writes website/public/product.svg: the one place the session is shown (the product page). The session file stays the single source.

Usage: scripts/dev-tools/make-product-image.py [session.txt]
"""
import sys
from html import escape
from pathlib import Path

root = Path(__file__).resolve().parents[2]
src = Path(next((a for a in sys.argv[1:] if not a.startswith("--")), "")) if any(not a.startswith("--") for a in sys.argv[1:]) else root / "docs/images/product-session.txt"
outs = [root / "website/public/product.svg"]

GROUND, WELL, RULE = "#151515", "#1c1b1a", "#3a3835"
INK, MUTED, DIM, LINK = "#ece9e3", "#b9b5ad", "#8f8b84", "#8fa3e8"
FONT = "'JetBrains Mono', ui-monospace, 'SF Mono', Menlo, Consolas, monospace"
SIZE, STEP, PAD, TOP = 15, 26, 32, 64
CHAR = 9.0  # advance of a 15px monospace glyph, used only to size the canvas

lines = [l.rstrip("\n") for l in src.read_text().splitlines() if l.strip()]
width = max(760, int(max(len(l) for l in lines) * CHAR) + PAD * 2 + 8)
height = TOP + len(lines) * STEP + PAD - 4

rows = []
for i, line in enumerate(lines):
    y = TOP + i * STEP
    if line.startswith("$ "):
        rows.append(f'<text x="{PAD}" y="{y}"><tspan fill="{LINK}">$ </tspan><tspan fill="{INK}">{escape(line[2:])}</tspan></text>')
    elif line.startswith("# "):
        rows.append(f'<text x="{PAD}" y="{y}" fill="{DIM}">{escape(line)}</text>')
    else:
        rows.append(f'<text x="{PAD}" y="{y}" fill="{MUTED}">{escape(line)}</text>')

svg = f'''<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img" aria-label="A terminal session: create a sandbox, run a command in it, snapshot it and fork the snapshot">
<rect width="{width}" height="{height}" fill="{GROUND}"/>
<rect x="0.5" y="0.5" width="{width - 1}" height="{height - 1}" fill="none" stroke="{RULE}"/>
<text x="{PAD}" y="30" font-family="{FONT}" font-size="13" fill="{DIM}">sandkiln</text>
<line x1="0" y1="44" x2="{width}" y2="44" stroke="{RULE}"/>
<g font-family="{FONT}" font-size="{SIZE}">
{chr(10).join(rows)}
</g>
</svg>
'''
if "--check" in sys.argv:
    stale = [o for o in outs if not o.exists() or o.read_text() != svg]
    for o in stale:
        print(f"{o.relative_to(root)} is stale: run scripts/dev-tools/make-product-image.py")
    sys.exit(1 if stale else 0)
for out in outs:
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(svg)
    print(f"wrote {out.relative_to(root)} ({width}x{height})")
