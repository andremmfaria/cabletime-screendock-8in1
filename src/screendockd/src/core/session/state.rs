use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tracing::debug;

use crate::core::policy::{DOCK_ALIVE_WINDOW, POLL_SLICE};

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
    /// The session was shut down or its reader died.
    Stopped,
}

impl SessionState {
    fn as_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(raw: u8) -> Self {
        match raw {
            0 => Self::Starting,
            1 => Self::WaitingForReady,
            2 => Self::AppLaunchedSent,
            3 => Self::ProjectionStartedSent,
            4 => Self::Settling,
            5 => Self::Active,
            6 => Self::AssumedActive,
            7 => Self::Lost,
            _ => Self::Stopped,
        }
    }

    /// True when video may be written.
    pub fn is_armed(self) -> bool {
        matches!(self, Self::Active | Self::AssumedActive)
    }
}

/// State shared between the session threads and their users.
#[derive(Debug)]
pub(super) struct Shared {
    pub(super) state: AtomicU8,
    /// Incremented each time the session becomes armed.
    pub(super) generation: AtomicU64,
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

impl Shared {
    pub(super) fn new(state: SessionState) -> Self {
        Self {
            state: AtomicU8::new(state.as_u8()),
            generation: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            base: Instant::now(),
            dock_heartbeats: AtomicU64::new(0),
            last_dock_heartbeat_ms: AtomicU64::new(0),
            heartbeats_sent: AtomicU64::new(0),
            heartbeat_errors: AtomicU64::new(0),
            consecutive_heartbeat_failures: AtomicU32::new(0),
            handshakes: AtomicU64::new(0),
            losses: AtomicU64::new(0),
            loss_reason: Mutex::new(None),
            fatal: Mutex::new(None),
        }
    }

    pub(super) fn state(&self) -> SessionState {
        SessionState::from_u8(self.state.load(Ordering::Acquire))
    }

    pub(super) fn set_state(&self, state: SessionState) {
        let previous = SessionState::from_u8(self.state.swap(state.as_u8(), Ordering::AcqRel));
        if previous != state {
            debug!(?previous, ?state, "session state");
        }
    }

    pub(super) fn note_dock_heartbeat(&self) {
        self.dock_heartbeats.fetch_add(1, Ordering::Relaxed);
        let ms = u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX - 1);
        self.last_dock_heartbeat_ms.store(ms + 1, Ordering::Relaxed);
    }

    /// True when the dock's heartbeat was seen recently.
    pub(super) fn dock_alive(&self) -> bool {
        let last = self.last_dock_heartbeat_ms.load(Ordering::Relaxed);
        if last == 0 {
            return false;
        }
        let now = u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX);
        now.saturating_sub(last - 1) <= DOCK_ALIVE_WINDOW.as_millis() as u64
    }

    pub(super) fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    pub(super) fn set_fatal(&self, message: String) {
        let mut slot = self.fatal.lock().unwrap_or_else(|e| e.into_inner());
        slot.get_or_insert(message);
    }
}

/// Cheap, cloneable view of a running session for the video loop.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub(super) shared: Arc<Shared>,
}

impl SessionHandle {
    pub fn state(&self) -> SessionState {
        self.shared.state()
    }

    /// True when video may be written.
    pub fn is_armed(&self) -> bool {
        self.state().is_armed()
    }

    /// Counter that changes each time the session becomes armed again. A change means the
    /// dock may have lost its decoder state, so the encoder should restart from a keyframe.
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }

    /// Reports a failed video write (or any other sign the dock dropped the session). The
    /// controller leaves the armed state and redoes the handshake.
    pub fn report_failure(&self, reason: &str) {
        let mut slot = self
            .shared
            .loss_reason
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        slot.get_or_insert_with(|| reason.to_string());
    }

    /// The error that ended the session for good (reader died), if any.
    pub fn fatal(&self) -> Option<String> {
        self.shared
            .fatal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
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

/// Sleeps until `deadline` in short slices; returns false when stopped first.
pub(super) fn sleep_until(deadline: Instant, shared: &Shared) -> bool {
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if shared.stopped() {
            return false;
        }
        std::thread::sleep(left.min(POLL_SLICE));
    }
    !shared.stopped()
}
