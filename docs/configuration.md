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
| `display.width` | integer | 1280 | Target width in pixels; sets header field B (`width * 256`) for video. Overridable per run with `--width` |
| `display.height` | integer | 720 | Parsed, not yet consumed |
| `display.orientation` | string | `landscape` | Parsed, not yet consumed. `landscape` or `portrait` |
| `daemon.log_level` | string | `info` | Default log filter: `trace`, `debug`, `info`, `warn` or `error` |
| `daemon.heartbeat_interval_ms` | integer | 1000 | Heartbeat tick interval in milliseconds |
| `device.vendor_id_hex` | string | `1de1` | Parsed, not yet consumed |
| `device.product_id_hex` | string | `f201` | Parsed, not yet consumed |

## Environment variables

| Variable | Effect |
|---|---|
| `SCREENDOCK_CONFIG_PATH` | Config file path, first in the resolution order |
| `RUST_LOG` | Log filter; takes precedence over `daemon.log_level` when set |

## Command line

```text
screendockd [--config PATH] <command>

discover
replay  [--mi01 PATH] [--width N] [--h264-file PATH] [--iterations N]
isolate --mode heartbeat|video|both [--mi00 PATH] [--mi01 PATH] [--width N]
        [--h264-file PATH] [--duration-secs N] [--fps N]
```

| Option | Default | Meaning |
|---|---|---|
| `--mi00`, `--mi01` | discovered | Use this path instead of sysfs discovery for that node. A regular file works for dry runs |
| `--width` | `display.width` | Target width in pixels for field B |
| `--h264-file` | synthetic messages | Annex-B H.264 file to send instead of filler messages |
| `--iterations` | 20 | Times `replay` repeats its message set |
| `--mode` | required | `isolate` condition |
| `--duration-secs` | 30 | `isolate` run length |
| `--fps` | 25 | `isolate` video frame rate |

Discovery runs only when a node needed by the command was not given on the command line. Heartbeat-only runs never open MI_01 and video-only runs never open MI_00.

## Permissions

The dock's hidraw nodes need to be readable and writable by the user running the daemon. `deploy/99-screendock.rules` grants the `screendock` group access to the dock's hidraw nodes. Install it to `/etc/udev/rules.d/`, create the group, add the user, and reload:

```text
sudo groupadd screendock
sudo usermod -aG screendock "$USER"
sudo cp deploy/99-screendock.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Log out and back in for the group change to apply.
