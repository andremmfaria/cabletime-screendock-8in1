# Installation

This page covers installing, permissions, the system terminal on the dock, the systemd user unit and verification. The root `README.md` has the short version.

## Packages

CI builds a `.deb`, an `.rpm` and a `.crate` for each version and attaches them to the matching `v<version>` release. The `.deb` and `.rpm` ship:

| Path | Content |
|---|---|
| `/usr/bin/screendockd` | The binary, built with `encode-ffmpeg` |
| `/lib/udev/rules.d/99-screendock.rules` (deb), `/usr/lib/udev/rules.d/99-screendock.rules` (rpm) | The udev rule |
| `/usr/lib/systemd/user/screendockd.service` | The systemd user unit |
| `/usr/lib/systemd/system/screendock-xorg.service`, `screendock-terminal.service`, `screendockd-system.service` | The system terminal units, see [System terminal](#system-terminal) |
| `/lib/udev/rules.d/98-screendock-system.rules` (deb), `/usr/lib/udev/rules.d/98-screendock-system.rules` (rpm) | The udev rule that starts the system terminal when the dock is plugged in |
| `/etc/screendockd/xorg.conf`, `/etc/screendockd/xorg.conf.d/` | X server config of the system terminal (a conffile in the `.deb`, a noreplace config file in the `.rpm`) and an empty config directory |
| `/usr/libexec/screendockd/xorg-start`, `terminal-start` | The launchers the units call |
| `/etc/screendockd/screendock_cfg.toml` | Template config, third in the [resolution order](configuration.md#config-file) |
| `/usr/share/doc/screendockd/` | `README.md`, `install.md`, `configuration.md`, `control.md` and `virtual-display.md` |

On GNOME under Wayland, extend mode needs a running PipeWire and the Mutter ScreenCast service of the session, which a normal GNOME session provides; no further package is needed. The portal path needs `xdg-desktop-portal` and a portal backend for the desktop.

The binary in both packages starts without further setup besides its libraries: `screendockd --version`, `--help`, `ctl --help` and `vdisplay probe` (with `--config` pointing at the template) run from an extracted package root. `deploy/check-packages.sh <dir>` verifies the payload, modes, the PipeWire and X server dependencies (`libpipewire-0.3-0t64 | libpipewire-0.3-0` in the `.deb`, `libpipewire-0.3.so.0` and `pipewire-libs` in the `.rpm`), the config files, the maintainer scripts and scriptlets, the unit lines and that start, and runs in the package workflow. `deploy/check-deploy.sh` runs the static checks of the system terminal files (launcher syntax and dry runs, `udevadm verify` of the activation rule and `systemd-analyze verify` of the three units) on the files in `deploy/` or on an extracted package root, and runs in CI.

The packages are built with the `encode-ffmpeg` and `capture-wayland` features. The runtime libraries come from the distribution: dependencies are derived from the binary for the `.deb`, which also names `libpipewire-0.3-0t64` or `libpipewire-0.3-0` explicitly, and the `.rpm` requires `ffmpeg-libs` and `pipewire-libs`. Both packages recommend `xdg-desktop-portal`, which a Wayland desktop uses to hand out screen streams. The system terminal adds dependencies on an X server with the dummy driver and libinput, `xauth`, `xset`, `xsetroot`, `xterm`, `login` and `util-linux`: `xserver-xorg-core`, `xserver-xorg-video-dummy`, `xserver-xorg-input-libinput`, `xauth`, `x11-xserver-utils`, `xterm`, `login` and `util-linux` in the `.deb`, and `xorg-x11-server-Xorg`, `xorg-x11-drv-dummy`, `xorg-x11-drv-libinput`, `/usr/bin/xauth`, `/usr/bin/xset`, `/usr/bin/xsetroot`, `/usr/bin/xterm` and `util-linux` in the `.rpm`. A Wayland session also needs a running PipeWire and a portal backend for its compositor (`xdg-desktop-portal-gnome`, `xdg-desktop-portal-wlr` or `xdg-desktop-portal-kde`); the packages do not require them.

## Permissions

The rule matches vendor `1de1` and product `f201` on the `hidraw` subsystem, sets mode `0660`, group `screendock`, and tag `uaccess`. The packages create the `screendock` group and the `screendock` system user. A desktop user who runs the daemon in their own session still needs to be in the group. Add your user once (for a build from source, create the group first with `sudo groupadd --system screendock`):

```text
sudo usermod -aG screendock "$USER"
```

For a build from source also install the rule:

```text
sudo cp deploy/99-screendock.rules /etc/udev/rules.d/99-screendock.rules
sudo udevadm control --reload-rules
sudo udevadm trigger
```

Log out and back in, replug the dock, and confirm access with `screendockd discover`. If you changed `device.vendor_id_hex` or `device.product_id_hex` for a test setup, the rule still matches only the dock's real IDs.

## System terminal

After installing a package and plugging the dock, a login prompt appears on the dock panel without any configuration. Three system services provide it:

| Unit | What it does |
|---|---|
| `screendock-xorg.service` | Runs Xorg as the `screendock` user on display `:70` with the dummy video driver (1280x720 at 60 Hz) and libinput for keyboard and pointer, through `/usr/libexec/screendockd/xorg-start`. The X authority file is `/run/screendock/Xauthority` and the log is `/var/log/screendock/xorg.log`. |
| `screendock-terminal.service` | Runs `xterm` (98 columns by 27 rows, green on black) with `login` as root on that display and restarts it after each logout, so the prompt returns. `login` runs under `setsid -wc`, which gives it its own session with the xterm pseudo terminal as controlling terminal; without that, `login` exits at once. |
| `screendockd-system.service` | Runs `screendockd run --backend x11` as the `screendock` user on that display, which mirrors it to the dock. |

The udev rule `98-screendock-system.rules` matches the hidraw node of USB interface 0 of device `1de1:f201`, tags it for systemd, gives it the alias `/dev/screendock` and sets `SYSTEMD_WANTS` to the daemon and terminal services. `screendock-xorg.service` is bound to `dev-screendock.device`, so unplugging the dock stops all three services, and plugging it in (or booting with it plugged in) starts them. The units have no `[Install]` section; the dock activates them. The rule takes effect for the dock when the package is installed, because the maintainer script reloads udev and re-triggers hidraw devices.

To log in, type a user name and password on a keyboard attached to the machine. The X server reads keyboards through libinput, so it needs read access to `/dev/input/event*`, which the `input` group has; the `screendock` user is created as a member of `input`. The keyboard is shared with the rest of the system, so keystrokes reach the dock terminal and any other session that reads the same keyboard. Logging out returns to the prompt.

The `screendock` user is a system account with no home directory and the `nologin` shell. The daemon of the system terminal keeps its control socket in `/run/screendockd/screendockd/control.sock`. To use `ctl` against it, run it as root or as `screendock` with `XDG_RUNTIME_DIR=/run/screendockd`:

```text
sudo XDG_RUNTIME_DIR=/run/screendockd screendockd ctl status
```

Logs:

```text
journalctl -u screendock-xorg.service -u screendock-terminal.service -u screendockd-system.service -f
less /var/log/screendock/xorg.log
```

Only one daemon can drive the dock. A desktop that runs the user unit `screendockd.service` and the system terminal both open the same hidraw nodes. The system terminal is enabled by default; to turn it off, mask the two services that act on the dock:

```text
sudo systemctl mask screendock-terminal.service screendockd-system.service
sudo systemctl unmask screendock-terminal.service screendockd-system.service
```

The first command opts out and the second opts back in. With both masked, `screendock-xorg.service` is never pulled in by the dock. The X server config is `/etc/screendockd/xorg.conf`; extra `*.conf` snippets go in `/etc/screendockd/xorg.conf.d/`, which is used instead of the distribution's `xorg.conf.d` snippets. Display, size and font are fixed in the unit and launcher files.

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

See the root `README.md` for the feature table. Build requirements for each feature are listed in [encoding.md](encoding.md#libx264-encoder-feature-encode-ffmpeg) and [capture.md](capture.md#wayland-backend-adapterscapturewayland_portalrs). Quality checks are in [testing.md](testing.md).

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

Removing the package stops the three system terminal services and reloads systemd and udev. Use `sudo apt purge screendockd` to also delete the `screendock` user, the `screendock` group and `/var/log/screendock`. Use `sudo dnf remove screendockd` for the RPM; it leaves the `screendock` user and group in place, and `sudo userdel screendock` and `sudo groupdel screendock` remove them.
