//! libx264 encoder through `ffmpeg-next` (feature `encode-ffmpeg`).
//!
//! Frames are converted from the capture layout (BGRA or RGBA, 4 bytes per pixel) to yuv420p and
//! scaled to the target size with swscale, then encoded by libx264 configured for the dock:
//! Constrained Baseline, no B-frames, one slice per frame (so an inter-frame is exactly one
//! NAL), a single encoder thread (frame threading would delay output by several frames), SPS and
//! PPS repeated on every keyframe, forced IDR keyframes. The packets come out as Annex-B and go
//! through [`messages_from_packet`](crate::adapters::encode::messages_from_packet).

use ffmpeg_next as ffmpeg;

use ffmpeg::format::Pixel;
use ffmpeg::software::scaling::{Context as Scaler, Flags};
use ffmpeg::{codec, encoder, frame, picture, Dictionary, Packet};

use crate::adapters::encode::messages_from_packet;
use crate::ports::capture::{Frame, PixelFormat};
use crate::ports::encode::{EncodeError, Encoder, EncoderSettings, VideoMessage};

/// Scaler plus the source geometry it was built for.
struct SourceScaler {
    scaler: Scaler,
    src: frame::Video,
    width: u32,
    height: u32,
    pixel: Pixel,
}

/// libx264 encoder producing dock video messages.
pub struct FfmpegEncoder {
    settings: EncoderSettings,
    encoder: encoder::Video,
    source: Option<SourceScaler>,
    pts: i64,
    force_keyframe: bool,
}

fn setup_err(e: ffmpeg::Error) -> EncodeError {
    EncodeError::Setup(e.to_string())
}

fn backend_err(e: ffmpeg::Error) -> EncodeError {
    EncodeError::Backend(e.to_string())
}

fn pixel_for(format: PixelFormat) -> Pixel {
    match format {
        PixelFormat::Bgrx8888 | PixelFormat::Bgra8888 => Pixel::BGRA,
        PixelFormat::Rgbx8888 | PixelFormat::Rgba8888 => Pixel::RGBA,
    }
}

/// The `x264-params` string for the settings. The VBV cap (`vbv-maxrate` equal to the target
/// rate, `vbv-bufsize` of `vbv_buffer_ms` at that rate, both in kilobits) bounds every frame,
/// so a keyframe cannot grow far beyond the buffer.
fn x264_params(settings: &EncoderSettings) -> String {
    let mut params = format!(
        "bframes=0:slices=1:sliced-threads=0:threads=1:aud=0:repeat-headers=1:\
         keyint={0}:min-keyint={0}:scenecut=0",
        settings.keyframe_interval
    );
    if settings.vbv_buffer_ms > 0 {
        let bufsize =
            (u64::from(settings.bitrate_kbps) * u64::from(settings.vbv_buffer_ms) / 1000).max(1);
        params.push_str(&format!(
            ":vbv-maxrate={}:vbv-bufsize={bufsize}",
            settings.bitrate_kbps
        ));
    }
    params
}

impl FfmpegEncoder {
    /// Opens libx264 for the settings' geometry and frame rate.
    pub fn new(settings: EncoderSettings) -> Result<Self, EncodeError> {
        ffmpeg::init().map_err(setup_err)?;
        let codec = encoder::find_by_name("libx264").ok_or_else(|| EncodeError::Unavailable {
            name: "ffmpeg",
            reason: "libavcodec was built without libx264".into(),
        })?;

        let fps = i32::try_from(settings.fps)
            .map_err(|_| EncodeError::Setup(format!("fps {} is too large", settings.fps)))?;
        let mut video = codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .map_err(setup_err)?;
        video.set_width(settings.geometry.width().0);
        video.set_height(settings.geometry.height().0);
        video.set_format(Pixel::YUV420P);
        video.set_time_base((1, fps));
        video.set_frame_rate(Some((fps, 1)));
        video.set_gop(settings.keyframe_interval);
        video.set_max_b_frames(0);
        video.set_bit_rate(settings.bitrate_kbps as usize * 1000);

        let mut options = Dictionary::new();
        options.set("preset", settings.preset.as_str());
        if settings.zerolatency {
            options.set("tune", "zerolatency");
        }
        options.set("profile", "baseline");
        options.set("forced-idr", "1");
        options.set("x264-params", &x264_params(&settings));
        let encoder = video.open_with(options).map_err(setup_err)?;

        Ok(Self {
            settings,
            encoder,
            source: None,
            pts: 0,
            force_keyframe: false,
        })
    }

