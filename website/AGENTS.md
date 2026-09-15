# AGENTS.md — website/

Read the root `AGENTS.md` first for project-wide conventions. This
directory is **one** Astro project that serves the whole public site:
the marketing pages at the root, and the full Starlight documentation
under `/docs`. There is no second project and no second lockfile.

## What this is for

The project's public face: architecture, real (not aspirational)
benchmark numbers, an honest shipped-vs-planned feature grid, SDK usage
examples, and a startup-latency write-up — plus the docs (getting
started, core concepts, task guides, and reference for the daemon's HTTP
API, both SDKs, and the CLI). It is meant to stay **accurate as the
project changes**, not be a one-time snapshot: treat stale content here
as a bug, exactly like a stale doc comment in code.

## Structure

- `src/pages/` — the marketing pages: `index.astro` (hero readout,
  isolation argument, feature manifest, SDK example, links out),
  `architecture.astro` (five-crate breakdown, boot lifecycle, links into
  the docs' deeper essays), `performance.astro` (measured numbers plus
  the startup-latency research), `roadmap.astro`, `changelog.astro`
  (parses the repo root's `CHANGELOG.md` at build time via `marked`
  rather than hand-transcribing it — one `## `-level heading becomes one
  rendered entry, so this page can't drift from what the file actually
  says; update `CHANGELOG.md` and this page follows). Also `llms.txt.ts`
  and `llms-full.txt.ts`, which generate the plain-text site index from
  the docs collection at build time.
- `src/content/docs/` — every docs page. Organized Getting Started →
  Core Concepts → Guides (task-oriented) → Reference → Architecture (the
  deep design-rationale essays, which live here rather than on a
  marketing page because they belong next to the API they explain).
- `src/components/`, `src/layouts/` — shared marketing-page pieces.
  `DocsSiteTitle.astro` overrides Starlight's site title so the docs
  header carries the same top-level nav; without it the docs are a
  one-way trip out of the marketing pages.
- `src/lib/scale.ts` — the shared log axis behind the homepage gauge row.
  All four gauges plot on one axis (100µs to 1s, a tick per decade), so
  their marks are directly comparable across columns. Positions are
  derived from the measurements themselves, so correcting a figure moves
  its mark too; nothing hard-codes a percentage. A measured range renders
  as a range and a single recorded figure renders as a tick, which is the
  publish-ranges-as-ranges rule below made visible rather than merely
  stated. A point mark needs `min-width`, not `width`: `markStyle` emits
  an inline `width:0.00%` that beats any class.
- `src/styles/tokens.css` — **the** design system, imported by both
  surfaces. `global.css` (marketing) and `starlight.css` (docs, via
  `customCss`) both consume it; `starlight.css` maps Starlight's own
  `--sl-*` variables onto these tokens and contains no hex literals. Add
  a color here or nowhere, or it will break in one theme or on one half
  of the site.

## The design system's own rules

Stated in `tokens.css`'s header comment, repeated here because breaking
one of them is easy and invisible in a single-page diff:

- **One family, headlines included.** Everything — display, headings,
  labels, body — is set in `--mono`. There is no second typeface and no
  `--sans`; hierarchy is carried by size, weight and colour. Because mono
  sets wide, `--measure` counts mono characters (58 ≈ 72 proportional
  ones) and headline measures are set in `ch` so a written line renders
  as one line. Labels stay lowercase, never tracked-out caps.
- **One hue: `--flare`.** The greys are warm and desaturated so the
  palette is a single axis. The accent is used liberally and
  deliberately — every measured number, exactly one phrase per section
  headline, the primary control, the live marks, the console's
  significant lines. It never tints a background or a body paragraph.
  Two ambers exist because one cannot do both jobs: `--flare` is the
  text-safe value in each theme, `--flare-solid` is the fill that carries
  `--flare-ink`.
- **Status is achromatic.** shipped / partial / planned are a filled,
  half-filled and hollow square of the same accent, not three hues. Same
  for findings: a solid left rule means settled, a dashed one means open.
  A second colour for "not done" would compete with the only hue there
  is.
- **Panels are plates, not cards.** No shadow is defined anywhere in the
  system, and none should be added; depth is a fill difference plus a
  hairline. A panel is sized to its content — a console or code block
  stretched well past its longest line reads as a layout accident, which
  is why the hero console caps at 75% (exactly three of the four gauge
  columns below it).
- **Radius is 0 everywhere**, including on Starlight's own controls,
  which `starlight.css` squares off explicitly.
- **Section separation is whitespace, and there is a lot of it.** Each
  band is short — a two-line headline, a sentence or two, one visual —
  with `--section-y` of air before the next. A rule appears only where it
  marks real structure; a divider under every section flattens the page
  into identical slabs.
- **Headlines are two terse lines** with one phrase in `--flare`, written
  as two `.l` block spans rather than a `<br>` so a narrow screen wraps
  inside a line instead of fighting a hard break.
