# Implementation Plan: Linux Userspace Driver for CABLETIME ScreenDock (CB09W)

Created: 2026-11-21

Revised: 2026-11-21 — incorporates three user decisions (hardware access, dual X11/Wayland support with a headless dev host, public distribution intent) that were carried as Open Questions in the prior draft.

Status: Draft

## Objective

Build a Linux userspace daemon that drives the CABLETIME ScreenDock's built-in 5.5" panel the same way `USBDisplayService.exe` does on Windows: capture a configured desktop output, encode it to H.264 matching the confirmed wire format, chunk and write it to the MI_01 `/dev/hidrawN` node, and concurrently maintain the MI_00 JSON-RPC heartbeat so the dock doesn't sit in its "Connecting Signal..." overlay. This is not a kernel driver — `hid-generic` already exposes both interfaces as hidraw nodes — it is a single compiled native program, preferably Rust, that owns capture, encode, framing, and the two HID channels.

This plan is built entirely on the confirmed findings in `docs/research/full-protocol-spec-2026-11-21.md` and `docs/research/linux-driver-feasibility-2026-11-21.md`. Nothing here re-litigates those findings; where something is still genuinely unresolved per those docs, it's carried forward as a named risk, not quietly assumed away. This project is now also intended for eventual public distribution, which changes how packaging-related sections are scoped below (see Distribution Intent and Open Questions).

## Acceptance Criteria

- [ ] A native Rust (or justified C++ fallback) binary runs on a Linux host, opens the dock's MI_00 and MI_01 hidraw nodes by resolving the correct interface rather than a hardcoded node number, and maintains a heartbeat on MI_00 at roughly 1/second indefinitely without the dock showing "Connecting Signal...".
- [ ] The same binary captures a live desktop frame source, encodes it to H.264 Constrained Baseline matching the confirmed keyframe-bundling behavior (SPS+PPS+SEI+IDR as one message, inter-frames single-NAL), frames it per the confirmed 16-byte-header/4096-byte-chunk wire format, and writes it to MI_01 via direct `write(2)` (not any ioctl/"set report" call).
- [ ] On the real physical unit, this produces a stable, continuously-updating, correctly-decoded picture sustained for at least 60 seconds — not a single blink, not a partial-corner render — at the panel's native 1280×720.
- [ ] The resolution-picker formula (`fieldA = 0`, `fieldB = target_width_in_pixels * 256`) is implemented and demonstrated working for at least one non-native resolution/orientation combination on the real device.
- [ ] Both capture backends (X11 XShm and Wayland xdg-desktop-portal+PipeWire) are implemented behind a common trait, each validated with synthetic headless display content during development, and at least one has additionally been validated against a real, interactive (non-synthetic) desktop session before this plan is considered complete end-to-end.
- [ ] A documented, repeatable permissions setup (udev rule + group) lets a non-root user run the daemon without `sudo`, written as a genuinely installable artifact rather than a personal one-off copy-paste step.
- [ ] Project structure, build instructions, and a short runbook exist in the repo so a second person could build and run this without re-deriving anything from the research docs.
- [ ] A LICENSE file exists at the repo root, reflecting a choice the user has made — not a default picked by this plan (see Open Questions).

## Out of Scope

- Touch/digitizer input (the Col02 collections on MI_00/MI_01) — not required for display-only use, not attempted in this plan.
- Replicating the macOS driver's mouse-injection behavior (`AMHIDEventFakeMouseProcessor`) — this SKU is non-touch and that capability is explicitly flagged as dormant/unneeded in `docs/security-assessment.md`; this driver will not implement any input-injection path.
- Audio (MI_02) and mass storage (MI_04) — confirmed stock-class devices with zero vendor involvement; Linux's existing USB Audio and Mass Storage class drivers already handle them with no work needed here.
- Resolving the unexplained static-analysis contradiction around the field A/B getters' code-level mechanism — the empirical formula is sufficient and this plan builds on it as-is.
- Resolving the `"reset"` JSON-RPC method's direction/trigger, or the SD/TF card reader's exact hub-port identity — both are explicitly flagged as open in the research and are irrelevant to this driver's function.
- Building actual deb/rpm/AUR packages or a signed-release pipeline — distribution intent is now confirmed (see below), but this plan's scope is a genuinely installable udev rule + systemd unit + README/runbook + LICENSE, not a packaging-format build system; that remains a reasonable follow-on once the daemon itself works.
- Any EDID exchange or resolution-negotiation protocol — confirmed not to exist on the wire; the daemon just picks a resolution and signals it via field B.