    /// Makes sure the scaler matches this frame's geometry, rebuilding it when the source changed.
    fn ensure_source(&mut self, frame: &Frame) -> Result<(), EncodeError> {
        let pixel = pixel_for(frame.format);
        let stale = self
            .source
            .as_ref()
            .is_none_or(|s| s.width != frame.width || s.height != frame.height || s.pixel != pixel);
        if stale {
            let scaler = Scaler::get(
                pixel,
                frame.width,
                frame.height,
                Pixel::YUV420P,
                self.settings.geometry.width().0,
                self.settings.geometry.height().0,
                Flags::BILINEAR,
            )
            .map_err(setup_err)?;
            self.source = Some(SourceScaler {
                scaler,
                src: frame::Video::new(pixel, frame.width, frame.height),
                width: frame.width,
                height: frame.height,
                pixel,
            });
        }
        Ok(())
    }

    fn drain_packets(&mut self) -> Result<Vec<VideoMessage>, EncodeError> {
        let mut messages = Vec::new();
        let mut packet = Packet::empty();
        loop {
            match self.encoder.receive_packet(&mut packet) {
                Ok(()) => {
                    if let Some(data) = packet.data() {
                        messages.extend(messages_from_packet(data)?);
                    }
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => break,
                Err(ffmpeg::Error::Eof) => break,
                Err(e) => return Err(backend_err(e)),
            }
        }
        Ok(messages)
    }
}

impl Encoder for FfmpegEncoder {
    fn name(&self) -> &'static str {
        "ffmpeg-libx264"
    }

    fn encode(&mut self, frame: &Frame) -> Result<Vec<VideoMessage>, EncodeError> {
        let row_bytes = frame.width as usize * frame.format.bytes_per_pixel();
        self.ensure_source(frame)?;
        let source = self.source.as_mut().expect("ensure_source set it");

        // The AVFrame's rows are padded to its own linesize, which rarely equals the capture
        // stride, so copy row by row.
        let linesize = source.src.stride(0);
        let dst = source.src.data_mut(0);
        for y in 0..frame.height as usize {
            let from = y * frame.stride;
            dst[y * linesize..y * linesize + row_bytes]
                .copy_from_slice(&frame.data[from..from + row_bytes]);
        }

        let mut yuv = frame::Video::new(
            Pixel::YUV420P,
            self.settings.geometry.width().0,
            self.settings.geometry.height().0,
        );
        source
            .scaler
            .run(&source.src, &mut yuv)
            .map_err(backend_err)?;

        yuv.set_pts(Some(self.pts));
        self.pts += 1;
        if std::mem::take(&mut self.force_keyframe) {
            yuv.set_kind(picture::Type::I);
        }
        self.encoder.send_frame(&yuv).map_err(backend_err)?;
        self.drain_packets()
    }

    fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::X264Preset;
    use crate::protocol::display::{DisplayGeometry, Orientation};

    fn settings(vbv_buffer_ms: u32) -> EncoderSettings {
        EncoderSettings {
            geometry: DisplayGeometry::new(1280, 720, Orientation::Landscape).unwrap(),
            fps: 25,
            keyframe_interval: 50,
            bitrate_kbps: 4000,
            preset: X264Preset::Ultrafast,
            zerolatency: true,
            vbv_buffer_ms,
        }
    }

    #[test]
    fn vbv_cap_follows_rate_and_buffer_time() {
        let params = x264_params(&settings(100));
        assert!(
            params.contains("vbv-maxrate=4000:vbv-bufsize=400"),
            "{params}"
        );
        assert!(params.contains("keyint=50:min-keyint=50"));
        assert!(!params.contains("forced-idr"));
        assert!(!x264_params(&settings(0)).contains("vbv"));
    }
}
