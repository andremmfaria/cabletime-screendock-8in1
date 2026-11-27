# Testing

All commands run from `src/screendockd`.

## Layout

| Group | Files | Covers |
|---|---|---|
| `protocol` | `framing.rs`, `edid.rs`, `control_wire.rs` | Header and chunk layout, the EDID parser with hostile input, the control request and response lines |
| `session` | `dock.rs`, `reconfig.rs` | The handshake and loss handling against the simulated dock, live bit rate, keyframe interval and size changes |
| `capture` | `x11.rs`, `virtual_x11.rs`, `follow_x11.rs`, `virtual_sway.rs`, `virtual_mutter.rs` | Capture on Xvfb, virtual monitors on Xorg, sway and headless GNOME Shell, watcher sources |
| `e2e` | `daemon_xvfb.rs`, `control_socket.rs`, `vdisplay_xorg.rs`, `follow_xorg.rs`, `follow_sway.rs`, `follow_rig.rs` | The daemon binary, the control socket, mode switching and following outside display changes (`follow_rig.rs` is the shared daemon rig) |
| `encode` | `ffmpeg.rs` | The libx264 encoder, with `--features encode-ffmpeg` |

Every directory under `src/` except the crate root has a `types.rs` with its declarations.

## Environment variables

| Variable | Effect when set |
|---|---|
| `SCREENDOCK_REQUIRE_XVFB` | A missing or unusable Xvfb fails the X11 tests instead of skipping them |
| `SCREENDOCK_REQUIRE_XORG` | A missing or unusable rootless Xorg with the dummy driver fails the Xorg tests |
| `SCREENDOCK_REQUIRE_SWAY` | A sway that cannot start headless fails the sway tests |
| `SCREENDOCK_REQUIRE_MUTTER` | A missing or unusable headless GNOME Shell fails the Mutter tests |
| `SCREENDOCK_REQUIRE_FFMPEG` | A missing `ffmpeg` binary fails the decode checks |

## Gate set

