//! The `ctl` subcommand: one request to a running daemon's control socket.
//!
//! Exit codes: 0 the request succeeded, 2 the daemon rejected it, 3 no daemon answers on the
//! socket, 4 the request was invalid.

use std::process::ExitCode;

use anyhow::{bail, Result};

use crate::adapters::control::{socket_path, UnixControlClient};
use crate::ports::control::{ControlClient, ControlError};
use crate::protocol::control::{ControlRequest, ControlResponse};
use crate::utils::config::Config;

use super::types::{CtlAction, CtlArgs};

/// Exit code for a daemon that rejected a valid request.
const EXIT_REJECTED: u8 = 2;

/// Exit code for no daemon on the socket.
const EXIT_UNREACHABLE: u8 = 3;

/// Exit code for an invalid request.
const EXIT_INVALID: u8 = 4;

fn request_for(action: &CtlAction) -> ControlRequest {
    match action {
        CtlAction::Status => ControlRequest::Status {},
        CtlAction::Get { key } => ControlRequest::Get { key: key.clone() },
    }
}

/// The exit code a response maps to.
fn exit_code(response: &ControlResponse) -> u8 {
    match response {
        ControlResponse::Status { .. }
        | ControlResponse::Values { .. }
        | ControlResponse::Applied { .. } => 0,
        ControlResponse::Rejected { .. } => EXIT_REJECTED,
        ControlResponse::Invalid { .. } => EXIT_INVALID,
    }
}

/// The text a response prints: `(stdout, stderr)`. A `get` of one key prints just the value.
fn render(request: &ControlRequest, response: &ControlResponse) -> (String, String) {
    let lines = |map: &std::collections::BTreeMap<String, String>| {
        map.iter()
            .map(|(k, v)| format!("{k}: {v}\n"))
            .collect::<String>()
    };
    match response {
        ControlResponse::Status { fields } => (lines(fields), String::new()),
        ControlResponse::Values { values } => match request {
            ControlRequest::Get { key: Some(_) } if values.len() == 1 => {
                let value = values.values().next().expect("one value");
                (format!("{value}\n"), String::new())
            }
            _ => (lines(values), String::new()),
        },
        ControlResponse::Applied { class, changed } => {
            let what = if changed.is_empty() {
                "nothing to change".to_string()
            } else {
                changed.join(", ")
            };
            (format!("applied ({class}): {what}\n"), String::new())
        }
        ControlResponse::Rejected {
            reason,
            rolled_back,
        } => {
            let note = if *rolled_back { " (rolled back)" } else { "" };
            (String::new(), format!("rejected{note}: {reason}\n"))
        }
        ControlResponse::Invalid { reason } => (String::new(), format!("invalid: {reason}\n")),
    }
}

pub(super) fn run(args: CtlArgs, config: &Config) -> Result<ExitCode> {
    let Some(path) = args.socket.or_else(|| socket_path(&config.control)) else {
        bail!("no control socket path: set control.socket, pass --socket, or set $XDG_RUNTIME_DIR");
    };
    let request = request_for(&args.action);
    let response = match UnixControlClient::new(&path).send(&request) {
        Ok(response) => response,
        Err(e @ ControlError::Unreachable { .. }) => {
            eprintln!("{e}");
            return Ok(ExitCode::from(EXIT_UNREACHABLE));
        }
        Err(e) => return Err(e.into()),
    };
    let (out, err) = render(&request, &response);
    print!("{out}");
    eprint!("{err}");
    Ok(ExitCode::from(exit_code(&response)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn one(key: &str, value: &str) -> BTreeMap<String, String> {
        BTreeMap::from([(key.to_string(), value.to_string())])
    }

    #[test]
    fn response_kinds_map_to_the_documented_exit_codes() {
        assert_eq!(
            exit_code(&ControlResponse::Status {
                fields: one("a", "b")
            }),
            0
        );
        assert_eq!(
            exit_code(&ControlResponse::Applied {
                class: "live".into(),
                changed: vec![]
            }),
            0
        );
        assert_eq!(exit_code(&ControlResponse::rejected("no")), 2);
        assert_eq!(exit_code(&ControlResponse::invalid("bad")), 4);
    }

    #[test]
    fn get_of_one_key_prints_the_bare_value() {
        let request = ControlRequest::Get {
            key: Some("bitrate".into()),
        };
        let response = ControlResponse::Values {
            values: one("bitrate", "4000"),
        };
        assert_eq!(render(&request, &response).0, "4000\n");
        let all = ControlRequest::Get { key: None };
        assert_eq!(render(&all, &response).0, "bitrate: 4000\n");
    }

    #[test]
    fn rejections_and_invalid_requests_print_to_stderr() {
        let request = ControlRequest::Status {};
        let (out, err) = render(
            &request,
            &ControlResponse::Rejected {
                reason: "busy".into(),
                rolled_back: true,
            },
        );
        assert!(out.is_empty());
        assert_eq!(err, "rejected (rolled back): busy\n");
        let (out, err) = render(&request, &ControlResponse::invalid("unknown setting"));
        assert!(out.is_empty());
        assert_eq!(err, "invalid: unknown setting\n");
    }
}
