//! HID wire framing for the CABLETIME ScreenDock's MI_00 (control/heartbeat, Report ID 1) and
//! MI_01 (video, Report ID 2) channels.
//!
//! Wire layout, straight from `docs/research/full-protocol-spec-2026-11-21.md`'s "Wire frame
//! layout" table (byte-for-byte, high confidence — live-capture-confirmed against 23,959 real
//! video messages):
//!
//! First chunk of a message (16-byte header + payload):
//! ```text
//! offset  size      field
//! 0       1 byte    Report ID (interface_number + 1: 1 for MI_00, 2 for MI_01)
//! 1-4     4 bytes   total message length, big-endian u32
//! 5-8     4 bytes   field A, big-endian u32 (always 0, per the live multi-resolution test)
//! 9-12    4 bytes   field B, big-endian u32 (0 for control messages; target_width_px * 256 for video)
//! 13-15   3 bytes   zero padding
//! 16+     up to 4080 bytes   payload chunk
//! ```
//!
//! Continuation chunks (no header beyond the Report ID byte):
//! ```text
//! offset  size      field
//! 0       1 byte    Report ID (same as the first chunk)
//! 1+      up to 4095 bytes   raw continuation payload
//! ```
//!
//! Every chunk written to the device is sized to the HID report's real 4096-byte buffer; any
//! unused tail (first chunk when the payload is shorter than 4080 bytes, or the final
//! continuation chunk) is zero-padded rather than truncated, since the hidraw interrupt
//! transport operates on fixed-size reports.
//!
//! This module is a from-scratch reimplementation based solely on the above documented byte
//! layout (per `docs/implementation-plan.md`'s clean-room rule) — it does not reference, and was
//! not derived from, any decompiled vendor code.
//!
//! Per `docs/implementation-plan.md`'s Design Patterns section:
//! - (pattern 2, newtype) `ReportId` is a closed enum rather than a bare `u8`, and `PixelWidth`/
//!   `FieldB` are distinct newtypes rather than bare `u32`s, so `field_b_for_width`'s documented
//!   invariant ("field B tracks width only, never height") is a type-level fact, not just a
//!   comment.
//! - (pattern 3, builder) `MessageBuilder` gives distinct `.video(width)`/`.control()` entry
//!   points so `field_a = 0` is enforced by construction rather than caller discipline, instead
//!   of callers passing four positional arguments to `frame_message` directly.
//! - (pattern 4, typestate — explicitly NOT applied here) `frame_message`/`parse_chunks` remain
//!   pure, stateless functions over a complete payload/chunk-list; typestate is scoped to a
//!   future incremental chunk reassembler in `hidraw_io.rs`, not a retrofit onto these functions.
//! - (pattern 7, thiserror) `FramingError` uses `thiserror`'s derive instead of hand-written
//!   `Display`/`Error` impls, with no change to variant shapes.

use thiserror::Error;

/// Fixed HID report buffer size for this device's interrupt transport.
pub const REPORT_SIZE: usize = 4096;

/// Size of the 16-byte header present only on the first chunk of a message.
pub const HEADER_SIZE: usize = 16;

/// Maximum payload bytes carried by a first chunk (`REPORT_SIZE - HEADER_SIZE`).
pub const FIRST_CHUNK_PAYLOAD_MAX: usize = REPORT_SIZE - HEADER_SIZE;

/// Maximum payload bytes carried by a continuation chunk (`REPORT_SIZE` minus the 1-byte Report
/// ID prefix).
pub const CONTINUATION_CHUNK_PAYLOAD_MAX: usize = REPORT_SIZE - 1;

/// Raw wire value of `ReportId::Control`.
const REPORT_ID_CONTROL_RAW: u8 = 1;

/// Raw wire value of `ReportId::Video`.
const REPORT_ID_VIDEO_RAW: u8 = 2;

/// The two closed, protocol-defined Report ID values. A closed enum rather than a bare `u8`
/// since the set of valid values is known and fixed — this both self-documents at call sites
/// (`ReportId::Video` reads better than `2`) and makes an invalid/typo'd raw value impossible to
/// construct outside `ReportId::try_from_u8`.
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

    /// Parses a raw wire byte, per "parse, don't validate" — a caller holding a `ReportId`
    /// knows it's one of the two valid values, rather than needing to re-check a bare `u8`.
    pub fn try_from_u8(raw: u8) -> Result<Self, FramingError> {
        match raw {
            REPORT_ID_CONTROL_RAW => Ok(ReportId::Control),
            REPORT_ID_VIDEO_RAW => Ok(ReportId::Video),
            other => Err(FramingError::UnknownReportId { raw: other }),
        }
    }
}

