use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tracing::{info, warn};

use super::events::DockEvent;
use super::state::sleep_until;
use super::{send_control, SessionError, SessionSettings, SessionState, Shared, SharedWriter};
use crate::core::policy::POLL_SLICE;
use crate::protocol::inbound::LOG_TEXT_MAX;
use crate::protocol::messages;

/// How a wait ended early.
pub(super) enum Interrupted {
    Stopped,
}

/// Outcome of the handshake's bounded waits.
pub(super) enum Wait<T> {
    Got(T),
    TimedOut,
}

pub(super) struct Controller {
    pub(super) settings: SessionSettings,
    pub(super) shared: Arc<Shared>,
    pub(super) writer: SharedWriter,
    pub(super) events: Receiver<DockEvent>,
    pub(super) next_id: u64,
    /// A ready announcement the monitor consumed; the next handshake starts from it instead
    /// of waiting for another one.
    pub(super) ready_pending: bool,
}

impl Controller {
    pub(super) fn run(&mut self) {
        while !self.shared.stopped() {
            if self.shared.fatal.lock().is_ok_and(|f| f.is_some()) {
                return;
            }
            match self.handshake() {
                Ok(state) => {
                    self.shared.set_state(state);
                    self.shared.generation.fetch_add(1, Ordering::AcqRel);
                    self.shared.handshakes.fetch_add(1, Ordering::Relaxed);
                    info!(?state, "session armed, video allowed");
                    let Some(reason) = self.monitor() else {
                        return;
                    };
                    self.shared.losses.fetch_add(1, Ordering::Relaxed);
                    self.shared.set_state(SessionState::Lost);
                    warn!(%reason, "session lost, redoing the handshake");
                }
                Err(HandshakeEnd::Stopped) => return,
                Err(HandshakeEnd::Failed(e)) => {
                    self.shared.set_state(SessionState::Lost);
                    warn!(error = %e, "handshake failed, retrying");
                    if !sleep_until(Instant::now() + self.settings.retry_delay, &self.shared) {
                        return;
                    }
                }
            }
        }
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
    /// only within the reply timeout. `Active` needs the ready announcement and `isAllowed`;
    /// every other outcome arms as `AssumedActive` after the settle delay.
    pub(super) fn handshake(&mut self) -> Result<SessionState, HandshakeEnd> {
        self.drain_events();
        self.shared.set_state(SessionState::WaitingForReady);
        let ready_timeout = self.settings.ready_timeout;
        let ready = if std::mem::take(&mut self.ready_pending) {
            Wait::Got(())
        } else {
            self.wait_event(ready_timeout, |e| {
                matches!(e, DockEvent::Ready).then_some(())
            })?
        };
        let ready_seen = matches!(ready, Wait::Got(()));
        if !ready_seen {
            warn!(
                ?ready_timeout,
                dock_alive = self.shared.dock_alive(),
                "dock did not announce ready, sending the handshake anyway"
            );
        }

        if ready_seen && self.settings.send_stopped {
            self.send(messages::PROJECTION_STOPPED)?;
        }
        let launch_id = self.next_id();
        self.shared.set_state(SessionState::AppLaunchedSent);
        self.send(&messages::app_launched(
            launch_id,
            &self.settings.machine_name,
            self.settings.size,
        ))?;
        let mut confirmed = self
            .wait_reply("app-launched", launch_id, ready_seen)?
            .is_some();

        self.send(messages::MOUSE_ICON_CHANGE)?;
        self.send(messages::EDID_ACK)?;
        let start_id = self.next_id();
        self.shared.set_state(SessionState::ProjectionStartedSent);
        self.send(&messages::projection_started(start_id))?;
        match self.wait_reply("notify-projection-started", start_id, ready_seen)? {
            None => confirmed = false,
            Some(Value::String(s)) if s == "isAllowed" => {}
            Some(other) if ready_seen => {
                return Err(HandshakeEnd::Failed(SessionError::Denied(truncate(
                    &other.to_string(),
                ))))
            }
            Some(_) => confirmed = false,
        }

        self.shared.set_state(SessionState::Settling);
        if !sleep_until(Instant::now() + self.settings.settle_delay, &self.shared) {
            return Err(HandshakeEnd::Stopped);
        }
        Ok(if ready_seen && confirmed {
            SessionState::Active
        } else {
            SessionState::AssumedActive
        })
    }

    /// Watches an armed session. Returns the loss reason, or `None` when shut down or the
    /// reader died.
    pub(super) fn monitor(&mut self) -> Option<String> {
        // Announcements that were queued while the handshake ran don't count as loss.
        self.drain_events();
        loop {
            if self.shared.stopped() || self.shared.fatal.lock().is_ok_and(|f| f.is_some()) {
                return None;
            }
            if let Some(reason) = self
                .shared
                .loss_reason
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                return Some(reason);
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
                return Some("heartbeat writes keep failing".into());
            }
            match self.events.recv_timeout(POLL_SLICE) {
                Ok(DockEvent::Ready) => {
                    self.ready_pending = true;
                    return Some("dock announced ready".into());
                }
                Ok(DockEvent::Reply { .. }) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }
}

pub(super) enum HandshakeEnd {
    Stopped,
    Failed(SessionError),
}

impl From<Interrupted> for HandshakeEnd {
    fn from(_: Interrupted) -> Self {
        HandshakeEnd::Stopped
    }
}

pub(super) fn truncate(text: &str) -> String {
    text.chars().take(LOG_TEXT_MAX * 2).collect()
}
