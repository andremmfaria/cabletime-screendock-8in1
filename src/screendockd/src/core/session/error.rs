use thiserror::Error;

use crate::ports::transport::TransportError;
use crate::protocol::framing::ReportId;

/// Errors from the session.
#[derive(Debug, Error)]
pub enum SessionError {
    /// The handle passed in isn't the MI_00 control channel.
    #[error("session needs the control (MI_00) channel, got {got:?}")]
    WrongChannel { got: ReportId },
    /// Duplicating the MI_00 descriptor for the reader thread failed.
    #[error("couldn't set up the MI_00 reader: {0}")]
    Setup(TransportError),
    /// The first heartbeat write failed, so the control channel is unusable.
    #[error("startup heartbeat write failed: {0}")]
    StartupHeartbeat(TransportError),
    /// A control message didn't fit one report.
    #[error("control message of {len} bytes doesn't fit one report")]
    MessageTooLarge { len: usize },
    /// Writing a control message failed.
    #[error("control write failed: {0}")]
    Write(TransportError),
    /// The dock answered a request with an error object.
    #[error("dock rejected {request}: {detail}")]
    Rejected {
        request: &'static str,
        detail: String,
    },
    /// The dock answered `notify-projection-started` with something other than `isAllowed`.
    #[error("dock denied projection: {0}")]
    Denied(String),
    /// The MI_00 reader stopped (read error or end of stream).
    #[error("MI_00 reader stopped: {0}")]
    ReaderStopped(String),
    /// A session thread panicked.
    #[error("{0} thread panicked")]
    Panicked(&'static str),
}
