"""Streamed background exec in Python: start a command detached, watch its
output live, then re-attach after it has finished and get the whole log
back at once. Python counterpart to ../exec-stream-logs (JS)."""

import time

from sandkiln import Sandbox

STEPS = 6
# A stand-in for something long-running worth watching (a build, a test
# run, a migration): several lines over several seconds, so a live tail and
# a replay are visibly different.
SCRIPT = f'for i in $(seq 1 {STEPS}); do echo "[step $i/{STEPS}] building module-$i"; sleep 1; done; echo "build finished"'


def indent(text: str) -> str:
    return "\n".join(f"    | {line}" for line in text.rstrip().split("\n"))


def main() -> None:
    print("Creating sandbox...")
    sandbox = Sandbox.create(tags={"example": "exec-stream-logs-python"})
    print(f"Sandbox {sandbox.id} ready.")
    try:
        session_id = sandbox.exec_stream("sh", ["-c", SCRIPT])
        print(f"Started background session {session_id}: exec_stream() returned immediately; the command is still running.\n")

        print("Attaching while it runs (replay of anything captured so far, then a live tail):")
        started = time.monotonic()
        live = b""
        with sandbox.attach_logs(session_id) as logs:
            for message in logs:  # bytes of output; str for the daemon's notices
                text = message if isinstance(message, str) else message.decode("utf-8", errors="replace")
                live += text.encode()
                print(indent(text))
        live_seconds = time.monotonic() - started
        print(f"  ...attached for {live_seconds:.1f}s, exit code {logs.exit_code}.\n")

        (session,) = sandbox.list_exec_streams()
        print(f"list_exec_streams(): {session.command} {' '.join(session.args[:1])} ..., exit code {session.exit_code}, started {session.started_at.isoformat()}.\n")

        print("Re-attaching to the same, now-finished session:")
        started = time.monotonic()
        replay = sandbox.attach_logs(session_id).read_all()  # LogOutput(data, notices, exit_code)
        replay_ms = (time.monotonic() - started) * 1000
        print(indent(replay.data.decode("utf-8", errors="replace")))
        print(f"  ...{len(replay.data)} bytes in {replay_ms:.0f}ms, exit code {replay.exit_code}.\n")

        print(
            f"The re-attach returned the full log in {replay_ms:.0f}ms instead of the {live_seconds:.0f}s the live attach took, "
            "because the daemon buffers a session's output independently of any one connection."
        )
    finally:
        sandbox.stop(keep=False)
        print(f"\nSandbox {sandbox.id} stopped.")


if __name__ == "__main__":
    main()
