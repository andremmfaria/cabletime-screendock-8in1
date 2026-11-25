//! The control socket server.

use std::fs::DirBuilder;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;

use tracing::{debug, info, warn};

use super::types::{
    ControlServer, ACCEPT_POLL, CLIENT_IO_TIMEOUT, DRAIN_LIMIT, DRAIN_TIMEOUT, MAX_SOCKET_PATH,
    REPLY_TIMEOUT,
};
use crate::ports::control::{Command, ControlError};
use crate::protocol::control::{ControlRequest, ControlResponse, MAX_REQUEST_LINE};

impl ControlServer {
    /// Binds the socket at `path` and serves it on a background thread. Every valid request
    /// becomes a [`Command::Request`] on `commands`.
    ///
    /// A missing parent directory is created with mode 0700. A socket file left by a dead
    /// daemon is replaced; one that still answers is an error. The socket file gets mode 0600.
    pub fn start(path: &Path, commands: Sender<Command>) -> Result<Self, ControlError> {
        if path.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(ControlError::PathTooLong(path.to_path_buf()));
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        remove_stale_socket(path)?;
        let listener = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        info!(path = %path.display(), "control socket listening");

        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("control-accept".into())
                .spawn(move || accept_loop(&listener, &commands, &stop))?
        };
        Ok(Self {
            path: path.to_path_buf(),
            stop,
            thread: Some(thread),
        })
    }

    /// The socket path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn remove_stale_socket(path: &Path) -> Result<(), ControlError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if !metadata.file_type().is_socket() {
        return Err(ControlError::NotASocket(path.to_path_buf()));
    }
    if UnixStream::connect(path).is_ok() {
        return Err(ControlError::AlreadyRunning(path.to_path_buf()));
    }
    std::fs::remove_file(path)?;
    Ok(())
}

fn accept_loop(listener: &UnixListener, commands: &Sender<Command>, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let commands = commands.clone();
                let spawned = std::thread::Builder::new()
                    .name("control-client".into())
                    .spawn(move || serve_client(stream, &commands));
                if let Err(e) = spawned {
                    warn!(error = %e, "could not start a control client thread");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(ACCEPT_POLL);
            }
            Err(e) => {
                warn!(error = %e, "control accept failed");
                std::thread::sleep(ACCEPT_POLL);
            }
        }
    }
}

/// Reads one line of at most [`MAX_REQUEST_LINE`] bytes. `Ok(None)` means the client closed
/// without sending anything; a line that never ends within the cap is returned as is, without
/// its newline, so the parser refuses it.
fn read_request_line(stream: &UnixStream) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    let mut reader = BufReader::new(stream.take(MAX_REQUEST_LINE as u64));
    reader.read_until(b'\n', &mut line)?;
    Ok((!line.is_empty()).then_some(line))
}

fn serve_client(mut stream: UnixStream, commands: &Sender<Command>) {
    let configured = stream.set_nonblocking(false).is_ok()
        && stream.set_read_timeout(Some(CLIENT_IO_TIMEOUT)).is_ok()
        && stream.set_write_timeout(Some(CLIENT_IO_TIMEOUT)).is_ok();
    if !configured {
        return;
    }
    let line = match read_request_line(&stream) {
        Ok(Some(line)) => line,
        Ok(None) => return,
        Err(e) => {
            debug!(error = %e, "control client sent no request");
            return;
        }
    };
    let response = match ControlRequest::from_line(&line) {
        Ok(request) => ask_daemon(commands, request),
        Err(e) => ControlResponse::invalid(e.to_string()),
    };
    if let Err(e) = stream.write_all(&response.to_line()) {
        debug!(error = %e, "control client went away before the answer");
        return;
    }
    // Closing with unread request bytes would reset the connection and could discard the
    // answer, so finish the write side and drain what the client still sends.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = stream.set_read_timeout(Some(DRAIN_TIMEOUT));
    let _ = std::io::copy(&mut (&stream).take(DRAIN_LIMIT), &mut std::io::sink());
}

fn ask_daemon(commands: &Sender<Command>, request: ControlRequest) -> ControlResponse {
    let (reply, answer) = mpsc::channel();
    if commands.send(Command::Request { request, reply }).is_err() {
        return ControlResponse::rejected("the daemon is shutting down");
    }
    answer
        .recv_timeout(REPLY_TIMEOUT)
        .unwrap_or_else(|_: mpsc::RecvTimeoutError| {
            ControlResponse::rejected("the daemon did not answer in time")
        })
}

#[cfg(test)]
mod tests {
    const TEST_WAIT: Duration = Duration::from_secs(5);

    use super::*;
    use crate::adapters::control::UnixControlClient;
    use crate::ports::control::ControlClient;
    use std::collections::BTreeMap;
    use std::os::unix::net::UnixListener as StdListener;
    use std::time::Duration;

    /// Answers every request with a status carrying the request's debug text, until the
    /// channel closes.
    fn answering_daemon() -> (Sender<Command>, std::thread::JoinHandle<usize>) {
        let (tx, rx) = mpsc::channel::<Command>();
        let handle = std::thread::spawn(move || {
            let mut served = 0;
            while let Ok(command) = rx.recv_timeout(TEST_WAIT) {
                if let Command::Request { request, reply } = command {
                    served += 1;
                    let mut fields = BTreeMap::new();
                    fields.insert("request".to_string(), format!("{request:?}"));
                    let _ = reply.send(ControlResponse::Status { fields });
                }
            }
            served
        });
        (tx, handle)
    }

