---
title: "Current numbers"
description: "Recorded benchmark results: create, first command, resume, fork, snapshot and parallel creates, with the conditions each was measured under."
---

These figures are medians from recorded runs of the [benchmark suite](/engineering/32-a-benchmark-you-can-rerun), read from the result files in the repository when this page is generated. They come from one shared development machine, so treat them as directional, and rerun the suite on your own hardware with `scripts/dev.sh bench-suite`. What each number does and does not include is in [what a start-latency number means](/engineering/29-what-a-start-latency-number-means).

## Create to first command, warm pool, one create every 4 seconds

The default warm pool keeps instances ready, and the pause between creates lets it refill: the steady state of a real workload.

| Measurement | Median | p95 | Min | Max | Runs |
| --- | ---: | ---: | ---: | ---: | ---: |
| create call returns | 22.4 ms | 36.7 ms | 18.4 ms | 49.3 ms | 20 |
| first command after create returns | 2.26 ms | 3.49 ms | 1.62 ms | 3.89 ms | 20 |
| create to first command | 25.9 ms | 38.8 ms | 20.4 ms | 51.0 ms | 20 |

Run `2026-11-24T1950-warm-pool-on-paced-i9-13900K.json`, 2026-11-24, daemon 0.0.1, guest kernel 5.10.223.

## Create to first command, warm pool, one create every 1.5 seconds

Shorter gaps: the pool refills as soon as a claim drains it.

| Measurement | Median | p95 | Min | Max | Runs |
| --- | ---: | ---: | ---: | ---: | ---: |
| create to first command | 27.2 ms | 49.4 ms | 21.4 ms | 49.5 ms | 20 |

Run `2026-11-24T1950-warm-pool-on-pause1500-gaps-i9-13900K.json`, 2026-11-24, daemon 0.0.1, guest kernel 5.10.223.

## Create to first command, warm pool, back to back

Twenty creates in a row outrun a pool of two, so most of them boot cold.

| Measurement | Median | p95 | Min | Max | Runs |
| --- | ---: | ---: | ---: | ---: | ---: |
| create to first command | 228 ms | 343 ms | 23.8 ms | 408 ms | 20 |

Run `2026-11-24T1950-warm-pool-on-burst-i9-13900K.json`, 2026-11-24, daemon 0.0.1, guest kernel 5.10.223.

## Create to first command, cold (pool disabled)

Every create boots a microVM.

| Measurement | Median | p95 | Min | Max | Runs |
| --- | ---: | ---: | ---: | ---: | ---: |
| create call returns | 23.6 ms | 32.6 ms | 19.2 ms | 47.5 ms | 20 |
| first command after create returns | 210 ms | 218 ms | 191 ms | 220 ms | 20 |
| create to first command | 234 ms | 244 ms | 218 ms | 245 ms | 20 |

Run `2026-11-24-warm-pool-off-paced-i9-13900K.json`, 2026-11-24, daemon 0.0.1, guest kernel 5.10.223.

## Create to first command, cold, default image (systemd)

Pool disabled, so every create boots a microVM; run alternately with the init-mode image below on the same daemon.

| Measurement | Median | p95 | Min | Max | Runs |
| --- | ---: | ---: | ---: | ---: | ---: |
| create call returns | 23.0 ms | 26.3 ms | 19.2 ms | 26.5 ms | 20 |
| first command after create returns | 213 ms | 236 ms | 200 ms | 238 ms | 20 |
| create to first command | 237 ms | 257 ms | 223 ms | 259 ms | 20 |

Run `2026-11-24T2030-default-image-cold-r3-i9-13900K.json`, 2026-11-24, daemon 0.0.1, guest kernel 5.10.223.

## Create to first command, cold, init-mode image (agent as PID 1)

The same run with an image whose `/sbin/init` is the guest agent, so systemd never starts. Opt-in per image, not the default.

| Measurement | Median | p95 | Min | Max | Runs |
| --- | ---: | ---: | ---: | ---: | ---: |
| create call returns | 23.5 ms | 26.5 ms | 18.9 ms | 26.8 ms | 20 |
| first command after create returns | 123 ms | 133 ms | 113 ms | 135 ms | 20 |
| create to first command | 144 ms | 159 ms | 133 ms | 160 ms | 20 |

Run `2026-11-24T2030-init-image-cold-r3-i9-13900K.json`, 2026-11-24, daemon 0.0.1, guest kernel 5.10.223.

## Snapshots, resume, fork, commands and parallel creates

Measured with the default pool's replenishment idle.

| Measurement | Median | p95 | Min | Max | Runs |
| --- | ---: | ---: | ---: | ---: | ---: |
| run a command in a running sandbox | 1.76 ms | 1.98 ms | 0.54 ms | 3.30 ms | 200 |
| take a snapshot | 1208 ms | 2091 ms | 1001 ms | 2615 ms | 20 |
| resume a snapshot | 13.1 ms | 14.9 ms | 8.94 ms | 15.1 ms | 20 |
| first command after a resume | 6.27 ms | 8.34 ms | 4.75 ms | 8.35 ms | 20 |
| fork a snapshot | 8.77 ms | 9.44 ms | 7.46 ms | 9.74 ms | 20 |
| first command after a fork | 7.46 ms | 8.72 ms | 5.11 ms | 8.89 ms | 20 |
| create to first command, 8 at once | 357 ms | 403 ms | 270 ms | 417 ms | 160 |
| creates per second, 8 at once | 20.1 ops/s | 21.3 | 19.1 | 21.4 | 20 |

Run `2026-11-24-i9-13900K.json`, 2026-11-24, daemon 0.0.1, guest kernel 5.10.223.

## Machine

13th Gen Intel(R) Core(TM) i9-13900K (32 threads), host kernel 6.8.0-138-generic. One shared box that also runs other work, so absolute figures vary run to run.
