# AGENTS.md — website/

Read the root `AGENTS.md` first for project-wide conventions. This
directory holds two separate Astro static sites: the marketing site
(this directory's own `src/`) and the docs site (`docs/`, its own
Astro+Starlight project). Both deploy together — see "Deployment
targets" below.

## What this is for

The project's public face: architecture, real (not aspirational)
benchmark numbers, an honest shipped-vs-planned feature grid, an SDK
usage example, and a startup-latency research write-up — plus a full
docs site (getting started, core concepts, guides, and reference for
the daemon's HTTP API and both SDKs/the CLI). Both are meant to stay
**accurate as the project changes**, not be a one-time snapshot — treat
stale content here as a bug, the same way a stale doc comment in code
would be.

## Structure

- **Marketing site** (`package.json`, `astro.config.mjs`, `src/`) — a
  multi-page Astro project: `src/pages/index.astro` (hero, isolation
  pitch, feature grid, SDK example, links out), `src/pages/architecture
  .astro` (four-crate breakdown, boot lifecycle, links into the docs
  site's deeper essays), `src/pages/performance.astro` (real benchmark
  numbers plus the current startup-latency research), `src/pages/
  roadmap.astro` (a real roadmap page, not a footer strip). Shared
  design tokens/styles live in `src/styles/global.css`, imported once
  by `src/layouts/BaseLayout.astro`. **Deliberately not a single
  giant scrolling page** — a long page buries exactly the content
  (benchmarks, roadmap depth) worth surfacing; each of those gets its
  own page instead.
- **Docs site** (`docs/`) — Astro + Starlight, its own `package.json`/
  lockfile (not part of the root npm workspace, so its toolchain can't
  drift the SDK/CLI's own dependency versions). Content lives in
  `docs/src/content/docs/`, organized as Getting Started → Core
  Concepts → Guides (task-oriented) → Reference (CLI, HTTP API, both
  SDKs) → Architecture (the deep design-rationale essays, moved here
  from the marketing site since they belong next to the API they
  explain, not on a landing page). Sidebar structure is configured in
  `docs/astro.config.mjs`, not auto-generated from the file tree.

## The two-deploy-target base-path problem

GitHub Pages serves this as a *project* page (a subpath —
`sumitkumar-17.github.io/sandkiln/`); Vercel serves it at its own
domain root (`sandkiln.vercel.app/`). One static build can't have two
different asset base paths baked in at once, so both `astro.config.mjs`
files read `ASTRO_BASE` at build time (default `"/"`, so a plain `npm
run build`/`astro dev` — and Vercel's own build — just works without
ceremony) — the Pages workflow (`.github/workflows/deploy-pages.yml`)
is the one place that sets `ASTRO_BASE=/sandkiln` before building both
projects. The docs site's own base is always `${ASTRO_BASE}/docs` — it
is deployed as a `/docs` subpath of the *same* site, not a separate
one, so its build output gets copied into the marketing site's own
`dist/docs/` before either deploy target uploads anything. If you add a new internal link **inside docs page content** (markdown
body text, not the sidebar config), it needs the full base-relative
path *including* the leading `/docs` — e.g. `[Drives](/docs/concepts/
drives/)`, not `/concepts/drives/`. This is the opposite of what you'd
guess from Starlight's own sidebar: `docs/astro.config.mjs`'s sidebar
`slug` values (`"getting-started/introduction"`, no leading slash, no
`/docs`) *do* get auto-prefixed with the configured `base` when
Starlight renders the nav, but a plain markdown link you write in a
page's own body is passed through untouched — verified by actually
building and curling the rendered page's real `href`, not by reading
the source markdown, after this exact confusion produced ~50 silently
broken links across every content page on the first pass. Cross-links
from a docs page back to the *marketing* site (e.g. mentioning the
Roadmap page) can't be fixed with a path at all — the two sites don't
share a build, so those stay plain text, not a link.

## Node version

Astro 7 requires Node **≥22.12** — noticeably newer than this repo's
Rust/CLI-side minimum (`>=18`, see `packages/*/package.json`). The dev
box's `nvm` default had to be bumped to pick this up; `scripts/remote.sh`
sources whatever `nvm`'s current default is, so this only needed fixing
once per host, not per build.

## Rules for editing content

- **Every claim needs to be true right now**, not aspirational. A
  feature card/roadmap row says "Shipped" only if it's actually
  verified working on real hardware (see root `AGENTS.md`'s
  verification standard) — not because code was written that's
  *supposed* to do it. Mark genuinely unfinished things "Planned," not
  "Shipped" with a footnote.
- **Benchmark numbers are re-measured, not carried forward by
  assumption** — when underlying performance-relevant code changes,
  re-run the relevant benchmark/load-test and update the numbers on
  `src/pages/performance.astro` (and the docs site's
  `architecture/startup-latency.md`, which cites the same numbers),
  don't leave stale figures next to new code.
- Respects light/dark mode via CSS custom properties defined once in
  `src/styles/global.css` (`:root`, then overridden for dark via
  `prefers-color-scheme` and `[data-theme]`) — if you add a new color
  anywhere, add it as a token in both places, not a one-off literal, or
  it'll break in one theme. The docs site's `docs/src/styles/custom.css`
  mirrors the same accent color into Starlight's own CSS variables so
  the two sites feel like one product.
- Never name a competing platform or company anywhere in this
  directory's content — describe techniques/patterns generically.

## Verifying a change

Both projects have a real build step now — `astro build` in `website/`
and `website/docs/` respectively, run via `scripts/remote.sh run` per
this session's established workflow (see root `AGENTS.md`). `astro
build` succeeding is necessary but not sufficient — actually start
`astro preview` and curl (or screenshot) the changed page before
calling a content change done; a broken internal link or a template
error (an unescaped `{ }` in a `.astro` file gets parsed as a live JS
expression, not literal text — this bit the SDK code sample once)
won't always fail the build loudly.

## Deployment targets

GitHub Pages (`.github/workflows/deploy-pages.yml`, on every push to
`main` touching this directory) is the primary deploy
(https://sumitkumar-17.github.io/sandkiln/). `vercel.json` (repo root)
makes the same merged build deployable on any platform that imports
this repo and reads that file. A second live mirror is up at
https://sandkiln.vercel.app, auto-deployed on every push to `main` via
that platform's own GitHub integration (not a workflow file in this
repo — the Vercel project's own Root Directory/Build/Output/Install
Command settings are auto-detect, so `vercel.json` stays the single
source of truth; they drifted to a stale `Root Directory: website`
leftover from the old single-file site once, silently breaking every
deploy until caught by actually inspecting the deployment logs, not
just the dashboard's green checkmark). Same content as Pages; if the
two ever visibly disagree, Pages is the source of truth
(`deploy-pages.yml` is the one deploy this repo directly controls and
verifies).

`website/docs/` is **also** deployed a third way: standalone, at its
own domain root, as a separate Vercel project ("sandkiln-docs", not
"sandkiln") aliased to https://sandkiln-docs.vercel.app — for anyone
who wants a clean docs-only link instead of the merged site's `/docs`
subpath. That project's Root Directory is `website/docs` and it sets
`ASTRO_DOCS_STANDALONE=true` (a production environment variable
configured on the project, not in any committed file), which
`website/docs/astro.config.mjs` reads to serve at base `"/"` instead of
computing `${ASTRO_BASE}/docs` — every internal link in this site's own
content is written as a path relative to the current page for exactly
this reason, so the same markdown works unmodified under both base
values; verify a content change against **both** builds
(`npm run build` and `ASTRO_DOCS_STANDALONE=true npm run build`), not
just one, before calling it done. `docs.sandkiln.vercel.app` (a
subdomain of the main site's own `*.vercel.app` alias) is **not**
available — Vercel only grants an account `*.vercel.app` and
`*.<team>.vercel.app`, not arbitrary subdomains of another alias it
already owns — that's why this one is `sandkiln-docs.vercel.app`
instead. This project auto-deploys on push same as the main one
(`vercel git connect`, done once from the CLI).
