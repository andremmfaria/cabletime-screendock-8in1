use std::sync::atomic::Ordering;
use std::time::Instant;

use tracing::warn;

use super::state::sleep_until;
use super::{SessionSettings, Shared, SharedWriter};
use crate::protocol::heartbeat::heartbeat_message;

/// Heartbeat writer: first tick one interval after the synchronous startup heartbeat, then one
/// per interval against fixed deadlines, with the constant session id.
pub(super) fn heartbeat_thread(writer: &SharedWriter, settings: &SessionSettings, shared: &Shared) {
    let start = Instant::now();
    let message = heartbeat_message(&settings.session_id);
    let mut tick: u32 = 1;
    loop {
        let deadline = start + settings.heartbeat_interval * tick;
        if !sleep_until(deadline, shared) {
            return;
        }
        let result = {
            let mut handle = writer.lock().unwrap_or_else(|e| e.into_inner());
            handle.write_chunks(&message)
        };
        match result {
            Ok(_) => {
                shared.heartbeats_sent.fetch_add(1, Ordering::Relaxed);
                shared
                    .consecutive_heartbeat_failures
                    .store(0, Ordering::Relaxed);
            }
            Err(e) => {
                shared.heartbeat_errors.fetch_add(1, Ordering::Relaxed);
                let failures = shared
                    .consecutive_heartbeat_failures
                    .fetch_add(1, Ordering::Relaxed)
                    + 1;
                warn!(error = %e, failures, "heartbeat write failed, will retry on the next tick");
            }
        }
        tick = tick.saturating_add(1);
    }
}
