# Testing

All commands run from `src/screendockd`.

## Default checks

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same checks, plus a clippy run with `--features capture-wayland` after installing the PipeWire development packages, and a separate `encode-ffmpeg` job that installs the libav development packages and runs clippy and the full test suite with `--features encode-ffmpeg`. No default test needs a dock, a hidraw node or a physical display. Hardware access is replaced by a regular file or pipe through `HidrawHandle::from_file`.

## Test groups

The integration tests are grouped by area, one test target per group: `cargo test --test protocol`, `cargo test --test session`, `cargo test --test capture`, `cargo test --test e2e` and `cargo test --test encode --features encode-ffmpeg`. The `capture` and `e2e` groups share the Xvfb and X11 painter helpers in `tests/support/`.

## Capture, Stage 1: Xvfb

`tests/capture/x11.rs` starts a real Xvfb (320x240, 24-bit depth) on a free display number, paints synthetic content into the root window through `x11rb`, captures it with `X11ShmCapture` and asserts exact pixel values. It covers:

- Painted regions: a red background, a blue rectangle and a green rectangle, checked at their corners and one pixel outside each edge, with width, height, format and stride.
- Repainting: a second frame reflects drawing done after the first capture.
- Boxed backend use through `Box<dyn CaptureBackend>`, and pacing at 20 fps.
- The `capture` subcommand run as a child process, writing two PNG files whose size and first pixel are checked.

Requirements: `Xvfb` on `PATH` (Debian and Ubuntu package `xvfb`) and a writable `/tmp/.X11-unix`, because XShm needs a unix socket for descriptor passing.

- When Xvfb is missing, or starts but never accepts a connection, the tests print a skip note and pass.
- With `SCREENDOCK_REQUIRE_XVFB` set to any value, the same condition fails the test. CI sets it, so the tests always run there.
- On a host where `/tmp/.X11-unix` is not writable by the current user, run the tests in a mount namespace that provides one:

```text
SCREENDOCK_REQUIRE_XVFB=1 bwrap --dev-bind / / --tmpfs /tmp/.X11-unix cargo test --test capture
```

Manual run of the same path:

```text
Xvfb :99 -screen 0 1280x720x24 -nolisten tcp &
DISPLAY=:99 cargo run -- capture --backend x11 --frames 3 --out-dir /tmp/capture-out
```

The PNG files show the empty Xvfb root window; paint something first (for example with `xsetroot -solid red`) to see content.

## Control socket

`adapters/control/server.rs` tests run the server in process against a stand-in daemon thread: the socket is mode 0600 inside a 0700 directory and is removed on drop, a stale socket file is replaced, a live socket is not taken over, a regular file at the path is left alone, an over-long path is refused, an oversized request line and malformed requests get an `invalid` answer without reaching the daemon, eight concurrent clients each get their own answer, a silent client does not block others, and requests are rejected once the daemon side is gone. `tests/protocol/control_wire.rs` pins the exact request and response lines, strictness and the line caps. `tests/e2e/control_socket.rs` runs `screendockd ctl status` and `ctl get` against the real daemon binary on Xvfb: the settings and version come back, an unknown key exits with code 4, a missing daemon exits with code 3, the socket is private, it is removed at exit and `control.enabled = false` creates none. Each daemon in the e2e tests gets its own `XDG_RUNTIME_DIR`, so parallel daemons never share a socket.

## Session and simulated dock

