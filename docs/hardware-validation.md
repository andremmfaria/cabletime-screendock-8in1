# Hardware validation

Status: partially validated on a real dock; the procedures below still require a person with the dock attached. Loopback tests against files and pipes say nothing about the dock's timing or rendering. Measurements from the procedures below are recorded by the person running them; none are recorded in this repository yet.

Milestones covered: M2 (native write timing), M3 (heartbeat dependency isolation), the M5 CPU criterion, M6 (the streaming daemon) and M7 (resolution picker).

## Verified on a real dock

- Report layout: every continuation report of a multi-report message carries a full 16-byte header (Report ID, then 15 zero bytes) followed by up to 4080 payload bytes. A looped H.264 test stream sent with this layout, with the picture width and height in header bytes 8 to 11 and 12 to 15, rendered a picture on the panel. This was confirmed with a Python prototype that writes the same byte layout as `framing.rs`; the Rust build has not been run against the dock.
- Known open item: tearing is visible on fast motion and is being examined.

## Preparation

1. Connect the dock's USB-C cable to the Linux host.
2. Build: `cd src/screendockd && cargo build --release --features encode-ffmpeg`. The binary is `target/release/screendockd`.
3. Set up permissions as described in [configuration.md](configuration.md#permissions) so no `sudo` is needed.
4. Confirm discovery: `screendockd discover`. It prints the MI_00 and MI_01 nodes. Record both paths and the kernel version (`uname -r`).
5. Generate a real, decodable stream for the dock. ffmpeg needs `-pix_fmt yuv420p` for the baseline profile:

   ```text
   ffmpeg -f lavfi -i testsrc=size=1280x720:rate=25 -t 4 -c:v libx264 -pix_fmt yuv420p -profile:v baseline -bf 0 -g 25 -f h264 test.h264
   ```

   This gives a keyframe every 25 frames and no B-frames. `screendockd` splits it into one keyframe bundle (SPS, PPS, SEI, IDR) followed by single-NAL inter-frames.
6. Record the host: CPU model, USB port type (USB 2, 3.x, direct or through a hub), and the `git rev-parse --short HEAD` of the build.

Without `--h264-file` the tools send synthetic filler messages: a 3577 byte keyframe-shaped message and a 67,666 byte (17 report) inter-frame-shaped message with the documented sizes and NAL layout. They are valid for timing and are not decodable video, so they will not produce a picture.

## M2: native write timing

Goal: show that a compiled client writing back-to-back with `write(2)` matches the chunk timing the research reports, and observe what the panel renders. The reference is 0.77 ms per chunk, at or below which the milestone's timing criterion is met.

### Run A: timing with the research message sizes

```text
screendockd replay --iterations 200
```

This writes a keyframe-shaped message and a 17 chunk message 200 times each to MI_01, with no delay between chunks. Output includes the per-chunk `write(2)` latency (min, mean, p50, p95, p99, max in microseconds), the per-message total write time, and the mean wall time per chunk compared with 0.77 ms.

Record:

- The full output text.
- The mean wall time per chunk and p99 per-chunk latency.
- Any error (the failing chunk index and errno text, for example `Resource temporarily unavailable` or `Broken pipe`), and any new `dmesg` lines about the device.
- Whether the run repeats consistently across at least three runs.

### Run B: real stream

```text
screendockd replay --h264-file test.h264 --iterations 1
screendockd isolate --mode video --h264-file test.h264 --duration-secs 30
```

The first command sends the whole stream unpaced, back-to-back. The second sends it looped at 25 fps for 30 seconds (the heartbeat is not running in this condition).

Record for each command what the panel shows: full picture, partial picture (describe which region), a single frame then nothing, the "Connecting Signal..." overlay, or nothing. Take a photo if possible. Also record the timing lines and, for the paced run, the number of late frames.

### M2 exit criteria

- Mean wall time per chunk at or below 0.77 ms. The captures show 0.76 to 0.91 ms per chunk on the wire; the replay output is a host-side write measure.
- The panel renders the full picture of the stream rather than a partial region.

## M3: heartbeat dependency isolation

Goal: observe on-screen behavior for each of three conditions, so it is known whether the heartbeat is needed for video to render or only suppresses the "Connecting Signal..." overlay.

Use the same `test.h264` and the same duration for each condition. Before each condition, unplug and replug the dock (or otherwise return it to its idle state) and wait for it to settle. Run:

```text
screendockd isolate --mode heartbeat --duration-secs 30
screendockd isolate --mode video --h264-file test.h264 --duration-secs 30
screendockd isolate --mode both --h264-file test.h264 --duration-secs 30
```

Condition (a), heartbeat only, sends one JSON-RPC heartbeat per `daemon.heartbeat_interval_ms` on MI_00 and nothing on MI_01. Condition (b), video only, sends the looped stream on MI_01 and nothing on MI_00. Condition (c) runs both at once.

For each condition record, with approximate timestamps relative to the start:

- Panel state before starting (idle, overlay text, previous image).
- Panel state during the run: picture, overlay shown or not, blank, flicker.
- Panel state at the end of the run and 10 seconds after the process exits (does it return to the overlay when the heartbeat stops, does the last frame persist).
- The tool's report: heartbeat ticks sent, video messages sent, late frames, per-chunk latency line.
- Any error text from the tool and any `dmesg` lines.

The result to derive from the three records: whether video renders without the heartbeat (b), whether the heartbeat alone changes the panel (a), and whether (c) differs from (b) beyond the overlay.

### M3 exit criteria

A written observation for all three conditions, not only (c).

## M5: CPU cost of capture plus encode

Goal: record the sustained CPU use of screen capture plus libx264 encoding at 25 fps and 1280x720. No dock is needed. Needs a build with `--features encode-ffmpeg` and an interactive session on the target host.

```text
screendockd bench --duration-secs 60
```

Run it once on an idle desktop and once while the desktop is busy (for example playing a video). Record the full report text (frames and fps, wall time, CPU seconds and percentage of one core, wall time in capture and in encode), the host CPU model and core count, `ffmpeg -version` for the libx264 build, the session type, and the commit. See [encoding.md](encoding.md#cpu-measurement) for what the figure includes. The reported frame rate should stay near 25; a lower rate means the encoder does not keep up on that host.

Status: requires measurement on the target host. No figures are recorded in this repository.

## M6: streaming daemon

Goal: confirm the full pipeline puts the live desktop on the panel with the heartbeat running. Needs the dock, permissions set up and a build with `--features encode-ffmpeg`.

```text
screendockd run
```

With the default config this captures the desktop, encodes at 1280x720, 25 fps, and writes to the discovered nodes until interrupted. Run for at least 10 minutes. Record:

- The panel content: live desktop, partial picture, frozen picture, overlay or blank. Move a window and confirm it follows with the expected latency. Take a photo if possible.
- The log: warn lines (write errors, capture errors), and the progress lines (frames, messages, write errors) every 5 seconds.
- The summary printed on exit after Ctrl-C: frames, messages, keyframes, error counters, heartbeats.
- Whether the panel recovers after unplugging and replugging the dock while `run` is active, or after sending the process SIGTERM and starting it again. After 100 consecutive failed frames the process exits with an error, so a supervisor (the shipped systemd user unit) is expected to restart it.
- Any `dmesg` lines about the device.

Exit criteria: the live desktop is visible and stays up for the whole run, shutdown with SIGINT and SIGTERM is clean (exit status 0), and a write failure does not stop the daemon while the failures are not continuous.

Status: requires on-device validation.

## M7: resolution picker

Goal: confirm the panel accepts a configured size and renders it correctly. Needs the dock and the M6 setup. For each size below, set `display.width`, `display.height` and `display.orientation` in the config, run `screendockd run` for a few minutes and record what the panel shows:

| `display.width` | `display.height` | `display.orientation` |
|---|---|---|
| 1280 | 720 | landscape |
| 900 | 600 | landscape |
| 720 | 1280 | portrait |

For each, record: whether the picture fills the panel, is cropped, letterboxed, stretched or rotated, whether the panel's own orientation changes, and any error from the tool. Also record whether a size not in the table (for example 1920x1080) is accepted and how it renders. Exit criterion: the native size renders correctly, and the result for every other size tried is written down, including failures.

Status: requires on-device validation. The header width and height are the configured values and are unit tested; how the panel treats sizes other than 1280x720 is unverified.

## Dry runs without a dock

The same commands run against regular files, which checks the tooling itself and not the device:

```text
touch /tmp/mi00 /tmp/mi01
screendockd replay --mi01 /tmp/mi01 --iterations 5
screendockd isolate --mode both --mi00 /tmp/mi00 --mi01 /tmp/mi01 --duration-secs 3
screendockd run --mi00 /tmp/mi00 --mi01 /tmp/mi01 --h264-file test.h264 --max-frames 50
```

The tools print a note when a node is not a character device. Timings from such runs are not hardware timings.
