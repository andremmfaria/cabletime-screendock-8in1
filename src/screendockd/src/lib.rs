//! `screendockd` — userspace Linux driver for the CABLETIME ScreenDock (CB09W).
//!
//! Exposed as a library (in addition to the `screendockd` binary) so integration tests under
//! `tests/` can exercise individual modules directly. See `docs/implementation-plan.md` for the
//! overall architecture and milestone breakdown; this crate implements device discovery, HID
//! wire framing, hidraw I/O, the MI_00 dock session (heartbeat, reader and handshake), the
//! hardware validation routines, screen capture, H.264 encoding and the daemon loop.

pub mod adapters;
pub mod bench;
pub mod config;
pub mod daemon;
pub mod discovery;
pub mod fake_dock;
pub mod heartbeat;
pub mod hidraw_io;
pub mod ports;
pub mod protocol;
pub mod session;
pub mod telemetry;
pub mod validation;
