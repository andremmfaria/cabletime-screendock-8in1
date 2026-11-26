//! HID wire framing for the CABLETIME ScreenDock's MI_00 (control/heartbeat, Report ID 1) and
//! MI_01 (video, Report ID 2) channels.
//!
//! Every report is `REPORT_SIZE` (4096) bytes and starts with a 16-byte header, on the first
//! report of a message and on every continuation report alike.
//!
//! First report of a message (header + payload):
//! ```text
//! offset  size      field
//! 0       1 byte    Report ID (1 for MI_00, 2 for MI_01)
//! 1-4     4 bytes   total payload length of the whole message, big-endian u32
//! 5-7     3 bytes   zero
//! 8-11    4 bytes   picture width in pixels, big-endian u32 (video; zero for control messages)
//! 12-15   4 bytes   picture height in pixels, big-endian u32 (video; zero for control messages)
//! 16+     up to 4080 bytes   payload chunk (Annex B H.264 or JSON text)
//! ```
//!
//! Continuation reports of a multi-report message:
//! ```text
//! offset  size      field
//! 0       1 byte    Report ID (same as the first report)
//! 1-15    15 bytes  zero
//! 16+     up to 4080 bytes   payload chunk
//! ```
//!
//! Every report written to the device is sized to the HID report's real 4096-byte buffer; any
//! unused tail is zero-padded rather than truncated, since the hidraw interrupt transport
//! operates on fixed-size reports.
//!
//! This module is a from-scratch implementation of the byte layout above, written from prose
//! descriptions only.
//!
//! Design notes:
//! - `ReportId` is a closed enum, and `PixelWidth`/`PixelHeight` are distinct newtypes, so a
//!   width can never be passed where a height is expected.
//! - `MessageBuilder` has distinct `.video(size)`/`.control()` entry points, so control
//!   messages always carry zero width and height by construction.
//! - `frame_message`/`parse_chunks` are pure, stateless functions over a complete payload or
//!   chunk list.
//! - `FramingError` derives its `Display`/`Error` impls with `thiserror`.

use thiserror::Error;

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
const REPORT_ID_CONTROL_RAW: u8 = 1;

/// Raw wire value of `ReportId::Video`.
const REPORT_ID_VIDEO_RAW: u8 = 2;

/// The two closed, protocol-defined Report ID values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportId {
    /// MI_00 (control/heartbeat channel).
    Control,
    /// MI_01 (video channel).
    Video,
}

impl ReportId {
    /// Serializes to the raw wire byte.
    pub fn as_u8(self) -> u8 {
        match self {
            ReportId::Control => REPORT_ID_CONTROL_RAW,
            ReportId::Video => REPORT_ID_VIDEO_RAW,
        }
    }

    /// Parses a raw wire byte; a caller holding a `ReportId` knows it is one of the two valid
    /// values.
    pub fn try_from_u8(raw: u8) -> Result<Self, FramingError> {
        match raw {
            REPORT_ID_CONTROL_RAW => Ok(ReportId::Control),
            REPORT_ID_VIDEO_RAW => Ok(ReportId::Video),
            other => Err(FramingError::UnknownReportId { raw: other }),
        }
    }
}

/// Picture width in pixels, as carried in header bytes 8..11.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelWidth(pub u32);

/// Picture height in pixels, as carried in header bytes 12..15.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelHeight(pub u32);

/// Picture size carried in the header of a video message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureSize {
    pub width: PixelWidth,
    pub height: PixelHeight,
}

impl PictureSize {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: PixelWidth(width),
            height: PixelHeight(height),
        }
    }

    /// The size field values of a control message: both zero.
    pub fn zero() -> Self {
        Self::new(0, 0)
    }
}

/// The 16-byte header fields present on the first report of every message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub report_id: ReportId,
    /// Total payload length of the whole message.
    pub payload_len: u32,
    pub size: PictureSize,
}

impl FrameHeader {
    /// Serializes this header to its exact 16-byte wire layout.
    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0] = self.report_id.as_u8();
        buf[1..5].copy_from_slice(&self.payload_len.to_be_bytes());
        // buf[5..8] stays zero.
        buf[8..12].copy_from_slice(&self.size.width.0.to_be_bytes());
        buf[12..16].copy_from_slice(&self.size.height.0.to_be_bytes());
        buf
    }

    /// Parses a header from the first `HEADER_SIZE` bytes of a buffer. Validates that bytes
    /// 5..8 are zero, since a mismatch there is a strong signal the buffer isn't a first
    /// report of this protocol.
    pub fn from_bytes(buf: &[u8]) -> Result<Self, FramingError> {
        if buf.len() < HEADER_SIZE {
            return Err(FramingError::HeaderTooShort { len: buf.len() });
        }
        let report_id = ReportId::try_from_u8(buf[0])?;
        let payload_len = u32::from_be_bytes(buf[1..5].try_into().unwrap());
        if buf[5..8] != [0, 0, 0] {
            return Err(FramingError::NonZeroPadding {
                bytes: [buf[5], buf[6], buf[7]],
            });
        }
        let width = u32::from_be_bytes(buf[8..12].try_into().unwrap());
        let height = u32::from_be_bytes(buf[12..16].try_into().unwrap());
        Ok(FrameHeader {
            report_id,
            payload_len,
            size: PictureSize::new(width, height),
        })
    }
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

