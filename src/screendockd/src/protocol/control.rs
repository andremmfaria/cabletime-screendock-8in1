//! Control wire format: one JSON object per line, one request and one response per connection.
//!
//! Requests carry a `cmd` tag and responses a `result` tag. Unknown fields are rejected, and a
//! request line over [`MAX_REQUEST_LINE`] bytes is refused before it is parsed. Nothing here
//! performs I/O.

pub use super::types::{
    ControlRequest, ControlResponse, ControlWireError, MAX_REQUEST_LINE, MAX_RESPONSE_LINE,
};

fn to_line<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let mut line = serde_json::to_vec(value).expect("control messages always serialize");
    line.push(b'\n');
    line
}

fn from_line<T: serde::de::DeserializeOwned>(
    line: &[u8],
    max: usize,
) -> Result<T, ControlWireError> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    if line.len() >= max {
        return Err(ControlWireError::TooLarge { max });
    }
    serde_json::from_slice(line).map_err(|e| ControlWireError::Json(e.to_string()))
}

impl ControlRequest {
    /// The request as one newline terminated line.
    pub fn to_line(&self) -> Vec<u8> {
        to_line(self)
    }

    /// Parses one request line, with or without its trailing newline.
    pub fn from_line(line: &[u8]) -> Result<Self, ControlWireError> {
        from_line(line, MAX_REQUEST_LINE)
    }
}

impl ControlResponse {
    /// The response as one newline terminated line.
    pub fn to_line(&self) -> Vec<u8> {
        to_line(self)
    }

    /// Parses one response line, with or without its trailing newline.
    pub fn from_line(line: &[u8]) -> Result<Self, ControlWireError> {
        from_line(line, MAX_RESPONSE_LINE)
    }

    /// A response for a request the daemon could not understand or validate.
    pub fn invalid(reason: impl Into<String>) -> Self {
        Self::Invalid {
            reason: reason.into(),
        }
    }

    /// A response for a valid request the daemon declined to apply.
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self::Rejected {
            reason: reason.into(),
            rolled_back: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn requests_use_the_cmd_tag_and_end_with_a_newline() {
        let line = ControlRequest::Set {
            key: "bitrate".into(),
            value: "2000".into(),
        }
        .to_line();
        assert_eq!(
            line,
            b"{\"cmd\":\"set\",\"key\":\"bitrate\",\"value\":\"2000\"}\n".to_vec()
        );
        assert_eq!(
            ControlRequest::Status {}.to_line(),
            b"{\"cmd\":\"status\"}\n"
        );
    }

    #[test]
    fn get_without_a_key_parses() {
        assert_eq!(
            ControlRequest::from_line(b"{\"cmd\":\"get\"}").unwrap(),
            ControlRequest::Get { key: None }
        );
    }

    #[test]
    fn unknown_fields_and_commands_are_rejected() {
        for bad in [
            &b"{\"cmd\":\"status\",\"extra\":1}"[..],
            b"{\"cmd\":\"explode\"}",
            b"{\"cmd\":\"set\",\"key\":\"a\"}",
            b"not json",
            b"",
        ] {
            assert!(ControlRequest::from_line(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn request_lines_at_the_cap_are_refused() {
        let mut line = br#"{"cmd":"get","key":""#.to_vec();
        line.resize(MAX_REQUEST_LINE, b'a');
        assert_eq!(
            ControlRequest::from_line(&line),
            Err(ControlWireError::TooLarge {
                max: MAX_REQUEST_LINE
            })
        );
    }

    #[test]
    fn responses_round_trip() {
        let mut values = BTreeMap::new();
        values.insert("bitrate".to_string(), "4000".to_string());
        for response in [
            ControlResponse::Status {
                fields: values.clone(),
            },
            ControlResponse::Values { values },
            ControlResponse::Applied {
                class: "live".into(),
                changed: vec!["bitrate".into()],
            },
            ControlResponse::Rejected {
                reason: "no".into(),
                rolled_back: true,
            },
            ControlResponse::invalid("bad"),
        ] {
            assert_eq!(
                ControlResponse::from_line(&response.to_line()).unwrap(),
                response
            );
        }
    }
}
