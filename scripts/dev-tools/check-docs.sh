#!/usr/bin/env bash
# Documentation checks (see docs/documentation-plan.md): relative links
# resolve, docs-site pages and docs.json agree, no banned wording, size
# budgets hold. Runs locally and needs only python3.
#
# Usage: scripts/dev-tools/check-docs.sh
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

python3 - <<'PY'
import json, re, subprocess, sys
from pathlib import Path

root = Path(".")
tracked = subprocess.check_output(["git", "ls-files"], text=True).split("\n")
files = [Path(f) for f in tracked if f and Path(f).exists()]
errors = []

# 1. Relative Markdown links resolve (skip code fences and external or anchor-only links).
link = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
for f in files:
    if f.suffix != ".md" or f.parts[0] in {"website", "node_modules"} or ".claude" in f.parts:
        continue
    in_fence = False
    for n, line in enumerate(f.read_text().splitlines(), 1):
        if line.lstrip().startswith("```"):
            in_fence = not in_fence
        if in_fence:
            continue
        for target in link.findall(line):
            if re.match(r"(https?:|mailto:|#)", target):
                continue
            path = target.split("#")[0]
            if not path:
                continue
            if path.startswith("/"):
                # Absolute links are routes on the docs site (Mintlify).
                base = Path("docs-site") / path.lstrip("/")
                found = any(base.with_suffix(ext).exists() for ext in (".mdx", ".md")) or (base / "index.mdx").exists() or base.is_file()
                if f.parts[0] == "docs-site" and not found:
                    errors.append(f"{f}:{n}: broken docs route {target}")
            elif not (f.parent / path).exists():
                errors.append(f"{f}:{n}: broken link {target}")

# 2. docs-site pages and docs.json agree.
nav = json.loads(Path("docs-site/docs.json").read_text())
listed = set()
def walk(node):
    if isinstance(node, str):
        listed.add(node)
    elif isinstance(node, list):
        for x in node:
            walk(x)
    elif isinstance(node, dict):
        for k in ("pages", "tabs", "groups", "navigation"):
            if k in node:
                walk(node[k])
walk(nav.get("navigation", nav))
pages = {str(p.relative_to("docs-site").with_suffix("")) for ext in ("*.mdx", "*.md") for p in Path("docs-site").rglob(ext) if p.name != "AGENTS.md"}
for p in sorted(pages - listed):
    errors.append(f"docs-site/{p} is not in docs.json")
for p in sorted(x for x in listed - pages if "/" in x or x == "index"):
    if not (Path(f"docs-site/{p}.mdx").exists() or Path(f"docs-site/{p}.md").exists()):
        errors.append(f"docs.json lists missing page {p}")

# 3. Banned wording.
banned = [(re.compile(r"\bPhase [0-9]"), "'Phase N' wording"),
          (re.compile(r"ROADMAP\.md'?s [\"'A-Z]"), "pointer to a removed ROADMAP.md section"),
          (re.compile(r"SELF_HOSTING\.md'?s [\"'a-z]"), "pointer to a removed SELF_HOSTING.md section")]
for f in files:
    if f.suffix not in {".md", ".mdx", ".rs", ".sh", ".ts", ".py"} or f.parts[0] in {"node_modules"} or ".claude" in f.parts or f.name == "check-docs.sh":
        continue
    if f.name == "CHANGELOG.md" or f.parts[:2] == ("website", "src") and "content" in f.parts:
        continue
    for n, line in enumerate(f.read_text(errors="ignore").splitlines(), 1):
        for rx, why in banned:
            if rx.search(line):
                errors.append(f"{f}:{n}: {why}")

# 4. Size budgets.
budgets = {"README.md": 110, "ROADMAP.md": 160}
for f in Path("docs/design-notes").glob("*.md"):
    budgets[str(f)] = 300
for f in files:
    if f.name == "AGENTS.md":
        budgets[str(f)] = 300
for name, limit in budgets.items():
    p = Path(name)
    if p.exists():
        n = len(p.read_text().splitlines())
        if n > limit:
            errors.append(f"{name}: {n} lines exceeds the {limit}-line budget")

# 5. Generated artifacts (numbers, charts, product image) match their sources.
for script in ("make-product-image", "make-logo", "make-benchmark-page", "sync-release-notes"):
    r = subprocess.run([sys.executable, f"scripts/dev-tools/{script}.py", "--check"], capture_output=True, text=True)
    if r.returncode != 0:
        errors.append((r.stdout + r.stderr).strip() or f"{script} --check failed")

for e in errors:
    print("FAIL", e)
print(f"docs-check: {len(errors)} problem(s)")
sys.exit(1 if errors else 0)
PY
