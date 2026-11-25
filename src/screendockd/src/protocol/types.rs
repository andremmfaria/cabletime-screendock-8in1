//! Types, enums and constants of the wire protocol: framing, Annex-B, display geometry, the
//! heartbeat and the dock message values.

use serde_json::Value;
use thiserror::Error;

use crate::protocol::framing::{PixelHeight, PixelWidth};
use crate::utils::types::PictureSize;

pub(super) const NAL_TYPE_MASK: u8 = 0x1F;

pub(super) const NAL_SLICE_NON_IDR: u8 = 1;

pub(super) const NAL_SLICE_IDR: u8 = 5;

pub(super) const NAL_SEI: u8 = 6;

pub(super) const NAL_SPS: u8 = 7;

pub(super) const NAL_PPS: u8 = 8;

pub(super) const NAL_AUD: u8 = 9;

/// Errors from splitting an Annex-B stream.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnnexBError {
    /// No `00 00 01` / `00 00 00 01` start code was found.
    #[error("no Annex-B start code found in the input")]
    NoStartCode,
    /// The stream contained no slice NAL (type 1 or 5), so there is nothing to display.
    #[error("stream contains no slice NAL units")]
    NoSlices,
}

/// Smallest accepted edge length in pixels (one H.264 macroblock).
pub const MIN_DIMENSION: u32 = 16;

/// Largest accepted edge length in pixels, within what Constrained Baseline encoders handle in
/// practice.
pub const MAX_DIMENSION: u32 = 4096;

/// Orientation of the encoded picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Landscape,
    Portrait,
}

/// Errors from validating the display settings.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DisplayError {
    /// The orientation string is neither `landscape` nor `portrait`.
    #[error("display.orientation must be \"landscape\" or \"portrait\", got {0:?}")]
    UnknownOrientation(String),
    /// A dimension is outside `MIN_DIMENSION..=MAX_DIMENSION`.
    #[error("display {axis} {value} is outside {MIN_DIMENSION}..={MAX_DIMENSION}")]
    OutOfRange { axis: &'static str, value: u32 },
    /// A dimension is odd; yuv420p chroma subsampling needs even sizes.
    #[error("display {axis} {value} must be even")]
    Odd { axis: &'static str, value: u32 },
    /// The width and height disagree with the stated orientation.
    #[error("{width}x{height} is not a {orientation:?} size")]
    OrientationMismatch {
        width: u32,
        height: u32,
        orientation: Orientation,
    },
}

/// Validated encoded-picture geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayGeometry {
    pub(super) width: PixelWidth,
    pub(super) height: PixelHeight,
    pub(super) orientation: Orientation,
}

/// Fixed HID report buffer size for this device's interrupt transport.
pub const REPORT_SIZE: usize = 4096;

/// Size of the header present at the start of every report (first and continuation).
pub const HEADER_SIZE: usize = 16;

/// Maximum payload bytes carried by a first report (`REPORT_SIZE - HEADER_SIZE`).
pub const FIRST_CHUNK_PAYLOAD_MAX: usize = REPORT_SIZE - HEADER_SIZE;

/// Maximum payload bytes carried by a continuation report. Continuation reports carry the same
/// 16-byte header as the first report (Report ID, then 15 zero bytes).
pub const CONTINUATION_CHUNK_PAYLOAD_MAX: usize = REPORT_SIZE - HEADER_SIZE;

/// Raw wire value of `ReportId::Control`.
pub(super) const REPORT_ID_CONTROL_RAW: u8 = 1;

/// Raw wire value of `ReportId::Video`.
pub(super) const REPORT_ID_VIDEO_RAW: u8 = 2;

/// The two closed, protocol-defined Report ID values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportId {
    /// MI_00 (control/heartbeat channel).
    Control,
    /// MI_01 (video channel).
    Video,
}

/// The 16-byte header fields present on the first report of every message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub report_id: ReportId,
    /// Total payload length of the whole message.
    pub payload_len: u32,
    pub size: PictureSize,
}

