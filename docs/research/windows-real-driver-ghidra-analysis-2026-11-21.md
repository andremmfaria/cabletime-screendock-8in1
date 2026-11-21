# Windows real driver Ghidra analysis (2026-11-21)

This pass analyzed the real installed Windows driver binaries pulled from a live install (not the Ghost Installer wrapper examined previously): ImgEnc.dll, USBDisplayService.exe, and USB Display.exe, all 32-bit MSVC PE binaries with no debug symbols but with recoverable RTTI/log-string class and method names. Analysis was done with pyghidra against a local Ghidra 11.4.1 install (GHIDRA_INSTALL_DIR=/opt/ghidra), decompiling targeted functions reached by cross-referencing hidapi exports and literal log strings rather than attempting a full-binary decompile. This resolves the single biggest open question from the prior correlation doc (docs/research/windows-mac-driver-correlation.md section 3, "H3"): which HID interface and report ID actually carries video, and what the wire framing looks like. Confidence is high because the finding comes directly from decompiled code paths that construct the HID write buffer byte by byte, not from string-grep inference.

## 1. ImgEnc.dll: what it actually is

ImgEnc.dll's only imports are KERNEL32.dll and libiomp5md.dll (Intel OpenMP); it does not import avcodec, avformat, Media Foundation, or any HID/USB API. Its 61 exports are entirely colorspace/pixel utility functions: BGR32ToRGB888, RGB32ToYCbCr420Planar, YCbCr420PlanerToRGB32_BT601/BT709, YCbCr422InterleavedTo*, resize/rotate/mirror/fast-copy helpers, plus a small JPEG codec (JpegEncInit/EncodeRGB888/EncodeYCbCr420Planar, JpegDecInit/Decode), and one proprietary-named pair, EZEncodeEncInit/EZEncodeEncEncode/EZEncodeEncRelease/EZEncodeEncSetParam, whose implementation was not decompiled this pass (lower priority once the HID framing was found, see section 4 for why). Conclusion (confirmed from exports/imports, not decompiled internals): ImgEnc.dll is a pixel-format/colorspace conversion and JPEG utility library, not the H.264 encoder. The "Image Encoder" name is misleading for this investigation's purposes.

## 2. The real H.264 encoder lives in USB Display.exe, not ImgEnc.dll

USB Display.exe (the GUI/tray app, 7.3MB) contains real encoder classes recovered from RTTI/log strings: AMFEncoder (AMD AMF), QSVEncoder (Intel QuickSync, with QSVEncoder::IsSupport, "QSV support with D3D11"/"QSV support with D3D9"), H264Encoder (generic/software fallback), and H264EncoderFactory::RequireEncoder which picks between them with logged fallback behavior ("HW encoder used. Use SW H264 encoder", "Encoder switch to SW encoder", "Device not accept H264 encoding"). This strongly confirms hypothesis H1 from the prior correlation doc (H.264, hardware-accelerated with software fallback) and now with code-level evidence rather than architectural-mirror-image guessing: the Windows driver's encode path is AMD AMF / Intel QSV hardware H.264, falling back to a software H264Encoder, not Windows Media Foundation's IMFTransform and not ImgEnc.dll. avutil-55/swresample-2/swscale-4 (imported by USB Display.exe) are used for pixel format conversion/scaling around this encoder pipeline, consistent with ImgEnc.dll's role in section 1; avformat-57 is present on disk but not directly imported by any of the three analyzed binaries, so its exact use (if any, vs. unused bundled dependency) is unconfirmed.

Further confirmation that the codec is H.264: EZDisplayTransmitter::SetVideoBuffer (decompiled, FUN_0052923f) logs "Got SPS/PPS" right after pulling encoder extradata via a vtable call (`(**(code**)(*vtable+0x28))(...)`), and copies it with `FUN_006fb780`. SPS/PPS are H.264 NAL unit types; their presence as a named, logged step in the transmit pipeline is strong corroborating evidence, independent of the encoder class names.

## 3. The decisive finding: EZDisplayTransmitter, two HID collections, two roles, one shared framing

### 3a. USBUtil::FindDisplayHIDPath (decompiled, FUN_004beab0 in USB Display.exe)

This function enumerates HID devices for VID_1DE1/PID_F201 and opens up to three named device-interface paths, logging a distinct message for each:

