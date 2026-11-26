# Hardware validation

Status: partially validated on a real dock; the procedures below still require a person with the dock attached. Loopback tests against files and pipes say nothing about the dock's timing or rendering. Measurements from the procedures below are recorded by the person running them; none are recorded in this repository yet.

Procedures covered: native write timing, heartbeat dependency isolation, the CPU criterion, the streaming daemon and the resolution picker.

## Verified on a real dock

- Report layout: every continuation report of a multi-report message carries a full 16-byte header (Report ID, then 15 zero bytes) followed by up to 4080 payload bytes. A looped H.264 test stream sent with this layout, with the picture width and height in header bytes 8 to 11 and 12 to 15, rendered a picture on the panel. This was confirmed with a Python prototype that writes the same byte layout as `framing.rs`; the Rust build has not been run against the dock.
- Session handshake: after a fresh replug the full handshake worked with the replies this driver expects: the `app-launched` result `{res-width,res-height}`, the `notify-projection-started` result `isAllowed`, and an EDID passthrough (id 101) from the dock about every 5 seconds until answered. The Python prototype did this; the Rust session has not been run against the dock.
- A dock that already holds a projection session from an earlier host process (heartbeat gap under about 5 seconds, or none) sends no `notify-app-accessories-are-ready` and does not answer `app-launched` or `notify-projection-started`, yet renders video with only heartbeats and correctly framed video. The Python prototype produced a picture in this silent state by sending, without waiting for replies, `app-launched`, `app_tx_mouse_icon_change`, `common.data_passthrough` id 103 and `notify-projection-started`, then waiting 0.8 s before video. The Rust session sends the same sequence after the 3 second ready wait, waits at most 1.5 s per reply, waits the 0.8 s settle delay and arms video in `AssumedActive`, with heartbeats still sent. It runs the full handshake again if the dock later announces ready. A session that skips these messages when no ready announcement is seen leaves the panel on the Connecting screen. Status: requires on-device validation of the Rust session with the full silent sequence.
- Glitches on fast motion: a Python prototype looping a 1280x720, 25 fps H.264 file of about 122 KB for 4 seconds (about 245 kbit/s, ffmpeg defaults) showed a moving counter smoothly with no lag and glitches only on a fast moving colour bar. At that bit rate the encoder is starved on high motion content, so the glitches are consistent with compression artifacts rather than framing or transport errors. The daemon encodes at `encode.bitrate_kbps` (default 4000); see [encoding.md](encoding.md#rate-control-and-picture-quality). Status: requires on-device validation of the daemon's picture on the same fast motion content.

## Preparation

1. Connect the dock's USB-C cable to the Linux host.
2. Build: `cd src/screendockd && cargo build --release --features encode-ffmpeg`. The binary is `target/release/screendockd`. The `replay` and `isolate` commands are in the `tools` feature, which is on by default.
3. Set up permissions as described in [configuration.md](configuration.md#permissions) so no `sudo` is needed.
4. Confirm discovery: `screendockd discover`. It prints the MI_00 and MI_01 nodes. Record both paths and the kernel version (`uname -r`).
5. Generate a real, decodable stream for the dock. ffmpeg needs `-pix_fmt yuv420p` for the baseline profile:

   ```text
   ffmpeg -f lavfi -i testsrc=size=1280x720:rate=25 -t 4 -c:v libx264 -pix_fmt yuv420p -profile:v baseline -bf 0 -g 25 -f h264 test.h264
   ```

   This gives a keyframe every 25 frames and no B-frames. `screendockd` splits it into one keyframe bundle (SPS, PPS, SEI, IDR) followed by single-NAL inter-frames.
6. Record the host: CPU model, USB port type (USB 2, 3.x, direct or through a hub), and the `git rev-parse --short HEAD` of the build.

Without `--h264-file` the tools send synthetic filler messages: a 3577 byte keyframe-shaped message and a 67,666 byte (17 report) inter-frame-shaped message with the documented sizes and NAL layout. They are valid for timing and are not decodable video, so they will not produce a picture.

## Native write timing

Goal: show that a compiled client writing back-to-back with `write(2)` matches the chunk timing the research reports, and observe what the panel renders. The reference is 0.77 ms per chunk, at or below which the timing criterion is met.

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

### Write timing exit criteria

- Mean wall time per chunk at or below 0.77 ms. The captures show 0.76 to 0.91 ms per chunk on the wire; the replay output is a host-side write measure.
- The panel renders the full picture of the stream rather than a partial region.

## Heartbeat dependency isolation

Goal: observe on-screen behavior for each of three conditions, so it is known whether the heartbeat is needed for video to render or only suppresses the "Connecting Signal..." overlay.

Use the same `test.h264` and the same duration for each condition. Before each condition, unplug and replug the dock (or otherwise return it to its idle state) and wait for it to settle. Run:

```text
screendockd isolate --mode heartbeat --duration-secs 30
screendockd isolate --mode video --h264-file test.h264 --duration-secs 30
screendockd isolate --mode both --h264-file test.h264 --duration-secs 30
```

Condition (a), heartbeat only, runs the dock session on MI_00 (heartbeat, reader and handshake) and nothing on MI_01. Condition (b), video only, sends the looped stream on MI_01 and nothing on MI_00; with no handshake a dock that has not been handshaken discards it. Condition (c) runs the session and then the video. Pass `--no-handshake` to get the bare heartbeat loop and ungated video of the earlier procedure.

For each condition record, with approximate timestamps relative to the start:

- Panel state before starting (idle, overlay text, previous image).
- Panel state during the run: picture, overlay shown or not, blank, flicker.
- Panel state at the end of the run and 10 seconds after the process exits (does it return to the overlay when the heartbeat stops, does the last frame persist).
- The tool's report: session counters (heartbeats sent, dock heartbeats seen, handshakes, losses) or heartbeat ticks sent, video messages sent, late frames, per-chunk latency line.
- Any error text from the tool and any `dmesg` lines.

The result to derive from the three records: whether video renders without the heartbeat (b), whether the heartbeat alone changes the panel (a), and whether (c) differs from (b) beyond the overlay.

### Heartbeat isolation exit criteria

A written observation for all three conditions, not only (c).

## Session handshake on a fresh dock

Goal: confirm that `screendockd run` brings up the panel from a freshly plugged dock without any other host software. Unplug and replug the dock, wait for it to settle, then run `RUST_LOG=debug screendockd run --h264-file test.h264`. Record:

- The log sequence: the state changes (`WaitingForReady`, `AppLaunchedSent`, `ProjectionStartedSent`, `Settling`), the `session armed` line with the armed state (`Active` or `AssumedActive`), and any `dock did not announce ready`, `handshake failed`, `session lost` or `no answer from the dock` warnings.
- The time from process start to the first picture on the panel.
- The same run again without replugging (the dock then holds the session from the first run): the expected log is the 3 second ready wait, the four handshake messages sent without `notify-projection-stopped`, then `AssumedActive` after the 0.8 s settle delay, whether or not dock heartbeats are seen, and the picture appears and the Connecting screen goes away.
- Whether the picture survives stopping and restarting the process, and unplugging and replugging while `run` is active.

Status: requires on-device validation of the Rust implementation. See the verified facts at the top of this page for what the Python prototype showed.

## CPU cost of capture plus encode

Goal: record the sustained CPU use of screen capture plus libx264 encoding at 25 fps and 1280x720. No dock is needed. Needs a build with `--features encode-ffmpeg` and an interactive session on the target host.

```text
screendockd bench --duration-secs 60
```

Run it once on an idle desktop and once while the desktop is busy (for example playing a video). Record the full report text (frames and fps, wall time, CPU seconds and percentage of one core, wall time in capture and in encode), the host CPU model and core count, `ffmpeg -version` for the libx264 build, the session type, and the commit. See [encoding.md](encoding.md#cpu-measurement) for what the figure includes. The reported frame rate should stay near 25; a lower rate means the encoder does not keep up on that host.

Status: requires measurement on the target host. No figures are recorded in this repository.

## Streaming daemon

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

## Resolution picker

Goal: confirm the panel accepts a configured size and renders it correctly. Needs the dock and the streaming daemon setup. For each size below, set `display.width`, `display.height` and `display.orientation` in the config, run `screendockd run` for a few minutes and record what the panel shows:

| `display.width` | `display.height` | `display.orientation` |
|---|---|---|
| 1280 | 720 | landscape |
| 900 | 600 | landscape |
| 720 | 1280 | portrait |

For each, record: whether the picture fills the panel, is cropped, letterboxed, stretched or rotated, whether the panel's own orientation changes, and any error from the tool. Also record whether a size not in the table (for example 1920x1080) is accepted and how it renders. Exit criterion: the native size renders correctly, and the result for every other size tried is written down, including failures.

Status: requires on-device validation. The header width and height are the configured values and are unit tested; how the panel treats sizes other than 1280x720 is unverified.

## Live size changes

Goal: confirm that `screendockd ctl set resolution` changes the picture size of a running daemon through the dock cycle, and that the panel shows each new size. Needs the dock, permissions set up, a running `screendockd run` and a build with `--features encode-ffmpeg`.

Procedure:

1. Start `RUST_LOG=info screendockd run` against the screen to mirror and wait until the log prints `session armed, video allowed`.
2. Run `screendockd ctl set resolution 900x720`, then `screendockd ctl set resolution 640x360`, then `screendockd ctl set resolution 1280x720`, waiting for each command to print its answer before the next.
3. Watch the panel during each change and write down the size you see, whether the picture fills the panel, is cropped, letterboxed or stretched, whether the Connecting screen appears, and how long the panel is blank.
4. Stop the daemon with SIGINT and read the summary line.

Expected observations: each command answers `applied (dock cycle)` after about 5 s with a silent dock. The log shows one `session armed, video allowed` line per change with `renegotiating` true, and the summary shows one more handshake than the number of changes (the initial one plus one per change), no session losses and no write errors. The panel shows the picture at the new size after each cycle.

Measured on a real dock with the Rust session (X11 backend capturing a 1280x720 Xvfb, three consecutive changes to 900x720, 640x360 and 1280x720):

- Each change answered `applied (dock cycle)`.
- The daemon sent 2765 messages in 130 s with 0 write errors and 0 session losses, and counted 4 handshakes: 1 initial and 3 cycles.
- The dock was silent: it sent no ready announcement and no replies, and `dock_alive` was false at each cycle. Each cycle took about 5 s end to end (two 1.5 s reply timeouts, the 0.8 s settle delay, and about 1 s for the encoder rebuild and verify), and the session re-armed as `AssumedActive` each time.

Observed on the panel by the user during the same three changes, with a 1280x720 X11 source showing an xterm:

- The panel flashed black and returned to the picture twice. Three changes were issued, so either one change showed no visible flash or two flashes merged.
- The xterm text and window looked the same size on the panel after each change. Either the capture scales the whole 1280x720 source into the encoded size (mirror mode), or the panel scales the picture to fit; which of the two happens is unknown.
- On some screen refreshes the text looked slightly blurred.

Not measured: the picture size the panel actually used after each change, and the heartbeat gaps during the cycle. The cycle is verified by write success only, because a silent dock gives no reply to check. Sizes beyond the three above, orientation changes and a dock that replies to the cycle messages (after a replug) were not run. The black flashes are an observation by eye, not a timed measurement.

Steps to verify the size the panel used:

1. For each size, save a screenshot of the source (`xwd` or `import -window root`) right after the change.
2. Capture the sent frames for the same size, by decoding the encoded output of the run with ffmpeg, and read the decoded width and height with `ffprobe`.
3. Compare the decoded frame with the screenshot: if the xterm occupies the same fraction of the frame in both, the capture scales the source into the encoded size; if it occupies a different fraction or is cropped, it does not.
4. Photograph the panel at each size and compare the xterm fraction of the panel with the decoded frame to tell whether the panel rescales.
5. Repeat with the dock replugged so that it answers the cycle messages, and note whether the black flash count then matches the number of changes.

Status: requires on-device validation of the picture size at each change.

## Dock EDID cache

Goal: confirm that a cold start caches the dock's EDID and that `ctl status` reports it. Needs the dock freshly replugged (a dock that holds a session sends no EDID) and a running `screendockd run`.

1. Remove the cache file if present: `rm -f "${XDG_STATE_HOME:-$HOME/.local/state}/screendockd/dock-edid.bin"`.
2. Replug the dock and start `screendockd run`.
3. Run `screendockd ctl status` and read `edid-source`, `edid-name`, `edid-native` and `edid-modes`; run `ctl get edid-modes`.
4. Check that the cache file exists with mode 0600, and compare it with `tests/fixtures/dock_edid.bin` using `cmp` (the fixture holds all 258 bytes the dock sent when it was captured).
5. Stop the daemon, start it again without replugging and read the same keys before the dock answers anything: the source is `cache` until a dock EDID arrives, and stays `cache` when the dock holds its session and sends none.

Expected: `dock`, `HDMI Extender`, `1920x1080@60` and the six modes `1920x1080@60,1280x720@60,1280x960@60,1280x800@60,1280x1024@60,1600x900@60`.

The parser and the fake dock cover this path with the captured bytes. Status: requires on-device validation of the cache file written by the Rust daemon and of the reported values on a cold start.

## Xorg with the dummy driver

Validated on the development host (Ubuntu 26.04, Intel JasperLake UHD with the `i915` kernel driver, no desktop session) with a rootless Xorg and the dummy driver. The server starts as the ordinary user, without a virtual terminal:

```text
/usr/lib/xorg/Xorg -config dummy.conf -configdir <empty dir> -noreset -logfile xorg.log -sharevts -novtswitch -keeptty -ac -nolisten tcp :N
```

The config has one `Device` with `Driver "dummy"` and `VideoRam 256000`, one `Monitor` with a single 1280x720 modeline, one `Screen` with `Virtual 4096 2160` and `ServerFlags` `AutoAddDevices`, `AutoEnableDevices` and `AutoAddGPU` set to `false`. `xrandr` 1.5.3 reports RandR 1.6.

| Check | Result |
|---|---|
| Outputs | `DUMMY0` connected and active, `DUMMY1` to `DUMMY15` disconnected, each with one usable CRTC |
| `xrandr --fb 6000x3000` | accepted, larger than the configured `Virtual` size |
| `xrandr --setmonitor VMON 1280/300x720/170+2720+0 none` | accepted and listed by `--listmonitors` |
| `--newmode`, `--addmode DUMMY1`, `--output DUMMY1 --mode ... --pos 1280x0` | `DUMMY1` becomes a connected output and a listed monitor at the position |
| Turning `DUMMY1` off, `--delmode`, `--rmmode` | the monitor disappears and the layout returns to one monitor |
| Two `Device` sections in one layout | two separate X screens, each with its own `DUMMY0`; not a second output on one screen |

Monitors made by `X11RandrDisplay` on this server, each captured with exact pixels over a painted rectangle:

| Tier | Result |
|---|---|
| 1, real output (`DUMMY1`, custom mode) | listed by `xrandr --listmonitors`, second output in the facts, capture exact, drop restores one monitor and the 1280x720 screen |
| 2, bare monitor over an enlarged screen | listed, capture exact (the pixels exist because the screen covers the rectangle), drop restores |
| Resize in both tiers | the listed geometry changes and the screen grows |
| Leaked monitor, then recovery | removed, screen size restored, state file deleted |

Run time switching with the daemon on this server (file nodes, no handshake): `ctl set mode extend` creates a tier 1 monitor `DUMMY1` at +1280+0, `mode mirror` removes it, SIGTERM restores the listing, and after SIGKILL `vdisplay cleanup` restores it.

Whether a desktop environment's display settings list a bare tier 2 monitor, and whether a compositor paints it, requires validation on a real session.

The `modesetting` driver on the i915 GPU cannot start without root here: Xorg reports `Cannot open /dev/tty0 (Permission denied)`, and `/dev/dri/card0` is `root:video` with mode 0660. Status: requires validation on a machine with a logged in X11 session and a real GPU driver.

## Headless sway

Validated on the development host (Ubuntu 26.04, sway 1.11, `grim`) with sway started as the ordinary user on a private runtime directory:

```text
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman sway -c /dev/null
```

| Check | Result |
|---|---|
| Start | one output `HEADLESS-1` at 1280x720; the IPC and Wayland sockets appear in the runtime directory |
| `create_output` | a new `HEADLESS-<n>` output |
| `output <name> mode WxH pos X Y` | any size is accepted at run time; `GET_OUTPUTS` reports the rectangle, with an empty mode list |
| `output <name> unplug` | the output disappears |
| `grim -o <name> -` after `output <name> bg #1060d0 solid_color` | a PNG of the output size (1280x720, then 900x720 after a resize) whose pixels equal the colour |

`WlrootsHeadlessDisplay` on this server: the output is created at the placed rectangle (right of `HEADLESS-1` by default), resized and moved, several monitors coexist, and dropping the monitor or the monitor manager unplugs the output. Switching the display mode through the monitor manager creates and removes the output.

Not verified: a prompt free `xdg-desktop-portal-wlr` screencast of a headless output, frames read from the output by the daemon, and sway on real hardware. Status: requires validation.

## Headless GNOME Shell

Validated on the development host (Ubuntu 26.04, GNOME Shell 50.1 with Mutter 50.1, PipeWire and WirePlumber from the distribution, no GPU access) with every process started as the ordinary user on a private runtime directory and a private session bus:

```text
dbus-daemon --session --nofork --nopidfile --address=unix:path=$XDG_RUNTIME_DIR/bus
pipewire
wireplumber
gnome-shell --headless --wayland --no-x11 --virtual-monitor 1280x720
```

The environment has `XDG_SESSION_TYPE=wayland` and `XDG_CURRENT_DESKTOP=GNOME`, and no `DISPLAY` or `WAYLAND_DISPLAY`. The shell logs `Created surfaceless renderer without GPU`, so buffers are shared memory.

| Check | Result |
|---|---|
| `org.gnome.Mutter.ScreenCast` | version 4 on the bus once the shell is up |
| `CreateSession`, `RecordVirtual`, `Start` | succeed; `PipeWireStreamAdded` carries the node id; the node is `Stream/Output/Video` |
| frames read by `MutterVirtualDisplay` and `WaylandPortalCapture::connect_stream` | `Bgrx8888`, 1280x720 when 1280x720 is asked for, 1600x900 when 1600x900 is asked for; the first pixel is the shell background, not black |
| the same size and first pixel read by `gst-launch-1.0 pipewiresrc` on a second monitor | the raw frame is 1280 x 720 x 4 bytes and starts with the same pixel |
| retargeting a running capture from one stream to another (1280x720 to 800x600) | the next frames have the new size |
| dropping the monitor | the node disappears from `pw-dump` |
| `org.gnome.Mutter.DisplayConfig` | one monitor with one mode, so the size of that monitor cannot change at run time |
| portal `ScreenCast` | `AvailableSourceTypes` is 0 and the backend does not export `org.freedesktop.impl.portal.ScreenCast`, so the portal virtual source is not usable |

Frames are produced when the shell repaints. A node delivers a frame to its first consumer at once; a second consumer of the same node, or a new size negotiated on a node that was already read, gets a frame only after the next repaint, which does not happen while nothing changes. A Wayland client sees only the output made with `--virtual-monitor`, not the `RecordVirtual` monitor, so no client can be drawn on it and the frames show the shell background.

GNOME on Wayland checklist for a machine with a real GPU session. Record the machine, `gnome-shell --version`, `echo $XDG_SESSION_TYPE`, the GPU and driver, the PipeWire version, the `xdg-desktop-portal` and `xdg-desktop-portal-gnome` versions, and pass or fail per line with the log file names.

1. Read the portal `AvailableSourceTypes` and `version` and save the output (headless it was 0).
2. Run `screendockd vdisplay probe` and save the output.
3. With `wayland_backend = "mutter"`, check the interface version with `gdbus introspect`, that extend mode starts, that the dock shows the monitor at the configured size, whether the monitor appears in Settings, Displays, and where Mutter places it.
4. With `wayland_backend = "portal"`, record the prompt, whether the restore token avoids it on the second start, the stream size, and whether the monitor appears in Settings, Displays.
5. Drag a window onto the monitor and check the picture, the frame rate and the CPU use against the figure in [encoding.md](encoding.md).
6. Change the size from Settings if the monitor is listed, else with `ctl set resolution`, and record whether the stream renegotiates and whether the dock cycle runs once.
7. Toggle `ctl set mode mirror` and `extend`, and the config file plus `ctl reload`.
8. Kill the daemon with SIGKILL and check that no monitor remains; lock the screen, suspend and resume.
9. Force a failure (stop the portal, or deny a prompt) and check the fallback to mirror with a logged reason.

Validated GPU driver: the development host has an Intel JasperLake UHD with the `i915` kernel driver, but GNOME Shell ran without GPU access there, so no GPU driver is validated for the Wayland paths. Capture uses shared memory buffers only, so no code path depends on the driver.

Not verified: GNOME on a real GPU session (lock screen, suspend, the Settings display page, buffer negotiation with a GPU), the portal virtual source, and the position Mutter gives the monitor. Status: requires validation.

## Dry runs without a dock

The same commands run against regular files, which checks the tooling itself and not the device:

```text
touch /tmp/mi00 /tmp/mi01
screendockd replay --mi00 /tmp/mi00 --mi01 /tmp/mi01 --iterations 5 --no-handshake
screendockd isolate --mode both --mi00 /tmp/mi00 --mi01 /tmp/mi01 --duration-secs 3 --no-handshake
screendockd run --mi00 /tmp/mi00 --mi01 /tmp/mi01 --h264-file test.h264 --max-frames 50 --no-handshake
```

Regular files cannot hold the dock's side of the handshake, so dry runs need `--no-handshake`. The tools print a note when a node is not a character device. Timings from such runs are not hardware timings.
