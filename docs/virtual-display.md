# Virtual display

The dock can show a monitor of its own instead of a copy of an existing screen. This page describes how the monitor is made on X11, where it is placed, how its changes are undone and how the pieces fit the display port. The `[virtual_display]` keys are in [configuration.md](configuration.md).

## Display port

`ports::display` defines the interfaces and `adapters::display` implements them.

| Item | Role |
|---|---|
| `DisplayFacts` | What the environment offers: display server, desktop, outputs with rectangles, RandR version, a free output |
| `VirtualDisplay` | One way of making monitors: `create(&MonitorSpec)` returns a `VirtualMonitor` |
| `VirtualMonitor` | A handle with `rect()`, `resize(&MonitorSpec)` and `pipewire_node()` for stream backed monitors. Dropping it removes the monitor and restores what creating it changed |
| `DisplayProvider` | Gathers the facts and builds the display for a policy decision (`virtual_display`) or a mirror of an output (`mirror`) |
| `MonitorSpec` | A `MonitorName`, a `Rect` and an optional physical size in millimetres. Without a size, 96 dots per inch is assumed |

`MonitorName` keeps printable ASCII without path separators, cuts the name at 32 characters and uses `ScreenDock` when nothing is left. The name comes from the dock's EDID when the config gives none.

`core::display_policy::decide` turns the facts and the config into a decision. The mirror display hands out the rectangle of an existing output and creates nothing.

## X11 tiers

| Tier | Mechanism | Appears in |
|---|---|---|
| 1, real output | A free (disconnected) output gets a custom mode through `CreateMode`, `AddOutputMode` and `SetCrtcConfig` at the monitor position | `xrandr`, the desktop's display settings |
| 2, bare monitor | `SetMonitor` defines a RandR monitor over an enlarged screen | `xrandr --listmonitors` and monitor aware clients; display settings may not list it |
| 3, mirror | The rectangle of an existing output is captured | nothing is created |

The policy picks tier 1 when the facts report a free output and tier 2 otherwise. When the server refuses tier 1, the daemon logs the reason and makes a tier 2 monitor. A tier 1 monitor carries the name of its output (`DUMMY1` on the dummy driver); a tier 2 monitor carries the monitor name. Tier 1 needs an output that can take a mode, which real drivers usually do not offer for unplugged connectors; the dummy driver offers fifteen.

Creating a monitor first grows the X screen so it holds the monitor rectangle, then defines the monitor. A position with a negative coordinate cannot be placed on an X screen and is refused. Resizing a tier 2 monitor replaces its definition; resizing a tier 1 monitor creates the new mode, moves the output to it and removes the old mode. The screen grows when the new rectangle needs it.

## Portal display

`PortalVirtualDisplay` asks the desktop portal for a screen stream of the virtual source type. It is used when `wayland_backend` is `portal`, or by `auto` where the portal's `AvailableSourceTypes` includes the virtual bit (value 4). The portal chooses the monitor's position and size, so `placement` and `anchor` do not apply; the monitor's rectangle is the requested position with the size the portal reports for the stream, falling back to the requested size.

- The start runs on a worker thread and is given 15 seconds. A start that does not answer in time is reported as `unsupported`, and the `fallback` setting then decides between mirroring with the reason and ending startup. A stream that arrives after the timeout is closed.
- The restore token the portal returns is kept in `$XDG_STATE_HOME/screendockd/portal-restore-token` (mode 0600, written atomically) and presented at the next start, so a portal that supports it can skip its prompt. A token file that holds anything but a printable token of at most 256 characters is ignored.
- `VirtualMonitor::pipewire_node` returns the PipeWire node of the stream. Dropping the monitor closes the portal session.
- A portal monitor cannot change its size: `resize` accepts a move and refuses another size, so the monitor manager recreates the monitor instead.
- The provider reads the portal's source types once per facts query, within 3 seconds, and treats a missing portal as unknown.
- The portal display needs the `capture-wayland` feature. Without it a request for the portal backend is refused with the reason.

The daemon's capture backend on Wayland opens its own portal session for the whole desktop and cannot follow a monitor rectangle. When it refuses to retarget and the `fallback` setting is `mirror`, the daemon logs the reason, removes the monitor and mirrors; with `fail` it ends.

Status: the portal display is tested against a mocked portal (token handling, timeout, session close, resize). A live portal virtual source requires validation: headless GNOME does not offer one.

## Placement

The monitor sits next to the anchor output as `placement` says: `right-of` (the default), `left-of`, `above`, `below`, or at fixed coordinates with `at`. The anchor is an output name or `primary`. The size is `[display] width` by `height`. Capture reads exactly the monitor rectangle through XShm; see [capture.md](capture.md).

## Recovery

X server state outlives the client, so a daemon that is killed would leave its monitor behind. Before the first change the daemon writes `$XDG_RUNTIME_DIR/screendockd/vdisplay-state.json` (directory mode 0700, file mode 0600). It records the display, the daemon's process id, the screen size before the change and the monitor, output and mode about to be created.

- Dropping the monitor, which also happens on a clean exit, removes the monitor, turns the output off, removes the mode, restores the screen size and deletes the file.
- Creating a monitor first looks at the file. When the daemon that wrote it is gone, its changes are undone. When that daemon is still running, creation fails.
- When the X server is gone, there is nothing left to restore and the file is dropped.
- A file larger than 64 KiB or that does not parse is an error.

A failed step during creation undoes the steps before it and removes the file.

## Switching modes while the daemon runs

`ctl set mode extend` and `ctl set mode mirror` switch between a virtual monitor and the whole desktop without restarting the daemon, and `ctl reload` applies a changed `[virtual_display]` section the same way. The switch goes through the change transaction of [control.md](control.md): prepare checks that the desktop can host a monitor and encodes a test frame, commit creates or removes the monitor, retargets capture and rebuilds the encoder with a fresh keyframe, verify watches the session. A failed step is rolled back, which removes a monitor that was just created. Without a handshake (`--no-handshake`) a switch that keeps the picture size needs no dock cycle.

| Change | Effect |
|---|---|
| `mode` to `extend` | the monitor is created at the placement and capture reads its rectangle |
| `mode` to `mirror` | the monitor is removed and capture reads the whole desktop |
| `placement`, `anchor`, `x`, `y` in extend mode | the monitor moves and capture follows |
| `resolution`, `orientation` in extend mode | the monitor resizes, capture follows and the dock cycle runs |
| `name`, `wayland-backend` in extend mode | the monitor is created again |

When the daemon starts in extend mode and the desktop cannot host a monitor, the `fallback` setting decides: `mirror` logs the reason and mirrors, `fail` ends startup with the reason. A runtime switch to extend that cannot be done is rejected with the reason instead.

On exit (SIGINT, SIGTERM or a normal end) the monitor is removed and the screen size restored.

## Commands

| Command | Output |
|---|---|
| `screendockd vdisplay probe` | the display server, desktop, RandR version, outputs with rectangles, a spare output, the mechanism extend mode would use, the placement it would compute and the configured mode |
| `screendockd vdisplay cleanup` | undoes the changes recorded in the recovery file when its daemon is gone, and prints `restored the display layout`, `nothing to clean up`, or that a daemon is still running |

`probe` needs a graphical session and prints a connection error without one. `cleanup` does nothing when the X server is gone.

The systemd user unit runs `screendockd vdisplay cleanup` as `ExecStopPost`, which covers a daemon that was killed, and `ctl reload` as `ExecReload`. A new daemon also undoes a leftover monitor before it makes its own.
