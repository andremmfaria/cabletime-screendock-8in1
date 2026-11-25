//! Session timing defaults and the polling constants shared by the session threads.

pub use super::types::{
    DEFAULT_MAX_HEARTBEAT_FAILURES, DEFAULT_READY_TIMEOUT, DEFAULT_REPLY_TIMEOUT,
    DEFAULT_RETRY_DELAY, DEFAULT_SETTLE_DELAY,
};
pub(super) use super::types::{DOCK_ALIVE_WINDOW, POLL_SLICE};