- `VID_1DE1&PID_F201&MI_00&COL01` -> opened, logged "HID Comm write device found"
- the same `MI_00&COL01` path checked again -> logged "HID Comm read device found" (the GUI replicates the same single collection in both a write and a read role; this matches USBDisplayService.exe's own heartbeat-then-listen sequence on MI_00, see 3b)
- `VID_1DE1&PID_F201&MI_01&COL01` -> opened, logged **"HID Display device found"**

The naming is the tell: MI_00 is logged as "HID Comm" (matches the live-capture-confirmed JSON-RPC heartbeat channel), MI_01 is logged as "HID Display" — a distinct, deliberately different label used nowhere else in this function. This is code-level confirmation, not inference, that **MI_01 is the video/display HID interface**, resolving the open ambiguity from the live captures (MI_01 had produced no input reports and all writes to it had failed when probed manually/blind; the real driver evidently requires the full handshake-then-SendHID sequence below before the device will accept data on it).

### 3b. USBDisplayService.exe's only HID-write call site: the MI_00 heartbeat (not video)

USBDisplayService.exe statically links the open-source hidapi library (confirmed by its own export table: hid_init, hid_open, hid_open_path, hid_write, hid_read, hid_read_timeout, hid_close, hid_enumerate, hid_get_device_info, hid_winapi_descriptor_reconstruct_pp_data/hid_winapi_get_container_id — the latter two are recent hidapi additions, so this is a reasonably current hidapi version vendored into the binary). Decompiling the one function that calls hid_write (FUN_0040407c) shows it: enumerates VID_1DE1/PID_F201, string-matches the device path against `VID_1DE1&PID_F201&MI_00&COL01` specifically, opens only that path, builds the exact JSON-RPC heartbeat string already seen in the live capture (`{"jsonrpc":"2.0", "method": "heartbeat", "params": {"session-id": "<tick-count>"}}`), writes it once (with retry), then loops hid_read_timeout on the same handle waiting for responses. There is no second hid_write call site in this binary. **USBDisplayService.exe never touches video; its only job is the MI_00 heartbeat/control exchange.** This matches the live-capture finding in the background exactly and former hypothesis that MI_00=control is settled with certainty.

### 3c. The shared wire framing (confirmed identical in both the heartbeat path and the video path)

Both FUN_0040407c (service, heartbeat) and FUN_00525cca (GUI, video — the real low-level write function called by `EZDisplayTransmitter::SendHID`, FUN_0052b624) build an **identical 16-byte header followed by payload, inside a fixed 4096-byte (0x1000) HID report**, written via `hid_write(handle, &header, 0x1000)`:

```
offset 0      : 1 byte  - Report ID  = *(hid_device_info + 0x24) + 1
                          (hid_device_info+0x24 is the interface_number field in
                          hidapi's struct hid_device_info; MI_00 -> interface 0 ->
                          Report ID 1 [matches the live-captured heartbeat Report ID 1];
                          MI_01 -> interface 1 -> Report ID 2 [inferred from the same
                          code pattern - this is the one value not independently
                          live-verified yet, see confidence note below])
offset 1-4    : 4 bytes - total payload length, big-endian uint32
                          (heartbeat: JSON string length; video: total frame/NAL size,
                          written only on the FIRST chunk of a multi-chunk transfer and
                          left as zero on continuation chunks)
offset 5-8    : 4 bytes - big-endian uint32, first-chunk-only metadata field A
                          (zero in the heartbeat path, which has no equivalent args;
                          in the video path this is "param_4" passed into SendHID -
                          not fully traced this pass, plausibly a PTS/timestamp half
                          or frame-type flag; see open item below)
offset 9-12   : 4 bytes - big-endian uint32, first-chunk-only metadata field B
                          (same caveat as field A; video path's "param_5")
offset 13-15  : 3 bytes - zero padding (uStack_1008's upper bytes in the decompile)
offset 16...  : payload - up to 4080 (0xff0) bytes of the JSON string or H.264 bitstream
                          chunk, memcpy'd in; buffer memset to 0 before each chunk so
                          any unused tail of the 4096-byte report is zero-padded
```

Both functions chunk larger payloads across multiple 4096-byte writes (`if (0xff0 < remaining) remaining = 0xff0;`), loop until the remaining-byte counter reaches zero, and treat a negative `hid_write` return as a hard failure (with retry/backoff logic). The length/metadata header fields are populated only once, on the very first chunk of a given logical message (`if (local_1064 == 0) { ...fill header fields... }`), and are all-zero on continuation chunks — i.e. the firmware is expected to use the Report-ID-tagged first chunk's length field to know how many subsequent same-Report-ID writes make up one complete message/frame, not a per-chunk length.

EZDisplayTransmitter::SendHID (FUN_0052b624) is called from two places in USB Display.exe: once with a payload that is the encoder's SPS/PPS extradata (`param_4=0, param_5=0`) immediately after "Got SPS/PPS" is logged, and once per captured frame from EZDisplayTransmitter::send_thread (FUN_00529a68) with `param_4`/`param_5` populated from values obtained via `FUN_00529791` (not traced this pass — likely a timestamp/PTS pair or frame-sequence metadata, given the surrounding frame-rate/throughput logging in EZDisplayTransmitter::LogFrameRate). A sibling function, EZDisplayTransmitter::SendTCP (FUN_0052b3f2), exists with the same chunking/retry structure but sends over a boost::asio TCP socket instead of HID — this is almost certainly the code path used for this vendor's wireless-casting product variants (EZCast/EZMira), not relevant to the USB-wired ScreenDock, and was not pursued further.

## 4. Confidence levels, explicitly

- **Confirmed from decompiled code, high confidence:** MI_00 (`VID_1DE1&PID_F201&MI_00&COL01`) is the JSON-RPC heartbeat/control channel, used by USBDisplayService.exe and redundantly re-opened by the GUI; this exactly reproduces the live-capture-observed Report ID 1 heartbeat traffic.
- **Confirmed from decompiled code, high confidence:** MI_01 (`VID_1DE1&PID_F201&MI_01&COL01`) is the distinctly-logged "HID Display" interface opened by USB Display.exe, and is the target of EZDisplayTransmitter::SendHID, which is fed H.264 SPS/PPS and per-frame encoded data. This resolves the open ambiguity between MI_00/MI_01 from the live-capture session with code-level evidence rather than inference.
- **Confirmed from decompiled code, high confidence:** the wire framing is a fixed 4096-byte HID report = 1-byte Report ID + three big-endian uint32 fields (total-length + two metadata fields, populated only on a message's first chunk) + payload, chunked at up to 4080 payload bytes per report, identical in structure between the control and video paths.
- **High confidence, not yet independently live-verified:** Report ID on MI_01 is 2 (derived as interface_number+1 from the same code pattern that gives Report ID 1 on MI_00, which *was* live-verified). Re-capturing traffic specifically filtered to MI_01 and checking the first payload byte against 2 would fully close this out.
- **Medium confidence, not traced this pass:** the exact semantics of the two first-chunk-only metadata fields (SendHID's param_4/param_5) - plausible candidates are a PTS/timestamp pair or a frame-type flag plus sequence number, based on their being zero for the SPS/PPS send and populated per-frame elsewhere, but the producing function (FUN_00529791) was not decompiled.
- **Confirmed from imports/exports, high confidence:** the H.264 encode itself happens in USB Display.exe via AMD AMF / Intel QuickSync hardware encoders with a software H264Encoder fallback (H264EncoderFactory::RequireEncoder), not in ImgEnc.dll and not via Windows Media Foundation. ImgEnc.dll is a colorspace-conversion/JPEG utility library (BGR/RGB/YCbCr conversions, resize/rotate, small JPEG codec, plus an unexamined proprietary EZEncodeEnc* function group that was deprioritized once the real encoder was found elsewhere).
- **Unconfirmed, informational only:** avformat-57.dll's role, since none of the three analyzed binaries import it directly; it may be used by a DLL not yet analyzed or be an unused bundled dependency.

## 5. What this changes versus the prior correlation doc

docs/research/windows-mac-driver-correlation.md's "H3" (packet framing is the unresolved blocker) is now resolved on the Windows side with concrete, decompiled evidence: Report ID 1 = control/heartbeat on MI_00, Report ID 2 (high confidence, not yet re-verified live) = video on MI_01, with a shared 16-byte-header-then-chunked-payload framing inside 4096-byte HID reports. The structural argument in that doc's section 2 (the firmware doesn't care which OS talks to it, so both drivers must produce compatible wire format) means this framing is very likely also what the Mac driver's AMHIDStreamingPacketizer produces - worth a quick cross-check against the already-decompiled Mac Ghidra projects (re/mac/ghidra_project_mac*/) for a matching 16-byte-header pattern, but that wasn't done in this pass since the Windows side alone already answers the task's core question.

## 6. Suggested next step for full closure

Re-run a live USB capture filtered specifically to the MI_01 interface while USB Display.exe is actively casting, and check: (a) the first byte of outgoing interrupt/output reports equals 2, (b) a 16-byte header matching the layout in section 3c precedes what looks like H.264 NAL data (look for 0x00 0x00 0x00 0x01 or 0x00 0x00 0x01 start codes shortly after byte 16 of the first chunk of a frame).
