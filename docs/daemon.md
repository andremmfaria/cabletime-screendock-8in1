# Daemon

`screendockd run` is the daemon: it opens the dock's two hidraw nodes, runs the MI_00 session (heartbeat, reader and handshake), and captures, encodes, frames and writes video to MI_01 once the session allows it. Source: `src/screendockd/src/app/daemon.rs` and the `run` subcommand in `src/cli/run.rs`.

Status: requires on-device validation. The loop is tested against regular files standing in for the nodes, a simulated dock, a real Xvfb display and the prerecorded test encoder. See [hardware-validation.md](hardware-validation.md) for the on-device procedure.

## Usage

```text
screendockd [--config PATH] run [--mi00 PATH] [--mi01 PATH] [--h264-file PATH]
                                [--backend auto|x11|wayland] [--max-frames N] [--no-handshake]
```

Without `--h264-file` the libx264 encoder is used, which needs a build with `--features encode-ffmpeg`. With it the prerecorded test encoder replays that Annex-B file. `--max-frames` stops after N frames; without it the daemon runs until SIGINT or SIGTERM. `--mi00` and `--mi01` replace sysfs discovery for that node. `--no-handshake` skips the handshake and the MI_00 reader and sends video at once; it is for regular files standing in for the nodes, since a real dock discards video until the handshake has run.

## Startup

Startup is fail fast. In this order, any failure prints the error chain and exits nonzero before any frame is sent:

1. Load the config and validate `[display]` and `[device]`.
2. Resolve MI_00 and MI_01 (discovery with the configured vendor and product IDs, unless both nodes are given on the command line).
3. Open both nodes.
4. Create the capture backend.
5. Create the encoder.
6. Write the first heartbeat to MI_00 and start the session threads.
7. Start the supervisor, the SIGHUP thread and the control socket.

## Loops

- Session threads, owned by `core::session::Session`: a heartbeat thread that sends one heartbeat on MI_00 every `daemon.heartbeat_interval_ms` against fixed deadlines, independent of video writes; a reader thread that drains and parses the dock's MI_00 reports and answers its EDID passthrough; and a controller thread that runs the handshake, redoes it when the session is lost and runs the dock cycle when the picture size changes. When the dock is silent (no ready announcement, no replies) the controller still sends `app-launched`, the cursor message, passthrough id 103 and `notify-projection-started`, waits the 0.8 s settle delay and arms video in `AssumedActive`. See [protocol-implementation.md](protocol-implementation.md#session-handshake).
- Video loop, on the main thread, span `video_loop`: `capture.next_frame()` (paced by the backend at `capture.fps`), `encoder.encode()`, framing each message with `MessageBuilder::video` (width and height from the display geometry), then one `write(2)` per report back to back. While the session is not armed the loop captures and writes nothing. When the session becomes armed again after a loss, the encoder is asked for a keyframe. A `KeyframeClock` asks for a keyframe on the first frame and then every `encode.keyframe_interval` frames, and every forced keyframe restarts its count. At the top of each iteration, armed or not, the loop applies the changes the supervisor published through `PipelineState` and reports the result back. The video header carries the picture size of the applied settings.
- Pipeline changes: a bit rate or keyframe interval change is applied to the running encoder and clock. A change of picture size, orientation, frame rate, preset or tune, or of the captured area, is applied in one piece by the video loop itself, which is the only writer of video, so nothing is written while it runs. The loop builds the new encoder through the `EncoderBuilder` held by `PipelineState` and encodes one black frame at the new size with it, then retargets capture when the area or the frame rate changed (a backend that cannot change its rate still serves a new encoder rate), then runs the dock cycle when the picture size changed, and only then swaps the encoder in, asks it for a keyframe and restarts the keyframe count. The first message of the new encoder is a keyframe bundle with SPS and PPS. If any step fails, the capture target is put back, the old encoder keeps running and the error goes to the supervisor; after a failed dock cycle the session redoes the handshake at the old size. The supervisor waits up to 15 seconds for such a change and 3 seconds for the others.
- Rollback and `Degraded`: the supervisor runs every change through the `Reconfig` transaction (prepare, commit, verify). Prepare builds and probes an encoder aside on the supervisor thread. Verify watches the session for 1 second after a rebuild or dock cycle and requires 3 written video messages (`PipelineState::note_written` counts them). A failure at commit or verify makes the pipeline apply the previous settings again, with a dock cycle at the previous size when the plan had one, and the supervisor keeps the previous settings. If that fails too, the supervisor becomes degraded: the settings stay at the last good ones, `ctl status` reports `state: degraded`, and the next `set` or `reload` first runs the dock cycle at the last good size before it goes on. The dock cycle at the previous size is also what brings the dock back after a refused size: the session returns to the previous size on its own and the forced cycle repeats it once.