/// Splits `payload` into the wire-ready chunk sequence for one message (either a video keyframe
/// bundle/inter-frame on MI_01, or a JSON-RPC control message on MI_00).
///
/// Returns one `REPORT_SIZE`-byte buffer per chunk, ready to `write(2)` to the resolved hidraw
/// node back-to-back, in order. Every chunk starts with a 16-byte header: the first carries the
/// payload length and picture size, continuations carry the Report ID and zeros. Each chunk's
/// tail is zero-padded out to `REPORT_SIZE`.
pub fn frame_message(report_id: ReportId, size: PictureSize, payload: &[u8]) -> Vec<Vec<u8>> {
    let header = FrameHeader {
        report_id,
        payload_len: payload.len() as u32,
        size,
    };

    let first_len = payload.len().min(FIRST_CHUNK_PAYLOAD_MAX);
    let mut first_chunk = vec![0u8; REPORT_SIZE];
    first_chunk[0..HEADER_SIZE].copy_from_slice(&header.to_bytes());
    first_chunk[HEADER_SIZE..HEADER_SIZE + first_len].copy_from_slice(&payload[0..first_len]);

    let mut chunks = Vec::with_capacity(1 + payload.len() / CONTINUATION_CHUNK_PAYLOAD_MAX);
    chunks.push(first_chunk);

    let report_id_raw = report_id.as_u8();
    let mut offset = first_len;
    while offset < payload.len() {
        let take = (payload.len() - offset).min(CONTINUATION_CHUNK_PAYLOAD_MAX);
        let mut chunk = vec![0u8; REPORT_SIZE];
        chunk[0] = report_id_raw;
        chunk[HEADER_SIZE..HEADER_SIZE + take].copy_from_slice(&payload[offset..offset + take]);
        chunks.push(chunk);
        offset += take;
    }

    chunks
}

/// Reverses `frame_message`: validates and reassembles a chunk sequence back into its header
/// fields and original payload.
pub fn parse_chunks(chunks: &[Vec<u8>]) -> Result<ParsedMessage, FramingError> {
    let first = chunks.first().ok_or(FramingError::NoChunks)?;
    let header = FrameHeader::from_bytes(first)?;

    let total_len = header.payload_len as usize;
    let first_payload_len = total_len.min(FIRST_CHUNK_PAYLOAD_MAX);
    if first.len() < HEADER_SIZE + first_payload_len {
        return Err(FramingError::TruncatedChunk { index: 0 });
    }

    let mut payload = Vec::with_capacity(total_len);
    payload.extend_from_slice(&first[HEADER_SIZE..HEADER_SIZE + first_payload_len]);

    let report_id_raw = header.report_id.as_u8();
    let mut remaining = total_len - first_payload_len;
    for (i, chunk) in chunks.iter().enumerate().skip(1) {
        if chunk.len() < HEADER_SIZE {
            return Err(FramingError::ContinuationHeaderTooShort { index: i });
        }
        if chunk[0] != report_id_raw {
            return Err(FramingError::ReportIdMismatch {
                index: i,
                expected: report_id_raw,
                got: chunk[0],
            });
        }
        if chunk[1..HEADER_SIZE].iter().any(|&b| b != 0) {
            return Err(FramingError::NonZeroContinuationHeader { index: i });
        }
        let take = remaining.min(CONTINUATION_CHUNK_PAYLOAD_MAX);
        if chunk.len() < HEADER_SIZE + take {
            return Err(FramingError::TruncatedChunk { index: i });
        }
        payload.extend_from_slice(&chunk[HEADER_SIZE..HEADER_SIZE + take]);
        remaining -= take;
    }

    if remaining != 0 {
        return Err(FramingError::IncompleteMessage { missing: remaining });
    }

    Ok(ParsedMessage {
        report_id: header.report_id,
        size: header.size,
        payload,
    })
}

/// Builds a complete wire-ready chunk sequence for one message, with distinct entry points for
/// the video and control channels, so control messages always carry zero width and height by
/// construction. A thin builder over the pure `frame_message` function.
pub struct MessageBuilder {
    report_id: ReportId,
    size: PictureSize,
    payload: Vec<u8>,
}

