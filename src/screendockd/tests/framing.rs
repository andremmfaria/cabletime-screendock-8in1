//! Integration tests for `framing.rs`'s chunking/header math, using real captured message
//! shapes documented in `docs/research/full-protocol-spec-2026-11-21.md`.
//!
//! The H.264 NAL *content* inside each fixture is synthetic placeholder bytes of the correct
//! length and with the correct NAL-type header byte — the research docs' own pcap files aren't
//! available in this sandbox, and `framing.rs`'s job is the header/chunking math, not H.264
//! content validation. Exact byte offsets/values asserted below come directly from the
//! documented wire layout, not guessed.

use screendockd::framing::{
    field_b_for_width, frame_message, parse_chunks, CONTINUATION_CHUNK_PAYLOAD_MAX,
    FIRST_CHUNK_PAYLOAD_MAX, REPORT_ID_CONTROL, REPORT_ID_VIDEO, REPORT_SIZE,
};

/// Builds one Annex-B NAL unit: a 4-byte start code, a 1-byte NAL header
/// (`(nal_ref_idc << 5) | nal_type`), and `body_len` filler bytes.
fn nal(nal_type: u8, nal_ref_idc: u8, body_len: usize) -> Vec<u8> {
    let mut v = vec![0x00, 0x00, 0x00, 0x01];
    v.push((nal_ref_idc << 5) | nal_type);
    v.extend(std::iter::repeat(0xAAu8).take(body_len));
    v
}

/// A real keyframe-bundle shape per the research doc: SPS (type 7) + PPS (type 8) + SEI (type 6)
/// + IDR slice (type 5) concatenated as one message, sized to exactly 3577 bytes total so this
/// matches the research's documented fixture length precisely.
fn keyframe_bundle_3577_bytes() -> Vec<u8> {
    let sps = nal(7, 3, 25); // 4 + 1 + 25 = 30 bytes
    let pps = nal(8, 3, 7); // 4 + 1 + 7  = 12 bytes
    let sei = nal(6, 0, 15); // 4 + 1 + 15 = 20 bytes
    let idr = nal(5, 3, 3510); // 4 + 1 + 3510 = 3515 bytes
    let mut payload = Vec::new();
    payload.extend(sps);
    payload.extend(pps);
    payload.extend(sei);
    payload.extend(idr);
    assert_eq!(payload.len(), 3577, "fixture construction sanity check");
    payload
}

/// A single-NAL inter-frame shape, sized to exactly 67666 bytes total, matching the research
/// doc's documented 17-chunk fixture length precisely.
fn inter_frame_67666_bytes() -> Vec<u8> {
    let payload = nal(1, 2, 67661); // 4 + 1 + 67661 = 67666 bytes
    assert_eq!(payload.len(), 67666, "fixture construction sanity check");
    payload
}

/// A plausible MI_00 heartbeat JSON-RPC payload (exact JSON content doesn't need to match a
/// captured one byte-for-byte per the task brief — just well-formed JSON of the documented
/// shape from `docs/research/full-protocol-spec-2026-11-21.md`'s MI_00 section).
fn heartbeat_payload() -> Vec<u8> {
    br#"{"jsonrpc":"2.0","method":"heartbeat","params":{"session-id":"1"}}"#.to_vec()
}

#[test]
fn single_chunk_keyframe_bundle_matches_documented_byte_layout() {
    let payload = keyframe_bundle_3577_bytes();
    let field_b = field_b_for_width(1280);
    assert_eq!(field_b, 327_680, "baseline 1280-wide field B per the research doc");

    let chunks = frame_message(REPORT_ID_VIDEO, 0, field_b, &payload);

    // 3577 < 4080 (FIRST_CHUNK_PAYLOAD_MAX): must fit in exactly one chunk.
    assert_eq!(chunks.len(), 1);
    let chunk = &chunks[0];
    assert_eq!(chunk.len(), REPORT_SIZE);

    // Header bytes, asserted at their exact documented offsets.
    assert_eq!(chunk[0], 2, "Report ID");
    assert_eq!(&chunk[1..5], &3577u32.to_be_bytes(), "total message length");
    assert_eq!(&chunk[5..9], &0u32.to_be_bytes(), "field A");
    assert_eq!(&chunk[9..13], &327_680u32.to_be_bytes(), "field B");
    assert_eq!(&chunk[13..16], &[0, 0, 0], "zero padding");

    // Payload bytes start at offset 16 and match the original fixture exactly.
    assert_eq!(&chunk[16..16 + 3577], payload.as_slice());

    // Everything past the real payload, out to the fixed 4096-byte report size, is zero.
    assert!(chunk[16 + 3577..].iter().all(|&b| b == 0));

    // Round-trip back through parse_chunks.
    let parsed = parse_chunks(&chunks).unwrap();
    assert_eq!(parsed.report_id, 2);
    assert_eq!(parsed.field_a, 0);
    assert_eq!(parsed.field_b, 327_680);
    assert_eq!(parsed.payload, payload);
}

