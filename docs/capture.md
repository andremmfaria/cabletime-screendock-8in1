# Capture

The capture layer turns the desktop into a stream of `Frame` values for the encoder. The `CaptureBackend` trait and the `Frame` type are declared in `src/screendockd/src/ports/types.rs` and re-exported from `src/screendockd/src/ports/capture.rs`; backend selection and the implementations are in `src/screendockd/src/adapters/capture/`.

## Types

| Item | Description |
|---|---|
| `Frame` | `width`, `height`, `format` (`PixelFormat`), `stride` (bytes per row) and `data`. `Frame::new` rejects empty frames, strides shorter than a row and buffers shorter than the rows need. Helpers: `pixel_rgb(x, y)`, `to_rgb8()`, `write_png(path)` |
| `PixelFormat` | Four-byte layouts named by memory byte order: `Bgrx8888`, `Bgra8888`, `Rgbx8888`, `Rgba8888` |
| `CaptureError` | `thiserror` enum: `NoSession`, `BackendUnavailable`, `Connection`, `Shm`, `Portal`, `Stream`, `InvalidFrame`, `Png`, `Unsupported`, `InvalidTarget`, `Io` |
| `CaptureBackend` | Object-safe, `Send` trait with `name()` and `next_frame() -> Result<Frame, CaptureError>`. `next_frame` blocks until a frame is due. `retarget(CaptureTarget)` points a running backend at another area and frame rate; the default refuses with `Unsupported` |
| `CaptureTarget` | A `CaptureArea` (`Whole` or `Region(Rect)` in source coordinates) and a frame rate, where zero disables pacing |

## Backend selection

The backend is chosen once at startup by `create_backend(&config, choice)`, which returns `Box<dyn CaptureBackend>`.

1. The choice comes from `capture.backend` in the config (`auto`, `x11` or `wayland`), or from `--backend` on the `capture` subcommand, which overrides the config.
2. `x11` and `wayland` select that backend regardless of the session.
3. `auto` detects the session type: `$XDG_SESSION_TYPE` set to `wayland` or `x11` decides; otherwise a non-empty `$WAYLAND_DISPLAY` means Wayland, then a non-empty `$DISPLAY` means X11. With none of these set, selection fails with `NoSession`.

## X11 backend (`adapters/capture/x11_shm.rs`)

`X11ShmCapture` captures the whole root window through the MIT-SHM extension. It uses `x11rb`, a pure-Rust X11 client, so it needs no system libraries to build.

- Setup connects to `capture.x11_display` (or `$DISPLAY`), checks that the root visual is 32 bits per pixel with red, green and blue masks `0xff0000`, `0xff00` and `0xff` and little-endian image byte order, and requires MIT-SHM 1.2 or newer.
- A memfd is created at the root window size, mapped, and handed to the server with `ShmAttachFd`.
- Each `next_frame` issues one `ShmGetImage` into that segment and copies the pixels into a `Frame` with format `Bgrx8888` and stride `width * 4`.
- Frames are paced to `capture.fps`; zero disables pacing.
- The root geometry is read at connect time and again on every `retarget`. `retarget` with `CaptureArea::Region` captures exactly that rectangle of the root window: frames from then on have the rectangle's size, and the shared segment is replaced by a larger one when the rectangle needs more bytes than it holds. A rectangle that is empty, starts at a negative coordinate or reaches outside the root window fails with `InvalidTarget` and the previous target stays. `CaptureArea::Whole` returns to the full root window, and the frame rate of the target sets the pacing.
- Because the segment is passed as a file descriptor, the server must be reachable through a unix socket. A TCP connection cannot use this backend.

Status: verified against Xvfb with painted content (see [testing.md](testing.md)). Not yet run against a real interactive X11 session.

## Wayland backend (`adapters/capture/wayland_portal.rs`)

`WaylandPortalCapture` uses the xdg-desktop-portal ScreenCast interface through `ashpd` and consumes frames with `pipewire-rs`. It is compiled only with the cargo feature `capture-wayland`, which needs the libpipewire-0.3 and libspa-0.2 development files plus clang for bindgen. Without the feature the type exists and `connect` returns `BackendUnavailable`.

With the feature enabled:

- `connect(width, height, fps)` starts a worker thread. The thread opens a portal session, selects a monitor source with the cursor embedded and no persisted permission, and starts the session. The compositor shows its own share dialog at this point.
- The thread then connects a PipeWire stream to the node the portal returned, requesting `BGRx`, `BGRA`, `RGBx` or `RGBA` at a size range with `display.width` by `display.height` as the preferred size.
- The process callback copies each buffer into a `Frame` and pushes it into a queue of two frames. When the consumer lags, new frames are dropped. Frames are rate limited to `capture.fps`.
- `next_frame` receives from the queue and returns `CaptureError::Stream` once the stream has ended.

Status: the portal half type-checks against ashpd 0.14. The PipeWire half is compiled by CI with the PipeWire development packages installed; it has not been run, because running it needs a Wayland compositor with a portal and PipeWire. Requires on-session validation (Stage 2 in [testing.md](testing.md)).

## Command line

```text
screendockd [--config PATH] capture [--frames N] [--out-dir DIR] [--backend auto|x11|wayland]
```

The command prints the detected session type and the selected backend, captures `N` frames (default 5) and writes `frame-0000.png`, `frame-0001.png` and so on into `DIR` (default the current directory, created if missing). It needs no dock.

## Relation to the panel size

Backends deliver frames at the source size. Scaling to the panel resolution is the encoder's job. `display.width` and `display.height` are passed to the Wayland backend as the preferred stream size and are printed by the `capture` subcommand as the panel target.
