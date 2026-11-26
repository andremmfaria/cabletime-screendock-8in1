//! The application layer: the daemon that wires a session, a capture backend and an encoder
//! together through their ports, and the supervisor that owns the live settings and hands changes to the video loop.

pub mod daemon;
pub mod monitor;
pub mod pipeline;
pub mod supervisor;
pub mod types;
