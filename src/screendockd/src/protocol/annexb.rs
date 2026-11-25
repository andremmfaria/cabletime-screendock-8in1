//! Annex-B H.264 stream splitting into dock-sized messages.
//!
//! The dock receives one message per access unit: a keyframe is a single message bundling
//! SPS + PPS + SEI + IDR slice, and every inter-frame is a single-NAL message (see
//! `docs/research/full-protocol-spec-2026-11-21.md`). This module groups an Annex-B byte stream
//! (e.g. `ffmpeg ... -f h264 out.h264`) that way, so hardware validation can replay a real,
//! decodable stream instead of synthetic filler. Start codes are kept on every NAL.
//!
//! Pure functions over bytes: no I/O, no framing.

pub use super::types::AnnexBError;
use super::types::{
    NAL_AUD, NAL_PPS, NAL_SEI, NAL_SLICE_IDR, NAL_SLICE_NON_IDR, NAL_SPS, NAL_TYPE_MASK,
};

/// Returns the byte offsets of every start code (offset of its first `00`), in order.
fn start_code_offsets(data: &[u8]) -> Vec<usize> {
    let mut offsets = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            // A preceding zero makes it the 4-byte form; it belongs to this start code.
            let start = if i > 0 && data[i - 1] == 0 { i - 1 } else { i };
            offsets.push(start);
            i += 3;
        } else {
            i += 1;
        }
    }
    offsets
}

/// Returns true when `message` contains an IDR slice NAL unit (a keyframe).
pub fn contains_idr(message: &[u8]) -> bool {
    split_nals(message)
        .map(|nals| nals.iter().any(|n| nal_type(n) == Some(NAL_SLICE_IDR)))
        .unwrap_or(false)
}

/// Splits an Annex-B stream into NAL units, each slice starting at its start code.
pub fn split_nals(data: &[u8]) -> Result<Vec<&[u8]>, AnnexBError> {
    let offsets = start_code_offsets(data);
    if offsets.is_empty() {
        return Err(AnnexBError::NoStartCode);
    }
    let mut nals = Vec::with_capacity(offsets.len());
    for (n, &start) in offsets.iter().enumerate() {
        let end = offsets.get(n + 1).copied().unwrap_or(data.len());
        nals.push(&data[start..end]);
    }
    Ok(nals)
}

/// The NAL unit type of a NAL slice that begins with a start code.
fn nal_type(nal: &[u8]) -> Option<u8> {
    let header = if nal.starts_with(&[0, 0, 0, 1]) {
        nal.get(4)
    } else {
        nal.get(3)
    };
    header.map(|b| b & NAL_TYPE_MASK)
}

/// Groups an Annex-B stream into per-access-unit messages: SPS/PPS/SEI accumulate and are
/// bundled with the IDR slice that follows; a non-IDR slice is its own message; access unit
/// delimiters are dropped; any other NAL type is ignored.
pub fn messages_from_annexb(data: &[u8]) -> Result<Vec<Vec<u8>>, AnnexBError> {
    let mut messages = Vec::new();
    let mut pending: Vec<u8> = Vec::new();
    for nal in split_nals(data)? {
        match nal_type(nal) {
            Some(NAL_SPS) | Some(NAL_PPS) | Some(NAL_SEI) => pending.extend_from_slice(nal),
            Some(NAL_SLICE_IDR) => {
                pending.extend_from_slice(nal);
                messages.push(std::mem::take(&mut pending));
            }
            Some(NAL_SLICE_NON_IDR) => {
                if !pending.is_empty() {
                    messages.push(std::mem::take(&mut pending));
                }
                messages.push(nal.to_vec());
            }
            Some(NAL_AUD) | Some(_) | None => {}
        }
    }
    if messages.is_empty() {
        return Err(AnnexBError::NoSlices);
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nal(start: &[u8], ty: u8, body: usize) -> Vec<u8> {
        let mut v = start.to_vec();
        v.push(0x60 | ty);
        v.extend(std::iter::repeat_n(0xAB, body));
        v
    }

    #[test]
    fn bundles_parameter_sets_with_idr_and_keeps_interframes_single() {
        let mut stream = Vec::new();
        stream.extend(nal(&[0, 0, 0, 1], NAL_AUD, 1));
        stream.extend(nal(&[0, 0, 0, 1], NAL_SPS, 10));
        stream.extend(nal(&[0, 0, 1], NAL_PPS, 4));
        stream.extend(nal(&[0, 0, 0, 1], NAL_SEI, 6));
        stream.extend(nal(&[0, 0, 0, 1], NAL_SLICE_IDR, 100));
        stream.extend(nal(&[0, 0, 0, 1], NAL_SLICE_NON_IDR, 50));
        stream.extend(nal(&[0, 0, 1], NAL_SLICE_NON_IDR, 20));

        let messages = messages_from_annexb(&stream).unwrap();
        assert_eq!(messages.len(), 3);
        // SPS(4+1+10) + PPS(3+1+4) + SEI(4+1+6) + IDR(4+1+100)
        assert_eq!(messages[0].len(), 15 + 8 + 11 + 105);
        assert_eq!(nal_type(&messages[0]), Some(NAL_SPS));
        assert_eq!(messages[1].len(), 4 + 1 + 50);
        assert_eq!(messages[2].len(), 3 + 1 + 20);
        assert_eq!(nal_type(&messages[2]), Some(NAL_SLICE_NON_IDR));
    }

    #[test]
    fn rejects_input_without_start_codes() {
        assert_eq!(
            messages_from_annexb(&[1, 2, 3, 4]).unwrap_err(),
            AnnexBError::NoStartCode
        );
    }

    #[test]
    fn rejects_stream_without_slices() {
        let stream = nal(&[0, 0, 0, 1], NAL_SPS, 5);
        assert_eq!(
            messages_from_annexb(&stream).unwrap_err(),
            AnnexBError::NoSlices
        );
    }
}
