//! Direct `write(2)` I/O against the resolved MI_00/MI_01 `/dev/hidrawN` nodes.
//!
//! Per `docs/implementation-plan.md`'s "hidraw access" decision, this intentionally does *not*
//! use the `hidapi` crate or any `ioctl`/"set report" call — the research confirms the correct
//! mechanism is a plain `write(2)` to the interrupt OUT path, and avoiding any extra abstraction
//! layer here is specifically what Milestone 2 is designed to validate (the write-timing risk
//! named in the plan's Risks section). Not implemented here: Milestone 2 needs real hardware
//! (the physical dock) to measure actual per-chunk write latency, which is out of scope for
//! Milestone 1 (discovery + framing, no hardware required).

use std::path::Path;

/// Placeholder for an opened hidraw node; Milestone 2 will give this a real `std::fs::File`
/// (or `nix`-based raw fd) field and a `write_chunk(&[u8])` method with measured timing.
pub struct HidrawWriter;

impl HidrawWriter {
    /// Opens a resolved hidraw node (e.g. the path from `discovery::DockHidrawPaths`) for
    /// writing.
    pub fn open(_path: &Path) -> std::io::Result<Self> {
        todo!("Milestone 2: open hidraw node, plain write(2) back-to-back, measure per-chunk timing")
    }
}
