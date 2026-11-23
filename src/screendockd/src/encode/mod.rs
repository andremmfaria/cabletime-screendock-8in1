//! H.264 encoding (Milestone 5).
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

use thiserror::Error;

use crate::annexb::{contains_idr, messages_from_annexb, AnnexBError};
use crate::capture::Frame;
use crate::display::DisplayGeometry;
use crate::utils::config::Config;

/// Frame rate used when the config leaves pacing disabled (`capture.fps = 0`).
pub const DEFAULT_FPS: u32 = 25;

/// One dock video message: Annex-B bytes ready for `framing::MessageBuilder::video`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoMessage {
    bytes: Vec<u8>,
    keyframe: bool,
}

impl VideoMessage {
    /// Wraps message bytes, detecting whether they hold an IDR slice.
    pub fn new(bytes: Vec<u8>) -> Self {
        let keyframe = contains_idr(&bytes);
        Self { bytes, keyframe }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// True when the message bundles an IDR slice.
    pub fn is_keyframe(&self) -> bool {
        self.keyframe
    }
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
    /// The input frame cannot be encoded.
    #[error("invalid input frame: {0}")]
    InvalidFrame(String),
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
    /// Target bit rate in kilobits per second.
    pub bitrate_kbps: u32,
}

impl EncoderSettings {
    /// Builds settings from the validated geometry and the config sections.
    pub fn from_config(config: &Config, geometry: DisplayGeometry) -> Self {
        Self {
            geometry,
            fps: if config.capture.fps == 0 {
                DEFAULT_FPS
            } else {
                config.capture.fps
            },
            keyframe_interval: config.encode.keyframe_interval.max(1),
            bitrate_kbps: config.encode.bitrate_kbps.max(1),
        }
    }
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
    use crate::annexb::split_nals;

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
