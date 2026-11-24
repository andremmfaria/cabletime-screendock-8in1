//! Session timing defaults and the polling constants shared by the session threads.

use std::time::Duration;

/// Default heartbeat tick interval when the config doesn't override it.
pub const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(1000);

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
