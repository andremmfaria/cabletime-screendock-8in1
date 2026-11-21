# Display protocol reverse engineering (path B progress)

Goal: get enough of the dock's proprietary framebuffer-over-HID protocol
to eventually push a rendered terminal to the 5.5" screen from this Linux
host. Status: real progress on transport and architecture, wire format
(compression/header bytes) still unknown; two concrete next steps listed
at the end.

## 1. Tooling

Ghidra upgraded from 11.4.1 to the latest release, **12.2**
(`ghidra_12.2_PUBLIC_20261016.zip`, installed to
`/home/andremmfaria/tools/ghidra_12.2_PUBLIC`, needed a user-writable
path since `/opt` isn't writable by this account). REA's `doctor` check
now reports `ghidra: healthy, 12.2`. Kept the old 11.4.1 install at
`/opt/ghidra` as well: Ghidra 12.x moved the Jython scripting engine out
into a separate, not-yet-installed extension, so headless `.py` scripts
default to PyGhidra and fail with `Python is not available`. All custom
Jython-based decompile scripts in this project still run against the
11.4.1 install; 12.2 is there specifically to satisfy REA's native-engine
version check, not yet used for scripting.

## 2. Port question (see also docs/capabilities.md and docs/security-assessment.md)

Resolved: the built-in screen needs nothing beyond the HOST port already
connected. Confirmed from CABLETIME's own user guide and from this host's
live USB topology (display device hangs off the same hub chain as HOST,
reachable via `/dev/hidraw0`/`hidraw1` with nothing else plugged in).

## 3. Live USB capture (usbmon)

Captured `/sys/kernel/debug/usb/usbmon/1u` (bus 1, where the dock sits)
for 90 seconds while idle, plus four deliberate test writes to the two
HID interfaces via raw `os.write()` on `/dev/hidraw0`/`hidraw1` (as root,
needed since the nodes are `root:root 0600`).

Findings:
- Idle traffic on device 015 is routine SCSI polling of the virtual
  CD-ROM's mass-storage LUN (`USBC`/`USBS` CBW/CSW signatures on bulk
  endpoint 5) — unrelated to the display, not worth chasing further.
- All four test writes (`report_id + 16 bytes` to each of endpoint 1 and
  endpoint 2, interrupt OUT) appear in the capture exactly as sent, 17
  bytes each, completing with status 0, no STALL/EPIPE. Confirms the
  kernel's raw hidraw write path reaches the device cleanly with no extra
  framing imposed by the driver stack.
- Separately, the kernel's generic `usbhid` driver already has oversized
  interrupt-IN reads queued on the same two endpoints, requesting up to
  32768 and 4096 bytes respectively — matching the descriptor's declared
  huge input-report sizes (12287 and 4095 bytes) found in the earlier
  security pass. This confirms the transport is alive and ready for large
  payloads in both directions; it does not by itself tell us what a real
  payload looks like, since nothing in this test triggered the device to
  send back structured data (no official driver was running to generate
  real traffic, and raw garbage bytes don't produce a defined response).

## 4. Ghidra decompile of the Swift driver (architecture confirmed, wire format not yet)

Ran a broader symbol search (all 32,296 symbols, not just functions) in
the Mac driver binary and found the real Swift class/method structure
behind the `AMHIDEventFakeMouseProcessor` family found in the earlier
pass:

```
AMHIDDevice
AMHIDStreamingPipeline
AMHIDStreamingPipeline.SurfaceQueue   <- nested class, holds a queue of "Surfaces"
AMHIDStreamingPacketizer
AMHIDStreamingPacket
AMHIDEventProvider
AMHIDEventProviderSourceDisplayResolutionDelegate
AMHIDEventFakeMouseProcessor
FakeMouseSwift
GenericDeviceHIDEventListener
StreamingPacketFileOutput             <- a debug sink that writes streaming packets to a file
streamingPipeline / streamingPipeline_is_ready_to_start
hidEventHandler
```

"Surface" is Apple's standard term for a GPU/video pixel buffer
(IOSurface). A `SurfaceQueue` feeding a `Packetizer` that produces
`Packet` objects is exactly the shape we'd expect for "take a rendered
frame, chop it into HID-sized chunks, send each chunk as a report" —
this independently confirms the earlier bandwidth-based guess (a raw
1280x720 frame doesn't fit over the ~750KB/s interrupt pipe without
chunking/compression) and narrows the unknown to the packetizer's actual
byte layout, not the overall approach.

**What's still missing:** the actual method bodies. Ghidra's headless
auto-analysis found these as class-metadata/string symbols and vtable
entries, not as directly callable functions it could decompile — Swift
dispatches class methods through protocol witness tables, which headless
batch scripting doesn't resolve on its own. Getting the real packetizer
byte layout (header bytes, chunk size, any compression) needs one of:

1. **Interactive Ghidra session**: open this same imported program in
   the Ghidra GUI, manually locate `AMHIDStreamingPacketizer`'s vtable,
   and decompile its methods by hand (Swift method resolution is
   workable interactively, just not reliably scriptable headlessly in
   the time spent here).
2. **A real capture against the actual official driver**: run `USB
   Display.app` on real macOS hardware (or the Windows exe in a VM) with
   the dock attached, capture USB traffic during that session (`usbmon`
   if passed through to a Linux VM host, or Wireshark/USBPcap on
   Windows), and correlate bytes-on-the-wire with what appears on the
   physical screen. This is the more reliable path: ground-truth traffic
   beats static analysis of Swift dispatch every time, and `StreamingPacketFileOutput`
   suggests the app may even have a debug mode that dumps the packet
   stream to a file directly, which would be the fastest way to get the
   exact format if it can be enabled.

## 5. Codec identified via pyghidra + VideoToolbox (major update)

Re-ran the decompile with `pyghidra` (pip package, drives Ghidra's real
Objective-C analyzer far better than the plain headless Jython path) and
got actual decompiled method bodies this time, not just symbol names. Found
the real encoder setup in `AMh264ImageCompressor::setup`:

```
_VTCompressionSessionCreate(0, width, height, 0x61766331, hwSpec, 0, 0, 0, self, &session);
// 0x61766331 = 'avc1' = H.264
_VTSessionSetProperty(session, kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse);
_VTSessionSetProperty(session, kVTCompressionPropertyKey_RealTime, kCFBooleanTrue);
_VTSessionSetProperty(session, kVTCompressionPropertyKey_ExpectedFrameRate, fps);
```

Plus `kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder`
and a `H264HardwareAccelerationValidator` that falls back to software
encoding if hardware isn't available. **The screen is a real-time H.264
video sink**: no B-frames (low latency), hardware-accelerated when
possible, configurable frame rate. This fully explains the earlier
bandwidth puzzle (a raw 1280x720 frame doesn't fit over ~750KB/s, but an
H.264 stream easily does) and gives a concrete target: render -> H.264
encode (e.g. ffmpeg libx264, `-tune zerolatency -bf 0 -profile baseline`
matches the discovered settings) -> get the NAL unit stream the real
driver would also produce.

The `AMHIDStreamingPacketizer.process` method that chunks NAL units into
HID packets still never resolved to a decompilable function even under
pyghidra's better analysis (it only appears as a string/debug-log
symbol) — Swift's protocol-witness-table dispatch is hiding the real
method body. The packet *header* format is still unknown.

