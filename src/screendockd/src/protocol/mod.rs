//! Wire-level content: HID framing, Annex-B splitting, display geometry, the heartbeat and host
//! messages, and parsing of the dock's messages. Nothing here performs I/O.

pub mod annexb;
pub mod control;
pub mod display;
pub mod framing;
pub mod heartbeat;
pub mod inbound;
pub mod messages;
pub mod types;
