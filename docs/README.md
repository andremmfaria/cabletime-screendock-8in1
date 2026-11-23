# screendockd documentation

Documentation for `screendockd`, an independent, unofficial Linux userspace driver for the CABLETIME ScreenDock (CB09W, USB 1de1:f201). It is not affiliated with or endorsed by CableTime. These pages describe the current state of the project; `implementation-plan.md` and `research/` are the planning and research record.

## Project pages

| Page | Contents |
|---|---|
| [architecture.md](architecture.md) | Modules, data flow, and implementation status of each one |
| [protocol-implementation.md](protocol-implementation.md) | How the wire protocol is implemented: framing, hidraw I/O, heartbeat, Annex-B splitting |
| [configuration.md](configuration.md) | Config file resolution, fields, command line, environment variables |
| [capture.md](capture.md) | Capture layer: frame types, backend selection, X11 XShm and Wayland portal backends, the `capture` command |
| [encoding.md](encoding.md) | Encoder trait, libx264 and prerecorded encoders, message policy, the `bench` CPU measurement |
| [daemon.md](daemon.md) | The `run` daemon: startup, loops, error policy, shutdown, resolution picker, device section |
| [testing.md](testing.md) | Test layers, how to run them, and what each one covers |
| [hardware-validation.md](hardware-validation.md) | Procedures to run on the dock for write timing (M2) and heartbeat isolation (M3), the streaming daemon (M6), the resolution picker (M7), and what to record |

## Device and research pages

| Page | Contents |
|---|---|
| [overview.md](overview.md) | What the device is |
| [specs.md](specs.md) | Technical specifications |
| [capabilities.md](capabilities.md) | Device capabilities |
| [security-assessment.md](security-assessment.md) | Security assessment of the vendor software |
| [reviews-and-caveats.md](reviews-and-caveats.md) | Reviews and caveats |
| [sources.md](sources.md) | Sources |
| [implementation-plan.md](implementation-plan.md) | Implementation plan and milestones |
| [research/](research/) | Protocol research, including `full-protocol-spec-2026-11-21.md` |