- **One non-user-triggered animation exists on the whole site** — the
  boot diagram's pulse, which traces the real four-step create path.
  Everything else animates only in response to a person's action, and
  `prefers-reduced-motion` removes the pulse entirely.

**Deliberately not a single scrolling page.** Benchmarks and roadmap
depth are the content most worth surfacing, and a long homepage buries
exactly those; each gets its own page instead.

## How the docs get their `/docs` prefix

`src/content.config.ts` passes a `generateId` to Starlight's
`docsLoader()` that prefixes every entry id with `docs/`. So
`src/content/docs/concepts/drives.md` serves at `/docs/concepts/drives/`
without the file tree needing a second literal `docs/` directory, and
the prefix is stated in exactly one place. Two consequences:

- Sidebar entries in `astro.config.mjs` must use the prefixed slug. The
  `docs()` helper at the top of that file does it — use it.
- Starlight and `src/pages/` share one route namespace. A marketing page
  and a docs page can't claim the same path.

**Internal links inside docs page content must be relative to the
current page** (`../concepts/drives/`), never absolute (`/docs/...`,
`/concepts/...`). Relative links survive the base-path change below;
absolute ones silently break on one deploy target. A previous version of
this site shipped ~50 broken links by getting this wrong, and the build
does not catch it — verify a changed link by curling the rendered
`href`, not by reading the markdown source.

## The base-path problem

GitHub Pages serves this as a *project* page (a subpath,
`sumitkumar-17.github.io/sandkiln/`); a mirror serves it at a domain
root. One static build can't bake in both, so `astro.config.mjs` reads
`ASTRO_BASE` (default `"/"`, so plain `npm run dev`/`npm run build` and
any root-served deploy work with no ceremony). The Pages workflow
(`.github/workflows/deploy-pages.yml`) is the single place that sets
`ASTRO_BASE=/sandkiln`. `ASTRO_SITE` separately sets the origin used for
absolute URLs (sitemap, canonicals, `llms.txt`).

Because the docs are part of this project, one `astro build` emits both
halves — `dist/` and `dist/docs/`. There is nothing to merge afterwards
and no second `npm ci` in CI.

## Integration order

Starlight registers `astro-expressive-code`, which must be set up before
`mdx()`. The integrations array is `[starlight(), mdx()]` for that
reason; reversing it fails the build with an explicit message.

## Node version

Astro 7 requires Node **≥22.12** — newer than this repo's Rust/CLI-side
minimum (`>=18`, see `packages/*/package.json`).

## Rules for editing content

- **Every claim must be true right now**, not aspirational. A feature
  row says "Shipped" only if it is actually verified working on real
  hardware (root `AGENTS.md`'s verification standard) — not because code
  exists that is supposed to do it. The feature manifest has three
  states on purpose: `done`, `partial` (built, with a stated gap), and
  `planned`. Reach for `partial` rather than overstating; several rows
  were wrong in both directions before that state existed.
- **Benchmark numbers are re-measured, not carried forward by
  assumption.** When performance-relevant code changes, re-run the
  relevant criterion bench or load test and update
  `src/pages/performance.astro` and the docs' `architecture/
  startup-latency.md`, which cite the same figures. Re-running them
  needs a Firecracker binary plus a built kernel and rootfs — a checkout
  alone cannot reproduce them, so if you cannot actually run them, say
  the numbers were carried forward rather than implying a fresh
  measurement.
- **Publish only numbers that exist in a recorded run.** No rounded
  midpoints of a measured range, no filling in a table cell that was
  never recorded — a dash and a note are correct, an invented figure is
  not. Ranges get published as ranges.
- Respects light and dark mode through the tokens described above.
  Three theme states must agree: no `data-theme` (follow the OS),
  `data-theme="light"`, and `data-theme="dark"` (Starlight's toggle
  writes the latter two).
- Never name a competing platform or company anywhere in this
  directory's content — describe techniques and patterns generically.

## Verifying a change

`npm run build` succeeding is necessary but not sufficient: a broken
internal link or a template error (an unescaped `{ }` in a `.astro` file
is parsed as a live JS expression, not literal text — this bit an SDK
code sample once) will not always fail it loudly. Actually run
`npm run preview` and curl the changed page.

```
cd website
npm install
npm run dev        # or: npm run build && npm run preview
```

## Deployment

GitHub Pages (`.github/workflows/deploy-pages.yml`, on every push to
`main` touching this directory) is the primary deploy and the source of
truth. `vercel.json` at the repo root makes the same single build
deployable on any platform that imports this repo and reads that file; a
live mirror auto-deploys from `main` that way. If the two ever visibly
disagree, Pages wins — it is the one deploy this repo directly controls
and verifies.

The docs previously had a third, standalone deploy at their own domain
root, which required an `ASTRO_DOCS_STANDALONE` env var and a second
build of a second project. Consolidating into one project removed both
that deploy and the whole class of base-path bugs it kept producing. Do
not reintroduce a separate docs project without a concrete reason that
outweighs that.
