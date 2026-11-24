# Documentation plan

How documentation is organised so it stays correct as the codebase grows.
Three rules drive everything below:

1. **One home per fact.** Every fact (a setting, a number, an endpoint, a
   decision) lives in exactly one place. Everywhere else links to it.
2. **Lose nothing, repeat nothing.** Shortening a page means moving detail to
   its home and linking, never deleting it.
3. **Enforced, not remembered.** `scripts/dev.sh docs-check` fails on broken
   links, unregistered pages, banned wording and size-budget overruns.

## Surfaces

| Surface | Audience | Owns | Must not contain |
| --- | --- | --- | --- |
| `README.md` | First-time visitor on GitHub | Pitch, one diagram, headline numbers, quick start, a link table | Feature lists, SDK walkthroughs, build steps, config detail |
| Website (`website/`) | Evaluator | One page: what it is, one install command, a real session, four measured numbers, links to the docs | Anything technical: architecture, reference, setup detail, posts |
| `docs-site/` (Mintlify) | User, operator, engineer | Five tabs: Docs, API Reference, Architecture, Performance, Case studies. The engineering posts live in `docs-site/engineering/` and are placed in the tabs by topic | Measurements history, decisions, roadmap |
| `docs/` | Maintainer | Findings, benchmarks, design notes, these plans | User how-tos |
| `ROADMAP.md` | Maintainer, contributor | Status table (done, partial, planned), ordered next steps | History, measurements, design reasoning |
| `CHANGELOG.md` | Upgrader | What changed per release | Explanations (link to docs) |
| `AGENTS.md` (root, per package) | Contributor, human or agent | Standard, file map, invariants, how to test | Narrative history |
| Code comments | Reader of that code | *Why* a non-obvious thing is so | Design essays, measurements (link to `docs/`) |

## Where a fact goes

| Fact | Home | Everyone else |
| --- | --- | --- |
| A daemon setting | `core/crates/daemon/src/config/settings.rs` registry; reference rendered by `sandkilnd --config-reference` and `sandkiln.example.toml` (drift-tested) | `docs-site/self-hosting/configuration.mdx` links to it |
| An HTTP endpoint | `docs-site/api-reference/http-api/<resource>.mdx` | SDK and CLI pages link to it |
| An SDK or CLI method | its `docs-site/api-reference/<client>/` page | quickstarts show one short example |
| A measurement | the recorded result files in `examples/benchmark-suite/results/`. The docs page `performance/current-numbers` and the website's figures are generated from them; `docs/benchmarks.md` holds method and history | README carries none; posts link to the page |
| A decision, finding or rejected idea | `docs/findings.md` or `docs/design-notes/<area>.md` | code comments link to it |
| How a mechanism works | `docs-site/internals/<topic>.mdx` | posts tell the story and link |
| A story with diagrams | `docs-site/engineering/NN-*.md` (the blog, part of the docs site) | internals pages link to it |
| What changed | `CHANGELOG.md` | none |
| What is planned | `ROADMAP.md` status table; detail in `docs/design-notes/ideas-not-started.md` | none |

## Page shapes

- **Concept** (`docs-site/concepts/`): what it is, when to use it, a minimal example, links to reference and internals.
- **Reference** (`docs-site/api-reference/`): a table of routes or methods, then one worked example. No prose history.
- **Internals** (`docs-site/internals/`): the mechanism, the invariant it protects, the failure mode it prevents, a diagram if it has moving parts.
- **Design note** (`docs/design-notes/`): context, decision, alternatives rejected and why, verification record, open questions.
- **Finding** (`docs/findings.md`): symptom, root cause, fix, status (fixed, open, note).
- **Post** (`docs-site/engineering/`, plain Markdown so prose can contain `<` and `{`): one subsystem or optimisation, structure, pattern, diagram, what was measured.

## Size budgets

Checked by `docs-check`. Hitting one means splitting by topic, not trimming facts.

| File | Budget |
| --- | --- |
| `README.md` | 90 lines |
| `ROADMAP.md` | 160 lines |
| a `docs/design-notes/` file | 300 lines |
| a per-package `AGENTS.md` | 300 lines |

## Changing the project

A user-visible change touches, in order: code, tests, the reference page, the
SDK and CLI pages if affected, `CHANGELOG.md`, `ROADMAP.md` if a status
changed, and a design note or finding if there was a decision or surprise.
`scripts/dev.sh docs-check` must pass. The code-side checklist is in
[code-conventions.md](code-conventions.md).

## Adding a new area

Add a `docs-site/concepts/` page and an `internals/` page (register both in
`docs-site/docs.json`), a `docs/design-notes/` file (index it in
[README.md](README.md)), and a row in the `ROADMAP.md` status table.

## What `docs-check` enforces

- Relative Markdown links resolve to a file.
- Every `docs-site/**/*.mdx` page is in `docs.json`, and every `docs.json` entry exists.
- No "Phase N" wording and no pointers to removed `ROADMAP.md` or `SELF_HOSTING.md` sections.
- The size budgets above.
