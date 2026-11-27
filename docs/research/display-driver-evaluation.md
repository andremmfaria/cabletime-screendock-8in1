# Evaluation: a proper display driver for the ScreenDock

Scope: Linux first, other operating systems briefly. The evaluation is read-only on the repo and host, with web research.

Labels used below: **Verified** means I read it in the repo, ran a read-only command on this host, or read it on the cited web page at the time of writing. **Inference** means my own reasoning or general knowledge that I did not check against a source. **Estimate** means engineer-days with a stated confidence.

## 1. Verdict

Do not write a kernel module. Build an evdi adapter behind the existing ports in `screendockd`, validate it with a short spike on a real GPU session before committing, and keep an own module as a documented fallback with explicit trigger conditions. Estimated cost of the evdi path: about 45 engineer-days likely (25 low, 80 high), medium confidence. An own virtual DRM module costs about 2.5 times that to build and adds a recurring per-kernel-release tax.

Two premises in the question need correcting.

- The panel is not 8 inch. The repo docs say the built-in panel is 5.5 inch, 1280x720 marketed, 1080p accepted (docs/overview.md, docs/specs.md, verified). "8-in-1" counts the dock's functions. This does not change the analysis.
- "Proper driver" does not remove the userspace daemon. The dock needs H.264 over hidraw; the daemon keeps encoding and transport under every option except the unrealistic one. The Windows vendor driver is itself a user-mode IddCx driver with encoding in userspace (docs/overview.md, verified), so even the vendor's "driver" is not a kernel graphics driver. A real kernel DRM driver only changes how the desktop sees the output, not how pixels reach the panel.

## 2. Ground truth from the repo and this host

### Repo (verified, docs read: design, virtual-display, capture, hardware-validation, reviews-and-caveats, research/)

- Ports exist for this: `CaptureBackend` (with `retarget`), `VirtualDisplay`, `VirtualMonitor` (`rect`, `resize`, `pipewire_node`), `DisplayProvider`, `EdidStore`, selected once at startup (`adapters::capture::create_backend`, `core::display_policy::decide`).
- Extend mode at the time of writing: X11 tier 1 (real RandR output with custom mode, but only where a free output accepts modes, in practice the dummy driver), tier 2 (bare RandR monitor), Mutter `RecordVirtual`, sway headless output, portal virtual source. Display settings may not list a bare monitor (docs/virtual-display.md).
- The dock sends its EDID over the control channel (`common.data_passthrough`, about every 5 s on a fresh replug), and the daemon caches it in `dock-edid.bin`, with a 258 byte fixture in tests. A dock that holds an old session is silent and sends none, so the cache is needed (docs/hardware-validation.md).
- A size change is a dock cycle of about 5 s end to end (two 1.5 s reply timeouts, 0.8 s settle, about 1 s encoder rebuild); the panel's behaviour at other sizes is partly unmeasured (docs/hardware-validation.md, docs/design.md "Known limits").
- Transport timing: back-to-back `write(2)` of 4096 byte reports, about 0.76 to 0.91 ms per chunk on the wire (docs/hardware-validation.md).
- Linux driver feasibility study already concluded this is not a kernel driver project, because `hid-generic` binds both vendor HID interfaces and hidraw writes reach the device (see the Linux driver feasibility study in this directory).
- Capture is never lossless to the encoder: Mutter streams deliver frames only on repaint, and a desktop that is not repainted produces no frames (docs/virtual-display.md). Damage-driven delivery already exists in spirit.

### Host (verified, read-only commands)

| Item | Result |
|---|---|
| OS and kernel | Ubuntu 26.04.1 LTS, `7.0.0-38-generic`; headers for `-38` and `-44` in `/usr/src` |
| Toolchain | `gcc`, `make`, `rustc`, `cargo` present; `dkms` not installed (`which dkms` empty) |
| In-tree modules available | `vkms`, `vgem`, `gud`, `udl`, `udlfb` as `.ko.zst` under `/lib/modules/7.0.0-38-generic`; config `CONFIG_DRM_VKMS=m`, `CONFIG_DRM_GUD=m`, `CONFIG_DRM_UDL=m` |
| evdi | No module on disk, none loaded, no libevdi. `apt-cache policy` shows `evdi-dkms` and `libevdi1` candidate `1.14.15+dfsg-0ubuntu1` from `resolute/universe`, not installed |
| Secure Boot | `mokutil --sb-state`: disabled (so signing cannot be exercised here) |
| DRM devices | `/dev/dri/card0` (PCI 0000:00:02.0, with HDMI-A-1 and HDMI-A-2 connectors in sysfs) and `renderD128`. I did not touch them |
| configfs | `/sys/kernel/config` present |
| `drm_writeback.h` | present in `/usr/src/linux-headers-7.0.0-38/include/drm/` |

