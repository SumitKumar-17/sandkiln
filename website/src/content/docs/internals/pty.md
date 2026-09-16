---
title: "Internals: PTY and forkpty"
description: What a pseudo-terminal actually is, and why an interactive shell needs a raw byte passthrough instead of sandkiln's usual request/response protocol.
---

## What it is

A **pseudo-terminal** (PTY) is a pair of virtual devices, a *master* and a *slave*, that together make a program think it's talking to a real physical terminal even when nothing physical is involved. A shell running on the slave end behaves exactly as it would on a real console: it prints a prompt, supports line editing, handles Ctrl+C by sending a real `SIGINT`, and so on. Whatever writes to the master end (an SSH client, a terminal emulator, or here, sandkiln's own guest agent) can read what the shell prints and send it keystrokes, byte for byte.

**`forkpty(2)`** is the POSIX call that sets this up in one step: it forks a child process, allocates a fresh PTY pair, connects the child's stdin/stdout/stderr to the slave side, and makes it the session leader of that terminal, all before the child execs the actual shell. The parent gets back a single file descriptor for the master side.

## Why sandkiln uses it here

Batch `exec` (one request, one response, a full `stdout`/`stderr`/`exit_code` blob once the command finishes) is the wrong shape for an interactive shell: there's no single "response" for a session where the user is still typing. A PTY session instead needs a live, bidirectional stream of raw bytes for as long as the connection stays open, which is a fundamentally different transport shape from sandkiln's framed `Request`/`Response` protocol (see [Internals: exec-stream](../exec-stream/) for the closest relative, and how it differs). A WebSocket already models exactly this shape, an open, bidirectional, message-framed connection, so `GET /sandboxes/:id/pty` upgrades to one and simply relays raw bytes both directions afterward.

## Key terms

- **Session leader**: the process `forkpty` makes the shell into, meaning the terminal is now "its" controlling terminal. This is what lets signals like `SIGHUP` (see below) mean anything to it.
- **`SIGHUP`**: the signal traditionally sent to every process attached to a terminal when that terminal goes away, historically, a modem hanging up. sandkiln's guest agent sends this to the shell's whole process group when the WebSocket closes *before* the shell itself exited, so a lost connection or a closed browser tab doesn't leave an orphaned shell running forever inside the guest.
- **Raw mode**: the CLI's `kiln sandbox pty` puts the *caller's own* local terminal into raw mode for the session, meaning keystrokes (including Ctrl+C, Ctrl+D) are sent to the remote shell immediately and unprocessed, rather than the local terminal doing its own line-editing/interpretation first. Without this, Ctrl+C would kill the local `kiln` process instead of interrupting whatever's running remotely.

## How it works in sandkiln

The daemon's `GET /sandboxes/:id/pty[?cols=&rows=]` upgrades the HTTP connection to a WebSocket, then opens a second, dedicated vsock connection to the guest agent, on `PTY_PORT`, a completely separate port from `AGENT_PORT` (which still carries ordinary `exec`/file-operation `Request`/`Response` traffic for the same sandbox at the same time). Exactly one framed message crosses this connection before it becomes a raw passthrough: a `PtyHandshake { cols, rows }`, telling the guest what terminal size to allocate. From that point on, every byte from the WebSocket goes to the pty master, and every byte from the pty master goes to the WebSocket, in both directions, until either side closes.

Both hangup directions are handled explicitly: if the shell exits first (the user typed `exit`), the guest agent notices the child process ended and closes cleanly. If the WebSocket closes first (a lost connection, a closed tab), the guest agent sends `SIGHUP` to the shell's process group, then reaps it with `waitpid`, so nothing is left running orphaned inside the guest.

## See it in action

A small script (matching this repo's own `scripts/lib/pty-check.mjs`, used by its integration test suite) opens a real PTY session, sends a command, and captures the raw bytes:

```
$ node pty_demo.mjs http://127.0.0.1:7777 <sandbox-id>

RAW BYTES RECEIVED:
"root@ubuntu-fc-uvm:/# echo hello-from-a-real-pty\r\nhello-from-a-real-pty\r\nroot@ubuntu-fc-uvm:/# exit\r\nexit\r\n"
```

Two details worth noticing in that output: the shell's own prompt (`root@ubuntu-fc-uvm:/#`) appears in the stream, since this is a real interactive terminal, not a stripped-down command channel, and every line ends in `\r\n`, not a bare `\n`, exactly the line-ending convention a real terminal uses. Neither of those is something the client sent; both come straight from the shell running against a genuine pty slave.
