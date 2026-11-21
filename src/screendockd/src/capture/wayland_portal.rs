//! xdg-desktop-portal + PipeWire capture backend for Wayland sessions.
//!
//! Per `docs/implementation-plan.md`'s Milestone 4 Stage 1, this will be validated during
//! development against a headless Weston/wlroots instance (synthetic-but-real framebuffer
//! content via the real portal/PipeWire negotiation path), with Stage 2 validating against a
//! real interactive Wayland session later. Not implemented here — Milestone 1 scope is discovery
//! + framing only.

use crate::capture::{CaptureBackend, CaptureError, Frame};

/// Placeholder; Milestone 4 will give this real fields (portal session handle, PipeWire stream).
pub struct WaylandPortalCapture;

impl CaptureBackend for WaylandPortalCapture {
    fn next_frame(&mut self) -> Result<Frame, CaptureError> {
        todo!("Milestone 4: xdg-desktop-portal ScreenCast + PipeWire frame consumption")
    }
}