What I could not do: load any module (no sudo), install evdi, or run a compositor against a DRM connector. Everything about evdi runtime behaviour below comes from upstream docs and a third party project, not from running it.

## 3. What "proper driver" means on Linux, option by option

### Option A: evdi as the kernel side, `screendockd` as the evdi client

evdi is a GPL kernel module plus an LGPL client library `libevdi`. The module creates a DRM device with a connector; a userspace client supplies an EDID with `evdi_connect`, receives mode changes, DPMS and cursor events, registers buffers and pulls damaged pixels (`evdi_grab_pixels`, `evdi_request_update`, `evdi_handle_events`).

Facts (verified from the URLs in section 9):

- Licences: module GPL v2, library LGPL v2.1 or later, remaining files MIT. Minimum kernel 4.15. Upstream says it verified Ubuntu kernels up to 6.15 and that newer or other-distro kernels "may require extra development".
- `libevdi` header on `main` is version 1.15.1 and exposes: `evdi_add_device`, `evdi_open`, `evdi_connect2` (EDID plus pixel area and pixel rate limits), `evdi_disconnect`, `evdi_register_buffer`, `evdi_request_update`, `evdi_grab_pixels` with `evdi_rect` damage rectangles, `evdi_get_event_ready` (a selectable fd), event handlers for mode change, DPMS, update ready, CRTC state, cursor and DDC/CI.
- The upstream docs state the library interface "is still changing" and not yet stable.
- The module is a platform driver, not a bus device. Upstream docs list `initial_device_count` as a workaround for X servers that build a static GPU list, and a `softdep` on the primary GPU driver so the compositor does not take evdi as its primary GPU.
- Distribution state: Debian trixie ships `evdi-dkms` 1.14.8; Ubuntu resolute universe has 1.14.15 (apt-cache, verified on this host). Breakage on new kernels is recurring: Fedora 40 kernel 6.10 broke 1.14.4 (displaylink-rpm issue 272), Ubuntu noble linux-6.17 HWE broke 1.14.2 (Launchpad bug 2133812). A third party project reports that 1.14.15 added kernel 7.0 support and 1.15.0 covers 7.2, and that Ubuntu 24.04's 1.14.2 does not compile on kernel 7.0 (unverified second hand, from the Powerology README).
- DKMS signing under Secure Boot works through a MOK enrolled once with `mokutil --import` and confirmed at the console on next boot; DKMS signs each rebuilt module with that key (safeguard.sh and vcheng.org pages). The private key being root readable removes the root/kernel boundary, as Ubuntu itself documents.

Closest precedent (verified README, not independently reproduced): `N7le/powerology-actions-usb-display-linux-driver` drives the same Actions Microelectronics chip family (`1de1:f201`, AM8268N, firmware `am_8268N_ezcast-usb_usbdisplay`) as a Python userspace client on top of evdi. It reports each panel appearing in Display Settings and arrangeable, 1920x1080 at 50 to 57 fps with hardware encoding on an i7-1355U plus RTX 3050, no custom kernel module, tested on kernel 7.0 with Cinnamon on X11. It says Wayland is untested. Its troubleshooting list is a free checklist of the integration pitfalls:

- `evdi_check_device(N)` indexes `/dev/dri/cardN`, not evdi device N, so enumerate through sysfs.
- Setting a mode on an evdi output blocks in X until the client services the modeset event, which deadlocks if the client calls `xrandr` itself.
- A `SIGKILL`ed client leaves the connector reporting connected with no owner ("no free evdi device").
- Xorg needs `xrandr --setprovideroutputsource` to attach the evdi provider.
- libevdi waits about 5 s for the X server to take DRM master at startup.
- Adding monitors makes X rebuild its layout and blank the desktop for seconds.

Inference: on Wayland compositors (Mutter, KWin, wlroots) evdi connectors appear like any DRM connector because DisplayLink's own driver relies on this; I have not verified it for this dock.

### Option B: own small virtual DRM driver (evdi-like, or vkms derived) plus the same userspace client

