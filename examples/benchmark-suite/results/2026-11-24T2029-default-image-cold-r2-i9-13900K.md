## Environment

- Date: 2026-11-24T14:59:56.020Z
- Daemon: version 0.0.1, http://127.0.0.1:7792
- Hardware: i9-13900K
- Host kernel: 6.8.0-138-generic
- Host CPU: 13th Gen Intel(R) Core(TM) i9-13900K (32 threads)
- Guest kernel: 5.10.223
- Rootfs clones so far: reflink=42, copy=0
- Live sandboxes at start: 0
- Options: iterations=20, pause-ms=0, concurrency=8, exec-samples=200, image=default

## Results

| metric | median | p95 | p99 | min | max | n |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| create_call | 20.12ms | 24.75ms | 26.62ms | 17.47ms | 27.09ms | 20 |
| first_command_after_create | 207.4ms | 230.8ms | 233.6ms | 196.0ms | 234.3ms | 20 |
| create_to_first_command | 228.8ms | 250.7ms | 259.2ms | 217.6ms | 261.4ms | 20 |
