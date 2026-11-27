//! Changes to a sway output as a watcher's source: the `output` events of the sway IPC socket
//! say that the layout changed, and the output list says what the output looks like now.

use std::io::{ErrorKind, Read};
use std::path::Path;

use super::types::{
    RectSource, SwayIpc, SwayOutputEvents, WatchTarget, IPC_MAGIC, IPC_MAX_PAYLOAD,
};
use crate::ports::display::{Rect, VdisplayError};

/// Bytes of an IPC message header: the magic, the length and the type.
const HEADER_LEN: usize = 14;

fn connection_err(e: impl std::fmt::Display) -> VdisplayError {
    VdisplayError::Connection(format!("sway ipc: {e}"))
}

impl SwayOutputEvents {
    /// Subscribes to `output` events on a new connection to the socket at `path`, and opens a
    /// second connection for reading the outputs.
    pub fn connect(path: &Path) -> Result<Self, VdisplayError> {
        let mut subscribed = SwayIpc::connect(path)?;
        subscribed.subscribe(&["output"])?;
        let events = subscribed.stream;
        events.set_nonblocking(true).map_err(connection_err)?;
        Ok(Self {
            events,
            queued: Vec::new(),
            query: SwayIpc::connect(path)?,
        })
    }

    /// Removes the complete messages from the queue and says how many there were. Lengths are
    /// compositor data and capped.
    fn drain_messages(&mut self) -> Result<usize, VdisplayError> {
        let mut count = 0;
        while self.queued.len() >= HEADER_LEN {
            if &self.queued[..6] != IPC_MAGIC {
                return Err(connection_err("event does not start with the ipc magic"));
            }
            let len =
                u32::from_ne_bytes(self.queued[6..10].try_into().expect("four bytes")) as usize;
            if len > IPC_MAX_PAYLOAD {
                return Err(connection_err(format!("event of {len} bytes is too large")));
            }
            if self.queued.len() < HEADER_LEN + len {
                break;
            }
            self.queued.drain(..HEADER_LEN + len);
            count += 1;
        }
        Ok(count)
    }
}

impl RectSource for SwayOutputEvents {
    fn changes_pending(&mut self) -> Result<bool, VdisplayError> {
        let mut buf = [0u8; 4096];
        loop {
            match self.events.read(&mut buf) {
                Ok(0) => return Err(connection_err("sway closed the event connection")),
                Ok(n) => self.queued.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(connection_err(e)),
            }
        }
        Ok(self.drain_messages()? > 0)
    }

    fn rect(&mut self, target: &WatchTarget) -> Result<Option<Rect>, VdisplayError> {
        let WatchTarget::Monitor(name) = target else {
            return Ok(None);
        };
        Ok(self
            .query
            .outputs()?
            .into_iter()
            .find(|o| &o.name == name && o.active)
            .map(|o| o.rect))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::display::sway_ipc::encode_message;
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    fn events_with(bytes: &[u8]) -> (SwayOutputEvents, UnixStream) {
        let (mut theirs, ours) = UnixStream::pair().unwrap();
        ours.set_nonblocking(true).unwrap();
        theirs.write_all(bytes).unwrap();
        let (query, _other) = UnixStream::pair().unwrap();
        (
            SwayOutputEvents {
                events: ours,
                queued: Vec::new(),
                query: SwayIpc { stream: query },
            },
            theirs,
        )
    }

    #[test]
    fn whole_events_are_counted_and_partial_ones_wait() {
        let one = encode_message(0x8000_0001, br#"{"change":"unspecified"}"#);
        let mut bytes = one.clone();
        bytes.extend_from_slice(&one[..9]);
        let (mut events, _peer) = events_with(&bytes);
        assert_eq!(events.changes_pending(), Ok(true));
        assert_eq!(events.queued.len(), 9);
        assert_eq!(events.changes_pending(), Ok(false));
    }

    #[test]
    fn silence_is_not_a_change() {
        let (mut events, _peer) = events_with(b"");
        assert_eq!(events.changes_pending(), Ok(false));
    }

    #[test]
    fn a_stream_that_is_not_ipc_or_claims_a_huge_event_is_refused() {
        let (mut events, _peer) = events_with(b"not an ipc message at all");
        assert!(events.changes_pending().is_err());
        let mut huge = IPC_MAGIC.to_vec();
        huge.extend_from_slice(&u32::MAX.to_ne_bytes());
        huge.extend_from_slice(&0x8000_0001u32.to_ne_bytes());
        let (mut events, _peer) = events_with(&huge);
        assert!(events.changes_pending().is_err());
    }

    #[test]
    fn a_closed_connection_is_an_error() {
        let (theirs, ours) = UnixStream::pair().unwrap();
        ours.set_nonblocking(true).unwrap();
        drop(theirs);
        let (query, _other) = UnixStream::pair().unwrap();
        let mut events = SwayOutputEvents {
            events: ours,
            queued: Vec::new(),
            query: SwayIpc { stream: query },
        };
        assert!(events.changes_pending().is_err());
    }
}