## Hardware Access (resolved)

The user will physically move the dock's USB cable from azul (the Windows research host) to the Linux development machine. This removes the three-option logistics gap the prior draft of this plan flagged as its single biggest schedule blocker — there is no further decision needed here, and Milestones 2, 3, 6, and 7 (all of which require real-hardware validation) can proceed as soon as the cable is moved, without USB/IP tunneling or dual-boot workarounds.

One operational detail remains, and this plan states it explicitly so it isn't rediscovered mid-project: moving the cable means the dock is no longer attached to azul. Any further work that needs to compare this driver's behavior against the real Windows `USBDisplayService.exe` reference implementation — e.g. "does the real app also behave this way under condition X" — now requires either (a) temporarily swapping the cable back to azul to re-run a comparison, which interrupts whatever Linux-side testing is in progress, or (b) working purely from the findings already captured in `docs/research/full-protocol-spec-2026-11-21.md` and `docs/research/linux-driver-feasibility-2026-11-21.md`, which are treated as settled per this plan's own ground rules. Milestones below default to (b) wherever the research already answers the question; (a) is only needed if a genuinely new reference-behavior question comes up that the research docs don't already cover.

## Risks

- **Native write-timing risk not yet proven with a real native client** (point 5 in the task brief): the PowerShell/.NET prototype showed partial decode under slow chunk timing and improved decode under tight back-to-back writes, but no compiled native client has been tested against the real hardware yet. Mitigation: Milestone 2 below is specifically designed to retire this risk early, before any capture/encode pipeline is built, using a minimal native binary replaying real captured bytes.
- **Heartbeat-dependency is unconfirmed as an isolated two-variable test** (point 6): it's unknown whether video-only (no heartbeat) or heartbeat-only (no video) produces different on-screen behavior. Mitigation: Milestone 3 below runs both isolated conditions on real hardware before building the full concurrent daemon, so the daemon's threading model isn't built on an unverified assumption.
- **Hardware-access logistics (resolved, operational note only).** The dock will be physically moved to the Linux development machine, so hardware availability itself is no longer a blocking risk. The residual risk is narrower: once moved, any new Windows-reference-behavior comparison requires swapping the cable back (interrupting Linux testing) or relying on the already-captured research findings. Mitigation: default to the research docs for reference behavior; treat a cable swap-back as a deliberate, scheduled action, not an ambient option.
- **Headless development host has no live desktop to capture from.** The Linux development machine is currently headless — no attached display, no running X11 or Wayland session — so neither capture backend has anything real to pull frames from during day-to-day development. Mitigation: Milestone 4 below runs each backend against a virtual/headless display server (Xvfb for X11, a headless Weston/wlroots instance for Wayland) that produces synthetic-but-real desktop content, exercising the actual XShm and portal/PipeWire code paths end-to-end without needing a physical display. This is explicitly a two-stage validation: synthetic headless correctness first (dev-time, on this host), then a separate real-interactive-desktop validation step later, on whichever machine this is actually deployed to or used on — the two are not conflated, and Milestone 4's exit criteria name both stages separately.
- **Screen capture portability across X11/Wayland, now both in scope.** The two environments need genuinely different capture APIs (XShm vs. PipeWire/xdg-desktop-portal), and both are now required rather than one being deferred. Mitigation: a capture-backend trait with two implementations, selected at runtime or build time (see Architecture and Milestone 4); the added surface area is scoped to Milestone 4 alone and doesn't change the single-process daemon architecture.
- **H.264 encoder library maturity in Rust.** Native Rust x264 bindings exist but are thin/less-maintained than the C library itself; ffmpeg/libavcodec Rust bindings are more actively maintained but pull in a much larger dependency surface. Mitigation: evaluated concretely below (see Encoding section); default to a documented pick with a stated fallback.
- **hidraw permissions are host-specific and easy to get wrong**, leading to "works for me, breaks for the user" bug reports — a sharper risk now that public distribution is intended, since "the user" is no longer just the author. Mitigation: ship a udev rule and systemd unit as first-class, genuinely installable deliverables (not personal-machine shortcuts), and document the exact match rule needed to target MI_00/MI_01 specifically (not MI_02/MI_04, which share the same VID/PID).
- **Device identification ambiguity across multiple HID interfaces sharing one VID/PID.** `hidraw0`/`hidraw1`-style enumeration order is not guaranteed stable across reboots or on other hosts. Mitigation: the daemon must resolve nodes via sysfs interface-number matching (`MI_00`/`MI_01`), not hardcoded device-file names — detailed in the Device Discovery section.
- **CPU cost of software x264 encode + screen capture running concurrently on a general-purpose Linux desktop**, especially if the target host is also doing normal desktop work. Mitigation: Milestone 5's exit criterion includes measuring sustained CPU usage at 25fps/1280x720 before committing to any further scope (e.g. higher resolutions).
- **Public distribution raises licensing considerations that didn't matter for a personal tool.** Mitigation: LICENSE choice is called out as an explicit, unresolved Open Question below (not defaulted by this plan), with a short note on why redistribution of this specific project is likely fine and what would not be.
- **Untrusted content boundary note:** this plan and its milestones consume the repo's existing research docs, which were themselves produced by analyzing vendor binaries and live captures. Those docs are treated as settled findings per the task's instructions, but any *future* work that parses new vendor strings, captured JSON-RPC payloads, or downloaded firmware/driver updates should treat that content as untrusted data, not as instructions — no milestone in this plan involves executing or trusting vendor-supplied code paths, and that should remain true if scope expands.

