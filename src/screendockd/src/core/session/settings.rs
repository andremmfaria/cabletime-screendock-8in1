use std::time::Duration;

pub use super::types::SessionSettings;
use crate::core::policy::{
    DEFAULT_MAX_HEARTBEAT_FAILURES, DEFAULT_READY_TIMEOUT, DEFAULT_REPLY_TIMEOUT,
    DEFAULT_RETRY_DELAY, DEFAULT_SETTLE_DELAY,
};
use crate::protocol::framing::PictureSize;
use crate::protocol::heartbeat::SessionId;
use crate::protocol::messages;

/// The host's hostname, or an empty string when it can't be read.
pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is valid for `buf.len()` bytes; the final byte is never written by a
    // successful call longer than the buffer, so the contents stay NUL terminated below.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len() - 1) };
    if rc != 0 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

impl SessionSettings {
    /// Defaults for a picture size: fresh session id, hostname-derived machine name, the full
    /// handshake enabled.
    pub fn new(size: PictureSize, heartbeat_interval: Duration) -> Self {
        Self {
            session_id: SessionId::generate(),
            machine_name: messages::machine_name_from_hostname(&hostname()),
            size,
            heartbeat_interval,
            handshake: true,
            send_stopped: true,
            ready_timeout: DEFAULT_READY_TIMEOUT,
            reply_timeout: DEFAULT_REPLY_TIMEOUT,
            settle_delay: DEFAULT_SETTLE_DELAY,
            retry_delay: DEFAULT_RETRY_DELAY,
            max_heartbeat_failures: DEFAULT_MAX_HEARTBEAT_FAILURES,
        }
    }

    /// Longest a dock cycle can take with these timeouts, plus a margin for scheduling.
    pub fn cycle_timeout(&self) -> Duration {
        self.reply_timeout * 2 + self.settle_delay + self.retry_delay + Duration::from_secs(2)
    }

    /// Same settings without the handshake and without the reader.
    pub fn without_handshake(mut self) -> Self {
        self.handshake = false;
        self
    }
}
