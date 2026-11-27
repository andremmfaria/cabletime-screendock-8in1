//! A small client for the sway IPC socket: run a command, list the outputs.
//!
//! Messages are `i3-ipc`, the payload length and the message type (both 32 bit, native byte
//! order) and then the payload. Everything that comes back is compositor data: lengths are
//! capped, JSON is parsed into typed fields, and output names are checked before they are
//! put into a command.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use serde::Deserialize;

use super::types::{
    SwayIpc, SwayOutput, IPC_GET_OUTPUTS, IPC_MAGIC, IPC_MAX_PAYLOAD, IPC_RUN_COMMAND,
    IPC_SUBSCRIBE, IPC_TIMEOUT,
};
use crate::ports::display::{OutputFacts, OutputId, Rect, VdisplayError};

fn connection_err(e: impl std::fmt::Display) -> VdisplayError {
    VdisplayError::Connection(format!("sway ipc: {e}"))
}

fn request_err(e: impl std::fmt::Display) -> VdisplayError {
    VdisplayError::Request(format!("sway ipc: {e}"))
}

/// Whether `name` is safe to put into a command: printable characters that sway output names
/// are made of, at most 64 of them.
pub fn is_valid_output_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The bytes of one IPC message.
pub fn encode_message(kind: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(14 + payload.len());
    out.extend_from_slice(IPC_MAGIC);
    out.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(payload);
    out
}

#[derive(Deserialize)]
struct RawRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Deserialize)]
struct RawOutput {
    name: String,
    #[serde(default)]
    active: bool,
    rect: RawRect,
}

#[derive(Deserialize)]
struct RawResult {
    success: bool,
    #[serde(default)]
    error: Option<String>,
}

impl SwayIpc {
    /// Connects to the socket at `path`.
    pub fn connect(path: &Path) -> Result<Self, VdisplayError> {
        let stream = UnixStream::connect(path).map_err(connection_err)?;
        stream
            .set_read_timeout(Some(IPC_TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(IPC_TIMEOUT)))
            .map_err(connection_err)?;
        Ok(Self { stream })
    }

    fn call(&mut self, kind: u32, payload: &[u8]) -> Result<Vec<u8>, VdisplayError> {
        self.stream
            .write_all(&encode_message(kind, payload))
            .map_err(request_err)?;
        let mut header = [0u8; 14];
        self.stream.read_exact(&mut header).map_err(request_err)?;
        if &header[..6] != IPC_MAGIC {
            return Err(request_err("reply does not start with the ipc magic"));
        }
        let len = u32::from_ne_bytes(header[6..10].try_into().expect("four bytes")) as usize;
        let reply_kind = u32::from_ne_bytes(header[10..14].try_into().expect("four bytes"));
        if len > IPC_MAX_PAYLOAD {
            return Err(request_err(format!("reply of {len} bytes is too large")));
        }
        if reply_kind != kind {
            return Err(request_err(format!(
                "reply of type {reply_kind} to a request of type {kind}"
            )));
        }
        let mut payload = vec![0u8; len];
        self.stream.read_exact(&mut payload).map_err(request_err)?;
        Ok(payload)
    }

    /// Runs one sway command and fails with sway's message when it does not succeed.
    pub fn run(&mut self, command: &str) -> Result<(), VdisplayError> {
        let reply = self.call(IPC_RUN_COMMAND, command.as_bytes())?;
        let results: Vec<RawResult> = serde_json::from_slice(&reply).map_err(request_err)?;
        match results.iter().find(|r| !r.success) {
            Some(failed) => Err(request_err(format!(
                "`{command}` failed: {}",
                failed.error.as_deref().unwrap_or("no reason given")
            ))),
            None => Ok(()),
        }
    }

    /// Asks sway to send the named events (for example `output`) on this connection. After
    /// this the connection carries events, not replies.
    pub fn subscribe(&mut self, events: &[&str]) -> Result<(), VdisplayError> {
        let payload = serde_json::to_vec(events).map_err(request_err)?;
        let reply = self.call(IPC_SUBSCRIBE, &payload)?;
        let result: RawResult = serde_json::from_slice(&reply).map_err(request_err)?;
        if result.success {
            Ok(())
        } else {
            Err(request_err("sway refused the subscription"))
        }
    }

    /// The outputs sway knows, active or not.
    pub fn outputs(&mut self) -> Result<Vec<SwayOutput>, VdisplayError> {
        let reply = self.call(IPC_GET_OUTPUTS, b"")?;
        parse_outputs(&reply)
    }
}

