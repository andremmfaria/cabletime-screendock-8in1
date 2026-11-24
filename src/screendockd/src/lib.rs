//! `screendockd` — userspace Linux driver for the CABLETIME ScreenDock (CB09W).
//!
//! Exposed as a library (in addition to the `screendockd` binary) so integration tests under
//! `tests/` can exercise individual modules directly. See `docs/implementation-plan.md` for the
//! overall architecture and milestone breakdown; this crate implements device discovery, HID
//! wire framing, hidraw I/O, the MI_00 dock session (heartbeat, reader and handshake), the
//! hardware validation routines, screen capture, H.264 encoding and the daemon loop.

pub mod annexb;
pub mod bench;
pub mod capture;
pub mod daemon;
pub mod discovery;
pub mod display;
pub mod encode;
pub mod fake_dock;
pub mod framing;
pub mod heartbeat;
pub mod hidraw_io;
pub mod session;
pub mod utils;
pub mod validation;
