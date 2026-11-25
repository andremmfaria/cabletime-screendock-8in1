# Encoding

The encode layer turns captured frames into the H.264 messages the dock's video channel carries. The `Encoder` trait and its data types are in `src/screendockd/src/ports/encode.rs`; the implementations and the message policy are in `src/screendockd/src/adapters/encode/`.

## Message policy

Every encoder produces `VideoMessage` values (Annex-B bytes plus a keyframe flag) under one policy, implemented once in `adapters::encode::messages_from_packet`:

- A keyframe is one message holding the SPS, PPS, SEI and IDR slice NAL units in that order.
- An inter-frame is one message holding exactly one NAL unit of type 1.
- Access unit delimiters are dropped. A packet with no slice NAL unit yields no message.
- Constrained Baseline profile, no B-frames, about 25 fps.

A message is written to MI_01 through `protocol::framing::MessageBuilder::video`, which splits it into chunks; see [protocol-implementation.md](protocol-implementation.md).

## The `Encoder` trait

| Method | Meaning |
|---|---|
| `name()` | Short identifier for logs |
| `encode(&Frame)` | Encodes one captured frame and returns zero or more messages in send order, normally exactly one |
| `request_keyframe()` | Makes the next output a keyframe bundle. The daemon calls it after a failed write so the panel can resynchronize |

`EncoderSettings` carries the validated `DisplayGeometry`, the frame rate, the keyframe interval and the target bit rate. `EncoderSettings::from_config` fills them from the config; a `capture.fps` of 0 encodes at 25 fps. `create_encoder` builds a boxed encoder from an `EncoderKind`.

Errors are the typed `EncodeError` enum: unavailable encoder, setup failure, backend failure, invalid input frame, invalid Annex-B output, missing leading keyframe in a prerecorded stream, and file I/O failure.

## libx264 encoder (feature `encode-ffmpeg`)

`adapters::encode::ffmpeg::FfmpegEncoder` drives libx264 through the `ffmpeg-next` crate.

- Input: BGRA or RGBA frames (4 bytes per pixel, any stride). Rows are copied into an AVFrame and converted to yuv420p with swscale (bilinear), scaling to the target size. The scaler is rebuilt if the source size or format changes.
- Encoder options: `preset` from `encode.preset`, `tune=zerolatency` when `encode.tune_zerolatency` is true, `profile=baseline`, the codec option `forced-idr=1`, and the x264 parameters `bframes=0`, `slices=1`, `sliced-threads=0`, `threads=1`, `aud=0`, `repeat-headers=1`, `keyint` and `min-keyint` equal to `encode.keyframe_interval`, `scenecut=0`, and, when `encode.vbv_buffer_ms` is above 0, `vbv-maxrate` equal to `encode.bitrate_kbps` and `vbv-bufsize` equal to `bitrate_kbps * vbv_buffer_ms / 1000` (both in kilobits). One slice per frame keeps every inter-frame a single NAL unit. One thread avoids the multi-frame delay of frame threading.
- Bit rate comes from `encode.bitrate_kbps`; the time base is 1/fps.
- The VBV cap bounds the size of any one frame, keyframes included, which bounds the number of 4080 byte reports a frame needs.
- `request_keyframe` marks the next frame as an I picture; `forced-idr` makes it an IDR.

Build requirements: the libav development files (`libavcodec-dev`, `libavformat-dev`, `libavutil-dev`, `libswscale-dev`, `libavdevice-dev`), `libx264-dev`, `clang` and `pkg-config` on Debian and Ubuntu.

```text
cargo build --release --features encode-ffmpeg
```

Without the feature, the `ffmpeg` encoder is not part of the build and `create_encoder` returns `EncodeError::Unavailable`. Everything else builds and tests without libav. The packages built by CI are built with the feature.

Verification: `tests/encode/ffmpeg.rs` encodes a 30 frame synthetic sequence from a 640x480 BGRA source with a padded stride and checks the message shape (SPS then PPS then IDR in keyframes, a single NAL in every inter-frame, one message per frame), the SPS profile (66 with constraint_set1) and a keyframe request. It then decodes the output with the `ffmpeg` command line tool and checks the decoded frame count and sizes, for a landscape (320x240) and a portrait (240x320) target. CI runs these in the `encode-ffmpeg` job. The feature does not compile on a host without the libav development files.

## Prerecorded test encoder

`adapters::encode::prerecorded::PrerecordedEncoder` replays the messages split from an Annex-B file (for example `ffmpeg ... -f h264`) in a loop, ignoring the pixels it is given. The first message must be a keyframe bundle. It needs no native libraries, so the whole daemon path is testable without libav. The `run` and `bench` commands select it with `--h264-file PATH`. `tests/fixtures/sample.h264` is a 30 frame 320x180 Constrained Baseline stream with a keyframe every 10 frames, used by the daemon tests.

## Resolution