/// Parses a `GET_OUTPUTS` reply.
pub fn parse_outputs(json: &[u8]) -> Result<Vec<SwayOutput>, VdisplayError> {
    let raw: Vec<RawOutput> = serde_json::from_slice(json).map_err(request_err)?;
    Ok(raw
        .into_iter()
        .map(|o| SwayOutput {
            name: o.name,
            active: o.active,
            rect: Rect::new(o.rect.x, o.rect.y, o.rect.width, o.rect.height),
        })
        .collect())
}

/// The active outputs of the sway at `socket` as display facts. The first is the primary.
pub fn sway_output_facts(socket: &Path) -> Result<Vec<OutputFacts>, VdisplayError> {
    let outputs = SwayIpc::connect(socket)?.outputs()?;
    Ok(outputs
        .into_iter()
        .filter(|o| o.active)
        .enumerate()
        .map(|(index, o)| OutputFacts {
            id: OutputId(index as u32),
            name: o.name,
            rect: o.rect,
            primary: index == 0,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// Answers each request with `reply` once, then returns what the client sent.
    fn serve(
        dir: &Path,
        kind: u32,
        reply: Vec<u8>,
    ) -> (std::path::PathBuf, std::thread::JoinHandle<Vec<u8>>) {
        let path = dir.join("sway.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut header = [0u8; 14];
            conn.read_exact(&mut header).unwrap();
            let len = u32::from_ne_bytes(header[6..10].try_into().unwrap()) as usize;
            let mut payload = vec![0u8; len];
            conn.read_exact(&mut payload).unwrap();
            let _ = conn.write_all(&encode_message(kind, &reply));
            payload
        });
        (path, handle)
    }

    #[test]
    fn messages_carry_magic_length_and_type() {
        let bytes = encode_message(3, b"abc");
        assert_eq!(&bytes[..6], b"i3-ipc");
        assert_eq!(u32::from_ne_bytes(bytes[6..10].try_into().unwrap()), 3);
        assert_eq!(u32::from_ne_bytes(bytes[10..14].try_into().unwrap()), 3);
        assert_eq!(&bytes[14..], b"abc");
    }

    #[test]
    fn output_names_are_limited_to_safe_characters() {
        assert!(is_valid_output_name("HEADLESS-2"));
        assert!(is_valid_output_name("DP-1"));
        for bad in ["", "a b", "a;b", "x\ny", "../x", &"n".repeat(65), "naïve"] {
            assert!(!is_valid_output_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn outputs_reply_is_parsed_into_typed_outputs() {
        let json = br#"[{"name":"HEADLESS-1","active":true,"rect":{"x":0,"y":0,"width":1280,"height":720},"modes":[]},
            {"name":"HEADLESS-2","active":false,"rect":{"x":5,"y":-6,"width":1,"height":2}}]"#;
        let outputs = parse_outputs(json).unwrap();
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].rect, Rect::new(0, 0, 1280, 720));
        assert!(outputs[0].active && !outputs[1].active);
        assert_eq!(outputs[1].rect, Rect::new(5, -6, 1, 2));
        assert!(parse_outputs(b"{}").is_err());
        assert!(parse_outputs(b"[{\"name\":1}]").is_err());
    }

    #[test]
    fn run_sends_the_command_and_reports_the_compositor_error() {
        let dir = tempfile::tempdir().unwrap();
        let (path, sent) = serve(dir.path(), 0, br#"[{"success":true}]"#.to_vec());
        SwayIpc::connect(&path)
            .unwrap()
            .run("create_output")
            .unwrap();
        assert_eq!(sent.join().unwrap(), b"create_output");

        let dir = tempfile::tempdir().unwrap();
        let (path, _) = serve(
            dir.path(),
            0,
            br#"[{"success":false,"error":"Unknown output"}]"#.to_vec(),
        );
        let err = SwayIpc::connect(&path)
            .unwrap()
            .run("output X")
            .unwrap_err();
        assert!(err.to_string().contains("Unknown output"), "{err}");
    }

    #[test]
    fn oversized_and_mistyped_replies_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let big = vec![b' '; IPC_MAX_PAYLOAD + 1];
        let (path, _) = serve(dir.path(), 3, big);
        assert!(SwayIpc::connect(&path).unwrap().outputs().is_err());

        let dir = tempfile::tempdir().unwrap();
        let (path, _) = serve(dir.path(), 7, b"[]".to_vec());
        assert!(SwayIpc::connect(&path).unwrap().outputs().is_err());
    }

    #[test]
    fn connecting_to_a_missing_socket_is_a_connection_error() {
        let err = SwayIpc::connect(Path::new("/nonexistent/sway.sock")).unwrap_err();
        assert!(matches!(err, VdisplayError::Connection(_)));
    }
}
