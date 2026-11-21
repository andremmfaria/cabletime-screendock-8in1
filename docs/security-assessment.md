# Security assessment - live probe on this host

Confidence-tagged findings from directly plugging the CABLETIME ScreenDock (CB09W / CT-DKS9-PW) into this host and inspecting it at the USB/block/network layer. No destructive actions taken; CD gadget mounted read-only, unmounted after inspection.

## Device composition (confirmed via lsusb/udevadm/sudo)

| Chip | VID:PID | Role |
|---|---|---|
| Genesys Logic | 05e3:0626 (USB3) / 05e3:0610 (USB2) | Internal hub bridging |
| Terminus Technology | 1a40:0801 | Secondary USB2 hub |
| **Actions Microelectronics "USB_DISPLAY"** | **1de1:f201** | Built-in 5.5" panel - composite device, see below |
| Genesys Logic USB Storage | 05e3:0764 | SD/TF card reader (empty slots enumerate as 0-byte `sda`/`sdb`) |
| Realtek RTL8153 | 0bda:8153 | Gigabit Ethernet - mature, unremarkable chip |

## Finding 1 - the built-in screen is NOT a standard USB video/display class device [confirmed]

The `1de1:f201` identity is a single composite USB device bundling **5 interfaces under one descriptor**: 2× HID, 1× Audio Control+Streaming (speaker passthrough, labelled "USB Display Audio (HDMI 1)"), and 1× Mass Storage (the virtual CD, see Finding 2). There is no USB Video Class (UVC) or DisplayLink-style interface at all.

This means the panel is driven by a **proprietary protocol tunneled over plain HID interrupt transfers** - a common cost-cutting trick for cheap secondary-display modules, since it avoids needing a signed kernel driver on Windows (HID devices get a generic driver "for free").

**Checked whether this is a disguised HID-injection ("BadUSB") device** - it is not. Pulled the raw report descriptors with `usbhid-dump` (root):

```
06 FF FF 09 00 A1 01 85 02 15 00 25 FF 19 00 29 FF 75 08 96 FF 2F 81 02 ...
```

`06 FF FF` = Usage Page **0xFFFF, fully vendor-defined** - not Generic Desktop (0x01) or Keyboard (0x07). Both HID interfaces (report IDs 0x01 and 0x02) declare only vendor-opaque byte arrays (255/4095-byte IN and OUT reports), not keys or pointer movement. So this specific device cannot impersonate a keyboard/mouse at the USB protocol level - it's an opaque data pipe, not a HID-injection vector. Rules out the most common "evil docking station" attack class for this unit.

Residual risk: any local process with access to `/dev/hidraw*` for this device can write arbitrary vendor-opaque data to the dock (push garbage/junk to the screen) - low severity, local-only, not a remote/supply-chain vector.

## Finding 2 - the dock ships an autorun CD-ROM gadget with an unsigned, packed Windows binary [confirmed, moderate concern]

One of the 5 composite interfaces (`bInterfaceClass 8 Mass Storage`, bulk-only/SCSI) presents itself to the OS as a literal CD-ROM (`SCSI_MODEL=File-CD_Gadget`, `ID_USB_INTERFACES` includes `:080650:`). This is the exact same USB mass-storage subclass/protocol (bulk-only transport, SCSI transparent command set, `08:06:50`) used by real optical drives - and by malicious "autorun USB drive" attacks.

Mounted read-only; contents:

```
USBDisplay.exe 7,579,696 bytes - PE32, Intel i386, UPX-packed, no Authenticode signature found
USBDisplay.ico 20,257 bytes
autorun.inf 81 bytes
```

`autorun.inf` content:
```
[autorun]
icon="USBDisplay.ico"
label="USB Display"
open=""USBDisplay.exe""
```

This is functionally **identical to the classic USB autorun/BadUSB social- engineering delivery mechanism** - a device that enumerates as an optical disc and offers to auto-launch an .exe the instant it's plugged into Windows. The difference between "legitimate bundled driver install" and "attack vector" here is entirely about supply-chain trust, not mechanism:

- The binary is **UPX-packed** (blocks static analysis without unpacking;
used both by legitimate small vendors to shrink binaries and by malware to evade signature AV - packing alone proves nothing either way).
- **CORRECTED (see docs/research/usbdisplay-exe-ghidra-analysis.md): it IS Authenticode-signed.** The earlier claim of no signature only checked `strings`, which cannot see a binary PKCS#7 blob. Proper check with `osslsigncode verify` against the original exe found two valid signatures (SHA1 and SHA256) signed by "Actions Microelectronics Co., Ltd." chained through DigiCert, RFC3161-timestamped May 13 2025, both verifying OK. This is routine vendor driver-package signing, not an anonymous dropper.
- SHA-256 of the shipped exe: `b3bac5ea2ac59f9f06a456a6d6b278ffbb2ec471b4dc00ce0b027ad7defebfd9`, not yet checked against VirusTotal/any threat-intel source; do that before running it on any Windows machine if you want extra assurance beyond the signature check above.
- Because the CD image lives in the dock's own firmware/flash, a compromised
firmware update (official or tampered) could silently swap this binary for something malicious, and Windows would auto-offer to run it with zero additional prompts beyond standard AutoPlay - a real, if vendor-common, supply-chain weak point. This is the same class of risk as any vendor- bundled "driver CD" dongle (common on KVMs, capture cards, cheap USB displays), not something unique to CABLETIME, but it is a genuine attack surface, not a theoretical one.