impl MessageBuilder {
    /// Starts building a video-channel (MI_01) message for the given picture size.
    pub fn video(size: PictureSize) -> Self {
        MessageBuilder {
            report_id: ReportId::Video,
            size,
            payload: Vec::new(),
        }
    }

    /// Starts building a control-channel (MI_00) message: width and height are zero.
    pub fn control() -> Self {
        MessageBuilder {
            report_id: ReportId::Control,
            size: PictureSize::zero(),
            payload: Vec::new(),
        }
    }

    /// Sets the message payload (the H.264 keyframe bundle/inter-frame, or the JSON-RPC
    /// message bytes).
    pub fn payload(mut self, payload: &[u8]) -> Self {
        self.payload = payload.to_vec();
        self
    }

    /// Produces the final wire-ready chunk sequence, exactly as `frame_message` would.
    pub fn build(self) -> Vec<Vec<u8>> {
        frame_message(self.report_id, self.size, &self.payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips_exact_bytes() {
        let header = FrameHeader {
            report_id: ReportId::Video,
            payload_len: 3577,
            size: PictureSize::new(1280, 720),
        };
        let bytes = header.to_bytes();
        assert_eq!(bytes[0], 2);
        assert_eq!(&bytes[1..5], &[0x00, 0x00, 0x0D, 0xF9]);
        assert_eq!(&bytes[5..8], &[0x00, 0x00, 0x00]);
        assert_eq!(&bytes[8..12], &[0x00, 0x00, 0x05, 0x00]);
        assert_eq!(&bytes[12..16], &[0x00, 0x00, 0x02, 0xD0]);
        assert_eq!(FrameHeader::from_bytes(&bytes).unwrap(), header);
    }

    #[test]
    fn rejects_nonzero_bytes_5_to_7() {
        let mut bytes = FrameHeader {
            report_id: ReportId::Video,
            payload_len: 10,
            size: PictureSize::zero(),
        }
        .to_bytes();
        bytes[6] = 0xFF;
        let err = FrameHeader::from_bytes(&bytes).unwrap_err();
        assert_eq!(
            err,
            FramingError::NonZeroPadding {
                bytes: [0x00, 0xFF, 0x00]
            }
        );
    }

    #[test]
    fn continuation_reports_carry_a_full_zero_header() {
        let payload: Vec<u8> = (0..9000).map(|i| (i % 251) as u8 | 1).collect();
        let chunks = MessageBuilder::video(PictureSize::new(1280, 720))
            .payload(&payload)
            .build();
        assert_eq!(chunks.len(), 3);
        assert_eq!(
            &chunks[1][..HEADER_SIZE],
            &[2u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(&chunks[1][HEADER_SIZE..], &payload[4080..8160]);
        assert_eq!(&chunks[2][HEADER_SIZE..HEADER_SIZE + 840], &payload[8160..]);
        assert!(chunks[2][HEADER_SIZE + 840..].iter().all(|&b| b == 0));
        assert_eq!(parse_chunks(&chunks).unwrap().payload, payload);
    }

    #[test]
    fn parse_rejects_old_style_continuation() {
        let payload = vec![7u8; 5000];
        let mut chunks = MessageBuilder::video(PictureSize::new(16, 16))
            .payload(&payload)
            .build();
        chunks[1][5] = 1;
        assert_eq!(
            parse_chunks(&chunks).unwrap_err(),
            FramingError::NonZeroContinuationHeader { index: 1 }
        );
    }

    #[test]
    fn message_builder_video_carries_width_and_height() {
        let chunks = MessageBuilder::video(PictureSize::new(1280, 720))
            .payload(&[0xAA, 0xBB, 0xCC])
            .build();
        assert_eq!(chunks.len(), 1);
        let header = FrameHeader::from_bytes(&chunks[0]).unwrap();
        assert_eq!(header.report_id, ReportId::Video);
        assert_eq!(header.size, PictureSize::new(1280, 720));
        assert_eq!(
            &chunks[0][HEADER_SIZE..HEADER_SIZE + 3],
            &[0xAA, 0xBB, 0xCC]
        );
    }

    #[test]
    fn message_builder_control_has_zero_size_and_matches_frame_message() {
        let payload = b"{\"jsonrpc\":\"2.0\"}";
        let via_builder = MessageBuilder::control().payload(payload).build();
        let via_frame_message = frame_message(ReportId::Control, PictureSize::zero(), payload);
        assert_eq!(via_builder, via_frame_message);
        assert_eq!(&via_builder[0][5..16], &[0u8; 11]);
    }
}
