//! `LogSession`: daemon-side state for one streamed background exec
//! session (`kiln logs -f` — see `crate::routes_logs` for the full
//! design). Own file, not part of `routes_logs.rs`: pure state a
//! background pump task mutates and any number of WebSocket clients read
//! concurrently, no HTTP/axum concern — same state/surface seam as
//! `pool.rs`/`routes_pool.rs`.
//!
//! stdout/stderr merge into one buffer in arrival order (like a real
//! terminal), even though the wire protocol still tags each chunk by
//! stream on the way in. Bounded ring buffer: holds only the last
//! [`BUFFER_CAP_BYTES`], older bytes dropped and counted in
//! `truncated_bytes` so a late attacher is told history is missing rather
//! than handed a silent partial log. Not persisted, not carried across
//! resume/fork/restore — same as `Sandbox::pty_session_count`, a resumed
//! sandbox is a *new* `Sandbox` value even though the guest's memory
//! carries over, so every session it held ends with it or a daemon
//! restart.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::SystemTime;
use tokio::sync::broadcast;

/// Kept modest on purpose — this is meant to cover "what did this
/// command already print," not serve as a general log-storage system.
/// 1 MiB is enough for a very large amount of ordinary CLI/build output
/// while staying cheap to hold per session, per sandbox.
const BUFFER_CAP_BYTES: usize = 1024 * 1024;

/// Broadcast channel capacity, in messages (each chunk from the guest,
/// pre-merge — see `pump_exec_stream`), not bytes. Only matters for a
/// slow live subscriber: the replay buffer above is what makes
/// reattaching after falling behind actually work, so this just needs to
/// be large enough that an ordinary transient slowdown doesn't trigger a
/// `Lagged` error, not large enough to *be* the replay mechanism itself.
const BROADCAST_CAPACITY: usize = 1024;

/// One chunk of new data, or the final exit — what a live subscriber
/// receives after the initial replay. `Data` is deliberately not
/// `Bytes`/`Arc<[u8]>`-shared: chunks are small (bounded by the guest's
/// own read buffer size) and this project doesn't otherwise reach for a
/// shared-buffer type, so a plain owned `Vec<u8>` per broadcast message
/// stays consistent with everything else here.
#[derive(Clone, Debug)]
pub enum LogEvent {
    Data(Vec<u8>),
    Exit(i32),
}

struct Inner {
    buffer: VecDeque<u8>,
    truncated_bytes: u64,
    exit_code: Option<i32>,
    sender: broadcast::Sender<LogEvent>,
}

pub struct LogSession {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    pub started_at: SystemTime,
    inner: Mutex<Inner>,
}

impl LogSession {
    pub fn new(id: String, command: String, args: Vec<String>) -> Self {
        let (sender, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            id,
            command,
            args,
            started_at: SystemTime::now(),
            inner: Mutex::new(Inner { buffer: VecDeque::new(), truncated_bytes: 0, exit_code: None, sender }),
        }
    }

    /// Appends one chunk of (already stream-merged) output. A no-op past
    /// `finish()` in practice, since nothing calls this again once the
    /// pump task has already sent an `Exit` and returned — not enforced
    /// here, just how `routes_logs::pump_exec_stream` actually drives it.
    pub fn append(&self, data: &[u8]) {
        let mut inner = self.inner.lock().unwrap();
        inner.buffer.extend(data.iter().copied());
        let cap = BUFFER_CAP_BYTES;
        if inner.buffer.len() > cap {
            let overflow = inner.buffer.len() - cap;
            inner.buffer.drain(0..overflow);
            inner.truncated_bytes += overflow as u64;
        }
        // No receivers is the common case (no client currently attached)
        // and not an error -- the buffer above is what makes a *later*
        // attach see this data; broadcast is purely for whoever's
        // already watching live.
        let _ = inner.sender.send(LogEvent::Data(data.to_vec()));
    }

