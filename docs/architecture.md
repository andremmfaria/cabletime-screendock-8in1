# Architecture

`screendockd` is one Rust crate (`src/screendockd`) with a library and a binary. The library holds every module; the binary (`src/main.rs`) only calls `cli::run`, which holds the command line. The daemon is one process: a video loop for capture, encode and framing on the calling thread, and a dock session (heartbeat, MI_00 reader and handshake controller threads) on MI_00, all talking to the dock through hidraw.

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
| `cli` | Argument definitions (`args`), shared helpers (`support`) and one handler per subcommand (`discover`, `capture`, `run`, and with feature `tools` `replay`, `isolate`, `bench`). It is the composition root: it builds the adapters and passes them to the app, the core and the tools | Implemented |
| `utils::config` | Loads the TOML config through a four step path resolution | Implemented |
| `utils::logging` | Initializes `tracing` from the config level or `RUST_LOG` | Implemented |
| `utils::types` | The configuration structs and enums and `ConfigError`, plus the plain value types shared by several layers: `PixelWidth`, `PixelHeight` and `PictureSize` | Implemented |
| `ports::encode` | `Encoder` trait, `VideoMessage`, `EncoderSettings` and `EncodeError`. See [encoding.md](encoding.md) | Implemented |
| `adapters::encode` | Message policy (`messages_from_packet`), `create_encoder`, libx264 encoder (`adapters::encode::ffmpeg`, feature `encode-ffmpeg`), prerecorded test encoder (`adapters::encode::prerecorded`). See [encoding.md](encoding.md) | Implemented. libx264 verified in CI against the ffmpeg decoder; not compilable on hosts without libav development files. Requires on-device validation |
| `protocol::display` | `DisplayGeometry`: validated width, height and orientation, header size | Implemented, unit tested |
| `core::policy` | Session timing defaults and polling constants | Implemented |
| `protocol::control` | The control wire format: `ControlRequest` and `ControlResponse` as one JSON object per line, with a 4096 byte request cap. See [control.md](control.md) | Implemented, unit tested |
| `ports::control` | `Command` (a request with its reply sender, or a reload), `ControlClient` and `ControlError` | Implemented |
| `adapters::control` | `ControlServer` (the 0600 Unix socket and its accept thread, removed on drop) and `UnixControlClient`; `socket_path` resolves the path from the config | Implemented, unit tested, tested against the daemon binary |
| `core::reconfig::keys` | The names of the live settings on the control interface: `SettingKey`, `setting_values`, `changed_keys` and `answer_read`, which answers `status` and `get` | Implemented, unit tested |
| `core::display_policy` | Pure display policy: `decide` chooses mirror or a virtual monitor mechanism (`X11Randr` with its tier, `Mutter`, `Portal`, `Wlroots`) from `DisplayFacts` and the `[virtual_display]` section, `virtual_support` reports `Supported`, `Unsupported` with a reason or `Unknown`, and `place_monitor` computes the monitor rectangle from the placement, anchor and size | Implemented, unit tested |
| `ports::display` | `DisplayFacts` (display server, desktop, outputs, RandR version, spare output, Mutter ScreenCast, portal source types, sway socket), `OutputFacts`, `OutputId` and `Rect` | Implemented, unit tested |
| `core::reconfig` | The live settings model: `LiveSettings` (every setting that can change while the daemon runs), `SettingsPatch` (a validated partial change), `ChangeClass`, `ReconfigAction`, `ReconfigPlan` and the pure `plan_change`. See [Live settings and change classes](#live-settings-and-change-classes) | Implemented, unit tested |
| `app::daemon` | The dock session plus the capture, encode, frame and write loop with the error policy; video is written only while the session is armed. See [daemon.md](daemon.md) | Implemented, tested against Xvfb and loopback files. Requires on-device validation |
| `tools::bench` | CPU time measurement of capture plus encode | Implemented, unit tested. Measurement on the target host is pending |
| `ports::capture` | `Frame` and `PixelFormat` types, `CaptureBackend` trait and `CaptureError`. See [capture.md](capture.md) | Implemented, unit tested |
| `adapters::capture` | `create_backend` and session detection, XShm X11 backend (`x11_shm`), portal and PipeWire Wayland backend (`wayland_portal`, feature `capture-wayland`) | Implemented. X11 verified against Xvfb. Wayland requires on-session validation. See [capture.md](capture.md) |

## Types per directory

Every source directory has a `types.rs` that holds the declarations of that directory: structs, enums, traits (the port interfaces), type aliases, constants, `thiserror` error enums and their trivial `Default`, `From`, `Deref` and `AsRef` implementations. Behaviour stays in the logic files of the directory: inherent `impl` blocks with logic, functions, state machines and threads. A logic file re-exports the public types it used to declare, so `protocol::framing::ReportId`, `ports::capture::Frame` and every other public path are unchanged. Fields that only the logic files of the same directory touch are `pub(super)` in `types.rs`. Types are placed in the lowest layer that can hold them: `utils/types.rs` holds the newtypes and the picture size that `protocol`, `core`, `adapters`, `tools` and `cli` all use, and `protocol::framing` re-exports them.

| Directory | `types.rs` holds |
|---|---|
| `utils` | `Config` and its section structs, `CaptureBackendChoice`, `X264Preset`, `DisplayMode`, `Placement`, `WaylandBackend`, `FallbackPolicy`, `EdidSource`, `VirtualDisplayConfig`, `ControlConfig`, `ConfigError`, the config path constants, the logging environment variable name, `PixelWidth`, `PixelHeight`, `PictureSize` |
| `protocol` | `ControlRequest`, `ControlResponse`, `ControlWireError`, the control line caps, `ReportId`, `FrameHeader`, `ParsedMessage`, `MessageBuilder`, `FramingError`, the report size constants, `AnnexBError`, the NAL type constants, `Orientation`, `DisplayGeometry`, `DisplayError`, `SessionId`, `SessionIdError`, `DockMessage`, `InboundAssembler`, the host message constants |
| `ports` | `Command`, `ControlClient`, `ControlError`, `OutputId`, `Rect`, `DisplayServer`, `OutputFacts`, `DisplayFacts`, `Frame`, `PixelFormat`, `CaptureBackend`, `CaptureError`, `Encoder`, `VideoMessage`, `EncoderSettings`, `EncodeError`, `DockTransport`, `TransportError`, `WriteTimings` |
| `core` | the session timing defaults and polling constants, and the display policy types `DisplayDecision`, `ExtendBackend`, `X11Tier`, `VirtualSupport` and `DisplayPolicyError` |
| `core::reconfig` | `SettingKey`, `LiveSettings`, `SettingsPatch`, `SettingsError`, `ChangeClass`, `ReconfigAction`, `ReconfigPlan`, `PlanOptions` and the range limits |
| `core::session` | `Session`, `SessionState`, `SessionHandle`, `SessionStats`, `SessionSettings`, `SessionError`, the shared state, the controller and reader event types |
| `adapters` | nothing of its own; the subdirectories hold the adapter types |
| `adapters::control` | `ControlServer`, `UnixControlClient`, the socket path, timeout and size constants |
| `adapters::capture` | `SessionType`, `BackendKind`, `X11ShmCapture`, `WaylandPortalCapture` and the Wayland stream state |
| `adapters::encode` | `EncoderKind`, `FfmpegEncoder`, `PrerecordedEncoder` |
| `adapters::transport` | `HidrawHandle`, `Mi00Path`, `Mi01Path`, `DeviceIds`, `DockHidrawPaths`, `DiscoveryError`, the discovery constants |
| `app` | `DaemonParams`, `DaemonReport`, `DaemonError`, the daemon constants |
| `cli` | the clap argument structs and enums and the parsed command |
| `tools` | the replay, isolate, heartbeat and bench parameter, report and statistics types and their errors |
| `testing` | `FakeDock`, `FakeDockConfig`, `StartedReply`, `VideoStats` and the fake dock state |

## Live settings and change classes

`core::reconfig::LiveSettings` holds the validated values that can change while the daemon runs: picture size and orientation, frame rate, preset, tune, bit rate, VBV buffer, keyframe interval, display mode, monitor placement, anchor, position, name, Wayland backend and the follow setting. `LiveSettings::try_from(&Config)` builds it from the config, and `SettingsPatch::apply_to` yields a new validated value from a partial change without touching the old one. `SettingsPatch::between` turns two settings into the patch of their differences.

`plan_change(old, new)` is pure and returns a `ReconfigPlan`: a `ChangeClass` and the ordered `ReconfigAction` list. The class is the highest class among the changed settings.

| Change | Class | Actions |
|---|---|---|
| bit rate, VBV buffer | live when the encoder changes its rate in place, else rebuild | `SetRate`, or `RebuildEncoder` |
| keyframe interval | live | `SetKeyframeInterval` |
| frame rate, preset, tune | rebuild | `RebuildEncoder` |
| width, height, orientation | dock cycle | `RebuildEncoder`, `RetargetCapture` and `ResizeMonitor` in extend mode, `RunDockCycle` |
| mirror to extend or back | rebuild, dock cycle when the picture size changes too | `CreateMonitor` or `DestroyMonitor`, `RebuildEncoder`, `RetargetCapture` |
| placement, anchor, position in extend mode | live | `RetargetCapture` |
| monitor name or Wayland backend in extend mode | rebuild | `RecreateMonitor`, `RetargetCapture`, `RebuildEncoder` |
| follow system changes | live | `SetFollow` |

Placement, name and backend changes in mirror mode need no action; the new values are kept for the next switch to extend. A plan with no actions is a no-op.

## Cargo features

| Feature | Default | Effect |
|---|---|---|
| `tools` | yes | Compiles the `tools` module and the `replay`, `isolate` and `bench` subcommands. Without it the library and the `discover`, `capture` and `run` commands still build, which shows that core, ports, adapters and app do not depend on the tools |
| `testing` | no | Compiles `testing::fake_dock`. The crate enables it for its own tests through a dev-dependency on itself, so `cargo test` always includes it and a normal or release build never does |
| `capture-wayland` | no | Wayland capture backend |
| `encode-ffmpeg` | no | libx264 encoder |

## Layering rules

Layers, lowest first: `utils`, `protocol`, `ports`, `core`, `adapters`, `app`, `cli`. `utils` is the cross-cutting base layer: configuration loading, logging setup and the plain value types shared by several layers (`utils::types`), no domain logic, and it imports nothing else from the crate. `tools` and `testing` are satellites.

| Layer | May import | Must not import |
|---|---|---|
| `utils` | std and external crates | any crate module |
| `protocol` | `utils::config` (only `protocol::display`), `utils::types` | ports, core, adapters, app, cli, tools, testing |
| `ports` | protocol, utils | core, adapters, app, cli, tools, testing |
| `core` | protocol, ports, utils | adapters, app, cli, tools, testing (outside unit tests) |
| `adapters` | ports, protocol, utils | core, app, cli |
| `app` | core, ports, protocol, utils | adapters, cli (outside unit tests) |
| `tools` | core, ports, protocol, utils | adapters (outside unit tests) |
| `testing` | adapters, ports, protocol | core, app, cli |
| `cli` | everything | nothing |

`mod.rs` files declare submodules and use no glob re-exports. Every directory declares `pub mod types;`, and the modules re-export their public types from `types` at the paths they have always had.

- Ports are the traits and data types the daemon depends on (`ports::capture::CaptureBackend`, `ports::encode::Encoder`, `ports::transport::DockTransport`). The implementations live in `adapters` and are chosen once at startup by `adapters::capture::create_backend` and `adapters::encode::create_encoder`, so the daemon never names a concrete backend or encoder.
- Mechanism and policy are separate. `adapters::transport::hidraw` moves framed buffers and checks only that each chunk's Report ID matches the channel the handle was opened for. `protocol::framing`, `protocol::heartbeat` and `protocol::messages` define what messages mean and never touch a file descriptor.
- Domain values are newtypes: the two node paths cannot be swapped, a pixel width cannot be passed where a height is expected, and Report IDs are a closed enum.
- Library modules return typed `thiserror` enums. `cli` is the application boundary and uses `anyhow` with context, so failures print a readable chain and exit nonzero.
- Reads are parsed at the I/O boundary: `HidrawHandle::read_message` returns a `ParsedMessage`, never raw buffers.
- `HidrawHandle` owns a `std::fs::File`, so the descriptor closes on drop. No method exposes the raw fd.

## Command line

The command line lives in `src/screendockd/src/cli/`. The binary provides `discover`, `replay`, `isolate`, `capture`, `run`, `bench` and `ctl`. `replay` and `isolate` are the hardware validation tools described in [hardware-validation.md](hardware-validation.md). `capture` grabs frames from the desktop and writes them as PNG files, and needs no dock; see [capture.md](capture.md). `run` is the daemon, see [daemon.md](daemon.md). `bench` measures capture plus encode CPU time, see [encoding.md](encoding.md#cpu-measurement). `ctl` talks to a running daemon through its control socket, see [control.md](control.md).

## Tests

`cargo test` in `src/screendockd` runs unit tests and the integration tests in `tests/`, grouped by area: `protocol` (framing and the control wire format), `session` (the session against a simulated dock), `capture` (X11 capture), `e2e` (the daemon and its control socket end to end against Xvfb) and, with `--features encode-ffmpeg`, `encode` (the libx264 encoder). Shared Xvfb and X11 painting helpers are in `tests/support/`. Hardware access is replaced by a regular file, pipe or socket through `HidrawHandle::from_file`, and the dock's conversation by `testing::fake_dock`. No default test needs a dock or a hidraw node. The X11 capture and daemon tests start their own Xvfb and skip when Xvfb is not installed. See [testing.md](testing.md).