    fn socket_in(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().join("run").join("control.sock")
    }

    #[test]
    fn socket_is_private_and_removed_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let (tx, daemon) = answering_daemon();
        let server = ControlServer::start(&path, tx).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let parent_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(parent_mode & 0o777, 0o700);
        drop(server);
        assert!(!path.exists());
        assert_eq!(daemon.join().unwrap(), 0);
    }

    #[test]
    fn client_gets_the_daemons_answer() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let (tx, daemon) = answering_daemon();
        let server = ControlServer::start(&path, tx).unwrap();
        let client = UnixControlClient::new(&path);
        match client.send(&ControlRequest::Status {}).unwrap() {
            ControlResponse::Status { fields } => assert_eq!(fields["request"], "Status"),
            other => panic!("{other:?}"),
        }
        drop(server);
        assert_eq!(daemon.join().unwrap(), 1);
    }

    #[test]
    fn stale_socket_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // A bound and dropped listener leaves its socket file behind with nobody listening.
        drop(StdListener::bind(&path).unwrap());
        assert!(path.exists());
        let (tx, _daemon) = answering_daemon();
        let server = ControlServer::start(&path, tx).unwrap();
        assert!(UnixControlClient::new(&path)
            .send(&ControlRequest::Status {})
            .is_ok());
        drop(server);
    }

    #[test]
    fn live_socket_is_not_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let (tx, _daemon) = answering_daemon();
        let _first = ControlServer::start(&path, tx.clone()).unwrap();
        assert!(matches!(
            ControlServer::start(&path, tx),
            Err(ControlError::AlreadyRunning(_))
        ));
    }

    #[test]
    fn regular_file_at_the_socket_path_is_not_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-socket");
        std::fs::write(&path, b"keep").unwrap();
        let (tx, _daemon) = answering_daemon();
        assert!(matches!(
            ControlServer::start(&path, tx),
            Err(ControlError::NotASocket(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"keep");
    }

    #[test]
    fn socket_path_longer_than_a_unix_address_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x".repeat(120));
        let (tx, _daemon) = answering_daemon();
        assert!(matches!(
            ControlServer::start(&path, tx),
            Err(ControlError::PathTooLong(_))
        ));
    }

    #[test]
    fn oversized_request_line_gets_an_invalid_answer_and_never_reaches_the_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let (tx, daemon) = answering_daemon();
        let server = ControlServer::start(&path, tx).unwrap();
        let mut stream = UnixStream::connect(&path).unwrap();
        let mut big = br#"{"cmd":"get","key":""#.to_vec();
        big.resize(MAX_REQUEST_LINE * 4, b'a');
        big.extend_from_slice(b"\"}\n");
        // The server stops reading at the cap and may close first, so a write error is fine.
        let _ = stream.write_all(&big);
        let mut answer = Vec::new();
        let _ = BufReader::new(&stream).read_until(b'\n', &mut answer);
        assert!(matches!(
            ControlResponse::from_line(&answer).unwrap(),
            ControlResponse::Invalid { .. }
        ));
        drop(server);
        assert_eq!(daemon.join().unwrap(), 0);
    }

    #[test]
    fn malformed_and_unknown_requests_get_invalid_answers() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let (tx, _daemon) = answering_daemon();
        let _server = ControlServer::start(&path, tx).unwrap();
        for bad in [&b"garbage\n"[..], b"{\"cmd\":\"explode\"}\n"] {
            let mut stream = UnixStream::connect(&path).unwrap();
            stream.write_all(bad).unwrap();
            let mut answer = Vec::new();
            BufReader::new(&stream)
                .read_until(b'\n', &mut answer)
                .unwrap();
            assert!(matches!(
                ControlResponse::from_line(&answer).unwrap(),
                ControlResponse::Invalid { .. }
            ));
        }
    }

    #[test]
    fn concurrent_clients_each_get_their_own_answer() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let (tx, daemon) = answering_daemon();
        let server = ControlServer::start(&path, tx).unwrap();
        let threads: Vec<_> = (0..8)
            .map(|n| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let key = format!("key{n}");
                    let request = ControlRequest::Get {
                        key: Some(key.clone()),
                    };
                    match UnixControlClient::new(&path).send(&request).unwrap() {
                        ControlResponse::Status { fields } => {
                            assert!(fields["request"].contains(&key), "{fields:?}")
                        }
                        other => panic!("{other:?}"),
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        drop(server);
        assert_eq!(daemon.join().unwrap(), 8);
    }

    #[test]
    fn silent_client_does_not_block_other_clients() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let (tx, _daemon) = answering_daemon();
        let _server = ControlServer::start(&path, tx).unwrap();
        let _silent = UnixStream::connect(&path).unwrap();
        assert!(UnixControlClient::new(&path)
            .send(&ControlRequest::Status {})
            .is_ok());
    }

    #[test]
    fn requests_are_rejected_when_the_daemon_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let (tx, rx) = mpsc::channel::<Command>();
        drop(rx);
        let _server = ControlServer::start(&path, tx).unwrap();
        assert!(matches!(
            UnixControlClient::new(&path)
                .send(&ControlRequest::Status {})
                .unwrap(),
            ControlResponse::Rejected { .. }
        ));
    }
}
