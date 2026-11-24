//! H.264 encoder implementations and the message policy they share.
//!
//! An [`Encoder`] turns captured [`Frame`]s into [`VideoMessage`]s, the unit the dock's video
//! channel carries. The message policy matches the vendor behavior documented in
//! `docs/research/full-protocol-spec-2026-11-21.md`:
//!
//! - the first message of a session (and every later keyframe) bundles SPS, PPS, SEI and the IDR
//!   slice into one multi-NAL message;
//! - every inter-frame is a message holding exactly one NAL (type 1);
//! - Constrained Baseline profile, no B-frames, roughly 25 fps.
//!
//! The bundling policy lives in [`messages_from_packet`] and is shared by every implementation,
//! so a new backend only has to produce Annex-B packets. Implementations:
//!
//! - [`ffmpeg::FfmpegEncoder`] (feature `encode-ffmpeg`): libx264 through libavcodec, including
//!   BGRA to yuv420p conversion and scaling to the target size.
//! - [`prerecorded::PrerecordedEncoder`]: replays messages split from an Annex-B file, needs no
//!   native libraries, used to test the daemon without libav.

#[cfg(feature = "encode-ffmpeg")]
pub mod ffmpeg;
pub mod prerecorded;

use std::path::PathBuf;

use crate::ports::encode::{EncodeError, Encoder, EncoderSettings, VideoMessage};
use crate::protocol::annexb::{messages_from_annexb, AnnexBError};

/// Which encoder `create_encoder` builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncoderKind {
    /// libx264 through ffmpeg (feature `encode-ffmpeg`).
    Ffmpeg,
    /// Replay of an Annex-B file.
    Prerecorded(PathBuf),
}

/// Constructs the requested encoder once at startup.
pub fn create_encoder(
    kind: &EncoderKind,
    settings: EncoderSettings,
) -> Result<Box<dyn Encoder>, EncodeError> {
    match kind {
        #[cfg(feature = "encode-ffmpeg")]
        EncoderKind::Ffmpeg => Ok(Box::new(ffmpeg::FfmpegEncoder::new(settings)?)),
        #[cfg(not(feature = "encode-ffmpeg"))]
        EncoderKind::Ffmpeg => Err(EncodeError::Unavailable {
            name: "ffmpeg",
            reason: "this build lacks the encode-ffmpeg feature".into(),
        }),
        EncoderKind::Prerecorded(path) => Ok(Box::new(prerecorded::PrerecordedEncoder::from_file(
            path, settings,
        )?)),
    }
}

/// Applies the message policy to one packet of Annex-B output: parameter sets and SEI are
/// bundled with the IDR slice that follows, inter-frame slices stand alone, access unit
/// delimiters are dropped. A packet without slices yields no messages.
pub fn messages_from_packet(packet: &[u8]) -> Result<Vec<VideoMessage>, EncodeError> {
    match messages_from_annexb(packet) {
        Ok(messages) => Ok(messages.into_iter().map(VideoMessage::new).collect()),
        Err(AnnexBError::NoSlices) => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::annexb::split_nals;

    fn nal(ty: u8, len: usize) -> Vec<u8> {
        let mut v = vec![0, 0, 0, 1, 0x60 | ty];
        v.extend(std::iter::repeat_n(0xAB, len));
        v
    }

    #[test]
    fn keyframe_packet_becomes_one_bundle() {
        let mut packet = Vec::new();
        for (ty, len) in [(7, 10), (8, 4), (6, 20), (5, 200)] {
            packet.extend(nal(ty, len));
        }
        let messages = messages_from_packet(&packet).unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].is_keyframe());
        assert_eq!(split_nals(messages[0].as_bytes()).unwrap().len(), 4);
    }

    #[test]
    fn inter_packet_is_single_nal_and_empty_packet_is_dropped() {
        let messages = messages_from_packet(&nal(1, 50)).unwrap();
        assert_eq!(messages.len(), 1);
        assert!(!messages[0].is_keyframe());
        assert_eq!(split_nals(messages[0].as_bytes()).unwrap().len(), 1);
        assert!(messages_from_packet(&nal(9, 1)).unwrap().is_empty());
    }

    #[test]
    fn non_annexb_output_is_an_error() {
        assert!(matches!(
            messages_from_packet(&[1, 2, 3, 4]),
            Err(EncodeError::AnnexB(AnnexBError::NoStartCode))
        ));
    }
}
