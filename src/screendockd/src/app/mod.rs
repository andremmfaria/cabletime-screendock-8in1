//! The application layer: the daemon that wires a session, a capture backend and an encoder
//! together through their ports, and the supervisor that owns the live settings.

pub mod daemon;
pub mod supervisor;
pub mod types;
