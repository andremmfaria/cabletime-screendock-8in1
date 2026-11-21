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

use std::error::Error;
use std::fmt;

/// Fixed HID report buffer size for this device's interrupt transport.
pub const REPORT_SIZE: usize = 4096;

/// Size of the 16-byte header present only on the first chunk of a message.
pub const HEADER_SIZE: usize = 16;

/// Maximum payload bytes carried by a first chunk (`REPORT_SIZE - HEADER_SIZE`).
pub const FIRST_CHUNK_PAYLOAD_MAX: usize = REPORT_SIZE - HEADER_SIZE;

/// Maximum payload bytes carried by a continuation chunk (`REPORT_SIZE` minus the 1-byte Report
/// ID prefix).
pub const CONTINUATION_CHUNK_PAYLOAD_MAX: usize = REPORT_SIZE - 1;

/// Report ID used by MI_00 (control/heartbeat channel).
pub const REPORT_ID_CONTROL: u8 = 1;

/// Report ID used by MI_01 (video channel).
pub const REPORT_ID_VIDEO: u8 = 2;

/// The 16-byte header fields present on the first chunk of every message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub report_id: u8,
    pub total_len: u32,
    pub field_a: u32,
    pub field_b: u32,
}

impl FrameHeader {
    /// Serializes this header to its exact 16-byte wire layout.
    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0] = self.report_id;
        buf[1..5].copy_from_slice(&self.total_len.to_be_bytes());
        buf[5..9].copy_from_slice(&self.field_a.to_be_bytes());
        buf[9..13].copy_from_slice(&self.field_b.to_be_bytes());
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
        let report_id = buf[0];
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
            field_b,
        })
    }
}

/// A message reconstructed from a chunk sequence: the header fields plus the reassembled
/// payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMessage {
    pub report_id: u8,
    pub field_a: u32,
    pub field_b: u32,
    pub payload: Vec<u8>,
}

/// Errors from building or parsing a chunk sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FramingError {
    /// `from_bytes` was given fewer than `HEADER_SIZE` bytes.
    HeaderTooShort { len: usize },
    /// The documented zero-padding region (header bytes 13-15) wasn't actually zero.
    NonZeroPadding { bytes: [u8; 3] },
    /// `parse_chunks` was given an empty chunk list.
    NoChunks,
    /// A chunk (by index in the sequence) was shorter than the payload its header/offset
    /// implies it should carry.
    TruncatedChunk { index: usize },
    /// A continuation chunk was empty, so it has no Report ID byte to read.
    EmptyContinuationChunk { index: usize },
    /// A continuation chunk's Report ID byte didn't match the first chunk's.
    ReportIdMismatch {
        index: usize,
        expected: u8,
        got: u8,
    },
    /// The chunk sequence ended before `total_len` bytes of payload were recovered.
    IncompleteMessage { missing: usize },
}

impl fmt::Display for FramingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FramingError::HeaderTooShort { len } => write!(
                f,
                "buffer too short to contain a {HEADER_SIZE}-byte header: got {len} bytes"
            ),
            FramingError::NonZeroPadding { bytes } => {
                write!(f, "header zero-padding region wasn't zero: {bytes:?}")
            }
            FramingError::NoChunks => write!(f, "no chunks provided"),
            FramingError::TruncatedChunk { index } => {
                write!(f, "chunk {index} shorter than its expected payload")
            }
            FramingError::EmptyContinuationChunk { index } => {
                write!(f, "continuation chunk {index} is empty (no Report ID byte)")
            }
            FramingError::ReportIdMismatch {
                index,
                expected,
                got,
            } => write!(
                f,
                "chunk {index} Report ID {got} doesn't match first chunk's {expected}"
            ),
            FramingError::IncompleteMessage { missing } => write!(
                f,
                "chunk sequence ended {missing} payload bytes short of total_len"
            ),
        }
    }
}

impl Error for FramingError {}

/// Splits `payload` into the wire-ready chunk sequence for one message (either a video keyframe
/// bundle/inter-frame on MI_01, or a JSON-RPC control message on MI_00).
///
/// Returns one `REPORT_SIZE`-byte buffer per chunk, ready to `write(2)` to the resolved hidraw
/// node back-to-back, in order. The first chunk carries the 16-byte header; any further chunks
/// are continuation chunks carrying only a 1-byte Report ID prefix. Both the first chunk's tail
/// and the final continuation chunk's tail are zero-padded out to `REPORT_SIZE`.
pub fn frame_message(report_id: u8, field_a: u32, field_b: u32, payload: &[u8]) -> Vec<Vec<u8>> {
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

    let mut offset = first_len;
    while offset < payload.len() {
        let remaining = payload.len() - offset;
        let take = remaining.min(CONTINUATION_CHUNK_PAYLOAD_MAX);
        let mut chunk = vec![0u8; REPORT_SIZE];
        chunk[0] = report_id;
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

    let mut remaining = total_len - first_payload_len;
    for (i, chunk) in chunks.iter().enumerate().skip(1) {
        if chunk.is_empty() {
            return Err(FramingError::EmptyContinuationChunk { index: i });
        }
        if chunk[0] != header.report_id {
            return Err(FramingError::ReportIdMismatch {
                index: i,
                expected: header.report_id,
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

/// Computes `field B = target_width_in_pixels * 256`, per the empirically-confirmed formula in
/// `docs/research/full-protocol-spec-2026-11-21.md` (zero exceptions across 9 tested
/// resolution/orientation combinations; field B tracks width only, never height).
pub fn field_b_for_width(target_width_px: u32) -> u32 {
    target_width_px * 256
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips_exact_bytes() {
        let header = FrameHeader {
            report_id: REPORT_ID_VIDEO,
            total_len: 3577,
            field_a: 0,
            field_b: 327_680,
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
        assert_eq!(field_b_for_width(1280), 327_680);
        assert_eq!(field_b_for_width(1920), 491_520);
        assert_eq!(field_b_for_width(900), 230_400);
    }

    #[test]
    fn rejects_nonzero_padding() {
        let mut bytes = FrameHeader {
            report_id: REPORT_ID_VIDEO,
            total_len: 10,
            field_a: 0,
            field_b: 0,
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
}
