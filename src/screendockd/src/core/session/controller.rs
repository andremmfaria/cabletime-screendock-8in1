use std::sync::atomic::Ordering;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use serde_json::Value;
use tracing::{info, warn};

use super::state::sleep_until;
use super::types::DockEvent;
pub(super) use super::types::{
    Controller, ControllerCommand, HandshakeEnd, Interrupted, MonitorEnd, Renegotiation, Wait,
};
use super::{send_control, SessionError, SessionState};
use crate::core::policy::POLL_SLICE;
use crate::protocol::inbound::LOG_TEXT_MAX;
use crate::protocol::messages;

impl Controller {
    pub(super) fn run(&mut self) {
        let mut pending: Option<Renegotiation> = None;
        while !self.shared.stopped() {
            if self.shared.fatal.lock().is_ok_and(|f| f.is_some()) {
                return;
            }
            if pending.is_none() {
                pending = self.take_command();
            }
            let renegotiating = pending.is_some();
            match self.handshake(renegotiating) {
                Ok(state) => {
                    self.shared.set_size(self.settings.size);
                    self.shared.set_state(state);
                    self.shared.generation.fetch_add(1, Ordering::AcqRel);
                    self.shared.handshakes.fetch_add(1, Ordering::Relaxed);
                    info!(?state, renegotiating, "session armed, video allowed");
                    if let Some(done) = pending.take() {
                        let _ = done.reply.send(Ok(()));
                    }
                    match self.monitor() {
                        MonitorEnd::Stopped => return,
                        MonitorEnd::Command(command) => {
                            pending = Some(self.begin(command));
                        }
                        MonitorEnd::Lost(reason) => {
                            self.shared.losses.fetch_add(1, Ordering::Relaxed);
                            self.shared.set_state(SessionState::Lost);
                            warn!(%reason, "session lost, redoing the handshake");
                        }
                    }
                }
                Err(HandshakeEnd::Stopped) => return,
                Err(HandshakeEnd::Failed(e)) => {
                    self.shared.set_state(SessionState::Lost);
                    warn!(error = %e, "handshake failed, retrying");
                    if let Some(failed) = pending.take() {
                        // The retries run at the size the dock last accepted.
                        self.settings.size = failed.previous;
                        let _ = failed.reply.send(Err(e));
                    }
                    if !sleep_until(Instant::now() + self.settings.retry_delay, &self.shared) {
                        return;
                    }
                }
            }
        }
    }

    /// A renegotiation requested while the session was not armed.
    fn take_command(&mut self) -> Option<Renegotiation> {
        self.commands
            .try_recv()
            .ok()
            .map(|command| self.begin(command))
    }

    /// Switches the settings to the requested size and remembers where to return to.
    fn begin(&mut self, command: ControllerCommand) -> Renegotiation {
        let ControllerCommand::Renegotiate { size, reply } = command;
        let previous = std::mem::replace(&mut self.settings.size, size);
        self.ready_pending = false;
        self.shared.set_state(SessionState::Renegotiating);
        Renegotiation { previous, reply }
    }

    pub(super) fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub(super) fn send(&self, payload: &[u8]) -> Result<(), HandshakeEnd> {
        send_control(&self.writer, payload).map_err(HandshakeEnd::Failed)
    }

    /// Waits up to `timeout` for an event accepted by `pick`; other events are discarded.
    pub(super) fn wait_event<T>(
        &self,
        timeout: Duration,
        mut pick: impl FnMut(&DockEvent) -> Option<T>,
    ) -> Result<Wait<T>, Interrupted> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.shared.stopped() || self.shared.fatal.lock().is_ok_and(|f| f.is_some()) {
                return Err(Interrupted::Stopped);
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Ok(Wait::TimedOut);
            };
            match self.events.recv_timeout(left.min(POLL_SLICE)) {
                Ok(event) => {
                    if let Some(found) = pick(&event) {
                        return Ok(Wait::Got(found));
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err(Interrupted::Stopped),
            }
        }
    }

    pub(super) fn drain_events(&self) {
        while self.events.try_recv().is_ok() {}
    }

    /// Waits up to the reply timeout for the reply to request `id`. `None` means nothing
    /// arrived in time; the caller proceeds regardless because the dock acts on the request
    /// even when its reply is not delivered. With `strict` an error reply fails the attempt.
    pub(super) fn wait_reply(
        &self,
        request: &'static str,
        id: u64,
        strict: bool,
    ) -> Result<Option<Value>, HandshakeEnd> {
        let timeout = self.settings.reply_timeout;
        let reply = self.wait_event(timeout, |event| match event {
            DockEvent::Reply {
                id: got,
                result,
                error,
            } if *got == id => Some((result.clone(), error.clone())),
            _ => None,
        })?;
        match reply {
            Wait::Got((result, error)) => {
                if let Some(error) = error {
                    if strict {
                        return Err(HandshakeEnd::Failed(SessionError::Rejected {
                            request,
                            detail: truncate(&error.to_string()),
                        }));
                    }
                    warn!(request, error = %truncate(&error.to_string()), "dock rejected a request on the silent path, continuing");
                    return Ok(None);
                }
                Ok(Some(result.unwrap_or(Value::Null)))
            }
            Wait::TimedOut => {
                warn!(
                    request,
                    ?timeout,
                    dock_alive = self.shared.dock_alive(),
                    "no answer from the dock, continuing"
                );
                Ok(None)
            }
        }
    }

