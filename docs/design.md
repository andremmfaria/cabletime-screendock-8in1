# Design

## What it is

`screendockd` is an independent, unofficial Linux userspace daemon for the CABLETIME ScreenDock (CB09W, USB 1de1:f201). The dock's built-in 5.5 inch 1280x720 panel receives H.264 video through a vendor HID interface. The daemon captures a desktop output, encodes it to H.264 with libx264, frames it for the dock's wire protocol and writes it to the MI_01 hidraw node, while a heartbeat and the session handshake run on MI_00. It is not a kernel driver: `hid-generic` already exposes both interfaces as `/dev/hidrawN` nodes, and `screendockd` is one native Rust program that owns capture, encoding, framing and both HID channels.

The wire facts come from the full protocol specification and the Linux driver feasibility study in [research/](research/). The implementation is written from the prose of those documents only, and no vendor code, decompiled output or vendor binary is part of the repository. The project is licensed under the MIT license (see `LICENSE`), and the README states that it is not affiliated with or endorsed by CableTime.

## Scope

Supported:

- Discovery of the MI_00 and MI_01 hidraw nodes by sysfs interface number and vendor and product ID, so `/dev/hidrawN` numbering does not matter.
- The MI_00 session: heartbeat, reader for the dock's messages, handshake, silent-dock arming, loss detection and the dock cycle for a size change. See [protocol-implementation.md](protocol-implementation.md).
- Capture from X11 (XShm) and, with the `capture-wayland` feature, from Wayland through the desktop portal and PipeWire. See [capture.md](capture.md).
- H.264 Constrained Baseline encoding without B-frames through libx264 (feature `encode-ffmpeg`), keyframe bundles of SPS, PPS, SEI and IDR as one message, single-NAL inter-frames. See [encoding.md](encoding.md).
- A configurable picture size with the width and height carried in every video header. See [daemon.md](daemon.md#resolution).
- Mirror mode and extend mode, where the daemon creates a monitor for the dock on X11 (RandR), GNOME (Mutter), sway (headless output) or through the portal. See [virtual-display.md](virtual-display.md).
- A control socket and the `ctl` command to read and change settings while the daemon runs. See [control.md](control.md).
- Packages (`.deb` and `.rpm`), a udev rule, a systemd user unit and a template config. See [install.md](install.md).

Not covered:

- Touch and digitizer input, and any input injection path.
- The audio interface (MI_02) and the mass storage interface (MI_04), which Linux's stock class drivers handle.
- The `reset` JSON-RPC method, which makes the dock re-enumerate USB and is never sent, and the optional `app_tx_mouse_coordinate` message.
- Any resolution negotiation: the dock has none, so the daemon picks a size and states it in every video header.

## Architecture

The architecture is described in [architecture.md](architecture.md): one process, a video loop (capture, encode, frame, write) on the calling thread, a dock session on MI_00 (heartbeat, reader and controller threads), and a supervisor thread that owns the live settings and serves the control socket. The crate is layered `utils`, `protocol`, `ports`, `core`, `adapters`, `app`, `cli`, with `tools` and `testing` as satellites. The import rules of each layer are in the layering table of that page.

Everything lives in one crate, `src/screendockd`, with a library and a thin binary. The repository layout:

```text
config/screendock_cfg.toml   default config, also packaged to /etc/screendockd/
deploy/                      udev rule, systemd user unit, package check, size comparison script
docs/                        project pages and research/
src/screendockd/             the crate (src/, tests/)
.github/workflows/           CI, tag release, packaging
```

## Design patterns

Each pattern below is applied in the code as follows.

### 1. Traits as ports, implementations as adapters

The traits in `ports/types.rs` are the contracts the daemon depends on: `CaptureBackend` (`next_frame`, optional `retarget`), `Encoder`, `DockTransport`, `EdidStore`, `ControlClient`, `VirtualDisplay`, `VirtualMonitor` and `DisplayProvider`. The implementations are in `adapters/`: `X11ShmCapture` (`adapters/capture/x11_shm.rs`) and `WaylandPortalCapture` (`adapters/capture/wayland_portal.rs`), `FfmpegEncoder` and `PrerecordedEncoder` (`adapters/encode/`), `HidrawHandle` (`adapters/transport/hidraw.rs`), `FileEdidStore` and the display providers in `adapters/display/`. The choice is made once at startup by `adapters::capture::create_backend` (session type detection with the configured override) and `adapters::encode::create_encoder`, which return boxed trait objects, so encode, framing and the daemon never know which backend is active. `CaptureBackend` is object safe, and a test pins that.

### 2. Newtypes for domain values

Values with distinct meaning have distinct types: `Mi00Path` and `Mi01Path` in `adapters/transport/types.rs` (the control and video paths cannot be swapped), `PixelWidth`, `PixelHeight` and `PictureSize` in `utils/types.rs`, `SessionId` in `protocol`, `OutputId` and `MonitorName` in `ports`. `ReportId` is a closed enum (`Control`, `Video`) in `protocol/types.rs`, so no raw report byte crosses an API.

### 3. Builder for message construction

`MessageBuilder` in `protocol/framing.rs` has `video(PictureSize)` and `control()` entry points that fill in the Report ID, width and height, a `payload(&[u8])` setter and `build()`, which calls the pure `frame_message`. Heartbeat, host messages and video all build their reports this way, so no caller writes header fields by hand.

### 4. Typestate for staged transactions

`core/reconfig/transaction.rs` models a live settings change as `Reconfig<Planned>`, `Reconfig<Prepared>`, `Reconfig<Committed>` and `Reconfig<Verified>`. Each step consumes the transaction, so a stage cannot be skipped, and two `compile_fail` doc tests show it. A committed transaction dropped without verification restores the previous settings. The pure functions in `protocol::framing` (`frame_message`, `parse_chunks`) and the session's `SessionState` (an enum checked at run time because the dock drives its changes) do not use typestate.

### 5. Parse at the boundary, named constants

`FrameHeader::from_bytes` returns a `Result` and folds the zero padding checks into producing the value. `HidrawHandle::read_message` returns a `ParsedMessage`, and `InboundAssembler` and `parse_dock_message` turn the dock's bytes into `DockMessage` values, so raw buffers do not leave the I/O layer. Wire sizes are named constants (`REPORT_SIZE`, `HEADER_SIZE`, `FIRST_CHUNK_PAYLOAD_MAX`, `CONTINUATION_CHUNK_PAYLOAD_MAX`, `MAX_REQUEST_LINE`, `EDID_BLOCK_LEN`) in the `types.rs` of their directory.

### 6. RAII ownership of OS resources

`discovery.rs` only resolves paths and opens nothing. `HidrawHandle` owns a `std::fs::File`, so the descriptor closes on drop, and no method exposes the raw fd; every write is a method on the handle. Other resources are released on drop the same way: `ControlServer` removes its socket, the display monitors (`MutterMonitor`, `WlrootsMonitor`, `X11MonitorGuard`) remove their monitor, `Session` stops its threads, and `X11ShmCapture` frees its shared memory.

### 7. Typed errors in libraries, `anyhow` at the boundary

Each directory declares its error enums with `thiserror` in `types.rs` (`FramingError`, `DiscoveryError`, `CaptureError`, `EncodeError`, `TransportError`, `SessionError`, `ConfigError` and others). `cli` is the application boundary and uses `anyhow` with context, so a failure prints a readable chain and exits nonzero. Startup is fail fast: configuration, discovery, opening nodes, backend, encoder and the first heartbeat each end the process on failure. After startup a failed capture, encode or write is logged and counted, and the loop continues; see [daemon.md](daemon.md#errors-after-startup).

### 8. Mechanism and policy separation

Mechanism code moves bytes and calls the system; policy code decides what they mean and never does I/O. `adapters::transport::hidraw` writes and reads framed buffers and checks only the channel's Report ID, while `protocol::framing`, `protocol::heartbeat` and `protocol::messages` define the content as pure functions. `adapters::encode::messages_from_packet` decides which NAL units form a message and `adapters::encode::ffmpeg` only runs libx264. `core::display_policy::decide` picks the virtual monitor mechanism from `DisplayFacts` as a pure function, and `adapters::display` gathers the facts and performs the calls. `core::reconfig::plan_change` classifies a settings change purely, and `app::supervisor` applies it.

### 9. Configuration and logging in `utils`

`utils/config.rs` loads one TOML file with `serde` and `toml`. The path is the first existing of `SCREENDOCK_CONFIG_PATH`, `$XDG_CONFIG_HOME/screendockd/screendock_cfg.toml` (through the `dirs` crate), `/etc/screendockd/screendock_cfg.toml` and `config/screendock_cfg.toml` in the repository; see [configuration.md](configuration.md). `utils/logging.rs` initializes `tracing` and `tracing-subscriber` with the level from `daemon.log_level`, overridden by `RUST_LOG`. Spans such as `video_loop` mark which loop emitted a line, since several loops run at once.

### Conventions that follow from these

- Every source directory has a `types.rs` that holds its structs, enums, traits, constants and error enums; logic files re-export the public types at their usual paths.
- `mod.rs` files declare submodules and use no glob re-exports.
- A message with two kinds (control and video) is handled by a `match` on `ReportId`. There is no visitor, no plugin registry for capture backends (the backends are compiled in by feature and chosen in `create_backend`) and no abstract factory.

## Technology

| Area | What is used |
|---|---|
| Language | Rust, edition 2021, one crate with a library and a binary |
| hidraw access | Plain `write(2)` and `read(2)` through `std::fs::File`, `poll(2)` for readability, no hidapi and no ioctl |
| H.264 | `ffmpeg-next` driving libavcodec's libx264 (feature `encode-ffmpeg`), Constrained Baseline, no B-frames, `ultrafast` preset and `zerolatency` tune by default |
| X11 capture | `x11rb` (pure Rust) with the MIT-SHM and RandR extensions, `memmap2` and `rustix` for the shared segment |
| Wayland capture | `ashpd` (xdg-desktop-portal ScreenCast) and `pipewire` (feature `capture-wayland`), `zbus` for `org.gnome.Mutter.ScreenCast`, `tokio` for the portal calls |
| Config | `serde` and `toml`, `dirs` for XDG paths |
| Logging | `tracing` and `tracing-subscriber` with the env filter |
| JSON | `serde_json` for the heartbeat, dock messages and the control wire |
| Command line | `clap` (derive), `anyhow` at the boundary, `thiserror` in libraries |
| Signals and CPU time | `signal-hook`, `libc` (`getrusage`) |
| Images | `png` for the `capture` command |
| Packaging | `cargo-deb`, `cargo-generate-rpm`, systemd user unit, udev rule `deploy/99-screendock.rules` |
| CI | GitHub Actions: `ci.yml`, `tag-release.yml` (tags the version and calls `package.yml`), `package.yml` |

Cargo features: `tools` (default), `testing`, `capture-wayland` and `encode-ffmpeg`; see [architecture.md](architecture.md#cargo-features).

## Validation status

- Automated: unit tests, integration tests against a simulated dock, regular files, pipes, Xvfb, Xorg with the dummy driver, headless sway and headless GNOME Shell, and libx264 output decoded with the `ffmpeg` tool. See [testing.md](testing.md).
- On the dock: partially validated. The procedures and the status of each item are in [hardware-validation.md](hardware-validation.md).
- The size comparison script `deploy/compare_sizes.sh` measures the sent picture against the captured screen without the dock; see [testing.md](testing.md#picture-size-comparison).

## Known limits

- How the panel treats sizes other than 1280x720 is not measured; the header carries the configured size.
- Mirror mode scales the whole captured source into the encoded size.
- Wayland capture through the portal needs a user session and its share prompt, which no automated test covers.
- The libx264 encoder does not build on hosts without the libav development files.
- A dock that holds a session from an earlier host process is silent on MI_00; the daemon handles it by sending the full handshake sequence without waiting for replies, so a missing reply is not an error.
- Video sent before the handshake completes is discarded by the dock, so the daemon holds it back until the session is armed.
