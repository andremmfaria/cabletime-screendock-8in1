//! Parsing of the dock's MI_00 traffic: report reassembly and JSON-RPC classification.
//!
//! Dock text is untrusted data: it is parsed as JSON, never executed.

use serde_json::Value;

use super::framing::{FIRST_CHUNK_PAYLOAD_MAX, HEADER_SIZE};

/// Largest dock message the reader reassembles; anything bigger is dropped as malformed.
const MAX_INBOUND_LEN: usize = 64 * 1024;

/// Longest dock-supplied string (method names) written to the log.
pub const LOG_TEXT_MAX: usize = 64;

/// A parsed message from the dock.
#[derive(Debug, Clone, PartialEq)]
pub enum DockMessage {
    /// `notify-app-accessories-are-ready`.
    Ready,
    /// The dock's heartbeat.
    Heartbeat,
    /// `common.data_passthrough` with the given params id.
    Passthrough { id: Option<u64> },
    /// A result or error for one of our requests.
    Reply {
        id: u64,
        result: Option<Value>,
        error: Option<Value>,
    },
    /// Any other notification (`tx-data-updated`, ...), with its truncated method name.
    Other { method: String },
    /// Text that isn't a JSON-RPC object this driver understands.
    Unrecognized,
}

/// Parses the JSON text of one dock message.
pub fn parse_dock_message(text: &[u8]) -> DockMessage {
    let Ok(value) = serde_json::from_slice::<Value>(text) else {
        return DockMessage::Unrecognized;
    };
    let Some(object) = value.as_object() else {
        return DockMessage::Unrecognized;
    };
    if let Some(method) = object.get("method").and_then(Value::as_str) {
        return match method {
            "notify-app-accessories-are-ready" => DockMessage::Ready,
            "heartbeat" => DockMessage::Heartbeat,
            "common.data_passthrough" => DockMessage::Passthrough {
                id: object
                    .get("params")
                    .and_then(|p| p.get("id"))
                    .and_then(Value::as_u64),
            },
            other => DockMessage::Other {
                method: other.chars().take(LOG_TEXT_MAX).collect(),
            },
        };
    }
    if let Some(id) = object.get("id").and_then(Value::as_u64) {
        let result = object.get("result").cloned();
        let error = object.get("error").cloned();
        if result.is_some() || error.is_some() {
            return DockMessage::Reply { id, result, error };
        }
    }
    DockMessage::Unrecognized
}

/// Reassembles the dock's MI_00 reports into message texts.
///
/// Reports use the same layout the host writes: a 16-byte header with the big-endian payload
/// length at bytes 1..4 on the first report, then payload from offset 16; continuation reports
/// repeat the header with a zero length. The reader is lenient about the other header bytes,
/// since the input is the dock's, but never trusts the length beyond [`MAX_INBOUND_LEN`].
#[derive(Debug, Default)]
pub struct InboundAssembler {
    expected: usize,
    buf: Vec<u8>,
}

impl InboundAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one report. Returns the complete message text when this report finishes one.
    pub fn feed(&mut self, report: &[u8]) -> Option<Vec<u8>> {
        if report.len() < HEADER_SIZE {
            return None;
        }
        let announced = u32::from_be_bytes(report[1..5].try_into().ok()?) as usize;
        let body = &report[HEADER_SIZE..];
        if announced != 0 {
            // A non-zero length always starts a new message, dropping any torn one.
            if announced > MAX_INBOUND_LEN {
                self.reset();
                return None;
            }
            self.expected = announced;
            self.buf.clear();
        } else if self.expected == 0 {
            return None;
        }
        let want = self.expected - self.buf.len();
        let take = want.min(body.len()).min(FIRST_CHUNK_PAYLOAD_MAX);
        self.buf.extend_from_slice(&body[..take]);
        if self.buf.len() >= self.expected {
            let message = std::mem::take(&mut self.buf);
            self.expected = 0;
            return Some(message);
        }
        None
    }

    fn reset(&mut self) {
        self.expected = 0;
        self.buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::framing::{frame_message, PictureSize, ReportId};

    #[test]
    fn assembler_handles_single_and_multi_report_messages() {
        let mut assembler = InboundAssembler::new();
        let single = frame_message(ReportId::Control, PictureSize::zero(), b"{\"a\":1}");
        assert_eq!(assembler.feed(&single[0]).unwrap(), b"{\"a\":1}");

        let big: Vec<u8> = (0..9000).map(|i| b'a' + (i % 26) as u8).collect();
        let chunks = frame_message(ReportId::Control, PictureSize::zero(), &big);
        assert_eq!(chunks.len(), 3);
        assert!(assembler.feed(&chunks[0]).is_none());
        assert!(assembler.feed(&chunks[1]).is_none());
        assert_eq!(assembler.feed(&chunks[2]).unwrap(), big);

        // A stray continuation and an oversized announcement are dropped.
        assert!(assembler.feed(&chunks[1]).is_none());
        let mut huge = vec![0u8; 4096];
        huge[0] = 1;
        huge[1..5].copy_from_slice(&(10_000_000u32).to_be_bytes());
        assert!(assembler.feed(&huge).is_none());
        assert!(assembler.feed(&[1, 2, 3]).is_none());
    }

    #[test]
    fn dock_messages_parse_by_kind() {
        let ready = b"{\n\t\"jsonrpc\":\t\"2.0\",\n\t\"method\":\t\"notify-app-accessories-are-ready\",\n\t\"params\":\t{\n\t\t\"res-width\":\t1920\n\t},\n\t\"jrpcSeq\":\t7730\n}";
        assert_eq!(parse_dock_message(ready), DockMessage::Ready);
        assert_eq!(
            parse_dock_message(br#"{"method":"heartbeat","params":{"session-id":"1"}}"#),
            DockMessage::Heartbeat
        );
        assert_eq!(
            parse_dock_message(br#"{"method":"common.data_passthrough","params":{"id":101,"size":258,"data":"AA=="}}"#),
            DockMessage::Passthrough { id: Some(101) }
        );
        assert_eq!(
            parse_dock_message(br#"{"id":4,"result":"isAllowed","jrpcSeq":7743}"#),
            DockMessage::Reply {
                id: 4,
                result: Some(Value::String("isAllowed".into())),
                error: None
            }
        );
        assert!(matches!(
            parse_dock_message(br#"{"id":4,"error":{"code":-32601}}"#),
            DockMessage::Reply { error: Some(_), .. }
        ));
        assert_eq!(
            parse_dock_message(br#"{"method":"tx-data-updated"}"#),
            DockMessage::Other {
                method: "tx-data-updated".into()
            }
        );
        assert_eq!(parse_dock_message(b"not json"), DockMessage::Unrecognized);
        assert_eq!(parse_dock_message(b"[1,2]"), DockMessage::Unrecognized);
    }
}
