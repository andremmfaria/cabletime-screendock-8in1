//! Types of the encoder adapters: the encoder kind and the encoder structs.

use std::path::PathBuf;

#[cfg(feature = "encode-ffmpeg")]
use ffmpeg::format::Pixel;
#[cfg(feature = "encode-ffmpeg")]
use ffmpeg::software::scaling::Context as Scaler;
#[cfg(feature = "encode-ffmpeg")]
use ffmpeg::{encoder, frame};
#[cfg(feature = "encode-ffmpeg")]
use ffmpeg_next as ffmpeg;

#[cfg(feature = "encode-ffmpeg")]
use crate::ports::encode::EncoderSettings;
use crate::ports::encode::VideoMessage;

/// Frames between the keyframes libx264 inserts on its own. It sits above every interval the
/// daemon accepts, so keyframes come only from the caller's clock and the interval can change
/// while the encoder runs.
#[cfg(feature = "encode-ffmpeg")]
pub(super) const KEYFRAME_CEILING: u32 = 3600;

/// Which encoder `create_encoder` builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncoderKind {
    /// libx264 through ffmpeg (feature `encode-ffmpeg`).
    Ffmpeg,
    /// Replay of an Annex-B file.
    Prerecorded(PathBuf),
}

#[cfg(feature = "encode-ffmpeg")]
/// Scaler plus the source geometry it was built for.
pub(super) struct SourceScaler {
    pub(super) scaler: Scaler,
    pub(super) src: frame::Video,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) pixel: Pixel,
}

#[cfg(feature = "encode-ffmpeg")]
/// libx264 encoder producing dock video messages.
pub struct FfmpegEncoder {
    pub(super) settings: EncoderSettings,
    pub(super) encoder: encoder::Video,
    pub(super) source: Option<SourceScaler>,
    pub(super) pts: i64,
    pub(super) force_keyframe: bool,
}

/// Replays a fixed message sequence in a loop.
#[derive(Debug)]
pub struct PrerecordedEncoder {
    pub(super) messages: Vec<VideoMessage>,
    pub(super) next: usize,
}
