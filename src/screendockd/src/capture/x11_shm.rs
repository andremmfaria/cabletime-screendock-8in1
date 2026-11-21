//! XShm (X Shared Memory Extension) capture backend for X11/Xorg sessions.
//!
//! Per `docs/implementation-plan.md`'s Milestone 4 Stage 1, this will be validated during
//! development against a virtual Xvfb display (synthetic-but-real framebuffer content), and
//! Stage 2 additionally validates against a real interactive X11 session later. Not implemented
//! here — Milestone 1 scope is discovery + framing only.

use crate::capture::{CaptureBackend, CaptureError, Frame};

/// Placeholder; Milestone 4 will give this real fields (X11 display connection, XShm segment).
pub struct X11ShmCapture;

impl CaptureBackend for X11ShmCapture {
    fn next_frame(&mut self) -> Result<Frame, CaptureError> {
        todo!("Milestone 4: XShm frame grab, validated against Xvfb then a real X11 session")
    }
}
