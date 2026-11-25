//! Types of the ports: frames, messages, settings, errors and the port traits.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

use crate::protocol::annexb::AnnexBError;
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
