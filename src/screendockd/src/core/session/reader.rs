use std::sync::mpsc::Sender;

use tracing::{debug, warn};

use super::events::DockEvent;
use super::{send_control, SessionState, Shared, SharedWriter};
use crate::core::policy::POLL_SLICE;
use crate::ports::transport::DockTransport;
use crate::protocol::inbound::{parse_dock_message, DockMessage, InboundAssembler};
use crate::protocol::messages;

/// MI_00 reader: always drains the dock's IN reports so the dock's writer never blocks.
pub(super) fn reader_thread(
    mut handle: Box<dyn DockTransport>,
    writer: &SharedWriter,
    events: &Sender<DockEvent>,
    shared: &Shared,
) {
    let mut assembler = InboundAssembler::new();
    while !shared.stopped() {
        match handle.wait_readable(POLL_SLICE) {
            Ok(false) => continue,
            Ok(true) => {}
            Err(e) => {
                shared.set_fatal(format!("poll on MI_00 failed: {e}"));
                shared.set_state(SessionState::Stopped);
                return;
            }
        }
        let report = match handle.read_report() {
            Ok(report) => report,
            Err(e) => {
                if !shared.stopped() {
                    shared.set_fatal(format!("read from MI_00 failed: {e}"));
                    shared.set_state(SessionState::Stopped);
                }
                return;
            }
        };
        let Some(text) = assembler.feed(&report) else {
            continue;
        };
        match parse_dock_message(&text) {
            DockMessage::Heartbeat => shared.note_dock_heartbeat(),
            DockMessage::Ready => {
                let _ = events.send(DockEvent::Ready);
            }
            DockMessage::Passthrough { id } => {
                if id == Some(messages::EDID_PASSTHROUGH_ID) {
                    if let Err(e) = send_control(writer, messages::EDID_ACK) {
                        warn!(error = %e, "couldn't answer the dock's EDID passthrough");
                    }
                }
            }
            DockMessage::Reply { id, result, error } => {
                let _ = events.send(DockEvent::Reply { id, result, error });
            }
            DockMessage::Other { method } => debug!(%method, "dock notification"),
            DockMessage::Unrecognized => debug!(len = text.len(), "unrecognized dock message"),
        }
    }
}
