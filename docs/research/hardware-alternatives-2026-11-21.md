# Hardware alternatives to the CABLETIME ScreenDock

Date: 2026-11-21

## Summary

Genuine small (5 to 7 inch) USB-C secondary screens that use real DisplayPort
Alt Mode essentially do not exist as a product category. Vendors that
advertise DP Alt Mode portable monitors start around 11.6 inches. Below that
size, the market is entirely small HDMI panels aimed at Raspberry Pi and
similar boards, driven over a plain HDMI cable with no compression and no
custom driver. DisplayLink chipsets (DL-3xxx through DL-7xxx) do exist in
small docks and monitors and have real Linux support through evdi plus
DisplayLink's own binary driver, but DisplayLink's own documentation
confirms its pipeline is also a software virtual framebuffer, compressed,
and sent over USB to be decoded on the device side, the same category of
approach as the ScreenDock, not a native video sink. Given that, the
simplest, cheapest, and best quality path to the user's actual goal (a
server status or terminal screen next to a machine) is to skip USB
secondary-screen devices entirely: take a real DisplayPort or HDMI signal
straight off the host GPU (built in output, or a USB-C to HDMI/DP adapter
that carries native DP Alt Mode, not a compressed USB graphics chip) into an
ordinary small HDMI monitor. That path has zero software decode step on
either end, works in Linux out of the box with no driver, and is roughly the
same price as the ScreenDock itself.

## Findings

### 1. Small-form-factor DP Alt Mode secondary screens

Searches across Amazon, AliExpress, Plugable, BenQ, ASUS, LG, and general
portable monitor roundups for 2026 turn up DP Alt Mode portable monitors
starting at 11.6 inches (examples: an AliExpress 11.6 inch unit at roughly
$50, ASUS ZenScreen and LG units in the 14 to 16 inch range, Espresso
Display at 13 to 15 inches). Espresso explicitly documents that its displays
require a USB-C cable with DP Alt Mode support and will not work over a
charge-only or data-only USB-C link (support.espres.so, "Using the correct
USB-C Cable"). None of the vendors surveyed sell a 5 to 7 inch panel driven
by native DP Alt Mode. The small end of the "USB-C monitor" market (5 to 7
inch, desk accessory style, e.g. the widely covered 5 inch secondary display
with built in USB-C hub reviewed by MakeUseOf) is implemented the same way
as the ScreenDock: a USB composite hub device with a small panel behind a
proprietary USB protocol and host side software doing the rendering and
encoding, not a true DP Alt Mode sink.

Bandwidth comparison, in principle: a DP Alt Mode link running 2 lanes of
HBR3 (the common USB-C configuration, reserving the other 2 lanes for USB
data) carries roughly 2 x 8.1 Gbps of raw DisplayPort signal, i.e. in the
tens of gigabits per second range, uncompressed or only lightly compressed
with DSC at up to 3:1 (VESA DP Alt Mode spec, usb.org D2T1-4 slide deck;
Plugable, "Exploring DisplayPort: DP Alt Mode, MST, DSC"). A full 4K60
uncompressed 24bpp signal needs about 4 lanes of HBR3 DisplayPort 1.4, and
even a 720p or 1080p signal at that link rate costs it almost nothing, so
DP Alt Mode monitors show the GPU's frame directly with sub-frame latency
and no re-encode artifacts. The ScreenDock's HID interrupt pipe, by
contrast, is in the same rough class as other USB HID interrupt channels,
commonly on the order of a few hundred KB/s to low single digit MB/s
depending on endpoint configuration and polling interval, several orders of
magnitude below a DP Alt Mode lane. That gap is exactly why the dock needs
a lossy H.264 encode step to fit the picture through the pipe at all, and
why frame rate and latency degrade further at 1080p than at 720p (more
pixels into the same narrow pipe, confirmed in this project's own
Ghidra/driver analysis, see mac-driver-analysis.md and
usbdisplay-exe-ghidra-analysis.md in this directory).

### 2. DisplayLink chipsets and Linux support

DisplayLink (now Synaptics) DL-3xxx, DL-41xx, DL-5xxx, DL-6xxx, and DL-7xxx
chips are used across a wide range of USB docks and some small monitors,
and genuinely do have mainstream Linux support. The open source `evdi`
kernel module (github.com/DisplayLink/evdi) exposes each DisplayLink output
as a standard Linux DRM device, so `xrandr`, GNOME, KDE, and Wayland
compositors see it as an ordinary display. On top of evdi, DisplayLink
ships its own closed source `DisplayLinkManager` user mode driver
(packaged for Ubuntu/Debian and, via the community `displaylink-rpm`
project, for Fedora/RHEL family distros) that does the actual rendering
and compression. The Arch Wiki and the displaylink-rpm README both confirm
this two part model: evdi in the kernel, proprietary manager in user space.

Critically, DisplayLink's own support article ("Why has DisplayLink not
released source code for the Linux DisplayLink Manager?",
support.displaylink.com) describes its pipeline explicitly: evdi delivers
raw pixel data from the virtual framebuffer, that is fed into a proprietary
compression stage called DL3 ("adaptive graphics technology"), and the
compressed stream is sent over the USB (or network) transport to the
DisplayLink chip, which presumably decodes it on the far end. In other
words, DisplayLink is not a true native video sink either. It is the same
general shape as the ScreenDock's pipeline: software renders and compresses
on the host, a chip on the peripheral decodes and displays. The differences
are that DisplayLink's compression (DL3) is purpose built for desktop
content and screen updates rather than full motion H.264, its transport
runs over a USB bulk/isochronous channel with far more bandwidth than a
HID interrupt pipe, and it has over a decade of driver maturity on both
Windows and Linux. That makes DisplayLink meaningfully better in practice
(better perceived responsiveness for desktop/text content, broad distro
support, no reverse engineering needed) but it is not an apples to oranges
win of "real video signal" versus "compressed stream." Mainstream Linux
support is DisplayLink's real advantage here, not protocol purity.
DisplayLink hardware in a 5 to 7 inch secondary screen form factor
specifically is uncommon; DisplayLink chips mostly show up in full size
docking monitors and docks (4K/multi monitor use cases), not small
desk-accessory panels, so this is a weaker fit for the user's stated need
than it might first sound.

