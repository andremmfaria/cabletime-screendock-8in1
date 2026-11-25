# Architecture

`screendockd` is one Rust crate (`src/screendockd`) with a library and a binary. The library holds every module; the binary (`src/main.rs`) is the command line entry point. The daemon is one process: a video loop for capture, encode and framing on the calling thread, and a dock session (heartbeat, MI_00 reader and handshake controller threads) on MI_00, all talking to the dock through hidraw.

## Data flow

```text
capture -> encode -> framing -> DockTransport (MI_01, Report ID 2)   [only while the session is armed]
core::session: heartbeat thread  -> framing -> DockTransport (MI_00, Report ID 1, writes)
               reader thread     <- DockTransport (MI_00, reads the dock's JSON, answers the EDID passthrough)
               controller thread    handshake, silent-dock arming (`AssumedActive`), loss detection, redo
adapters::transport::discovery resolves the two /dev/hidrawN paths once at startup
adapters::transport::hidraw implements DockTransport on those nodes
```

Session arming rule: video is allowed in `Active` (every reply seen) and in `AssumedActive`. `AssumedActive` is entered when the ready wait or a reply wait times out, including when the dock sent no MI_00 message at all; the full handshake message sequence is sent in that case too, because a silent dock still processes it. Heartbeats continue and the reader keeps draining while armed. A later ready announcement, repeated heartbeat write failures or repeated video write failures send the controller back through the full handshake.

The dock exposes two HID interfaces under one VID/PID. MI_00 is the JSON-RPC control channel (Report ID 1) and MI_01 is the video channel (Report ID 2). `/dev/hidrawN` numbering is not stable, so nodes are resolved by sysfs interface number.

## Modules