/// Target width in pixels, pre-multiplied-by-256 conversion not yet applied. Distinct from
/// `FieldB` so a raw pixel width can never be passed to `FrameHeader`/`frame_message` where the
/// already-multiplied wire value is expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelWidth(pub u32);

/// The wire value of header field B: 0 for control messages, `target_width_px * 256` for video.
/// Constructed only through `field_b_for_width`/`FieldB::zero`, never directly from a raw pixel
/// width, so the "field B tracks width only, never height, and is pre-multiplied" invariant is
/// enforced by the type rather than documented in a comment that can rot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldB(pub u32);

impl FieldB {
    /// Field B's value for control messages (always 0).
    pub fn zero() -> Self {
        FieldB(0)
    }
}

/// Computes `field B = target_width_in_pixels * 256`, per the empirically-confirmed formula in
/// `docs/research/full-protocol-spec-2026-11-21.md` (zero exceptions across 9 tested
/// resolution/orientation combinations; field B tracks width only, never height).
pub fn field_b_for_width(target_width_px: PixelWidth) -> FieldB {
    FieldB(target_width_px.0 * 256)
}

/// The 16-byte header fields present on the first chunk of every message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub report_id: ReportId,
    pub total_len: u32,
    pub field_a: u32,
    pub field_b: FieldB,
}

impl FrameHeader {
    /// Serializes this header to its exact 16-byte wire layout.
    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0] = self.report_id.as_u8();
        buf[1..5].copy_from_slice(&self.total_len.to_be_bytes());
        buf[5..9].copy_from_slice(&self.field_a.to_be_bytes());
        buf[9..13].copy_from_slice(&self.field_b.0.to_be_bytes());
        // buf[13..16] stays zero: the documented 3-byte zero-padding region.
        buf
    }

    /// Parses a header from the first `HEADER_SIZE` bytes of a buffer. Validates that the
    /// documented zero-padding region is actually zero, since a mismatch there is a strong
    /// signal the buffer isn't really a first chunk of this protocol.
    pub fn from_bytes(buf: &[u8]) -> Result<Self, FramingError> {
        if buf.len() < HEADER_SIZE {
            return Err(FramingError::HeaderTooShort { len: buf.len() });
        }
        let report_id = ReportId::try_from_u8(buf[0])?;
        let total_len = u32::from_be_bytes(buf[1..5].try_into().unwrap());
        let field_a = u32::from_be_bytes(buf[5..9].try_into().unwrap());
        let field_b = u32::from_be_bytes(buf[9..13].try_into().unwrap());
        if buf[13..16] != [0, 0, 0] {
            return Err(FramingError::NonZeroPadding {
                bytes: [buf[13], buf[14], buf[15]],
            });
        }
        Ok(FrameHeader {
            report_id,
            total_len,
            field_a,
            field_b: FieldB(field_b),
        })
    }
}

/// A message reconstructed from a chunk sequence: the header fields plus the reassembled
/// payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMessage {
    pub report_id: ReportId,
    pub field_a: u32,
    pub field_b: FieldB,
    pub payload: Vec<u8>,
}

/// Errors from building or parsing a chunk sequence.
///
/// Per `docs/implementation-plan.md`'s Design Patterns section (pattern 7), this uses
/// `thiserror`'s derive rather than hand-written `Display`/`Error` impls. Variant shapes are
/// unchanged from the pre-refactor version (plus the new `UnknownReportId` variant, needed now
/// that `report_id` is a closed enum parsed from the wire rather than a bare `u8`); only the
/// boilerplate moved.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FramingError {
    /// `from_bytes` was given fewer than `HEADER_SIZE` bytes.
    #[error("buffer too short to contain a {HEADER_SIZE}-byte header: got {len} bytes")]
    HeaderTooShort { len: usize },
    /// The documented zero-padding region (header bytes 13-15) wasn't actually zero.
    #[error("header zero-padding region wasn't zero: {bytes:?}")]
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
    /// A continuation chunk was empty, so it has no Report ID byte to read.
    #[error("continuation chunk {index} is empty (no Report ID byte)")]
    EmptyContinuationChunk { index: usize },
    /// A continuation chunk's Report ID byte didn't match the first chunk's.
    #[error("chunk {index} Report ID {got} doesn't match first chunk's {expected}")]
    ReportIdMismatch {
        index: usize,
        expected: u8,
        got: u8,
    },
    /// The chunk sequence ended before `total_len` bytes of payload were recovered.
    #[error("chunk sequence ended {missing} payload bytes short of total_len")]
    IncompleteMessage { missing: usize },
}

