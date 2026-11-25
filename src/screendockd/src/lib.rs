//! `screendockd` — userspace Linux driver for the CABLETIME ScreenDock (CB09W).
//!
//! Exposed as a library (in addition to the `screendockd` binary) so integration tests under
//! `tests/` can exercise individual modules directly. See `docs/implementation-plan.md` for the
//! overall architecture and milestone breakdown; this crate implements device discovery, HID
//! wire framing, hidraw I/O, the MI_00 dock session (heartbeat, reader and handshake), the
//! hardware validation routines, screen capture, H.264 encoding and the daemon loop.

pub mod adapters;
pub mod app;
pub mod config;
pub mod core;
pub mod ports;
pub mod protocol;
pub mod telemetry;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
#[cfg(feature = "tools")]
pub mod tools;