Same architecture as A, but we maintain the module. Ingredients: atomic modeset with simple display pipe or full atomic helpers, a connector whose EDID is supplied from userspace, hotplug uevent, GEM shmem buffers, a userspace ABI (chardev or ioctl plus dma-buf or mmap) delivering damaged framebuffer regions and mode and DPMS events, GPL licensing, DKMS packaging.

Evidence on effort: evdi has 734 commits and 11 years of work behind a commercial product (GitHub page, verified), and still needs a patch for most new kernels. Reading the kernel docs, DRM helper libraries cover much of the pipeline ("simple display pipe helpers" for drivers that would have fit fbdev, kernel docs, verified), which is why a minimal module is plausible, but the userspace ABI, locking and teardown races are the expensive part (inference).

### Option C: in-tree virtual drivers as is (vkms, vgem, writeback) plus capture

`vkms` is a software only KMS driver "useful for testing and for running X on headless machines" and supports multiple instances through configfs (kernel docs, verified; the module exists on this host). Its own TODO list in the same page says EDID specification support is still wanted (verified wording: "Also needs support to specify the EDID"), so the dock EDID cannot be injected through vkms as documented. A `drm.edid_firmware` kernel parameter override may work on any connector (inference, not checked).

How pixels would reach us, and why this is weak:

- Writeback connectors let a client request the composed output into a buffer (`drm_writeback.h` exists locally; semantics are inference). Compositors do not use writeback connectors for normal operation (inference), and the daemon cannot be DRM master of a connector a compositor owns.
- Reading the scanout framebuffer of the compositor's vkms plane, as `kmsgrab` does, needs CAP_SYS_ADMIN or root, delivers whole frames with no damage information and still needs modes and EDID wired somewhere else (inference).
- `gud` is the in-kernel Generic USB Display host driver, mainline since 5.13 (verified via gud-gadget README). It is a protocol for devices that speak GUD; the dock does not, so gud is only a reference for how a USB display driver is structured, not something to reuse. `udl` is DisplayLink's old open kernel driver for DL-1xx/DL-3xx style devices; same conclusion.

### Option D: USB/HID kernel driver that encodes in kernel

Not realistic. Reasons specific to this problem:

- The dock needs H.264 Constrained Baseline. The kernel has no software H.264 encoder and should not carry one (large SIMD heavy codec code under the FPU-in-kernel restrictions, inference). The userspace encoders (libx264, VAAPI, NVENC, QSV) are userspace APIs.
- V4L2 memory to memory encoders exist only for certain SoCs (inference).
- A kernel driver would still need the encoder fed from a DRM scanout. That is Option B plus an encoder, which is strictly more work.
- A thin kernel HID transport adds nothing, because `hid-generic` plus hidraw already works, with 0.77 ms per chunk measured (verified in repo docs).

### Option E: userspace only (current) and extensions

Already shipped: X11 tiers, Mutter, sway, portal. Possible extension: a KDE (KWin) virtual output adapter and a Weston/headless adapter (inference: KWin offers virtual outputs and a ScreenCast interface; not verified here). This keeps all limits from section 5 (not a connector, no greeter, no TTY).

### Option F: headless DRM and dummy plug approaches

- Physical dummy HDMI plug with an EDID emulator: gives a real connector, zero software, but needs a physical GPU port and a plug per dock, and it cannot take the dock's EDID unless the plug is flashed (inference). Not a product answer.
- `drm.edid_firmware=<connector>:edid/dock.bin` forces an EDID on a real, unplugged connector (inference, general DRM feature). It ties the dock to an unused GPU port and a kernel command line edit. Not a product answer either.

### Option G: Wayland compositor integration

Already used: Mutter `RecordVirtual`, wlroots headless outputs, portal virtual monitors. Limits are already documented in the repo: Mutter virtual monitors do not necessarily show in Settings, headless GNOME has no runtime resize, portal virtual sources are unvalidated live (docs/virtual-display.md, verified). Per compositor integrations scale linearly with the number of desktops (inference).

## 4. What the evdi path buys, and what it does not change

