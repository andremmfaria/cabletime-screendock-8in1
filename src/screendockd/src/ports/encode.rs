//! Encode port: the [`Encoder`] trait, the [`VideoMessage`] unit the dock's video channel
//! carries, the settings every encoder takes and the error type.
//!
//! Implementations live in `adapters::encode`.

use std::path::PathBuf;

use thiserror::Error;

use crate::config::{Config, X264Preset};
use crate::ports::capture::Frame;
use crate::protocol::annexb::{contains_idr, AnnexBError};
use crate::protocol::display::DisplayGeometry;

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
    /// Target bit rate in kilobits per second; also the VBV maximum rate.
    pub bitrate_kbps: u32,
    /// libx264 preset.
    pub preset: X264Preset,
    /// Apply libx264's `zerolatency` tune.
    pub zerolatency: bool,
    /// VBV buffer in milliseconds of `bitrate_kbps`; 0 disables the cap.
    pub vbv_buffer_ms: u32,
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
            preset: config.encode.preset,
            zerolatency: config.encode.tune_zerolatency,
            vbv_buffer_ms: config.encode.vbv_buffer_ms,
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
