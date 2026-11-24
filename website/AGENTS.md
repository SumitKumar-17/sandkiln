# AGENTS.md: website/

Read root `AGENTS.md` first. Marketing site only: an Astro project with no docs content of its own. Full docs are the
Mintlify project in `docs-site/`, live at https://sandkiln.mintlify.site/; any in-site docs link is an absolute URL to that
domain. Content must stay accurate as the project changes; stale content is a bug.

## Structure

- `src/pages/`: `index.astro` (hero, isolation argument, feature grid, SDK example), `architecture.astro`,
  `performance.astro`, `roadmap.astro`, and `engineering/` (routes for the posts in `src/content/engineering/`, defined by
  `src/content.config.ts`). There is no changelog page; that is `docs-site/release-notes.mdx`.
- `src/components/`, `src/layouts/`: shared pieces. `Header.astro` links to docs through one `DOCS_URL` constant.
- `src/lib/scale.ts`: the shared log axis behind the homepage gauges (100us to 1s). Positions derive from the measurements,
  so fixing a number moves its mark. A point mark needs `min-width`, not `width`.
- `src/styles/tokens.css`: the design system, consumed by `global.css`. Add a colour here or nowhere.

## Design rules

(Also in `tokens.css`'s header; breaking one is invisible in a single-page diff.)

- One family: everything in `--mono`, no sans. Hierarchy is size, weight and colour. Labels are lowercase, never tracked caps.
- One hue, `--flare`, on measured numbers, one phrase per headline and the primary control; never a background or body
  paragraph. `--flare` is the text-safe amber, `--flare-solid` the fill under `--flare-ink`.
- Status is achromatic: shipped, partial and planned are a filled, half-filled and hollow square.
- Panels are plates: no shadow, depth from a fill difference plus a hairline. Radius is 0 everywhere.
- Sections separate with whitespace, not dividers. Headlines are two terse lines with one phrase in `--flare`, as two `.l`
  spans (not `<br>`).
- One non-user-triggered animation (the boot diagram pulse); `prefers-reduced-motion` removes it.
- Three theme states must agree: no `data-theme` (OS), `light`, `dark`.

## Content rules

- Every claim true now. Feature-grid states are `done`, `partial` and `planned`; use `partial` rather than overstate.
- Benchmarks are re-measured, not carried forward. Update `performance.astro` and
  `docs-site/architecture/startup-latency.mdx` together, and say "carried forward" if you did not re-run them (it needs a real
  Firecracker, kernel and rootfs). Publish only numbers from a recorded run; ranges stay ranges.
- Never name a competing platform in this directory.

## Build and deploy

```
cd website && npm install && npm run dev      # or: npm run build && npm run preview
```

A passing build is necessary, not sufficient: an unescaped `{ }` in `.astro` parses as JS and may not fail loudly. Preview and
curl the changed page. Astro 7 needs Node 22.12 or newer.

GitHub Pages (`deploy-pages.yml`, on pushes to `main` touching this directory) is the source of truth, served as a project page;
`astro.config.mjs` reads `ASTRO_BASE` (default `/`, set to `/sandkiln` only by that workflow) and `ASTRO_SITE`. A mirror at
https://sandkiln.vercel.app builds from the same source via `vercel.json`; Pages wins if they disagree. Docs deploy separately
through Mintlify's GitHub integration.
