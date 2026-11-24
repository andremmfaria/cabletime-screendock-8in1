//! Dock session on MI_00: continuous heartbeat, a reader that always drains the dock's IN
//! reports, and the handshake state machine that arms the video sink.
//!
//! The dock discards every video frame, silently, until three things have happened: the host
//! sent `app-launched` (a request with an id and a `platform`), the host sent
//! `notify-projection-started` (a request with an id), and the dock answered `isAllowed`. A
//! dock that already holds a projection session from an earlier host process announces and
//! answers nothing, yet processes these messages and then renders video. The state machine
//! therefore always sends the full message sequence, bounds every wait, and arms as
//! [`SessionState::AssumedActive`] when the ready announcement or a reply does not arrive,
//! whether or not the dock sent anything at all (a session-holding dock often echoes no
//! heartbeat either). Video is never blocked on dock replies. The controller keeps watching in
//! the armed states: if the dock announces `notify-app-accessories-are-ready` (it holds no
//! session), or heartbeat or video writes keep failing, the session is lost and the full
//! handshake is run without restarting the process.
//!
//! Threads, all owned by [`Session`]:
//! - heartbeat: one fixed-deadline writer, independent of video writes (they use MI_01);
//! - reader: polls MI_00, reassembles and parses the dock's JSON, answers the dock's EDID
//!   passthrough, and forwards the rest to the controller as [`DockEvent`]s;
//! - controller: runs the handshake, then watches for loss and starts over.
//!
//! Mechanism/policy split: the `messages` module is policy (exact bytes of every host message),
//! [`InboundAssembler`] and [`parse_dock_message`] are the parsing mechanism, and the
//! controller sequences them. Timeouts are settings so tests run in milliseconds.
//!
//! Dock text is untrusted data: it is parsed as JSON, never executed, and only truncated method
//! names reach the logs.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::framing::{MessageBuilder, PictureSize, ReportId, FIRST_CHUNK_PAYLOAD_MAX, HEADER_SIZE};
use crate::heartbeat::{heartbeat_message, SessionId};
use crate::hidraw_io::{HidrawHandle, HidrawIoError};

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
const POLL_SLICE: Duration = Duration::from_millis(50);

/// Largest dock message the reader reassembles; anything bigger is dropped as malformed.
const MAX_INBOUND_LEN: usize = 64 * 1024;

/// A dock heartbeat counts as proof of life for this long.
const DOCK_ALIVE_WINDOW: Duration = Duration::from_secs(3);

/// Longest dock-supplied string (method names) written to the log.
const LOG_TEXT_MAX: usize = 64;

/// Errors from the session.
#[derive(Debug, Error)]
pub enum SessionError {
    /// The handle passed in isn't the MI_00 control channel.
    #[error("session needs the control (MI_00) channel, got {got:?}")]
    WrongChannel { got: ReportId },
    /// Duplicating the MI_00 descriptor for the reader thread failed.
    #[error("couldn't set up the MI_00 reader: {0}")]
    Setup(HidrawIoError),
    /// The first heartbeat write failed, so the control channel is unusable.
    #[error("startup heartbeat write failed: {0}")]
    StartupHeartbeat(HidrawIoError),
    /// A control message didn't fit one report.
    #[error("control message of {len} bytes doesn't fit one report")]
    MessageTooLarge { len: usize },
    /// Writing a control message failed.
    #[error("control write failed: {0}")]
    Write(HidrawIoError),
    /// The dock answered a request with an error object.
    #[error("dock rejected {request}: {detail}")]
    Rejected {
        request: &'static str,
        detail: String,
    },
    /// The dock answered `notify-projection-started` with something other than `isAllowed`.
    #[error("dock denied projection: {0}")]
    Denied(String),
    /// The MI_00 reader stopped (read error or end of stream).
    #[error("MI_00 reader stopped: {0}")]
    ReaderStopped(String),
    /// A session thread panicked.
    #[error("{0} thread panicked")]
    Panicked(&'static str),
}

/// Exact bytes of every host message on MI_00. Heartbeats live in `heartbeat.rs`.
///
/// Everything except the heartbeat is compact JSON with alphabetical keys, ending in one
/// newline that is counted in the length field.
pub mod messages {
    use super::PictureSize;

    /// Platform string the dock requires in `app-launched`.
    pub const PLATFORM: &str = "windows";
    /// Product type string the vendor driver sends.
    pub const PRODUCT_TYPE: &str = "QuattroTx CT10";
    /// Longest machine name sent, in characters.
    pub const MACHINE_NAME_MAX: usize = 15;
    /// Id of the EDID passthrough message the dock sends.
    pub const EDID_PASSTHROUGH_ID: u64 = 101;

    /// `notify-projection-stopped`.
    pub const PROJECTION_STOPPED: &[u8] =
        b"{\"jsonrpc\":\"2.0\",\"method\":\"notify-projection-stopped\"}\n";

    /// `app_tx_mouse_icon_change` with an empty cursor.
    pub const MOUSE_ICON_CHANGE: &[u8] = b"{\"jsonrpc\":\"2.0\",\"method\":\"app_tx_mouse_icon_change\",\"params\":{\"cursor_base64_data\":\"\"}}\n";

    /// The reply to the dock's EDID passthrough (id 101): passthrough id 103 with data `AQ==`.
    pub const EDID_ACK: &[u8] = b"{\"jsonrpc\":\"2.0\",\"method\":\"common.data_passthrough\",\"params\":{\"data\":\"AQ==\",\"id\":103,\"size\":1}}\n";

    /// Uppercases a hostname and truncates it to 15 characters, as the vendor driver does for
    /// `platform-machine-name`.
    pub fn machine_name_from_hostname(hostname: &str) -> String {
        hostname
            .trim()
            .to_uppercase()
            .chars()
            .filter(|c| !c.is_control())
            .take(MACHINE_NAME_MAX)
            .collect()
    }

    /// `app-launched` request.
    pub fn app_launched(id: u64, machine_name: &str, size: PictureSize) -> Vec<u8> {
        // `to_string` on a `str` value cannot fail and escapes quotes and backslashes.
        let name = serde_json::Value::from(machine_name).to_string();
        format!(
            "{{\"id\":{id},\"jsonrpc\":\"2.0\",\"method\":\"app-launched\",\"params\":{{\"platform\":\"{PLATFORM}\",\"platform-machine-name\":{name},\"product-type\":\"{PRODUCT_TYPE}\",\"res-height\":{},\"res-width\":{}}}}}\n",
            size.height.0, size.width.0
        )
        .into_bytes()
    }

