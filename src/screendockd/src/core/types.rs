//! Types and constants of the core: session timing defaults, polling constants and the display
//! policy decision types.

use std::time::Duration;

use thiserror::Error;

use crate::ports::display::OutputId;

/// How long to wait for the dock's ready announcement before deciding whether it already
/// holds a session.
pub const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(3);

/// Longest wait for the result of one request (`app-launched`, `notify-projection-started`).
/// The handshake continues when it expires.
pub const DEFAULT_REPLY_TIMEOUT: Duration = Duration::from_millis(1500);

/// Pause between `isAllowed` and the first video report, matching the 0.7 to 1.1 s the vendor
/// driver leaves for its first keyframe.
pub const DEFAULT_SETTLE_DELAY: Duration = Duration::from_millis(800);

/// Pause before a failed handshake is retried.
pub const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(1);

/// Consecutive failed heartbeat writes after which the session counts as lost.
pub const DEFAULT_MAX_HEARTBEAT_FAILURES: u32 = 3;

/// Longest single sleep or wait, so stop requests stay responsive.
pub(super) const POLL_SLICE: Duration = Duration::from_millis(50);

/// A dock heartbeat counts as proof of life for this long.
pub(super) const DOCK_ALIVE_WINDOW: Duration = Duration::from_secs(3);

/// Decides which frames must be keyframes: the first one, then one every `interval` frames.
/// The encoder only produces a keyframe when asked, so the interval can change while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyframeClock {
    pub(super) interval: u32,
    pub(super) until_key: u32,
}

/// How the monitor is provided on X11.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11Tier {
    /// A real output that takes a custom mode.
    RealOutput(OutputId),
    /// A bare RandR monitor object over an enlarged screen.
    BareMonitor,
}

/// A mechanism that can provide a virtual monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtendBackend {
    X11Randr(X11Tier),
    /// `org.gnome.Mutter.ScreenCast` `RecordVirtual`.
    Mutter,
    /// The desktop portal's virtual source type.
    Portal,
    /// A headless output on a wlroots compositor (sway).
    Wlroots,
}

/// What the dock shows: a copy of a screen, or a virtual monitor of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayDecision {
    /// Capture an existing screen. The reason says why no monitor is created.
    Mirror { reason: String },
    /// Create a virtual monitor through the backend.
    Extend(ExtendBackend),
}

/// Whether a mechanism can work in the environment the facts describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualSupport {
    Supported,
    /// Cannot work, with the reason.
    Unsupported(String),
    /// The facts do not say; trying is the only test.
    Unknown,
}

/// Errors from the display policy.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DisplayPolicyError {
    /// Extend mode was requested, no mechanism applies and the fallback is `fail`.
    #[error("no virtual monitor mechanism is available: {0}")]
    NoVirtualDisplay(String),
    /// The anchor names no output.
    #[error("anchor output {0:?} was not found")]
    UnknownAnchor(String),
}
