//! Types of the application layer: daemon parameters, report and errors.

use std::time::Duration;

use thiserror::Error;

use crate::core::session::{SessionError, SessionSettings};
use crate::protocol::display::DisplayGeometry;
use crate::protocol::framing::ReportId;

/// Consecutive failed frames after which the daemon gives up.
pub const DEFAULT_MAX_CONSECUTIVE_ERRORS: u32 = 100;

/// Pause after a failed capture so a broken backend cannot spin the CPU.
pub(super) const CAPTURE_RETRY_DELAY: Duration = Duration::from_millis(40);

/// Pause between checks while the session is not armed.
pub(super) const UNARMED_POLL: Duration = Duration::from_millis(20);

/// Consecutive failed video writes after which the session is told to redo the handshake.
pub(super) const WRITE_FAILURES_BEFORE_REHANDSHAKE: u32 = 3;

/// How often the loop logs a progress line.
pub(super) const STATS_INTERVAL: Duration = Duration::from_secs(5);

/// Fatal daemon errors.
#[derive(Debug, Error)]
pub enum DaemonError {
    /// A handle was opened for the wrong channel.
    #[error("{what} handle is the {got:?} channel")]
    WrongChannel { what: &'static str, got: ReportId },
    /// The session could not start (for example the first heartbeat write failed).
    #[error("session failed: {0}")]
    Session(#[from] SessionError),
    /// The session ended for good while the daemon ran (the MI_00 reader died).
    #[error("session ended: {0}")]
    SessionEnded(String),
    /// Too many frames in a row failed.
    #[error("{count} consecutive frame failures, last: {last}")]
    TooManyErrors { count: u32, last: String },
}

/// Loop settings.
#[derive(Debug, Clone)]
pub struct DaemonParams {
    pub geometry: DisplayGeometry,
    /// MI_00 session settings: session id, handshake switch and timeouts.
    pub session: SessionSettings,
    /// Stop after this many frames were sent (tests and dry runs); `None` runs until stopped.
    pub max_frames: Option<u64>,
    pub max_consecutive_errors: u32,
}

/// Counters from a finished run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonReport {
    pub frames_sent: u64,
    pub messages_sent: u64,
    pub keyframes_sent: u64,
    pub capture_errors: u64,
    pub encode_errors: u64,
    pub write_errors: u64,
    pub heartbeats_sent: u64,
    pub heartbeat_errors: u64,
    /// Times the session became armed (first handshake plus every redo).
    pub handshakes: u64,
    /// Times an armed session was lost and the handshake redone.
    pub session_losses: u64,
    /// Heartbeats the dock sent that the reader saw.
    pub dock_heartbeats_seen: u64,
    pub elapsed: Duration,
}