#[test]
fn multi_chunk_inter_frame_requires_exactly_seventeen_chunks() {
    let payload = inter_frame_67666_bytes();
    let field_b = field_b_for_width(1280);

    let chunks = frame_message(REPORT_ID_VIDEO, 0, field_b, &payload);

    // 4080 (first chunk) + 16 * 4095 (if all continuation chunks were full) would be 69,  520
    // bytes of capacity; the documented fixture needs exactly 17 chunks total (1 first +
    // 16 continuation), with the last continuation chunk partially filled. Verify the exact
    // split: first chunk carries FIRST_CHUNK_PAYLOAD_MAX (4080) bytes, then 15 full 4095-byte
    // continuation chunks, then one final continuation chunk with the remainder.
    let expected_total_chunks = 17;
    assert_eq!(chunks.len(), expected_total_chunks);

    let remaining_after_first = payload.len() - FIRST_CHUNK_PAYLOAD_MAX;
    let full_continuation_chunks = remaining_after_first / CONTINUATION_CHUNK_PAYLOAD_MAX;
    let last_chunk_payload_len = remaining_after_first % CONTINUATION_CHUNK_PAYLOAD_MAX;
    assert_eq!(full_continuation_chunks, 15);
    assert_eq!(last_chunk_payload_len, 2161);

    // First chunk: header + exactly FIRST_CHUNK_PAYLOAD_MAX payload bytes.
    let first = &chunks[0];
    assert_eq!(first.len(), REPORT_SIZE);
    assert_eq!(first[0], 2);
    assert_eq!(&first[1..5], &67666u32.to_be_bytes());
    assert_eq!(&first[9..13], &327_680u32.to_be_bytes());
    assert_eq!(
        &first[16..16 + FIRST_CHUNK_PAYLOAD_MAX],
        &payload[0..FIRST_CHUNK_PAYLOAD_MAX]
    );

    // A full continuation chunk (index 1): 1-byte Report ID + 4095 raw payload bytes, no header.
    let cont1 = &chunks[1];
    assert_eq!(cont1.len(), REPORT_SIZE);
    assert_eq!(cont1[0], 2, "continuation chunk still carries the Report ID byte");
    let expected_slice =
        &payload[FIRST_CHUNK_PAYLOAD_MAX..FIRST_CHUNK_PAYLOAD_MAX + CONTINUATION_CHUNK_PAYLOAD_MAX];
    assert_eq!(&cont1[1..1 + CONTINUATION_CHUNK_PAYLOAD_MAX], expected_slice);

    // The final (17th) chunk: partially filled (2161 payload bytes), zero-padded tail.
    let last = chunks.last().unwrap();
    assert_eq!(last.len(), REPORT_SIZE);
    assert_eq!(last[0], 2);
    let last_payload_start = payload.len() - last_chunk_payload_len;
    assert_eq!(
        &last[1..1 + last_chunk_payload_len],
        &payload[last_payload_start..]
    );
    assert!(last[1 + last_chunk_payload_len..].iter().all(|&b| b == 0));

    // Round-trip back through parse_chunks.
    let parsed = parse_chunks(&chunks).unwrap();
    assert_eq!(parsed.report_id, 2);
    assert_eq!(parsed.field_a, 0);
    assert_eq!(parsed.field_b, 327_680);
    assert_eq!(parsed.payload, payload);
}

#[test]
fn mi00_heartbeat_control_message_is_single_chunk_report_id_one() {
    let payload = heartbeat_payload();
    // MI_00 control channel: field B's semantics are video-specific (width * 256) per the
    // research doc; for a non-video control message there's no documented meaning for field B,
    // so this test uses 0 as the structurally sensible default — field A is always 0 regardless
    // of channel per the research doc's live-tested finding.
    let chunks = frame_message(REPORT_ID_CONTROL, 0, 0, &payload);

    assert_eq!(chunks.len(), 1);
    let chunk = &chunks[0];
    assert_eq!(chunk[0], 1, "MI_00 Report ID is 1 (interface_number + 1)");
    assert_eq!(&chunk[1..5], &(payload.len() as u32).to_be_bytes());
    assert_eq!(&chunk[9..13], &0u32.to_be_bytes());
    assert_eq!(&chunk[16..16 + payload.len()], payload.as_slice());

    let parsed = parse_chunks(&chunks).unwrap();
    assert_eq!(parsed.report_id, 1);
    assert_eq!(parsed.payload, payload);
    // Sanity check this is well-formed JSON of the documented heartbeat shape.
    let text = std::str::from_utf8(&parsed.payload).unwrap();
    assert!(text.contains(r#""method":"heartbeat""#));
    assert!(text.contains(r#""jsonrpc":"2.0""#));
}