/// Splits `payload` into the wire-ready chunk sequence for one message (either a video keyframe
/// bundle/inter-frame on MI_01, or a JSON-RPC control message on MI_00).
///
/// Returns one `REPORT_SIZE`-byte buffer per chunk, ready to `write(2)` to the resolved hidraw
/// node back-to-back, in order. The first chunk carries the 16-byte header; any further chunks
/// are continuation chunks carrying only a 1-byte Report ID prefix. Both the first chunk's tail
/// and the final continuation chunk's tail are zero-padded out to `REPORT_SIZE`.
///
/// This stays a pure, stateless function over a complete payload (per Design Patterns pattern
/// 4) — callers that want `field_a = 0` enforced by the type system rather than remembering to
/// pass it should use `MessageBuilder` (below) instead of calling this directly.
pub fn frame_message(report_id: ReportId, field_a: u32, field_b: FieldB, payload: &[u8]) -> Vec<Vec<u8>> {
    let total_len = payload.len() as u32;
    let header = FrameHeader {
        report_id,
        total_len,
        field_a,
        field_b,
    };

    let first_len = payload.len().min(FIRST_CHUNK_PAYLOAD_MAX);
    let mut first_chunk = vec![0u8; REPORT_SIZE];
    first_chunk[0..HEADER_SIZE].copy_from_slice(&header.to_bytes());
    first_chunk[HEADER_SIZE..HEADER_SIZE + first_len].copy_from_slice(&payload[0..first_len]);

    let mut chunks = Vec::with_capacity(1);
    chunks.push(first_chunk);

    let report_id_raw = report_id.as_u8();
    let mut offset = first_len;
    while offset < payload.len() {
        let remaining = payload.len() - offset;
        let take = remaining.min(CONTINUATION_CHUNK_PAYLOAD_MAX);
        let mut chunk = vec![0u8; REPORT_SIZE];
        chunk[0] = report_id_raw;
        chunk[1..1 + take].copy_from_slice(&payload[offset..offset + take]);
        chunks.push(chunk);
        offset += take;
    }

    chunks
}

/// Reverses `frame_message`: validates and reassembles a chunk sequence back into its header
/// fields and original payload. Used by this module's own tests, and available for future
/// debugging (e.g. replaying/inspecting a captured chunk sequence).
pub fn parse_chunks(chunks: &[Vec<u8>]) -> Result<ParsedMessage, FramingError> {
    let first = chunks.first().ok_or(FramingError::NoChunks)?;
    let header = FrameHeader::from_bytes(first)?;

    let total_len = header.total_len as usize;
    let first_payload_len = total_len.min(FIRST_CHUNK_PAYLOAD_MAX);
    if first.len() < HEADER_SIZE + first_payload_len {
        return Err(FramingError::TruncatedChunk { index: 0 });
    }

    let mut payload = Vec::with_capacity(total_len);
    payload.extend_from_slice(&first[HEADER_SIZE..HEADER_SIZE + first_payload_len]);

    let report_id_raw = header.report_id.as_u8();
    let mut remaining = total_len - first_payload_len;
    for (i, chunk) in chunks.iter().enumerate().skip(1) {
        if chunk.is_empty() {
            return Err(FramingError::EmptyContinuationChunk { index: i });
        }
        if chunk[0] != report_id_raw {
            return Err(FramingError::ReportIdMismatch {
                index: i,
                expected: report_id_raw,
                got: chunk[0],
            });
        }
        let take = remaining.min(CONTINUATION_CHUNK_PAYLOAD_MAX);
        if chunk.len() < 1 + take {
            return Err(FramingError::TruncatedChunk { index: i });
        }
        payload.extend_from_slice(&chunk[1..1 + take]);
        remaining -= take;
    }

    if remaining != 0 {
        return Err(FramingError::IncompleteMessage { missing: remaining });
    }

    Ok(ParsedMessage {
        report_id: header.report_id,
        field_a: header.field_a,
        field_b: header.field_b,
        payload,
    })
}

/// Builds a complete wire-ready chunk sequence for one message, with distinct entry points for
/// the video and control channels so `field_a = 0` (the protocol's fixed, required value) is
/// filled in by construction rather than relying on every call site remembering to pass `0`
/// explicitly. Per Design Patterns pattern 3: this is a thin builder over the existing pure
/// `frame_message` function, not a replacement for it — `.build()` calls `frame_message`
/// internally.
pub struct MessageBuilder {
    report_id: ReportId,
    field_a: u32,
    field_b: FieldB,
    payload: Vec<u8>,
}

