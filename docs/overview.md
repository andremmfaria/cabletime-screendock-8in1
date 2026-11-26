# CABLETIME ScreenDock - 8-in-1 USB-C Docking Station with Built-in Display: Overview

## What it is
The CABLETIME ScreenDock is an 8-in-1 USB-C docking station with an **integrated 5.5" companion display** (marketed as 1280x720/720p, but confirmed capable of 1080p output at the cost of very small on-screen icons/text, and both resolutions show visible lag). CABLETIME's own marketing language describes it loosely as a "secondary monitor," but live USB enumeration on real Windows hardware (see the USB enumeration check under docs/research/) shows this is **not** DisplayPort Alt Mode: the dock enumerates as a single USB composite device (VID_1DE1&PID_F201, VIA Labs) exposing two vendor-defined HID interfaces, a USB audio interface ("USB Display Audio"), and a mass-storage (driver CD) interface, with no USB Video Class node anywhere. The screen is actually driven by a Windows Indirect Display Driver (IddCx) virtual adapter from **Actions Microelectronics** ("extension Graphics Adapter" on a "ROOT\EXTENSION_VIRTUAL_BUS"), which software-composites the desktop/window image on the host and ships frames to the dock over the vendor HID channel, almost certainly as a hardware-encoded H.264/HEVC stream chunked into HID packets rather than a raw framebuffer. This is a materially lower-fidelity path than real DP Alt Mode: it directly explains the observed low frame rate and visible lag even at 720p, worsening at 1080p, since the pipe is HID-class USB bandwidth plus host-side software composition and encoding rather than a native GPU-driven video sink. It is **not** a standalone smart display either way: no OS, no Wi-Fi, no built-in apps - everything shown on it comes from the connected Windows or Mac computer, and a driver ("USB Display") must be installed first. [Verified: CABLETIME official product/user-guide pages, GlobeNewswire/Yahoo press release, New Atlas hands-on, plus direct USB enumeration against real hardware]

## Model / SKU
- **Model number:** CB09W
- **SKU:** CT-DKS9-PW
- Confirmed directly from the official CABLETIME product page spec table and repeated in the official user-guide URL/title (`cabletime-screendock-ct-dks9-pw-user-guide`). This is the correct "8-in-1 ScreenDock" variant - CABLETIME sells several unrelated docking stations (e.g. the 15-in-1 "CB14G" / CT-DK142-AG with no screen, 20-in-1 Thunderbolt dock, 10-in-1 hub, etc.); do not confuse specs across these.

## Box contents (as listed by CABLETIME)
- ScreenDock unit (foldable screen + dock body)
- Gen2 USB-C cable, 1 m (HOST cable, supports both data and DisplayPort Alt Mode video - required; a charge-only cable will not work)
- (No AC/USB-C PD charger included - sold separately; the user must supply their own PD charger to use the pass-through charging port)

Note: the official spec table lists only "Gen2 USB-C Cable 1m" under Accessory. No printed manual URL/PDF was found; CABLETIME instead publishes an online "User Guide" article (see sources.md) and downloadable driver packages - treat that as the manual.

## Price and where to buy
- **Current price (cabletimetech.com official store):** $139.00 (list $159.00, i.e. "sale" price shown)
- **Kickstarter campaign pricing (before general retail):**
 - Super Early Bird (first 100 units): $109
 - Kickstarter Special: $119
 - Double pack: $189
 - Triple pack: $259
 - Projected standard MSRP after campaign: $169
- The campaign funded with 820 backers and $128,160 (4,272% of its $3,000 goal).
- **Not found on Amazon** (searched amazon.com and amazon.ae for "ScreenDock" / model CB09W / SKU CT-DKS9-PW - no listing located). The only CABLETIME docking station on Amazon found in search results is an unrelated 15-in-1 model (ASIN B0CKL87Z7N, model CB14G) - **do not confuse this with the ScreenDock**.
- Confirmed purchase channel: the brand's own Shopify store at cabletimetech.com, and Kickstarter/PledgeBox.

## Where this research was found
- Official: cabletimetech.com product page (specs table, SKU, pricing) and cabletimetech.com/blogs/knowledge user guide (setup, driver downloads, troubleshooting)
- Press/launch coverage: GlobeNewswire press release, Yahoo Finance syndication, Forbes, New Atlas (hands-on), TechPowerUp news post
- Crowdfunding record: Kickstarter campaign page and PledgeBox tracker mirror (funding stats, reward tiers)
- Video coverage: YouTube unboxing/review ("CABLETIME ScreenDock: The Most Useful Desk Upgrade You Didn't Know You Needed") with port-by-port walkthrough

See `sources.md` for the full link list and what each was used for.