The checks that run before every change, from `src/screendockd`: `cargo fmt --check`; `cargo clippy --all-targets -- -D warnings` with the default features, with `--no-default-features`, with `--features encode-ffmpeg`, with `--features capture-wayland` and with `--features encode-ffmpeg,capture-wayland`; `cargo test` and `cargo test --features encode-ffmpeg,capture-wayland` with the five variables above set; `cargo build --no-default-features`; `cargo build --release --features encode-ffmpeg,capture-wayland`. The layer rules in [architecture.md](architecture.md#layering-rules) are checked with a search for forbidden `crate::` imports in each layer; the only matches are the `#[cfg(test)]` imports of the adapters in `tools`.

## Test evidence by feature

What each feature rests on, as the tests that exercise it. Test names are given in short form; every one is in the file named.

| Feature | Evidence |
|---|---|
| Live settings | `core/reconfig/plan.rs` tests give the class and actions of every row of the change table, including combined changes. `tests/session/reconfig.rs`: `live_bitrate_and_keyframe_interval_changes_need_no_new_handshake`, `fps_and_preset_changes_rebuild_the_encoder_without_a_dock_cycle`. `app/supervisor.rs` tests cover `live_change_is_applied_reported_and_visible_in_get` and the invalid, rejected and unchanged cases. |
| Reload | `app/supervisor.rs`: `reload_applies_only_the_differences_to_the_live_settings`, `reload_with_an_unsupported_difference_applies_nothing`, `signal_reload_takes_the_same_path_without_a_reply`. `tests/e2e/control_socket.rs`: `sighup_and_ctl_reload_apply_the_config_file_differences`. |
| Control socket | `tests/e2e/control_socket.rs` (`ctl_reports_the_running_daemons_settings_and_the_socket_is_private`, `ctl_set_applies_live_changes_and_rejects_what_the_pipeline_cannot_do`, `ctl_without_a_daemon_exits_with_code_3`, `disabled_control_socket_is_not_created`) and `tests/protocol/control_wire.rs`. |
| Dock cycle and resolution changes | `tests/session/reconfig.rs`: `renegotiation_runs_stop_launch_at_the_new_size_and_start_in_order`, `size_change_rebuilds_the_encoder_and_runs_the_dock_cycle_with_video_at_the_new_size`, `video_headers_follow_the_new_size_and_none_is_written_while_the_cycle_runs`, `portrait_size_is_accepted_as_a_dock_cycle_with_swapped_headers`, `commands_sent_during_a_cycle_are_queued_and_served_in_order`. |
| Rollback and degraded state | `tests/session/reconfig.rs`: `a_size_the_dock_refuses_leaves_the_old_encoder_and_size_running`, `a_dock_that_loses_the_session_after_the_cycle_is_rolled_back_to_the_last_good_size`, `a_rollback_that_fails_leaves_the_daemon_degraded_until_the_next_command_recovers_it`. `core/reconfig/transaction.rs`: `a_failed_commit_restores_the_previous_settings`, `a_failed_verify_restores_the_previous_settings`, `a_committed_change_dropped_unverified_is_rolled_back`. `app/supervisor.rs`: `a_failed_rollback_degrades_the_daemon_and_the_next_command_resyncs_first`. |
| EDID | `tests/protocol/edid.rs`: `dock_edid_parses_to_its_name_native_mode_and_mode_list`, `trailing_bytes_after_the_declared_blocks_are_ignored`, `truncated_input_is_an_error_at_every_length`, `names_are_reduced_to_printable_ascii_without_path_separators`, `arbitrary_bytes_behind_a_valid_header_never_panic`. The daemon tests with the simulated dock cover the cache and the synthetic fallback. |
| X11 virtual monitor | `tests/capture/virtual_x11.rs`: `bare_monitor_is_listed_captured_exactly_and_removed_on_drop`, `output_monitor_is_a_real_output_captured_exactly_and_removed_on_drop`, `an_output_the_server_refuses_falls_back_to_a_bare_monitor`, `resizing_changes_the_monitor_on_the_server_in_both_tiers`, `recovery_removes_a_monitor_that_a_killed_daemon_left_behind`. `tests/e2e/vdisplay_xorg.rs`: `ctl_switches_between_mirror_and_extend_and_exit_restores_the_layout`, `a_monitor_left_by_a_killed_daemon_is_removed_by_vdisplay_cleanup`, `a_new_daemon_undoes_the_monitor_a_killed_one_left_behind`, `vdisplay_probe_prints_the_outputs_and_the_extend_mechanism`. |
| Mode switching | The X11 tests above, `tests/capture/virtual_sway.rs` (`switching_the_mode_creates_the_output_right_of_the_anchor_and_removes_it_again`), `tests/capture/virtual_mutter.rs` (`switching_the_mode_creates_the_stream_monitor_and_removes_it_again`) and `app/monitor.rs` (`switching_to_extend_creates_the_monitor_right_of_the_anchor_and_back_removes_it`). With the section absent the mode is mirror (configuration tests). |
| Display policy | `core/display_policy.rs`: `gnome_picks_mutter_even_when_the_portal_offers_nothing`, `sway_picks_the_wlroots_headless_output`, `other_wayland_compositors_use_the_portal_only_when_it_offers_a_virtual_source`, `x11_without_randr_monitors_falls_back_to_mirror_with_a_reason`, `fail_policy_turns_a_missing_mechanism_into_an_error`, the placement tests. |
| Following display changes | `tests/capture/follow_x11.rs` (the watcher reports resize, move and removal made with `xrandr`, and nothing for the daemon's own changes). `tests/e2e/follow_xorg.rs`: `an_xrandr_mode_change_runs_one_dock_cycle_with_a_rebuilt_encoder_and_a_new_keyframe`, `a_size_set_with_ctl_runs_one_dock_cycle_and_the_echo_is_not_followed`, `with_following_off_an_xrandr_mode_change_leaves_the_dock_alone`, `turning_the_monitor_off_with_xrandr_reverts_the_daemon_to_mirror`. `tests/e2e/follow_sway.rs` has the same set for `swaymsg` plus `a_swaymsg_position_change_retargets_capture_without_a_dock_cycle`. `app/supervisor.rs`: `a_monitor_resized_from_outside_changes_the_size_through_a_dock_cycle`, `a_monitor_moved_from_outside_retargets_capture_without_a_dock_cycle`, `a_removed_monitor_reverts_to_mirror`. |
| sway headless | `tests/capture/virtual_sway.rs`: `headless_output_is_created_at_the_requested_rectangle_and_unplugged_on_drop`, `headless_output_holds_the_pixels_a_second_client_reads_at_every_size`, `resizing_the_headless_output_changes_size_and_position`, `several_headless_monitors_are_independent`. |
| Mutter | `tests/capture/virtual_mutter.rs`: `mutter_monitor_streams_frames_at_the_requested_size`, `mutter_stream_takes_the_size_the_capture_asks_for`, `retargeting_to_another_stream_switches_the_frames`, `mutter_stream_node_is_removed_when_the_monitor_drops`. The call sequence is covered without a shell by the `mutter_mock_*` tests in `adapters/display/mutter.rs`. |
| Portal virtual source | Mocked portal tests in `adapters/display/portal.rs`: `a_portal_that_does_not_answer_in_time_is_unsupported_and_its_late_stream_is_closed`, `restore_token_is_presented_again_and_replaced_after_a_start`, `dropping_the_monitor_closes_the_portal_session`. |
| Packages | `deploy/check-packages.sh` in the package workflow: payload files and modes, the PipeWire dependency in both packages, the unit lines and a start of the extracted binary. |

The display, Mutter and sway live tests need the matching `SCREENDOCK_REQUIRE_*` variable to fail instead of skip; the gate set above runs them with all five set.

## Default checks

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same checks, plus a clippy run with `--features capture-wayland` after installing the PipeWire development packages, one clippy run with `--features encode-ffmpeg,capture-wayland`, and a separate `encode-ffmpeg` job that installs the libav development packages and runs clippy and the full test suite with `--features encode-ffmpeg`. No default test needs a dock, a hidraw node or a physical display. Hardware access is replaced by a regular file or pipe through `HidrawHandle::from_file`.

## Test groups

The integration tests are grouped by area, one test target per group: `cargo test --test protocol`, `cargo test --test session`, `cargo test --test capture`, `cargo test --test e2e` and `cargo test --test encode --features encode-ffmpeg`. The `capture` and `e2e` groups share the Xvfb, Xorg and X11 painter helpers in `tests/support/`.

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

## Capture, Stage 1: Xorg with the dummy driver

`tests/support/xorg.rs` starts a rootless Xorg with the dummy video driver on a free display number between 100 and 499, with a generated config and log in a temporary directory: one 1280x720 screen with a declared framebuffer of 4096x2160, no input devices. Unlike Xvfb, this server accepts a framebuffer larger than its start size (`xrandr --fb`), lists RandR monitors (`xrandr --setmonitor`) and has 16 outputs, `DUMMY0` active and `DUMMY1` to `DUMMY15` disconnected, any of which becomes a monitor when it is given a mode and a position. `tests/capture/virtual_x11.rs` covers:

- the facts gathered from the server: one active output `DUMMY0` at 1280x720, RandR 1.5 or newer, a spare output;
- the framebuffer growing to 6000x3000 and a bare monitor appearing in `xrandr --listmonitors`;
- a custom mode on `DUMMY1` turning it into a second monitor at the requested position;
- a bare monitor and a real output monitor: listed by `xrandr --listmonitors`, captured with exactly the monitor pixels (painted corners checked), removed on drop with the screen size and the recovery file restored;
- an output the server refuses falling back to a bare monitor;
- resizing in both tiers;
- recovery of a monitor that was leaked as a killed daemon would leave it.

The unit tests of `adapters::display::x11_randr` run the same tier logic against a recording double: the exact request order, screen growth, undo on drop, undo after a refused step, negative positions and leaked monitors. The recovery file tests cover the round trip, file modes, corrupt and oversized files and a state owned by a running process.

Requirements: `Xorg` with the `dummy` driver (Debian and Ubuntu packages `xserver-xorg-core` and `xserver-xorg-video-dummy`) and `xrandr` (package `x11-xserver-utils`). The helper runs `/usr/lib/xorg/Xorg` directly when it exists, because the `Xorg` wrapper refuses a custom config for non-root users.

- When Xorg or the dummy driver is missing, or the server cannot start rootless, the tests print a skip note and pass.
- With `SCREENDOCK_REQUIRE_XORG` set to any value, the same condition fails the test.

## Virtual monitor in the daemon

`tests/e2e/vdisplay_xorg.rs` runs the real binary on the Xorg dummy server with file nodes and `--no-handshake`:

- `ctl set mode extend` and `mirror` create and remove a monitor at `+1280+0`, with the recovery file present exactly while the monitor exists, and a SIGTERM exit restores the monitor list and the screen size;
- after SIGKILL the monitor stays, and `vdisplay cleanup` restores the layout and a second run prints that there is nothing to clean up;
- a new daemon undoes the monitor a killed one left;
- `vdisplay probe` prints the outputs, the extend mechanism and the placement.

`tests/capture/follow_x11.rs` uses `xrandr` as the outside party on the Xorg dummy server: a bare monitor redefined, moved or deleted, and an output given another mode, are each reported once as resized, moved or removed; the daemon's own resize on either tier and a change it has observed report nothing. `tests/e2e/follow_xorg.rs` runs the daemon loop against the simulated dock on the same server: an `xrandr` mode change on the monitor's output gives one dock cycle within 2 seconds with `app-launched` at the new size, a rebuilt encoder whose first output is a keyframe with parameter sets, capture retargeted to the new rectangle and no heartbeat gap of 2 seconds; a size set with `ctl` gives exactly one cycle and no echo; with `follow_system_changes` off the dock sees nothing; turning the output off reverts to mirror without a cycle. The watcher's unit tests use a scripted desktop: debounce, one report per change, announced changes, blinks and removal. The supervisor tests cover each kind of display event.

`tests/e2e/follow_sway.rs` runs the same daemon rig on headless sway: a `swaymsg` mode change gives one dock cycle with a rebuilt encoder and a keyframe, a position change retargets capture without a cycle, `ctl set resolution` does not loop, `follow_system_changes` off leaves the dock alone, and an unplugged output reverts to mirror. Unit tests cover the sway event reader (whole and partial events, bad magic, oversized events, closed connection) and the frame size tracking for streams (one report per renegotiation, short blips, the settle period, non-stream areas).

The unit tests of `app::monitor` use a fake display provider: switching creates and drops the monitor, resizes follow the picture size, an unsupported desktop changes nothing, the start fallback follows the policy and a resync reconciles the monitor with the settings.

## Control socket

`adapters/control/server.rs` tests run the server in process against a stand-in daemon thread: the socket is mode 0600 inside a 0700 directory and is removed on drop, a stale socket file is replaced, a live socket is not taken over, a regular file at the path is left alone, an over-long path is refused, an oversized request line and malformed requests get an `invalid` answer without reaching the daemon, eight concurrent clients each get their own answer, a silent client does not block others, and requests are rejected once the daemon side is gone. `tests/protocol/control_wire.rs` pins the exact request and response lines, strictness and the line caps. `tests/e2e/control_socket.rs` runs `screendockd ctl status` and `ctl get` against the real daemon binary on Xvfb: the settings and version come back, an unknown key exits with code 4, a missing daemon exits with code 3, the socket is private, it is removed at exit and `control.enabled = false` creates none. Each daemon in the e2e tests gets its own `XDG_RUNTIME_DIR`, so parallel daemons never share a socket.

The `protocol` group's `edid` tests parse the dock's own 258 byte message (`tests/fixtures/dock_edid.bin`, captured from a cold start) to its name, preferred mode, physical size, mode list, extension count and trailing byte count, and check a checksum failure in the base and in the extension block, the error for every truncated length, a wrong header, an extension count beyond the input, names with control characters, path separators and line feeds, and 2000 pseudo-random blocks behind a valid header without a panic.

The daemon tests with the fake dock check the EDID path: a dock that sends the captured EDID gives the `dock` source, the name, the native mode and the mode list, and the bytes reach the store; a dock that sends none falls back to the stored copy, then to the configured size; a dock whose EDID does not parse is neither reported nor stored. The supervisor tests check that the read-only keys appear in `status`, in `get` and in `get` with one key, and that `set` refuses them. The file store tests check the round trip, mode 0600, and the handling of empty, oversized and non-file entries.

## Supervisor

`app/supervisor.rs` tests drive the supervisor through its command channel with a recording pipeline: a live change is applied and visible in `get`, setting the current value does not touch the pipeline, a change the pipeline cannot do is rejected and leaves the settings alone, a pipeline without a live rate treats a bit rate change as a rebuild, a pipeline failure keeps the old settings, bad keys and values are invalid, a reload applies only the differences to the live settings, a reload with one unsupported difference applies nothing, a reload that cannot read the file is reported, a signal reload takes the same path without a reply, and `status` carries the extra fields and the change count. `tests/e2e/control_socket.rs` also runs `ctl set` and `ctl reload` against the real daemon binary: a live change takes effect, a size change is rejected with exit code 2, bad input gives exit code 4, and editing the config file followed by SIGHUP or `ctl reload` applies the difference, while a broken file is reported and changes nothing.

## Live encoder changes

`core/keyframe.rs` tests cover the first-frame keyframe, the spacing, a forced keyframe restarting the count, and a shorter or longer interval. `app/pipeline.rs` tests use a stand-in video loop: published knobs reach it and the waiter sees the result, waiting without a loop times out, the offered steps follow the rate support, an encoder refusal is reported and the old knobs come back, and a follow-only change does not touch the loop. `tests/session/reconfig.rs` runs the daemon against the simulated dock and changes the bit rate and keyframe interval through the supervisor: the encoder receives exactly one rate change, no new host message and no second handshake appear, the keyframes after the change are exactly 5 frames apart, and no heartbeat gap reaches 2 seconds. `tests/encode/ffmpeg.rs` (feature `encode-ffmpeg`) lowers the bit rate of the running libx264 encoder on noise frames, checks that frames get smaller, and decodes the whole sequence with the `ffmpeg` tool; a second test changes the keyframe clock from 10 to 5 frames mid stream and checks the keyframe positions and the decode. The live rate test is skipped, with a note, when the linked libavcodec does not apply a live rate change. `tests/e2e/control_socket.rs` also sets the bit rate and keyframe interval on the real daemon binary.

## Session and simulated dock

`testing::fake_dock::FakeDock` (compiled for unit tests and under the `testing` feature, which the crate's own tests enable through a dev-dependency on itself) stands in for the dock: MI_00 and MI_01 are Unix datagram socket pairs (datagrams keep report boundaries like a hidraw read), and a thread speaks the dock side. A fresh dock announces ready and answers `app-launched` and `notify-projection-started`; a dock that holds a session only heartbeats. It records every host message byte for byte, checks the MI_00 report layout (4096 bytes, Report ID 1, zero header bytes) and the MI_01 header layout, and counts video reports that arrive before its sink is started. Tests in `core/session/tests.rs` cover the exact bytes of every host message, the inbound assembler and parser, the full handshake order, the settle delay, the bounded fallback to `AssumedActive`, the exact first 64 bytes of the heartbeat and `app-launched` reports (equal to the reference prototype's framing), a silent dock that must receive `app-launched`, cursor, passthrough 103 and `notify-projection-started` in order and without `notify-projection-stopped`, a fully silent dock (no ready, no heartbeat echo, no replies) that receives that sequence before any video report, arms in `AssumedActive` and receives video, a late ready announcement that triggers the full handshake from `AssumedActive`, denied projection, an error reply to `app-launched`, loss on a re-announced ready and on a reported write failure (with the handshake redone and ids continuing), heartbeat cadence, startup failures and reader end of stream. `tests/session/dock.rs` runs `daemon::run` against the fake dock, including a dock with a held session and a fully silent dock. `FakeDock` also records the size of every `app-launched` (`launch_sizes`), lists the host methods without heartbeats in order (`host_methods`), logs each change of video header size (`VideoStats::sizes`), refuses `app-launched` at one size (`reject_size`) and drops the session after a number of video messages (`drop_session_after`). `tests/session/reconfig.rs` also runs the daemon with a supervisor that can rebuild the encoder: a size change gives one rebuilt encoder whose first message is a keyframe with SPS, a dock cycle, video headers at the new size and no heartbeat gap of 2 seconds; frame rate and preset changes rebuild the encoder and retarget capture with no dock cycle; an encoder that refuses the new size, and a dock that refuses it, leave the old encoder and size running. `core/reconfig/transaction.rs` tests drive the transaction with a stub target: every stage in order, a failed prepare that restores nothing, a failed commit and a failed verify that restore, a failed restore that is reported with the first reason, a live change that has nothing to restore and a committed change dropped unverified that is restored; two `compile_fail` doc tests show that a stage cannot be skipped. `app/supervisor.rs` tests cover the stages around a recording pipeline, the rollback, the degraded state with its `status` fields and the resync that comes before the next change. `tests/session/reconfig.rs` adds a portrait size through the supervisor, a dock that loses the session after the cycle (rolled back to the last good size, with video at the old size again), a rollback that fails and leaves the daemon degraded until the dock accepts launches again, and three commands queued behind a dock cycle that are served in order. `FakeDock::drop_session_after_next_launch` counts video messages from the next `app-launched`. `tests/capture/x11.rs` retargets `X11ShmCapture` to a rectangle and back, refuses rectangles outside the screen and applies a new frame rate. `tests/encode/ffmpeg.rs` builds a libx264 encoder at a second size and decodes its stream. `tests/session/reconfig.rs` drives `Session::renegotiate` with them: the cycle order and the new size in `app-launched`, the stop sent to a silent dock, video headers that switch to the new size with nothing written while the session is unarmed, a refused size that fails the cycle and brings the old size back, a session without a handshake, two requests served in order, and a dock that drops the session after some video. `validation.rs` has an `isolate` run with a session. These tests need no display and no hardware.

## Encoder

Unit tests cover the message policy (`messages_from_packet`), the prerecorded encoder (looping, keyframe restart, rejection of streams without a leading keyframe), `DisplayGeometry` (header size for 1280x720, 900x600 and portrait 720x1280, orientation and range checks) and device ID parsing and override in discovery.

`tests/encode/ffmpeg.rs` is built only with `--features encode-ffmpeg`, which needs the libav development files, `libx264-dev`, `clang` and `pkg-config`. It encodes a synthetic BGRA sequence with the libx264 encoder, checks the message shape and the SPS profile, and decodes the result with the `ffmpeg` command line tool. When the `ffmpeg` binary is missing the decode checks print a skip note and pass; with `SCREENDOCK_REQUIRE_FFMPEG` set to any value they fail instead. CI sets it.

```text
cargo test --features encode-ffmpeg
```

Status: this feature does not compile on hosts without the libav development files; CI is where it is built and tested.

## Daemon

`tests/e2e/daemon_xvfb.rs` runs the real `screendockd run --no-handshake` binary against Xvfb with the prerecorded encoder and regular files for MI_00 and MI_01 (files cannot hold a dock conversation), and checks the written video messages against the source stream, the heartbeat, graceful exit on SIGTERM and SIGINT, and fail-fast startup. Unit tests in `src/app/daemon.rs` cover the error policy with synthetic backends and loopback files. These tests need Xvfb on the same terms as the capture tests (`SCREENDOCK_REQUIRE_XVFB`). See [daemon.md](daemon.md#tests).

## Picture size comparison

`deploy/compare_sizes.sh` measures how close the picture the daemon sends is to the screen it captured, for several picture sizes, without a dock. It needs `Xvfb`, `xterm`, `ffmpeg` with `x11grab`, `ffprobe` and `python3`, and a binary built with `--features encode-ffmpeg`.

```text
deploy/compare_sizes.sh [size ...]
```

The default sizes are `1280x720`, `900x720` and `640x360`. Environment variables:

| Variable | Default | Meaning |
|---|---|---|
| `SCREENDOCKD` | `src/screendockd/target/release/screendockd` | The binary to run |
| `COMPARE_DISPLAY` | `150` | Display number of the private Xvfb; the script refuses a number that is in use |
| `COMPARE_SECONDS` | `6` | Seconds the daemon runs before the screenshot is taken |

The script starts its own Xvfb (1280x720) with an `xterm` that prints a fixed text pattern, and stops both on exit. For each size it writes a config with that `display.width` and `display.height`, starts a fresh `screendockd run --backend x11 --no-handshake` against two regular files standing in for the MI_00 and MI_01 nodes, and lets it run. It then grabs one frame of the display with `ffmpeg` (`x11grab`) as a PNG and stops the daemon. A size is set through the config because `ctl set resolution` needs a session, and `--no-handshake` has none.

It then reads the MI_01 file back: every report has a 16 byte header, bytes 1 to 4 of the first report hold the payload length as a big-endian u32, and each report carries up to 4080 payload bytes. The first message with an IDR slice at or after message 40 (the keyframe interval is 50 frames) is written as an Annex B file, decoded with `ffmpeg` to a PNG, and compared with the screenshot scaled to the decoded size, using the `psnr` and `ssim` filters. The script prints one row per size:

```text
size       decoded    PSNR dB    SSIM
1280x720   1280x720   17.700141  0.591896
900x720    900x720    19.726888  0.510269
640x360    640x360    24.103294  0.948752
```

What the numbers mean:

- `decoded` is the size of the decoded frame; it equals the configured size when the header, the encoder and the framing agree.
- PSNR is the average peak signal to noise ratio over the three colour channels in decibels, higher is closer. SSIM is structural similarity from 0 to 1, 1 is identical.
- The comparison covers capture, scaling to the encoded size, libx264 at the configured bit rate and the decode. It does not include anything the panel does after it receives the stream, so it says nothing about the panel itself.
- Text on a black background is the hardest content for a lossy encoder at the default `ultrafast` preset and `encode.vbv_buffer_ms` of 100, so the absolute values are low; compare sizes with each other and runs with the same settings, not with a fixed threshold.
- The source screenshot is taken after the keyframe was produced, and the xterm content is static, so differences come from encoding and scaling, not from content changes. A sized-down picture compares against a scaled screenshot, so SSIM at a size whose aspect ratio differs from 16:9 (900x720) includes the stretch of that scaling.

The script never opens `/dev/hidraw*` and uses only the display number it was given.

## CPU measurement

`screendockd bench` measures capture plus encode CPU time; see [encoding.md](encoding.md#cpu-measurement). The unit tests check the arithmetic and the loop with a synthetic backend; the figure that matters needs the target host and the libx264 build.

## Capture, Stage 1: Wayland

`WaylandPortalCapture` is exercised without a desktop through the headless GNOME Shell and sway tests below, which read frames from PipeWire streams that the compositor creates. The portal screen share path itself, with its prompt, is not automated.

Status: the portal share prompt requires validation on a desktop session.

The portal virtual display is unit tested against a mocked portal source: restore token reuse and replacement, the start timeout, closing a late stream, closing the session on drop and the resize rules. These tests run in every build.

## Capture: headless GNOME Shell

The Mutter tests (`tests/capture/virtual_mutter.rs`, helper `tests/support/mutter.rs`, built with `--features capture-wayland`) start `dbus-daemon --session`, `pipewire`, `wireplumber` and `gnome-shell --headless --wayland --no-x11 --virtual-monitor 1280x720` on a private runtime directory under `/tmp`, in one process group that is signalled when the test ends. They cover the frames of a `RecordVirtual` stream through `WaylandPortalCapture::connect_stream` (size, `Bgrx8888`, not black, and the same size and first pixel read with `gst-launch-1.0 pipewiresrc` on a second monitor), the size a capture asks for, retargeting a running capture from one stream to another, and the node disappearing when the monitor drops. The provider facts, the policy choice and the mode switch through the monitor manager (create the monitor, read frames at the configured size through the capture, remove the monitor and its node) run against the same shell. The call order of the Mutter display is unit tested against a recording double (`mutter_mock_*` in `adapters::display::mutter`).

The tests skip with a message when gnome-shell, pipewire, wireplumber or dbus-daemon is missing or the shell cannot start; `SCREENDOCK_REQUIRE_MUTTER=1` turns that into a failure. The GStreamer reference needs `gst-launch-1.0` with the PipeWire plugin and the node check needs `pw-dump`; each is skipped when missing.

## Capture: headless sway

The sway tests (`tests/capture/virtual_sway.rs`, helper `tests/support/sway.rs`) start sway with `WLR_BACKENDS=headless`, `WLR_LIBINPUT_NO_DEVICES=1` and `WLR_RENDERER=pixman` on a private runtime directory under `/tmp`, and stop it when the test ends. They cover output creation at the placed rectangle, resizing, several monitors, pixels read back with `grim`, display facts and policy choice, and switching the display mode through the monitor manager. The IPC client has unit tests against a fake socket.

The tests skip with a message when sway cannot start; `SCREENDOCK_REQUIRE_SWAY=1` turns that into a failure. The pixel test also needs `grim`. CI installs sway and grim and does not set the variable (the same holds for the Mutter tests, for which it installs gnome-shell, pipewire, wireplumber and GStreamer's PipeWire plugin), so a runner that cannot run sway headless skips these tests.

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
