//! H.264 encoder wrapper (Milestone 5).
//!
//! Per `docs/implementation-plan.md`'s Concrete Technology Choices, this will wrap `ffmpeg-next`
//! driving libavcodec's `libx264` encoder, configured for Constrained Baseline profile, no
//! B-frames, ~25fps, replicating the vendor's confirmed keyframe-bundling behavior documented in
//! `docs/research/full-protocol-spec-2026-11-21.md`: the first message of a session bundles
//! SPS+PPS+SEI+IDR-slice as one multi-NAL message, and every subsequent inter-frame is a single
//! NAL type-1 message. Both message shapes are exactly what `framing.rs` already knows how to
//! frame/chunk (see its unit tests for fixtures matching this exact shape).
//!
//! Not implemented here: depends on Milestone 4 Stage 1 (a frame source) and pulls in the
//! `ffmpeg-next` dependency, out of scope for Milestone 1.

/// Placeholder for the encoder handle; Milestone 5 will give this real fields (encoder context,
/// configured width/height/profile, keyframe-bundle buffering state).
pub struct Encoder;

impl Encoder {
    /// Encodes one captured frame, returning either a keyframe bundle or a single inter-frame
    /// NAL, ready to be handed to `framing::frame_message`.
    pub fn encode_frame(&mut self) -> ! {
        todo!("Milestone 5: ffmpeg-next libx264 Constrained Baseline encode, keyframe bundling")
    }
}
