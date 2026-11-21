# CABLETIME ScreenDock (CB09W / CT-DKS9-PW) - Technical Specifications

Source of the core table: official CABLETIME product page spec sheet, cross-checked against the press release, New Atlas hands-on, and the YouTube unboxing/port walkthrough. Figures not found in any of these are marked "not found" - none are invented.

## Identification
| Field | Value |
|---|---|
| Model | CB09W |
| SKU | CT-DKS9-PW |
| Brand / Manufacturer | CABLETIME (Ningbo Cabletime Electronics Co., Ltd., Ningbo, China; founded 2007) |
| Product family | "ScreenDock" - distinct from CABLETIME's other docks (e.g. CB14G 15-in-1, 20-in-1 Thunderbolt dock, 10-in-1 hub) |

## Port list / layout (8-in-1)
Per the official spec table ("Output" row) and the YouTube port walkthrough, which also describes physical placement:

| Port | Spec |
|---|---|
| HDMI out ×1 | 4K @ 60Hz |
| USB-C (HOST) | Connects to computer; carries USB data + DisplayPort Alt Mode video. **Not a charging-only port** - host cable must support both data and video. |
| USB-C 3.0 (data) | 5Gbps per the YouTube reviewer's on-device port labeling; described in official spec as "USB-C 3.0" |
| USB-A 3.0 | labeled "5Gbps" on the device in the review video |
| USB-A 2.0 | labeled "480Mbps" on the device in the review video (used in user guide for mouse/keyboard) |
| SD card slot | SD 3.0 |
| TF (microSD) card slot | TF 3.0 |
| LAN / Ethernet (RJ45) | 1 Gbps |
| USB-C PD | 100W power **input** (pass-through) |

That's 8 functional ports/slots total (HDMI, USB-C data, USB-A 3.0, USB-A 2.0, SD, TF, LAN, USB-C PD) in addition to the dedicated USB-C HOST connector used purely for the computer link - matching the "8-in-1" naming. Physical layout per the user guide/video: HOST sits between the PD 100W port and the card slots; Ethernet socket is at the far left; SD/TF slots sit between HOST and the USB-C 5Gbps port; brightness and power buttons are beside the USB-A ports; ports are placed on two sides only so the unit can stand upright like a photo frame.

## Built-in display
| Field | Value |
|---|---|
| Size | 5.5 inches |
| Resolution / refresh | 1280×720 @ 60Hz ("720P") |
| Type | Foldable panel, adjustable viewing angle |
| Touch | Non-touch; controlled via the host computer's mouse/trackpad/keyboard |
| Brightness | 3 levels, dedicated brightness button; separate physical POWER button to turn the screen on/off independent of the dock |
| Recognized as | A real secondary display (via DisplayPort Alt Mode) once the "USB Display" driver is installed - not a standalone smart panel |

## External HDMI output
- Up to 4K @ 60Hz (requires a compatible computer GPU/USB-C port, cable, and display - not guaranteed on every host)
- MST (Multi-Stream Transport) is referenced as supported on Windows & Mac ("Mac&Win MST" per Kickstarter tagline) for multi-screen setups, though CABLETIME's own troubleshooting text frames the HDMI output and the built-in screen as two independently-driven outputs rather than detailing MST behavior - **treat MST support as vendor-claimed, not independently verified in a review**.

## Power delivery
| Field | Value |
|---|---|
| Max PD input | 100W (via USB-C PD charger, sold separately - not included in box) |
| Max pass-through to laptop | Up to 85W per CABLETIME's own user guide ("accepts up to 100W input and supplies up to 85W to a compatible laptop - the 100W input rating does not mean 100W reaches the laptop") |
| Charging cable needed | A cable rated for the power level desired; shared across other connected devices if using a multiport charger |

## Data transfer speeds
| Port | Speed (as labeled on-device, per review video) |
|---|---|
| USB-C data port | 5Gbps |
| USB-A (blue) | 5Gbps |
| USB-A (black) | 480Mbps (USB 2.0) |
| HOST USB-C | Data + DP Alt-Mode video combined; not used for general bulk data in typical use |

All speeds are *maximum link rates*; CABLETIME's own user guide notes actual throughput also depends on the drive/cable/host.

## Card reader
- SD 3.0 slot
- TF (microSD) 3.0 slot
- No independently-verified real-world transfer benchmark found (reviews describe only that the slots exist and are "SD 3.0 / TF 3.0" branded; no measured MB/s reported anywhere found).

## Ethernet
- RJ45, up to 1 Gbps "with suitable network equipment, cabling and settings" (CABLETIME's own caveat - rated speed does not guarantee that internet throughput).

## OS / device compatibility
| Platform | Status |
|---|---|
| Windows | Supported; driver "USB Display" for Windows, installed via auto-run executable or manual download |
| macOS | Supported; minimum macOS version stated in the Mac driver documentation is **macOS 11.0**; requires granting the USB Display app "Screen Recording" permission in Privacy & Security |
| ChromeOS | **Not found** - no official statement or review confirms/denies ChromeOS compatibility |
| Android / iPad | **Not found** - product is marketed as a PC/Mac desktop accessory; no iPadOS/Android support is claimed or tested anywhere in the sources reviewed |
| Apple Silicon (M-series) Macs specifically | Not explicitly itemized by chip; general "Mac" support is claimed, used successfully with a "Mac Mini" in the YouTube review, but no M1/M2/M3/M4-specific statement found |

Requirement for all platforms: the computer's USB-C port must support DisplayPort Alt Mode (video-capable USB-C), and the host cable must carry both data and video - a charge-only cable will not drive the built-in screen.

## Driver downloads (official, direct links found)
- Windows driver: `https://cdn.shopify.com/s/files/1/0642/3091/6354/files/WindowsUSBDisplay.rar` (RAR archive, named "USBDisplay.exe" inside)
- macOS driver: `https://cdn.shopify.com/s/files/1/0642/3091/6354/files/MacOS_USB_Display.app.zip`
- Both linked from the official user-guide page and the product page; no separate "driver portal" exists beyond these direct CDN files.

## Dimensions / weight / material
| Field | Value |
|---|---|
| Material | Plastic - PC+ABS (per official spec table) |
| Form factor | Foldable screen on a dock base; described as "pocket size" / good for travel in the YouTube review |
| Exact dimensions (mm/in) | **Not found** - no official datasheet, retailer listing, or review stated precise physical dimensions |
| Weight | **Not found** - not stated in any source reviewed |

## Warranty
- 2-year warranty per the official user guide ("ScreenDock has a two-year warranty, subject to your purchase channel and the current CABLETIME warranty policy"). CABLETIME's other similar docking stations (e.g. the unrelated CB14G 15-in-1) are also sold with a 2-year manufacturer warranty, consistent with brand-wide policy.
- Support contact: support@cabletimetech.com - CABLETIME asks for the model (CT-DKS9-PW), computer model/OS, driver version, and a photo/screenshot of the issue.

## Explicitly unverified / not found
- Exact product dimensions and weight
- Independent lab-measured data-transfer or card-reader benchmark numbers
- ChromeOS and Android/iPad compatibility (neither confirmed nor denied by any source)
- Independently confirmed MST multi-monitor behavior (vendor claim only)
- Formal PDF datasheet or printed manual (none located - CABLETIME publishes an HTML "user guide" page instead)
