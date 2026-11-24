#!/usr/bin/env python3
"""Writes website/src/data/numbers.json: the only numbers the product page shows.

They are medians read from the committed benchmark results, never typed by
hand. `--check` fails if the committed file is stale (run by docs-check).

Usage: scripts/dev-tools/make-numbers.py [--check]
"""
import json
import statistics
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[2]
results = root / "examples/benchmark-suite/results"
out = root / "website/src/data/numbers.json"


def median(name: str, metric: str) -> int:
    run = json.loads((results / name).read_text())
    return round(statistics.median(run["metrics"][metric]["samples"]))


numbers = {
    "warm_create_to_first_command_ms": median("2026-11-24-warm-pool-on-paced-i9-13900K.json", "create_to_first_command"),
    "cold_create_to_first_command_ms": median("2026-11-24-warm-pool-off-paced-i9-13900K.json", "create_to_first_command"),
}
text = json.dumps(numbers, indent=2) + "\n"

if "--check" in sys.argv:
    if not out.exists() or out.read_text() != text:
        print(f"{out.relative_to(root)} is stale: run scripts/dev-tools/make-numbers.py")
        sys.exit(1)
    sys.exit(0)
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(text)
print(f"wrote {out.relative_to(root)}: {numbers}")
