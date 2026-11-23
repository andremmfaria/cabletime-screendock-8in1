# Encoding

The `encode` module turns captured frames into the H.264 messages the dock's video channel carries. Source: `src/screendockd/src/encode/`.

## Message policy

Every encoder produces `VideoMessage` values (Annex-B bytes plus a keyframe flag) under one policy, implemented once in `encode::messages_from_packet`:

- A keyframe is one message holding the SPS, PPS, SEI and IDR slice NAL units in that order.
- An inter-frame is one message holding exactly one NAL unit of type 1.
- Access unit delimiters are dropped. A packet with no slice NAL unit yields no message.
- Constrained Baseline profile, no B-frames, about 25 fps.

A message is written to MI_01 through `framing::MessageBuilder::video`, which splits it into chunks; see [protocol-implementation.md](protocol-implementation.md).

## The `Encoder` trait

| Method | Meaning |
|---|---|
| `name()` | Short identifier for logs |
| `encode(&Frame)` | Encodes one captured frame and returns zero or more messages in send order, normally exactly one |
| `request_keyframe()` | Makes the next output a keyframe bundle. The daemon calls it after a failed write so the panel can resynchronize |

`EncoderSettings` carries the validated `DisplayGeometry`, the frame rate, the keyframe interval and the target bit rate. `EncoderSettings::from_config` fills them from the config; a `capture.fps` of 0 encodes at 25 fps. `create_encoder` builds a boxed encoder from an `EncoderKind`.

Errors are the typed `EncodeError` enum: unavailable encoder, setup failure, backend failure, invalid input frame, invalid Annex-B output, missing leading keyframe in a prerecorded stream, and file I/O failure.

## libx264 encoder (feature `encode-ffmpeg`)

`encode::ffmpeg::FfmpegEncoder` drives libx264 through the `ffmpeg-next` crate.

- Input: BGRA or RGBA frames (4 bytes per pixel, any stride). Rows are copied into an AVFrame and converted to yuv420p with swscale (bilinear), scaling to the target size. The scaler is rebuilt if the source size or format changes.
- Encoder options: `preset=ultrafast`, `tune=zerolatency`, `profile=baseline`, and the x264 parameters `bframes=0`, `slices=1`, `sliced-threads=0`, `threads=1`, `aud=0`, `repeat-headers=1`, `forced-idr=1`, `keyint` and `min-keyint` equal to `encode.keyframe_interval`, `scenecut=0`. One slice per frame keeps every inter-frame a single NAL unit. One thread avoids the multi-frame delay of frame threading.
- Bit rate comes from `encode.bitrate_kbps`; the time base is 1/fps.
- `request_keyframe` marks the next frame as an I picture; `forced-idr` makes it an IDR.

Build requirements: the libav development files (`libavcodec-dev`, `libavformat-dev`, `libavutil-dev`, `libswscale-dev`, `libavdevice-dev`), `libx264-dev`, `clang` and `pkg-config` on Debian and Ubuntu.

```text
cargo build --release --features encode-ffmpeg
```

Without the feature, the `ffmpeg` encoder is not part of the build and `create_encoder` returns `EncodeError::Unavailable`. Everything else builds and tests without libav. The packages built by CI are built with the feature.

Verification: `tests/encode_ffmpeg.rs` encodes a 30 frame synthetic sequence from a 640x480 BGRA source with a padded stride and checks the message shape (SPS then PPS then IDR in keyframes, a single NAL in every inter-frame, one message per frame), the SPS profile (66 with constraint_set1) and a keyframe request. It then decodes the output with the `ffmpeg` command line tool and checks the decoded frame count and sizes, for a landscape (320x240) and a portrait (240x320) target. CI runs these in the `encode-ffmpeg` job. The feature does not compile on a host without the libav development files.

## Prerecorded test encoder

`encode::prerecorded::PrerecordedEncoder` replays the messages split from an Annex-B file (for example `ffmpeg ... -f h264`) in a loop, ignoring the pixels it is given. The first message must be a keyframe bundle. It needs no native libraries, so the whole daemon path is testable without libav. The `run` and `bench` commands select it with `--h264-file PATH`. `tests/fixtures/sample.h264` is a 30 frame 320x180 Constrained Baseline stream with a keyframe every 10 frames, used by the daemon tests.

## Resolution

`display::DisplayGeometry` validates `display.width`, `display.height` and `display.orientation` and exposes the encode size and header field B. See [daemon.md](daemon.md#resolution).

## CPU measurement

The `bench` command measures the CPU cost of capture plus encode, the M5 CPU criterion. It runs the capture backend and the encoder back to back for a fixed time and reports frames, achieved frame rate, wall time, process CPU time (user and system, from `getrusage`) and CPU as a percentage of one core, plus the wall time spent in capture and in encode. HID writes are not part of the measurement and no dock is needed.

```text
screendockd bench --duration-secs 60
screendockd bench --duration-secs 60 --backend x11
```

For the 25 fps, 1280x720 criterion, run it on the target desktop with the default config (`display` 1280x720, `capture.fps` 25), on an idle session and again while the desktop is busy, and record the reported CPU percentage, the host CPU model, the libx264 version (`ffmpeg -version`) and the commit. The figure is process wide, so it includes encoder helper threads; divide by the core count for a per-machine share. Status: requires measurement on the target host; no figures are recorded in this repository.
