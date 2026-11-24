//! Dock session on MI_00: continuous heartbeat, a reader that always drains the dock's IN
//! reports, and the handshake state machine that arms the video sink.
//!
//! The dock discards every video frame, silently, until three things have happened: the host
//! sent `app-launched` (a request with an id and a `platform`), the host sent
//! `notify-projection-started` (a request with an id), and the dock answered `isAllowed`. A
//! dock that already holds a projection session from an earlier host process announces and
//! answers nothing, yet processes these messages and then renders video. The state machine
//! therefore always sends the full message sequence, bounds every wait, and arms as
//! [`SessionState::AssumedActive`] when the ready announcement or a reply does not arrive,
//! whether or not the dock sent anything at all (a session-holding dock often echoes no
//! heartbeat either). Video is never blocked on dock replies. The controller keeps watching in
//! the armed states: if the dock announces `notify-app-accessories-are-ready` (it holds no
//! session), or heartbeat or video writes keep failing, the session is lost and the full
//! handshake is run without restarting the process.
//!
//! Threads, all owned by [`Session`]:
//! - heartbeat: one fixed-deadline writer, independent of video writes (they use MI_01);
//! - reader: polls MI_00, reassembles and parses the dock's JSON, answers the dock's EDID
//!   passthrough, and forwards the rest to the controller as [`DockEvent`]s;
//! - controller: runs the handshake, then watches for loss and starts over.
//!
//! Mechanism/policy split: [`messages`] is policy (exact bytes of every host message),
//! [`InboundAssembler`] and [`parse_dock_message`] are the parsing mechanism, and the
//! controller sequences them. Timeouts are settings so tests run in milliseconds.
//!
//! Dock text is untrusted data: it is parsed as JSON, never executed, and only truncated method
//! names reach the logs.

mod controller;
mod error;
mod events;
mod heartbeat;
mod reader;
mod settings;
mod state;
#[cfg(test)]
mod tests;

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub use crate::core::policy::{
    DEFAULT_MAX_HEARTBEAT_FAILURES, DEFAULT_READY_TIMEOUT, DEFAULT_REPLY_TIMEOUT,
    DEFAULT_RETRY_DELAY, DEFAULT_SETTLE_DELAY,
};
pub use error::SessionError;
pub use settings::{hostname, SessionSettings};
pub use state::{SessionHandle, SessionState, SessionStats};

use controller::Controller;
use heartbeat::heartbeat_thread;
use reader::reader_thread;
use state::Shared;

use crate::ports::transport::{DockTransport, TransportError};
use crate::protocol::framing::{MessageBuilder, ReportId, FIRST_CHUNK_PAYLOAD_MAX};
use crate::protocol::heartbeat::heartbeat_message;

/// A running session: owns the heartbeat, reader and controller threads.
#[derive(Debug)]
pub struct Session {
    shared: Arc<Shared>,
    threads: Vec<(&'static str, JoinHandle<()>)>,
}

type SharedWriter = Arc<Mutex<Box<dyn DockTransport>>>;

fn send_control(writer: &SharedWriter, payload: &[u8]) -> Result<(), SessionError> {
    if payload.len() > FIRST_CHUNK_PAYLOAD_MAX {
        return Err(SessionError::MessageTooLarge { len: payload.len() });
    }
    let chunks = MessageBuilder::control().payload(payload).build();
    let mut handle = writer.lock().unwrap_or_else(|e| e.into_inner());
    handle
        .write_chunks(&chunks)
        .map(|_| ())
        .map_err(SessionError::Write)
}

impl Session {
    /// Starts the session on an MI_00 handle.
    ///
    /// The first heartbeat is written synchronously, so a dead control channel fails here.
    /// With `settings.handshake` the reader and controller threads follow and the session
    /// starts unarmed; without it only the heartbeat thread runs and video is armed at once.
    pub fn start(
        mi00: impl DockTransport,
        settings: SessionSettings,
    ) -> Result<Self, SessionError> {
        let mi00: Box<dyn DockTransport> = Box::new(mi00);
        if mi00.channel() != ReportId::Control {
            return Err(SessionError::WrongChannel {
                got: mi00.channel(),
            });
        }
        let reader_handle = if settings.handshake {
            Some(mi00.try_clone().map_err(SessionError::Setup)?)
        } else {
            None
        };
        let writer: SharedWriter = Arc::new(Mutex::new(mi00));
        let initial = if settings.handshake {
            SessionState::Starting
        } else {
            SessionState::Active
        };
        let shared = Arc::new(Shared::new(initial));

        {
            let mut handle = writer.lock().unwrap_or_else(|e| e.into_inner());
            handle
                .write_chunks(&heartbeat_message(&settings.session_id))
                .map_err(SessionError::StartupHeartbeat)?;
        }
        shared.heartbeats_sent.store(1, Ordering::Relaxed);

        let mut threads: Vec<(&'static str, JoinHandle<()>)> = Vec::new();
        let hb_shared = Arc::clone(&shared);
        let hb_writer = Arc::clone(&writer);
        let hb_settings = settings.clone();
        threads.push((
            "heartbeat",
            std::thread::Builder::new()
                .name("session-heartbeat".into())
                .spawn(move || heartbeat_thread(&hb_writer, &hb_settings, &hb_shared))
                .map_err(|e| SessionError::Setup(TransportError::Clone(e)))?,
        ));

        if let Some(reader_handle) = reader_handle {
            let (tx, rx) = mpsc::channel();
            let rd_shared = Arc::clone(&shared);
            let rd_writer = Arc::clone(&writer);
            threads.push((
                "reader",
                std::thread::Builder::new()
                    .name("session-reader".into())
                    .spawn(move || reader_thread(reader_handle, &rd_writer, &tx, &rd_shared))
                    .map_err(|e| SessionError::Setup(TransportError::Clone(e)))?,
            ));
            let ctl_shared = Arc::clone(&shared);
            let ctl_writer = Arc::clone(&writer);
            threads.push((
                "controller",
                std::thread::Builder::new()
                    .name("session-controller".into())
                    .spawn(move || {
                        let mut controller = Controller {
                            settings,
                            shared: ctl_shared,
                            writer: ctl_writer,
                            events: rx,
                            next_id: 1,
                            ready_pending: false,
                        };
                        controller.run();
                    })
                    .map_err(|e| SessionError::Setup(TransportError::Clone(e)))?,
            ));
        } else {
            shared.generation.store(1, Ordering::Release);
            shared.handshakes.store(1, Ordering::Relaxed);
        }

        Ok(Self { shared, threads })
    }

    /// A cloneable view for the video loop.
    pub fn handle(&self) -> SessionHandle {
        SessionHandle {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Stops all threads and returns the counters.
    pub fn shutdown(mut self) -> Result<SessionStats, SessionError> {
        self.shared.stop.store(true, Ordering::Relaxed);
        let mut panicked = None;
        for (name, thread) in self.threads.drain(..) {
            if thread.join().is_err() {
                panicked = Some(name);
            }
        }
        if let Some(name) = panicked {
            return Err(SessionError::Panicked(name));
        }
        self.shared.set_state(SessionState::Stopped);
        Ok(SessionStats {
            heartbeats_sent: self.shared.heartbeats_sent.load(Ordering::Relaxed),
            heartbeat_errors: self.shared.heartbeat_errors.load(Ordering::Relaxed),
            dock_heartbeats_seen: self.shared.dock_heartbeats.load(Ordering::Relaxed),
            handshakes: self.shared.handshakes.load(Ordering::Relaxed),
            losses: self.shared.losses.load(Ordering::Relaxed),
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        for (_, thread) in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}
