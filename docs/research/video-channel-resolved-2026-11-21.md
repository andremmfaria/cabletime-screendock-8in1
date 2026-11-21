docs/research/video-channel-resolved-2026-11-21.md

# RESOLVED: the CABLETIME ScreenDock video channel

## Headline finding

The built-in screen's video is H.264, delivered as HID reports on **MI_01** (`VID_1DE1&PID_F201&MI_01&COL01`), **Report ID 2**, confirmed independently via static analysis (Ghidra decompilation of the real Windows driver binaries) and a live USB capture of the exact same bytes. The two lines of evidence agree exactly, closing out the question that drove two full research sessions of live USB captures.

## Why this took so long: the root confusion

USBPcap's "device" field is the shared USB address of the whole composite device (`VID_1DE1&PID_F201`), not a per-interface identifier. MI_00 and MI_01 both appear under the same device address (54, in the cold-start capture that resolved this) — the only thing distinguishing their traffic is the Report ID byte inside the HID payload (1 for MI_00, 2 for MI_01). Every earlier capture this session scanned and bucketed traffic by device address alone, which made MI_01's entire data stream invisible: it was sitting right there, mixed into the same address as the already-identified, already-dismissed-as-"heartbeat" MI_00 traffic, distinguished only by a single header byte nobody had checked for yet.

## Code-level evidence (Ghidra, via delegated static-analysis pass)

Analyzed the real installed Windows driver binaries, pulled live from azul (`C:\Program Files (x86)\USB Display\`), not the old installer-wrapper analysis from an earlier session (which only ever covered a generic Ghost Installer Studio shell with no real driver logic inside it):

- `ImgEnc.dll` (6.4MB) is a colorspace/JPEG utility library (BGR/RGB/YCbCr conversion, resize/rotate, a small JPEG codec) — not the video encoder, contrary to this session's initial assumption based on its name alone.
- The real H.264 encoder lives in `USB Display.exe` itself: AMD AMF / Intel QSV hardware encoder paths via `H264EncoderFactory::RequireEncoder`, with a software fallback, SPS/PPS extraction confirming H.264 (matches the Mac side's `avc1`/VideoToolbox finding from an earlier session almost exactly, as the structural argument in `windows-mac-driver-correlation.md` predicted it should).
- `USBDisplayService.exe` statically links `hidapi` (open-source) and its only `hid_write` call site sends the JSON-RPC heartbeat to `MI_00&COL01` — confirmed, matches this session's live-read finding exactly, and confirms the service never touches video itself.
- `USBUtil::FindDisplayHIDPath` in `USB Display.exe` opens `MI_00&COL01` (logged "HID Comm") and separately opens `MI_01&COL01` (logged **"HID Display device found"**) — distinct, deliberate labeling in the vendor's own code. `EZDisplayTransmitter::SendHID` → the actual packetization function writes H.264 SPS/PPS and per-frame slice data to the MI_01 handle.
- Framing, derived from the decompiled packetization function (same function used for both the heartbeat and video, parameterized by interface): 1-byte Report ID (`interface_number + 1`) + three 4-byte big-endian fields (total message length, and two further metadata fields, all three populated only on a message's first chunk, zero on continuation chunks) + payload, inside a fixed 4096-byte HID report, chunked across multiple writes for messages larger than one report.

Full writeup: `docs/research/windows-real-driver-ghidra-analysis-2026-11-21.md` (written by the delegated analysis pass; reusable Ghidra scripts at `re/pyghidra_win_svc.py`, `re/pyghidra_win_svc2.py`, `re/pyghidra_win_gui.py`, `re/pyghidra_win_gui2.py`).

## Wire-level confirmation (this session's cold-start capture)

Re-scanned the 833-second cold-start capture (`re/captures/usbcap-azul-coldstart-2026-11-21.pcap`, 594MB, 2.03M records — captured from a genuine physical disconnect/reconnect, with USBDisplayService and USB Display.exe both freshly re-establishing their connection to the device) specifically for Report-ID-2 records, which earlier passes this session had not isolated:

- 28,579 Report-ID-2, 4096-byte records found on device address 54 (the same address already identified as carrying MI_00's heartbeat traffic via Report ID 1), spanning the full 813-second window from the moment video actually started (t=20.37s) to the end of the capture, at ~35.2 records/sec.
- 23,959 of these are "first chunk" records (nonzero total-length field); message sizes range from 67 bytes to 255,983 bytes, averaging ~1278 bytes — consistent with small inter-frame deltas and occasional large keyframes for a real-time, low-latency, continuously-adapting H.264 stream of a mostly-static desktop.
- The actual video data: byte 0 is `0x02` (Report ID 2, confirming MI_01 exactly as the Ghidra analysis predicted), bytes 1-4 are the big-endian total-length field (e.g. `0x00000df9` = 3577 for the first sampled frame), the header occupies exactly 16 bytes total (`4096 - 4080 = 16`, matching the Ghidra-derived "4080 bytes of payload" figure exactly), and byte 16 onward is a literal H.264 Annex-B bytestream: `00 00 00 01 67 42 c0 20 da 01 40 16 e8 40 ...` — a real SPS NAL unit (type 7), Baseline profile (`0x42`), with subsequent frames showing NAL type 1 (non-IDR slice) at the same offset. Exactly matches the code-derived framing, byte for byte.

## What this retires

- **Device 24** (the ~531 pkt/sec, 64-byte, high-entropy interrupt channel flagged earlier this session as the leading-but-unconfirmed video-channel hypothesis) is **not** the video channel. It remains unidentified — worth a quick follow-up check (it may be the digitizer/touch collection's heartbeat-like polling, or something on MI_02/MI_04), but it's no longer the open question blocking this investigation.
- **MI_00's JSON-RPC heartbeat** finding from earlier this session stands confirmed and unchanged — it really is just a heartbeat/status channel, now additionally confirmed by the vendor's own code (`hid_write` call site, `"HID Comm"` log string).
- **DisplayPort Alt Mode** is now doubly ruled out: not just by the earlier IddCx-virtual-display-adapter finding, but directly by this session's decompiled code showing the GUI app's own `SendHID` function writing H.264 to a HID report. The vendor's marketing claim that the screen is "recognized via DisplayPort Alt Mode" almost certainly refers to the USB-C port/cable needing the full SuperSpeed + sideband pinout (a real, separate requirement — a charge-only cable genuinely won't carry the USB data this mechanism needs), not to the actual pixel transport protocol.
- The WinUSB/raw-URB route attempted earlier this session is no longer necessary — we now have the full wire format from legitimate code analysis plus a confirming live capture, which is a stronger result than raw-URB access would have given on its own.

## Open items if this gets revisited

- Decode and reassemble a full multi-chunk message end to end (continuation chunks, all-zero header fields, concatenate payload-after-report-ID bytes) and feed it through a real H.264 decoder to visually confirm it reconstructs an actual frame matching what was on azul's screen at that timestamp — the conclusive, visual proof-of-concept this finding deserves, not yet done.
- Identify what device 24 actually is, now that it's confirmed not to be video — likely a quick, low-priority cleanup task.
- The two remaining metadata fields in the 16-byte header (beyond total length) are not yet decoded — likely a sequence number and/or a frame-type/flags field; could be worked out by correlating with the continuation-chunk boundaries and NAL type patterns across a longer run.
