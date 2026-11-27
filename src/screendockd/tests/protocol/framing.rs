//! Integration tests for `framing.rs`'s chunking/header math, using real captured message
//! shapes documented in `docs/research/full-protocol-spec-2026-11-21.md`.
//!
//! The H.264 NAL *content* inside each fixture is synthetic placeholder bytes of the correct
//! length and with the correct NAL-type header byte - the research docs' own pcap files aren't
//! available in this sandbox, and `framing.rs`'s job is the header/chunking math, not H.264
//! content validation. Exact byte offsets/values asserted below come directly from the
//! documented wire layout, not guessed.
//!
//! Layout under test: every report starts with a 16-byte header. The first report carries the
//! payload length (bytes 1..4), width (bytes 8..11) and height (bytes 12..15); continuation
//! reports carry the Report ID followed by 15 zero bytes. Payload bytes start at offset 16.

use screendockd::protocol::framing::{
    frame_message, parse_chunks, FramingError, MessageBuilder, PictureSize, ReportId,
    CONTINUATION_CHUNK_PAYLOAD_MAX, FIRST_CHUNK_PAYLOAD_MAX, HEADER_SIZE, REPORT_SIZE,
};

/// Builds one Annex-B NAL unit: a 4-byte start code, a 1-byte NAL header
/// (`(nal_ref_idc << 5) | nal_type`), and `body_len` filler bytes.
fn nal(nal_type: u8, nal_ref_idc: u8, body_len: usize) -> Vec<u8> {
    let mut v = vec![0x00, 0x00, 0x00, 0x01];
    v.push((nal_ref_idc << 5) | nal_type);
    v.extend(std::iter::repeat_n(0xAAu8, body_len));
    v
}

/// A keyframe-bundle shape: SPS (type 7) + PPS (type 8) + SEI (type 6) + IDR slice (type 5)
/// concatenated as one message of exactly 3577 bytes.
fn keyframe_bundle_3577_bytes() -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend(nal(7, 3, 25));
    payload.extend(nal(8, 3, 7));
    payload.extend(nal(6, 0, 15));
    payload.extend(nal(5, 3, 3510));
    assert_eq!(payload.len(), 3577, "fixture construction sanity check");
    payload
}

/// A single-NAL inter-frame shape of exactly 67666 bytes.
fn inter_frame_67666_bytes() -> Vec<u8> {
    let payload = nal(1, 2, 67661);
    assert_eq!(payload.len(), 67666, "fixture construction sanity check");
    payload
}

fn heartbeat_payload() -> Vec<u8> {
    br#"{"jsonrpc":"2.0", "method": "heartbeat", "params": {"session-id": "25282281"}}"#.to_vec()
}

#[test]
fn single_report_keyframe_bundle_matches_documented_byte_layout() {
    let payload = keyframe_bundle_3577_bytes();
    let chunks = frame_message(ReportId::Video, PictureSize::new(1280, 720), &payload);

    // 3577 < 4080: exactly one report.
    assert_eq!(chunks.len(), 1);
    let chunk = &chunks[0];
    assert_eq!(chunk.len(), REPORT_SIZE);

    assert_eq!(chunk[0], 2, "Report ID");
    assert_eq!(&chunk[1..5], &3577u32.to_be_bytes(), "payload length");
    assert_eq!(&chunk[5..8], &[0, 0, 0], "bytes 5..7 are zero");
    assert_eq!(&chunk[8..12], &1280u32.to_be_bytes(), "width");
    assert_eq!(&chunk[12..16], &720u32.to_be_bytes(), "height");
    assert_eq!(&chunk[16..16 + 3577], payload.as_slice());
    assert!(chunk[16 + 3577..].iter().all(|&b| b == 0));

    let parsed = parse_chunks(&chunks).unwrap();
    assert_eq!(parsed.report_id, ReportId::Video);
    assert_eq!(parsed.size, PictureSize::new(1280, 720));
    assert_eq!(parsed.payload, payload);
}