    /// `notify-projection-started` request.
    pub fn projection_started(id: u64) -> Vec<u8> {
        format!("{{\"id\":{id},\"jsonrpc\":\"2.0\",\"method\":\"notify-projection-started\"}}\n")
            .into_bytes()
    }
}

/// The host's hostname, or an empty string when it can't be read.
pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is valid for `buf.len()` bytes; the final byte is never written by a
    // successful call longer than the buffer, so the contents stay NUL terminated below.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len() - 1) };
    if rc != 0 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
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

impl SessionSettings {
    /// Defaults for a picture size: fresh session id, hostname-derived machine name, the full
    /// handshake enabled.
    pub fn new(size: PictureSize, heartbeat_interval: Duration) -> Self {
        Self {
            session_id: SessionId::generate(),
            machine_name: messages::machine_name_from_hostname(&hostname()),
            size,
            heartbeat_interval,
            handshake: true,
            send_stopped: true,
            ready_timeout: DEFAULT_READY_TIMEOUT,
            reply_timeout: DEFAULT_REPLY_TIMEOUT,
            settle_delay: DEFAULT_SETTLE_DELAY,
            retry_delay: DEFAULT_RETRY_DELAY,
            max_heartbeat_failures: DEFAULT_MAX_HEARTBEAT_FAILURES,
        }
    }

    /// Same settings without the handshake and without the reader.
    pub fn without_handshake(mut self) -> Self {
        self.handshake = false;
        self
    }
}

/// A parsed message from the dock.
#[derive(Debug, Clone, PartialEq)]
pub enum DockMessage {
    /// `notify-app-accessories-are-ready`.
    Ready,
    /// The dock's heartbeat.
    Heartbeat,
    /// `common.data_passthrough` with the given params id.
    Passthrough { id: Option<u64> },
    /// A result or error for one of our requests.
    Reply {
        id: u64,
        result: Option<Value>,
        error: Option<Value>,
    },
    /// Any other notification (`tx-data-updated`, ...), with its truncated method name.
    Other { method: String },
    /// Text that isn't a JSON-RPC object this driver understands.
    Unrecognized,
}

/// Parses the JSON text of one dock message.
pub fn parse_dock_message(text: &[u8]) -> DockMessage {
    let Ok(value) = serde_json::from_slice::<Value>(text) else {
        return DockMessage::Unrecognized;
    };
    let Some(object) = value.as_object() else {
        return DockMessage::Unrecognized;
    };
    if let Some(method) = object.get("method").and_then(Value::as_str) {
        return match method {
            "notify-app-accessories-are-ready" => DockMessage::Ready,
            "heartbeat" => DockMessage::Heartbeat,
            "common.data_passthrough" => DockMessage::Passthrough {
                id: object
                    .get("params")
                    .and_then(|p| p.get("id"))
                    .and_then(Value::as_u64),
            },
            other => DockMessage::Other {
                method: other.chars().take(LOG_TEXT_MAX).collect(),
            },
        };
    }
    if let Some(id) = object.get("id").and_then(Value::as_u64) {
        let result = object.get("result").cloned();
        let error = object.get("error").cloned();
        if result.is_some() || error.is_some() {
            return DockMessage::Reply { id, result, error };
        }
    }
    DockMessage::Unrecognized
}

/// Reassembles the dock's MI_00 reports into message texts.
///
/// Reports use the same layout the host writes: a 16-byte header with the big-endian payload
/// length at bytes 1..4 on the first report, then payload from offset 16; continuation reports
/// repeat the header with a zero length. The reader is lenient about the other header bytes,
/// since the input is the dock's, but never trusts the length beyond [`MAX_INBOUND_LEN`].
#[derive(Debug, Default)]
pub struct InboundAssembler {
    expected: usize,
    buf: Vec<u8>,
}

impl InboundAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one report. Returns the complete message text when this report finishes one.
    pub fn feed(&mut self, report: &[u8]) -> Option<Vec<u8>> {
        if report.len() < HEADER_SIZE {
            return None;
        }
        let announced = u32::from_be_bytes(report[1..5].try_into().ok()?) as usize;
        let body = &report[HEADER_SIZE..];
        if announced != 0 {
            // A non-zero length always starts a new message, dropping any torn one.
            if announced > MAX_INBOUND_LEN {
                self.reset();
                return None;
            }
            self.expected = announced;
            self.buf.clear();
        } else if self.expected == 0 {
            return None;
        }
        let want = self.expected - self.buf.len();
        let take = want.min(body.len()).min(FIRST_CHUNK_PAYLOAD_MAX);
        self.buf.extend_from_slice(&body[..take]);
        if self.buf.len() >= self.expected {
            let message = std::mem::take(&mut self.buf);
            self.expected = 0;
            return Some(message);
        }
        None
    }

    fn reset(&mut self) {
        self.expected = 0;
        self.buf.clear();
    }
}

/// An event the reader forwards to the controller.
#[derive(Debug)]
enum DockEvent {
    Ready,
    Reply {
        id: u64,
        result: Option<Value>,
        error: Option<Value>,
    },
}

/// State shared between the session threads and their users.
#[derive(Debug)]
struct Shared {
    state: AtomicU8,
    /// Incremented each time the session becomes armed.
    generation: AtomicU64,
    stop: AtomicBool,
    base: Instant,
    dock_heartbeats: AtomicU64,
    /// Milliseconds since `base` of the last dock heartbeat, plus one; zero means never.
    last_dock_heartbeat_ms: AtomicU64,
    heartbeats_sent: AtomicU64,
    heartbeat_errors: AtomicU64,
    consecutive_heartbeat_failures: AtomicU32,
    handshakes: AtomicU64,
    losses: AtomicU64,
    loss_reason: Mutex<Option<String>>,
    fatal: Mutex<Option<String>>,
}

impl Shared {
    fn new(state: SessionState) -> Self {
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

    fn state(&self) -> SessionState {
        SessionState::from_u8(self.state.load(Ordering::Acquire))
    }

    fn set_state(&self, state: SessionState) {
        let previous = SessionState::from_u8(self.state.swap(state.as_u8(), Ordering::AcqRel));
        if previous != state {
            debug!(?previous, ?state, "session state");
        }
    }

    fn note_dock_heartbeat(&self) {
        self.dock_heartbeats.fetch_add(1, Ordering::Relaxed);
        let ms = u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX - 1);
        self.last_dock_heartbeat_ms.store(ms + 1, Ordering::Relaxed);
    }

