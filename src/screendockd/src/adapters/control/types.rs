//! Types of the control adapters: the socket server guard and the client.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// Directory under `$XDG_RUNTIME_DIR` that holds the daemon's runtime files.
pub(super) const RUNTIME_SUBDIR: &str = "screendockd";

/// File name of the control socket inside the runtime directory.
pub(super) const SOCKET_FILE_NAME: &str = "control.sock";

/// Longest socket path a Unix socket address holds (`sun_path` is 108 bytes with the NUL).
pub(super) const MAX_SOCKET_PATH: usize = 107;

/// Pause between accept attempts, which bounds how long a stop request waits.
pub(super) const ACCEPT_POLL: Duration = Duration::from_millis(25);

/// Longest the server waits for a client to send its request or take its answer.
pub(super) const CLIENT_IO_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest the server waits for a client to finish sending after it answered.
pub(super) const DRAIN_TIMEOUT: Duration = Duration::from_millis(200);

/// Most bytes the server discards from a client after it answered.
pub(super) const DRAIN_LIMIT: u64 = 1 << 20;

/// Longest the server waits for the daemon to answer a request.
pub(super) const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the client waits for an answer.
pub const CLIENT_REPLY_TIMEOUT: Duration = Duration::from_secs(60);

/// The listening control socket. Dropping it stops the accept thread and removes the socket
/// file.
#[derive(Debug)]
pub struct ControlServer {
    pub(super) path: PathBuf,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) thread: Option<JoinHandle<()>>,
}

/// Client of a daemon's control socket.
#[derive(Debug, Clone)]
pub struct UnixControlClient {
    pub(super) path: PathBuf,
    pub(super) timeout: Duration,
}
