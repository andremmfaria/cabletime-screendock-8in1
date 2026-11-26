# Control

A running `screendockd run` serves a control socket. `screendockd ctl` is the client; any program that speaks the line format below can use the socket as well.

## Socket

The socket path is `control.socket` from the config, or `$XDG_RUNTIME_DIR/screendockd/control.sock` when that is empty. `--socket PATH` on `ctl` overrides both. With `control.enabled = false`, or when neither a configured path nor `$XDG_RUNTIME_DIR` exists, the daemon serves no socket.

- The daemon creates a missing parent directory with mode 0700 and gives the socket mode 0600, so only the owning user can connect.
- A socket file left by a daemon that is gone is replaced at startup. A socket that still answers makes startup fail with an error, and a path that exists and is not a socket is never removed.
- The path must fit a Unix socket address (107 bytes).
- The daemon removes the socket file when it exits normally.

## Wire format

One connection carries one request and one response, each a single JSON object on one line ending in a newline. A request line is limited to 4096 bytes, including the newline; a longer line is answered with `invalid` and never reaches the daemon. Unknown fields are rejected. Requests carry a `cmd` field.

| Request | Fields |
|---|---|
| `{"cmd":"status"}` | none |
| `{"cmd":"get"}` or `{"cmd":"get","key":"bitrate"}` | optional `key` |
| `{"cmd":"set","key":"bitrate","value":"2000"}` | `key`, `value` |
| `{"cmd":"reload"}` | none |

Responses carry a `result` field.

| Response | Fields | Meaning |
|---|---|---|
| `status` | `fields`: map of text to text | The daemon's state and every setting |
| `values` | `values`: map of text to text | The requested settings |
| `applied` | `class`, `changed`: list of setting names | The change was applied |
| `rejected` | `reason`, `rolled_back` | A valid request the daemon did not apply |
| `invalid` | `reason` | The request was not understood, names an unknown setting or holds a value that is out of range |

Request text from clients is never used as a path or format string. Names echoed in an error are cut to 40 characters.

## Commands

```text
screendockd [--config PATH] ctl [--socket PATH] status
screendockd [--config PATH] ctl [--socket PATH] get [KEY]
screendockd [--config PATH] ctl [--socket PATH] set KEY VALUE
screendockd [--config PATH] ctl [--socket PATH] reload
```

`status` prints `name: value` lines: every setting plus `version`, `pid`, `changes_applied` and `state`, which is `running` or `degraded`, with `degraded_reason` when degraded. `get` without a key prints every setting in the same form; `get KEY` prints only the value. `set` and `reload` print `applied (CLASS): NAMES` with the settings that changed, or `applied (CLASS): nothing to change`. A command that fails prints its reason on standard error. A value that starts with a minus sign, such as `set x -20`, is read as a value.

| Exit code | Meaning |
|---|---|
| 0 | The request succeeded |
| 1 | `ctl` itself failed (no socket path, unreadable config, protocol error) |
| 2 | The daemon rejected the request |
| 3 | No daemon answers on the socket |
| 4 | The request was invalid |

## Changing settings

`set` and `reload` go through the supervisor, the one owner of the live settings in the daemon, in this order:

