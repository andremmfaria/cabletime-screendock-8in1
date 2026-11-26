//! Virtual monitors on an X server through RandR.
//!
//! Tier 1 turns a free output on with a custom mode: the monitor is a real output and shows in
//! the desktop's display settings. Tier 2 defines a bare RandR monitor over an enlarged screen.
//! Tier 1 falls back to tier 2 when the server refuses it. Before the first change a recovery
//! file records what is about to happen; dropping the monitor undoes every change and removes
//! the file.

use super::types::{RandrBackend, RecoveryState, RecoveryStore, ScreenSize, XRandr, MODE_PREFIX};
pub use super::types::{X11MonitorGuard, X11RandrDisplay};
use crate::ports::display::{
    MonitorSpec, OutputId, Rect, VdisplayError, VirtualDisplay, VirtualMonitor, X11Tier,
};

impl X11RandrDisplay {
    /// A display for the tier policy chose on `display` (or `$DISPLAY`); `store` holds the
    /// recovery file.
    pub fn new(display: Option<String>, tier: X11Tier, store: RecoveryStore) -> Self {
        Self {
            display,
            tier,
            store,
        }
    }
}

impl VirtualDisplay for X11RandrDisplay {
    fn name(&self) -> &'static str {
        match self.tier {
            X11Tier::RealOutput(_) => "x11-randr-output",
            X11Tier::BareMonitor => "x11-randr-monitor",
        }
    }

    fn create(&self, spec: &MonitorSpec) -> Result<Box<dyn VirtualMonitor>, VdisplayError> {
        match super::recovery::recover(&self.store)? {
            super::types::RecoveryOutcome::StillRunning(pid) => {
                return Err(VdisplayError::Recovery(format!(
                    "daemon {pid} already changed this display"
                )))
            }
            super::types::RecoveryOutcome::Restored => {
                tracing::info!("undid the changes of an earlier daemon that did not exit cleanly")
            }
            super::types::RecoveryOutcome::NothingToDo => {}
        }
        if let X11Tier::RealOutput(output) = self.tier {
            let attempt = XRandr::connect(self.display.as_deref()).and_then(|backend| {
                create_monitor(
                    backend,
                    X11Tier::RealOutput(output),
                    &self.store,
                    self.display.clone(),
                    spec,
                )
            });
            match attempt {
                Ok(guard) => return Ok(Box::new(guard)),
                Err(reason) => tracing::warn!(
                    %reason,
                    "the output could not take the monitor; using a bare RandR monitor"
                ),
            }
        }
        let backend = XRandr::connect(self.display.as_deref())?;
        let guard = create_monitor(
            backend,
            X11Tier::BareMonitor,
            &self.store,
            self.display.clone(),
            spec,
        )?;
        Ok(Box::new(guard))
    }
}

fn mode_name(spec: &MonitorSpec) -> String {
    format!(
        "{MODE_PREFIX}{}x{}-{}",
        spec.rect.width,
        spec.rect.height,
        std::process::id()
    )
}

/// The screen size that holds the current screen and `rect`.
fn grown(current: ScreenSize, rect: Rect) -> Result<ScreenSize, VdisplayError> {
    if rect.x < 0 || rect.y < 0 {
        return Err(VdisplayError::Unsupported(
            "a monitor at a negative position cannot be placed on an X screen".into(),
        ));
    }
    let width = current.width.max(rect.right() as u32);
    let height = current.height.max(rect.bottom() as u32);
    if (width, height) == (current.width, current.height) {
        return Ok(current);
    }
    Ok(ScreenSize {
        width,
        height,
        width_mm: width * 254 / 960,
        height_mm: height * 254 / 960,
    })
}

