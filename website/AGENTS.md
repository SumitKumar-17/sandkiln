# AGENTS.md — website/

Read root `AGENTS.md` first. **One** Astro project serves the whole
public site: marketing pages at the root, full Starlight docs under
`/docs`. No second project, no second lockfile.

## What this is for

The public face: architecture, real (not aspirational) benchmarks, an
honest shipped/partial/planned feature grid, SDK examples, startup-latency
research, plus the docs (getting started, concepts, guides, reference).
Stays accurate as the project changes — stale content here is a bug, like
a stale doc comment in code.

## Structure

- `src/pages/` — `index.astro` (hero, isolation argument, feature
  manifest, SDK example), `architecture.astro` (crate breakdown, boot
  lifecycle), `performance.astro`, `roadmap.astro`, `changelog.astro`
  (parses root `CHANGELOG.md` via `marked` at build time — one `## `
  heading per entry, so it can't drift from the file). `llms.txt.ts`/
  `llms-full.txt.ts` generate the plain-text site index at build time.
- `src/content/docs/` — Getting Started → Core Concepts → Guides →
  Reference → Architecture (deep design-rationale essays, kept next to
  the API they explain rather than on a marketing page).
- `src/components/`, `src/layouts/` — shared marketing pieces.
  `DocsSiteTitle.astro` gives the docs header the same top-level nav as
  the marketing pages — without it the docs are a one-way trip out.
- `src/lib/scale.ts` — the shared log axis behind the homepage gauge row
  (100µs–1s, one tick per decade, all four gauges comparable). Positions
  derive from the measurements themselves — fixing a number moves its
  mark, nothing hardcodes a percentage. A point mark needs `min-width`,
  not `width` (`markStyle`'s inline `width:0.00%` beats any class).
- `src/styles/tokens.css` — **the** design system, consumed by both
  `global.css` (marketing) and `starlight.css` (docs, via `customCss`,
  no hex literals of its own). Add a colour here or nowhere.

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
- **Radius is 0 everywhere**, Starlight's own controls squared off
  explicitly in `starlight.css`.
- **Section separation is whitespace**, generously — a divider under
  every section would flatten the page into identical slabs.
- **Headlines are two terse lines**, one phrase in `--flare`, as two
  `.l` spans (not `<br>`) so a narrow screen wraps inside a line.
- **One non-user-triggered animation on the whole site** — the boot
  diagram's pulse; everything else responds to user action, and
  `prefers-reduced-motion` removes the pulse.

**Deliberately not a single scrolling page** — benchmarks and roadmap
depth deserve their own pages, not burial in a long homepage.

## `/docs` prefix and links

`src/content.config.ts` prefixes every docs entry id with `docs/` via
Starlight's `docsLoader()` — so sidebar slugs in `astro.config.mjs` must
use that prefix (the `docs()` helper does it) and Starlight/`src/pages/`
share one route namespace.

**Internal docs links must be relative** (`../concepts/drives/`), never
absolute (`/docs/...`) — absolute links silently break on one deploy
target. A previous version shipped ~50 broken links this way; the build
doesn't catch it. Verify by curling the rendered `href`, not reading markdown.

## Base path, integration order, Node version

GitHub Pages serves this as a project page (`sumitkumar-17.github.io/sandkiln/`);
a mirror serves it at a domain root — `astro.config.mjs` reads `ASTRO_BASE`
(default `/`), set to `/sandkiln` only by `deploy-pages.yml`. `ASTRO_SITE`
sets the absolute-URL origin separately. One `astro build` emits both
`dist/` and `dist/docs/` — nothing to merge, no second `npm ci`.

Starlight must register before `mdx()` (`astro-expressive-code` ordering)
— `[starlight(), mdx()]`, reversing it fails the build with an explicit
message. Astro 7 needs Node **≥22.12**, newer than the Rust/CLI minimum (`>=18`).

## Rules for editing content

- **Every claim true right now**, not aspirational — "Shipped" only if
  verified on real hardware. Three feature-grid states exist
  (`done`/`partial`/`planned`) specifically so a gap gets `partial`
  instead of overstated.
- **Benchmarks are re-measured, not carried forward.** Re-run the
  relevant bench/load test when performance-relevant code changes,
  update `performance.astro` and `architecture/startup-latency.md`
  together. Can't reproduce them without a real Firecracker+kernel+rootfs
  — say "carried forward" if you didn't actually re-run them.
- **Only publish numbers from a recorded run** — no rounded midpoints,
  no invented table cells. Ranges stay ranges.
- Three theme states must agree: no `data-theme` (OS), `="light"`, `="dark"`.
- Never name a competing platform anywhere in this directory's content.

## Verifying a change

`npm run build` passing is necessary, not sufficient — a broken internal
link or an unescaped `{ }` in `.astro` (parsed as live JS, not literal
text — bit an SDK code sample once) won't always fail loudly. Run
`npm run preview` and curl the changed page.

```
cd website && npm install && npm run dev
# or: npm run build && npm run preview
```

## Deployment

GitHub Pages (`deploy-pages.yml`, every push to `main` touching this dir)
is the source of truth; `vercel.json` makes the same build deployable
elsewhere, auto-deploying a live mirror from `main`. Pages wins if they
disagree. A prior standalone third deploy for the docs (its own domain,
`ASTRO_DOCS_STANDALONE`) was removed along with the whole class of
base-path bugs it caused — don't reintroduce one without a concrete
reason that outweighs that.
