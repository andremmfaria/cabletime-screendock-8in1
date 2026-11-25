//! Types of the application layer: daemon parameters, report and errors.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use thiserror::Error;

use crate::core::reconfig::{LiveSettings, PlanOptions, ReconfigAction, ReconfigPlan};
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
    /// Settings the supervisor changes while the loop runs.
    pub pipeline: Arc<PipelineState>,
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

/// Produces the settings the config file holds now, or the reason it cannot.
pub type ReloadFn = Box<dyn FnMut() -> Result<LiveSettings, String> + Send>;

/// What the supervisor drives: the running pipeline, seen as the steps it can execute.
pub trait PipelineHandle: Send {
    /// The steps this pipeline can execute while it runs.
    fn supported(&self) -> Vec<ReconfigAction>;

    /// Facts that change how the supervisor classifies a change.
    fn options(&self) -> PlanOptions;

    /// Executes a plan whose steps are all supported. The error text is shown to the client.
    fn apply(&mut self, plan: &ReconfigPlan, settings: &LiveSettings) -> Result<(), String>;
}

/// A pipeline that can only record the follow setting; every other step is unsupported.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullPipeline;

/// The single owner of the live settings; see [`supervisor`](super::supervisor).
pub struct Supervisor {
    pub(super) live: LiveSettings,
    pub(super) reload: ReloadFn,
    pub(super) pipeline: Box<dyn PipelineHandle>,
    pub(super) extra: BTreeMap<String, String>,
    pub(super) changes_applied: u64,
}

/// The encoder settings the supervisor can change while the video loop runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineKnobs {
    pub bitrate_kbps: u32,
    pub vbv_buffer_ms: u32,
    pub keyframe_interval: u32,
}

#[derive(Debug)]
pub(super) struct PipelineShared {
    pub(super) published: PipelineKnobs,
    pub(super) applied: PipelineKnobs,
    pub(super) version: u64,
    pub(super) applied_version: u64,
    pub(super) failure: Option<(u64, String)>,
}

/// Hand-over between the supervisor and the video loop. The supervisor publishes new knobs
/// and waits; the loop applies them at a frame boundary and reports the result.
#[derive(Debug)]
pub struct PipelineState {
    pub(super) shared: Mutex<PipelineShared>,
    pub(super) done: Condvar,
    pub(super) live_rate: bool,
}

/// The pipeline handle of a running daemon: bit rate and keyframe interval changes go through
/// [`PipelineState`] to the video loop.
#[derive(Debug, Clone)]
pub struct EncoderPipeline {
    pub(super) state: Arc<PipelineState>,
}

/// Longest the supervisor waits for the video loop to apply a change.
pub(super) const APPLY_TIMEOUT: Duration = Duration::from_secs(3);