### 3. HDMI-in small panel plus a real GPU output or a native USB-C to HDMI/DP adapter

This is the approach that sidesteps the whole class of problem. Two sub
options:

- Use a spare native HDMI/DisplayPort output already on the server's GPU or
  motherboard, if one exists, straight into an ordinary small HDMI monitor.
  Zero adapters, zero drivers, zero compression.
- If the only free port is USB-C, use a USB-C to HDMI (or to DisplayPort)
  adapter that is itself just a DP Alt Mode to HDMI/DP protocol converter,
  not a USB graphics chip. Multiple independent sources confirm these
  adapters carry the GPU's native DisplayPort Alt Mode signal through to
  HDMI with a passive protocol conversion chip, and are plug and play on
  Linux because the OS driver stack only ever sees a normal DisplayPort
  output, with no new driver needed (Framework Community thread on USB-C to
  HDMI with an AMD GPU: "USB-C to HDMI adapters use DisplayPort over USB-C
  internally and then have a DisplayPort to HDMI protocol conversion chip
  ... it relies on the DisplayPort capabilities of the GPU and OS, not any
  HDMI capabilities"; Ask Ubuntu answer on USB to HDMI adapters for Ubuntu,
  which separates true USB-C to HDMI/DP adapters, "plug-and-play, even in
  Ubuntu, no configuration, no special drivers," from DisplayLink style USB
  graphics adapters that do need a driver). These adapters run $15 to $30
  (Wirecutter/NYT roundup of USB-C video adapters; Plugable and Cable
  Matters product lines).

  Caveat worth flagging plainly: this only works if the host's USB-C port
  actually exposes DP Alt Mode. Some USB-C ports are power/data only, and a
  "USB-C to HDMI" adapter cannot invent a video signal that was never there
  (the ScreenDock's own USB-C port on the host side may well be one of
  these data-only ports, which is part of why it needs a software encode
  path in the first place). This has to be checked on the specific host
  before buying an adapter.

  On the display side, small HDMI monitors purpose built for Raspberry Pi
  and similar single board computers are a mature, cheap, driver-free
  category: 5 inch 800x480 panels from GeeekPi, CrowPi, and similar
  vendors (roughly $25 to $45 depending on touch support), and larger
  rack-oriented panels like the DeskPi/GeeekPi 7.84 inch 1280x400 touch LCD
  built specifically for 10 inch server rack mounts (DeskPi store lists it
  around $79 to $89), explicitly marketed for server monitoring dashboards.
  These are plain HDMI sinks: no compression, no encode/decode round trip,
  and vendor documentation describes them as "driver-free" for the display
  path (touch may need a small config tweak on some boards, unrelated to
  video).

