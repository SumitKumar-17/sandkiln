# Load testing: finding the concurrency ceiling

Planned, not started.

`scripts/load-test.sh` reports concurrency and latency for one fixed value
chosen up front; it does not find the box's ceiling. The next version should
ramp concurrency (for example 4, 8, 16, 32 and onward) until something
degrades: p95 spiking, a real error rate, or a resource running out (tap
pool, configured vCPU/memory ceiling, file descriptors). That point is the
number worth publishing.

Also needed: how a cold create's latency degrades as concurrency rises.
Every number in [benchmarks](../benchmarks.md) is one sandbox at a time;
nothing has measured lock contention in the daemon's in-memory state or a
tap pool running low. Output should be a kept report, reusable like
`scripts/bench-report.sh`, not a terminal scroll.

Baseline for reference: 4 workers x 5 cycles, 0 errors, create mean 211ms
(p95 577ms), exec mean 366ms, delete mean 131ms, 5.38 cycles/sec (measured
before several later fixes).
