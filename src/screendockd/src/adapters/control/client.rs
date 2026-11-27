//! The control socket client.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use super::types::{UnixControlClient, CLIENT_REPLY_TIMEOUT};
use crate::ports::control::{ControlClient, ControlError};
use crate::protocol::control::{ControlRequest, ControlResponse, MAX_RESPONSE_LINE};

impl UnixControlClient {
    /// A client for the daemon at `path`.
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            timeout: CLIENT_REPLY_TIMEOUT,
        }
    }

    /// The socket path.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl ControlClient for UnixControlClient {
    fn send(&self, request: &ControlRequest) -> Result<ControlResponse, ControlError> {
        let mut stream =
            UnixStream::connect(&self.path).map_err(|source| ControlError::Unreachable {
                path: self.path.clone(),
                source,
            })?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        stream.write_all(&request.to_line())?;
        let mut line = Vec::new();
        BufReader::new((&stream).take(MAX_RESPONSE_LINE as u64)).read_until(b'\n', &mut line)?;
        if line.is_empty() {
            return Err(ControlError::Wire(
                "the daemon closed without an answer".into(),
            ));
        }
        Ok(ControlResponse::from_line(&line)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_socket_is_unreachable() {
        let dir = tempfile::tempdir().unwrap();
        let client = UnixControlClient::new(&dir.path().join("none.sock"));
        assert!(matches!(
            client.send(&ControlRequest::Status {}),
            Err(ControlError::Unreachable { .. })
        ));
    }

    #[test]
    fn garbled_answer_is_a_protocol_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = Vec::new();
            BufReader::new(&stream)
                .read_until(b'\n', &mut line)
                .unwrap();
            stream.write_all(b"not json\n").unwrap();
        });
        assert!(matches!(
            UnixControlClient::new(&path).send(&ControlRequest::Status {}),
            Err(ControlError::Wire(_))
        ));
        server.join().unwrap();
    }
}
