//! Paced heartbeat loop for the hardware validation tools.
//!
//! Mechanism/policy split: `protocol::heartbeat` is the policy (what the message is, how it is
//! framed); `run_heartbeat_loop` only schedules ticks and delegates the write to
//! `DockTransport`. The long-running daemon heartbeat lives in `core::session`, on its own thread
//! next to the MI_00 reader.
//!
//! Error policy: the loop stops and returns the first failed write. Whether that is fatal
//! (validation tools) or retried (the daemon) is the caller's decision.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::ports::transport::{DockTransport, TransportError};
use crate::protocol::framing::ReportId;
use crate::protocol::heartbeat::{heartbeat_message, SessionId};

/// Longest single sleep inside the loop, so a stop request is noticed promptly.
const STOP_POLL_SLICE: Duration = Duration::from_millis(50);

/// Errors from the heartbeat loop.
#[derive(Debug, Error)]
pub enum HeartbeatError {
    /// The handle passed in isn't the MI_00 control channel.
    #[error("heartbeat needs the control (MI_00) channel, got {got:?}")]
    WrongChannel { got: ReportId },
    /// Writing a heartbeat message failed.
    #[error("heartbeat tick {tick} write failed: {source}")]
    Write { tick: u64, source: TransportError },
}

/// Summary of a finished heartbeat loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeartbeatStats {
    /// Number of heartbeat messages written.
    pub ticks_sent: u64,
}

/// Sends a heartbeat with a constant `session_id` on `handle` every `interval`, starting
/// immediately.
///
/// Ticks are scheduled against a fixed deadline (`start + n * interval`) so write time does not
/// accumulate as drift. The loop ends when `stop` becomes true, when `max_ticks` messages have
/// been sent, or on the first write error.
pub fn run_heartbeat_loop(
    handle: &mut dyn DockTransport,
    session_id: &SessionId,
    interval: Duration,
    stop: &AtomicBool,
    max_ticks: Option<u64>,
) -> Result<HeartbeatStats, HeartbeatError> {
    if handle.channel() != ReportId::Control {
        return Err(HeartbeatError::WrongChannel {
            got: handle.channel(),
        });
    }

    let start = Instant::now();
    let mut ticks_sent = 0u64;
    loop {
        if stop.load(Ordering::Relaxed) || max_ticks.is_some_and(|max| ticks_sent >= max) {
            return Ok(HeartbeatStats { ticks_sent });
        }

        let tick = ticks_sent + 1;
        handle
            .write_chunks(&heartbeat_message(session_id))
            .map_err(|source| HeartbeatError::Write { tick, source })?;
        ticks_sent = tick;

        if max_ticks.is_some_and(|max| ticks_sent >= max) {
            return Ok(HeartbeatStats { ticks_sent });
        }

        let deadline = start + interval * u32::try_from(ticks_sent).unwrap_or(u32::MAX);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if stop.load(Ordering::Relaxed) {
                return Ok(HeartbeatStats { ticks_sent });
            }
            std::thread::sleep(left.min(STOP_POLL_SLICE));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::transport::hidraw::HidrawHandle;
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom};

    fn id(s: &str) -> SessionId {
        SessionId::new(s).unwrap()
    }

    fn control_loopback() -> (HidrawHandle, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(dir.path().join("mi00"))
            .unwrap();
        (HidrawHandle::from_file(file, ReportId::Control), dir)
    }

    #[test]
    fn loop_sends_requested_ticks_with_a_constant_session_id() {
        let (mut handle, _dir) = control_loopback();
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        let stats = run_heartbeat_loop(
            &mut handle,
            &id("42"),
            Duration::from_millis(20),
            &stop,
            Some(3),
        )
        .unwrap();
        assert_eq!(stats.ticks_sent, 3);
        // Two inter-tick waits of 20 ms each.
        assert!(started.elapsed() >= Duration::from_millis(40));

        let mut ids = Vec::new();
        handle.file_for_test().seek(SeekFrom::Start(0)).unwrap();
        for _ in 0..3 {
            let parsed = handle.read_message().unwrap();
            let value: serde_json::Value = serde_json::from_slice(&parsed.payload).unwrap();
            ids.push(value["params"]["session-id"].as_str().unwrap().to_string());
        }
        assert_eq!(ids, ["42", "42", "42"]);
    }

    #[test]
    fn loop_honours_stop_flag() {
        let (mut handle, _dir) = control_loopback();
        let stop = AtomicBool::new(true);
        let stats = run_heartbeat_loop(&mut handle, &id("1"), Duration::from_secs(60), &stop, None)
            .unwrap();
        assert_eq!(stats.ticks_sent, 0);
    }

    #[test]
    fn rejects_video_channel_handle() {
        let dir = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(dir.path().join("x")).unwrap();
        let mut handle = HidrawHandle::from_file(file, ReportId::Video);
        let stop = AtomicBool::new(false);
        let err = run_heartbeat_loop(
            &mut handle,
            &id("1"),
            Duration::from_millis(1),
            &stop,
            Some(1),
        )
        .unwrap_err();
        assert!(matches!(err, HeartbeatError::WrongChannel { .. }));
    }
}
