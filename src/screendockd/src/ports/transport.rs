//! Transport port: the [`DockTransport`] trait the session, the daemon and the validation tools
//! write and read through, with its error type and write timing.
//!
//! The hidraw implementation lives in `adapters::transport::hidraw`.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

use crate::protocol::framing::{FramingError, ParsedMessage, ReportId, REPORT_SIZE};

/// Errors from opening, writing to or reading from a dock transport (a hidraw node).
#[derive(Debug, Error)]
pub enum TransportError {
    /// The node couldn't be opened (missing, no permission, ...).
    #[error("couldn't open hidraw node {path:?}: {source}")]
    Open { path: PathBuf, source: io::Error },
    /// A chunk was not exactly `REPORT_SIZE` bytes long.
    #[error("chunk {index} is {len} bytes, expected exactly {REPORT_SIZE}")]
    BadChunkSize { index: usize, len: usize },
    /// A chunk's Report ID byte didn't match the channel this handle was opened for.
    #[error("chunk {index} has Report ID {got}, but this handle is the {channel:?} channel")]
    WrongChannel {
        index: usize,
        channel: ReportId,
        got: u8,
    },
    /// `write(2)` failed on a chunk.
    #[error("write of chunk {index} failed: {source}")]
    Write { index: usize, source: io::Error },
    /// `write(2)` accepted fewer bytes than the chunk holds.
    #[error("short write on chunk {index}: {written} of {expected} bytes")]
    ShortWrite {
        index: usize,
        written: usize,
        expected: usize,
    },
    /// `read(2)` failed.
    #[error("read failed: {0}")]
    Read(io::Error),
    /// Duplicating the descriptor failed.
    #[error("couldn't duplicate the hidraw descriptor: {0}")]
    Clone(io::Error),
    /// `poll(2)` failed.
    #[error("poll failed: {0}")]
    Poll(io::Error),
    /// The read side hit end-of-file (or a zero-length read) before a full message arrived.
    #[error("end of stream while reading chunk {index}")]
    UnexpectedEof { index: usize },
    /// The inbound chunks didn't parse as a valid message.
    #[error("inbound message failed to parse: {0}")]
    Framing(#[from] FramingError),
}

/// Per-chunk write timing for one message, in write order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteTimings {
    /// Duration of each individual `write(2)` call.
    pub per_chunk: Vec<Duration>,
    /// Wall time from just before the first write to just after the last.
    pub total: Duration,
}

/// One of the dock's two channels as the session, the daemon and the validation tools see it:
/// framed reports go out through [`write_chunks`](DockTransport::write_chunks) and come back
/// through [`read_report`](DockTransport::read_report) and [`read_message`](DockTransport::read_message).
pub trait DockTransport: Send + 'static {
    /// The channel this transport was opened for.
    fn channel(&self) -> ReportId;

    /// Writes each chunk with one write call, in order, and returns per-chunk timing.
    fn write_chunks(&mut self, chunks: &[Vec<u8>]) -> Result<WriteTimings, TransportError>;

    /// Reads one raw report without interpreting it.
    fn read_report(&mut self) -> Result<Vec<u8>, TransportError>;

    /// Reads and parses one complete message.
    fn read_message(&mut self) -> Result<ParsedMessage, TransportError>;

    /// Waits up to `timeout` for the transport to become readable.
    fn wait_readable(&self, timeout: Duration) -> Result<bool, TransportError>;

    /// Duplicates the transport, so a reader thread and a writer can each own one.
    fn try_clone(&self) -> Result<Box<dyn DockTransport>, TransportError>;
}

impl DockTransport for Box<dyn DockTransport> {
    fn channel(&self) -> ReportId {
        (**self).channel()
    }

    fn write_chunks(&mut self, chunks: &[Vec<u8>]) -> Result<WriteTimings, TransportError> {
        (**self).write_chunks(chunks)
    }

    fn read_report(&mut self) -> Result<Vec<u8>, TransportError> {
        (**self).read_report()
    }

    fn read_message(&mut self) -> Result<ParsedMessage, TransportError> {
        (**self).read_message()
    }

    fn wait_readable(&self, timeout: Duration) -> Result<bool, TransportError> {
        (**self).wait_readable(timeout)
    }

    fn try_clone(&self) -> Result<Box<dyn DockTransport>, TransportError> {
        (**self).try_clone()
    }
}
