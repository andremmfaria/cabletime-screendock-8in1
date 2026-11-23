# Daemon

`screendockd run` is the daemon: it opens the dock's two hidraw nodes, sends heartbeats on MI_00, and captures, encodes, frames and writes video to MI_01. Source: `src/screendockd/src/daemon.rs` and the `run` subcommand in `src/main.rs`.

Status: requires on-device validation. The loop is tested against regular files standing in for the nodes, a real Xvfb display and the prerecorded test encoder. See [hardware-validation.md](hardware-validation.md) for the on-device procedure.

## Usage

```text
screendockd [--config PATH] run [--mi00 PATH] [--mi01 PATH] [--h264-file PATH]
                                [--backend auto|x11|wayland] [--max-frames N]
```

Without `--h264-file` the libx264 encoder is used, which needs a build with `--features encode-ffmpeg`. With it the prerecorded test encoder replays that Annex-B file. `--max-frames` stops after N frames; without it the daemon runs until SIGINT or SIGTERM. `--mi00` and `--mi01` replace sysfs discovery for that node.

## Startup

Startup is fail fast. In this order, any failure prints the error chain and exits nonzero before any frame is sent:

1. Load the config and validate `[display]` and `[device]`.
2. Resolve MI_00 and MI_01 (discovery with the configured vendor and product IDs, unless both nodes are given on the command line).
3. Open both nodes.
4. Create the capture backend.
5. Create the encoder.
6. Write the first heartbeat (tick 1) to MI_00.

## Loops

- Heartbeat thread: sends tick 2 and later on MI_00 every `daemon.heartbeat_interval_ms`, scheduled against fixed deadlines so write time does not accumulate as drift. Its tracing span is `heartbeat_loop`.
- Video loop, on the main thread, span `video_loop`: `capture.next_frame()` (paced by the backend at `capture.fps`), `encoder.encode()`, framing each message with `MessageBuilder::video` (field B from the display geometry), then one `write(2)` per chunk back to back.

## Errors after startup

Failures in the loop are per frame and recoverable. A failed capture, encode or write is logged at warn level and counted, the frame or message is dropped, and the loop continues. After an encode failure or a failed write the encoder is asked for a keyframe so the panel can resynchronize. A failed heartbeat write is logged and retried at the next tick.

After 100 consecutive failed frames the daemon exits with an error, so a supervisor restarts it instead of the daemon spinning against a dead device. A successful frame resets the count. A progress line (frames, messages, write errors) is logged every 5 seconds at info level.

## Shutdown

SIGINT and SIGTERM set a shared flag. The video loop checks it every frame and the heartbeat thread every 50 ms. The loop finishes the frame in flight, the heartbeat thread stops, handles close on drop and the process exits with status 0. A summary of counters (frames, messages, keyframes, errors by kind, heartbeats) is printed on exit.

## Resolution

`display::DisplayGeometry` is built from `display.width`, `display.height` and `display.orientation`. These are the encoded picture size exactly as written; orientation is a consistency check, not a rotation.

| Rule | Value |
|---|---|
| Edge length | even, 16 to 4096 pixels |
| `landscape` | width at least height |
| `portrait` | height at least width |
| Header field B | `width * 256` |

| Size | Orientation | Field B |
|---|---|---|
| 1280x720 | landscape | 327680 |
| 900x600 | landscape | 230400 |
| 720x1280 | portrait | 184320 |

The same geometry sets the libx264 output size (the capture is scaled to it by the encoder) and the field B written into every video message header. The Wayland backend receives it as its preferred stream size. Invalid values are a startup error.

## Device section

`device.vendor_id_hex` and `device.product_id_hex` are the vendor and product IDs discovery matches against sysfs. They accept four hex digits, case insensitive, with an optional `0x` prefix. They default to the dock's `1de1` and `f201`. A malformed value is a startup error. The `discover`, `replay`, `isolate` and `run` commands all use them.

## Tests

- Unit tests in `daemon.rs` use a synthetic capture backend, the prerecorded encoder and loopback files: message order and field B, heartbeat content, recovery from capture errors, giving up after too many, persistent write errors, a fatal startup heartbeat failure and rejected swapped handles.
- `tests/daemon_e2e.rs` runs the real binary against Xvfb with the prerecorded encoder and files for the nodes: it checks the video messages byte for byte against the source stream, the heartbeat, a clean exit on SIGTERM and on SIGINT, and a nonzero exit with no video written when a node cannot be opened.
