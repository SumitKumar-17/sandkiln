# sandkiln docs

Detail that used to live in `ROADMAP.md`: findings, measurements, design
reasoning, rejected ideas and open questions. The [roadmap](../ROADMAP.md) is
the short current-state view.

Single source of truth: user-facing and mechanism docs live in `docs-site/`
(Mintlify: concepts, internals, architecture, API reference) and narrative
write-ups in `website/src/content/engineering/` (posts 01-28). Files here link
to those instead of repeating them and keep only what they do not carry
(decisions, caveats, verification records, open questions). When a fact
changes, edit it in one place.

| File | Contents |
| --- | --- |
| [findings.md](findings.md) | Redevelopment audit findings 1-26: symptom, root cause, fix, status. Findings 20, 25 and 26 are fixed; 27 is open. |
| [benchmarks.md](benchmarks.md) | Method, current numbers, before/after history, profiling conclusions. |
| [documentation-plan.md](documentation-plan.md) | Where each kind of fact lives, page shapes, size budgets, what `docs-check` enforces. |
| [code-conventions.md](code-conventions.md) | Layout, file shape, error and state rules, recipes for adding an endpoint, setting or script. |
| [development.md](development.md) | Build, test and benchmark commands, repository layout. |

## Design notes

| File | Contents |
| --- | --- |
| [snapshots.md](design-notes/snapshots.md) | Snapshot, resume, fork, time-travel, archive, named and idle sandboxes |
| [pools-and-lineage.md](design-notes/pools-and-lineage.md) | Warm pools, resume failures, snapshot lineage |
| [drives-and-storage.md](design-notes/drives-and-storage.md) | Drives, read-only sharing, remote storage mounts |
| [egress-firewall.md](design-notes/egress-firewall.md) | Per-sandbox egress policy |
| [security-hardening.md](design-notes/security-hardening.md) | Isolation, jailer, limits, rate limiting, encryption at rest |
| [preview.md](design-notes/preview.md) | Preview proxy, tunnel, PTY, streamed exec |
| [tags-and-metadata.md](design-notes/tags-and-metadata.md) | History store, metadata service |
| [observability.md](design-notes/observability.md) | Logging, request ids, metrics |
| [sdks-and-cli.md](design-notes/sdks-and-cli.md) | SDK and CLI surface and behavior |
| [images-and-auth.md](design-notes/images-and-auth.md) | Auth, images, networking, docs and examples |
| [load-testing.md](design-notes/load-testing.md) | Planned concurrency-ceiling test |
| [latency-investigation.md](design-notes/latency-investigation.md) | Open latency questions |
| [feature-plan.md](design-notes/feature-plan.md) | Ordered feature plan: goals, approach and verification per item |
| [ideas-not-started.md](design-notes/ideas-not-started.md) | Future directions and non-goals |

## Where each former roadmap section went

| Former `ROADMAP.md` section | Now |
| --- | --- |
| Redevelopment audit, plan, re-measure, state record | [findings.md](findings.md) |
| What works today (history) | [findings.md](findings.md#state-of-what-works-before-the-audit), [../ROADMAP.md](../ROADMAP.md) status table |
| Networking, Authentication, Images, Documentation and examples | [images-and-auth.md](design-notes/images-and-auth.md) |
| Client SDKs, CLI | [sdks-and-cli.md](design-notes/sdks-and-cli.md) |
| Persistence and snapshotting | [snapshots.md](design-notes/snapshots.md) |
| Drives and remote storage | [drives-and-storage.md](design-notes/drives-and-storage.md) |
| Firewall and egress policy | [egress-firewall.md](design-notes/egress-firewall.md) |
| Security hardening, multi-agent isolation, privileged workloads | [security-hardening.md](design-notes/security-hardening.md) |
| Dev servers and live preview | [preview.md](design-notes/preview.md) |
| Tags and sandbox metadata | [tags-and-metadata.md](design-notes/tags-and-metadata.md) |
| Benchmarking (incl. pools and lineage) | [benchmarks.md](benchmarks.md), [pools-and-lineage.md](design-notes/pools-and-lineage.md) |
| Observability | [observability.md](design-notes/observability.md) |
| Multi-node, Ecosystem, Ideas not started, writing-style pass | [ideas-not-started.md](design-notes/ideas-not-started.md) |
| Load testing | [load-testing.md](design-notes/load-testing.md) |
| Further latency investigation | [latency-investigation.md](design-notes/latency-investigation.md) |
