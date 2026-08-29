# AGENTS.md — website/

Read the root `AGENTS.md` first for project-wide conventions. This is a
single static HTML file (`index.html`) — no build step, no framework —
deployed via GitHub Pages on every push that touches this directory (see
`.github/workflows/deploy-pages.yml`).

## What this is for

The project's public face: architecture, real (not aspirational)
benchmark numbers, an honest shipped-vs-planned feature grid, an SDK
usage example. It is meant to stay **accurate as the project changes**,
not be a one-time snapshot — treat stale content here as a bug, the same
way a stale doc comment in code would be.

**Design-level architecture writeups belong here, not as new markdown
files scattered through the repo.** If a change needs explaining beyond
what a code comment or `AGENTS.md`'s "why" notes can carry — a real
design rationale, a diagram, a comparison — add it to the relevant
section of this page rather than creating a new `.md` file. This page
already has an Architecture section (the four-crate breakdown, the boot
lifecycle) as the place for that kind of content.

## Rules for editing this page

- **Every claim here needs to be true right now**, not aspirational. A
  feature card says "Shipped" only if it's actually verified working on
  real hardware (see root `AGENTS.md`'s verification standard) — not
  because code was written that's *supposed* to do it. Mark genuinely
  unfinished things "Planned," not "Shipped" with a footnote.
- **Benchmark numbers are re-measured, not carried forward by assumption**
  — when underlying performance-relevant code changes, re-run the
  relevant benchmark/load-test and update the numbers here, don't leave
  stale figures next to new code.
- The page is a single file with inlined CSS/JS by design (no build
  step, deploys as-is) — don't split it into multiple files or introduce
  a bundler for it.
- Respects light/dark mode via CSS custom properties defined once at the
  top (`:root`, then overridden for dark via `prefers-color-scheme` and
  `[data-theme]`) — if you add a new color anywhere, add it as a token
  in both places, not a one-off literal, or it'll break in one theme.

## Verifying a change

There's no build step to run — open the file directly in a browser, or
push to `main` and let the Pages workflow deploy it
(https://sumitkumar-17.github.io/sandkiln/), and actually look at it in
both light and dark mode before calling a visual change done.

## Deployment targets

GitHub Pages (above) is the primary deploy, driven by
`.github/workflows/deploy-pages.yml` on every push to `main`. The root
`vercel.json` (`outputDirectory: "website"`, no build command) makes this
directory also deployable as a static site on any platform that imports a
git repo and reads that file — importing this repo there and pointing it
at the default root directory is enough, no further config needed. This
is a hosting choice only, same as Pages is — it has no bearing on what
gets written in this file's content.
