docs/research/usb-capture-session-2-2026-11-21.md

# USB capture session 2 (2026-11-21): architecture re-think after device recovery

## What happened this session

The dock got stuck showing "Connecting Signal..." after cumulative disconnect/reconnect/service-kill actions from the prior session. Root cause found and fixed: an earlier `pnputil /disable-device` call (queued, not applied live) had silently taken effect on a later re-enumeration, leaving the composite device's four child interfaces (MI_00, MI_01, MI_02 audio, MI_04 mass storage) in `Disconnected` state while the parent shell stayed "Connected." `pnputil /enable-device` on the parent instance ID fixed it immediately (USB Display.exe's memory jumped from 10 MB idle to 140 MB active within seconds), restoring live, screen-matching video with no reboot needed.

## The post-recovery puzzle

Three fresh captures (87s, 118s, and a 345s flash/correlation test) were taken from `\\.\USBPcap1` (confirmed the only physical USB host controller on azul via `DEVPKEY_Device_LocationPaths`/`Get-PnpDevice`) while video was confirmed live and screen-matching throughout. None showed the previous session's signature: 4096-byte interrupt-OUT reports with plaintext Annex-B H.264 structure (SPS/slice NAL headers). That finding, from the very first capture of this whole investigation (`windows-usb-capture-analysis-2026-11-21.md`), has not reappeared since.

Device-by-device audit of the post-recovery captures:
- Device 30: confirmed genuine non-silent PCM-shaped audio (16/24-bit sample structure visible in raw bytes) — the video's soundtrack, not video.
- Device 16: initially looked correlated with a live black/white flash test, but direct byte inspection showed a textbook LED brightness-ramp pattern (progressive fade toward `0xff` across consecutive 40ms-spaced packets) — almost certainly the CORSAIR RGB mouse identified via STRING descriptors in the prior session's capture-2 analysis. The apparent correlation with the flash test was coincidental (RGB effects run independent cycles).
- Device 6 (and session-reused low addresses 15/22/26/29 etc.): various small HID/status devices, previously identified by STRING descriptor as other peripherals (HyperX mic, Dell sensor, Commander PRO, etc.) or the dock's own periodic vendor control-polling request (`c00508e903010400` → `8400ffff`), confirmed again at the same ~0.5s cadence, still read as status polling not a handshake.
- Device 24: 64-byte interrupt-OUT reports at a strikingly regular, nearly-constant rate (~531 packets/sec, ~34 KB/s total, flat across both 87s and 118s captures and the 345s flash test regardless of on-screen content). Byte content is high-entropy with no recognizable plaintext NAL structure when reassembled across consecutive packets (NAL-type-byte distribution came out statistically uniform across all 32 possible 5-bit values, including codes that don't exist in real H.264 — a strong signal of either encryption/obfuscation or coincidental noise, not plaintext H.264).

## Why device 24 is the leading video-channel candidate (revised conclusion)

Initially dismissed device 24 as unrelated (shape-matched against a previous session's STRING-descriptor identification of a CORSAIR Lighting Node CORE at a different address) — but addresses are reassigned fresh on every enumeration, so that cross-session shape-matching is not reliable identity evidence on its own.

Two pieces of external research changed the read on this:
1. Actions Microelectronics' own site (confirmed signer of this dock's Windows driver via Authenticode, see `windows-mac-driver-correlation.md`) markets exactly two proprietary USB/WiFi/Ethernet "display transmission" compression modes: "Compact Display" (high compression, lossy, 50-100ms latency) and **"Sync Display" (low compression, visually lossless, sub-1ms latency, positioned for "Ethernet display and Graphic or Video")**. A sub-1ms-latency real-time channel is far more consistent with a fixed-schedule, constant-rate delivery mechanism than a variable "send only what changed" scheme — which is exactly device 24's observed behavior (dead-constant 531 pkt/sec regardless of content).
2. The direct architectural precedent, DisplayLink (a comparable proprietary USB-to-display chip vendor), historically encrypted/obfuscated its USB video protocol specifically to look like noise to observers, and that encryption was broken via a simple replay attack (dumping and resending the same USB data reproduced the same image on the display) — see Echtler & Hodges, "Reverse-Engineering DisplayLink devices" (2009).

Under an encryption/obfuscation hypothesis, "looks like uniform random bytes" stops being evidence against device 24 being the video channel — it is what encrypted/obfuscated real-time video is expected to look like. The earlier dismissal (treating the uniform NAL-byte distribution as proof of "not video") assumed plaintext structure, which isn't a safe assumption for this vendor's chip, and is actively undermined by both of the findings above.

## Recommended next step: a DisplayLink-style replay test

Capture a clean, isolated sequence of device-24 packets while the screen shows a known, static, highly distinctive image (not a video, so the sequence is short and deterministic). Then, later, resend that exact same byte sequence to the device (e.g. via a Linux host with a custom libusb script, or on Windows via a raw HID write) and observe whether the dock's screen reproduces that same (now stale) image. A positive result conclusively identifies device 24 as the video channel and demonstrates the "encryption" (if any) is not a strong per-session cipher — exactly how DisplayLink's protocol was broken. A negative result (no visible change) would argue against device 24 and reopen the device-address search.

This test does not require defeating any cryptography to be informative either way, and is a much stronger signal than further byte-pattern inspection of a single capture.

## Open items

- Device 24's true current STRING-descriptor identity was not re-confirmed in this session's captures (no enumeration/GET_DESCRIPTOR event occurred during steady-state operation) — only inferred from traffic shape and external vendor research.
- The replay test above has not yet been attempted.
- If the replay test succeeds, the actual packet/header framing used in the CABLETIME-session (post pnputil-fix) traffic should be re-examined for the 64-byte chunking scheme (4-byte sequence header + payload), which differs from the original 4096-byte single-report framing seen in the very first capture — these may be two different operating modes (e.g. "Compact"/"Sync Display" selectable by the driver) rather than the same protocol.