`testing::fake_dock::FakeDock` (compiled for unit tests and under the `testing` feature, which the crate's own tests enable through a dev-dependency on itself) stands in for the dock: MI_00 and MI_01 are Unix datagram socket pairs (datagrams keep report boundaries like a hidraw read), and a thread speaks the dock side. A fresh dock announces ready and answers `app-launched` and `notify-projection-started`; a dock that holds a session only heartbeats. It records every host message byte for byte, checks the MI_00 report layout (4096 bytes, Report ID 1, zero header bytes) and the MI_01 header layout, and counts video reports that arrive before its sink is started. Tests in `session.rs` cover the exact bytes of every host message, the inbound assembler and parser, the full handshake order, the settle delay, the bounded fallback to `AssumedActive`, the exact first 64 bytes of the heartbeat and `app-launched` reports (equal to the reference prototype's framing), a silent dock that must receive `app-launched`, cursor, passthrough 103 and `notify-projection-started` in order and without `notify-projection-stopped`, a fully silent dock (no ready, no heartbeat echo, no replies) that receives that sequence before any video report, arms in `AssumedActive` and receives video, a late ready announcement that triggers the full handshake from `AssumedActive`, denied projection, an error reply to `app-launched`, loss on a re-announced ready and on a reported write failure (with the handshake redone and ids continuing), heartbeat cadence, startup failures and reader end of stream. `tests/session/dock.rs` runs `daemon::run` against the fake dock, including a dock with a held session and a fully silent dock. `validation.rs` has an `isolate` run with a session. These tests need no display and no hardware.

## Encoder

Unit tests cover the message policy (`messages_from_packet`), the prerecorded encoder (looping, keyframe restart, rejection of streams without a leading keyframe), `DisplayGeometry` (header size for 1280x720, 900x600 and portrait 720x1280, orientation and range checks) and device ID parsing and override in discovery.

`tests/encode/ffmpeg.rs` is built only with `--features encode-ffmpeg`, which needs the libav development files, `libx264-dev`, `clang` and `pkg-config`. It encodes a synthetic BGRA sequence with the libx264 encoder, checks the message shape and the SPS profile, and decodes the result with the `ffmpeg` command line tool. When the `ffmpeg` binary is missing the decode checks print a skip note and pass; with `SCREENDOCK_REQUIRE_FFMPEG` set to any value they fail instead. CI sets it.

```text
cargo test --features encode-ffmpeg
```

Status: this feature does not compile on hosts without the libav development files; CI is where it is built and tested.

## Daemon

`tests/e2e/daemon_xvfb.rs` runs the real `screendockd run --no-handshake` binary against Xvfb with the prerecorded encoder and regular files for MI_00 and MI_01 (files cannot hold a dock conversation), and checks the written video messages against the source stream, the heartbeat, graceful exit on SIGTERM and SIGINT, and fail-fast startup. Unit tests in `src/daemon.rs` cover the error policy with synthetic backends and loopback files. These tests need Xvfb on the same terms as the capture tests (`SCREENDOCK_REQUIRE_XVFB`). See [daemon.md](daemon.md#tests).

## CPU measurement

`screendockd bench` measures capture plus encode CPU time; see [encoding.md](encoding.md#cpu-measurement). The unit tests check the arithmetic and the loop with a synthetic backend; the figure that matters needs the target host and the libx264 build.

## Capture, Stage 1: Wayland

A headless compositor (Weston or a wlroots compositor with a PipeWire and portal setup) is needed to exercise `WaylandPortalCapture` without a desktop. The default host has none of these, so the Wayland backend has no automated runtime test. Its compile check is the CI feature build.

Status: requires validation.

## Capture, Stage 2: real desktop

Run on a machine with an interactive session. Status: requires validation.

X11 session:

1. Confirm `echo $XDG_SESSION_TYPE` prints `x11`.
2. Run `screendockd capture --frames 5 --out-dir /tmp/cap`.
3. Open the PNG files and confirm they show the actual desktop at the full screen size.
4. Run again with `capture.fps` set to a low value such as 5 and confirm the frames are paced.

Wayland session (build with `cargo build --features capture-wayland`, with the PipeWire development packages installed):

1. Confirm `echo $XDG_SESSION_TYPE` prints `wayland`.
2. Run `screendockd capture --frames 5 --out-dir /tmp/cap` and accept the compositor's share dialog.
3. Open the PNG files and confirm they show the selected monitor.
4. Record the pixel format and size that were negotiated, and whether the compositor delivered frames while the screen was static.
5. Run with `--backend x11` inside the Wayland session to see how XWayland capture differs (it shows only X11 clients).

Record results in [hardware-validation.md](hardware-validation.md) style: what was run, what was seen, and anything unexpected.
