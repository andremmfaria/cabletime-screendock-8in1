//! `screendockd` — userspace Linux driver for the CABLETIME ScreenDock (CB09W).
//!
//! Exposed as a library (in addition to the `screendockd` binary) so integration tests under
//! `tests/` can exercise individual modules directly. See `docs/implementation-plan.md` for the
//! overall architecture and milestone breakdown; this crate implements device discovery, HID
//! wire framing, hidraw I/O, the heartbeat and the hardware validation routines; capture and
//! encode are still typed stubs.

pub mod annexb;
pub mod discovery;
pub mod framing;
pub mod heartbeat;
pub mod capture;
pub mod encode;
pub mod hidraw_io;
pub mod utils;
pub mod validation;