| Aspect | Today (userspace compositor APIs) | With evdi (Option A or B) | Confidence |
|---|---|---|---|
| Connector in all desktops | X11 tier 1 only on dummy-like outputs; Mutter/sway/portal each different | One standard DRM connector in Xorg, Mutter, KWin, wlroots | Medium (inference for Wayland, upstream docs say GNOME and KDE settings applets manage evdi displays) |
| Settings UI listing | Depends on desktop, bare monitors may be unlisted | Listed like any display, per upstream docs ("displays can be controlled by standard tools, eg. `xrandr` or display settings applets") | Medium to high |
| Resolution list from EDID | Daemon picks one size | Compositor builds modes from the dock EDID that the daemon passes to `evdi_connect2` | High on mechanism, but see risk R5 on a dock that only supports few sizes |
| Multi-GPU | Capture follows the compositor | Compositor renders on its GPU and copies to the evdi buffer; works on hybrid systems with DisplayLink at the time of writing (inference), NVIDIA proprietary stack has been a recurring pain (inference) | Low to medium |
| Login screen and greeter | Not available, the daemon is a user unit in the user session | Possible only if the daemon runs as a system service before the greeter, because the connector exists only while a client is connected | Medium; requires a system unit and group based hidraw access, see section 7 |
| TTY / console | Not available | Not automatic; fbcon would bind to the evdi device only if kernel and module config allow it (inference) | Low |
| Color management | None | Display gets an EDID, so colord can key a profile on it; no ICC pipeline inside evdi (inference) | Low |
| Hotplug semantics | Daemon creates and removes monitors | Dock plug and unplug maps to `evdi_connect` and `evdi_disconnect`, compositor sees real hotplug | High |
| Latency | Capture to encode to HID | Same, plus one CPU copy and the compositor's blit to the evdi buffer, at most one frame interval extra in the worst case (inference, not measured; 25 fps means 40 ms) | Low |
| Encode and transport | libx264 plus hidraw | Identical. The dock is still fed H.264 over 4096 byte reports, 0.77 ms per chunk | Verified |
| Dock resizing | Dock cycle of about 5 s | Same dock cycle, now triggered by an evdi `mode_changed` event; it must run off the event thread to avoid the modeset deadlock noted by the Powerology README | High |

What it does not buy: lower latency, higher fps, better H.264, touch or audio, or any independence from the daemon.

Steelman of staying userspace only: the daemon already produces a usable extended monitor on X11, GNOME and sway with automatic recovery, and an evdi adapter costs about 45 days plus a standing dependency on a third party out-of-tree module that lags new kernels. If the user mostly wants "second screen works", the marginal value is the Settings UI and greeter. I weigh that as real but not small, because the repo itself documents that bare monitors may not show in settings. The staged path below tests that weight cheaply before committing.

## 5. Effort estimates

All numbers are engineer-days, low / likely / high, one engineer who knows the repo. Confidence is my confidence that the likely figure is within a factor of 1.5. These are estimates, not measurements. Anchor points: the Powerology client is a small Python program on evdi (README, verified); `screendockd` already has the ports, reconfig transaction, EDID cache and system change follower, so most Option A work is integration and packaging, not new protocol work.

### Option A: evdi adapter, no own kernel code

