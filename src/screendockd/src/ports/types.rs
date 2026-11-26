//! Types of the ports: frames, messages, settings, errors and the port traits.

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

use thiserror::Error;

use crate::protocol::annexb::AnnexBError;
use crate::protocol::control::{ControlRequest, ControlResponse};
use crate::protocol::display::DisplayGeometry;
use crate::protocol::framing::{FramingError, ParsedMessage, ReportId, REPORT_SIZE};
use crate::utils::config::X264Preset;

/// Layout of the bytes in one pixel of a [`Frame`]. All supported layouts use four bytes per
/// pixel; the name lists the byte order in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Bytes B, G, R, unused (X11 little-endian 24-bit depth in 32-bit pixels).
    Bgrx8888,
    /// Bytes B, G, R, A.
    Bgra8888,
    /// Bytes R, G, B, unused.
    Rgbx8888,
    /// Bytes R, G, B, A.
    Rgba8888,
}

/// One captured image. Rows are `stride` bytes apart and may carry padding after the last pixel.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// Bytes from the start of one row to the start of the next.
    pub stride: usize,
    pub data: Vec<u8>,
}

/// Failures from selecting or running a capture backend.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// Neither `$XDG_SESSION_TYPE`, `$WAYLAND_DISPLAY` nor `$DISPLAY` identify a session.
    #[error(
        "no graphical session detected ($XDG_SESSION_TYPE, $WAYLAND_DISPLAY and $DISPLAY are unset)"
    )]
    NoSession,
    /// The selected backend exists but cannot run in this build or session.
    #[error("capture backend {backend} unavailable: {reason}")]
    BackendUnavailable {
        backend: &'static str,
        reason: String,
    },
    /// Connecting to the display server or portal failed.
    #[error("capture connection failed: {0}")]
    Connection(String),
    /// Shared memory setup or the XShm request failed.
    #[error("shared memory capture failed: {0}")]
    Shm(String),
    /// The desktop portal refused or aborted the ScreenCast session.
    #[error("screencast portal error: {0}")]
    Portal(String),
    /// The PipeWire stream failed or ended.
    #[error("pipewire stream error: {0}")]
    Stream(String),
    /// The backend produced a buffer that does not match its stated geometry or format.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),
    /// PNG encoding failed.
    #[error("png encoding failed: {0}")]
    Png(String),
    /// The backend cannot do what was asked while it runs.
    #[error("{0} is not supported by this capture backend")]
    Unsupported(&'static str),
    /// The requested capture area does not fit the source.
    #[error("invalid capture target: {0}")]
    InvalidTarget(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// Common interface the encode loop consumes, independent of which backend is active. Object
/// safe, and `Send` so a boxed backend can move into the capture thread.
pub trait CaptureBackend: Send {
    /// Short backend identifier for logs (`"x11-shm"`, `"wayland-portal"`).
    fn name(&self) -> &'static str;

    /// Blocks until the next frame is due and returns it.
    fn next_frame(&mut self) -> Result<Frame, CaptureError>;

    /// Points the running backend at another area and frame rate. Frames after the call
    /// carry the new area's size. Backends that cannot do this keep the default, which
    /// refuses.
    fn retarget(&mut self, _target: CaptureTarget) -> Result<(), CaptureError> {
        Err(CaptureError::Unsupported("retargeting while running"))
    }
}

/// The part of the source a backend captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureArea {
    /// The whole source (the root window on X11).
    Whole,
    /// A rectangle of the source in its own coordinates.
    Region(Rect),
}

/// Where a capture backend looks and how fast it produces frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureTarget {
    pub area: CaptureArea,
    /// Frames per second; zero disables pacing.
    pub fps: u32,
}

/// Frame rate used when the config leaves pacing disabled (`capture.fps = 0`).
pub const DEFAULT_FPS: u32 = 25;

/// One dock video message: Annex-B bytes ready for `framing::MessageBuilder::video`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoMessage {
    pub(super) bytes: Vec<u8>,
    pub(super) keyframe: bool,
}

