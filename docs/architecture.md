# Architecture

`screendockd` is one Rust crate (`src/screendockd`) with a library and a binary. The library holds every module; the binary (`src/main.rs`) is the command line entry point. The daemon is one process with independent loops for capture, encode and heartbeat, all writing to the dock through hidraw.

## Data flow

```text
capture -> encode -> framing -> hidraw_io (MI_01, Report ID 2)
heartbeat -----------> framing -> hidraw_io (MI_00, Report ID 1)
discovery resolves the two /dev/hidrawN paths once at startup
```

The dock exposes two HID interfaces under one VID/PID. MI_00 is the JSON-RPC control channel (Report ID 1) and MI_01 is the video channel (Report ID 2). `/dev/hidrawN` numbering is not stable, so nodes are resolved by sysfs interface number.

## Modules

| Module | Role | Status |
|---|---|---|
| `discovery` | Resolves MI_00 and MI_01 hidraw paths from sysfs into the `Mi00Path` and `Mi01Path` newtypes | Implemented, unit tested against a synthetic sysfs tree |
| `framing` | Pure functions and types for the 16-byte header, chunking and parsing: `ReportId`, `PixelWidth`, `FieldB`, `FrameHeader`, `MessageBuilder`, `frame_message`, `parse_chunks` | Implemented, unit and integration tested |
| `hidraw_io` | `HidrawHandle`: owns one open node, writes chunks with `write(2)`, reads and parses messages | Implemented, loopback tested. Requires on-device validation |
| `heartbeat` | Builds the JSON-RPC heartbeat message and runs a paced send loop on MI_00 | Implemented, loopback tested. Requires on-device validation |
| `annexb` | Splits an Annex-B H.264 stream into dock messages (keyframe bundle, single inter-frame NALs) | Implemented, unit tested |
| `validation` | Replay timing and heartbeat isolation routines used by the CLI | Implemented, loopback tested. Requires on-device validation |
| `utils::config` | Loads the TOML config through a four step path resolution | Implemented |
| `utils::logging` | Initializes `tracing` from the config level or `RUST_LOG` | Implemented |
| `encode` | H.264 encoder wrapper | Stub |
| `capture` | `CaptureBackend` trait, X11 and Wayland backends | Stub |

## Layering rules

- Mechanism and policy are separate. `hidraw_io` moves framed buffers and checks only that each chunk's Report ID matches the channel the handle was opened for. `framing` and `heartbeat` define what messages mean and never touch a file descriptor.
- Domain values are newtypes: the two node paths cannot be swapped, a pixel width cannot be passed where a pre-multiplied field B is expected, and Report IDs are a closed enum.
- Library modules return typed `thiserror` enums. `main.rs` is the application boundary and uses `anyhow` with context, so failures print a readable chain and exit nonzero.
- Reads are parsed at the I/O boundary: `HidrawHandle::read_message` returns a `ParsedMessage`, never raw buffers.
- `HidrawHandle` owns a `std::fs::File`, so the descriptor closes on drop. No method exposes the raw fd.

## Command line

The binary currently provides `discover`, `replay` and `isolate`. `replay` and `isolate` are the hardware validation tools described in [hardware-validation.md](hardware-validation.md). The capture and encode daemon is not wired yet.

## Tests

`cargo test` in `src/screendockd` runs unit tests and the `tests/framing.rs` integration tests. Hardware access is replaced by a regular file or pipe through `HidrawHandle::from_file`. No test needs a dock, a hidraw node or a display.
