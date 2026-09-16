---
title: "Internals: exec-stream sessions"
description: Why a detached background command needs its own buffering model, distinct from both batch exec and a PTY.
---

## What it is

"Run this command, then let me watch its output whenever I feel like checking, from the beginning if I've never looked yet, live if it's still running, and identically if I check again after it's already finished" is a shape that neither of sandkiln's other two execution modes covers. Batch `exec` blocks until the command finishes and hands back one blob; a PTY session is a live interactive terminal, tied to exactly one connection for its whole life. exec-stream is a third, distinct shape: a **detached background process** whose output the daemon captures independently of whether anyone is currently watching, so that "watching" and "running" are decoupled entirely.

## Why sandkiln uses it here

This is the mechanism behind `kiln sandbox exec-stream` / `kiln logs`: starting a build, a long-running server, or any other multi-second command without holding one HTTP request open for its whole duration, and being able to check in on it, from any number of separate connections, at any point, including well after it's already finished. A CI runner disconnecting and reconnecting, or two different people wanting to watch the same build, are both the same case: neither should re-run the command or lose the output that happened before they attached.

## Key terms

- **Replay-then-live-tail**: what attaching to a session actually delivers, everything captured so far, sent immediately, followed by a live stream of anything new. Not two separate modes; one continuous behavior a client can't tell apart from "I've been watching the whole time."
- **`LogSession`**: the daemon-side object holding one session's state, a bounded ring buffer of captured output (`BUFFER_CAP_BYTES`, oldest bytes dropped once exceeded, with `truncated_bytes` tracking how much so a late attacher can be told some history is gone) plus a `tokio::sync::broadcast` channel that fans the live tail out to however many WebSocket clients are currently attached.
- **`EXEC_STREAM_PORT`**: a third dedicated vsock port, alongside `AGENT_PORT` (batch exec/file ops) and `PTY_PORT` (interactive sessions) -- one guest connection per background session, carrying framed `ExecStreamEvent`s (`Stdout`, `Stderr`, and a final `Exit`) rather than raw bytes.

## How it works in sandkiln

`POST /sandboxes/:id/exec-stream` opens a connection to the guest on `EXEC_STREAM_PORT`, sends one `ExecStreamHandshake` naming the command, and returns a session id immediately without waiting for anything to finish. A background "pump" task on the daemon side keeps reading framed events off that vsock connection for as long as the guest process runs, writing each one into the session's `LogSession` (both into the ring buffer, for future replay, and out through the broadcast channel, for anyone currently attached), independent of whether any WebSocket client is connected at that exact moment.

`GET /sandboxes/:id/exec-stream/:id/logs` (a WebSocket) is what a client actually attaches with: on connect, the daemon sends the ring buffer's current contents as a burst, then keeps the connection open and forwards anything new from the broadcast channel. A second, later attach to the same session id gets the same replay, now including everything that happened while nobody was watching, which is the entire point.

Not carried across resume, fork, or a restored checkpoint, and doesn't survive a daemon restart, this is in-memory daemon-process state tied to one specific session, not guest state Firecracker's own snapshot mechanism would capture.

## See it in action

Starting a session and attaching to it *while it's still running*:

```
$ curl -s -X POST http://127.0.0.1:7777/sandboxes/<id>/exec-stream \
    -H 'content-type: application/json' \
    -d '{"command":"sh","args":["-c","for i in 1 2 3; do echo line-$i; sleep 1; done"]}'

{"id":"3e161c59-c3c3-4103-9022-9e87cefeaade"}
```

Attaching immediately (before the 3-second command has finished):

```
FIRST ATTACH (mid-run, replay + live tail): "line-1\nline-2\nline-3\n[process exited with code 0]\n"
```

Attaching again afterward, to the same session id:

```
SECOND ATTACH (after finish, full replay): "line-1\nline-2\nline-3\n[process exited with code 0]\n"
```

Both attaches saw the identical complete log, the second one entirely from the daemon's own buffer, with no guest connection still open by the time it connected. One real gotcha worth knowing if you're writing a client by hand: buffered replay frames arrive as a WebSocket `Blob` by default in a browser or Node's native `WebSocket`, not a string or `ArrayBuffer`, set `binaryType = "arraybuffer"` before reading `event.data`, or a naive `console.log(event.data)` just prints `Blob {...}` instead of the actual bytes.