/// Errors from encoding.
#[derive(Debug, Error)]
pub enum EncodeError {
    /// The requested encoder is not part of this build.
    #[error("encoder {name} unavailable: {reason}")]
    Unavailable { name: &'static str, reason: String },
    /// The encoder library failed to open or configure.
    #[error("encoder setup failed: {0}")]
    Setup(String),
    /// The encoder library rejected a frame or failed while encoding.
    #[error("encoding failed: {0}")]
    Backend(String),
    /// The encoder cannot do what was asked while it runs.
    #[error("{0} is not supported by this encoder")]
    Unsupported(&'static str),
    /// The encoder output did not split into dock messages.
    #[error("encoder output is not valid Annex-B: {0}")]
    AnnexB(#[from] AnnexBError),
    /// A prerecorded stream does not start with a keyframe bundle.
    #[error("prerecorded stream must start with an IDR keyframe")]
    NoLeadingKeyframe,
    /// Reading a prerecorded stream failed.
    #[error("couldn't read {path:?}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Settings every encoder implementation receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderSettings {
    /// Encoded picture size.
    pub geometry: DisplayGeometry,
    /// Frames per second.
    pub fps: u32,
    /// Frames between keyframes.
    pub keyframe_interval: u32,
    /// Target bit rate in kilobits per second; also the VBV maximum rate.
    pub bitrate_kbps: u32,
    /// libx264 preset.
    pub preset: X264Preset,
    /// Apply libx264's `zerolatency` tune.
    pub zerolatency: bool,
    /// VBV buffer in milliseconds of `bitrate_kbps`; 0 disables the cap.
    pub vbv_buffer_ms: u32,
}

/// The rate settings an encoder can change while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateSettings {
    /// Target bit rate in kilobits per second; also the VBV maximum rate.
    pub bitrate_kbps: u32,
    /// VBV buffer in milliseconds of `bitrate_kbps`; 0 disables the cap.
    pub vbv_buffer_ms: u32,
}

/// Converts frames to dock video messages.
pub trait Encoder {
    /// Short identifier for logs.
    fn name(&self) -> &'static str;

    /// Encodes one captured frame. The result holds zero or more messages in send order:
    /// a keyframe bundle or a single inter-frame NAL, normally exactly one.
    fn encode(&mut self, frame: &Frame) -> Result<Vec<VideoMessage>, EncodeError>;

    /// Makes the next output a keyframe bundle, used after a failed write so the panel can
    /// resynchronize.
    fn request_keyframe(&mut self);

    /// Changes the target rate and the VBV buffer without rebuilding the encoder. The
    /// stream stays decodable: no keyframe is needed and no state is lost. Encoders that
    /// cannot do this keep the default, which refuses.
    fn set_rate(&mut self, _rate: RateSettings) -> Result<(), EncodeError> {
        Err(EncodeError::Unsupported("changing the rate while running"))
    }

    /// True when [`set_rate`](Encoder::set_rate) takes effect on the running encoder.
    fn supports_live_rate(&self) -> bool {
        false
    }
}

/// Errors from opening, writing to or reading from a dock transport (a hidraw node).
#[derive(Debug, Error)]
pub enum TransportError {
    /// The node couldn't be opened (missing, no permission, ...).
    #[error("couldn't open hidraw node {path:?}: {source}")]
    Open { path: PathBuf, source: io::Error },
    /// A chunk was not exactly `REPORT_SIZE` bytes long.
    #[error("chunk {index} is {len} bytes, expected exactly {REPORT_SIZE}")]
    BadChunkSize { index: usize, len: usize },
    /// A chunk's Report ID byte didn't match the channel this handle was opened for.
    #[error("chunk {index} has Report ID {got}, but this handle is the {channel:?} channel")]
    WrongChannel {
        index: usize,
        channel: ReportId,
        got: u8,
    },
    /// `write(2)` failed on a chunk.
    #[error("write of chunk {index} failed: {source}")]
    Write { index: usize, source: io::Error },
    /// `write(2)` accepted fewer bytes than the chunk holds.
    #[error("short write on chunk {index}: {written} of {expected} bytes")]
    ShortWrite {
        index: usize,
        written: usize,
        expected: usize,
    },
    /// `read(2)` failed.
    #[error("read failed: {0}")]
    Read(io::Error),
    /// Duplicating the descriptor failed.
    #[error("couldn't duplicate the hidraw descriptor: {0}")]
    Clone(io::Error),
    /// `poll(2)` failed.
    #[error("poll failed: {0}")]
    Poll(io::Error),
    /// The read side hit end-of-file (or a zero-length read) before a full message arrived.
    #[error("end of stream while reading chunk {index}")]
    UnexpectedEof { index: usize },
    /// The inbound chunks didn't parse as a valid message.
    #[error("inbound message failed to parse: {0}")]
    Framing(#[from] FramingError),
}

/// Per-chunk write timing for one message, in write order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteTimings {
    /// Duration of each individual `write(2)` call.
    pub per_chunk: Vec<Duration>,
    /// Wall time from just before the first write to just after the last.
    pub total: Duration,
}

/// One of the dock's two channels as the session, the daemon and the validation tools see it:
/// framed reports go out through [`write_chunks`](DockTransport::write_chunks) and come back
/// through [`read_report`](DockTransport::read_report) and [`read_message`](DockTransport::read_message).
pub trait DockTransport: Send + 'static {
    /// The channel this transport was opened for.
    fn channel(&self) -> ReportId;

    /// Writes each chunk with one write call, in order, and returns per-chunk timing.
    fn write_chunks(&mut self, chunks: &[Vec<u8>]) -> Result<WriteTimings, TransportError>;

    /// Reads one raw report without interpreting it.
    fn read_report(&mut self) -> Result<Vec<u8>, TransportError>;

    /// Reads and parses one complete message.
    fn read_message(&mut self) -> Result<ParsedMessage, TransportError>;

    /// Waits up to `timeout` for the transport to become readable.
    fn wait_readable(&self, timeout: Duration) -> Result<bool, TransportError>;

    /// Duplicates the transport, so a reader thread and a writer can each own one.
    fn try_clone(&self) -> Result<Box<dyn DockTransport>, TransportError>;
}

/// Identifier of an output (a display connector) on the display server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OutputId(pub u32);

/// A rectangle in desktop coordinates. The origin may be negative on desktops that allow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// The display server the session talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayServer {
    X11,
    Wayland,
    /// No graphical session was found.
    #[default]
    None,
}

