//! Parsing of the dock's MI_00 traffic: report reassembly and JSON-RPC classification.
//!
//! Dock text is untrusted data: it is parsed as JSON, never executed.

use serde_json::Value;

use super::framing::{FIRST_CHUNK_PAYLOAD_MAX, HEADER_SIZE};
pub use super::types::{DockMessage, InboundAssembler, LOG_TEXT_MAX};
use super::types::{MAX_INBOUND_LEN, MAX_PASSTHROUGH_DATA};

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
            "common.data_passthrough" => {
                let params = object.get("params");
                DockMessage::Passthrough {
                    id: params.and_then(|p| p.get("id")).and_then(Value::as_u64),
                    data: params
                        .and_then(|p| p.get("data"))
                        .and_then(Value::as_str)
                        .filter(|text| text.len() <= MAX_PASSTHROUGH_DATA)
                        .and_then(decode_base64),
                }
            }
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

/// Decodes standard base64 with optional padding. Any other character fails the whole text.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
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
            DockMessage::Passthrough {
                id: Some(101),
                data: Some(vec![0])
            }
        );
        assert_eq!(
            parse_dock_message(
                br#"{"method":"common.data_passthrough","params":{"id":103,"data":"AQ=="}}"#
            ),
            DockMessage::Passthrough {
                id: Some(103),
                data: Some(vec![1])
            }
        );
        assert_eq!(
            parse_dock_message(
                br#"{"method":"common.data_passthrough","params":{"id":101,"data":"not base64!"}}"#
            ),
            DockMessage::Passthrough {
                id: Some(101),
                data: None
            }
        );
        assert_eq!(
            parse_dock_message(br#"{"method":"common.data_passthrough"}"#),
            DockMessage::Passthrough {
                id: None,
                data: None
            }
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
