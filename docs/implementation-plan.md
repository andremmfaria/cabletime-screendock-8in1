# Implementation Plan: Linux Userspace Driver for CABLETIME ScreenDock (CB09W)

Created: 2026-11-21
Status: Draft

## Objective

Build a Linux userspace daemon that drives the CABLETIME ScreenDock's built-in 5.5" panel the same way `USBDisplayService.exe` does on Windows: capture a configured desktop output, encode it to H.264 matching the confirmed wire format, chunk and write it to the MI_01 `/dev/hidrawN` node, and concurrently maintain the MI_00 JSON-RPC heartbeat so the dock doesn't sit in its "Connecting Signal..." overlay. This is not a kernel driver — `hid-generic` already exposes both interfaces as hidraw nodes — it is a single compiled native program, preferably Rust, that owns capture, encode, framing, and the two HID channels.

This plan is built entirely on the confirmed findings in `docs/research/full-protocol-spec-2026-11-21.md` and `docs/research/linux-driver-feasibility-2026-11-21.md`. Nothing here re-litigates those findings; where something is still genuinely unresolved per those docs, it's carried forward as a named risk, not quietly assumed away.

## Acceptance Criteria

- [ ] A native Rust (or justified C++ fallback) binary runs on a Linux host, opens the dock's MI_00 and MI_01 hidraw nodes by resolving the correct interface rather than a hardcoded node number, and maintains a heartbeat on MI_00 at roughly 1/second indefinitely without the dock showing "Connecting Signal...".
- [ ] The same binary captures a live desktop frame source, encodes it to H.264 Constrained Baseline matching the confirmed keyframe-bundling behavior (SPS+PPS+SEI+IDR as one message, inter-frames single-NAL), frames it per the confirmed 16-byte-header/4096-byte-chunk wire format, and writes it to MI_01 via direct `write(2)` (not any ioctl/"set report" call).
- [ ] On the real physical unit, this produces a stable, continuously-updating, correctly-decoded picture sustained for at least 60 seconds — not a single blink, not a partial-corner render — at the panel's native 1280×720.
- [ ] The resolution-picker formula (`fieldA = 0`, `fieldB = target_width_in_pixels * 256`) is implemented and demonstrated working for at least one non-native resolution/orientation combination on the real device.
- [ ] A documented, repeatable permissions setup (udev rule + group) lets a non-root user run the daemon without `sudo`.
- [ ] Project structure, build instructions, and a short runbook exist in the repo so a second person could build and run this without re-deriving anything from the research docs.

## Out of Scope

- Touch/digitizer input (the Col02 collections on MI_00/MI_01) — not required for display-only use, not attempted in this plan.
- Replicating the macOS driver's mouse-injection behavior (`AMHIDEventFakeMouseProcessor`) — this SKU is non-touch and that capability is explicitly flagged as dormant/unneeded in `docs/security-assessment.md`; this driver will not implement any input-injection path.
- Audio (MI_02) and mass storage (MI_04) — confirmed stock-class devices with zero vendor involvement; Linux's existing USB Audio and Mass Storage class drivers already handle them with no work needed here.
- Resolving the unexplained static-analysis contradiction around the field A/B getters' code-level mechanism — the empirical formula is sufficient and this plan builds on it as-is.
- Resolving the `"reset"` JSON-RPC method's direction/trigger, or the SD/TF card reader's exact hub-port identity — both are explicitly flagged as open in the research and are irrelevant to this driver's function.
- Packaging/distribution (deb/rpm/AUR, systemd unit beyond a basic example) unless the user confirms this will be distributed — see Open Questions.
- Any EDID exchange or resolution-negotiation protocol — confirmed not to exist on the wire; the daemon just picks a resolution and signals it via field B.

## Risks

