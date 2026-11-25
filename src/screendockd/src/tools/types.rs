//! Types of the validation and measurement tools: reports, statistics, parameters and errors.

use std::time::Duration;

use thiserror::Error;

use crate::core::session::{SessionError, SessionSettings, SessionStats};
use crate::ports::capture::CaptureError;
use crate::ports::encode::EncodeError;
use crate::ports::transport::TransportError;
use crate::protocol::framing::{PictureSize, ReportId};

/// Errors that abort a measurement.
#[derive(Debug, Error)]
pub enum BenchError {
    #[error("capture failed: {0}")]
    Capture(#[from] CaptureError),
    #[error("encode failed: {0}")]
    Encode(#[from] EncodeError),
    #[error("getrusage failed: {0}")]
    Rusage(std::io::Error),
}

/// Process CPU time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub user: Duration,
    pub system: Duration,
}

/// Result of one measurement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchReport {
    pub frames: u64,
    pub messages: u64,
    pub encoded_bytes: u64,
    pub wall: Duration,
    pub cpu: CpuTimes,
    /// Wall time spent inside `next_frame` (includes pacing waits).
    pub capture_wall: Duration,
    /// Wall time spent inside `encode`.
    pub encode_wall: Duration,
    /// Bytes of each frame's messages, in order.
    pub frame_bytes: Vec<u64>,
    /// Bytes of each keyframe message, in order.
    pub keyframe_bytes: Vec<u64>,
    /// Bytes of each non-keyframe message, in order.
    pub inter_bytes: Vec<u64>,
}

/// Summary of a list of sizes in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeStats {
    pub count: usize,
    pub min: u64,
    pub median: u64,
    pub p95: u64,
    pub max: u64,
}

/// Longest single sleep inside the loop, so a stop request is noticed promptly.
pub(super) const STOP_POLL_SLICE: Duration = Duration::from_millis(50);

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

/// Per-chunk write latency the PowerShell/.NET prototype achieved, in milliseconds. The
/// write-timing exit criterion is at or below this figure.
pub const REFERENCE_MS_PER_CHUNK: f64 = 0.77;

/// Size of the research's keyframe bundle fixture (SPS+PPS+SEI+IDR), in bytes.
pub const SYNTHETIC_KEYFRAME_LEN: usize = 3577;

/// Size of the research's 17-chunk inter-frame fixture, in bytes.
pub const SYNTHETIC_INTERFRAME_LEN: usize = 67_666;

/// Longest single sleep while waiting, so deadlines and stop requests stay responsive.
pub(super) const SLEEP_SLICE: Duration = Duration::from_millis(20);

/// Errors from the validation routines.
#[derive(Debug, Error)]
pub enum ValidationError {
    /// A hidraw write failed.
    #[error("hidraw I/O failed: {0}")]
    Io(#[from] TransportError),
    /// The heartbeat loop failed.
    #[error("heartbeat failed: {0}")]
    Heartbeat(#[from] HeartbeatError),
    /// The caller passed unusable parameters.
    #[error("invalid validation input: {0}")]
    InvalidInput(String),
    /// The heartbeat thread panicked.
    #[error("heartbeat thread panicked")]
    HeartbeatPanicked,
    /// The dock session failed to start or ended.
    #[error("session failed: {0}")]
    Session(#[from] SessionError),
    /// The session did not become armed in time.
    #[error("dock session not armed within {0:?}")]
    NotArmed(Duration),
    /// The session ended for good.
    #[error("dock session ended: {0}")]
    SessionEnded(String),
}

/// Summary statistics over a set of durations.
#[derive(Debug, Clone, PartialEq)]
pub struct TimingStats {
    pub count: usize,
    pub min: Duration,
    pub max: Duration,
    pub mean: Duration,
    pub p50: Duration,
    pub p95: Duration,
    pub p99: Duration,
}

/// Result of a replay run.
#[derive(Debug, Clone)]
pub struct ReplayReport {
    pub messages_sent: usize,
    pub chunks_sent: usize,
    /// Stats over every individual `write(2)` call; `None` if nothing was sent.
    pub chunk_stats: Option<TimingStats>,
    /// Stats over per-message total write time (first write start to last write end).
    pub message_stats: Option<TimingStats>,
    /// Wall time per message divided by its chunk count, averaged, in milliseconds. This is
    /// the figure comparable to `REFERENCE_MS_PER_CHUNK`, since it includes inter-write gaps.
    pub mean_ms_per_chunk_wall: f64,
}

/// The three isolation conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolateMode {
    /// Heartbeat on MI_00 only, no video.
    Heartbeat,
    /// Video on MI_01 only, no heartbeat.
    Video,
    /// Both concurrently.
    Both,
}

/// Parameters for an isolation run.
#[derive(Debug, Clone)]
pub struct IsolateParams {
    pub mode: IsolateMode,
    pub duration: Duration,
    pub heartbeat_interval: Duration,
    pub size: PictureSize,
    /// MI_00 session settings. `Some` runs the dock session (handshake, reader, heartbeat) on
    /// the MI_00 handle and gates video on it; `None` sends a bare heartbeat loop and ungated
    /// video, which a dock that has not been handshaken discards.
    pub session: Option<SessionSettings>,
    pub fps: u32,
    /// One looped group of pictures: element 0 must be the keyframe bundle, the rest
    /// inter-frames. The loop restarts at the keyframe each time it wraps.
    pub gop: Vec<Vec<u8>>,
}

/// Result of an isolation run.
#[derive(Debug, Clone)]
pub struct IsolateReport {
    pub mode: IsolateMode,
    pub elapsed: Duration,
    pub heartbeat: Option<HeartbeatStats>,
    /// Session counters when the dock session ran.
    pub session: Option<SessionStats>,
    pub video_messages: usize,
    pub video_chunk_stats: Option<TimingStats>,
    /// Frames whose send finished after their scheduled slot (the stream fell behind `fps`).
    pub video_late_frames: usize,
}

pub(super) struct VideoOutcome {
    pub(super) messages: usize,
    pub(super) chunk_samples: Vec<Duration>,
    pub(super) late: usize,
}
