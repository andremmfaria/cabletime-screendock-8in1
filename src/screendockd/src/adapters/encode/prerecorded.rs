//! Test encoder that replays pre-encoded Annex-B messages.
//!
//! Every [`Encoder::encode`] call returns the next message of the stream and ignores the pixels
//! of the frame it is given, looping back to the first keyframe bundle at the end. It needs no
//! native libraries, so the whole daemon path (capture, framing, hidraw writes) is testable on
//! hosts without libav.

use std::path::Path;

use crate::ports::capture::Frame;
use crate::ports::encode::{EncodeError, Encoder, EncoderSettings, VideoMessage};
use crate::protocol::annexb::messages_from_annexb;

/// Replays a fixed message sequence in a loop.
#[derive(Debug)]
pub struct PrerecordedEncoder {
    messages: Vec<VideoMessage>,
    next: usize,
}

impl PrerecordedEncoder {
    /// Builds the encoder from already split messages. The first one must be a keyframe bundle.
    pub fn new(messages: Vec<Vec<u8>>, _settings: EncoderSettings) -> Result<Self, EncodeError> {
        let messages: Vec<VideoMessage> = messages.into_iter().map(VideoMessage::new).collect();
        if !messages.first().is_some_and(VideoMessage::is_keyframe) {
            return Err(EncodeError::NoLeadingKeyframe);
        }
        Ok(Self { messages, next: 0 })
    }

    /// Reads an Annex-B file (for example from `ffmpeg -f h264`) and splits it into messages.
    pub fn from_file(path: &Path, settings: EncoderSettings) -> Result<Self, EncodeError> {
        let data = std::fs::read(path).map_err(|source| EncodeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::new(messages_from_annexb(&data)?, settings)
    }
}

impl Encoder for PrerecordedEncoder {
    fn name(&self) -> &'static str {
        "prerecorded"
    }

    fn encode(&mut self, _frame: &Frame) -> Result<Vec<VideoMessage>, EncodeError> {
        let message = self.messages[self.next].clone();
        self.next = (self.next + 1) % self.messages.len();
        Ok(vec![message])
    }

    fn request_keyframe(&mut self) {
        self.next = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::capture::PixelFormat;
    use crate::protocol::display::{DisplayGeometry, Orientation};

    fn settings() -> EncoderSettings {
        EncoderSettings {
            geometry: DisplayGeometry::new(1280, 720, Orientation::Landscape).unwrap(),
            fps: 25,
            keyframe_interval: 10,
            bitrate_kbps: 1000,
            preset: crate::config::X264Preset::Ultrafast,
            zerolatency: true,
            vbv_buffer_ms: 200,
        }
    }

    fn frame() -> Frame {
        Frame::new(2, 2, PixelFormat::Bgrx8888, 8, vec![0; 16]).unwrap()
    }

    fn stream() -> Vec<Vec<u8>> {
        vec![
            vec![
                0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65, 3,
            ],
            vec![0, 0, 0, 1, 0x41, 4],
        ]
    }

    #[test]
    fn loops_and_restarts_on_keyframe_request() {
        let mut enc = PrerecordedEncoder::new(stream(), settings()).unwrap();
        let kinds: Vec<bool> = (0..5)
            .map(|_| enc.encode(&frame()).unwrap()[0].is_keyframe())
            .collect();
        assert_eq!(kinds, [true, false, true, false, true]);
        enc.encode(&frame()).unwrap();
        enc.request_keyframe();
        assert!(enc.encode(&frame()).unwrap()[0].is_keyframe());
    }

    #[test]
    fn rejects_stream_without_leading_keyframe() {
        let err = PrerecordedEncoder::new(vec![stream().remove(1)], settings()).unwrap_err();
        assert!(matches!(err, EncodeError::NoLeadingKeyframe));
    }

    #[test]
    fn reports_missing_file() {
        let err = PrerecordedEncoder::from_file(Path::new("/nonexistent/x.h264"), settings())
            .unwrap_err();
        assert!(matches!(err, EncodeError::Io { .. }));
    }
}