    /// True when the dock's heartbeat was seen recently.
    fn dock_alive(&self) -> bool {
        let last = self.last_dock_heartbeat_ms.load(Ordering::Relaxed);
        if last == 0 {
            return false;
        }
        let now = u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX);
        now.saturating_sub(last - 1) <= DOCK_ALIVE_WINDOW.as_millis() as u64
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn set_fatal(&self, message: String) {
        let mut slot = self.fatal.lock().unwrap_or_else(|e| e.into_inner());
        slot.get_or_insert(message);
    }
}

/// Cheap, cloneable view of a running session for the video loop.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    shared: Arc<Shared>,
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

/// A running session: owns the heartbeat, reader and controller threads.
#[derive(Debug)]
pub struct Session {
    shared: Arc<Shared>,
    threads: Vec<(&'static str, JoinHandle<()>)>,
}

type SharedWriter = Arc<Mutex<HidrawHandle>>;

fn send_control(writer: &SharedWriter, payload: &[u8]) -> Result<(), SessionError> {
    if payload.len() > FIRST_CHUNK_PAYLOAD_MAX {
        return Err(SessionError::MessageTooLarge { len: payload.len() });
    }
    let chunks = MessageBuilder::control().payload(payload).build();
    let mut handle = writer.lock().unwrap_or_else(|e| e.into_inner());
    handle
        .write_chunks(&chunks)
        .map(|_| ())
        .map_err(SessionError::Write)
}

impl Session {
    /// Starts the session on an MI_00 handle.
    ///
    /// The first heartbeat is written synchronously, so a dead control channel fails here.
    /// With `settings.handshake` the reader and controller threads follow and the session
    /// starts unarmed; without it only the heartbeat thread runs and video is armed at once.
    pub fn start(mi00: HidrawHandle, settings: SessionSettings) -> Result<Self, SessionError> {
        if mi00.channel() != ReportId::Control {
            return Err(SessionError::WrongChannel {
                got: mi00.channel(),
            });
        }
        let reader_handle = if settings.handshake {
            Some(mi00.try_clone().map_err(SessionError::Setup)?)
        } else {
            None
        };
        let writer: SharedWriter = Arc::new(Mutex::new(mi00));
        let initial = if settings.handshake {
            SessionState::Starting
        } else {
            SessionState::Active
        };
        let shared = Arc::new(Shared::new(initial));

        {
            let mut handle = writer.lock().unwrap_or_else(|e| e.into_inner());
            handle
                .write_chunks(&heartbeat_message(&settings.session_id))
                .map_err(SessionError::StartupHeartbeat)?;
        }
        shared.heartbeats_sent.store(1, Ordering::Relaxed);

        let mut threads: Vec<(&'static str, JoinHandle<()>)> = Vec::new();
        let hb_shared = Arc::clone(&shared);
        let hb_writer = Arc::clone(&writer);
        let hb_settings = settings.clone();
        threads.push((
            "heartbeat",
            std::thread::Builder::new()
                .name("session-heartbeat".into())
                .spawn(move || heartbeat_thread(&hb_writer, &hb_settings, &hb_shared))
                .map_err(|e| SessionError::Setup(HidrawIoError::Clone(e)))?,
        ));

        if let Some(reader_handle) = reader_handle {
            let (tx, rx) = mpsc::channel();
            let rd_shared = Arc::clone(&shared);
            let rd_writer = Arc::clone(&writer);
            threads.push((
                "reader",
                std::thread::Builder::new()
                    .name("session-reader".into())
                    .spawn(move || reader_thread(reader_handle, &rd_writer, &tx, &rd_shared))
                    .map_err(|e| SessionError::Setup(HidrawIoError::Clone(e)))?,
            ));
            let ctl_shared = Arc::clone(&shared);
            let ctl_writer = Arc::clone(&writer);
            threads.push((
                "controller",
                std::thread::Builder::new()
                    .name("session-controller".into())
                    .spawn(move || {
                        let mut controller = Controller {
                            settings,
                            shared: ctl_shared,
                            writer: ctl_writer,
                            events: rx,
                            next_id: 1,
                            ready_pending: false,
                        };
                        controller.run();
                    })
                    .map_err(|e| SessionError::Setup(HidrawIoError::Clone(e)))?,
            ));
        } else {
            shared.generation.store(1, Ordering::Release);
            shared.handshakes.store(1, Ordering::Relaxed);
        }

        Ok(Self { shared, threads })
    }

    /// A cloneable view for the video loop.
    pub fn handle(&self) -> SessionHandle {
        SessionHandle {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Stops all threads and returns the counters.
    pub fn shutdown(mut self) -> Result<SessionStats, SessionError> {
        self.shared.stop.store(true, Ordering::Relaxed);
        let mut panicked = None;
        for (name, thread) in self.threads.drain(..) {
            if thread.join().is_err() {
                panicked = Some(name);
            }
        }
        if let Some(name) = panicked {
            return Err(SessionError::Panicked(name));
        }
        self.shared.set_state(SessionState::Stopped);
        Ok(SessionStats {
            heartbeats_sent: self.shared.heartbeats_sent.load(Ordering::Relaxed),
            heartbeat_errors: self.shared.heartbeat_errors.load(Ordering::Relaxed),
            dock_heartbeats_seen: self.shared.dock_heartbeats.load(Ordering::Relaxed),
            handshakes: self.shared.handshakes.load(Ordering::Relaxed),
            losses: self.shared.losses.load(Ordering::Relaxed),
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        for (_, thread) in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// Sleeps until `deadline` in short slices; returns false when stopped first.
fn sleep_until(deadline: Instant, shared: &Shared) -> bool {
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if shared.stopped() {
            return false;
        }
        std::thread::sleep(left.min(POLL_SLICE));
    }
    !shared.stopped()
}

/// Heartbeat writer: first tick one interval after the synchronous startup heartbeat, then one
/// per interval against fixed deadlines, with the constant session id.
fn heartbeat_thread(writer: &SharedWriter, settings: &SessionSettings, shared: &Shared) {
    let start = Instant::now();
    let message = heartbeat_message(&settings.session_id);
    let mut tick: u32 = 1;
    loop {
        let deadline = start + settings.heartbeat_interval * tick;
        if !sleep_until(deadline, shared) {
            return;
        }
        let result = {
            let mut handle = writer.lock().unwrap_or_else(|e| e.into_inner());
            handle.write_chunks(&message)
        };
        match result {
            Ok(_) => {
                shared.heartbeats_sent.fetch_add(1, Ordering::Relaxed);
                shared
                    .consecutive_heartbeat_failures
                    .store(0, Ordering::Relaxed);
            }
            Err(e) => {
                shared.heartbeat_errors.fetch_add(1, Ordering::Relaxed);
                let failures = shared
                    .consecutive_heartbeat_failures
                    .fetch_add(1, Ordering::Relaxed)
                    + 1;
                warn!(error = %e, failures, "heartbeat write failed, will retry on the next tick");
            }
        }
        tick = tick.saturating_add(1);
    }
}

/// MI_00 reader: always drains the dock's IN reports so the dock's writer never blocks.
fn reader_thread(
    mut handle: HidrawHandle,
    writer: &SharedWriter,
    events: &Sender<DockEvent>,
    shared: &Shared,
) {
    let mut assembler = InboundAssembler::new();
    while !shared.stopped() {
        match handle.wait_readable(POLL_SLICE) {
            Ok(false) => continue,
            Ok(true) => {}
            Err(e) => {
                shared.set_fatal(format!("poll on MI_00 failed: {e}"));
                shared.set_state(SessionState::Stopped);
                return;
            }
        }
        let report = match handle.read_report() {
            Ok(report) => report,
            Err(e) => {
                if !shared.stopped() {
                    shared.set_fatal(format!("read from MI_00 failed: {e}"));
                    shared.set_state(SessionState::Stopped);
                }
                return;
            }
        };
        let Some(text) = assembler.feed(&report) else {
            continue;
        };
        match parse_dock_message(&text) {
            DockMessage::Heartbeat => shared.note_dock_heartbeat(),
            DockMessage::Ready => {
                let _ = events.send(DockEvent::Ready);
            }
            DockMessage::Passthrough { id } => {
                if id == Some(messages::EDID_PASSTHROUGH_ID) {
                    if let Err(e) = send_control(writer, messages::EDID_ACK) {
                        warn!(error = %e, "couldn't answer the dock's EDID passthrough");
                    }
                }
            }
            DockMessage::Reply { id, result, error } => {
                let _ = events.send(DockEvent::Reply { id, result, error });
            }
            DockMessage::Other { method } => debug!(%method, "dock notification"),
            DockMessage::Unrecognized => debug!(len = text.len(), "unrecognized dock message"),
        }
    }
}

/// How a wait ended early.
enum Interrupted {
    Stopped,
}

/// Outcome of the handshake's bounded waits.
enum Wait<T> {
    Got(T),
    TimedOut,
}

struct Controller {
    settings: SessionSettings,
    shared: Arc<Shared>,
    writer: SharedWriter,
    events: Receiver<DockEvent>,
    next_id: u64,
    /// A ready announcement the monitor consumed; the next handshake starts from it instead
    /// of waiting for another one.
    ready_pending: bool,
}

impl Controller {
    fn run(&mut self) {
        while !self.shared.stopped() {
            if self.shared.fatal.lock().is_ok_and(|f| f.is_some()) {
                return;
            }
            match self.handshake() {
                Ok(state) => {
                    self.shared.set_state(state);
                    self.shared.generation.fetch_add(1, Ordering::AcqRel);
                    self.shared.handshakes.fetch_add(1, Ordering::Relaxed);
                    info!(?state, "session armed, video allowed");
                    let Some(reason) = self.monitor() else {
                        return;
                    };
                    self.shared.losses.fetch_add(1, Ordering::Relaxed);
                    self.shared.set_state(SessionState::Lost);
                    warn!(%reason, "session lost, redoing the handshake");
                }
                Err(HandshakeEnd::Stopped) => return,
                Err(HandshakeEnd::Failed(e)) => {
                    self.shared.set_state(SessionState::Lost);
                    warn!(error = %e, "handshake failed, retrying");
                    if !sleep_until(Instant::now() + self.settings.retry_delay, &self.shared) {
                        return;
                    }
                }
            }
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn send(&self, payload: &[u8]) -> Result<(), HandshakeEnd> {
        send_control(&self.writer, payload).map_err(HandshakeEnd::Failed)
    }

    /// Waits up to `timeout` for an event accepted by `pick`; other events are discarded.
    fn wait_event<T>(
        &self,
        timeout: Duration,
        mut pick: impl FnMut(&DockEvent) -> Option<T>,
    ) -> Result<Wait<T>, Interrupted> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.shared.stopped() || self.shared.fatal.lock().is_ok_and(|f| f.is_some()) {
                return Err(Interrupted::Stopped);
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Ok(Wait::TimedOut);
            };
            match self.events.recv_timeout(left.min(POLL_SLICE)) {
                Ok(event) => {
                    if let Some(found) = pick(&event) {
                        return Ok(Wait::Got(found));
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err(Interrupted::Stopped),
            }
        }
    }

    fn drain_events(&self) {
        while self.events.try_recv().is_ok() {}
    }

    /// Waits up to the reply timeout for the reply to request `id`. `None` means nothing
    /// arrived in time; the caller proceeds regardless because the dock acts on the request
    /// even when its reply is not delivered. With `strict` an error reply fails the attempt.
    fn wait_reply(
        &self,
        request: &'static str,
        id: u64,
        strict: bool,
    ) -> Result<Option<Value>, HandshakeEnd> {
        let timeout = self.settings.reply_timeout;
        let reply = self.wait_event(timeout, |event| match event {
            DockEvent::Reply {
                id: got,
                result,
                error,
            } if *got == id => Some((result.clone(), error.clone())),
            _ => None,
        })?;
        match reply {
            Wait::Got((result, error)) => {
                if let Some(error) = error {
                    if strict {
                        return Err(HandshakeEnd::Failed(SessionError::Rejected {
                            request,
                            detail: truncate(&error.to_string()),
                        }));
                    }
                    warn!(request, error = %truncate(&error.to_string()), "dock rejected a request on the silent path, continuing");
                    return Ok(None);
                }
                Ok(Some(result.unwrap_or(Value::Null)))
            }
            Wait::TimedOut => {
                warn!(
                    request,
                    ?timeout,
                    dock_alive = self.shared.dock_alive(),
                    "no answer from the dock, continuing"
                );
                Ok(None)
            }
        }
    }

    /// Runs one handshake and returns the armed state to enter.
    ///
    /// The full message sequence is sent whether or not the dock announced ready or answers:
    /// a dock that announces nothing still processes the messages. Replies are waited for
    /// only within the reply timeout. `Active` needs the ready announcement and `isAllowed`;
    /// every other outcome arms as `AssumedActive` after the settle delay.
    fn handshake(&mut self) -> Result<SessionState, HandshakeEnd> {
        self.drain_events();
        self.shared.set_state(SessionState::WaitingForReady);
        let ready_timeout = self.settings.ready_timeout;
        let ready = if std::mem::take(&mut self.ready_pending) {
            Wait::Got(())
        } else {
            self.wait_event(ready_timeout, |e| {
                matches!(e, DockEvent::Ready).then_some(())
            })?
        };
        let ready_seen = matches!(ready, Wait::Got(()));
        if !ready_seen {
            warn!(
                ?ready_timeout,
                dock_alive = self.shared.dock_alive(),
                "dock did not announce ready, sending the handshake anyway"
            );
        }

        if ready_seen && self.settings.send_stopped {
            self.send(messages::PROJECTION_STOPPED)?;
        }
        let launch_id = self.next_id();
        self.shared.set_state(SessionState::AppLaunchedSent);
        self.send(&messages::app_launched(
            launch_id,
            &self.settings.machine_name,
            self.settings.size,
        ))?;
        let mut confirmed = self
            .wait_reply("app-launched", launch_id, ready_seen)?
            .is_some();

        self.send(messages::MOUSE_ICON_CHANGE)?;
        self.send(messages::EDID_ACK)?;
        let start_id = self.next_id();
        self.shared.set_state(SessionState::ProjectionStartedSent);
        self.send(&messages::projection_started(start_id))?;
        match self.wait_reply("notify-projection-started", start_id, ready_seen)? {
            None => confirmed = false,
            Some(Value::String(s)) if s == "isAllowed" => {}
            Some(other) if ready_seen => {
                return Err(HandshakeEnd::Failed(SessionError::Denied(truncate(
                    &other.to_string(),
                ))))
            }
            Some(_) => confirmed = false,
        }

        self.shared.set_state(SessionState::Settling);
        if !sleep_until(Instant::now() + self.settings.settle_delay, &self.shared) {
            return Err(HandshakeEnd::Stopped);
        }
        Ok(if ready_seen && confirmed {
            SessionState::Active
        } else {
            SessionState::AssumedActive
        })
    }

    /// Watches an armed session. Returns the loss reason, or `None` when shut down or the
    /// reader died.
    fn monitor(&mut self) -> Option<String> {
        // Announcements that were queued while the handshake ran don't count as loss.
        self.drain_events();
        loop {
            if self.shared.stopped() || self.shared.fatal.lock().is_ok_and(|f| f.is_some()) {
                return None;
            }
            if let Some(reason) = self
                .shared
                .loss_reason
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                return Some(reason);
            }
            if self
                .shared
                .consecutive_heartbeat_failures
                .load(Ordering::Relaxed)
                >= self.settings.max_heartbeat_failures
            {
                self.shared
                    .consecutive_heartbeat_failures
                    .store(0, Ordering::Relaxed);
                return Some("heartbeat writes keep failing".into());
            }
            match self.events.recv_timeout(POLL_SLICE) {
                Ok(DockEvent::Ready) => {
                    self.ready_pending = true;
                    return Some("dock announced ready".into());
                }
                Ok(DockEvent::Reply { .. }) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }
}

enum HandshakeEnd {
    Stopped,
    Failed(SessionError),
}

impl From<Interrupted> for HandshakeEnd {
    fn from(_: Interrupted) -> Self {
        HandshakeEnd::Stopped
    }
}

fn truncate(text: &str) -> String {
    text.chars().take(LOG_TEXT_MAX * 2).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_dock::{FakeDock, FakeDockConfig, StartedReply};
    use crate::framing::frame_message;

    fn size() -> PictureSize {
        PictureSize::new(1280, 720)
    }

    fn fast_settings() -> SessionSettings {
        let mut s = SessionSettings::new(size(), Duration::from_millis(50));
        s.session_id = SessionId::new("25282281").unwrap();
        s.machine_name = "VERMELHO".into();
        s.ready_timeout = Duration::from_millis(400);
        s.reply_timeout = Duration::from_millis(400);
        s.settle_delay = Duration::from_millis(60);
        s.retry_delay = Duration::from_millis(50);
        s
    }

    fn wait_for(what: &str, limit: Duration, mut cond: impl FnMut() -> bool) {
        let start = Instant::now();
        while !cond() {
            assert!(start.elapsed() < limit, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn texts(dock: &FakeDock) -> Vec<String> {
        dock.host_texts()
    }

    fn non_heartbeat(dock: &FakeDock) -> Vec<String> {
        texts(dock)
            .into_iter()
            .filter(|t| !t.contains("\"heartbeat\""))
            .collect()
    }

    /// What a silent dock must receive, in order: no projection-stopped, since the dock never
    /// announced ready.
    fn silent_sequence() -> Vec<String> {
        [
            messages::app_launched(1, "VERMELHO", size()),
            messages::MOUSE_ICON_CHANGE.to_vec(),
            messages::EDID_ACK.to_vec(),
            messages::projection_started(2),
        ]
        .into_iter()
        .map(|m| String::from_utf8(m).unwrap())
        .collect()
    }

    #[test]
    fn silent_sequence_has_the_exact_vendor_bytes() {
        assert_eq!(
            silent_sequence(),
            [
                "{\"id\":1,\"jsonrpc\":\"2.0\",\"method\":\"app-launched\",\"params\":{\"platform\":\"windows\",\"platform-machine-name\":\"VERMELHO\",\"product-type\":\"QuattroTx CT10\",\"res-height\":720,\"res-width\":1280}}\n",
                "{\"jsonrpc\":\"2.0\",\"method\":\"app_tx_mouse_icon_change\",\"params\":{\"cursor_base64_data\":\"\"}}\n",
                "{\"jsonrpc\":\"2.0\",\"method\":\"common.data_passthrough\",\"params\":{\"data\":\"AQ==\",\"id\":103,\"size\":1}}\n",
                "{\"id\":2,\"jsonrpc\":\"2.0\",\"method\":\"notify-projection-started\"}\n",
            ]
        );
    }

    #[test]
    fn first_report_bytes_match_the_reference_prototype() {
        let hex =
            |report: &[u8]| -> String { report[..64].iter().map(|b| format!("{b:02x}")).collect() };
        let id = SessionId::new("25282281").unwrap();
        assert_eq!(
            hex(&heartbeat_message(&id)[0]),
            "010000004e00000000000000000000007b226a736f6e727063223a22322e30222c20226d6574686f64223a2022686561727462656174222c2022706172616d73"
        );
        let launched = messages::app_launched(1, "VERMELHO", size());
        let chunks = MessageBuilder::control().payload(&launched).build();
        assert_eq!(
            hex(&chunks[0]),
            "01000000b600000000000000000000007b226964223a312c226a736f6e727063223a22322e30222c226d6574686f64223a226170702d6c61756e63686564222c"
        );
    }

    #[test]
    fn message_bytes_are_exact() {
        assert_eq!(
            messages::app_launched(1, "AZUL", PictureSize::new(1920, 1080)),
            b"{\"id\":1,\"jsonrpc\":\"2.0\",\"method\":\"app-launched\",\"params\":{\"platform\":\"windows\",\"platform-machine-name\":\"AZUL\",\"product-type\":\"QuattroTx CT10\",\"res-height\":1080,\"res-width\":1920}}\n"
        );
        assert_eq!(
            messages::app_launched(3, "AZUL", size()).len(),
            0xb2,
            "captured length of the 1280x720 request"
        );
        assert_eq!(
            messages::projection_started(4),
            b"{\"id\":4,\"jsonrpc\":\"2.0\",\"method\":\"notify-projection-started\"}\n"
        );
        assert_eq!(
            messages::PROJECTION_STOPPED,
            b"{\"jsonrpc\":\"2.0\",\"method\":\"notify-projection-stopped\"}\n"
        );
        assert_eq!(
            messages::MOUSE_ICON_CHANGE,
            b"{\"jsonrpc\":\"2.0\",\"method\":\"app_tx_mouse_icon_change\",\"params\":{\"cursor_base64_data\":\"\"}}\n"
        );
        assert_eq!(
            messages::EDID_ACK,
            b"{\"jsonrpc\":\"2.0\",\"method\":\"common.data_passthrough\",\"params\":{\"data\":\"AQ==\",\"id\":103,\"size\":1}}\n"
        );
    }

    #[test]
    fn machine_name_is_uppercase_and_truncated_to_15() {
        assert_eq!(
            messages::machine_name_from_hostname("vermelho\n"),
            "VERMELHO"
        );
        assert_eq!(
            messages::machine_name_from_hostname("a-very-long-hostname-indeed"),
            "A-VERY-LONG-HOS"
        );
        let quoted = messages::app_launched(1, "A\"B", size());
        assert!(serde_json::from_slice::<Value>(&quoted).is_ok());
    }

    #[test]
    fn assembler_handles_single_and_multi_report_messages() {
        let mut assembler = InboundAssembler::new();
        let single = frame_message(ReportId::Control, PictureSize::zero(), b"{\"a\":1}");
        assert_eq!(assembler.feed(&single[0]).unwrap(), b"{\"a\":1}");

        let big: Vec<u8> = (0..9000).map(|i| b'a' + (i % 26) as u8).collect();
        let chunks = frame_message(ReportId::Control, PictureSize::zero(), &big);
        assert_eq!(chunks.len(), 3);
        assert!(assembler.feed(&chunks[0]).is_none());
        assert!(assembler.feed(&chunks[1]).is_none());
        assert_eq!(assembler.feed(&chunks[2]).unwrap(), big);

        // A stray continuation and an oversized announcement are dropped.
        assert!(assembler.feed(&chunks[1]).is_none());
        let mut huge = vec![0u8; 4096];
        huge[0] = 1;
        huge[1..5].copy_from_slice(&(10_000_000u32).to_be_bytes());
        assert!(assembler.feed(&huge).is_none());
        assert!(assembler.feed(&[1, 2, 3]).is_none());
    }

    #[test]
    fn dock_messages_parse_by_kind() {
        let ready = b"{\n\t\"jsonrpc\":\t\"2.0\",\n\t\"method\":\t\"notify-app-accessories-are-ready\",\n\t\"params\":\t{\n\t\t\"res-width\":\t1920\n\t},\n\t\"jrpcSeq\":\t7730\n}";
        assert_eq!(parse_dock_message(ready), DockMessage::Ready);
        assert_eq!(
            parse_dock_message(br#"{"method":"heartbeat","params":{"session-id":"1"}}"#),
            DockMessage::Heartbeat
        );
        assert_eq!(
            parse_dock_message(br#"{"method":"common.data_passthrough","params":{"id":101,"size":258,"data":"AA=="}}"#),
            DockMessage::Passthrough { id: Some(101) }
        );
        assert_eq!(
            parse_dock_message(br#"{"id":4,"result":"isAllowed","jrpcSeq":7743}"#),
            DockMessage::Reply {
                id: 4,
                result: Some(Value::String("isAllowed".into())),
                error: None
            }
        );
        assert!(matches!(
            parse_dock_message(br#"{"id":4,"error":{"code":-32601}}"#),
            DockMessage::Reply { error: Some(_), .. }
        ));
        assert_eq!(
            parse_dock_message(br#"{"method":"tx-data-updated"}"#),
            DockMessage::Other {
                method: "tx-data-updated".into()
            }
        );
        assert_eq!(parse_dock_message(b"not json"), DockMessage::Unrecognized);
        assert_eq!(parse_dock_message(b"[1,2]"), DockMessage::Unrecognized);
    }

    #[test]
    fn full_handshake_against_a_fresh_dock_sends_the_exact_sequence() {
        let (dock, mi00) = FakeDock::spawn(FakeDockConfig::fresh());
        let session = Session::start(mi00, fast_settings()).unwrap();
        let handle = session.handle();
        assert!(!handle.is_armed(), "not armed before the handshake runs");
        wait_for("armed", Duration::from_secs(5), || handle.is_armed());
        assert_eq!(handle.state(), SessionState::Active);

        // The passthrough answer goes out once from the reader (reply to the dock's EDID) and
        // once from the handshake; their relative order is not fixed.
        let all: Vec<String> = texts(&dock)
            .into_iter()
            .filter(|t| !t.contains("\"heartbeat\""))
            .collect();
        let passthroughs = all
            .iter()
            .filter(|t| t.contains("data_passthrough"))
            .count();
        assert_eq!(passthroughs, 2);
        let sent: Vec<String> = all
            .into_iter()
            .filter(|t| !t.contains("data_passthrough"))
            .collect();
        assert_eq!(
            sent,
            [
                "{\"jsonrpc\":\"2.0\",\"method\":\"notify-projection-stopped\"}\n",
                "{\"id\":1,\"jsonrpc\":\"2.0\",\"method\":\"app-launched\",\"params\":{\"platform\":\"windows\",\"platform-machine-name\":\"VERMELHO\",\"product-type\":\"QuattroTx CT10\",\"res-height\":720,\"res-width\":1280}}\n",
                "{\"jsonrpc\":\"2.0\",\"method\":\"app_tx_mouse_icon_change\",\"params\":{\"cursor_base64_data\":\"\"}}\n",
                "{\"id\":2,\"jsonrpc\":\"2.0\",\"method\":\"notify-projection-started\"}\n",
            ]
        );
        // Heartbeats carry the exact vendor text and the constant id.
        let beats: Vec<String> = texts(&dock)
            .into_iter()
            .filter(|t| t.contains("\"heartbeat\""))
            .collect();
        assert!(!beats.is_empty());
        assert!(beats.iter().all(|b| b
            == r#"{"jsonrpc":"2.0", "method": "heartbeat", "params": {"session-id": "25282281"}}"#));
        assert_eq!(dock.format_violations(), 0);
        let stats = session.shutdown().unwrap();
        assert_eq!(stats.handshakes, 1);
        assert!(stats.dock_heartbeats_seen > 0);
    }

    #[test]
    fn video_is_not_armed_until_is_allowed_plus_settle_delay() {
        let mut config = FakeDockConfig::fresh();
        config.launch_reply_delay = Some(Duration::from_millis(150));
        let (dock, mi00) = FakeDock::spawn(config);
        let mut settings = fast_settings();
        settings.settle_delay = Duration::from_millis(300);
        let session = Session::start(mi00, settings).unwrap();
        let handle = session.handle();
        wait_for("armed", Duration::from_secs(5), || handle.is_armed());
        let armed_at = Instant::now();
        let allowed_at = dock.allowed_sent_at().expect("dock sent isAllowed");
        assert!(
            armed_at.duration_since(allowed_at) >= Duration::from_millis(250),
            "armed only {:?} after isAllowed",
            armed_at.duration_since(allowed_at)
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn silent_dock_with_heartbeats_ends_in_assumed_active() {
        // The dock already holds a session: heartbeats only, no ready, no replies.
        let (dock, mi00) = FakeDock::spawn(FakeDockConfig::holding_session());
        let session = Session::start(mi00, fast_settings()).unwrap();
        let handle = session.handle();
        let started = Instant::now();
        wait_for("armed", Duration::from_secs(5), || handle.is_armed());
        assert_eq!(handle.state(), SessionState::AssumedActive);
        assert!(
            started.elapsed() >= Duration::from_millis(350),
            "waited the ready timeout"
        );
        assert_eq!(non_heartbeat(&dock), silent_sequence());
        session.shutdown().unwrap();
    }

    #[test]
    fn unanswered_requests_time_out_into_assumed_active_when_dock_is_alive() {
        let mut config = FakeDockConfig::fresh();
        config.launch_reply_delay = None;
        let (_dock, mi00) = FakeDock::spawn(config);
        let session = Session::start(mi00, fast_settings()).unwrap();
        let handle = session.handle();
        wait_for("armed", Duration::from_secs(5), || handle.is_armed());
        assert_eq!(handle.state(), SessionState::AssumedActive);
        session.shutdown().unwrap();
    }

    #[test]
    fn fully_silent_dock_arms_as_assumed_active_and_video_flows() {
        // No ready, no heartbeat echo, no replies: the dock still renders written video.
        let mut config = FakeDockConfig::holding_session();
        config.heartbeat_interval = None;
        let (dock, mi00, mi01) = FakeDock::spawn_with_video(config);
        let session = Session::start(mi00, fast_settings()).unwrap();
        let handle = session.handle();
        wait_for("armed", Duration::from_secs(5), || handle.is_armed());
        assert_eq!(handle.state(), SessionState::AssumedActive);
        assert_eq!(handle.generation(), 1);

        assert_eq!(non_heartbeat(&dock), silent_sequence());
        let mut mi01 = mi01;
        let chunks = crate::framing::MessageBuilder::video(size())
            .payload(&[0xAB; 5000])
            .build();
        for _ in 0..3 {
            mi01.write_chunks(&chunks).unwrap();
        }
        wait_for("video seen", Duration::from_secs(3), || {
            dock.video_stats().messages == 3
        });
        let video = dock.video_stats();
        assert_eq!(video.reports_before_sink, 0);
        assert_eq!(video.layout_violations, 0);
        // The handshake messages all precede the first video report; nothing else follows.
        assert_eq!(non_heartbeat(&dock), silent_sequence());
        let before = video.control_messages_before_video.unwrap();
        assert!(before >= 4);
        let stats = session.shutdown().unwrap();
        assert_eq!(stats.handshakes, 1);
        assert_eq!(stats.dock_heartbeats_seen, 0);
        assert!(stats.heartbeats_sent >= 2);
    }

    #[test]
    fn late_ready_after_assumed_active_runs_the_full_handshake() {
        let mut config = FakeDockConfig::holding_session();
        config.launch_reply_delay = Some(Duration::from_millis(30));
        config.started_reply = StartedReply::Allowed;
        config.send_edid = true;
        let (dock, mi00) = FakeDock::spawn(config);
        let session = Session::start(mi00, fast_settings()).unwrap();
        let handle = session.handle();
        wait_for("assumed", Duration::from_secs(5), || handle.is_armed());
        assert_eq!(handle.state(), SessionState::AssumedActive);
        let generation = handle.generation();

        // The dock loses its session and starts announcing: the handshake must follow at once.
        dock.drop_session();
        wait_for("handshake", Duration::from_secs(5), || {
            handle.generation() > generation && handle.is_armed()
        });
        assert_eq!(handle.state(), SessionState::Active);
        let launches = texts(&dock)
            .iter()
            .filter(|t| t.contains("\"app-launched\""))
            .count();
        // One from the silent first attempt, one after the dock announced ready.
        assert_eq!(launches, 2);
        assert!(dock.sink_started());
        let stats = session.shutdown().unwrap();
        assert_eq!(stats.handshakes, 2);
        assert_eq!(stats.losses, 1);
    }

    #[test]
    fn denied_projection_does_not_arm() {
        let mut config = FakeDockConfig::fresh();
        config.started_reply = StartedReply::Denied;
        let (_dock, mi00) = FakeDock::spawn(config);
        let session = Session::start(mi00, fast_settings()).unwrap();
        let handle = session.handle();
        // The denied attempt fails fast and never arms.
        std::thread::sleep(Duration::from_millis(300));
        assert!(!handle.is_armed());
        assert_eq!(handle.generation(), 0);
        // The dock now holds app state and stops announcing, so the retry ends in the
        // bounded fallback rather than blocking video forever.
        wait_for("fallback", Duration::from_secs(5), || handle.is_armed());
        assert_eq!(handle.state(), SessionState::AssumedActive);
        assert_eq!(handle.generation(), 1);
        session.shutdown().unwrap();
    }

    #[test]
    fn error_reply_to_app_launched_fails_the_attempt_and_retries() {
        let (dock, mi00) = FakeDock::spawn(FakeDockConfig::fresh());
        dock.set_launch_error(true);
        let session = Session::start(mi00, fast_settings()).unwrap();
        let handle = session.handle();
        std::thread::sleep(Duration::from_millis(250));
        assert!(!handle.is_armed());
        assert_eq!(handle.generation(), 0);
        wait_for("fallback", Duration::from_secs(5), || handle.is_armed());
        assert_eq!(handle.state(), SessionState::AssumedActive);
        session.shutdown().unwrap();
    }

    #[test]
    fn re_announced_ready_redoes_the_handshake_without_restarting() {
        let (dock, mi00) = FakeDock::spawn(FakeDockConfig::fresh());
        let session = Session::start(mi00, fast_settings()).unwrap();
        let handle = session.handle();
        wait_for("armed", Duration::from_secs(5), || handle.is_armed());
        let first_generation = handle.generation();

        dock.drop_session();
        wait_for("re-armed", Duration::from_secs(5), || {
            handle.is_armed() && handle.generation() > first_generation
        });
        let launches = texts(&dock)
            .iter()
            .filter(|t| t.contains("\"app-launched\""))
            .count();
        assert_eq!(launches, 2);
        // Ids keep counting up across handshakes.
        assert!(texts(&dock)
            .iter()
            .any(|t| t.starts_with("{\"id\":3,") && t.contains("app-launched")));
        let stats = session.shutdown().unwrap();
        assert_eq!(stats.handshakes, 2);
        assert_eq!(stats.losses, 1);
    }

    #[test]
    fn reported_write_failure_redoes_the_handshake() {
        let (dock, mi00) = FakeDock::spawn(FakeDockConfig::fresh());
        let session = Session::start(mi00, fast_settings()).unwrap();
        let handle = session.handle();
        wait_for("armed", Duration::from_secs(5), || handle.is_armed());
        let generation = handle.generation();
        // The dock still holds its session, so the redo ends without a ready announcement.
        let _ = &dock;
        handle.report_failure("video write failed");
        wait_for("re-armed", Duration::from_secs(5), || {
            handle.generation() > generation && handle.is_armed()
        });
        session.shutdown().unwrap();
    }

    #[test]
    fn heartbeat_keeps_its_cadence_independent_of_other_traffic() {
        let (dock, mi00) = FakeDock::spawn(FakeDockConfig::holding_session());
        let mut settings = fast_settings();
        settings.heartbeat_interval = Duration::from_millis(40);
        settings.ready_timeout = Duration::from_secs(30);
        let session = Session::start(mi00, settings).unwrap();
        std::thread::sleep(Duration::from_millis(600));
        let times = dock.heartbeat_times();
        assert!(times.len() >= 10, "{} heartbeats", times.len());
        let worst = times
            .windows(2)
            .map(|w| w[1].duration_since(w[0]))
            .max()
            .unwrap();
        assert!(worst < Duration::from_millis(200), "worst gap {worst:?}");
        session.shutdown().unwrap();
    }

    #[test]
    fn startup_write_failure_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ro");
        std::fs::write(&path, b"").unwrap();
        let ro = HidrawHandle::from_file(
            std::fs::OpenOptions::new().read(true).open(&path).unwrap(),
            ReportId::Control,
        );
        let err = Session::start(ro, fast_settings().without_handshake()).unwrap_err();
        assert!(matches!(err, SessionError::StartupHeartbeat(_)));
        let video = HidrawHandle::from_file(std::fs::File::open(&path).unwrap(), ReportId::Video);
        assert!(matches!(
            Session::start(video, fast_settings()).unwrap_err(),
            SessionError::WrongChannel { .. }
        ));
    }

    #[test]
    fn without_handshake_arms_immediately_and_only_heartbeats() {
        let dir = tempfile::tempdir().unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(dir.path().join("mi00"))
            .unwrap();
        let handle = HidrawHandle::from_file(file, ReportId::Control);
        let session = Session::start(handle, fast_settings().without_handshake()).unwrap();
        assert!(session.handle().is_armed());
        std::thread::sleep(Duration::from_millis(180));
        let stats = session.shutdown().unwrap();
        assert!(stats.heartbeats_sent >= 3);
        assert_eq!(stats.handshakes, 1);
    }

    #[test]
    fn reader_end_of_stream_is_fatal() {
        // A regular file reads as end of stream immediately.
        let dir = tempfile::tempdir().unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(dir.path().join("mi00"))
            .unwrap();
        let handle = HidrawHandle::from_file(file, ReportId::Control);
        let session = Session::start(handle, fast_settings()).unwrap();
        let sh = session.handle();
        wait_for("fatal", Duration::from_secs(3), || sh.fatal().is_some());
        assert!(!sh.is_armed());
        session.shutdown().unwrap();
    }
}