Honest assessment: for the user's actual goal, a server status/terminal
screen sitting next to or on a machine, this HDMI-monitor-plus-real-signal
path is simpler and higher quality than continuing to pursue the
ScreenDock's proprietary protocol. It requires no reverse engineering, no
custom Linux driver work, no H.264 decode, and no dealing with a vendor
specific HID protocol that was only ever built for Windows and macOS in the
first place. The only real risk is confirming the host's USB-C port (or
finding a free native HDMI/DP port) actually carries a video signal before
buying an adapter, which is a five minute check, not a research project.

### 4. Concrete recommendations (prices approximate, November 2026)

Priority order: real native video signal first, confirmed Linux support
second, small footprint third.

1. If the host machine has any spare native HDMI or DisplayPort output
   (common on most desktop GPUs and many mini PCs), use it directly. Cost:
   $0, just need a cable.
2. If the only free port is USB-C and it supports DP Alt Mode, a
   USB-C to HDMI adapter, for example Plugable USB-C to HDMI adapter or
   Cable Matters USB-C to HDMI adapter, roughly $15 to $25. These show up to
   Linux as a plain DisplayPort/HDMI output with no driver install.
3. Display side: GeeekPi or CrowPi 5 inch 800x480 HDMI touchscreen
   (roughly $25 to $45) for a compact desk/shelf unit, or the
   DeskPi/GeeekPi 7.84 inch 1280x400 rack mount touch LCD (roughly $79 to
   $89) if the server lives in a 10 inch rack and a dedicated status strip
   is wanted. Both are plain HDMI sinks with no compression stage.
4. Only if DisplayLink hardware is already on hand or free, and the box
   runs a mainstream distro (Ubuntu, Fedora, Arch) with evdi plus the
   official DisplayLink Linux driver installed: acceptable fallback, with
   the caveat from section 2 that its own compression stage means it is
   not meaningfully more "correct" than the ScreenDock in principle, only
   better supported in practice, and small DisplayLink specific panels in
   the 5 to 7 inch class are hard to find, so this mainly applies to docks
   driving a separate monitor rather than an all-in-one small panel.

Do not keep pursuing reverse engineering of the CABLETIME ScreenDock's HID
H.264 protocol as a path to a better experience. Even a perfect
reimplementation of its protocol is still bottlenecked by a sub-megabyte
per second HID interrupt pipe carrying a lossy, re-encoded H.264 stream,
which is an inherently worse signal path than a real DisplayPort/HDMI
connection that was available the whole time for roughly the same money.
The reverse engineering work already done in this project (see
mac-driver-analysis.md, usbdisplay-exe-ghidra-analysis.md,
display-protocol-reverse-engineering.md) is useful documentation of how the
dock works, but it should not be treated as a stepping stone toward a
usable Linux driver for this device; the ceiling on quality is the dock's
own encode pipeline, not anything a driver could fix.

## Confidence Assessment

- High confidence: DP Alt Mode portable monitors in the 5 to 7 inch class
  are not a real product category today (consistent absence across
  multiple vendor and marketplace searches); DisplayLink's own pipeline
  uses evdi plus a proprietary compressed transport, confirmed directly by
  DisplayLink's support documentation; USB-C to HDMI/DP adapters that
  expose the GPU's native DP Alt Mode signal are plug-and-play on Linux
  with no driver, confirmed by multiple independent community sources;
  small HDMI panels for Raspberry Pi style use are cheap, mature, and
  driver-free for video.
