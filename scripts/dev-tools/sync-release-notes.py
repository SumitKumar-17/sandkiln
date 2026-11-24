#!/usr/bin/env python3
"""Generates docs-site/release-notes.md from CHANGELOG.md, the single source.

The docs site cannot include a repository file, so the page is a generated
copy. Repository-relative links are rewritten so they work on the site:
`docs-site/<page>.mdx` becomes the docs route, other repository paths become
GitHub links. `--check` fails if the committed page is stale (docs-check).

Usage: scripts/dev-tools/sync-release-notes.py [--check]
"""
import re
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[2]
src = (root / "CHANGELOG.md").read_text()
out = root / "docs-site/release-notes.md"
GITHUB = "https://github.com/SumitKumar-17/sandkiln/blob/main/"

body = re.sub(r"^# Changelog\n+", "", src, count=1)


def link(m: re.Match) -> str:
    target = m.group(2)
    if re.match(r"(https?:|mailto:|#|/)", target):
        return m.group(0)
    page = re.match(r"docs-site/(.+?)\.mdx?(#.*)?$", target)
    if page:
        return f"[{m.group(1)}](/{page.group(1)}{page.group(2) or ''})"
    return f"[{m.group(1)}]({GITHUB}{target})"


body = re.sub(r"\[([^\]]+)\]\(([^)\s]+)\)", link, body)


def escape_mdx(text: str) -> str:
    """The docs site parses Markdown as MDX: a bare `<` or `{` outside code is a syntax error."""
    out, in_fence = [], False
    for line in text.split("\n"):
        if line.lstrip().startswith("```"):
            in_fence = not in_fence
            out.append(line)
            continue
        if in_fence:
            out.append(line)
            continue
        parts = re.split(r"(`[^`]*`)", line)  # keep inline code untouched
        out.append("".join(p if p.startswith("`") else p.replace("<", "&lt;").replace("{", "&#123;") for p in parts))
    return "\n".join(out)


body = escape_mdx(body)
text = (
    "---\n"
    "title: Changelog\n"
    "description: Every real change, nothing rounded up, across the daemon, the clients and the docs.\n"
    "---\n\n"
    + body
)
if "--check" in sys.argv:
    if not out.exists() or out.read_text() != text:
        print(f"{out.relative_to(root)} is stale: run scripts/dev-tools/sync-release-notes.py")
        sys.exit(1)
    sys.exit(0)
out.write_text(text)
print(f"wrote {out.relative_to(root)}")