/// A message reconstructed from a chunk sequence: the header fields plus the reassembled
/// payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMessage {
    pub report_id: ReportId,
    pub size: PictureSize,
    pub payload: Vec<u8>,
}

/// Errors from building or parsing a chunk sequence.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FramingError {
    /// `from_bytes` was given fewer than `HEADER_SIZE` bytes.
    #[error("buffer too short to contain a {HEADER_SIZE}-byte header: got {len} bytes")]
    HeaderTooShort { len: usize },
    /// Header bytes 5..8 weren't zero.
    #[error("header bytes 5..8 weren't zero: {bytes:?}")]
    NonZeroPadding { bytes: [u8; 3] },
    /// A header's Report ID byte wasn't one of the two protocol-defined values.
    #[error("unknown Report ID byte {raw} (expected 1 for control or 2 for video)")]
    UnknownReportId { raw: u8 },
    /// `parse_chunks` was given an empty chunk list.
    #[error("no chunks provided")]
    NoChunks,
    /// A chunk (by index in the sequence) was shorter than the payload its header/offset
    /// implies it should carry.
    #[error("chunk {index} shorter than its expected payload")]
    TruncatedChunk { index: usize },
    /// A continuation chunk was shorter than the 16-byte continuation header.
    #[error("continuation chunk {index} is shorter than the {HEADER_SIZE}-byte header")]
    ContinuationHeaderTooShort { index: usize },
    /// A continuation chunk's Report ID byte didn't match the first chunk's.
    #[error("chunk {index} Report ID {got} doesn't match first chunk's {expected}")]
    ReportIdMismatch { index: usize, expected: u8, got: u8 },
    /// A continuation chunk's header bytes 1..16 weren't all zero.
    #[error("continuation chunk {index} header bytes 1..16 weren't zero")]
    NonZeroContinuationHeader { index: usize },
    /// The chunk sequence ended before the announced payload length was recovered.
    #[error("chunk sequence ended {missing} payload bytes short of the payload length")]
    IncompleteMessage { missing: usize },
}

/// Builds a complete wire-ready chunk sequence for one message, with distinct entry points for
/// the video and control channels, so control messages always carry zero width and height by
/// construction. A thin builder over the pure `frame_message` function.
pub struct MessageBuilder {
    pub(super) report_id: ReportId,
    pub(super) size: PictureSize,
    pub(super) payload: Vec<u8>,
}

/// The JSON-RPC method name of the heartbeat.
pub const HEARTBEAT_METHOD: &str = "heartbeat";

/// Longest accepted session id, in digits.
pub(super) const SESSION_ID_MAX_LEN: usize = 20;

/// Errors from constructing a [`SessionId`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionIdError {
    /// The id was empty or longer than the accepted maximum.
    #[error("session id must be 1 to {SESSION_ID_MAX_LEN} characters, got {0}")]
    BadLength(usize),
    /// The id contained a character that is not an ASCII digit.
    #[error("session id must contain only ASCII digits")]
    NotDigits,
}

/// The heartbeat `session-id`: a non-empty string of ASCII digits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(pub(super) String);

/// Largest dock message the reader reassembles; anything bigger is dropped as malformed.
pub(super) const MAX_INBOUND_LEN: usize = 64 * 1024;

/// Longest dock-supplied string (method names) written to the log.
pub const LOG_TEXT_MAX: usize = 64;

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

/// Reassembles the dock's MI_00 reports into message texts.
///
/// Reports use the same layout the host writes: a 16-byte header with the big-endian payload
/// length at bytes 1..4 on the first report, then payload from offset 16; continuation reports
/// repeat the header with a zero length. The reader is lenient about the other header bytes,
/// since the input is the dock's, but never trusts the length beyond [`MAX_INBOUND_LEN`].
#[derive(Debug, Default)]
pub struct InboundAssembler {
    pub(super) expected: usize,
    pub(super) buf: Vec<u8>,
}

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
