//! JSON-RPC heartbeat on MI_00.
//!
//! Per `docs/research/full-protocol-spec-2026-11-21.md`, MI_00 is a JSON-RPC control channel
//! (HID Report ID 1) whose confirmed method is
//! `{"jsonrpc":"2.0","method":"heartbeat","params":{"session-id":"<tick-count>"}}`, sent
//! roughly once per second.
//!
//! Mechanism/policy split: `heartbeat_payload` and `heartbeat_message` are the policy (what the
//! message is, how it is framed); `run_heartbeat_loop` only schedules ticks and delegates the
//! write to `HidrawHandle`.
//!
//! Error policy: the loop stops and returns the first failed write. Whether that is fatal
//! (validation tools) or retried (the daemon) is the caller's decision.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
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

/// Builds the JSON-RPC heartbeat payload bytes for a given tick.
pub fn heartbeat_payload(tick: u64) -> Vec<u8> {
    let message = json!({
        "jsonrpc": JSONRPC_VERSION,
        "method": HEARTBEAT_METHOD,
        "params": { "session-id": tick.to_string() },
    });
    // Serializing a `json!` value cannot fail (string keys, no non-finite floats).
    serde_json::to_vec(&message).expect("heartbeat JSON serialization is infallible")
}

/// Builds the wire-ready chunk sequence (Report ID 1, zero width and height) for a given tick.
pub fn heartbeat_message(tick: u64) -> Vec<Vec<u8>> {
    MessageBuilder::control()
        .payload(&heartbeat_payload(tick))
        .build()
}

/// Summary of a finished heartbeat loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeartbeatStats {
    /// Number of heartbeat messages written.
    pub ticks_sent: u64,
}

/// Sends a heartbeat on `handle` every `interval`, starting immediately with tick 1.
///
/// Ticks are scheduled against a fixed deadline (`start + n * interval`) so write time does not
/// accumulate as drift. The loop ends when `stop` becomes true, when `max_ticks` messages have
/// been sent, or on the first write error.
pub fn run_heartbeat_loop(
    handle: &mut HidrawHandle,
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
            .write_chunks(&heartbeat_message(tick))
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

    #[test]
    fn payload_has_documented_shape() {
        let value: serde_json::Value = serde_json::from_slice(&heartbeat_payload(7)).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["method"], "heartbeat");
        assert_eq!(value["params"]["session-id"], "7");
    }

    #[test]
    fn message_is_single_control_chunk() {
        let chunks = heartbeat_message(1);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), REPORT_SIZE);
        let parsed = parse_chunks(&chunks).unwrap();
        assert_eq!(parsed.report_id, ReportId::Control);
        assert_eq!(parsed.size, PictureSize::zero());
        assert_eq!(parsed.payload, heartbeat_payload(1));
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
    fn loop_sends_requested_ticks_with_increasing_session_id() {
        let (mut handle, _dir) = control_loopback();
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        let stats =
            run_heartbeat_loop(&mut handle, Duration::from_millis(20), &stop, Some(3)).unwrap();
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
        assert_eq!(ids, ["1", "2", "3"]);
    }

    #[test]
    fn loop_honours_stop_flag() {
        let (mut handle, _dir) = control_loopback();
        let stop = AtomicBool::new(true);
        let stats = run_heartbeat_loop(&mut handle, Duration::from_secs(60), &stop, None).unwrap();
        assert_eq!(stats.ticks_sent, 0);
    }

    #[test]
    fn rejects_video_channel_handle() {
        let dir = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(dir.path().join("x")).unwrap();
        let mut handle = HidrawHandle::from_file(file, ReportId::Video);
        let stop = AtomicBool::new(false);
        let err =
            run_heartbeat_loop(&mut handle, Duration::from_millis(1), &stop, Some(1)).unwrap_err();
        assert!(matches!(err, HeartbeatError::WrongChannel { .. }));
    }
}