| Work item | Low | Likely | High | Notes |
|---|---|---|---|---|
| libevdi binding and `EvdiDisplay` adapter (device discovery by sysfs, add/open, connect with dock EDID, event loop, buffer registry, `grab_pixels`) | 6 | 10 | 18 | `dlopen` of `libevdi.so.1` through `libloading` behind a cargo feature, or bindgen with link time dependency; dlopen keeps the MIT daemon and the LGPL library loosely coupled |
| EDID and mode handling (feed `EdidStore` to `evdi_connect2`, honour pixel area and rate limits, map `mode_changed` into the existing reconfig transaction, cap refresh at the dock's declared range) | 3 | 5 | 9 | reuses `FileEdidStore` and the follow path |
| Hotplug and power (dock arrival and removal to connect and disconnect, DPMS to pause encode and heartbeat policy, stale connector recovery after SIGKILL, system unit variant) | 4 | 7 | 12 | the stale connector problem is a known evdi pitfall |
| Damage tracking and frame delivery (use `evdi_rect` to skip identical frames, convert to `Frame`, pacing, behaviour when no frame arrives) | 2 | 4 | 7 | the dock's behaviour on a long gap without frames is not measured in the repo |
| Packaging (deb and rpm: `Recommends` evdi-dkms and libevdi1, modprobe.d snippet with `initial_device_count`, udev rule with group access, system service, `vdisplay cleanup` for evdi) | 3 | 5 | 10 | |
| CI on multiple kernels | 3 | 6 | 10 | we do not build the module; CI proves the adapter compiles and a VM with the distro evdi package loads and connects |
| Testing without hardware (fake libevdi trait double, VM based smoke test with module loaded, compositor in VM) | 3 | 6 | 10 | cannot run on this host: no module, no sudo |
| Secure Boot (docs and install guidance; rely on distro DKMS and MOK flow) | 1 | 2 | 4 | |
| Docs and config (new page, updates to virtual-display, capture, configuration, install, testing, security) | 2 | 3 | 5 | |
| **Total** | **27** | **48** | **85** | confidence medium |

Maintenance per kernel release: about 0 for our code; triage of evdi build breakage is on upstream and distro packagers, with our exposure as users. Budget 0.5 to 1 day per new LTS kernel for retesting, confidence medium. Evidence of how often upstream lags: issues at 6.10, 6.17 (links in section 9).

### Option B: own virtual DRM module plus the same userspace client

| Work item | Low | Likely | High | Notes |
|---|---|---|---|---|
| Kernel module (platform device, KMS pipeline, EDID from userspace, GEM shmem, damage and event ABI, teardown races) | 25 | 45 | 80 | confidence low; largest uncertainty is the userspace ABI and locking |
| EDID and mode handling in kernel plus userspace | 5 | 8 | 12 | |
| Hotplug and power | 5 | 8 | 15 | |
| Damage tracking and frame delivery to userspace | 4 | 8 | 14 | |
| Userspace adapter (same as A) | 8 | 12 | 20 | slightly cheaper than A's client because we own the ABI |
| Packaging DKMS, deb, rpm | 4 | 7 | 12 | |
| CI on multiple kernels (build matrix, VM boot tests) | 8 | 12 | 20 | |
| Testing without hardware (VM, selftests, IGT style tests) | 8 | 12 | 20 | |
| Secure Boot and signing flow | 2 | 3 | 6 | |
| **Total** | **69** | **115** | **199** | confidence low |

Maintenance per kernel release: 2 / 4 / 8 days, with about 5 to 6 releases a year (inference on cadence), so 10 to 45 days a year for as long as the module is supported. This is the main reason to reject B unless evdi fails.

### Option C: vkms or vgem plus kmsgrab style capture

| Work item | Low | Likely | High | Notes |
|---|---|---|---|---|
| KMS framebuffer capture adapter (privileged, whole frames) | 8 | 14 | 25 | |
| EDID and modes through `drm.edid_firmware` and vkms configfs | 4 | 8 | 15 | the vkms doc lists EDID support as wanted, so this may not work at all |
| Hotplug, power, privilege model | 4 | 8 | 15 | needs CAP_SYS_ADMIN or root |
| Packaging, CI, testing | 5 | 8 | 15 | |
| **Total** | **21** | **38** | **70** | confidence low; likely a dead end because of EDID and root |

### Option D: kernel mode encode

Not estimated. Any honest range would exceed 250 days and still rely on userspace encoders, so I classify it as no-go.

### Option E and G: more userspace adapters (for example KWin virtual outputs)

5 / 8 / 15 days per desktop, confidence low (not researched beyond inference). They extend coverage but do not give a connector, greeter or TTY.

### Other operating systems (brief, mostly inference)

| Target | What a "proper" display means | Effort low / likely / high | Notes |
|---|---|---|---|
| Windows | An Indirect Display Driver (IddCx) adapter; user-mode only per Microsoft ("The IDD uses a user-mode model and doesn't support kernel-mode components"). The vendor driver itself is IddCx (repo docs, verified) | 40 / 70 / 120 | Needs driver signing and a Microsoft partner account for production (inference); open source IDD samples exist (VirtualDrivers/Virtual-Display-Driver, which lists custom EDID support); an unsigned test mode works for development only |
| macOS | A virtual display through private `CGVirtualDisplay` style APIs or a DriverKit extension | 30 / 55 / 100 | Largely unresearched; the vendor app needs the Screen Recording permission (verified in the repo's reviews page), which suggests a capture based design |
| ChromeOS, Android | Not covered | n/a | Not researched |

I recommend not starting any non-Linux port until the Linux evdi path has passed its go/no-go gates, because the protocol, EDID and dock cycle logic are the shared asset and the Linux stack is where measurements exist.

## 6. What can be tested in this environment

Verified capability on this host: Rust and C toolchains, kernel headers for the running kernel, mounted configfs, in-tree `vkms`, `vgem`, `gud`, `udl` modules on disk. Not available: evdi, `dkms`, sudo, a way to load modules, a graphical compositor session against real connectors, and Secure Boot (disabled).

| Test | Here? | How |
|---|---|---|
| Adapter logic against a fake libevdi (trait double returning scripted events and frames) | Yes | unit and integration tests, same style as the existing Mutter and X11 doubles |
| FFI signature check against `evdi_lib.h` | Yes (needs header vendored or `libevdi-dev`, which is not installed; headers are LGPL, MIT compatible for compile only use is a point to confirm) | compile test |
| Out-of-tree evdi module compile against `7.0.0-38` and `-44` headers | Probably yes (build is unprivileged with `make -C /lib/modules/$(uname -r)/build M=...`), but needs the evdi source, which means a download; not done here | inference |
| Load evdi, create a connector, run a compositor | No | needs root and module load: use a VM or a spare host |
| Greeter, TTY, Secure Boot MOK flow | No | needs a VM with OVMF and a display manager |
| Real dock behaviour with evdi frames | No | hardware validation procedure to add to docs/hardware-validation.md |

Existing tests (Xvfb, Xorg dummy, headless sway and GNOME) stay valid for the other adapters and become the regression suite that the evdi adapter must not break.

## 7. Risks, legal and safety

| Id | Risk | Severity | Mitigation |
|---|---|---|---|
| R1 | Out-of-tree module on every user's kernel: evdi build failures on new kernels left users without a display in the past (Fedora 40 with 6.10, Ubuntu noble HWE with 6.17) | Medium | Keep the evdi path optional with automatic fallback to the current compositor adapters (`fallback` setting already exists); never make it the only way to run |
| R2 | Kernel panics or hangs from a module bug | Medium for A (third party, mature), high for B (own code) | Prefer A; for B require a soak test and a kill switch (module blacklist documented) |
| R3 | Secure Boot: unsigned DKMS modules refuse to load; MOK enrolment needs console interaction; the MOK private key being root readable weakens the kernel boundary (Ubuntu docs) | Medium | Document the distro flow, do not ship our own signing key, never advise disabling Secure Boot |
| R4 | Distro policy: Ubuntu and Debian package evdi in universe/main (version skew: Debian trixie 1.14.8, Ubuntu resolute 1.14.15 verified), Fedora uses a separate RPM project; an own module would need its own packaging or rejection by distros | Medium | Depend on distro evdi packages; document minimum 1.14.15 for kernel 7.0 (second hand, to verify) |
| R5 | Mode negotiation mismatch: a desktop expects fast modesets while the dock needs a 5 s cycle; EDID modes the compositor selects may exceed what the panel renders well (1080p is accepted but icons are tiny and the repo says both sizes lag) | Medium | Advertise a small EDID subset, run the dock cycle off the event thread, keep the compositor's modeset unblocked (README deadlock pitfall) |
| R6 | Privilege: greeter support needs the daemon as a system service with hidraw access beyond `uaccess` (seat user only) | Medium | Group based udev rule plus a dedicated system user; extend docs/security-assessment.md; this widens the attack surface of the daemon, which parses dock JSON and EDID |
| R7 | Multi-GPU and NVIDIA proprietary interactions | Low to medium | Document as unsupported until tested; do not promise |
| R8 | Stale connector after crash, X provider attach, 5 s libevdi master wait, layout blanking on connect | Low | Known from the Powerology README; handle in `vdisplay cleanup` and startup ordering |
| R9 | evdi API explicitly unstable | Low | Pin a minimum libevdi version and gate on `evdi_get_lib_version` |
| R10 | Legal: licences. The daemon is MIT; dynamic loading of LGPL libevdi is fine as long as the library stays replaceable. An own module must be GPL and live in its own directory and package, never linked into the MIT daemon. Do not copy evdi code into the MIT tree | Medium | Keep a licence boundary in docs; legal review before shipping B |
| R11 | Trademark: "DisplayLink" and "evdi" belong to DisplayLink (UK) Ltd; CABLETIME and the USB IDs belong to their owners | Low | Do not describe the daemon as DisplayLink compatible beyond factual "uses evdi"; keep the existing "not affiliated" statement; use neutral package names |
| R12 | Clean-room claim: the repo is clean-room from protocol prose. Studying evdi's public API is fine; copying kernel code is not | Low | Write the adapter from the public header and docs only |

## 8. Recommendation and staged path

### Decision

Option A as an optional backend, staged, with an explicit stop. Alternatives considered and the property that ruled each out:

- Own module (B): the cost driver is DRM API churn across 5 to 6 kernel releases a year, which evdi's issue history shows consuming a full-time team; our value is in the protocol, not the compositor interface.
- vkms and writeback (C): EDID injection is a documented gap and frame delivery needs root and gives no damage information.
- Kernel encode (D): there is no kernel encoder and no reason to build one.
- More compositor adapters (E, G): do not produce a connector, greeter or TTY, which are the stated goals.

Steelman against my pick, and my answer: evdi may never work well on the user's actual desktop (NVIDIA, GNOME on Wayland). That is why Stage 0 exists before any adapter code.

### Stage 0: spike, 3 to 5 days, throw away code

Run on a host with a real GPU session where modules can be loaded (not this one). Install `evdi-dkms` 1.14.15 or newer from the distro and use the Powerology client or a 100 line Rust spike. Measure, with the real dock:

1. The dock EDID passed to `evdi_connect2` produces a connector in GNOME Settings (Wayland and X11), KDE Settings and `xrandr` or `wlr-randr`.
2. A mode change made in Settings reaches the client as `mode_changed`, and the 5 s dock cycle completes without blocking the compositor.
3. Frames arrive as damage updates, with CPU cost and added latency compared with the current Mutter or X11 path (record at 25 fps, 720p).
4. Plug and unplug of the dock, daemon crash and restart, suspend and resume leave no stale connector and no layout corruption.
5. One kernel upgrade with DKMS rebuild succeeds on the test distro.

### Stage 1: adapter in this repo, 25 to 45 days

Exact changes (proposals; names are mine, not existing code):

- `src/screendockd/src/adapters/evdi/` new module with `types.rs` (errors, event enum, constants), `sys.rs` (the `libevdi` function table loaded with `libloading`, minimum version check) and `device.rs` (sysfs discovery of evdi nodes, add, open, connect, disconnect, buffer registry, event loop on its own thread with `evdi_get_event_ready` polled alongside a stop fd).
- `adapters/display/evdi.rs`: `EvdiVirtualDisplay` implementing `VirtualDisplay`, `EvdiMonitor` implementing `VirtualMonitor`. `create` connects with the cached dock EDID. `rect()` comes from the compositor's `mode_changed` (like the Mutter case, `placement` and `anchor` do not apply). `resize` is an EDID or mode change request, which for evdi means the compositor decides, so `resize` mirrors the Mutter behaviour and only retargets capture.
- `adapters/capture/evdi.rs`: `EvdiCapture` implementing `CaptureBackend`. It owns the frame source for its device, returns `Frame` values (the evdi pixel format is expected to be `DRM_FORMAT_XRGB8888`, which maps to `Bgrx8888`; confirm in Stage 0) and uses damage rectangles to skip unchanged frames. This needs one new capture area variant in `ports/types.rs` (for example `CaptureArea::Device { id }`) because the existing `Whole`, `Region`, `Stream` do not name an evdi device; X11 and Wayland backends refuse it with `Unsupported`, as they do for other foreign areas at the time of writing.
- `core/display_policy.rs`: add the evdi mechanism. Order under `auto`: evdi only when the library loads, the module is present and the dock EDID is cached; otherwise fall through to the current order. `DisplayFacts` gains `evdi_available` and the reason when absent, so `vdisplay probe` can print it.
- Config keys under `[virtual_display]` (proposals): `driver = "auto" | "evdi" | "compositor"` (default `compositor` until the go criteria below pass), `evdi_library` (path override), `evdi_device_count`, `evdi_edid = "dock" | "<path>"`, `evdi_refresh_hz` (cap). Live keys: none initially; a change recreates the connector.
- `vdisplay cleanup`: also disconnects and releases evdi devices owned by a dead daemon (recovery file records device index).
- `deploy/`: `modprobe.d` snippet (`options evdi initial_device_count=1`, `softdep evdi pre: drm_kms_helper`, matching the upstream module README), a group based udev rule variant for the system service, `screendockd-system.service` for greeter support, deb `Recommends: evdi-dkms, libevdi1`, rpm `Recommends`.
- Docs: new `docs/evdi.md` (design, setup, limits, measured results from Stage 0), update `virtual-display.md` (new mechanism and policy order), `capture.md` (new backend), `configuration.md` (keys), `install.md` (module, Secure Boot MOK steps, system service), `testing.md` (fake libevdi tests, VM procedure), `hardware-validation.md` (new procedures), `security-assessment.md` (system service, evdi trust boundary: upstream itself says the module and library channel is not access controlled), `design.md` (scope list).
- Tests: fake `libevdi` table for event ordering, mode change race, dock cycle off event thread, SIGKILL recovery; keep all existing adapter tests green; add a VM smoke test in CI that loads the distro evdi package and connects with the fixture EDID.

### Stage 2: promote to default, 5 to 10 days

Flip `driver` default to `auto` after the gates pass, ship the system unit as opt in, add the greeter documentation after a VM test with GDM and SDDM.

### Go and no-go criteria

Go from Stage 0 to Stage 1 only if all hold on at least one GNOME session and one KDE or wlroots session:

- the connector appears in the desktop Settings with the dock EDID modes,
- a mode change does not hang the compositor and the dock cycle completes,
- added latency is at most one frame interval (40 ms at 25 fps) and CPU overhead is under 10 percent of one core above the current path at 720p,
- plug, unplug, crash and resume leave no stale state in three consecutive runs,
- a distro packaged evdi builds on the current LTS kernel (or the maintainers' fix is available within one release).

Go from Stage 1 to default `auto` only if the CI VM test and a two week soak on a real session show no crash and no unrecoverable stale connector.

Consider Option B (own module) only if at least one of these holds, and prefer carrying a small patch set against evdi first: evdi cannot carry the needed EDID or mode behaviour and upstream will not accept patches; evdi fails to build on two consecutive LTS kernels with no fix; or a licensing or trademark objection arises. Any B decision needs a fresh estimate and legal review. Do not start Windows or macOS ports before Stage 1 passes.

## 9. Sources (untrusted web content, used as data)

- evdi repository, licences, kernel support, future development notes: https://github.com/DisplayLink/evdi
- evdi library header (API surface, version 1.15.1): https://raw.githubusercontent.com/DisplayLink/evdi/main/library/evdi_lib.h
- evdi module README (module options, softdep, sysfs add): https://raw.githubusercontent.com/DisplayLink/evdi/main/module/README.md
- evdi docs, API not stable: https://displaylink.github.io/evdi
- DisplayLink on why Manager is closed, pipeline description: https://support.displaylink.com/knowledgebase/articles/1104056-why-has-displaylink-not-released-source-code-for-t
- Debian evdi-dkms package: https://packages.debian.org/trixie/evdi-dkms
- evdi build breakage on Fedora 40 kernel 6.10: https://github.com/displaylink-rpm/displaylink-rpm/issues/272
- evdi build breakage on Ubuntu noble linux-6.17: https://bugs.launchpad.net/bugs/2133812
- Powerology Actions USB display driver (same chip family, evdi based): https://github.com/N7le/powerology-actions-usb-display-linux-driver
- vkms kernel documentation: https://docs.kernel.org/gpu/vkms.html
- DRM mode setting helpers: https://docs.kernel.org/gpu/drm-kms-helpers.html (a fetch of the writeback section of drm-kms.html timed out, so writeback statements are inference)
- GUD gadget project (GUD host driver mainline since 5.13): https://github.com/samcday/gud-gadget
- GUD driver patch series: https://lwn.net/Articles/818874
- Windows Indirect Display Driver model: https://learn.microsoft.com/en-us/windows-hardware/drivers/display/indirect-display-driver-model-overview
- Open source IDD based virtual display driver: https://github.com/VirtualDrivers/Virtual-Display-Driver
- DKMS, MOK and Secure Boot: https://safeguard.sh/resources/blog/linux-kernel-module-signing-and-dkms-explained-sig-enforce-mok-keys-secure-boot-and-out-of-tree-drivers and https://vcheng.org/2023/01/22/ubuntu-linux-secure-boot-dkms

## 10. Open questions

- Does the panel render sizes other than 1280x720 correctly? Unmeasured in the repo, and it limits which EDID modes are worth advertising.
- Does the dock cope with a long gap without frames when the compositor does not repaint?
- Does GNOME list a Mutter `RecordVirtual` monitor in Settings at the time of writing? The repo says it depends on the desktop; not tested here.
- Does the evdi pixel format and stride match the `Frame` assumptions? Confirm in Stage 0.