#[test]
fn multi_report_inter_frame_requires_exactly_seventeen_reports() {
    let payload = inter_frame_67666_bytes();
    let chunks = frame_message(ReportId::Video, PictureSize::new(1280, 720), &payload);
    assert_eq!(chunks.len(), 17);

    // 4080 bytes in the first report, then 15 full 4080-byte continuations, then 2386.
    let remaining_after_first = payload.len() - FIRST_CHUNK_PAYLOAD_MAX;
    assert_eq!(remaining_after_first / CONTINUATION_CHUNK_PAYLOAD_MAX, 15);
    let last_len = remaining_after_first % CONTINUATION_CHUNK_PAYLOAD_MAX;
    assert_eq!(last_len, 2386);

    let first = &chunks[0];
    assert_eq!(first[0], 2);
    assert_eq!(&first[1..5], &67666u32.to_be_bytes());
    assert_eq!(&first[8..12], &1280u32.to_be_bytes());
    assert_eq!(&first[12..16], &720u32.to_be_bytes());
    assert_eq!(
        &first[16..16 + FIRST_CHUNK_PAYLOAD_MAX],
        &payload[0..FIRST_CHUNK_PAYLOAD_MAX]
    );

    // Every continuation report: Report ID, 15 zero bytes, then up to 4080 payload bytes.
    for (i, chunk) in chunks.iter().enumerate().skip(1) {
        assert_eq!(chunk.len(), REPORT_SIZE);
        assert_eq!(chunk[0], 2, "continuation {i} Report ID");
        assert!(chunk[1..HEADER_SIZE].iter().all(|&b| b == 0), "chunk {i}");
        let start = FIRST_CHUNK_PAYLOAD_MAX + (i - 1) * CONTINUATION_CHUNK_PAYLOAD_MAX;
        let end = (start + CONTINUATION_CHUNK_PAYLOAD_MAX).min(payload.len());
        assert_eq!(
            &chunk[HEADER_SIZE..HEADER_SIZE + (end - start)],
            &payload[start..end]
        );
    }
    let last = chunks.last().unwrap();
    assert!(last[HEADER_SIZE + last_len..].iter().all(|&b| b == 0));

    let parsed = parse_chunks(&chunks).unwrap();
    assert_eq!(parsed.size, PictureSize::new(1280, 720));
    assert_eq!(parsed.payload, payload);
}

#[test]
fn exact_multiples_of_the_payload_capacity_do_not_add_an_empty_report() {
    let payload = vec![0x11u8; FIRST_CHUNK_PAYLOAD_MAX + CONTINUATION_CHUNK_PAYLOAD_MAX];
    let chunks = frame_message(ReportId::Video, PictureSize::new(64, 64), &payload);
    assert_eq!(chunks.len(), 2);
    assert_eq!(parse_chunks(&chunks).unwrap().payload, payload);
    let one_over = vec![0x11u8; FIRST_CHUNK_PAYLOAD_MAX + 1];
    assert_eq!(
        frame_message(ReportId::Video, PictureSize::new(64, 64), &one_over).len(),
        2
    );
}

#[test]
fn mi00_control_message_is_one_report_with_zero_width_and_height() {
    let payload = heartbeat_payload();
    let chunks = MessageBuilder::control().payload(&payload).build();

    assert_eq!(chunks.len(), 1);
    let chunk = &chunks[0];
    assert_eq!(chunk[0], 1, "MI_00 Report ID is 1");
    assert_eq!(&chunk[1..5], &(payload.len() as u32).to_be_bytes());
    assert_eq!(
        &chunk[5..16],
        &[0u8; 11],
        "bytes 5..15 are zero for control"
    );
    assert_eq!(&chunk[16..16 + payload.len()], payload.as_slice());

    let parsed = parse_chunks(&chunks).unwrap();
    assert_eq!(parsed.report_id, ReportId::Control);
    assert_eq!(parsed.size, PictureSize::zero());
    assert_eq!(parsed.payload, payload);
}

#[test]
fn builder_video_matches_frame_message_byte_for_byte() {
    let payload = keyframe_bundle_3577_bytes();
    let size = PictureSize::new(900, 720);
    let via_builder = MessageBuilder::video(size).payload(&payload).build();
    assert_eq!(via_builder, frame_message(ReportId::Video, size, &payload));
}

#[test]
fn packed_report_layout_without_continuation_header_is_rejected() {
    // A continuation laid out as Report ID plus payload (no zero header) must not parse.
    let payload = inter_frame_67666_bytes();
    let mut chunks = frame_message(ReportId::Video, PictureSize::new(1280, 720), &payload);
    chunks[1][1] = 0xAA;
    assert_eq!(
        parse_chunks(&chunks).unwrap_err(),
        FramingError::NonZeroContinuationHeader { index: 1 }
    );
}
