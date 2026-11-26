# screendockd

An independent, unofficial Linux userspace driver for the CABLETIME ScreenDock (CB09W, USB 1de1:f201). This project is not affiliated with, endorsed by, or supported by CableTime or Actions Microelectronics. Product names are used only to identify the hardware this software works with.

## What it is

The ScreenDock is a USB-C dock with a built-in 5.5 inch 1280x720 panel. The panel is not a standard display: it receives H.264 video through a vendor HID interface, and the vendor ships drivers only for Windows and macOS. `screendockd` talks to the dock directly through Linux hidraw. It captures the desktop, encodes it with libx264, frames it for the dock's wire protocol, and sends it together with the control channel heartbeat. The wire protocol was worked out from USB captures and public documentation; see [docs/](docs/README.md).

Status: the code is complete and tested against files, pipes and a virtual X11 display, but it has not been validated on a real dock. The procedures for doing so are in [docs/hardware-validation.md](docs/hardware-validation.md). Expect rough edges until that is done.

## Requirements

- Linux with hidraw and udev. The dock must be connected through its HOST USB-C port.
- An X11 session for screen capture. Wayland capture exists behind the `capture-wayland` build feature and has not been run on a real session.
- To build from source: a Rust toolchain (stable). For the libx264 encoder (feature `encode-ffmpeg`): the libav development files (`libavcodec-dev`, `libavformat-dev`, `libavutil-dev`, `libswscale-dev`, `libavdevice-dev`), `libx264-dev`, `clang` and `pkg-config` on Debian and Ubuntu. For the Wayland backend (feature `capture-wayland`): `libpipewire-0.3-dev`, `libspa-0.2-dev` and `clang`.

## Build from source

```text
git clone https://github.com/andremmfaria/cabletime-screendock-8in1.git
cd cabletime-screendock-8in1/src/screendockd
cargo build --release --features encode-ffmpeg
```

The binary is `target/release/screendockd`. Cargo features:

| Feature | Adds | Extra build requirements |
|---|---|---|
| `encode-ffmpeg` | The libx264 encoder. Without it `run` needs `--h264-file` | libav development files, libx264, clang |
| `capture-wayland` | Wayland capture through xdg-desktop-portal and PipeWire | libpipewire and libspa development files, clang |

The default build (no features) captures X11, replays prerecorded H.264 files and runs every validation command, using only pure Rust dependencies.

## Install from a package

Each tagged release has a `.deb` and an `.rpm` attached on the [releases page](https://github.com/andremmfaria/cabletime-screendock-8in1/releases). The packages are built with `encode-ffmpeg`.

```text
sudo apt install ./screendockd_<version>_amd64.deb
sudo dnf install ./screendockd-<version>-1.x86_64.rpm
```

A package installs `/usr/bin/screendockd`, the udev rule, the systemd user unit at `/usr/lib/systemd/user/screendockd.service`, and a template config at `/etc/screendockd/screendock_cfg.toml`. The RPM needs `ffmpeg-libs` (RPM Fusion on Fedora). Packages do not create the `screendock` group or add you to it; do that in the next section.

## Permissions: udev rule and group

`deploy/99-screendock.rules` gives the `screendock` group read and write access to the dock's hidraw nodes (and tags them `uaccess` for the active local seat). Packages already install the rule. From source, install it by hand. Then create the group, add your user, and reload udev:

```text
sudo groupadd --system screendock
sudo usermod -aG screendock "$USER"
sudo cp deploy/99-screendock.rules /etc/udev/rules.d/99-screendock.rules
sudo udevadm control --reload-rules
sudo udevadm trigger
```

Log out and back in so the group change applies, then replug the dock.

## Run as a systemd user service

The package installs the unit. Enable and start it:

```text
systemctl --user daemon-reload
systemctl --user enable --now screendockd.service
journalctl --user -u screendockd.service -f
```

The unit runs `/usr/bin/screendockd run` and restarts it on failure. For a build from source, install the binary and a unit that points at it:

```text
install -Dm755 target/release/screendockd ~/.local/bin/screendockd
mkdir -p ~/.config/systemd/user
sed 's#/usr/bin/screendockd#%h/.local/bin/screendockd#' ../../deploy/screendockd.service > ~/.config/systemd/user/screendockd.service
systemctl --user daemon-reload
systemctl --user enable --now screendockd.service
```

## Configuration

The first existing file in this order is used:

1. The path in `SCREENDOCK_CONFIG_PATH`.
2. `$XDG_CONFIG_HOME/screendockd/screendock_cfg.toml` (default `~/.config/screendockd/screendock_cfg.toml`).
3. `/etc/screendockd/screendock_cfg.toml`, the packaged template.
4. `config/screendock_cfg.toml` in the repository, for running from a checkout.

`--config PATH` bypasses the order. To customize, copy the template to the per-user path and edit it. Every field is described in [docs/configuration.md](docs/configuration.md).

## Running

```text
screendockd run
```

This discovers the dock, starts the heartbeat, and streams the desktop until SIGINT or SIGTERM. Use `RUST_LOG=debug` for more logging. `screendockd --help` lists every command.

## Validation subcommands

| Command | Needs the dock | Purpose |
|---|---|---|
| `discover` | yes | Print the MI_00 (control) and MI_01 (video) hidraw nodes |
| `capture` | no | Grab frames from the desktop and write them as PNG files |
| `bench` | no | Measure CPU cost of capture plus encode |
| `replay` | yes | Write timing test on MI_01 |
| `isolate` | yes | Run heartbeat only, video only, or both, to observe the panel |

`run`, `replay` and `isolate` accept `--mi00` and `--mi01` paths, so a regular file can stand in for a node on a dry run. Details and recording instructions are in [docs/hardware-validation.md](docs/hardware-validation.md).

## Troubleshooting

- `failed to resolve dock hidraw nodes`: the dock is not connected or not enumerated. Check `lsusb | grep 1de1:f201`, use the HOST port with a data cable, and run `screendockd discover`.
- `Permission denied` opening a node: the udev rule is not installed, the group does not exist, or you have not logged in again after `usermod`. Check `groups` and `ls -l /dev/hidraw*`.
- `encoder ffmpeg unavailable`: the binary was built without `encode-ffmpeg`. Use a packaged build or rebuild with the feature.
- Capture fails with no session: `DISPLAY` is not set in the service environment. Run `systemctl --user import-environment DISPLAY XAUTHORITY` from your session, or set `capture.x11_display` in the config.
- The panel shows "Connecting Signal...": the heartbeat is not reaching MI_00. Check the log for write errors.
- Nothing appears on a Wayland session: build with `capture-wayland`, accept the compositor's share dialog, and see [docs/testing.md](docs/testing.md).

## Documentation and license

The documentation index is [docs/README.md](docs/README.md); installation details are in [docs/install.md](docs/install.md). The project is licensed under the terms in [LICENSE](LICENSE).