    /// Marks the session finished. Idempotent in effect (a second call
    /// would just overwrite the same `Some`), but `routes_logs`'s pump
    /// task only ever calls this once, right before it returns.
    pub fn finish(&self, exit_code: i32) {
        let mut inner = self.inner.lock().unwrap();
        inner.exit_code = Some(exit_code);
        let _ = inner.sender.send(LogEvent::Exit(exit_code));
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.inner.lock().unwrap().exit_code
    }

    /// Replay-then-live-tail: everything currently buffered, plus a
    /// receiver for everything appended from this point on. Snapshotting
    /// the buffer and subscribing happen under the same lock `append`/
    /// `finish` take, so no byte is ever missed or double-delivered.
    pub fn subscribe(&self) -> (Vec<u8>, u64, Option<i32>, broadcast::Receiver<LogEvent>) {
        let inner = self.inner.lock().unwrap();
        let replay: Vec<u8> = inner.buffer.iter().copied().collect();
        (replay, inner.truncated_bytes, inner.exit_code, inner.sender.subscribe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_then_subscribe_replays_everything_buffered_so_far() {
        let session = LogSession::new("s1".to_string(), "tail".to_string(), vec!["-f".to_string()]);
        session.append(b"hello ");
        session.append(b"world");

        let (replay, truncated, exit_code, _rx) = session.subscribe();
        assert_eq!(replay, b"hello world");
        assert_eq!(truncated, 0);
        assert_eq!(exit_code, None);
    }

    #[test]
    fn subscribe_before_any_append_replays_nothing() {
        let session = LogSession::new("s1".to_string(), "echo".to_string(), vec![]);
        let (replay, truncated, exit_code, _rx) = session.subscribe();
        assert!(replay.is_empty());
        assert_eq!(truncated, 0);
        assert_eq!(exit_code, None);
    }

    #[test]
    fn append_beyond_the_cap_drops_from_the_front_and_tracks_truncation() {
        let session = LogSession::new("s1".to_string(), "yes".to_string(), vec![]);
        session.append(&vec![b'a'; BUFFER_CAP_BYTES]);
        session.append(b"tail-end");

        let (replay, truncated, _, _rx) = session.subscribe();
        assert_eq!(replay.len(), BUFFER_CAP_BYTES);
        assert!(replay.ends_with(b"tail-end"));
        assert_eq!(truncated, 8);
    }

    #[test]
    fn finish_records_the_exit_code_and_is_visible_to_a_later_subscriber() {
        let session = LogSession::new("s1".to_string(), "true".to_string(), vec![]);
        session.append(b"done\n");
        session.finish(0);

        assert_eq!(session.exit_code(), Some(0));
        let (replay, _, exit_code, _rx) = session.subscribe();
        assert_eq!(replay, b"done\n");
        assert_eq!(exit_code, Some(0));
    }

    #[test]
    fn a_live_subscriber_receives_appends_and_the_final_exit() {
        let session = LogSession::new("s1".to_string(), "sh".to_string(), vec![]);
        let (_replay, _, _, mut rx) = session.subscribe();

        session.append(b"line one\n");
        session.finish(7);

        match rx.try_recv() {
            Ok(LogEvent::Data(data)) => assert_eq!(data, b"line one\n"),
            other => panic!("expected a Data event, got {other:?}"),
        }
        match rx.try_recv() {
            Ok(LogEvent::Exit(code)) => assert_eq!(code, 7),
            other => panic!("expected an Exit event, got {other:?}"),
        }
    }

    #[test]
    fn attaching_after_finish_still_reports_the_exit_code_and_full_replay() {
        let session = LogSession::new("s1".to_string(), "sh".to_string(), vec![]);
        session.append(b"only line\n");
        session.finish(1);

        let (replay, _, exit_code, _rx) = session.subscribe();
        assert_eq!(replay, b"only line\n");
        assert_eq!(exit_code, Some(1));
    }
}