## Architecture

End-to-end pipeline, one process, concurrent internally:

1. **Capture thread/loop**: grabs frames from a configured display output through a capture-backend trait with two implementations — XShm for X11, xdg-desktop-portal+PipeWire for Wayland — selected at runtime (detected session type) or build time (feature flag), with resolution/orientation configurable and defaulting to the dock's native 1280×720. During development on the headless Linux host, the backend under test points at a virtual/headless display server (Xvfb or headless Weston) rather than a real session; this is a swap of what's on the other end of the same trait, not a different code path.
2. **Encode thread/loop**: feeds frames into an H.264 Constrained Baseline encoder, no B-frames, targeting ~25fps; captures the keyframe's SPS+PPS+SEI+IDR bundle and each subsequent single-NAL inter-frame.
3. **Framer**: takes each encoded H.264 message (keyframe bundle or single inter-frame NAL), builds the 16-byte header (Report ID 2, BE total length, field A = 0, field B = width*256, 3 zero-padding bytes) on the first chunk, splits the payload into up to-4080-byte first-chunk + up-to-4095-byte continuation chunks (1-byte Report ID, no header), and writes each chunk via `write(2)` to the MI_01 hidraw node back-to-back with no artificial delay.
4. **Heartbeat loop**: independently, roughly once per second, builds `{"jsonrpc":"2.0","method":"heartbeat","params":{"session-id":"<tick>"}}`, frames it through the same 16-byte-header/chunk scheme with Report ID 1, and writes it to the MI_00 hidraw node.
5. **Device discovery module**: runs once at startup (and optionally on hotplug) to resolve the correct `/dev/hidrawN` paths for MI_00 and MI_01 specifically.

Why one process rather than two cooperating processes (e.g. a separate heartbeat daemon and a separate video daemon): the heartbeat-dependency question (risk above) is unresolved, and running both inside one process with two independent loops/threads keeps the failure mode simple to reason about and lets Milestone 3's isolated two-variable test map directly onto "comment out one loop" rather than needing IPC. The alternative (two processes) was considered and rejected for this reason — it adds process-lifecycle and IPC complexity to answer a question that's cheaper to answer with a single flag in one binary.

## Concrete Technology Choices

### Language: Rust (primary), per user preference

Rust is used for the daemon's core: device discovery, framing/chunking, hidraw I/O, heartbeat JSON-RPC construction, and process orchestration. These are all well-served by mature Rust crates with no forcing function toward C++.

### hidraw access: raw file I/O via `std::fs`/`nix`, not the `hidapi` crate

Two real options:
- **`hidapi` crate** (Rust bindings over the C `hidapi` library): cross-platform, matches what the vendor's own Windows service uses internally. But it's a C library binding, adds a build-time C dependency, and its abstraction doesn't buy anything here since this is Linux-only and the write mechanism needed is already known precisely (`write(2)` on the raw node — no report-descriptor-driven logic needed).
- **Plain `write(2)` via `std::fs::File`/`nix::unistd::write`** directly against `/dev/hidrawN`: simpler, zero extra C dependency, and — critically for the timing-sensitive multi-chunk risk — gives the most direct, lowest-overhead path from encoded bytes to the syscall, with no library-level buffering or abstraction between the daemon and the kernel.

