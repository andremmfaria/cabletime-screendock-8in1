//! Types of the dock session: state, settings, handle, statistics, errors and the shared state.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;
use thiserror::Error;

use crate::ports::transport::{DockTransport, TransportError};
use crate::protocol::framing::{PictureSize, ReportId};
use crate::protocol::heartbeat::SessionId;

/// A running session: owns the heartbeat, reader and controller threads.
#[derive(Debug)]
pub struct Session {
    pub(super) shared: Arc<Shared>,
    pub(super) threads: Vec<(&'static str, JoinHandle<()>)>,
    pub(super) commands: Option<Sender<ControllerCommand>>,
}

/// A request to the controller thread.
#[derive(Debug)]
pub(super) enum ControllerCommand {
    /// Run the stop, launch and start cycle at a new picture size and answer when the session
    /// is armed again or the cycle failed.
    Renegotiate {
        size: PictureSize,
        reply: Sender<Result<(), SessionError>>,
    },
}

/// A renegotiation in progress: the size to return to when it fails, and who is waiting.
pub(super) struct Renegotiation {
    pub(super) previous: PictureSize,
    pub(super) reply: Sender<Result<(), SessionError>>,
}

/// Why `Controller::monitor` returned.
pub(super) enum MonitorEnd {
    /// The session shut down or its reader died.
    Stopped,
    /// The armed session was lost.
    Lost(String),
    /// A client asked for another picture size.
    Command(ControllerCommand),
}

pub(super) type SharedWriter = Arc<Mutex<Box<dyn DockTransport>>>;

/// How a wait ended early.
pub(super) enum Interrupted {
    Stopped,
}

/// Outcome of the handshake's bounded waits.
pub(super) enum Wait<T> {
    Got(T),
    TimedOut,
}

pub(super) struct Controller {
    pub(super) settings: SessionSettings,
    pub(super) shared: Arc<Shared>,
    pub(super) writer: SharedWriter,
    pub(super) events: Receiver<DockEvent>,
    pub(super) commands: Receiver<ControllerCommand>,
    pub(super) next_id: u64,
    /// A ready announcement the monitor consumed; the next handshake starts from it instead
    /// of waiting for another one.
    pub(super) ready_pending: bool,
}

pub(super) enum HandshakeEnd {
    Stopped,
    Failed(SessionError),
}

impl From<Interrupted> for HandshakeEnd {
    fn from(_: Interrupted) -> Self {
        HandshakeEnd::Stopped
    }
}

/// Errors from the session.
#[derive(Debug, Error)]
pub enum SessionError {
    /// The handle passed in isn't the MI_00 control channel.
    #[error("session needs the control (MI_00) channel, got {got:?}")]
    WrongChannel { got: ReportId },
    /// Duplicating the MI_00 descriptor for the reader thread failed.
    #[error("couldn't set up the MI_00 reader: {0}")]
    Setup(TransportError),
    /// The first heartbeat write failed, so the control channel is unusable.
    #[error("startup heartbeat write failed: {0}")]
    StartupHeartbeat(TransportError),
    /// A control message didn't fit one report.
    #[error("control message of {len} bytes doesn't fit one report")]
    MessageTooLarge { len: usize },
    /// Writing a control message failed.
    #[error("control write failed: {0}")]
    Write(TransportError),
    /// The dock answered a request with an error object.
    #[error("dock rejected {request}: {detail}")]
    Rejected {
        request: &'static str,
        detail: String,
    },
    /// The dock answered `notify-projection-started` with something other than `isAllowed`.
    #[error("dock denied projection: {0}")]
    Denied(String),
    /// A session thread panicked.
    #[error("{0} thread panicked")]
    Panicked(&'static str),
    /// The session runs without a handshake, so there is nothing to renegotiate.
    #[error("the session has no handshake to redo")]
    RenegotiationUnavailable,
    /// The cycle did not finish within the caller's timeout. It may still complete later.
    #[error("the dock cycle did not finish in time")]
    RenegotiationTimedOut,
    /// The session ended before the cycle finished.
    #[error("the session ended during the dock cycle")]
    SessionClosed,
}

/// An event the reader forwards to the controller.
#[derive(Debug)]
pub(super) enum DockEvent {
    Ready,
    Reply {
        id: u64,
        result: Option<Value>,
        error: Option<Value>,
    },
}

/// Session settings. `handshake: false` keeps only the heartbeat thread and arms video
/// immediately, for regular files standing in for the nodes.
#[derive(Debug, Clone)]
pub struct SessionSettings {
    pub session_id: SessionId,
    pub machine_name: String,
    pub size: PictureSize,
    pub heartbeat_interval: Duration,
    pub handshake: bool,
    pub send_stopped: bool,
    pub ready_timeout: Duration,
    pub reply_timeout: Duration,
    pub settle_delay: Duration,
    pub retry_delay: Duration,
    pub max_heartbeat_failures: u32,
}

/// Session state. `Active` and `AssumedActive` are the armed states in which video may be
/// written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Created, the controller has not started the handshake yet.
    Starting,
    /// Waiting for the dock's ready announcement.
    WaitingForReady,
    /// `app-launched` sent, waiting for its result.
    AppLaunchedSent,
    /// `notify-projection-started` sent, waiting for `isAllowed`.
    ProjectionStartedSent,
    /// `isAllowed` received, waiting out the settle delay.
    Settling,
    /// Handshake completed with every reply seen.
    Active,
    /// The dock was alive but did not answer; video is allowed anyway.
    AssumedActive,
    /// The session was lost or the handshake failed; video is held back until it is redone.
    Lost,
    /// The stop, launch and start cycle runs at a new picture size; video is held back.
    Renegotiating,
    /// The session was shut down or its reader died.
    Stopped,
}

/// State shared between the session threads and their users.
#[derive(Debug)]
pub(super) struct Shared {
    pub(super) state: AtomicU8,
    /// Incremented each time the session becomes armed.
    pub(super) generation: AtomicU64,
    /// Picture size the dock was last launched at, as `width << 32 | height`.
    pub(super) size: AtomicU64,
    pub(super) stop: AtomicBool,
    pub(super) base: Instant,
    pub(super) dock_heartbeats: AtomicU64,
    /// Milliseconds since `base` of the last dock heartbeat, plus one; zero means never.
    pub(super) last_dock_heartbeat_ms: AtomicU64,
    pub(super) heartbeats_sent: AtomicU64,
    pub(super) heartbeat_errors: AtomicU64,
    pub(super) consecutive_heartbeat_failures: AtomicU32,
    pub(super) handshakes: AtomicU64,
    pub(super) losses: AtomicU64,
    pub(super) loss_reason: Mutex<Option<String>>,
    pub(super) fatal: Mutex<Option<String>>,
}

/// Cheap, cloneable view of a running session for the video loop.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub(super) shared: Arc<Shared>,
    pub(super) commands: Option<Sender<ControllerCommand>>,
}

/// Counters from a finished session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionStats {
    pub heartbeats_sent: u64,
    pub heartbeat_errors: u64,
    pub dock_heartbeats_seen: u64,
    /// Times the session became armed (first handshake plus every redo).
    pub handshakes: u64,
    /// Times an armed session was lost.
    pub losses: u64,
}