    /// Runs one handshake and returns the armed state to enter.
    ///
    /// The full message sequence is sent whether or not the dock announced ready or answers:
    /// a dock that announces nothing still processes the messages. Replies are waited for
    /// only within the reply timeout. `Active` needs the ready announcement (or a
    /// renegotiation) and `isAllowed`; every other outcome arms as `AssumedActive` after the
    /// settle delay.
    ///
    /// A renegotiation starts from a dock that holds a session, so it does not wait for a
    /// ready announcement, always sends `notify-projection-stopped` first and treats an error
    /// reply as a failure.
    pub(super) fn handshake(&mut self, renegotiating: bool) -> Result<SessionState, HandshakeEnd> {
        self.drain_events();
        self.step(SessionState::WaitingForReady, renegotiating);
        let ready_timeout = self.settings.ready_timeout;
        let ready_seen = if renegotiating {
            self.shared.set_state(SessionState::Renegotiating);
            false
        } else {
            let ready = if std::mem::take(&mut self.ready_pending) {
                Wait::Got(())
            } else {
                self.wait_event(ready_timeout, |e| {
                    matches!(e, DockEvent::Ready).then_some(())
                })?
            };
            let seen = matches!(ready, Wait::Got(()));
            if !seen {
                warn!(
                    ?ready_timeout,
                    dock_alive = self.shared.dock_alive(),
                    "dock did not announce ready, sending the handshake anyway"
                );
            }
            seen
        };
        let strict = ready_seen || renegotiating;

        if renegotiating || (ready_seen && self.settings.send_stopped) {
            self.send(messages::PROJECTION_STOPPED)?;
        }
        let launch_id = self.next_id();
        self.step(SessionState::AppLaunchedSent, renegotiating);
        self.send(&messages::app_launched(
            launch_id,
            &self.settings.machine_name,
            self.settings.size,
        ))?;
        let mut confirmed = self
            .wait_reply("app-launched", launch_id, strict)?
            .is_some();

        self.send(messages::MOUSE_ICON_CHANGE)?;
        self.send(messages::EDID_ACK)?;
        let start_id = self.next_id();
        self.step(SessionState::ProjectionStartedSent, renegotiating);
        self.send(&messages::projection_started(start_id))?;
        match self.wait_reply("notify-projection-started", start_id, strict)? {
            None => confirmed = false,
            Some(Value::String(s)) if s == "isAllowed" => {}
            Some(other) if strict => {
                return Err(HandshakeEnd::Failed(SessionError::Denied(truncate(
                    &other.to_string(),
                ))))
            }
            Some(_) => confirmed = false,
        }

        self.step(SessionState::Settling, renegotiating);
        if !sleep_until(Instant::now() + self.settings.settle_delay, &self.shared) {
            return Err(HandshakeEnd::Stopped);
        }
        Ok(if strict && confirmed {
            SessionState::Active
        } else {
            SessionState::AssumedActive
        })
    }

    /// Publishes a handshake step. A renegotiation stays `Renegotiating` throughout.
    fn step(&self, state: SessionState, renegotiating: bool) {
        if !renegotiating {
            self.shared.set_state(state);
        }
    }

    /// Watches an armed session until it is lost, a renegotiation is requested, or the session
    /// shuts down or its reader dies.
    pub(super) fn monitor(&mut self) -> MonitorEnd {
        // Announcements that were queued while the handshake ran don't count as loss.
        self.drain_events();
        loop {
            if self.shared.stopped() || self.shared.fatal.lock().is_ok_and(|f| f.is_some()) {
                return MonitorEnd::Stopped;
            }
            if let Some(reason) = self
                .shared
                .loss_reason
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                return MonitorEnd::Lost(reason);
            }
            if self
                .shared
                .consecutive_heartbeat_failures
                .load(Ordering::Relaxed)
                >= self.settings.max_heartbeat_failures
            {
                self.shared
                    .consecutive_heartbeat_failures
                    .store(0, Ordering::Relaxed);
                return MonitorEnd::Lost("heartbeat writes keep failing".into());
            }
            if let Ok(command) = self.commands.try_recv() {
                return MonitorEnd::Command(command);
            }
            match self.events.recv_timeout(POLL_SLICE) {
                Ok(DockEvent::Ready) => {
                    self.ready_pending = true;
                    return MonitorEnd::Lost("dock announced ready".into());
                }
                Ok(DockEvent::Reply { .. }) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return MonitorEnd::Stopped,
            }
        }
    }
}

pub(super) fn truncate(text: &str) -> String {
    text.chars().take(LOG_TEXT_MAX * 2).collect()
}
