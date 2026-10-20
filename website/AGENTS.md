# AGENTS.md — website/

Read root `AGENTS.md` first. This is the marketing site only — an Astro
project with no docs content of its own. Full docs are a separate
Mintlify project at `docs-site/` (see its own `AGENTS.md`), live at
https://sandkiln.mintlify.site/. Any in-site link to docs is an
absolute URL to that domain, not an internal route.

## What this is for

The public face: architecture, real (not aspirational) benchmarks, an
honest shipped/partial/planned feature grid, SDK examples, startup-latency
research. Stays accurate as the project changes — stale content here is a
bug, like a stale doc comment in code.

## Structure

- `src/pages/` — `index.astro` (hero, isolation argument, feature
  manifest, SDK example), `architecture.astro` (crate breakdown, boot
  lifecycle), `performance.astro`, `roadmap.astro`. No changelog page —
  that moved to `docs-site/release-notes.mdx` (a manual copy, not
  auto-synced; see `docs-site/AGENTS.md`).
- `src/components/`, `src/layouts/` — shared marketing pieces.
  `Header.astro`'s nav links to docs with a plain absolute URL to the
  Mintlify site (`DOCS_URL` constant); update that one place if the
  Mintlify domain ever changes.
- `src/lib/scale.ts` — the shared log axis behind the homepage gauge row
  (100µs–1s, one tick per decade, all four gauges comparable). Positions
  derive from the measurements themselves — fixing a number moves its
  mark, nothing hardcodes a percentage. A point mark needs `min-width`,
  not `width` (`markStyle`'s inline `width:0.00%` beats any class).
- `src/styles/tokens.css` — **the** design system, consumed by
  `global.css`. Add a colour here or nowhere.

## The design system's own rules

(Stated in `tokens.css`'s header; repeated here since breaking one is
invisible in a single-page diff.)

- **One family, headlines included** — everything in `--mono`, no
  `--sans`. Hierarchy is size/weight/colour only. `--measure` counts mono
  characters (58 ≈ 72 proportional); headline measures are in `ch`.
  Labels stay lowercase, never tracked-out caps.
- **One hue, `--flare`.** Greys are warm/desaturated so the palette is
  one axis. Used liberally but deliberately (every measured number, one
  phrase per headline, the primary control) — never tints a background
  or body paragraph. Two ambers exist because one can't do both jobs:
  `--flare` is text-safe, `--flare-solid` is the fill under `--flare-ink`.
- **Status is achromatic** — shipped/partial/planned are a
  filled/half-filled/hollow square of the same accent, not three hues.
  Findings: solid rule = settled, dashed = open. A second colour would
  compete with the one hue that exists.
- **Panels are plates, not cards** — no shadow anywhere; depth is a fill
  difference plus a hairline. Sized to content (hero console caps at 75%
  width, matching three of the four gauge columns below it).
- **Radius is 0 everywhere.**
- **Section separation is whitespace**, generously — a divider under
  every section would flatten the page into identical slabs.
- **Headlines are two terse lines**, one phrase in `--flare`, as two
  `.l` spans (not `<br>`) so a narrow screen wraps inside a line.
- **One non-user-triggered animation on the whole site** — the boot
  diagram's pulse; everything else responds to user action, and
  `prefers-reduced-motion` removes the pulse.

**Deliberately not a single scrolling page** — benchmarks and roadmap
depth deserve their own pages, not burial in a long homepage.

## Base path and Node version

GitHub Pages serves this as a project page (`sumitkumar-17.github.io/sandkiln/`);
a mirror serves it at a domain root — `astro.config.mjs` reads `ASTRO_BASE`
(default `/`), set to `/sandkiln` only by `deploy-pages.yml`. `ASTRO_SITE`
sets the absolute-URL origin separately. Astro 7 needs Node **≥22.12**,
newer than the Rust/CLI minimum (`>=18`).

## Rules for editing content

- **Every claim true right now**, not aspirational — "Shipped" only if
  verified on real hardware. Three feature-grid states exist
  (`done`/`partial`/`planned`) specifically so a gap gets `partial`
  instead of overstated.
- **Benchmarks are re-measured, not carried forward.** Re-run the
  relevant bench/load test when performance-relevant code changes,
  update `performance.astro` and `docs-site/architecture/startup-latency.mdx`
  together. Can't reproduce them without a real Firecracker+kernel+rootfs
  — say "carried forward" if you didn't actually re-run them.
- **Only publish numbers from a recorded run** — no rounded midpoints,
  no invented table cells. Ranges stay ranges.
- Three theme states must agree: no `data-theme` (OS), `="light"`, `="dark"`.
- Never name a competing platform anywhere in this directory's content.

## Verifying a change

`npm run build` passing is necessary, not sufficient — an unescaped `{ }`
in `.astro` (parsed as live JS, not literal text — bit an SDK code sample
once) won't always fail loudly. Run `npm run preview` and curl the
changed page.

```
cd website && npm install && npm run dev
# or: npm run build && npm run preview
```

## Deployment

GitHub Pages (`deploy-pages.yml`, every push to `main` touching this dir)
is the source of truth; `vercel.json` makes the same build deployable
elsewhere, auto-deploying a live mirror from `main` at
https://sandkiln.vercel.app. Pages wins if they disagree. Docs deploy
separately, automatically, from `docs-site/` via Mintlify's own GitHub
integration — nothing in this directory's build touches them.