/// Makes the monitor `spec` describes on `backend` with `tier`, recording first what will
/// change. On failure everything done so far is undone.
pub(super) fn create_monitor<B: RandrBackend + Send>(
    mut backend: B,
    tier: X11Tier,
    store: &RecoveryStore,
    display: Option<String>,
    spec: &MonitorSpec,
) -> Result<X11MonitorGuard<B>, VdisplayError> {
    let screen = backend.screen_size()?;
    let mut state = RecoveryState {
        display,
        pid: std::process::id(),
        screen,
        monitor: None,
        output: None,
        mode: None,
    };
    match tier {
        X11Tier::BareMonitor => {
            if backend
                .monitor_names()?
                .iter()
                .any(|n| n == spec.name.as_str())
            {
                return Err(VdisplayError::Request(format!(
                    "a monitor named {:?} exists already",
                    spec.name.as_str()
                )));
            }
            state.monitor = Some(spec.name.as_str().to_string());
        }
        X11Tier::RealOutput(output) => {
            state.output = Some(output.0);
            state.mode = Some(mode_name(spec));
        }
    }
    store.save(&state)?;
    if let Err(e) = apply(&mut backend, tier, &state, spec) {
        let _ = super::recovery::undo(&mut backend, &state);
        store.remove();
        return Err(e);
    }
    Ok(X11MonitorGuard {
        backend,
        state,
        store: store.clone(),
        rect: spec.rect,
        tier,
        restore_on_drop: true,
    })
}

/// The requests that make the monitor, after the screen is large enough.
fn apply<B: RandrBackend>(
    backend: &mut B,
    tier: X11Tier,
    state: &RecoveryState,
    spec: &MonitorSpec,
) -> Result<(), VdisplayError> {
    let size = grown(state.screen, spec.rect)?;
    if size != state.screen {
        backend.set_screen_size(size)?;
    }
    match tier {
        X11Tier::BareMonitor => {
            backend.set_monitor(spec.name.as_str(), spec.rect, spec.physical_mm())
        }
        X11Tier::RealOutput(output) => {
            let name = state.mode.as_deref().unwrap_or_default();
            let mode = backend.ensure_mode(name, spec.rect.width, spec.rect.height)?;
            backend.show_output(output, mode, spec.rect.x, spec.rect.y)
        }
    }
}

impl<B: RandrBackend + Send> X11MonitorGuard<B> {
    /// Leaves the monitor in place when the guard is dropped, as a killed daemon would. The
    /// recovery file stays, so a later recovery still finds it.
    pub fn leak_on_drop(&mut self) {
        self.restore_on_drop = false;
    }

    /// The tier the monitor was made with, which differs from the requested one after a
    /// fallback.
    pub fn tier(&self) -> X11Tier {
        self.tier
    }
}

impl<B: RandrBackend + Send> VirtualMonitor for X11MonitorGuard<B> {
    fn rect(&self) -> Rect {
        self.rect
    }

    fn resize(&mut self, spec: &MonitorSpec) -> Result<(), VdisplayError> {
        let size = grown(self.backend.screen_size()?, spec.rect)?;
        if size != self.backend.screen_size()? {
            self.backend.set_screen_size(size)?;
        }
        match self.tier {
            X11Tier::BareMonitor => {
                // The server refuses to redefine a monitor in place, so the old one goes first.
                self.backend.delete_monitor(spec.name.as_str())?;
                self.backend
                    .set_monitor(spec.name.as_str(), spec.rect, spec.physical_mm())?;
            }
            X11Tier::RealOutput(output) => self.resize_output(output, spec)?,
        }
        self.rect = spec.rect;
        Ok(())
    }
}

impl<B: RandrBackend + Send> X11MonitorGuard<B> {
    /// A real output gets the new mode first; the old mode is dropped once nothing uses it.
    fn resize_output(&mut self, output: OutputId, spec: &MonitorSpec) -> Result<(), VdisplayError> {
        let old = self.state.mode.clone().unwrap_or_default();
        let fresh = mode_name(spec);
        let mode = self
            .backend
            .ensure_mode(&fresh, spec.rect.width, spec.rect.height)?;
        self.backend.hide_output(output)?;
        self.backend
            .show_output(output, mode, spec.rect.x, spec.rect.y)?;
        if old != fresh {
            let _ = self.backend.drop_mode(output, &old);
        }
        self.state.mode = Some(fresh);
        self.store.save(&self.state)
    }
}

