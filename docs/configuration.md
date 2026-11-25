# Configuration

## Config file

`screendockd` reads one TOML file named `screendock_cfg.toml`. The first existing file in this order is used:

1. The path in the `SCREENDOCK_CONFIG_PATH` environment variable, if the file exists.
2. `$XDG_CONFIG_HOME/screendockd/screendock_cfg.toml` (default `~/.config/screendockd/screendock_cfg.toml`).
3. `/etc/screendockd/screendock_cfg.toml`, the template shipped by the packages.
4. `config/screendock_cfg.toml` at the repository root, for running from a checkout.

The command line option `--config PATH` bypasses this order and loads that file directly.

## Fields

| Key | Type | Default | Used by |
|---|---|---|---|
| `display.width` | integer | 1280 | Encoded picture width in pixels; written into the header of every video message and used as the encoder output width. Passed to the Wayland backend as the preferred stream width. Overridable per run with `--width` for `replay` and `isolate`. Even, 16 to 4096 |
| `display.height` | integer | 720 | Encoded picture height in pixels; the header height of every video message, the encoder output height and the Wayland preferred stream height. Even, 16 to 4096 |
| `display.orientation` | string | `landscape` | `landscape` (width at least height) or `portrait` (height at least width). Checked against the size; not a rotation. See [daemon.md](daemon.md#resolution) |
| `encode.preset` | string | `ultrafast` | libx264 preset: `ultrafast`, `superfast`, `veryfast`, `faster`, `fast` or `medium`; any other value is a config error |
| `encode.tune_zerolatency` | boolean | true | Apply the libx264 `zerolatency` tune |
| `encode.bitrate_kbps` | integer | 4000 | libx264 target bit rate in kilobits per second; also the VBV maximum rate |
| `encode.keyframe_interval` | integer | 50 | Frames between keyframes |
| `encode.vbv_buffer_ms` | integer | 100 | VBV buffer in milliseconds of `encode.bitrate_kbps` (100 at 4000 gives 400 kbit); bounds the size of any one frame; 0 turns the cap off |
| `virtual_display.mode` | string | `mirror` | `mirror` captures an existing screen; `extend` creates a monitor for the dock and captures exactly that monitor. Live key |
| `virtual_display.placement` | string | `right-of` | `right-of`, `left-of`, `above`, `below` or `at`, relative to the anchor. Applied on X11 and sway; GNOME and the portal choose their own layout position. Live key |
| `virtual_display.anchor` | string | `primary` | Output name the placement is relative to, or `primary`. Live key |
| `virtual_display.x`, `virtual_display.y` | integer | 0 | Monitor position when `placement` is `at`. Within plus or minus 32767. Live keys |
| `virtual_display.name` | string | empty | Monitor name, printable ASCII up to 64 bytes. Empty uses the name from the dock's EDID, else `ScreenDock`. Live key |
| `virtual_display.fallback` | string | `mirror` | `mirror` continues in mirror mode and reports the reason when the monitor cannot be created; `fail` ends startup with an error. Read when the monitor is created |
| `virtual_display.wayland_backend` | string | `auto` | `auto`, `mutter`, `portal` or `wlroots`. `auto` picks Mutter on GNOME, a headless output on sway, and the portal where it offers a virtual source. Live key |
| `virtual_display.follow_system_changes` | boolean | true | Follow size, rotation and position changes made in the desktop's display settings. Live key |
| `virtual_display.edid` | string | `dock` | `dock` uses the dock's EDID with synthetic values when the dock sent none; `synthetic` always uses synthetic values. Read when the monitor is created |
| `virtual_display.restore_on_exit` | boolean | true | Restore the previous layout when the daemon exits. Read when the monitor is created |
| `control.enabled` | boolean | true | Serve the control socket |
| `control.socket` | string | empty | Socket path. Empty uses `$XDG_RUNTIME_DIR/screendockd/control.sock` |
| `capture.backend` | string | `auto` | `auto`, `x11` or `wayland`. `auto` follows the detected session (`$XDG_SESSION_TYPE`, then `$WAYLAND_DISPLAY`, then `$DISPLAY`). `wayland` needs a build with the `capture-wayland` feature |
| `capture.x11_display` | string | unset | X11 display to capture, for example `:0`. Unset uses `$DISPLAY` |
| `capture.fps` | integer | 25 | Capture frame rate. 0 captures as fast as the backend allows |
| `daemon.log_level` | string | `info` | Default log filter: `trace`, `debug`, `info`, `warn` or `error` |
| `daemon.heartbeat_interval_ms` | integer | 1000 | Heartbeat interval in milliseconds |
| `device.vendor_id_hex` | string | `1de1` | USB vendor ID that discovery matches. Four hex digits, optional `0x` |
| `device.product_id_hex` | string | `f201` | USB product ID that discovery matches. Four hex digits, optional `0x` |

## Live keys

The keys marked live in the table above, together with `display.width`, `display.height`, `display.orientation`, `capture.fps`, `encode.preset`, `encode.tune_zerolatency`, `encode.bitrate_kbps`, `encode.vbv_buffer_ms` and `encode.keyframe_interval`, are the settings the daemon can change while it runs. The `[virtual_display]` and `[control]` sections are optional; without them the daemon mirrors and serves the control socket. The monitor size in extend mode is `display.width` by `display.height`. The class of each change is described in [architecture.md](architecture.md#live-settings-and-change-classes).

## Environment variables

| Variable | Effect |
|---|---|
| `SCREENDOCK_CONFIG_PATH` | Config file path, first in the resolution order |
| `RUST_LOG` | Log filter; takes precedence over `daemon.log_level` when set |

## Command line

```text
screendockd [--config PATH] <command>

discover
replay  [--mi00 PATH] [--mi01 PATH] [--width N] [--height N] [--h264-file PATH] [--iterations N]
        [--no-handshake]
isolate --mode heartbeat|video|both [--mi00 PATH] [--mi01 PATH] [--width N] [--height N] [--no-handshake]
        [--h264-file PATH] [--duration-secs N] [--fps N]
capture [--frames N] [--out-dir DIR] [--backend auto|x11|wayland]
run     [--mi00 PATH] [--mi01 PATH] [--h264-file PATH] [--backend auto|x11|wayland]
        [--max-frames N] [--no-handshake]
bench   [--h264-file PATH] [--backend auto|x11|wayland] [--duration-secs N]
```

| Option | Default | Meaning |
|---|---|---|
| `--mi00`, `--mi01` | discovered | Use this path instead of sysfs discovery for that node. A regular file works for dry runs |
| `--width` | `display.width` | Picture width in pixels for the video header |
| `--height` | `display.height` | Picture height in pixels for the video header |
| `--no-handshake` | off | `run`, `replay` and `isolate`: skip the dock session handshake and the MI_00 reader, send only heartbeats and send video immediately. For regular files standing in for the nodes; a real dock discards video until the handshake has run |
| `--h264-file` | synthetic messages for `replay` and `isolate`; libx264 for `run` and `bench` | Annex-B H.264 file to send instead of filler messages (`replay`, `isolate`), or to replay through the test encoder instead of libx264 (`run`, `bench`) |
| `--iterations` | 20 | Times `replay` repeats its message set |
| `--mode` | required | `isolate` condition |
| `--duration-secs` | 30 | `isolate` run length |
| `--fps` | 25 | `isolate` video frame rate |
| `--frames` | 5 | Frames `capture` writes as `frame-NNNN.png` |
| `--out-dir` | current directory | Directory for `capture` output, created if missing |
| `--backend` | `capture.backend` | Capture backend for this run |
| `--max-frames` | unlimited | `run` stops after this many frames |

Discovery runs only when a node needed by the command was not given on the command line. Heartbeat-only runs never open MI_01 and video-only runs never open MI_00. `replay` opens MI_00 as well, to run the handshake, unless `--no-handshake` is given. `capture` and `bench` never touch the dock; see [capture.md](capture.md) and [encoding.md](encoding.md#cpu-measurement). `run` is described in [daemon.md](daemon.md). `bench` takes `--duration-secs` like `isolate`.

## Permissions

The dock's hidraw nodes need to be readable and writable by the user running the daemon. `deploy/99-screendock.rules` grants the `screendock` group access to the dock's hidraw nodes. Install it to `/etc/udev/rules.d/`, create the group, add the user, and reload:

```text
sudo groupadd --system screendock
sudo usermod -aG screendock "$USER"
sudo cp deploy/99-screendock.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Log out and back in for the group change to apply.
