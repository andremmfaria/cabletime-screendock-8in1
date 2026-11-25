//! The control wire format as an outside consumer sees it: exact lines, strictness and caps.

use screendockd::protocol::control::{
    ControlRequest, ControlResponse, MAX_REQUEST_LINE, MAX_RESPONSE_LINE,
};

#[test]
fn every_request_round_trips_through_its_line() {
    for request in [
        ControlRequest::Status {},
        ControlRequest::Get { key: None },
        ControlRequest::Get {
            key: Some("bitrate".into()),
        },
        ControlRequest::Set {
            key: "resolution".into(),
            value: "1600x900".into(),
        },
        ControlRequest::Reload {},
    ] {
        let line = request.to_line();
        assert_eq!(line.last(), Some(&b'\n'));
        assert_eq!(line.iter().filter(|b| **b == b'\n').count(), 1);
        assert_eq!(ControlRequest::from_line(&line).unwrap(), request);
    }
}

#[test]
fn request_lines_hold_exactly_the_documented_fields() {
    assert_eq!(
        ControlRequest::Reload {}.to_line(),
        b"{\"cmd\":\"reload\"}\n".to_vec()
    );
    assert_eq!(
        ControlRequest::Get {
            key: Some("fps".into())
        }
        .to_line(),
        b"{\"cmd\":\"get\",\"key\":\"fps\"}\n".to_vec()
    );
}

#[test]
fn strange_requests_are_errors_not_panics() {
    for bad in [
        &b"{}"[..],
        b"[]",
        b"null",
        b"{\"cmd\":\"set\",\"key\":1,\"value\":2}",
        b"{\"cmd\":\"status\",\"cmd\":\"reload\"}",
        b"\xff\xfe",
        b"{\"cmd\":\"get\",\"key\":\"\\ud800\"}",
    ] {
        assert!(ControlRequest::from_line(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn line_caps_apply_to_both_directions() {
    let long_key = "k".repeat(MAX_REQUEST_LINE);
    let line = ControlRequest::Get {
        key: Some(long_key),
    }
    .to_line();
    assert!(ControlRequest::from_line(&line).is_err());

    let reason = "r".repeat(MAX_RESPONSE_LINE);
    let line = ControlResponse::invalid(reason).to_line();
    assert!(ControlResponse::from_line(&line).is_err());
}

#[test]
fn response_lines_carry_the_result_tag() {
    let line = ControlResponse::Rejected {
        reason: "encoder refused".into(),
        rolled_back: true,
    }
    .to_line();
    assert_eq!(
        String::from_utf8(line).unwrap(),
        "{\"result\":\"rejected\",\"reason\":\"encoder refused\",\"rolled_back\":true}\n"
    );
}