/// One output of the desktop as the display server reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFacts {
    pub id: OutputId,
    pub name: String,
    pub rect: Rect,
    pub primary: bool,
}

/// What the environment offers for creating a virtual monitor. Gathered by an adapter, read by
/// the display policy; holds data only.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplayFacts {
    pub server: DisplayServer,
    /// `$XDG_CURRENT_DESKTOP`, for example `ubuntu:GNOME`. Empty when unset.
    pub desktop: String,
    pub outputs: Vec<OutputFacts>,
    /// RandR version as (major, minor) when the X server has the extension.
    pub randr_version: Option<(u32, u32)>,
    /// An output that can take a custom mode, so the monitor can be a real output.
    pub spare_output: Option<OutputId>,
    /// `org.gnome.Mutter.ScreenCast` is on the session bus.
    pub mutter_screencast: bool,
    /// The portal's `AvailableSourceTypes` bit mask, when a portal answered.
    pub portal_source_types: Option<u32>,
    /// A sway IPC socket is reachable.
    pub sway_socket: bool,
}

/// Work for the daemon's supervisor, from the control server or a signal handler.
#[derive(Debug)]
pub enum Command {
    /// A control request; the answer goes back through `reply`.
    Request {
        request: ControlRequest,
        reply: Sender<ControlResponse>,
    },
    /// Re-read the config file (SIGHUP). No answer is sent.
    Reload,
}

/// Errors from the control socket, on the server and the client side.
#[derive(Debug, Error)]
pub enum ControlError {
    /// No daemon answers on the socket.
    #[error("no daemon is listening on {path:?}: {source}")]
    Unreachable { path: PathBuf, source: io::Error },
    /// Another daemon already serves the socket.
    #[error("another daemon already serves {0:?}")]
    AlreadyRunning(PathBuf),
    /// The path exists and is not a socket.
    #[error("{0:?} exists and is not a socket")]
    NotASocket(PathBuf),
    /// The path does not fit a Unix socket address.
    #[error("socket path {0:?} is too long")]
    PathTooLong(PathBuf),
    /// Socket I/O failed.
    #[error("control socket i/o failed: {0}")]
    Io(#[from] io::Error),
    /// The peer sent something that is not a control message.
    #[error("control protocol error: {0}")]
    Wire(String),
}

/// Sends one control request to a running daemon and returns its answer.
pub trait ControlClient {
    fn send(&self, request: &ControlRequest) -> Result<ControlResponse, ControlError>;
}

/// Where the dock's EDID is kept between runs. Implementations hold raw bytes and never
/// interpret them.
pub trait EdidStore: Send + Sync + std::fmt::Debug {
    /// The stored bytes, or `None` when nothing usable is stored.
    fn load(&self) -> Option<Vec<u8>>;

    /// Replaces the stored bytes.
    fn save(&self, bytes: &[u8]) -> io::Result<()>;
}
