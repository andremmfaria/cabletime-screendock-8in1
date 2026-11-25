//! JSON-RPC heartbeat message.
//!
//! The heartbeat is the exact text the vendor driver sends, including its spacing:
//! `{"jsonrpc":"2.0", "method": "heartbeat", "params": {"session-id": "<digits>"}}`, with no
//! trailing newline. The session id is a digit string chosen once per run and kept constant,
//! since a changed id makes the dock re-announce itself.

use thiserror::Error;

use super::framing::MessageBuilder;

/// The JSON-RPC method name of the heartbeat.
pub const HEARTBEAT_METHOD: &str = "heartbeat";

/// Longest accepted session id, in digits.
const SESSION_ID_MAX_LEN: usize = 20;

/// Errors from constructing a [`SessionId`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionIdError {
    /// The id was empty or longer than the accepted maximum.
    #[error("session id must be 1 to {SESSION_ID_MAX_LEN} characters, got {0}")]
    BadLength(usize),
    /// The id contained a character that is not an ASCII digit.
    #[error("session id must contain only ASCII digits")]
    NotDigits,
}

/// The heartbeat `session-id`: a non-empty string of ASCII digits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(String);

impl SessionId {
    /// Validates a session id string.
    pub fn new(id: impl Into<String>) -> Result<Self, SessionIdError> {
        let id = id.into();
        if id.is_empty() || id.len() > SESSION_ID_MAX_LEN {
            return Err(SessionIdError::BadLength(id.len()));
        }
        if !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(SessionIdError::NotDigits);
        }
        Ok(Self(id))
    }

    /// A fresh id from the wall clock in milliseconds, reduced to eight digits like the vendor
    /// driver's tick-count ids.
    pub fn generate() -> Self {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_millis());
        Self(((millis % 100_000_000) + 10_000_000).to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Builds the exact heartbeat text for a session id.
pub fn heartbeat_payload(session_id: &SessionId) -> Vec<u8> {
    format!(
        r#"{{"jsonrpc":"2.0", "method": "{HEARTBEAT_METHOD}", "params": {{"session-id": "{}"}}}}"#,
        session_id.as_str()
    )
    .into_bytes()
}

/// Builds the wire-ready report sequence (Report ID 1, zero width and height) for a heartbeat.
pub fn heartbeat_message(session_id: &SessionId) -> Vec<Vec<u8>> {
    MessageBuilder::control()
        .payload(&heartbeat_payload(session_id))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::framing::{parse_chunks, PictureSize, ReportId, REPORT_SIZE};

    fn id(s: &str) -> SessionId {
        SessionId::new(s).unwrap()
    }

    #[test]
    fn payload_is_the_exact_vendor_text() {
        assert_eq!(
            heartbeat_payload(&id("25282281")),
            br#"{"jsonrpc":"2.0", "method": "heartbeat", "params": {"session-id": "25282281"}}"#
        );
        // The length field is 78 for an eight digit id, as captured.
        assert_eq!(heartbeat_payload(&id("25282281")).len(), 78);
    }

    #[test]
    fn session_id_validation() {
        assert!(SessionId::new("").is_err());
        assert_eq!(SessionId::new("12a"), Err(SessionIdError::NotDigits));
        assert!(SessionId::new("1".repeat(21)).is_err());
        let generated = SessionId::generate();
        assert!(SessionId::new(generated.as_str()).is_ok());
    }

    #[test]
    fn message_is_single_control_chunk() {
        let chunks = heartbeat_message(&id("7"));
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), REPORT_SIZE);
        let parsed = parse_chunks(&chunks).unwrap();
        assert_eq!(parsed.report_id, ReportId::Control);
        assert_eq!(parsed.size, PictureSize::zero());
        assert_eq!(parsed.payload, heartbeat_payload(&id("7")));
        let value: serde_json::Value = serde_json::from_slice(&parsed.payload).unwrap();
        assert_eq!(value["method"], "heartbeat");
        assert_eq!(value["params"]["session-id"], "7");
    }
}
