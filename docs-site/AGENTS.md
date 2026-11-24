# AGENTS.md — docs-site/

Read root `AGENTS.md` first. This is the full docs site: a Mintlify
project, independent of `website/` (marketing pages only). Deploys
automatically from `main` via Mintlify's GitHub integration to
https://sandkiln.mintlify.site/ — no build step in this repo, no
CI workflow to maintain.

## Structure

`docs.json` is the nav and theme config. Navigation is five top-level
tabs, each its own groups:

- **Docs**: Getting Started, Self-Hosting (manual setup, jailer,
  configuration, operations and security, troubleshooting), Core Concepts,
  Guides.
- **API Reference**: HTTP API, JS/TS SDK, Python SDK, CLI, MCP server, each
  split into `overview` + one page per resource under `api-reference/<section>/`.
- **Architecture**: Overview, How it works (posts 02-10), Crate tour (11-15),
  Internals (one `internals/*.mdx` page per mechanism).
- **Performance**: Measuring (the performance model, what a latency number
  means, the benchmark suite, startup latency) and Optimizations (one post per
  measured change).
- **Case studies**: Correctness (bugs: symptom, evidence, root cause, fix),
  Agents and CI, Bug hunts and notes.

The engineering posts are plain Markdown in `engineering/NN-*.md` (plain `.md`
so prose may contain `<` and `{`), each with a short `sidebarTitle`; they are
placed in the tabs above by topic, not by number. Everything else is `.mdx`.
Page paths in `docs.json` are root-relative with no extension
(`"concepts/networking"`, not `"concepts/networking.mdx"` or
`"/concepts/networking"`). Validate with `npx mint validate` and
`npx mint broken-links` (run on the dev box).

## Conventions

- Frontmatter: `title` + `description`, nothing else.
- Internal links are root-relative, no extension, `#anchor` where needed:
  `/internals/local-tunnel#the-fix`, not a relative `../` path and not
  `.mdx`.
- Callouts: `<Tip>`, `<Note>`, `<Warning>`, `<Info>`, `<Check>`, `<Danger>`.
  No Starlight `:::` aside syntax — this project was migrated off
  Starlight and none of that syntax is valid MDX here.
- Everything else (code blocks, tables, bullets) is plain Markdown,
  valid MDX as-is.

## Verifying a change

Install Mintlify's CLI once (`npm install -g mint`), then from this
directory:

```
mint validate        # docs.json schema + frontmatter
mint broken-links     # every internal link actually resolves
mint dev              # real local preview server
```

All three need to pass before pushing — nothing else catches a broken
link or an invalid `docs.json` here; there is no build step.

## Relationship to `website/`

The marketing site (`website/`) links out to this site by absolute URL
(`https://sandkiln.mintlify.site/...`) — see `website/AGENTS.md`.
There is no shared content, shared config, or shared design system
between the two; they're separate deployments by design.

## `release-notes.mdx` is a manual copy, not auto-synced

Named `release-notes.mdx`, not `changelog.mdx` — Mintlify has its own
built-in Changelog feature (`<Update>` blocks) that reserves the
`/changelog` path; a plain content page there gets silently shadowed
(served a 404 instead of the file, confirmed live — not assumed).

Unlike the old website page (which parsed root `CHANGELOG.md` at Astro
build time so it could never drift), this page is a one-time copy —
Mintlify has no build step to parse it automatically. Update it by hand
whenever `CHANGELOG.md` gains a new entry; there's nothing that catches
drift here the way there was before.
