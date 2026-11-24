## Environment

- Date: 2026-11-24T14:59:45.339Z
- Daemon: version 0.0.1, http://127.0.0.1:7792
- Hardware: i9-13900K
- Host kernel: 6.8.0-138-generic
- Host CPU: 13th Gen Intel(R) Core(TM) i9-13900K (32 threads)
- Guest kernel: 5.10.223
- Rootfs clones so far: reflink=0, copy=0
- Live sandboxes at start: 0
- Options: iterations=20, pause-ms=0, concurrency=8, exec-samples=200, image=default

## Results

| metric | median | p95 | p99 | min | max | n |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| create_call | 22.08ms | 26.41ms | 32.82ms | 18.61ms | 34.43ms | 20 |
| first_command_after_create | 210.5ms | 235.2ms | 253.5ms | 191.7ms | 258.0ms | 20 |
| create_to_first_command | 233.7ms | 261.1ms | 277.3ms | 210.5ms | 281.3ms | 20 |
