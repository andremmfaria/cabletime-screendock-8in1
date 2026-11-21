Correlating the Windows and Mac drivers: what we actually have, and
hypotheses about the protocol they share.

Honest framing up front: this correlation is asymmetric. The Mac side
gave deep structural evidence (real Swift classes, a decompiled
VideoToolbox H.264 encoder setup, a named HID packetizer pipeline). The
Windows side gave almost nothing usable at the code level, for a
concrete reason explained below. What follows is what we can respectively
confirm from each, then hypotheses built from combining them plus one
structural argument that doesn't depend on either binary.

1. What we have from each side

Windows (USBDisplay.exe): a Ghost Installer Studio (Borland/Delphi-era
generic installer toolkit) wrapper, properly Authenticode-signed by
"Actions Microelectronics Co., Ltd.", PE version info reading
FileDescription "USB Display", Company "Actions-Micro", FileVersion
1.0.2.95, Copyright 2025. VirusTotal: 0/71. It bundles two embedded
payload files as PE RCDATA resources, COMREGC (5,632 bytes) and UNPACK
(35,328 bytes), both referenced by the installer's own "Can't extract
COMREGC.exe" / "Can't extract unpack.dll" error strings.

Pulled both directly from the PE resource table with `pefile` (7z choked
on them, 0-byte output, "Data Error"). Both are high entropy (7.86 and
7.91 bits/byte respectively, true-random territory) and don't respond to
plain zlib/raw-deflate decompression - encrypted or proprietarily
obfuscated by the installer toolkit itself, not casually recoverable.

Corroborating evidence this is generic toolkit infrastructure, not
vendor driver code: the same resource table also contains `DVCLAL` and
`PACKAGEINFO`, which a web search confirms are standard Delphi/
C++Builder VCL runtime-package metadata resources present in any binary
built with that toolchain - boilerplate, not application logic. COMREGC
and UNPACK sit in the identical resource category alongside them, and
their names match a textbook pattern for Delphi-era installer builders:
a "COM registration" helper utility and the installer's own unpacker
library. Conclusion: **the real Windows "USB Display" driver application
was never captured in this pass.** What ships inside this installer and
what we examined is the installer wrapper only; the actual driver binary
gets written to disk only during a real Windows install, which didn't
happen here (no Windows/Wine execution was performed, by design, per the
earlier decision to avoid running untrusted code).

Mac (USB Display.app): the full picture from the two earlier passes -
bundle ID `com.winnerwave.USBDisplay`, Sparkle auto-update pointed at
ezcast.com (so this is a white-labeled AWIND/EZCast build), and real
decompiled Swift/Objective-C structure: `AMHIDDevice`,
`AMHIDStreamingPipeline` -> `SurfaceQueue` -> `AMHIDStreamingPacketizer`
-> `AMHIDStreamingPacket`, an `AMh264ImageCompressor` that calls
`VTCompressionSessionCreate` with codec `0x61766331` ('avc1' = H.264),
hardware-accelerated when available, real-time mode, no B-frames. Also
`AMHIDEventFakeMouseProcessor`/`CGEventPost` for synthesized mouse input
from the same vendor-opaque HID channel.

2. A structural argument that doesn't depend on either binary

The dock's firmware (the Actions Microelectronics chip) is a single,
fixed piece of hardware that has no idea which OS is talking to it. Both
the Windows and Mac drivers MUST produce byte-for-byte the same kind of
data over the same HID interface for the chip's decoder to render it,
because the chip only has one decoder implementation on one side of the
wire. This means: **whatever wire format the Mac driver produces is very
likely identical to what the Windows driver produces**, regardless of
each driver's internal implementation language or video API. This is
the strongest reason to trust the H.264/avc1 finding even without ever
seeing the real Windows driver's code - it's a property of the firmware,
not of either host app.

3. Hypotheses

H1 (high confidence): the Windows driver also encodes to H.264, almost
certainly via Windows Media Foundation's hardware encoder path
(`IMFTransform` / Intel Quick Sync or equivalent) rather than a software
x264, mirroring the Mac side's VideoToolbox-with-hardware-fallback
design. This is an architectural mirror-image guess from the platform's
standard equivalent API, not evidence from the Windows binary itself.

H2 (medium confidence): the Windows driver likely has an equivalent
input-injection capability to the Mac's `AMHIDEventFakeMouseProcessor`,
if it's built from the same shared EZCast/Actions reference SDK implied
by the Mac bundle ID. Not confirmed - we have zero Windows code evidence
either way, this is inferred purely from "same vendor, same chip, same
likely shared codebase" reasoning.

H3 (the actual blocker, unresolved by this correlation): the exact HID
packet header/framing the firmware expects is a firmware-side protocol
detail, so finding it in the Windows driver would be just as hard as in
the Mac driver, for the identical reason - it would be implemented
through whatever this chip vendor's proprietary SDK wraps on both
platforms, and Ghost Installer's encryption of COMREGC/UNPACK means we
can't even get Windows-side code to try. **Correlating the two drivers
does not unblock the packet-framing problem** - both sides require
either a live capture against real hardware or a successful interactive
Ghidra vtable trace, exactly as concluded in
docs/research/display-protocol-reverse-engineering.md. The genuinely new
information from this pass is the Delphi/Ghost-Installer provenance of
the Windows payloads (ruling that avenue out cleanly) and the structural
argument in section 2 (raising confidence in H.264 without needing
Windows-side confirmation).

4. What would change this

If real Windows hardware or a Wine/VM environment becomes available to
actually run the installer and capture the real driver binary it drops
to disk, that binary (not the installer wrapper) would be worth the same
pyghidra-based Objective-C/x86 decompile treatment given to the Mac app,
and a live USB capture of either driver running against the real dock
would resolve the packet-framing question directly - see section 6 of
the display-protocol doc for that plan.
