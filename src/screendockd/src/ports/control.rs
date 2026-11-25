//! Control port: the command the control server and the signal handler hand to the daemon, and
//! the client interface `screendockd ctl` uses.

pub use super::types::{Command, ControlClient, ControlError};

impl From<crate::protocol::control::ControlWireError> for ControlError {
    fn from(e: crate::protocol::control::ControlWireError) -> Self {
        Self::Wire(e.to_string())
    }
}
