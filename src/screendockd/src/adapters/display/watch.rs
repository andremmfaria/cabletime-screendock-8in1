//! Reports changes made to the virtual monitor from outside the daemon.
//!
//! A [`MonitorWatcher`] runs on its own thread. Every notification from the display server
//! restarts a short quiet period; when it ends the watcher reads the monitor's rectangle and
//! compares it with the one the daemon expects. A difference is reported once. The daemon
//! changes the expected rectangle before it changes the monitor, so what its own changes
//! cause is never reported.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use super::types::{ExpectedRect, MonitorWatcher, RectSource, WatchTarget};
use super::types::{WATCH_DEBOUNCE, WATCH_POLL, WATCH_RECHECK};
use crate::ports::display::{DisplayEvent, DisplayEventSink, Rect, VdisplayError};

/// What differs between the rectangle the daemon expects and what the desktop shows. `None`
/// when they agree.
pub fn classify(expected: Rect, observed: Option<Rect>) -> Option<DisplayEvent> {
    let rect = match observed {
        None => return Some(DisplayEvent::Removed),
        Some(rect) if rect == expected => return None,
        Some(rect) => rect,
    };
    Some(DisplayEvent::Changed {
        rect,
        resized: (rect.width, rect.height) != (expected.width, expected.height),
        moved: (rect.x, rect.y) != (expected.x, expected.y),
    })
}

/// Reads the monitor once more before a missing monitor is reported: the daemon's own resize
/// of a bare monitor removes it for an instant.
fn confirmed(
    source: &mut dyn RectSource,
    target: &WatchTarget,
    first: Option<Rect>,
) -> Result<Option<Rect>, VdisplayError> {
    if first.is_some() {
        return Ok(first);
    }
    std::thread::sleep(WATCH_DEBOUNCE);
    source.rect(target)
}

impl MonitorWatcher {
    /// Starts watching `target` on a new thread. `expected` is the rectangle the daemon
    /// wants; differences go to `notify`.
    pub fn spawn(
        mut source: impl RectSource + 'static,
        target: WatchTarget,
        expected: ExpectedRect,
        notify: DisplayEventSink,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("display-watch".into())
            .spawn(move || watch(&mut source, &target, &expected, &notify, &flag))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for MonitorWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn watch(
    source: &mut dyn RectSource,
    target: &WatchTarget,
    expected: &ExpectedRect,
    notify: &DisplayEventSink,
    stop: &AtomicBool,
) {
    let mut quiet_since: Option<Instant> = None;
    let mut reported: Option<DisplayEvent> = None;
    let mut last_look = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        match source.changes_pending() {
            Ok(true) => quiet_since = Some(Instant::now()),
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%error, "lost the display connection; no longer following the monitor");
                return;
            }
        }
        if quiet_since.is_none() && last_look.elapsed() >= WATCH_RECHECK {
            quiet_since = Some(Instant::now() - WATCH_DEBOUNCE);
        }
        if quiet_since.is_some_and(|t| t.elapsed() >= WATCH_DEBOUNCE) {
            quiet_since = None;
            last_look = Instant::now();
            let want = *expected.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(want) = want {
                let seen = source
                    .rect(target)
                    .and_then(|first| confirmed(source, target, first));
                match seen {
                    Ok(seen) => {
                        let event = classify(want, seen);
                        if event != reported {
                            reported = event;
                            if let Some(event) = event {
                                notify(event);
                            }
                        }
                    }
                    Err(error) => tracing::warn!(%error, "could not read the monitor"),
                }
            }
        }
        std::thread::sleep(WATCH_POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, Receiver};
    use std::sync::Mutex;
    use std::time::Duration;

    const START: Rect = Rect {
        x: 1280,
        y: 0,
        width: 1280,
        height: 720,
    };

    /// A desktop whose monitor can be changed by the test.
    #[derive(Clone)]
    struct Desktop {
        rect: Arc<Mutex<Option<Rect>>>,
        pending: Arc<AtomicBool>,
    }

