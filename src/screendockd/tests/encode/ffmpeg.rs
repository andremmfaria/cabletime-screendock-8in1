//! Encodes a synthetic frame sequence with the libx264 encoder and verifies the result: message
//! shape (keyframe bundle, then single-NAL inter-frames), Constrained Baseline in the SPS, and a
//! full decode by the `ffmpeg` command line tool.
//!
//! Only built with `--features encode-ffmpeg`. Skips (passes with a note on stderr) when the
//! `ffmpeg` binary is missing; `SCREENDOCK_REQUIRE_FFMPEG=1` turns that into a failure, and CI
//! sets it.

use std::io::Write;
use std::process::{Command, Stdio};

use screendockd::adapters::encode::ffmpeg::FfmpegEncoder;
use screendockd::ports::capture::{Frame, PixelFormat};
use screendockd::ports::encode::{Encoder, EncoderSettings, VideoMessage};
use screendockd::protocol::annexb::split_nals;
use screendockd::protocol::display::{DisplayGeometry, Orientation};

const SRC_W: u32 = 640;
const SRC_H: u32 = 480;
const FRAMES: usize = 30;
const KEYFRAME_INTERVAL: u32 = 10;

fn settings(width: u32, height: u32, orientation: Orientation) -> EncoderSettings {
    EncoderSettings {
        geometry: DisplayGeometry::new(width, height, orientation).unwrap(),
        fps: 25,
        keyframe_interval: KEYFRAME_INTERVAL,
        bitrate_kbps: 1500,
        preset: screendockd::utils::config::X264Preset::Ultrafast,
        zerolatency: true,
        vbv_buffer_ms: 200,
    }
}

/// A moving gradient with a padded stride, so stride handling and motion are both exercised.
fn synthetic_frame(n: usize) -> Frame {
    let stride = SRC_W as usize * 4 + 64;
    let mut data = vec![0u8; stride * SRC_H as usize];
    for y in 0..SRC_H as usize {
        for x in 0..SRC_W as usize {
            let at = y * stride + x * 4;
            data[at] = ((x + n * 8) % 256) as u8;
            data[at + 1] = ((y + n * 4) % 256) as u8;
            data[at + 2] = ((x + y) % 256) as u8;
            data[at + 3] = 255;
        }
    }
    Frame::new(SRC_W, SRC_H, PixelFormat::Bgra8888, stride, data).unwrap()
}

fn encode_sequence(encoder: &mut FfmpegEncoder) -> Vec<VideoMessage> {
    let mut out = Vec::new();
    for n in 0..FRAMES {
        out.extend(encoder.encode(&synthetic_frame(n)).expect("encode"));
    }
    out
}

fn nal_types(message: &VideoMessage) -> Vec<u8> {
    split_nals(message.as_bytes())
        .unwrap()
        .iter()
        .map(|nal| {
            let header = if nal.starts_with(&[0, 0, 0, 1]) { 4 } else { 3 };
            nal[header] & 0x1F
        })
        .collect()
}

fn ffmpeg_available() -> bool {
    let ok = Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        assert!(
            std::env::var_os("SCREENDOCK_REQUIRE_FFMPEG").is_none(),
            "ffmpeg binary unavailable but SCREENDOCK_REQUIRE_FFMPEG is set"
        );
        eprintln!("skipping decode check: ffmpeg binary not found");
    }
    ok
}

#[test]
fn messages_follow_the_dock_policy() {
    let mut encoder = FfmpegEncoder::new(settings(320, 240, Orientation::Landscape)).unwrap();
    let messages = encode_sequence(&mut encoder);
    assert_eq!(
        messages.len(),
        FRAMES,
        "one message per frame, no B-frame delay"
    );

    for (n, message) in messages.iter().enumerate() {
        let types = nal_types(message);
        if n % KEYFRAME_INTERVAL as usize == 0 {
            assert!(message.is_keyframe(), "frame {n} should be a keyframe");
            // SPS and PPS first, IDR slice last, bundled in one message.
            assert_eq!(types[0], 7, "frame {n}: {types:?}");
            assert_eq!(types[1], 8, "frame {n}: {types:?}");
            assert_eq!(*types.last().unwrap(), 5, "frame {n}: {types:?}");
        } else {
            assert_eq!(types, [1], "frame {n} must be a single non-IDR slice NAL");
        }
    }

    // SPS payload: profile_idc, constraint flags, level_idc. Constrained Baseline is profile 66
    // with constraint_set1 (0x40) set.
    let nals = split_nals(messages[0].as_bytes()).unwrap();
    let sps = nals[0];
    let header = if sps.starts_with(&[0, 0, 0, 1]) { 4 } else { 3 };
    assert_eq!(sps[header + 1], 66, "profile_idc");
    assert_ne!(sps[header + 2] & 0x40, 0, "constraint_set1_flag");
}

#[test]
fn keyframe_request_forces_an_idr() {
    let mut encoder = FfmpegEncoder::new(settings(320, 240, Orientation::Landscape)).unwrap();
    for n in 0..3 {
        encoder.encode(&synthetic_frame(n)).unwrap();
    }
    encoder.request_keyframe();
    let messages = encoder.encode(&synthetic_frame(3)).unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].is_keyframe());
}

fn decode_with_cli(messages: &[VideoMessage], width: u32, height: u32) -> usize {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.h264");
    let mut file = std::fs::File::create(&path).unwrap();
    for message in messages {
        file.write_all(message.as_bytes()).unwrap();
    }
    drop(file);

    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&path)
        .args(["-f", "rawvideo", "-pix_fmt", "gray", "-"])
        .output()
        .expect("run ffmpeg");
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "ffmpeg decode reported: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout.len() % (width * height) as usize, 0);
    output.stdout.len() / (width * height) as usize
}

#[test]
fn ffmpeg_cli_decodes_landscape_output() {
    if !ffmpeg_available() {
        return;
    }
    let mut encoder = FfmpegEncoder::new(settings(320, 240, Orientation::Landscape)).unwrap();
    let messages = encode_sequence(&mut encoder);
    assert_eq!(decode_with_cli(&messages, 320, 240), FRAMES);
}

#[test]
fn ffmpeg_cli_decodes_portrait_output() {
    if !ffmpeg_available() {
        return;
    }
    let mut encoder = FfmpegEncoder::new(settings(240, 320, Orientation::Portrait)).unwrap();
    let messages = encode_sequence(&mut encoder);
    assert_eq!(decode_with_cli(&messages, 240, 320), FRAMES);
}