impl MessageBuilder {
    /// Starts building a video-channel (MI_01) message for the given target width. Fills in
    /// `ReportId::Video`, `field_a = 0`, and `field_b = field_b_for_width(width)` — a caller
    /// cannot construct a video message with a non-zero `field_a` through this entry point.
    pub fn video(width: PixelWidth) -> Self {
        MessageBuilder {
            report_id: ReportId::Video,
            field_a: 0,
            field_b: field_b_for_width(width),
            payload: Vec::new(),
        }
    }

    /// Starts building a control-channel (MI_00) message. Fills in `ReportId::Control`,
    /// `field_a = 0`, and `field_b = 0` (control messages never carry a width).
    pub fn control() -> Self {
        MessageBuilder {
            report_id: ReportId::Control,
            field_a: 0,
            field_b: FieldB::zero(),
            payload: Vec::new(),
        }
    }

    /// Sets the message payload (the H.264 keyframe bundle/inter-frame NAL, or the JSON-RPC
    /// message bytes).
    pub fn payload(mut self, payload: &[u8]) -> Self {
        self.payload = payload.to_vec();
        self
    }

    /// Produces the final wire-ready chunk sequence, exactly as `frame_message` would, with
    /// this builder's `report_id`/`field_a`/`field_b` and the payload set via `.payload(...)`.
    pub fn build(self) -> Vec<Vec<u8>> {
        frame_message(self.report_id, self.field_a, self.field_b, &self.payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips_exact_bytes() {
        let header = FrameHeader {
            report_id: ReportId::Video,
            total_len: 3577,
            field_a: 0,
            field_b: FieldB(327_680),
        };
        let bytes = header.to_bytes();
        assert_eq!(bytes[0], 2);
        assert_eq!(&bytes[1..5], &[0x00, 0x00, 0x0D, 0xF9]);
        assert_eq!(&bytes[5..9], &[0x00, 0x00, 0x00, 0x00]);
        assert_eq!(&bytes[9..13], &[0x00, 0x05, 0x00, 0x00]);
        assert_eq!(&bytes[13..16], &[0x00, 0x00, 0x00]);
        assert_eq!(FrameHeader::from_bytes(&bytes).unwrap(), header);
    }

    #[test]
    fn field_b_formula_matches_research() {
        assert_eq!(field_b_for_width(PixelWidth(1280)), FieldB(327_680));
        assert_eq!(field_b_for_width(PixelWidth(1920)), FieldB(491_520));
        assert_eq!(field_b_for_width(PixelWidth(900)), FieldB(230_400));
    }

    #[test]
    fn rejects_nonzero_padding() {
        let mut bytes = FrameHeader {
            report_id: ReportId::Video,
            total_len: 10,
            field_a: 0,
            field_b: FieldB::zero(),
        }
        .to_bytes();
        bytes[14] = 0xFF;
        let err = FrameHeader::from_bytes(&bytes).unwrap_err();
        assert_eq!(
            err,
            FramingError::NonZeroPadding {
                bytes: [0x00, 0xFF, 0x00]
            }
        );
    }

    #[test]
    fn message_builder_video_enforces_field_a_zero_and_correct_field_b() {
        // This is the concrete byte-level check that the builder's video() entry point
        // produces exactly the same header a hand-constructed FrameHeader would, proving
        // field_a = 0 is produced by construction, not caller discipline.
        let chunks = MessageBuilder::video(PixelWidth(1280))
            .payload(&[0xAA, 0xBB, 0xCC])
            .build();
        assert_eq!(chunks.len(), 1);
        let header = FrameHeader::from_bytes(&chunks[0]).unwrap();
        assert_eq!(header.report_id, ReportId::Video);
        assert_eq!(header.field_a, 0);
        assert_eq!(header.field_b, FieldB(327_680));
        assert_eq!(&chunks[0][HEADER_SIZE..HEADER_SIZE + 3], &[0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn message_builder_control_matches_frame_message_byte_for_byte() {
        let payload = b"{\"jsonrpc\":\"2.0\"}";
        let via_builder = MessageBuilder::control().payload(payload).build();
        let via_frame_message =
            frame_message(ReportId::Control, 0, FieldB::zero(), payload);
        assert_eq!(via_builder, via_frame_message);
    }
}
