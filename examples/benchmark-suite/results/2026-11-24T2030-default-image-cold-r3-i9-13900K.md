## Environment

- Date: 2026-11-24T15:00:06.470Z
- Daemon: version 0.0.1, http://127.0.0.1:7792
- Hardware: i9-13900K
- Host kernel: 6.8.0-138-generic
- Host CPU: 13th Gen Intel(R) Core(TM) i9-13900K (32 threads)
- Guest kernel: 5.10.223
- Rootfs clones so far: reflink=84, copy=0
- Live sandboxes at start: 0
- Options: iterations=20, pause-ms=0, concurrency=8, exec-samples=200, image=default

## Results

| metric | median | p95 | p99 | min | max | n |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| create_call | 22.99ms | 26.29ms | 26.44ms | 19.24ms | 26.48ms | 20 |
| first_command_after_create | 213.4ms | 236.0ms | 237.4ms | 200.3ms | 237.7ms | 20 |
| create_to_first_command | 237.0ms | 257.1ms | 258.3ms | 222.8ms | 258.6ms | 20 |
