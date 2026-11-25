//! Types of the test support: the fake dock and its configuration.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// What the fake answers to `notify-projection-started`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartedReply {
    /// `"result":"isAllowed"`.
    Allowed,
    /// `"result":"isDenied"`.
    Denied,
    /// No reply at all.
    Silent,
}

/// Fake dock behavior.
#[derive(Debug, Clone)]
pub struct FakeDockConfig {
    /// Announce ready (plus tx-data) every `ready_interval` while no session is held.
    pub announce_ready: bool,
    pub ready_interval: Duration,
    /// Dock heartbeat period; `None` makes a silent dock.
    pub heartbeat_interval: Option<Duration>,
    /// Delay before the `app-launched` result; `None` never answers it.
    pub launch_reply_delay: Option<Duration>,
    pub started_reply: StartedReply,
    /// Send the EDID passthrough (id 101) after `app-launched`, repeating until answered.
    pub send_edid: bool,
    /// Start with a projection session already held (no announcements, no replies needed).
    pub holds_session: bool,
}

/// What the fake saw on its simulated MI_01.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VideoStats {
    pub reports: u64,
    pub messages: u64,
    /// Reports that arrived while the sink was not started (a real dock drops these).
    pub reports_before_sink: u64,
    /// Reports that broke the header layout.
    pub layout_violations: u64,
    /// Width and height of the first video message.
    pub first_size: Option<(u32, u32)>,
    /// Number of complete host control messages (heartbeats included) logged when the first
    /// video report arrived.
    pub control_messages_before_video: Option<usize>,
}

#[derive(Debug)]
pub(super) struct DockState {
    pub(super) app_known: bool,
    pub(super) sink_started: bool,
    pub(super) edid_pending: bool,
    pub(super) next_edid: Instant,
    pub(super) next_ready: Instant,
    pub(super) next_heartbeat: Instant,
    pub(super) seq: u64,
    pub(super) scheduled: Vec<(Instant, Vec<u8>, SinkEffect)>,
    pub(super) host_log: Vec<(Instant, Vec<u8>)>,
    pub(super) format_violations: u64,
    pub(super) allowed_sent_at: Option<Instant>,
    pub(super) video: VideoStats,
    pub(super) launch_error: bool,
}

/// Side effect applied when a scheduled message is sent.
#[derive(Debug, Clone, Copy)]
pub(super) enum SinkEffect {
    None,
    StartSink,
}

#[derive(Debug)]
pub(super) struct Inner {
    pub(super) config: FakeDockConfig,
    pub(super) state: Mutex<DockState>,
    pub(super) stop: AtomicBool,
}

/// A running fake dock.
#[derive(Debug)]
pub struct FakeDock {
    pub(super) inner: Arc<Inner>,
    pub(super) thread: Option<JoinHandle<()>>,
}
