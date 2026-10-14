# AGENTS.md — docs-site/

Read root `AGENTS.md` first. This is the full docs site: a Mintlify
project, independent of `website/` (marketing pages only). Deploys
automatically from `main` via Mintlify's GitHub integration to
https://sandkiln.mintlify.site/ — no build step in this repo, no
CI workflow to maintain.

## Structure

`docs.json` is the nav (groups: Getting Started, Core Concepts, Guides,
Internals, Reference, Architecture) and theme config. Every page is
`.mdx`; page paths in `docs.json` are root-relative with no extension
(`"concepts/networking"`, not `"concepts/networking.mdx"` or
`"/concepts/networking"`).

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
