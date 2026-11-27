//! JSON-RPC heartbeat on MI_00.
//!
//! The heartbeat is the exact text the vendor driver sends, including its spacing:
//! `{"jsonrpc":"2.0", "method": "heartbeat", "params": {"session-id": "<digits>"}}`, with no
//! trailing newline. The session id is a digit string chosen once per run and kept constant,
//! since a changed id makes the dock re-announce itself.
//!
//! Mechanism/policy split: `heartbeat_payload` and `heartbeat_message` are the policy (what the
//! message is, how it is framed); `run_heartbeat_loop` only schedules ticks and delegates the
//! write to `HidrawHandle`. The long-running daemon heartbeat lives in `session.rs`, on its own
//! thread next to the MI_00 reader.
//!
//! Error policy: the loop stops and returns the first failed write. Whether that is fatal
//! (validation tools) or retried (the daemon) is the caller's decision.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::framing::{MessageBuilder, ReportId};
use crate::hidraw_io::{HidrawHandle, HidrawIoError};

/// The JSON-RPC protocol version string carried in every message.
pub const JSONRPC_VERSION: &str = "2.0";

/// The JSON-RPC method name of the heartbeat.
pub const HEARTBEAT_METHOD: &str = "heartbeat";

/// Default tick interval when the config doesn't override it.
pub const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(1000);

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
    Write { tick: u64, source: HidrawIoError },
}

/// Longest accepted session id, in digits.
const SESSION_ID_MAX_LEN: usize = 20;

/// Errors from constructing a [`SessionId`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionIdError {
    /// The id was empty or longer than the accepted maximum.
    #[error("session id must be 1 to {SESSION_ID_MAX_LEN} characters, got {0}")]
    BadLength(usize),
    /// The id contained a character that is not an ASCII digit.
    #[error("session id must contain only ASCII digits")]
    NotDigits,
}

/// The heartbeat `session-id`: a non-empty string of ASCII digits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(String);

impl SessionId {
    /// Validates a session id string.
    pub fn new(id: impl Into<String>) -> Result<Self, SessionIdError> {
        let id = id.into();
        if id.is_empty() || id.len() > SESSION_ID_MAX_LEN {
            return Err(SessionIdError::BadLength(id.len()));
        }
        if !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(SessionIdError::NotDigits);
        }
        Ok(Self(id))
    }

    /// A fresh id from the wall clock in milliseconds, reduced to eight digits like the vendor
    /// driver's tick-count ids.
    pub fn generate() -> Self {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_millis());
        Self(((millis % 100_000_000) + 10_000_000).to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Builds the exact heartbeat text for a session id.
pub fn heartbeat_payload(session_id: &SessionId) -> Vec<u8> {
    format!(
        r#"{{"jsonrpc":"2.0", "method": "{HEARTBEAT_METHOD}", "params": {{"session-id": "{}"}}}}"#,
        session_id.as_str()
    )
    .into_bytes()
}

/// Builds the wire-ready report sequence (Report ID 1, zero width and height) for a heartbeat.
pub fn heartbeat_message(session_id: &SessionId) -> Vec<Vec<u8>> {
    MessageBuilder::control()
        .payload(&heartbeat_payload(session_id))
        .build()
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
    handle: &mut HidrawHandle,
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
    use crate::framing::{parse_chunks, PictureSize, REPORT_SIZE};
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom};

    fn id(s: &str) -> SessionId {
        SessionId::new(s).unwrap()
    }

    #[test]
    fn payload_is_the_exact_vendor_text() {
        assert_eq!(
            heartbeat_payload(&id("25282281")),
            br#"{"jsonrpc":"2.0", "method": "heartbeat", "params": {"session-id": "25282281"}}"#
        );
        // The length field is 78 for an eight digit id, as captured.
        assert_eq!(heartbeat_payload(&id("25282281")).len(), 78);
    }

    #[test]
    fn session_id_validation() {
        assert!(SessionId::new("").is_err());
        assert_eq!(SessionId::new("12a"), Err(SessionIdError::NotDigits));
        assert!(SessionId::new("1".repeat(21)).is_err());
        let generated = SessionId::generate();
        assert!(SessionId::new(generated.as_str()).is_ok());
    }

    #[test]
    fn message_is_single_control_chunk() {
        let chunks = heartbeat_message(&id("7"));
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), REPORT_SIZE);
        let parsed = parse_chunks(&chunks).unwrap();
        assert_eq!(parsed.report_id, ReportId::Control);
        assert_eq!(parsed.size, PictureSize::zero());
        assert_eq!(parsed.payload, heartbeat_payload(&id("7")));
        let value: serde_json::Value = serde_json::from_slice(&parsed.payload).unwrap();
        assert_eq!(value["method"], "heartbeat");
        assert_eq!(value["params"]["session-id"], "7");
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