**Decision: plain file I/O.** The property that rules out `hidapi` here specifically is the timing risk (risk #1 above): the research explicitly attributes the PowerShell/.NET prototype's partial-decode problem to interpreter/GC overhead layered on top of the raw write, and recommends a native implementation with no such layer. Pulling in `hidapi`'s own abstraction layer reintroduces a layer of indirection between the daemon and the syscall for no capability this project needs (no cross-platform requirement, no report-descriptor parsing needed since the framing is already fully known). Steelman for `hidapi` anyway: if the project ever needs read-side HID reports (e.g. for the dead "reset" method or future touch support) hidapi's enumeration/open-by-VID-PID helpers are convenient — but that's speculative future scope, not this plan's MVP.

### H.264 encoding: `ffmpeg`/`libavcodec` via an existing Rust binding, not a bare libx264 binding

Three real options, evaluated concretely rather than named-dropped:
- **Bare `libx264` via a Rust FFI crate** (e.g. `x264` crate bindings): smallest dependency surface, matches the exact encoder the vendor's own software uses (confirmed software `H264Encoder` = libx264 on azul). But the most actively maintained Rust `x264`-binding crates are thin, low-level wrappers with sparse documentation and inconsistent upkeep — a real risk for a project that needs precise control over GOP structure (keyframe bundling), profile (Constrained Baseline), and B-frame suppression.
- **`ffmpeg-next`/`ffmpeg_sys_next` crates (libavcodec via FFI)**: actively maintained, exposes libavcodec's x264 encoder wrapper (`libx264` via FFmpeg) with the full set of encoder options (`profile=baseline`, `bf=0`, GOP control) through FFmpeg's own well-documented option-string interface, which is also usable for screen capture input muxing if ever needed. Heavier dependency (pulls in FFmpeg's full shared-library surface at link/runtime), but that's a one-time packaging cost, not an ongoing maintenance risk, since FFmpeg's API stability and documentation quality are much higher than a thin direct libx264 binding.
- **A pure-Rust H.264 encoder** (no mature production-grade option exists at Constrained Baseline encode quality as of this writing) — not practical, ruled out outright.

**Decision: `ffmpeg-next` driving libavcodec's `libx264` wrapper, configured for Constrained Baseline / no B-frames / target ~25fps.** The property ruling out bare libx264 bindings is maintenance/control risk on exactly the knobs this project needs precisely (GOP/profile/B-frames) — FFmpeg's option-string interface is the more documented, more test-covered path to get those knobs right, even though it's a C library behind the Rust binding either way. Risk accepted and flagged explicitly: this is the one component where Rust's own ecosystem is "bindings over a C library" rather than a native Rust implementation, because (per the task's stated fallback condition) no Rust-native H.264 encoder exists at the needed quality/control level — this is not a blanket "use C++ instead" case, it's a scoped FFI dependency inside an otherwise-Rust daemon, which is a normal and common pattern, not a deviation from the Rust-first preference.

### Screen capture: both backends, behind a trait, resolved for a headless dev host

- **X11/Xorg**: `XShm` (X Shared Memory Extension) via the `x11` or `xcb`-family Rust crates — mature, simple, synchronous, well-trodden path for screen capture on Xorg sessions.
- **Wayland**: no direct XShm equivalent; the correct modern path is the `xdg-desktop-portal` `ScreenCast` portal backed by PipeWire, consumed via the `ashpd` crate (portal D-Bus calls) plus `pipewire-rs` (frame consumption) — meaningfully more moving parts (D-Bus portal negotiation, user consent dialog, PipeWire stream setup) than XShm.

**Decision: build both, behind a `CaptureBackend` trait, selected at runtime (detect session type via `$XDG_SESSION_TYPE`/presence of a Wayland socket) with a build-time feature flag as a fallback override.** This supersedes the prior draft's "pick one for the MVP" framing — the user wants both X11 and Wayland supported, not a single target. The trait exposes a `next_frame() -> Frame` (or equivalent) call; `x11_shm.rs` and `wayland_portal.rs` implement it independently, so the encode/framing/hidraw layers never know which backend is active.

**The genuine complication: the Linux development machine is currently headless** — no attached display, no running X11 or Wayland session at all — so there is nothing for either backend to capture from during development on this host. Three options were weighed, not picked blindly:
1. **Run virtual/headless display servers for dev-time testing** — Xvfb for the X11 backend, a headless Weston/wlroots instance for the Wayland backend — each producing synthetic-but-real framebuffer content (e.g. a test pattern, a terminal, a moving clock) that the real XShm/portal+PipeWire code paths can actually pull frames from.
2. **Mock the capture layer entirely** (feed the encoder a programmatically-generated test image, bypassing XShm/PipeWire calls) during dev, and only exercise the real capture APIs once a display is eventually attached.
3. **Defer all capture-backend work** until a machine with a real display is available, treating Milestone 4 as blocked rather than working around the headlessness.

**Decision: option 1 — Xvfb/headless Weston.** This is the one judgment call in this revision (the other two decisions are direct user instructions). Reasoning: option 2 (full mock) never actually exercises the real XShm or xdg-desktop-portal/PipeWire call paths — the exact code this plan is trying to de-risk — so a mock-only pass would give false confidence and just relocate the real validation to later, more expensive debugging. Option 3 (defer) stalls Milestone 4 indefinitely with no committed date for a display to show up, and this plan's whole milestone-ordering philosophy is to de-risk unknowns early rather than let them sit unexercised. Xvfb and headless Weston are the standard, well-supported way to give a headless Linux host something that looks like a real X11/Wayland session to any program that asks for one — including a real XShm grab or a real portal-mediated ScreenCast session — so they exercise the actual production code paths, just against synthetic content instead of a human-used desktop. Steelman against this choice: synthetic headless content can mask real-world rendering edge cases (e.g. GPU-composited effects, multi-monitor layouts, actual user window content) that only show up on a live interactive desktop, so passing Xvfb/Weston tests is necessary but not sufficient — which is exactly why Milestone 4's exit criteria below keep the two validation stages explicitly separate rather than treating headless success as "done."

### JSON-RPC construction: `serde_json`

Straightforward; no real alternative worth evaluating — it's the de facto standard Rust JSON crate, used here only to build small nested structs (heartbeat message) into bytes, no deserialization-heavy workload.

## Device Discovery and Permissions

The dock enumerates as composite `1de1:f201` with MI_00 (HID, Report ID 1) and MI_01 (HID, Report ID 2) alongside MI_02 (Audio) and MI_04 (Mass Storage) sharing the same VID/PID. `/dev/hidrawN` enumeration order is not guaranteed stable, so the daemon must not hardcode device file names.

Resolution approach: walk `/sys/class/hidraw/hidraw*/device/` symlinks back to each hidraw node's parent USB interface, read that interface's `bInterfaceNumber` (sysfs exposes this on the interface's own device node, e.g. `.../1-2:1.0/bInterfaceNumber`), and match interface number `0` → MI_00 and interface number `1` → MI_01, plus cross-checking `idVendor`/`idProduct` (`1de1`/`f201`) on the shared USB device ancestor to avoid matching some unrelated vendor-HID device on the system. This can be done with plain `std::fs` directory walking and string parsing — no extra crate needed beyond maybe `udev`-crate if the plain-sysfs-walk approach proves fragile in testing (fallback, not primary plan).

Permissions: `/dev/hidrawN` nodes default to root-only or a restrictive group (`root:root 0600` on many distros, though some ship broader `plugdev`/`input`-group access). Now that this project is meant for public distribution, the udev rule and systemd unit are written as genuinely installable/packageable artifacts rather than "copy this file to make it work for me":

```
# deploy/99-screendock.rules
# Install to /etc/udev/rules.d/99-screendock.rules (or ship via a distro package's udev.rules.d hook).
# Grants the 'screendock' group read/write access to this device's HID interfaces only
# (not MI_02/MI_04, which are stock Audio/Mass-Storage class devices and need no special rule).
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="1de1", ATTRS{idProduct}=="f201", MODE="0660", GROUP="screendock", TAG+="uaccess"
```

```
# deploy/screendockd.service
# Install to ~/.config/systemd/user/screendockd.service (per-user service — no root needed to run,
# matching the udev rule's group-based access model) or package this path via a distro's systemd
# user-unit install hook. Enable with: systemctl --user enable --now screendockd.service
[Unit]
Description=CABLETIME ScreenDock (CB09W) userspace display driver
After=graphical-session.target

[Service]
ExecStart=%h/.local/bin/screendockd
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
```

Both files are tracked in `deploy/` and referenced by name (not pasted ad hoc) from the runbook/README, with explicit install paths and the `screendock` group-creation step (`groupadd screendock && usermod -aG screendock "$USER"`, then `udevadm control --reload-rules && udevadm trigger`), so a packager or an end user following only the README can get a working install without reading this plan or the research docs. This remains the single most common real-world "driver doesn't work" support issue for this class of project, and matters more now that the audience is "the public," not just the author.

## Distribution Intent (resolved)

The user wants this eventually distributed/shared publicly, not just a personal tool. This changes the bar for the udev rule and systemd unit above (now written as installable artifacts, not personal shortcuts) and for the README/runbook (Milestone 8, revised below) — both need to work for someone who isn't the author and doesn't have this plan's context.

**Open item: LICENSE choice.** This plan does not pick one — that's the user's call — but names the realistic options so the decision is scoped rather than open-ended:
- **MIT**: maximally permissive, minimal friction for anyone who wants to use, fork, or embed this driver (including in closed-source tooling); fits a small utility project where maximizing adoption/reuse matters more than controlling downstream use. Doesn't require derivative works to stay open.
- **Apache-2.0**: similarly permissive, but adds an explicit patent grant/termination clause — arguably the more deliberate choice for a reverse-engineered hardware protocol project, since it addresses patent concerns head-on without imposing copyleft obligations.
- **GPL-3.0**: copyleft — any distributed derivative must also be released under GPL-3.0 with source available. Fits a "give back to whoever else reverse-engineers or improves on this" ethos and guards against a closed-source fork, at the cost of friction for anyone who'd want to embed this in proprietary tooling.

**Why redistribution is likely fine for this specific project, and what would not be:** this driver is a clean-room reimplementation of a reverse-engineered wire protocol — the Rust code, framing logic, and encoder configuration here are original work based on independent analysis of captured USB/HID traffic and JSON-RPC messages, not on vendor source code or disassembled/recompiled vendor binaries. That's the generally-understood basis on which independent reimplementations of undocumented hardware protocols get distributed. What would **not** be fine is shipping any of the vendor's actual binaries, drivers, or firmware images (`USBDisplayService.exe` and anything captured alongside it in `re/`) as part of this project's distribution — the repo's existing `.gitignore` already correctly excludes those from version control, and that exclusion should remain a hard rule as distribution gets closer, not just a VCS hygiene habit.

## Testing/Validation Strategy

With hardware access resolved (see above), every milestone from "does a native write work on real hardware" onward can proceed once the cable is moved, with one residual note: any comparison against the real Windows reference app requires either swapping the cable back to azul or relying on the already-captured research findings (see Hardware Access above) — this plan defaults to the latter wherever the research already answers the question.

Capture-backend testing (Milestone 4) has its own, separate two-stage validation reality because the Linux development host is headless:
- **Stage 1 — synthetic headless validation (dev-time, this host):** both backends run against Xvfb (X11) and a headless Weston/wlroots instance (Wayland), proving the real capture code paths produce correct frames from synthetic-but-real framebuffer content.
- **Stage 2 — real-desktop validation (separate, later, possibly different hardware):** at least one backend is additionally validated against an actual interactive X11 or Wayland session on whatever machine this eventually gets deployed to or used on. This is explicitly not required to happen on the same timeline or the same host as Stage 1, and Milestone 4's exit criteria name both stages so Stage 1 passing alone is never mistaken for "capture is done."

## Project Structure

Proposed layout, added alongside the existing `docs/` and `re/` directories:

```
cabletime-screendock-8in1/
├── docs/                        # existing — unchanged
├── re/                          # existing — unchanged (reverse-engineering artifacts)
├── LICENSE                      # new — pending user's choice (see Open Questions)
├── README.md                    # new — public-facing install/build/run instructions
├── src/screendockd/             # new Rust crate — the daemon
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs              # CLI parsing, startup, thread orchestration
│   │   ├── discovery.rs         # hidraw node resolution (MI_00 / MI_01 by sysfs interface number)
│   │   ├── framing.rs           # 16-byte header build, chunking, both Report ID 1 and 2 cases
│   │   ├── heartbeat.rs         # JSON-RPC heartbeat message construction + loop
│   │   ├── capture/
│   │   │   ├── mod.rs           # CaptureBackend trait + runtime/build-time backend selection
│   │   │   ├── x11_shm.rs       # XShm implementation (tested against Xvfb in dev)
│   │   │   └── wayland_portal.rs # xdg-desktop-portal + PipeWire implementation (tested against headless Weston in dev)
│   │   ├── encode.rs            # ffmpeg-next wrapper: init, encode loop, keyframe bundling
│   │   └── hidraw_io.rs         # write(2) wrapper around the resolved hidraw nodes
│   └── tests/                   # unit tests for framing/chunking math (no hardware needed)
├── deploy/
│   ├── 99-screendock.rules      # udev rule — installable artifact, see Device Discovery and Permissions
│   └── screendockd.service      # systemd user-service unit — installable artifact, see above
└── docs/implementation-plan.md  # this file
```

This keeps the daemon's source isolated under `src/`, parallel to the existing `docs/`/`re/` top-level split, and keeps the udev rule and service unit in a `deploy/` directory rather than mixed into `src/`. A top-level `LICENSE` and public-facing `README.md` are added given the confirmed distribution intent. A Cargo workspace (vs. a single crate) was considered and rejected for the MVP: there's exactly one binary target and no clear second crate (e.g. a shared protocol library consumed by more than one binary) yet — a workspace can be introduced later if, e.g., a separate CLI diagnostic tool is wanted, without this being a breaking restructure now.

## Milestones

Ordered to front-load the riskiest unknowns (write timing, heartbeat dependency) before any capture/encode pipeline work, per the task's explicit instruction not to build everything and discover a blocker late.

1. **Device discovery + framing unit tests (no hardware required).** Implement `discovery.rs` (sysfs interface-number resolution for MI_00/MI_01) and `framing.rs` (header construction + chunk splitting), covered by unit tests using the known real captured byte layouts from the research docs (e.g. the 3577-byte keyframe bundle, the 17-chunk 67,666-byte message) as fixtures. Owner: implementer. Dependencies: none.
   - Exit criterion: unit tests reproduce the exact byte layout documented in `full-protocol-spec-2026-11-21.md` for both a single-chunk and a multi-chunk message, byte-for-byte, with no hardware involved.

2. **Native write-timing validation (requires hardware, now on-site after the cable move).** A minimal native Rust binary that opens the real MI_01 hidraw node and replays the exact real captured keyframe bundle + the real 17-chunk follow-up message from the research, back-to-back via `write(2)` with no artificial delay, and measures actual per-chunk timing. Owner: implementer. Dependencies: Milestone 1 (framing logic), dock's USB cable moved to the Linux development machine.
   - Exit criterion: measured per-chunk write latency is at or below the ~0.77ms/chunk figure the PowerShell prototype achieved (ideally closer to the pcap's native ~0.76–0.91ms), and the dock visibly renders the full strip/region this data represents — directly testing whether a compiled binary genuinely closes the gap the research flagged as unproven, rather than assuming it does.

3. **Heartbeat-dependency isolation test (requires hardware).** Run three short real-hardware sessions: (a) heartbeat loop only, no video, (b) video stream only (replaying Milestone 2's approach extended to a short real GOP), no heartbeat, (c) both together. Owner: implementer. Dependencies: Milestone 2 passed (video write path trusted), heartbeat framing implemented (reuses Milestone 1's framing code with Report ID 1).
   - Exit criterion: documented, observed on-screen behavior for all three conditions (not just (c)) — this directly retires the "never isolated as a clean two-variable test" gap named in the research, and tells the daemon's threading model whether the heartbeat loop is load-bearing for video rendering or purely cosmetic (overlay-suppression only).

4. **Capture backend abstraction: both X11 and Wayland, validated in two stages.** Implement the `CaptureBackend` trait plus both `x11_shm.rs` (XShm) and `wayland_portal.rs` (xdg-desktop-portal + PipeWire) implementations, with runtime session-type detection (build-time feature flag as override). Owner: implementer. Dependencies: none (independent of hardware milestones; can run in parallel with 2/3).
   - **Stage 1 exit criterion (headless, dev-time, this host):** each backend, run against its respective virtual display (Xvfb for XShm, headless Weston/wlroots for the portal+PipeWire path), produces captured frames that are verifiably correct (e.g. dumped to PNG and matching the synthetic content shown on the virtual display) at the configured resolution. This can be fully developed and verified without the dock attached and without any real display attached.
   - **Stage 2 exit criterion (real desktop, separate, later, possibly different hardware):** at least one backend is additionally run against a real, interactive X11 or Wayland session — on whichever machine this eventually gets deployed to or used on — and produces captured frames that visibly match what's actually on that screen. This stage is explicitly not required to complete on the same timeline as Stage 1, and is not satisfied by Stage 1 passing alone.

5. **Encoder integration (requires hardware for the final check, not for development).** Wire `ffmpeg-next` to encode captured frames to H.264 Constrained Baseline, no B-frames, ~25fps, replicate the keyframe-bundling behavior (SPS+PPS+SEI+IDR as one message). Owner: implementer. Dependencies: Milestone 4 Stage 1 (frame source — Stage 2 is not a blocker here), Milestone 1 (framing).
   - Exit criterion: encoded output, when decoded by `ffmpeg` locally (no hardware needed yet), produces a correct, continuously-updating decode of the (synthetic or real) captured desktop for at least 60 seconds, and measured sustained CPU usage for capture+encode together is recorded and reported to the user before proceeding.

6. **Full pipeline integration on real hardware.** Combine Milestones 2 (proven write timing), 3 (proven/documented heartbeat behavior), and 5 (proven encode) into the single daemon described in the Architecture section, running continuously. Owner: implementer. Dependencies: Milestones 2, 3, 5.
   - Exit criterion: the acceptance criteria's core bullet — 60+ seconds of stable, correctly-decoded, continuously-updating picture on the real dock at native resolution, heartbeat running throughout.

7. **Resolution/orientation picker.** Implement the `fieldB = width * 256` computation for a configurable target resolution/orientation, re-encoding at that resolution. Owner: implementer. Dependencies: Milestone 6.
   - Exit criterion: at least one non-native resolution/orientation combination (e.g. the 900-wide or portrait case from the research's live test) renders correctly on the real device.

8. **Public-distribution packaging, permissions, and repo write-up.** Ship the udev rule and systemd unit as installable artifacts (not personal-machine shortcuts), write a public-facing README/runbook covering install, build, group setup, and running the daemon, and resolve the LICENSE open item (user decision — see Open Questions) by adding the chosen `LICENSE` file at the repo root. Owner: implementer (packaging), user (LICENSE decision). Dependencies: Milestone 6 (so the runbook describes a working daemon, not a hypothetical one); Milestone 4 Stage 2 result should be noted in the README if available at this point, even if it completes later.
   - Exit criterion: a second person — a member of the public, not the author, with no access to this plan or the research docs — can follow only the README/runbook to build, install, and run the daemon and see it work on the real device, and the repo has a LICENSE file reflecting the user's actual choice.

## Open Questions

- **LICENSE choice**: MIT, Apache-2.0, or GPL-3.0 (see Distribution Intent above for the tradeoffs) — needs an explicit answer from the user; this plan deliberately does not default to one.
- **Real-desktop validation target for Milestone 4 Stage 2**: will this be the same Linux development machine once a display is eventually attached to it, or a different machine entirely (e.g. wherever the user actually intends to run this daily)? Affects when Stage 2 can realistically be scheduled; not a blocker for Stages 1 or for Milestones 5–8, but worth the user confirming so it doesn't silently slip indefinitely.
- **Target resolution/orientation defaults beyond native 1280×720**: does the user want the daemon to default to native panel resolution always, or support a configured non-native target as a first-class default (e.g. matching the 1080p mode mentioned in `docs/overview.md` as usable but laggy)? Affects Milestone 7's scope and default config. Need an answer from the user.
- **Whether the dead `"reset"` JSON-RPC method or touch/digitizer collections are ever wanted in a future phase** — explicitly out of scope for this plan's MVP, but worth the user confirming there's no near-term need, since it would change the `hidapi`-vs-raw-I/O decision above (hidapi's enumeration conveniences matter more if read-side HID traffic is ever needed).

## Reasoning Transparency Notes (per Planner's own operating rules)

- The single-process/two-loop architecture decision was the most load-bearing structural call in this plan; the steelman against it (two processes, cleaner separation) is noted inline in the Architecture section rather than hidden.
- The ffmpeg-vs-bare-libx264 encoder decision is the one place this plan's "Rust first" framing gets most tested — both options are FFI-over-C either way, so the real decision is about binding maturity/control-surface, not language purity, and that's stated directly rather than papered over as if ffmpeg were a "native Rust" answer.
- The capture-backend scope changed from "pick one" to "build both" by direct user instruction, not a planning judgment call — the actual judgment call this revision introduces is how to test both backends on a headless dev host. That decision (Xvfb + headless Weston, option 1 of three considered) is laid out with its steelman in the Screen Capture section above: the two rejected alternatives were a full capture mock (doesn't exercise the real XShm/portal code paths, defeats the point) and deferring capture work until a display is physically available (stalls a milestone indefinitely against this plan's own de-risk-early philosophy). The steelman against the chosen option stands on its own: synthetic headless content cannot substitute for real-desktop validation, which is exactly why Milestone 4 keeps the two stages explicitly separate rather than letting headless success be mistaken for "done."
- The hardware-access and distribution-intent changes in this revision are direct user decisions incorporated as given, not judgment calls this plan is making on the user's behalf — the one place this revision exercises genuine planning judgment is the headless-dev-host capture-testing resolution above.
- No rationale above was generated post-hoc to justify a conclusion already assumed; each alternative considered is tied to a specific property of this project (the timing risk, the GOP/profile control need, the headless-host complication, the licensing tradeoffs) rather than generic best-practice language.
