# Testing

All commands run from `src/screendockd`.

## Default checks

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same checks, plus a clippy run with `--features capture-wayland` after installing the PipeWire development packages. No default test needs a dock, a hidraw node or a physical display. Hardware access is replaced by a regular file or pipe through `HidrawHandle::from_file`.

## Capture, Stage 1: Xvfb

`tests/capture_x11.rs` starts a real Xvfb (320x240, 24-bit depth) on a free display number, paints synthetic content into the root window through `x11rb`, captures it with `X11ShmCapture` and asserts exact pixel values. It covers:

- Painted regions: a red background, a blue rectangle and a green rectangle, checked at their corners and one pixel outside each edge, with width, height, format and stride.
- Repainting: a second frame reflects drawing done after the first capture.
- Boxed backend use through `Box<dyn CaptureBackend>`, and pacing at 20 fps.
- The `capture` subcommand run as a child process, writing two PNG files whose size and first pixel are checked.

Requirements: `Xvfb` on `PATH` (Debian and Ubuntu package `xvfb`) and a writable `/tmp/.X11-unix`, because XShm needs a unix socket for descriptor passing.

- When Xvfb is missing, or starts but never accepts a connection, the tests print a skip note and pass.
- With `SCREENDOCK_REQUIRE_XVFB` set to any value, the same condition fails the test. CI sets it, so the tests always run there.
- On a host where `/tmp/.X11-unix` is not writable by the current user, run the tests in a mount namespace that provides one:

```text
SCREENDOCK_REQUIRE_XVFB=1 bwrap --dev-bind / / --tmpfs /tmp/.X11-unix cargo test --test capture_x11
```

Manual run of the same path:

```text
Xvfb :99 -screen 0 1280x720x24 -nolisten tcp &
DISPLAY=:99 cargo run -- capture --backend x11 --frames 3 --out-dir /tmp/capture-out
```

The PNG files show the empty Xvfb root window; paint something first (for example with `xsetroot -solid red`) to see content.

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
