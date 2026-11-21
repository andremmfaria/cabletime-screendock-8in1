//! `screendockd` binary entry point.
//!
//! Milestone 1 scope only: the discovery and framing modules are fully implemented and unit
//! tested (see `src/discovery.rs`, `src/framing.rs`, `tests/framing.rs`), but the runtime
//! orchestration described in `docs/implementation-plan.md`'s Architecture section (capture
//! thread, encode thread, framer, heartbeat loop, all wired to real hidraw I/O) is not built yet
//! — that lands across Milestones 2-6. This `main` is intentionally a placeholder, not a stub
//! `todo!()`, so the binary builds and runs (producing a clear "not implemented yet" message)
//! rather than panicking if someone runs it today.

fn main() {
    eprintln!(
        "screendockd: Milestone 1 scaffold only (device discovery + HID wire framing). \
         The capture/encode/heartbeat pipeline is not wired up yet — see \
         docs/implementation-plan.md for the milestone roadmap."
    );
}
