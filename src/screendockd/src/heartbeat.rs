//! JSON-RPC heartbeat loop on MI_00.
//!
//! Per `docs/research/full-protocol-spec-2026-11-21.md`, MI_00 is a JSON-RPC control channel
//! (HID Report ID 1) whose confirmed, live-referenced method is:
//! `{"jsonrpc":"2.0","method":"heartbeat","params":{"session-id":"<tick-count>"}}`, sent
//! periodically (observed roughly once per second from the vendor's own Windows service).
//!
//! This module's actual runtime loop (building the message, framing it via `framing::frame_message`
//! with Report ID 1, writing it to the resolved MI_00 hidraw node on a ~1s cadence) is Milestone 3
//! scope, gated on the heartbeat-dependency isolation test described in
//! `docs/implementation-plan.md`'s Milestones section — that test needs real hardware to determine
//! whether the heartbeat is load-bearing for video rendering or purely cosmetic, which in turn
//! shapes this loop's error-handling/threading behavior. Not implemented here to avoid building
//! runtime logic against an unverified assumption.

/// Runs the heartbeat loop against an already-open MI_00 hidraw writer.
///
/// # Panics
/// Always — this is a Milestone 1 structural stub. See the module doc comment.
pub fn run_heartbeat_loop() -> ! {
    todo!("Milestone 3: ~1/sec heartbeat loop on MI_00, after the isolated hardware test")
}