**Practical implication for you (Linux host):** none of this autorun mechanism fires on Linux - `udisks2`/GNOME etc. don't auto-execute from `autorun.inf`, and this host doesn't appear to auto-mount removable optical media automatically either (had to mount manually as root). The exposure is real only if this dock is plugged into a Windows machine with AutoPlay enabled.

## Finding 3 - standard chips, no distinctive concern [confirmed, low severity]

- Genesys Logic hub chips and the RTL8153 Ethernet adapter are extremely
common, mature silicon used across hundreds of dock/hub products; no dock-specific finding here beyond generic "USB-Ethernet dongle" risk (kernel `r8152` driver attack surface is a known general class, not specific to this unit).
- The card reader enumerates as two empty 0-byte generic mass-storage LUNs
(SD + microSD) - ordinary behavior, carries only the generic "untrusted card inserted" risk any card reader has.

## Not done / out of scope for this pass

- UPDATE: `USBDisplay.exe` has since been unpacked and run through Ghidra (full results in docs/research/usbdisplay-exe-ghidra-analysis.md). Summary: it's built with the generic "Ghost Installer" toolkit (not bespoke code), properly Authenticode-signed, and no path was found for the dock's firmware to trigger code execution on its own; execution only happens through the standard autorun-CD-plus-user-consent chain described above. The two payload files it extracts (comregc.exe, unpack.dll) were not themselves extracted or analyzed.
- Did not submit the SHA-256 hash to VirusTotal (would require an external
lookup you'd need to approve, since it discloses the hash to a third party - ask if you want this done).
- Did not attempt firmware extraction/dumping from the dock itself (no
obvious exposed firmware-update interface was found during enumeration; would need vendor update-tool traffic capture to investigate further).
- Did not test actual HID traffic content (what bytes the vendor's real
Windows driver sends/receives) since no Windows host was available here.

## Finding 4 - the official macOS driver turns the vendor-opaque HID channel into real system-wide mouse injection [confirmed, most significant finding, see docs/research/mac-driver-analysis.md]

Pulled the official macOS driver (`MacOS_USB_Display.app.zip`, link in specs.md) and inspected the Mach-O binary directly, no execution needed. Finding 1 above (HID Usage Page 0xFFFF, opaque, "not a keyboard/mouse at the USB protocol level") is still correct about the raw kernel HID layer, but does not extend to the vendor's own userspace driver code, which is where the real capability sits.

The Mac app opens the dock's vendor-opaque HID interface via IOHIDManager, runs incoming reports through functions literally named `AMHIDEventFakeMouseProcessor`/`AMHIDEventFakeMouseLocationTranformer`, and posts the result as real mouse movement/click/scroll events system-wide via macOS's `CGEventPost` (confirmed via direct imports of `CGEventCreateMouseEvent`, `CGEventPost`, `CGWarpMouseCursorPosition`, `CGEventCreateScrollWheelEvent2`). That is the same API real remote-control tools and plenty of malware use for input injection.

Also found: the app's bundle ID is `com.winnerwave.USBDisplay` and its Sparkle auto-updater points at `ezcast.com` - this driver is a white-label of AWIND/EZCast's touch-capable casting product, not bespoke CABLETIME/Actions code, which explains why it carries input-injection machinery a plain non-touch secondary-display dock has no documented reason to need. On this specific SKU the capability looks dormant (no touch panel to drive it), but the code path exists, is wired into the HID report pipeline, and would fire on whatever bytes the vendor-opaque channel delivers. Not tested live (would need real macOS hardware with the dock attached).

Whether the Windows driver does the same was not confirmed (its real logic lives in payload files inside the installer, `comregc.exe`/`unpack.dll`, not yet extracted).

## Bottom line

Revised after Finding 4. The device cannot register as a keyboard/mouse at the raw USB/kernel level (HID usage pages rule that out), and the Windows exe is a legitimately signed, generic Ghost Installer package that VirusTotal's 71 vendors all call clean. But the official macOS driver is capable of real system-wide mouse injection sourced from the same vendor-opaque HID channel, inherited from a shared EZCast codebase rather than written for this SKU. On this non-touch unit that path looks dormant, not actively driven by the dock's own firmware, but it is live code wired to the OS input queue, not a theoretical risk. Treat the dock as: safe for normal use, don't blindly click through a Windows AutoPlay prompt for the bundled driver (get it from CABLETIME's site directly), and be aware the Mac driver carries more capability than a plain secondary-display driver needs.
