use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use tracing::debug;

pub(super) use super::types::Shared;
pub use super::types::{SessionHandle, SessionState, SessionStats};
use crate::core::policy::{DOCK_ALIVE_WINDOW, POLL_SLICE};

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
