//! `screendockd` — userspace Linux driver for the CABLETIME ScreenDock (CB09W).
//!
//! Exposed as a library (in addition to the `screendockd` binary) so integration tests under
//! `tests/` can exercise individual modules directly. See `docs/implementation-plan.md` for the
//! overall architecture and milestone breakdown; this crate implements device discovery, HID
//! wire framing, hidraw I/O, the heartbeat, the hardware validation routines and screen capture;
//! encode is still a typed stub.

pub mod annexb;
pub mod bench;
pub mod capture;
pub mod daemon;
pub mod discovery;
pub mod display;
pub mod encode;
pub mod framing;
pub mod heartbeat;
pub mod hidraw_io;
pub mod utils;
pub mod validation;
