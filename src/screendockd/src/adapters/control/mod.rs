//! Control socket: a Unix stream socket that carries one newline delimited JSON request and one
//! response per connection.
//!
//! The server turns each request into a [`Command`](crate::ports::control::Command) for the
//! daemon's supervisor and writes back the answer. It never interprets a request beyond
//! parsing the wire format, so every policy decision stays with the supervisor. Requests are
//! client data: a line is capped, parsed as JSON only, and nothing in it is used as a path or
//! a format string.

mod client;
mod server;
pub mod types;

use std::path::PathBuf;

pub use self::types::{ControlServer, UnixControlClient, CLIENT_REPLY_TIMEOUT};
use self::types::{RUNTIME_SUBDIR, SOCKET_FILE_NAME};
use crate::utils::config::ControlConfig;

/// The socket path the config selects: the configured path, or the default under
/// `$XDG_RUNTIME_DIR`. `None` when neither is available.
pub fn socket_path(config: &ControlConfig) -> Option<PathBuf> {
    if !config.socket.is_empty() {
        return Some(PathBuf::from(&config.socket));
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty())?;
    Some(
        PathBuf::from(runtime)
            .join(RUNTIME_SUBDIR)
            .join(SOCKET_FILE_NAME),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_socket_path_wins() {
        let config = ControlConfig {
            enabled: true,
            socket: "/run/custom.sock".into(),
        };
        assert_eq!(
            socket_path(&config),
            Some(PathBuf::from("/run/custom.sock"))
        );
    }
}
