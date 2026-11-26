# Installation

This page covers installing, permissions, the systemd user unit and verification. The root `README.md` has the short version.

## Packages

CI builds a `.deb`, an `.rpm` and a `.crate` for each version and attaches them to the matching `v<version>` release. The `.deb` and `.rpm` ship:

| Path | Content |
|---|---|
| `/usr/bin/screendockd` | The binary, built with `encode-ffmpeg` |
| `/lib/udev/rules.d/99-screendock.rules` (deb), `/usr/lib/udev/rules.d/99-screendock.rules` (rpm) | The udev rule |
| `/usr/lib/systemd/user/screendockd.service` | The systemd user unit |
| `/etc/screendockd/screendock_cfg.toml` | Template config, third in the [resolution order](configuration.md#config-file) |

The packages are built with the `encode-ffmpeg` and `capture-wayland` features. The runtime libraries come from the distribution: dependencies are derived from the binary for the `.deb`, which also names `libpipewire-0.3-0t64` or `libpipewire-0.3-0` explicitly, and the `.rpm` requires `ffmpeg-libs` and `pipewire-libs`. Both packages recommend `xdg-desktop-portal`, which a Wayland desktop uses to hand out screen streams. A Wayland session also needs a running PipeWire and a portal backend for its compositor (`xdg-desktop-portal-gnome`, `xdg-desktop-portal-wlr` or `xdg-desktop-portal-kde`); the packages do not require them.

## Permissions

The rule matches vendor `1de1` and product `f201` on the `hidraw` subsystem, sets mode `0660`, group `screendock`, and tag `uaccess`. Packages do not create the group. Create it and add your user once:

```text
sudo groupadd --system screendock
sudo usermod -aG screendock "$USER"
```

For a build from source also install the rule:

```text
sudo cp deploy/99-screendock.rules /etc/udev/rules.d/99-screendock.rules
sudo udevadm control --reload-rules
sudo udevadm trigger
```

Log out and back in, replug the dock, and confirm access with `screendockd discover`. If you changed `device.vendor_id_hex` or `device.product_id_hex` for a test setup, the rule still matches only the dock's real IDs.

## Service

The unit is `PartOf` and `WantedBy` `graphical-session.target`, runs `/usr/bin/screendockd run`, and restarts on failure after two seconds. `ExecReload` runs `screendockd ctl reload`, and `ExecStopPost` runs `screendockd vdisplay cleanup` so a monitor left by a killed daemon is removed; both need the same `DISPLAY` and `XAUTHORITY` as the daemon, and cleanup does nothing when the X server is gone. The daemon exits with an error after 100 consecutive failed frames so that this restart takes effect.

```text
systemctl --user daemon-reload
systemctl --user enable --now screendockd.service
systemctl --user status screendockd.service
journalctl --user -u screendockd.service -f
```

The service needs the session environment for capture. On X11 it needs `DISPLAY` (and `XAUTHORITY` if the X server requires it). Most desktops import these into the user manager at login. If they are missing, import them from a terminal inside the session:

```text
systemctl --user import-environment DISPLAY XAUTHORITY
systemctl --user restart screendockd.service
```

To run a build from source as a service, install the binary to `~/.local/bin` and a unit whose `ExecStart` points at it:

```text
install -Dm755 src/screendockd/target/release/screendockd ~/.local/bin/screendockd
mkdir -p ~/.config/systemd/user
sed 's#/usr/bin/screendockd#%h/.local/bin/screendockd#' deploy/screendockd.service > ~/.config/systemd/user/screendockd.service
systemctl --user daemon-reload
systemctl --user enable --now screendockd.service
```

A unit in `~/.config/systemd/user/` takes precedence over the packaged one.

## Build from source

See the root `README.md` for the feature table. Build requirements for each feature are listed in [encoding.md](encoding.md#libx264-encoder-feature-encode-ffmpeg) and [capture.md](capture.md#wayland-backend-wayland_portalrs). Quality checks are in [testing.md](testing.md).

## Verification

1. `screendockd --version` prints the installed version.
2. `screendockd discover` prints the two hidraw nodes when the dock is connected and permissions are right.
3. `screendockd capture --frames 1 --out-dir /tmp/cap` writes a PNG of the desktop and needs no dock.
4. `screendockd run` starts the daemon in the foreground.

## Removal

```text
systemctl --user disable --now screendockd.service
sudo apt remove screendockd
sudo rm -f /etc/udev/rules.d/99-screendock.rules
```

Use `sudo dnf remove screendockd` for the RPM. The `screendock` group is left in place.
