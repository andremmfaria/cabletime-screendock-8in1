//! Transport port: the [`DockTransport`] trait the session, the daemon and the validation tools
//! write and read through, with its error type and write timing.
//!
//! The hidraw implementation lives in `adapters::transport::hidraw`.

use std::time::Duration;

pub use super::types::{DockTransport, EdidStore, TransportError, WriteTimings};
use crate::protocol::framing::{ParsedMessage, ReportId};

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