## Supervisor and threads

Besides the session threads and the video loop, `run` starts these threads:

- `supervisor`: owns the live settings. It receives every request from the control socket and every reload, validates and classifies the change, and applies it through the pipeline or rejects it. See [control.md](control.md#changing-settings).
- `control-accept` and one short-lived `control-client` thread per connection: the control socket server, started when `control.enabled` is true and a socket path exists. A failure to bind the socket (another daemon serves it, or the path is not usable) is a startup error.
- `sighup`: turns SIGHUP into a reload command for the supervisor.

On shutdown the SIGHUP thread stops, the control socket closes and its file is removed, and the supervisor ends once no sender is left. The supervisor runs on its own thread, so a slow request never delays the video loop or the heartbeat.

## Errors after startup

Failures in the loop are per frame and recoverable. A failed capture, encode or write is logged at warn level and counted, the frame or message is dropped, and the loop continues. After an encode failure or a failed write the encoder is asked for a keyframe so the panel can resynchronize. A failed heartbeat write is logged and retried at the next tick. Three consecutive failed video writes are reported to the session, which redoes the handshake. If the MI_00 reader dies (read error or end of stream) the daemon exits with an error.

After 100 consecutive failed frames the daemon exits with an error, so a supervisor restarts it instead of the daemon spinning against a dead device. A successful frame resets the count. A progress line (frames, messages, write errors) is logged every 5 seconds at info level.

## Shutdown

SIGINT and SIGTERM set a shared flag. The video loop checks it every frame and the session threads every 50 ms. The loop finishes the frame in flight, the session threads stop, handles close on drop and the process exits with status 0. A summary of counters (frames, messages, keyframes, errors by kind, heartbeats, handshakes, session losses) is printed on exit.

## Resolution

`protocol::display::DisplayGeometry` is built from `display.width`, `display.height` and `display.orientation`. These are the encoded picture size exactly as written; orientation is a consistency check, not a rotation.

| Rule | Value |
|---|---|
| Edge length | even, 16 to 4096 pixels |
| `landscape` | width at least height |
| `portrait` | height at least width |
| Header width and height | the configured width and height, unchanged |

The same geometry sets the libx264 output size (the capture is scaled to it by the encoder) and the width and height written into the header of every video message. The Wayland backend receives it as its preferred stream size. Invalid values are a startup error.

## Device section

`device.vendor_id_hex` and `device.product_id_hex` are the vendor and product IDs discovery matches against sysfs. They accept four hex digits, case insensitive, with an optional `0x` prefix. They default to the dock's `1de1` and `f201`. A malformed value is a startup error. The `discover`, `replay`, `isolate` and `run` commands all use them.

## Tests

- Unit tests in `app/daemon.rs` use a synthetic capture backend, the prerecorded encoder and loopback files: message order and header size, heartbeat content, recovery from capture errors, giving up after too many, persistent write errors, a fatal startup heartbeat failure and rejected swapped handles. Session behavior is tested in `session.rs` and `tests/session/dock.rs` against a simulated dock: message bytes, handshake order, bounded waits, loss and redo, and that no video precedes the sink.
- `tests/e2e/daemon_xvfb.rs` runs the real binary against Xvfb with the prerecorded encoder and files for the nodes: it checks the video messages byte for byte against the source stream, the heartbeat, a clean exit on SIGTERM and on SIGINT, and a nonzero exit with no video written when a node cannot be opened.