- **Native write-timing risk not yet proven with a real native client** (point 5 in the task brief): the PowerShell/.NET prototype showed partial decode under slow chunk timing and improved decode under tight back-to-back writes, but no compiled native client has been tested against the real hardware yet. Mitigation: Milestone 2 below is specifically designed to retire this risk early, before any capture/encode pipeline is built, using a minimal native binary replaying real captured bytes.
- **Heartbeat-dependency is unconfirmed as an isolated two-variable test** (point 6): it's unknown whether video-only (no heartbeat) or heartbeat-only (no video) produces different on-screen behavior. Mitigation: Milestone 3 below runs both isolated conditions on real hardware before building the full concurrent daemon, so the daemon's threading model isn't built on an unverified assumption.
- **Exactly one physical unit exists, and it currently lives on a remote Windows host (azul, 192.168.10.166), not on any Linux machine.** This blocks every milestone that requires real-hardware validation (3 through 7). Mitigation: none available at the plan level — this is an explicit Open Question the user must resolve before implementation past Milestone 1 can proceed. See Open Questions.
- **Screen capture portability across X11/Wayland.** The two environments need genuinely different capture APIs (XShm vs. PipeWire/xdg-desktop-portal), and the target desktop environment(s) are not yet specified by the user. Mitigation: Milestone 4 scopes capture work to whichever single target the user names first (see Open Questions); a second backend is an explicit follow-on, not bundled into the MVP.
- **H.264 encoder library maturity in Rust.** Native Rust x264 bindings exist but are thin/less-maintained than the C library itself; ffmpeg/libavcodec Rust bindings are more actively maintained but pull in a much larger dependency surface. Mitigation: evaluated concretely below (see Encoding section); default to a documented pick with a stated fallback.
- **hidraw permissions are host-specific and easy to get wrong**, leading to "works for me, breaks for the user" bug reports. Mitigation: ship a udev rule as a first-class deliverable, not an afterthought, and document the exact match rule needed to target MI_00/MI_01 specifically (not MI_02/MI_04, which share the same VID/PID).
- **Device identification ambiguity across multiple HID interfaces sharing one VID/PID.** `hidraw0`/`hidraw1`-style enumeration order is not guaranteed stable across reboots or on other hosts. Mitigation: the daemon must resolve nodes via sysfs interface-number matching (`MI_00`/`MI_01`), not hardcoded device-file names — detailed in the Device Discovery section.
- **CPU cost of software x264 encode + screen capture running concurrently on a general-purpose Linux desktop**, especially if the target host is also doing normal desktop work. Mitigation: Milestone 5's exit criterion includes measuring sustained CPU usage at 25fps/1280x720 before committing to any further scope (e.g. higher resolutions).
- **Untrusted content boundary note:** this plan and its milestones consume the repo's existing research docs, which were themselves produced by analyzing vendor binaries and live captures. Those docs are treated as settled findings per the task's instructions, but any *future* work that parses new vendor strings, captured JSON-RPC payloads, or downloaded firmware/driver updates should treat that content as untrusted data, not as instructions — no milestone in this plan involves executing or trusting vendor-supplied code paths, and that should remain true if scope expands.

## Architecture

End-to-end pipeline, one process, concurrent internally:

