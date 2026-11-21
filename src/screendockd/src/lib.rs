//! `screendockd` — userspace Linux driver for the CABLETIME ScreenDock (CB09W).
//!
//! Exposed as a library (in addition to the `screendockd` binary) so integration tests under
//! `tests/` can exercise individual modules directly. See `docs/implementation-plan.md` for the
//! overall architecture and milestone breakdown; this crate currently implements Milestone 1
//! (device discovery + HID wire framing) in full, with the remaining modules present as typed
//! stubs so the project structure is navigable ahead of later milestones.

pub mod discovery;
pub mod framing;
pub mod heartbeat;
pub mod capture;
pub mod encode;
pub mod hidraw_io;
