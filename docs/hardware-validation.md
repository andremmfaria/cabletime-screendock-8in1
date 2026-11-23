# Hardware validation

Status: requires on-device validation. Everything here is implemented and passes loopback tests against files and pipes, which say nothing about the dock's timing or rendering. The procedures below are run by a person with the dock attached. Results are recorded by the person running them; none are recorded in this repository yet.

Two milestones are covered: M2 (native write timing) and M3 (heartbeat dependency isolation).

## Preparation

1. Connect the dock's USB-C cable to the Linux host.
2. Build: `cd src/screendockd && cargo build --release`. The binary is `target/release/screendockd`.
3. Set up permissions as described in [configuration.md](configuration.md#permissions) so no `sudo` is needed.
4. Confirm discovery: `screendockd discover`. It prints the MI_00 and MI_01 nodes. Record both paths and the kernel version (`uname -r`).
5. Generate a real, decodable stream for the dock. ffmpeg needs `-pix_fmt yuv420p` for the baseline profile:

   ```text
   ffmpeg -f lavfi -i testsrc=size=1280x720:rate=25 -t 4 -c:v libx264 -pix_fmt yuv420p -profile:v baseline -bf 0 -g 25 -f h264 test.h264
   ```

   This gives a keyframe every 25 frames and no B-frames. `screendockd` splits it into one keyframe bundle (SPS, PPS, SEI, IDR) followed by single-NAL inter-frames.
6. Record the host: CPU model, USB port type (USB 2, 3.x, direct or through a hub), and the `git rev-parse --short HEAD` of the build.

Without `--h264-file` the tools send synthetic filler messages: a 3577 byte keyframe-shaped message and a 67,666 byte (17 chunk) inter-frame-shaped message with the documented sizes and NAL layout. They are valid for timing and are not decodable video, so they will not produce a picture.

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

## Dry runs without a dock

The same commands run against regular files, which checks the tooling itself and not the device:

```text
touch /tmp/mi00 /tmp/mi01
screendockd replay --mi01 /tmp/mi01 --iterations 5
screendockd isolate --mode both --mi00 /tmp/mi00 --mi01 /tmp/mi01 --duration-secs 3
```

The tools print a note when a node is not a character device. Timings from such runs are not hardware timings.