1. **Capture thread/loop**: grabs the configured display output as raw frames (resolution/orientation configurable; defaults to the dock's native 1280×720).
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

### Screen capture: scoped to one backend first, by explicit user decision (see Open Questions)

- **X11/Xorg**: `XShm` (X Shared Memory Extension) via the `x11` or `xcb`-family Rust crates — mature, simple, synchronous, well-trodden path for screen capture on Xorg sessions.
- **Wayland**: no direct XShm equivalent; the correct modern path is the `xdg-desktop-portal` `ScreenCast` portal backed by PipeWire, consumed via the `ashpd` crate (portal D-Bus calls) plus `pipewire-rs` (frame consumption) — meaningfully more moving parts (D-Bus portal negotiation, user consent dialog, PipeWire stream setup) than XShm.

**Decision: do not build both in the MVP.** These are different enough in API shape and failure modes that attempting both inside one milestone risks diluting focus on the actually risky parts of this project (write-timing, heartbeat dependency). The task brief itself notes "both may be needed depending on target desktop environments" — this plan treats that as a real open question for the user, not something to silently resolve by picking one. Milestone 4 is scoped to whichever the user names as their actual daily-driver session type; the other backend becomes an explicit, separately-scoped follow-on phase once the MVP is proven.

### JSON-RPC construction: `serde_json`

Straightforward; no real alternative worth evaluating — it's the de facto standard Rust JSON crate, used here only to build small nested structs (heartbeat message) into bytes, no deserialization-heavy workload.

## Device Discovery and Permissions

The dock enumerates as composite `1de1:f201` with MI_00 (HID, Report ID 1) and MI_01 (HID, Report ID 2) alongside MI_02 (Audio) and MI_04 (Mass Storage) sharing the same VID/PID. `/dev/hidrawN` enumeration order is not guaranteed stable, so the daemon must not hardcode device file names.

Resolution approach: walk `/sys/class/hidraw/hidraw*/device/` symlinks back to each hidraw node's parent USB interface, read that interface's `bInterfaceNumber` (sysfs exposes this on the interface's own device node, e.g. `.../1-2:1.0/bInterfaceNumber`), and match interface number `0` → MI_00 and interface number `1` → MI_01, plus cross-checking `idVendor`/`idProduct` (`1de1`/`f201`) on the shared USB device ancestor to avoid matching some unrelated vendor-HID device on the system. This can be done with plain `std::fs` directory walking and string parsing — no extra crate needed beyond maybe `udev`-crate if the plain-sysfs-walk approach proves fragile in testing (fallback, not primary plan).

Permissions: `/dev/hidrawN` nodes default to root-only or a restrictive group (`root:root 0600` on many distros, though some ship broader `plugdev`/`input`-group access). A udev rule is a first-class deliverable:

```
# /etc/udev/rules.d/99-screendock.rules
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="1de1", ATTRS{idProduct}=="f201", MODE="0660", GROUP="screendock"
```

paired with instructions to create a `screendock` group, add the running user to it, and reload udev rules (`udevadm control --reload-rules && udevadm trigger`). This must be documented in the repo's runbook, not just mentioned here, since it's the single most common real-world "driver doesn't work" support issue for this class of project.

## Testing/Validation Strategy — Logistics Gap, Flagged Explicitly

**This is the most important open item in the whole plan.** The only physical unit of this hardware currently lives attached to a remote Windows research host (azul, 192.168.10.166), not on any Linux machine. Every milestone from "does a native write work on real hardware" onward requires physical or remote access to the actual device — software running against mocked/fabricated byte streams can validate framing logic but cannot validate the thing this plan's biggest risks are actually about (real-hardware write timing, real-hardware heartbeat-dependency behavior).

Three realistic options, none assumed here as the answer:
1. **Physically move the dock to a Linux host.** Simplest, zero new infrastructure, but requires physical access/logistics (is azul co-located with a Linux machine? can the dock be unplugged from azul without disrupting other work there?).
2. **USB/IP (`usbip`) from azul to a Linux host.** Keeps the dock physically attached to azul, tunnels the USB device over the network to a Linux host that then sees it as a local hidraw device. Real tradeoff: USB/IP adds network-stack latency and jitter into exactly the one thing this project has already identified as timing-sensitive (sub-millisecond inter-chunk gaps) — this could reintroduce the same partial-decode problem the native-client fix is supposed to solve, for reasons unrelated to the daemon's own code. This needs to be tested empirically, not assumed safe, before relying on it for the timing-sensitive milestones.
3. **Install a Linux environment directly on azul** (dual-boot, or a live-boot USB) so the dock is tested on bare-metal Linux without moving it or networking it. Avoids both the physical-move logistics and the USB/IP latency risk, at the cost of disrupting azul's current Windows setup for the duration of testing.

**This plan does not pick one of these.** It is an explicit Open Question the user must resolve before Milestone 3 (the first milestone requiring real-hardware validation) can start. The milestone breakdown below is ordered so that Milestones 1 (and as much of 2 as possible without hardware) can proceed immediately regardless of this decision, buying time to resolve the logistics in parallel.

## Project Structure

Proposed layout, added alongside the existing `docs/` and `re/` directories:

```
cabletime-screendock-8in1/
├── docs/                        # existing — unchanged
├── re/                          # existing — unchanged (reverse-engineering artifacts)
├── src/screendockd/             # new Rust crate — the daemon
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs              # CLI parsing, startup, thread orchestration
│   │   ├── discovery.rs         # hidraw node resolution (MI_00 / MI_01 by sysfs interface number)
│   │   ├── framing.rs           # 16-byte header build, chunking, both Report ID 1 and 2 cases
│   │   ├── heartbeat.rs         # JSON-RPC heartbeat message construction + loop
│   │   ├── capture/
│   │   │   ├── mod.rs           # capture backend trait
│   │   │   └── x11_shm.rs       # (or wayland_portal.rs, per Milestone 4 decision)
│   │   ├── encode.rs            # ffmpeg-next wrapper: init, encode loop, keyframe bundling
│   │   └── hidraw_io.rs         # write(2) wrapper around the resolved hidraw nodes
│   └── tests/                   # unit tests for framing/chunking math (no hardware needed)
├── deploy/
│   ├── 99-screendock.rules      # udev rule
│   └── screendockd.service      # example systemd user-service unit (optional, see Open Questions)
└── docs/implementation-plan.md  # this file
```

This keeps the daemon's source isolated under `src/`, parallel to the existing `docs/`/`re/` top-level split, and keeps the udev rule and any service unit in a `deploy/` directory rather than mixed into `src/`. A Cargo workspace (vs. a single crate) was considered and rejected for the MVP: there's exactly one binary target and no clear second crate (e.g. a shared protocol library consumed by more than one binary) yet — a workspace can be introduced later if, e.g., a separate CLI diagnostic tool is wanted, without this being a breaking restructure now.

## Milestones

Ordered to front-load the riskiest unknowns (write timing, heartbeat dependency) before any capture/encode pipeline work, per the task's explicit instruction not to build everything and discover a blocker late.

1. **Device discovery + framing unit tests (no hardware required).** Implement `discovery.rs` (sysfs interface-number resolution for MI_00/MI_01) and `framing.rs` (header construction + chunk splitting), covered by unit tests using the known real captured byte layouts from the research docs (e.g. the 3577-byte keyframe bundle, the 17-chunk 67,666-byte message) as fixtures. Owner: implementer. Dependencies: none.
   - Exit criterion: unit tests reproduce the exact byte layout documented in `full-protocol-spec-2026-11-21.md` for both a single-chunk and a multi-chunk message, byte-for-byte, with no hardware involved.

2. **Native write-timing validation (requires hardware — the logistics gap above).** A minimal native Rust binary that opens the real MI_01 hidraw node and replays the exact real captured keyframe bundle + the real 17-chunk follow-up message from the research, back-to-back via `write(2)` with no artificial delay, and measures actual per-chunk timing. Owner: implementer. Dependencies: Milestone 1 (framing logic), hardware-access logistics resolved.
   - Exit criterion: measured per-chunk write latency is at or below the ~0.77ms/chunk figure the PowerShell prototype achieved (ideally closer to the pcap's native ~0.76–0.91ms), and the dock visibly renders the full strip/region this data represents — directly testing whether a compiled binary genuinely closes the gap the research flagged as unproven, rather than assuming it does.

3. **Heartbeat-dependency isolation test (requires hardware).** Run three short real-hardware sessions: (a) heartbeat loop only, no video, (b) video stream only (replaying Milestone 2's approach extended to a short real GOP), no heartbeat, (c) both together. Owner: implementer. Dependencies: Milestone 2 passed (video write path trusted), heartbeat framing implemented (reuses Milestone 1's framing code with Report ID 1).
   - Exit criterion: documented, observed on-screen behavior for all three conditions (not just (c)) — this directly retires the "never isolated as a clean two-variable test" gap named in the research, and tells the daemon's threading model whether the heartbeat loop is load-bearing for video rendering or purely cosmetic (overlay-suppression only).

4. **Capture backend for the user's confirmed target desktop environment (requires Open Question answer, not hardware).** Implement either `x11_shm.rs` or the Wayland portal backend, whichever the user names (see Open Questions), producing raw frames at the configured resolution from the live desktop. Owner: implementer. Dependencies: user's target-DE decision.
   - Exit criterion: captured frames are verifiably correct (e.g. dumped to a PNG and visually matching the real desktop) at the target resolution, independent of any encoder/hardware work — this can be developed and tested without the dock attached at all.

5. **Encoder integration (requires hardware for the final check, not for development).** Wire `ffmpeg-next` to encode captured frames to H.264 Constrained Baseline, no B-frames, ~25fps, replicate the keyframe-bundling behavior (SPS+PPS+SEI+IDR as one message). Owner: implementer. Dependencies: Milestone 4 (frame source), Milestone 1 (framing).
   - Exit criterion: encoded output, when decoded by `ffmpeg` locally (no hardware needed yet), produces a correct, continuously-updating decode of the live desktop for at least 60 seconds, and measured sustained CPU usage for capture+encode together is recorded and reported to the user before proceeding.

6. **Full pipeline integration on real hardware.** Combine Milestones 2 (proven write timing), 3 (proven/documented heartbeat behavior), and 5 (proven encode) into the single daemon described in the Architecture section, running continuously. Owner: implementer. Dependencies: Milestones 2, 3, 5.
   - Exit criterion: the acceptance criteria's core bullet — 60+ seconds of stable, correctly-decoded, continuously-updating picture on the real dock at native resolution, heartbeat running throughout.

7. **Resolution/orientation picker.** Implement the `fieldB = width * 256` computation for a configurable target resolution/orientation, re-encoding at that resolution. Owner: implementer. Dependencies: Milestone 6.
   - Exit criterion: at least one non-native resolution/orientation combination (e.g. the 900-wide or portrait case from the research's live test) renders correctly on the real device.

8. **Permissions/runbook finalization and repo write-up.** Ship the udev rule, document group setup, write a short "build and run" runbook referencing this plan. Owner: implementer. Dependencies: Milestone 6 (so the runbook describes a working daemon, not a hypothetical one).
   - Exit criterion: a second person, following only the runbook (not this research history), can build and run the daemon and see it work on the real device.

## Open Questions

- **Hardware access logistics for Milestones 2, 3, 6, 7** (physically move the dock to a Linux host vs. USB/IP tunnel from azul vs. dual-boot/live-boot azul itself): need an answer from the user before any real-hardware milestone can start. This is the single biggest schedule blocker in this plan.
- **Target desktop environment(s) to support for Milestone 4** (X11/Xorg vs. Wayland, and if Wayland, which compositor — portal behavior can vary): need an answer from the user; the plan defaults to building one first based on whichever matches the user's actual daily-use Linux machine, not both.
- **Deployment model**: is this a personal tool the user runs manually/via a user-session systemd unit on their own machine, or is broader distribution/packaging (deb/rpm/AUR, signed releases) ever wanted? This changes how much the `deploy/` directory and documentation need to generalize beyond one person's setup. Need an answer from the user; this plan's `deploy/` scope (Milestone 8) assumes "personal tool" unless told otherwise.
- **Target resolution/orientation defaults beyond native 1280×720**: does the user want the daemon to default to native panel resolution always, or support a configured non-native target as a first-class default (e.g. matching the 1080p mode mentioned in `docs/overview.md` as usable but laggy)? Affects Milestone 7's scope and default config. Need an answer from the user.
- **Whether the dead `"reset"` JSON-RPC method or touch/digitizer collections are ever wanted in a future phase** — explicitly out of scope for this plan's MVP, but worth the user confirming there's no near-term need, since it would change the `hidapi`-vs-raw-I/O decision above (hidapi's enumeration conveniences matter more if read-side HID traffic is ever needed).

## Reasoning Transparency Notes (per Planner's own operating rules)

- The single-process/two-loop architecture decision was the most load-bearing structural call in this plan; the steelman against it (two processes, cleaner separation) is noted inline in the Architecture section rather than hidden.
- The ffmpeg-vs-bare-libx264 encoder decision is the one place this plan's "Rust first" framing gets most tested — both options are FFI-over-C either way, so the real decision is about binding maturity/control-surface, not language purity, and that's stated directly rather than papered over as if ffmpeg were a "native Rust" answer.
- The capture-backend scoping decision (one backend, not both, in the MVP) is presented as a genuine open question needing the user's input, not a silent default — this plan intentionally does not guess the user's desktop environment.
- No rationale above was generated post-hoc to justify a conclusion already assumed; each alternative considered is tied to a specific property of this project (the timing risk, the GOP/profile control need, the logistics gap) rather than generic best-practice language.