impl<B: RandrBackend + Send> Drop for X11MonitorGuard<B> {
    fn drop(&mut self) {
        if !self.restore_on_drop {
            return;
        }
        let errors = super::recovery::undo(&mut self.backend, &self.state);
        if errors.is_empty() {
            self.store.remove();
        } else {
            for error in errors {
                tracing::warn!(%error, "restoring the display layout failed; `vdisplay cleanup` retries");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::display::MonitorName;
    use std::sync::{Arc, Mutex};

    /// Records every request and refuses the ones named in `refuse`.
    #[derive(Clone)]
    struct Recorder {
        log: Arc<Mutex<Vec<String>>>,
        screen: Arc<Mutex<ScreenSize>>,
        refuse: &'static str,
    }

    impl Recorder {
        fn new(refuse: &'static str) -> Self {
            Self {
                log: Arc::default(),
                screen: Arc::new(Mutex::new(ScreenSize {
                    width: 1920,
                    height: 1080,
                    width_mm: 508,
                    height_mm: 285,
                })),
                refuse,
            }
        }

        fn note(&self, entry: String) -> Result<(), VdisplayError> {
            let verb = entry.split(' ').next().unwrap().to_string();
            self.log.lock().unwrap().push(entry);
            if verb == self.refuse {
                Err(VdisplayError::Request(format!("{verb} refused")))
            } else {
                Ok(())
            }
        }

        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    impl RandrBackend for Recorder {
        fn screen_size(&mut self) -> Result<ScreenSize, VdisplayError> {
            Ok(*self.screen.lock().unwrap())
        }
        fn set_screen_size(&mut self, size: ScreenSize) -> Result<(), VdisplayError> {
            self.note(format!("screen {}x{}", size.width, size.height))?;
            *self.screen.lock().unwrap() = size;
            Ok(())
        }
        fn monitor_names(&mut self) -> Result<Vec<String>, VdisplayError> {
            Ok(Vec::new())
        }
        fn set_monitor(
            &mut self,
            name: &str,
            rect: Rect,
            _: (u32, u32),
        ) -> Result<(), VdisplayError> {
            self.note(format!(
                "monitor {name} {}x{}+{}+{}",
                rect.width, rect.height, rect.x, rect.y
            ))
        }
        fn delete_monitor(&mut self, name: &str) -> Result<(), VdisplayError> {
            self.note(format!("delmonitor {name}"))
        }
        fn ensure_mode(&mut self, name: &str, w: u32, h: u32) -> Result<u32, VdisplayError> {
            self.note(format!("mode {w}x{h}"))?;
            let _ = name;
            Ok(7)
        }
        fn show_output(
            &mut self,
            o: OutputId,
            mode: u32,
            x: i32,
            y: i32,
        ) -> Result<(), VdisplayError> {
            self.note(format!("show {} {mode} {x},{y}", o.0))
        }
        fn hide_output(&mut self, o: OutputId) -> Result<(), VdisplayError> {
            self.note(format!("hide {}", o.0))
        }
        fn drop_mode(&mut self, o: OutputId, _: &str) -> Result<(), VdisplayError> {
            self.note(format!("dropmode {}", o.0))
        }
    }

    fn spec(x: i32, w: u32, h: u32) -> MonitorSpec {
        MonitorSpec::new(MonitorName::new("HDMI Extender"), Rect::new(x, 0, w, h))
    }

    fn store(dir: &tempfile::TempDir) -> RecoveryStore {
        RecoveryStore::new(dir.path().join("state.json"))
    }

    #[test]
    fn bare_monitor_grows_the_screen_defines_the_monitor_and_undoes_both_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let rec = Recorder::new("");
        let guard = create_monitor(
            rec.clone(),
            X11Tier::BareMonitor,
            &store,
            None,
            &spec(1920, 1280, 720),
        )
        .unwrap();
        assert_eq!(guard.rect(), Rect::new(1920, 0, 1280, 720));
        assert_eq!(
            rec.log(),
            ["screen 3200x1080", "monitor HDMI Extender 1280x720+1920+0"]
        );
        assert!(
            store.path().exists(),
            "state is recorded while the monitor exists"
        );
        drop(guard);
        assert_eq!(
            rec.log()[2..],
            ["delmonitor HDMI Extender", "screen 1920x1080"]
        );
        assert!(!store.path().exists());
    }

    #[test]
    fn a_monitor_inside_the_screen_leaves_the_screen_size_alone() {
        let dir = tempfile::tempdir().unwrap();
        let rec = Recorder::new("");
        let guard = create_monitor(
            rec.clone(),
            X11Tier::BareMonitor,
            &store(&dir),
            None,
            &spec(640, 1280, 720),
        )
        .unwrap();
        assert_eq!(rec.log(), ["monitor HDMI Extender 1280x720+640+0"]);
        drop(guard);
        assert_eq!(rec.log().len(), 2);
    }

    #[test]
    fn real_output_gets_a_mode_and_is_turned_on_then_off_again() {
        let dir = tempfile::tempdir().unwrap();
        let rec = Recorder::new("");
        let guard = create_monitor(
            rec.clone(),
            X11Tier::RealOutput(OutputId(42)),
            &store(&dir),
            None,
            &spec(1920, 1280, 720),
        )
        .unwrap();
        assert_eq!(
            rec.log(),
            ["screen 3200x1080", "mode 1280x720", "show 42 7 1920,0"]
        );
        drop(guard);
        assert_eq!(
            rec.log()[3..],
            ["hide 42", "dropmode 42", "screen 1920x1080"]
        );
    }

    #[test]
    fn a_refused_step_undoes_the_earlier_ones_and_removes_the_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let rec = Recorder::new("show");
        let result = create_monitor(
            rec.clone(),
            X11Tier::RealOutput(OutputId(42)),
            &store,
            None,
            &spec(1920, 1280, 720),
        );
        assert!(result.is_err());
        assert_eq!(
            rec.log()[3..],
            ["hide 42", "dropmode 42", "screen 1920x1080"]
        );
        assert!(!store.path().exists());
    }

    #[test]
    fn resizing_a_bare_monitor_redefines_it_and_grows_the_screen() {
        let dir = tempfile::tempdir().unwrap();
        let rec = Recorder::new("");
        let mut guard = create_monitor(
            rec.clone(),
            X11Tier::BareMonitor,
            &store(&dir),
            None,
            &spec(1920, 640, 360),
        )
        .unwrap();
        guard.resize(&spec(1920, 1600, 900)).unwrap();
        assert_eq!(guard.rect(), Rect::new(1920, 0, 1600, 900));
        assert_eq!(
            rec.log().last().unwrap(),
            "monitor HDMI Extender 1600x900+1920+0"
        );
        assert!(rec.log().contains(&"screen 3520x1080".to_string()));
    }

    #[test]
    fn resizing_a_real_output_switches_modes_and_drops_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let rec = Recorder::new("");
        let mut guard = create_monitor(
            rec.clone(),
            X11Tier::RealOutput(OutputId(42)),
            &store(&dir),
            None,
            &spec(1920, 640, 360),
        )
        .unwrap();
        guard.resize(&spec(1920, 1600, 900)).unwrap();
        let log = rec.log();
        assert_eq!(
            log[log.len() - 4..],
            [
                "mode 1600x900",
                "hide 42",
                "show 42 7 1920,0",
                "dropmode 42"
            ]
        );
    }

    #[test]
    fn a_negative_position_is_unsupported_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let rec = Recorder::new("");
        let result = create_monitor(
            rec.clone(),
            X11Tier::BareMonitor,
            &store(&dir),
            None,
            &spec(-1280, 1280, 720),
        );
        assert!(matches!(result, Err(VdisplayError::Unsupported(_))));
        assert!(!rec.log().iter().any(|l| l.starts_with("monitor")));
    }

    #[test]
    fn a_leaked_monitor_keeps_its_state_for_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let rec = Recorder::new("");
        let mut guard = create_monitor(
            rec.clone(),
            X11Tier::BareMonitor,
            &store,
            None,
            &spec(1920, 1280, 720),
        )
        .unwrap();
        guard.leak_on_drop();
        drop(guard);
        assert_eq!(rec.log().len(), 2);
        assert!(store.path().exists());
    }
}
