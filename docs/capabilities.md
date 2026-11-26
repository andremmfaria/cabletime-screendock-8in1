# CABLETIME ScreenDock - What You Can Actually Do With It

## Display modes
- **Mirror mode:** default immediately after driver install - the 5.5" screen shows the exact same image as the main computer display.
- **Extend mode:** switch via Windows Display Settings or macOS System Settings → Displays, after which the small screen behaves like a normal second monitor. The user manually drags an app window onto it (same as dragging to any extended monitor) - there is no automatic widget system; "AI dashboard," "clock," "meeting notes," etc. are just ordinary app/browser windows the user chooses to park there.
- Screen orientation, scaling, and arrangement are configured the same way as any standard external display.

## Maximum simultaneous displays
With a laptop as host, CABLETIME's own user guide describes a 3-screen setup:
1. Laptop's own screen
2. ScreenDock's 5.5" built-in screen
3. One external monitor/TV/projector via the dock's HDMI port (up to 4K@60Hz)

No source documents more than this 3-screen (laptop + ScreenDock + 1 HDMI) configuration. MST ("Mac&Win MST") is referenced in marketing copy as enabling additional multi-screen support, but no source actually demonstrates driving more than one external HDMI display from this single-HDMI-port unit - treat any "more than 3 total screens" claim as unverified.

## Charging / power passthrough
- Dock accepts up to 100W via its USB-C PD input port (charger sold separately).
- Delivers up to 85W to the connected laptop through the HOST connection (CABLETIME's own stated ceiling - the 100W input figure is not what reaches the laptop).
- HOST (data+video) and PD (power) are separate ports; you can use the display features over HOST alone with no charger attached at all.
- If other devices are drawing power from a shared multiport charger, available wattage to the laptop is reduced.

## Data / peripherals
- One USB-C 5Gbps data port and two USB-A ports (one 5Gbps "3.0", one 480Mbps "2.0" - the slower one is suggested in the user guide specifically for keyboard/mouse, the faster ones for drives).
- SD 3.0 and microSD(TF) 3.0 card slots, read/write via standard OS file browsers (no slideshow/auto-import feature - opening a photo on the small screen requires manually dragging a photo-viewer window there).
- Gigabit Ethernet for wired networking (actual speed depends on network gear).

## Practical/marketed use cases (vendor framing - not independently benchmarked, but mechanically plausible given it's a real secondary display)
- AI chat/dashboard panel kept visible beside main work (ChatGPT, Claude, Codex windows)
- Clock/weather/calendar widget apps, to-do lists, focus timers
- Meeting support: agenda, notes, or chat window parked off the main screen during a video call
- Home/pet camera monitoring window
- System monitoring (CPU/GPU load, temps, FPS) during gaming or rendering
- Digital photo frame mode when the unit is folded upright (ports on only two sides let it stand like a frame)
- Reference/tutorial video playback alongside active work

## Port usage: does the display need anything beyond HOST? [confirmed, verified against live hardware]

No. CABLETIME's own user guide (cabletimetech.com/blogs/knowledge/cabletime-screendock-ct-dks9-pw-user-guide) is explicit: "Keep ScreenDock connected to your computer through HOST... Leave the dock's other ports empty for now. You do not need to add a charger to install USB Display. A charger alone cannot show computer content on the small screen." The guide treats the external HDMI output as an independent path from the built-in screen ("Its working picture does not confirm that the computer can send video to an HDMI monitor through its USB-C port").

Verified directly on this host's live USB topology: the display's composite device (Actions `1de1:f201`) hangs off the same internal hub chain as the HOST connector, already fully enumerated and reachable via `/dev/hidraw0`/`hidraw1` with only the HOST cable connected, no PD charger, card, Ethernet, or HDMI cable attached. See docs/research/display-protocol-reverse-engineering.md for the raw HID report layout being reverse-engineered on those two interfaces.

## Hard limitations
- No standalone OS, no Wi-Fi, no built-in apps - it is 100% dependent on the connected PC/Mac being powered on and running USB Display.
- Non-touch - all interaction happens through the host computer's input devices.
- Needs ~30 seconds after driver install (or waking the display) before the screen activates; the HOST cable must remain connected the whole time.
- A charge-only USB-C cable will not drive the screen - it must carry both USB data and DisplayPort Alt Mode video.
- "MDClock"-style fancy clock widgets shown in marketing video require a separate paid third-party subscription app - not something ScreenDock provides itself.
