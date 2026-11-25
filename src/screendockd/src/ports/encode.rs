//! Encode port: the [`Encoder`] trait, the [`VideoMessage`] unit the dock's video channel
//! carries, the settings every encoder takes and the error type.
//!
//! Implementations live in `adapters::encode`.

pub use super::types::{
    EncodeError, Encoder, EncoderSettings, RateSettings, VideoMessage, DEFAULT_FPS,
};
use crate::protocol::annexb::contains_idr;
use crate::protocol::display::DisplayGeometry;
use crate::utils::config::Config;

impl VideoMessage {
    /// Wraps message bytes, detecting whether they hold an IDR slice.
    pub fn new(bytes: Vec<u8>) -> Self {
        let keyframe = contains_idr(&bytes);
        Self { bytes, keyframe }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// True when the message bundles an IDR slice.
    pub fn is_keyframe(&self) -> bool {
        self.keyframe
    }
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