`protocol::display::DisplayGeometry` validates `display.width`, `display.height` and `display.orientation` and exposes the encode size and the header width and height. See [daemon.md](daemon.md#resolution).

## CPU measurement

The `bench` command (`tools::bench`, feature `tools`) measures the CPU cost of capture plus encode, the CPU criterion. It runs the capture backend and the encoder back to back for a fixed time and reports frames, achieved frame rate, wall time, process CPU time (user and system, from `getrusage`) and CPU as a percentage of one core, plus the wall time spent in capture and in encode. HID writes are not part of the measurement and no dock is needed.

```text
screendockd bench --duration-secs 60
screendockd bench --duration-secs 60 --backend x11
```

For the 25 fps, 1280x720 criterion, run it on the target desktop with the default config (`display` 1280x720, `capture.fps` 25), on an idle session and again while the desktop is busy, and record the reported CPU percentage, the host CPU model, the libx264 version (`ffmpeg -version`) and the commit. The figure is process wide, so it includes encoder helper threads; divide by the core count for a per-machine share. Status: requires measurement on the target host; no figures are recorded in this repository.

## Rate control and picture quality

The picture is average bit rate controlled at `encode.bitrate_kbps`. When the bit rate is too low for the content, libx264 raises the quantizer (the QP in its log) and the picture shows blocking and smearing, most visibly on large, fast moving areas such as a moving colour bar; static or slowly changing content such as a counter stays clean at the same bit rate. An ffmpeg default encode of a 1280x720, 25 fps clip is about 250 kbit/s, which is far below what fast motion needs. At 4000 kbit/s the same content is encoded at a much lower quantizer (see the table).

- `bitrate_kbps` sets the average size: about `bitrate_kbps * 1000 / 8 / fps` bytes per frame (20000 bytes per frame at 4000 kbit/s and 25 fps). Raise it when fast motion looks blocky.
- `vbv_buffer_ms` bounds the burst size. A keyframe holds a whole picture, so without a cap it is the largest frame; with the cap it is limited to roughly the buffer size. A smaller buffer lowers the keyframe size and the number of reports per frame but lowers quality on keyframes and on sudden motion.
- `keyframe_interval` sets how often a keyframe is sent; a larger value means fewer keyframe bursts and slower recovery after a lost frame. A keyframe is also sent after a failed write and after the session is armed again.
- `preset` trades CPU for quality at one bit rate. `tune_zerolatency` removes lookahead and frame delay and should stay on.

Encoding is single threaded, one slice per frame, no B-frames, so one frame goes out as one message with no reordering delay. The video loop writes every report of a message back to back; capture is paced against fixed deadlines by the backend. A static screen produces very small inter-frames.

### Measured frame sizes

`screendockd bench` prints the size distribution of the encoded frames (keyframes and inter-frames separately, with the largest size in reports of 4080 payload bytes), the bit rate over the measured wall time and the CPU use. The figures below are from a 12 second run per row at 1280x720 with a 25 fps capture target, on an Xvfb screen showing ffplay with `testsrc2` plus a hue sweep (a busy, fast changing picture that is harder to encode than a typical desktop), with the preset `ultrafast` except where stated, on a host that also ran the pattern generator. Sizes are bytes.

| bitrate_kbps | vbv_buffer_ms | keyframe median / max | inter-frame median / p95 / max | max reports | measured kbit/s | avg QP (I / P) | CPU, one core |
|---|---|---|---|---|---|---|---|
| 250 | 200 | 3434 / 3925 | 865 / 2051 / 3859 | 1 | 183 | 51 / 50 | 38.7% |
| 1000 | 200 | 9608 / 14169 | 5481 / 7509 / 8670 | 4 | 922 | 42 / 46 | 41.1% |
| 2000 | 100 | 15903 / 19212 | 9609 / 12575 / 14740 | 5 | 1899 | 35 / 36 | 42.6% |
| 4000 | off | 33839 / 46538 | 20970 / 24509 / 30370 | 12 | 3950 | 24 / 28 | 42.9% |
| 4000 | 100 | 29675 / 37854 | 19995 / 25610 / 29619 | 10 | 3854 | 26 / 27 | 43.5% |
| 4000 | 50 | 20715 / 22782 | 15919 / 19557 / 21168 | 6 | 3098 | 31 / 30 | 43.3% |
| 6000 | 100 | 44950 / 57414 | 29949 / 41541 / 52956 | 15 | 5491 | 19 / 22 | 43.5% |
| 4000, `veryfast` | 200 | 30935 / 36737 | 21152 / 25154 / 27663 | 10 | 3492 | 10 / 14 | 71.3% |

Observations from these runs: a target of 250 to 1000 kbit/s on this content gives a quantizer of 42 to 51 (heavy blocking), while 4000 kbit/s gives about 27. At 4000 kbit/s a 100 ms buffer cut the largest keyframe from 46538 to 37854 bytes and a 50 ms buffer to 22782 bytes. `veryfast` reached a much lower quantizer at the same target (the encoder ran under its target rate) for about 1.6 times the CPU, and the loop reached 21.7 frames per second instead of 24 on this host, so a slower preset needs a machine with CPU headroom. The achieved frame rate in these runs was 23 to 24.5 fps because capture, scaling and encoding run on one thread on a busy host; the encode call took about 23 ms per frame at `ultrafast`.
