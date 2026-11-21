//! Capture backend abstraction (Milestone 4).
//!
//! Per `docs/implementation-plan.md`'s Architecture and Screen Capture sections: a
//! `CaptureBackend` trait with two implementations — `x11_shm` (XShm, for X11/Xorg sessions) and
//! `wayland_portal` (xdg-desktop-portal + PipeWire, for Wayland sessions) — selected at runtime
//! (detected session type) or build time (feature flag). During development on the headless
//! Linux host, each backend points at a virtual/headless display server (Xvfb or headless
//! Weston) rather than a real session, per Milestone 4's two-stage validation plan.
//!
//! Not implemented here: this module is structural scaffolding only. Milestone 4 fills in the
//! real `Frame` representation (pixel format, dimensions, buffer ownership) and both backend
//! implementations.

pub mod wayland_portal;
pub mod x11_shm;

/// Minimal placeholder; Milestone 4 will define the real captured-frame representation.
#[derive(Debug)]
pub struct Frame;

/// Minimal placeholder error type; Milestone 4 will give this real variants (no session,
/// portal/PipeWire negotiation failure, XShm attach failure, etc.).
#[derive(Debug)]
pub struct CaptureError;

/// Common interface the encode loop consumes, independent of which backend is active.
pub trait CaptureBackend {
    /// Blocks until the next frame is available and returns it.
    fn next_frame(&mut self) -> Result<Frame, CaptureError>;
}