- Medium confidence: exact current prices for the specific listed products
  (GeeekPi, CrowPi, DeskPi, Plugable, Cable Matters) since retail pricing
  fluctuates and the pages pulled are a mix of manufacturer and marketplace
  listings rather than live checkout prices.
- Could not verify: whether any boutique or very recent (late 2026)
  product specifically markets itself as a sub-7 inch DP Alt Mode secondary
  screen; none turned up in this search pass, but a negative result here
  is not absolute proof none exists anywhere, only that it is not a
  findable, marketed category.

## Sources

- https://support.espres.so/en-US/using-the-correct-usb-c-cable-211367: confirms Espresso Display requires DP Alt Mode capable USB-C, used to establish portable DP Alt Mode monitors' minimum practical size class.
- https://www.usb.org/sites/default/files/D2T1-4%20-%20VESA%20DP%20Alt%20Mode%20over%20USB%20Type-C.pdf: VESA/USB-IF DP Alt Mode bandwidth and DSC figures.
- https://plugable.com/blogs/news/exploring-displayport-dp-alt-mode-mst-dsc-and-key-advancements-in-video-connectivity: DP Alt Mode, DSC, and lane allocation explanation.
- https://plugable.com/products/usbc-pdmon: vendor description of DP Alt Mode giving "native-level performance without extra software or drivers," used as representative of true Alt Mode monitors.
- https://support.displaylink.com/knowledgebase/articles/1104056-why-has-displaylink-not-released-source-code-for-t: DisplayLink's own description of its evdi-to-DL3-compression-to-USB pipeline, direct evidence it is software compressed, not a native sink.
- https://github.com/DisplayLink/evdi: evdi project description and architecture (kernel DRM module, libevdi wrapper).
- https://github.com/displaylink-rpm/displaylink-rpm: confirms current DL-3xxx through DL-7xxx family Linux packaging and that it bundles evdi plus the proprietary manager.
- https://wiki.archlinux.org/title/DisplayLink: practical Linux setup confirming the evdi plus proprietary driver two part model.
- https://community.frame.work/t/solved-linux-support-for-usb-c-to-hdmi-on-amd-rx-7700s-yes-lspci-wakes-it-up/54015: confirms USB-C to HDMI adapters are native DP Alt Mode to HDMI protocol converters relying on GPU/OS DP support, not separate drivers.
- https://askubuntu.com/questions/1322169/usb-to-hdmi-adapter-that-works-with-ubuntu: distinguishes driver-free native USB-C video adapters from DisplayLink-style USB graphics adapters that need drivers; approximate pricing ($15 to $30).
- https://www.nytimes.com/wirecutter/reviews/best-usb-c-video-cables-and-adapters: pricing and model references for USB-C to HDMI adapters.
- https://www.crowpi.cc/products/raspberry-pi-monitor-5-inch-hdmi-touchscreen-display-800x480-compatible-with-raspberry-pi-4-3b-3b-2b-bb-black-banana-pi-windows-10-8-7: 5 inch HDMI panel spec and "driver-free" claim.
- https://www.amazon.com/clp/B01L6O2NEE: GeeekPi 5 inch HDMI monitor listing.
- https://deskpi.com/products/deskpi-7-84-inch-touch-screen-1280x400-tft-lcd-display-for-10-inch-2u-rack-rackmate-supports-installation-of-t0-t1-t2: DeskPi 7.84 inch rack mount panel, price and server monitoring use case.
- https://www.youtube.com/watch?v=9oGIGiwP-oI: real world use of the DeskPi 7.84 inch rack panel for a server status dashboard, HDMI-only, "no weird drivers."
- /home/andremmfaria/projects/cabletime-screendock-8in1/docs/research/mac-driver-analysis.md: this project's own confirmed H.264/HID pipeline details, used as the baseline being compared against.
- /home/andremmfaria/projects/cabletime-screendock-8in1/docs/research/usbdisplay-exe-ghidra-analysis.md: this project's own Ghidra findings on the encode path, used as baseline.