1. The key and value are read and the new settings are validated as a whole. A bad key, a value of the wrong type, a value out of range or a size that is odd or disagrees with the orientation gives `invalid` (exit code 4) and changes nothing.
2. The change is classified (see [architecture.md](architecture.md#live-settings-and-change-classes)) into the steps it needs.
3. If the running pipeline can do every step, they are executed as a transaction (see below) and the answer is `applied` with the class and the names of the settings that changed. Setting a key to its current value is `applied` with no names.
4. Otherwise the answer is `rejected` (exit code 2) with the steps the pipeline cannot do, and the settings stay as they were.

The settings of the running daemon are not written to the config file; they last until the daemon exits. Requests are handled one at a time in arrival order.

The running pipeline executes these changes: `bitrate`, `vbv-buffer-ms` and `keyframe-interval` on the running encoder, the follow setting, and the settings that only matter in extend mode (`placement`, `anchor`, `x`, `y`, `name` and `wayland-backend`) while the mode is `mirror`, which are recorded for the next switch to extend. Changes to `resolution`, `orientation`, `fps`, `preset` and `tune` rebuild the encoder in the video loop, retarget capture when the frame rate changes, and for a new picture size run the dock cycle (see [daemon.md](daemon.md#loops)). They are rejected when the daemon runs with `--no-handshake`, because there is no session to cycle. A change to `mode` needs steps the pipeline cannot do and is rejected.

The transaction has three stages. Prepare builds the new encoder aside and encodes one frame with it; an error here changes nothing. Commit puts the change into effect: the video loop swaps the encoder, retargets capture and runs the dock cycle. Verify watches the pipeline for 1 second: the session must not be lost, must be armed at the end, and at least 3 video messages must have been written since the change. A change that fails at commit or verify, and is more than live, is rolled back: the previous settings are put back, which for a size change runs the dock cycle again at the previous size. The answer is `rejected` with the reason and `rolled_back` true (`ctl` prints `rejected (rolled back): REASON`), and the live settings stay at the previous values. A live change has nothing to undo, so a refusal of one reports `rolled_back` false.

If the rollback fails as well, the daemon is degraded. The answer is `rejected` with `rolled_back` false and a reason that names both failures. The live settings stay at the last values that worked, `status` shows `state: degraded` and `degraded_reason`, and nothing is retried on its own. The next `set` or `reload` first brings the pipeline back to the last good settings with a dock cycle at that size; when that fails the answer is `rejected` and the daemon stays degraded, and when it works `state` returns to `running` and the command goes on. `status` shows `state: running` otherwise.

The video loop applies a bit rate or keyframe interval change at a frame boundary and the supervisor waits up to 3 seconds for it; a change that rebuilds the encoder or runs the dock cycle gets up to 15 seconds. If the encoder refuses the change, the answer is `rejected` with the encoder's reason and the previous values stay. No dock handshake runs, no `app-launched` is sent and the heartbeat is not interrupted.

### Class of every key

| Key | Class | Steps |
|---|---|---|
| `bitrate`, `vbv-buffer-ms` | live when the encoder changes its rate in place, else rebuild | set the encoder rate, or rebuild the encoder |
| `keyframe-interval` | live | change the keyframe clock |
| `fps`, `preset`, `tune` | rebuild | rebuild the encoder |
| `resolution`, `orientation` | dock cycle | rebuild the encoder, retarget capture and resize the monitor in extend mode, run the dock cycle |
| `mode` | rebuild, dock cycle when the picture size changes too | create or destroy the monitor, rebuild the encoder, retarget capture |
| `placement`, `anchor`, `x`, `y` | live in extend mode, no step in mirror mode | retarget capture |
| `name`, `wayland-backend` | rebuild in extend mode, no step in mirror mode | recreate the monitor, retarget capture, rebuild the encoder |
| `follow-system-changes` | live | none |

The libx264 encoder changes its rate in place when the linked libavcodec applies the change to the running encoder. The daemon measures this once at startup by encoding noise frames, lowering the rate and checking that the frames get clearly smaller. When it does not hold, `bitrate` and `vbv-buffer-ms` are rebuild class and are rejected by the running pipeline. The prerecorded test encoder accepts rate changes without any effect.

## Reload and SIGHUP

`ctl reload` and SIGHUP both make the daemon read the config file again: the file given with `--config`, or the first file of the resolution order in [configuration.md](configuration.md#config-file). The new file's settings are compared with the live settings, and only the differences are applied through the same path as `set`, as one change: if any difference is rejected, nothing is applied. This also reverts a setting changed with `set` to the file's value. A file that cannot be read or parsed is reported as `rejected` with the reason and changes nothing. SIGHUP has no client to answer, so its result is logged: `config reloaded` with the class and the changed names at info level, or `config reload failed` with the reason at warn level.

## Read-only keys

`status` and `get` also carry facts about the dock's panel. They cannot be set; `ctl set` with one of these names is invalid. `get` without a key lists them with the settings, and `get` with one of these names returns just that value.

| Key | Value |
|---|---|
| `edid-source` | `dock` when the dock sent its EDID in this run, `cache` when only an earlier run's EDID is stored, `config` when neither exists or `virtual_display.edid` is `synthetic` |
| `edid-name` | The monitor name from the EDID, or `ScreenDock` |
| `edid-native` | The preferred mode as `WIDTHxHEIGHT@HZ`, or just the size when the refresh rate is unknown (the `config` source) |
| `edid-modes` | The mode list, comma separated, in the same notation: detailed timings, then standard timings |

## Settings

| Key | Value | Setting |
|---|---|---|
| `resolution` | `WIDTHxHEIGHT` | `display.width` and `display.height` |
| `orientation` | `landscape` or `portrait` | `display.orientation` |
| `fps` | whole number, 0 to 240 | `capture.fps` |
| `preset` | libx264 preset name | `encode.preset` |
| `tune` | `true` or `false` | `encode.tune_zerolatency` |
| `bitrate` | kbit/s, 1 to 100000 | `encode.bitrate_kbps` |
| `vbv-buffer-ms` | milliseconds, 0 to 10000 | `encode.vbv_buffer_ms` |
| `keyframe-interval` | frames, 1 to 3600 | `encode.keyframe_interval` |
| `mode` | `mirror` or `extend` | `virtual_display.mode` |
| `placement` | `right-of`, `left-of`, `above`, `below` or `at` | `virtual_display.placement` |
| `anchor` | output name or `primary` | `virtual_display.anchor` |
| `x`, `y` | whole number within plus or minus 32767 | `virtual_display.x`, `virtual_display.y` |
| `name` | printable ASCII up to 64 bytes | `virtual_display.name` |
| `wayland-backend` | `auto`, `mutter`, `portal` or `wlroots` | `virtual_display.wayland_backend` |
| `follow-system-changes` | `true` or `false` | `virtual_display.follow_system_changes` |
