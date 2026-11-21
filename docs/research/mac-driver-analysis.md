# macOS driver (USB Display.app) analysis

Pulled from the official download link in docs/specs.md:
`https://cdn.shopify.com/s/files/1/0642/3091/6354/files/MacOS_USB_Display.app.zip`
(2,074,634 bytes, zip archive, downloaded clean, HTTP 200).

Chose this over fighting Wine with the Windows Ghost Installer exe: a macOS
`.app.zip` is a plain zip, no execution needed to inspect it, Mach-O
binaries read directly with `file`/`strings`. Correct call, this path
surfaced the single most important finding of the whole investigation (see
section 3).

## 1. Windows exe hash verdict (VirusTotal)

Submitted the SHA-256 of the signed original (`b3bac5ea2ac59f9f06a456a6d6b278ffbb2ec471b4dc00ce0b027ad7defebfd9`)
to VirusTotal's public lookup. Result: **0/71 vendors flagged it malicious**,
tagged `peexe`, `overlay`, `upx`, `signed`, `executes-dropped-file`,
`detect-debug-environment`. All consistent with the earlier static findings
(signed Ghost Installer wrapper that self-extracts bundled files); nothing
new or concerning here.

## 2. Package identity: this is NOT bespoke CABLETIME/Actions code

`Info.plist` of the main app:

```
CFBundleIdentifier: com.winnerwave.USBDisplay
CFBundleVersion: 9
LSMinimumSystemVersion: 11.0
LSUIElement: true   (background agent, no Dock icon/menu bar app)
```

Auto-update config in the same plist (Sparkle framework):

```
SUFeedURL: https://www.ezcast.com/upgrade/appupgrade/usb-display/maccast.xml
SUPublicDSAKeyFile: dsa_pub.pem
```

Binary strings reference `EZCastApp.EZChannel.*` class names and
`"due to ezcast fw resolution limit"`. **This app is a white-labeled build
of AWIND/EZCast's screen-casting product line**, not something CABLETIME or
Actions Microelectronics wrote from scratch. Explains why interface names
and HID-handling code look far more capable than a simple secondary-display
driver needs to be: it's shared code from a touch/interactive casting
product, repackaged for this non-touch dock.

Sparkle auto-update is signed with an embedded DSA public key
(`dsa_pub.pem`), which is the correct, secure way Sparkle verifies update
packages — an attacker would need the matching private key (presumably
held by EZCast/Winnerwave) to push a malicious update through this channel.
Standard risk profile for any app with built-in auto-update, nothing dock-
specific.

One weaker spot: `NSAppTransportSecurity` carries an explicit exception
allowing **plain HTTP** (not HTTPS) to `http://cdn.iezcast.com/`. Content
served from that CDN over HTTP is interceptable/tamperable on a hostile
network (classic MITM target). Did not identify exactly what's fetched from
that domain in this pass (likely icons/assets for the broader EZCast app
family, not confirmed).

## 3. The dock's vendor-opaque HID channel feeds real system-wide input injection [confirmed, most significant finding]

The earlier pass (Linux-side USB descriptor inspection) found the device's
two HID interfaces declare Usage Page `0xFFFF` (fully vendor-opaque), and
concluded from that alone that the device "cannot impersonate a
keyboard/mouse at the USB protocol level." **That conclusion is still
correct about the raw USB/kernel HID layer, but it does not extend to what
the vendor's own userspace driver does with that opaque data** — and this
is exactly where the real capability sits.

Binary symbols in `USB Display.app/Contents/MacOS/USB Display` (Swift +
Objective-C, universal x86_64/arm64):

```
AMHIDEventProvider
AMHIDEventProviderSourceDisplayResolutionDelegate
AMHIDEventFakeMouseLocationTranformer
AMHIDEventFakeMouseProcessor
AMHIDEventLastClick
AMHIDStreamingPipeline
AMHIDStreamingOutput / AMHIDStreamingPacket
IOHIDManagerCreate / IOHIDManagerSetDeviceMatching /
IOHIDManagerRegisterDeviceMatchingCallback / IOHIDDeviceOpen /
IOHIDDeviceGetProperty / IOHIDDeviceRegisterInputReportWithTimeStampCallback
```

And, critically, direct imports of Apple's **CoreGraphics event-injection
API**:

```
CGEventCreate
CGEventCreateMouseEvent
CGEventCreateScrollWheelEvent2
CGEventGetLocation
CGEventSetIntegerValueField
CGEventPost
CGWarpMouseCursorPosition
```

Put together: the app opens the dock's vendor-opaque HID interface with
`IOHIDManager`, registers for input reports, runs them through an
`AMHIDEventFakeMouseProcessor` / `AMHIDEventFakeMouseLocationTranformer`
pipeline, and **posts the result as real synthetic mouse events system-wide
via `CGEventPost`** — the exact same API legitimate remote-control tools
(and plenty of malware) use for mouse/cursor injection on macOS.

This means the dock + its official Mac driver together have a genuine,
non-theoretical local input-injection capability: whatever HID reports the
device's vendor-opaque interface sends, the host-side driver will interpret
and inject as real mouse movement, clicks, and scroll events into the
active macOS session. On this specific non-touch ScreenDock SKU there's no
documented control surface that should be generating such events (no touch
panel, no trackpad) — this is almost certainly dormant shared code
inherited from EZCast's touch-capable product line (see section 2) rather
than something this SKU's firmware actively drives today. But "dormant
code path, present and wired to the OS input queue" is a meaningfully
different risk than "not possible" — if the dock's firmware (official or a
tampered/counterfeit unit) ever sent the right HID report, this driver
would turn it into real mouse input with no further permission prompt.

**This revises the Linux-side security assessment's framing.** The earlier
"it's an opaque data pipe, not a HID-injection vector" conclusion should be
read as "not at the raw USB/kernel level" — at the application layer, on
macOS specifically, the vendor's own software is fully capable of mouse-
event injection sourced from that same opaque channel. Whether the
equivalent Windows driver (inside the still-unextracted `comregc.exe`/
`unpack.dll` payloads) does the same was not confirmed in this pass.

## 4. LoginItem helper

`USB Display AutoLaunch.app` (inside `Contents/Library/LoginItems/`) is the
standard macOS pattern for a login-item helper registered via
`SMLoginItemSetEnabled` (modern replacement for a LaunchAgent plist) — this
is exactly how legitimate macOS apps implement "launch at login," not a
persistence mechanism specific to this product. Binary is small (170KB),
strings show only `bundleIdentifier` of interest, consistent with a thin
helper whose only job is registering/launching the main app.

## Open items if revisited

- Confirm whether the Windows driver (inside the still-unextracted
  `comregc.exe`/`unpack.dll`) implements the same fake-mouse-injection
  pipeline, or whether this is Mac/EZCast-specific.
- Identify what `cdn.iezcast.com` actually serves over the allowed plain-
  HTTP exception.
- This pass did not attempt to trigger the fake-mouse pipeline live (would
  need macOS hardware with the dock attached and the app installed; not
  available here) — everything above is static-analysis-confirmed code
  presence, not an observed live exploit.