## 6. Empirical live-fire test (Ghidra upgraded to 12.2 at /opt/ghidra,
replacing 11.4.1; old `~/tools` copy removed per instruction; Jython
extension installed for 12.2, but `pyghidra` pip package turned out to be
the better scripting path and is what produced the section 5 findings)

Generated a real H.264 keyframe test file with ffmpeg (`test_frame.h264`,
1280x720, baseline profile, zerolatency, `keyint=1`, matches the
discovered encoder settings) and tried three live framing hypotheses
against the real hardware, asking the user to check the physical screen
after each:

1. Report 1 (interface 0) via `/dev/hidraw0`, raw chunks up to 4094 bytes,
   no header. Wrote cleanly (5 chunks, no USB errors). **No visible
   change** - screen stayed on the dock's own built-in idle/setup UI
   (confirmed this idle screen itself is rendered by the Actions chip's
   own onboard firmware, independent of any host software - "ver
   23099135" visible in the corner is the chip's firmware version, not
   anything we sent).
2. Report 2 (interface 1) via `/dev/hidraw1`, raw chunks up to 12286
   bytes, no header: **`ETIMEDOUT` on the very first write**, every time,
   regardless of chunk size (also failed re-chunked to 760 bytes to
   match the endpoint's real 768-byte max packet). This isn't a sizing
   bug - the endpoint persistently refuses OUT data. Combined with the
   Mac driver's `AMHIDEventFakeMouseProcessor` reading HID *input*
   reports to synthesize mouse/touch events, the working theory is now:
   **interface 1 / report 2 is a device-to-host input/touch-event
   channel (IN direction), not a video sink** - interface 0 / report 1
   is the one actually built to accept host-to-device OUT traffic.
3. Tried the undocumented Feature report (ID 0x20, Digitizer usage page,
   2 bytes) as a possible "arm streaming" handshake via `HIDIOCSFEATURE`,
   several plausible values - **hard STALL (`EPIPE`) on both interfaces,
   every value tried**. Confirms this feature report is declared in the
   descriptor but not actually implemented by this SKU's firmware
   (vestigial, inherited from a touch-capable sibling product, as
   theorized in the earlier Mac-driver pass).
4. Retried report 1 with endpoint-correct 63-byte chunks (matching its
   real 64-byte max packet, vs. the oversized 4094-byte chunks in
   attempt 1): all 310 chunks sent with zero USB errors. **Still no
   visible change on the physical screen.**

## Bottom line for "can we display a terminal on it right now"

Still no. Real progress was made (port question resolved, codec identified
as real-time H.264/avc1 via VideoToolbox, transport confirmed live and
channel roles narrowed to "report 1 = host-to-device, report 2 = probably
device-to-host input"), and real H.264 test data was sent to real hardware
multiple times with correct chunk sizing, with zero USB-level errors. None
of it produced any visible change on the physical screen.

This means the remaining unknown is a genuine protocol detail we have not
yet found: either a required handshake/mode-switch sequence that must
precede streaming (the one documented Feature report we tried, ID 0x20,
hard-stalled, so it's not that one - there may be another undocumented
command, possibly sent as a specific first OUT report rather than a
Feature report), or a packet header/framing convention inside report 1
itself that our raw-passthrough attempts didn't include, or the chunk
boundaries need to align with H.264 NAL unit boundaries rather than
arbitrary byte offsets. Blind guessing has a low hit rate against an
undocumented binary protocol and burns the user's attention checking the
screen each time; continuing to narrow it further without a ground-truth
capture (real official driver running against this hardware, traffic
captured with usbmon/Wireshark) or a successful interactive Ghidra trace
of `AMHIDStreamingPacketizer`'s actual vtable method is likely to need
many more low-probability iterations rather than a few more.