| Module | Role | Status |
|---|---|---|
| `adapters::transport::discovery` | Resolves MI_00 and MI_01 hidraw paths from sysfs into the `Mi00Path` and `Mi01Path` newtypes, matching the vendor and product IDs in `DeviceIds` (the dock's by default, overridable from the `[device]` config) | Implemented, unit tested against a synthetic sysfs tree |
| `protocol::framing` | Pure functions and types for the 16-byte header, chunking and parsing: `ReportId`, `PixelWidth`, `PixelHeight`, `PictureSize`, `FrameHeader`, `MessageBuilder`, `frame_message`, `parse_chunks` | Implemented, unit and integration tested |
| `ports::transport` | `DockTransport` trait (channel, `write_chunks`, `read_report`, `read_message`, `wait_readable`, `try_clone`), `TransportError` and `WriteTimings` | Implemented |
| `adapters::transport::hidraw` | `HidrawHandle`, the `DockTransport` implementation: owns one open node, writes chunks with `write(2)`, reads and parses messages, duplicates the descriptor (`try_clone`) and waits for readability with `poll(2)` (`wait_readable`) | Implemented, loopback tested. Requires on-device validation |
| `protocol::heartbeat` | The `SessionId` newtype and the exact heartbeat text | Implemented, unit tested |
| `tools::heartbeat_loop` | `run_heartbeat_loop`: a paced send loop on MI_00 used by the validation tools | Implemented, loopback tested |
| `protocol::messages` | Exact bytes of every host message on MI_00 other than the heartbeat | Implemented, unit tested |
| `protocol::inbound` | `InboundAssembler` and `parse_dock_message`: reassembly and classification of the dock's MI_00 messages | Implemented, unit tested |
| `core::session` | The MI_00 dock session over any `DockTransport`: `Session`, `SessionHandle`, `SessionState`, `SessionSettings` and the handshake state machine, split into `settings`, `state`, `events`, `reader`, `heartbeat` and `controller` modules. See [protocol-implementation.md](protocol-implementation.md#session-handshake) |
| `testing::fake_dock` | A simulated dock over Unix datagram socket pairs, used by session, daemon and validation tests. Not used by the daemon | Implemented, loopback tested. Requires on-device validation |
| `protocol::annexb` | Splits an Annex-B H.264 stream into dock messages (keyframe bundle, single inter-frame NALs) | Implemented, unit tested |
| `tools::validation` | Replay timing and heartbeat isolation routines used by the CLI. They run the dock session first when video is sent with an MI_00 handle | Implemented, loopback tested. Requires on-device validation |
| `config` | Loads the TOML config through a four step path resolution | Implemented |
| `telemetry::logging` | Initializes `tracing` from the config level or `RUST_LOG` | Implemented |
| `ports::encode` | `Encoder` trait, `VideoMessage`, `EncoderSettings` and `EncodeError`. See [encoding.md](encoding.md) | Implemented |
| `adapters::encode` | Message policy (`messages_from_packet`), `create_encoder`, libx264 encoder (`adapters::encode::ffmpeg`, feature `encode-ffmpeg`), prerecorded test encoder (`adapters::encode::prerecorded`). See [encoding.md](encoding.md) | Implemented. libx264 verified in CI against the ffmpeg decoder; not compilable on hosts without libav development files. Requires on-device validation |
| `protocol::display` | `DisplayGeometry`: validated width, height and orientation, header size | Implemented, unit tested |
| `core::policy` | Session timing defaults and polling constants | Implemented |
| `app::daemon` | The dock session plus the capture, encode, frame and write loop with the error policy; video is written only while the session is armed. See [daemon.md](daemon.md) | Implemented, tested against Xvfb and loopback files. Requires on-device validation |
| `tools::bench` | CPU time measurement of capture plus encode | Implemented, unit tested. Measurement on the target host is pending |
| `ports::capture` | `Frame` and `PixelFormat` types, `CaptureBackend` trait and `CaptureError`. See [capture.md](capture.md) | Implemented, unit tested |
| `adapters::capture` | `create_backend` and session detection, XShm X11 backend (`x11_shm`), portal and PipeWire Wayland backend (`wayland_portal`, feature `capture-wayland`) | Implemented. X11 verified against Xvfb. Wayland requires on-session validation. See [capture.md](capture.md) |

## Cargo features

| Feature | Default | Effect |
|---|---|---|
| `tools` | yes | Compiles the `tools` module and the `replay`, `isolate` and `bench` subcommands. Without it the library and the `discover`, `capture` and `run` commands still build, which shows that core, ports, adapters and app do not depend on the tools |
| `testing` | no | Compiles `testing::fake_dock`. The crate enables it for its own tests through a dev-dependency on itself, so `cargo test` always includes it and a normal or release build never does |
| `capture-wayland` | no | Wayland capture backend |
| `encode-ffmpeg` | no | libx264 encoder |

## Layering rules

- Ports are the traits and data types the daemon depends on (`ports::capture::CaptureBackend`, `ports::encode::Encoder`, `ports::transport::DockTransport`). The implementations live in `adapters` and are chosen once at startup by `adapters::capture::create_backend` and `adapters::encode::create_encoder`, so the daemon never names a concrete backend or encoder.
- Mechanism and policy are separate. `adapters::transport::hidraw` moves framed buffers and checks only that each chunk's Report ID matches the channel the handle was opened for. `protocol::framing`, `protocol::heartbeat` and `protocol::messages` define what messages mean and never touch a file descriptor.
- Domain values are newtypes: the two node paths cannot be swapped, a pixel width cannot be passed where a height is expected, and Report IDs are a closed enum.
- Library modules return typed `thiserror` enums. `main.rs` is the application boundary and uses `anyhow` with context, so failures print a readable chain and exit nonzero.
- Reads are parsed at the I/O boundary: `HidrawHandle::read_message` returns a `ParsedMessage`, never raw buffers.
- `HidrawHandle` owns a `std::fs::File`, so the descriptor closes on drop. No method exposes the raw fd.

## Command line

The binary provides `discover`, `replay`, `isolate`, `capture`, `run` and `bench`. `replay` and `isolate` are the hardware validation tools described in [hardware-validation.md](hardware-validation.md). `capture` grabs frames from the desktop and writes them as PNG files, and needs no dock; see [capture.md](capture.md). `run` is the daemon, see [daemon.md](daemon.md). `bench` measures capture plus encode CPU time, see [encoding.md](encoding.md#cpu-measurement).

## Tests

`cargo test` in `src/screendockd` runs unit tests and the integration tests in `tests/` (framing, X11 capture, daemon end to end, daemon against a simulated dock; with `--features encode-ffmpeg` also the libx264 encoder). Hardware access is replaced by a regular file, pipe or socket through `HidrawHandle::from_file`, and the dock's conversation by `fake_dock`. No default test needs a dock or a hidraw node. The X11 capture and daemon tests start their own Xvfb and skip when Xvfb is not installed. See [testing.md](testing.md).
