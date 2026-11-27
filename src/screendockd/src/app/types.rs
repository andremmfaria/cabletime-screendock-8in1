//! Types of the application layer: daemon parameters, report and errors.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use thiserror::Error;

use crate::core::reconfig::{LiveSettings, PlanOptions, ReconfigAction, ReconfigPlan};
use crate::core::session::{SessionError, SessionHandle, SessionSettings};
use crate::ports::capture::{CaptureArea, CaptureBackend};
use crate::ports::control::Command;
use crate::ports::display::{DisplayProvider, Rect, VirtualMonitor};
use crate::ports::encode::{EncodeError, Encoder, EncoderSettings};
use crate::ports::transport::EdidStore;
use crate::protocol::display::DisplayGeometry;
use crate::protocol::framing::ReportId;
use crate::utils::config::X264Preset;
use crate::utils::types::EdidSource;
use crate::utils::types::VirtualDisplayConfig;

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

    /// Builds and checks the new pieces without changing anything visible. The default does
    /// nothing.
    fn prepare(&mut self, _plan: &ReconfigPlan, _settings: &LiveSettings) -> Result<(), String> {
        Ok(())
    }

    /// Checks that the pipeline works after a change was applied. The default accepts.
    fn verify(&mut self, _plan: &ReconfigPlan, _settings: &LiveSettings) -> Result<(), String> {
        Ok(())
    }

    /// Puts the previous settings back; `plan` is the change from the failed settings to
    /// them. The default applies it like any other plan.
    fn restore(&mut self, plan: &ReconfigPlan, settings: &LiveSettings) -> Result<(), String> {
        self.apply(plan, settings)
    }

    /// Read-only facts about the dock's panel for `status` and `get`. The default has none.
    fn display_fields(&self) -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    /// Brings a pipeline in an unknown state back to `settings`, which are the last ones that
    /// worked. The default does nothing.
    fn resync(&mut self, _settings: &LiveSettings) -> Result<(), String> {
        Ok(())
    }

    /// The virtual monitor is at `rect` because something outside the daemon put it there.
    /// The pipeline records it so the monitor is not moved back and the change is not
    /// reported again. The default does nothing.
    fn observed(&mut self, _rect: Rect) {}
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
    /// Why the daemon is degraded: a change failed and so did putting the old settings back.
    pub(super) degraded: Option<String>,
}

/// The encoder settings the supervisor can change while the video loop runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineKnobs {
    pub bitrate_kbps: u32,
    pub vbv_buffer_ms: u32,
    pub keyframe_interval: u32,
    /// Encoded picture size and orientation; also the size in every video header.
    pub geometry: DisplayGeometry,
    pub fps: u32,
    pub preset: X264Preset,
    pub tune_zerolatency: bool,
    /// The part of the source that is captured.
    pub area: CaptureArea,
}

/// Builds an encoder for a set of settings. The video loop calls it when a change needs a new
/// encoder.
#[derive(Clone)]
pub struct EncoderBuilder(pub(super) Arc<BuildFn>);

pub(super) type BuildFn =
    dyn Fn(EncoderSettings) -> Result<Box<dyn Encoder>, EncodeError> + Send + Sync;

impl fmt::Debug for EncoderBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EncoderBuilder")
    }
}

/// The encoder the video loop runs: the one it was started with until a rebuild replaces it.
pub(super) struct EncoderSlot<'a> {
    pub(super) base: &'a mut dyn Encoder,
    pub(super) replacement: Option<Box<dyn Encoder>>,
}

/// What a change to the running pipeline works on.
pub(super) struct LoopParts<'a, 's> {
    pub(super) slot: &'a mut EncoderSlot<'s>,
    pub(super) capture: &'a mut dyn CaptureBackend,
    pub(super) session: &'a SessionHandle,
    pub(super) pipeline: &'a PipelineState,
    pub(super) cycle_timeout: Duration,
}

/// Longest the supervisor waits for a change that rebuilds the encoder or runs the dock cycle.
pub(super) const CYCLE_APPLY_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub(super) struct PipelineShared {
    pub(super) published: PipelineKnobs,
    pub(super) applied: PipelineKnobs,
    pub(super) version: u64,
    pub(super) applied_version: u64,
    pub(super) failure: Option<(u64, String)>,
    /// Run the dock cycle for `version` even when the picture size did not change.
    pub(super) force_cycle: bool,
}

/// Hand-over between the supervisor and the video loop. The supervisor publishes new knobs
/// and waits; the loop applies them at a frame boundary and reports the result.
#[derive(Debug)]
pub struct PipelineState {
    pub(super) shared: Mutex<PipelineShared>,
    pub(super) done: Condvar,
    pub(super) live_rate: bool,
    pub(super) builder: Option<EncoderBuilder>,
    /// The session the video loop runs against, for the verify stage.
    pub(super) session: Mutex<Option<SessionHandle>>,
    /// Video messages written so far.
    pub(super) written: AtomicU64,
    /// What is known about the dock's panel, and where it is cached.
    pub(super) display: Mutex<DisplayState>,
}

/// The dock's EDID as the pipeline state tracks it.
#[derive(Debug)]
pub(super) struct DisplayState {
    pub(super) store: Option<Arc<dyn EdidStore>>,
    pub(super) prefer: EdidSource,
    pub(super) fallback: (u16, u16),
    pub(super) cached: Option<Vec<u8>>,
    pub(super) dock: Option<Vec<u8>>,
}

/// The pipeline handle of a running daemon: bit rate and keyframe interval changes go through
/// [`PipelineState`] to the video loop.
#[derive(Debug, Clone)]
pub struct EncoderPipeline {
    pub(super) state: Arc<PipelineState>,
    /// Session losses and written messages when the last change was applied.
    pub(super) baseline: Option<(u64, u64)>,
    /// The capture area the monitor management chose; `None` keeps the one in the state.
    pub(super) area: Option<CaptureArea>,
}

/// Owns the virtual monitor of a running daemon: creates it, moves it, resizes it and removes
/// it as the live settings change, and says which area of the desktop to capture.
pub struct MonitorManager {
    pub(super) provider: Box<dyn DisplayProvider>,
    pub(super) base: VirtualDisplayConfig,
    pub(super) monitor: Option<Box<dyn VirtualMonitor>>,
    /// Where the monitor's watcher sends what it sees; `None` follows nothing.
    pub(super) events: Option<std::sync::mpsc::Sender<Command>>,
}

/// An [`EncoderPipeline`] that also manages the virtual monitor, so switching between mirror
/// and extend, moving and resizing the monitor go through the same transaction as every other
/// change.
pub struct MonitorPipeline {
    pub(super) inner: EncoderPipeline,
    pub(super) monitors: MonitorManager,
}

/// How long a changed pipeline is watched for a session loss.
pub(super) const VERIFY_WINDOW: Duration = Duration::from_secs(1);

/// Video messages that must be written after a change before it counts as working.
pub(super) const VERIFY_MESSAGES: u64 = 3;

/// Longest the supervisor waits for the video loop to apply a change.
pub(super) const APPLY_TIMEOUT: Duration = Duration::from_secs(3);