    impl Desktop {
        fn change(&self, rect: Option<Rect>) {
            *self.rect.lock().unwrap() = rect;
            self.pending.store(true, Ordering::Relaxed);
        }
    }

    impl RectSource for Desktop {
        fn changes_pending(&mut self) -> Result<bool, VdisplayError> {
            Ok(self.pending.swap(false, Ordering::Relaxed))
        }
        fn rect(&mut self, _target: &WatchTarget) -> Result<Option<Rect>, VdisplayError> {
            Ok(*self.rect.lock().unwrap())
        }
    }

    fn watched() -> (
        Desktop,
        ExpectedRect,
        MonitorWatcher,
        Receiver<DisplayEvent>,
    ) {
        let desktop = Desktop {
            rect: Arc::new(Mutex::new(Some(START))),
            pending: Arc::new(AtomicBool::new(false)),
        };
        let expected: ExpectedRect = Arc::new(Mutex::new(Some(START)));
        let (tx, rx) = mpsc::channel();
        let watcher = MonitorWatcher::spawn(
            desktop.clone(),
            WatchTarget::Monitor("ScreenDock".into()),
            Arc::clone(&expected),
            Box::new(move |event| {
                let _ = tx.send(event);
            }),
        )
        .unwrap();
        (desktop, expected, watcher, rx)
    }

    const LONG: Duration = Duration::from_secs(2);
    const SHORT: Duration = Duration::from_millis(600);

    #[test]
    fn classify_names_what_differs() {
        assert_eq!(classify(START, Some(START)), None);
        assert_eq!(classify(START, None), Some(DisplayEvent::Removed));
        let bigger = Rect::new(1280, 0, 1600, 900);
        assert_eq!(
            classify(START, Some(bigger)),
            Some(DisplayEvent::Changed {
                rect: bigger,
                resized: true,
                moved: false
            })
        );
        let moved = Rect::new(0, 720, 1280, 720);
        assert_eq!(
            classify(START, Some(moved)),
            Some(DisplayEvent::Changed {
                rect: moved,
                resized: false,
                moved: true
            })
        );
    }

    #[test]
    fn a_resize_from_outside_is_reported_once_after_the_quiet_period() {
        let (desktop, _expected, _watcher, rx) = watched();
        let bigger = Rect::new(1280, 0, 1600, 900);
        desktop.change(Some(bigger));
        desktop.change(Some(bigger));
        let event = rx.recv_timeout(LONG).unwrap();
        assert_eq!(
            event,
            DisplayEvent::Changed {
                rect: bigger,
                resized: true,
                moved: false
            }
        );
        desktop.change(Some(bigger));
        assert!(rx.recv_timeout(SHORT).is_err(), "the same change twice");
    }

    #[test]
    fn a_change_the_daemon_announced_first_is_not_reported() {
        let (desktop, expected, _watcher, rx) = watched();
        let bigger = Rect::new(1280, 0, 1600, 900);
        *expected.lock().unwrap() = Some(bigger);
        desktop.change(Some(bigger));
        assert!(rx.recv_timeout(SHORT).is_err());
    }

    #[test]
    fn a_vanished_monitor_is_reported_as_removed() {
        let (desktop, _expected, _watcher, rx) = watched();
        desktop.change(None);
        assert_eq!(rx.recv_timeout(LONG).unwrap(), DisplayEvent::Removed);
    }

    #[test]
    fn a_monitor_that_is_back_after_a_blink_is_not_reported() {
        let (desktop, _expected, _watcher, rx) = watched();
        desktop.change(None);
        std::thread::sleep(WATCH_DEBOUNCE / 2);
        desktop.change(Some(START));
        assert!(rx.recv_timeout(SHORT).is_err());
    }

    #[test]
    fn nothing_is_reported_while_no_rectangle_is_expected() {
        let (desktop, expected, _watcher, rx) = watched();
        *expected.lock().unwrap() = None;
        desktop.change(None);
        assert!(rx.recv_timeout(SHORT).is_err());
    }
}
