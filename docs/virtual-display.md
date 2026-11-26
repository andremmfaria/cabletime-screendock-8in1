# Virtual display

The dock can show a monitor of its own instead of a copy of an existing screen. This page describes how the monitor is made on X11, where it is placed, how its changes are undone and how the pieces fit the display port. The `[virtual_display]` keys are in [configuration.md](configuration.md).

## Display port

`ports::display` defines the interfaces and `adapters::display` implements them.

| Item | Role |
|---|---|
| `DisplayFacts` | What the environment offers: display server, desktop, outputs with rectangles, RandR version, a free output |
| `VirtualDisplay` | One way of making monitors: `create(&MonitorSpec)` returns a `VirtualMonitor` |
| `VirtualMonitor` | A handle with `rect()` and `resize(&MonitorSpec)`. Dropping it removes the monitor and restores what creating it changed |
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

## Placement

The monitor sits next to the anchor output as `placement` says: `right-of` (the default), `left-of`, `above`, `below`, or at fixed coordinates with `at`. The anchor is an output name or `primary`. The size is `[display] width` by `height`. Capture reads exactly the monitor rectangle through XShm; see [capture.md](capture.md).

## Recovery

X server state outlives the client, so a daemon that is killed would leave its monitor behind. Before the first change the daemon writes `$XDG_RUNTIME_DIR/screendockd/vdisplay-state.json` (directory mode 0700, file mode 0600). It records the display, the daemon's process id, the screen size before the change and the monitor, output and mode about to be created.

- Dropping the monitor, which also happens on a clean exit, removes the monitor, turns the output off, removes the mode, restores the screen size and deletes the file.
- Creating a monitor first looks at the file. When the daemon that wrote it is gone, its changes are undone. When that daemon is still running, creation fails.
- When the X server is gone, there is nothing left to restore and the file is dropped.
- A file larger than 64 KiB or that does not parse is an error.

A failed step during creation undoes the steps before it and removes the file.
