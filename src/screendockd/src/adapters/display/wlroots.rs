//! The wlroots display: a headless output of sway, created, sized and placed through the
//! sway IPC socket and unplugged when the monitor is dropped.
//!
//! sway chooses the name of a new output (`HEADLESS-<n>`), so the monitor name of the spec
//! does not apply. A headless output keeps existing until it is unplugged or sway exits, so a
//! daemon that is killed leaves its output behind.

use std::path::PathBuf;
use std::time::Instant;

use super::sway_ipc::is_valid_output_name;
use super::types::{MonitorWatcher, SwayIpc, SwayOutputEvents, WatchTarget, OUTPUT_SETTLE};
pub use super::types::{WlrootsHeadlessDisplay, WlrootsMonitor};
use crate::ports::display::{
    DisplayEventSink, MonitorSpec, Rect, VdisplayError, VirtualDisplay, VirtualMonitor,
};

impl WlrootsHeadlessDisplay {
    /// A display that talks to the sway whose IPC socket is at `socket`.
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    /// A display for the sway named by `$SWAYSOCK`.
    pub fn from_env() -> Result<Self, VdisplayError> {
        std::env::var_os("SWAYSOCK")
            .filter(|v| !v.is_empty())
            .map(Self::new)
            .ok_or_else(|| VdisplayError::Unsupported("$SWAYSOCK is not set".into()))
    }
}

/// Sets the mode and position of `name` and waits until sway reports the new geometry.
fn apply(ipc: &mut SwayIpc, name: &str, rect: Rect) -> Result<(), VdisplayError> {
    ipc.run(&format!(
        "output {name} mode {}x{} pos {} {}",
        rect.width, rect.height, rect.x, rect.y
    ))?;
    let start = Instant::now();
    loop {
        let current = ipc
            .outputs()?
            .into_iter()
            .find(|o| o.name == name)
            .ok_or_else(|| VdisplayError::Request(format!("output {name} disappeared")))?;
        if current.rect == rect {
            return Ok(());
        }
        if start.elapsed() > OUTPUT_SETTLE {
            return Err(VdisplayError::Request(format!(
                "output {name} is {}x{} at {},{}, not the requested {}x{} at {},{}",
                current.rect.width,
                current.rect.height,
                current.rect.x,
                current.rect.y,
                rect.width,
                rect.height,
                rect.x,
                rect.y
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn unplug(ipc: &mut SwayIpc, name: &str) {
    if let Err(e) = ipc.run(&format!("output {name} unplug")) {
        tracing::warn!(output = name, error = %e, "could not unplug the headless output");
    }
}

impl VirtualDisplay for WlrootsHeadlessDisplay {
    fn name(&self) -> &'static str {
        "wlroots"
    }

    fn create(&self, spec: &MonitorSpec) -> Result<Box<dyn VirtualMonitor>, VdisplayError> {
        let mut ipc = SwayIpc::connect(&self.socket)?;
        let before: Vec<String> = ipc.outputs()?.into_iter().map(|o| o.name).collect();
        ipc.run("create_output")?;
        let created: Vec<String> = ipc
            .outputs()?
            .into_iter()
            .map(|o| o.name)
            .filter(|n| !before.contains(n))
            .collect();
        let name = match created.as_slice() {
            [name] if is_valid_output_name(name) => name.clone(),
            [] => {
                return Err(VdisplayError::Request(
                    "sway made no output for create_output".into(),
                ))
            }
            other => {
                // Not ours to guess at: leave any output that appeared alone.
                return Err(VdisplayError::Request(format!(
                    "expected one new output, found {}",
                    other.len()
                )));
            }
        };
        if let Err(e) = apply(&mut ipc, &name, spec.rect) {
            unplug(&mut ipc, &name);
            return Err(e);
        }
        Ok(Box::new(WlrootsMonitor {
            socket: self.socket.clone(),
            ipc,
            name,
            rect: spec.rect,
            expected: std::sync::Arc::new(std::sync::Mutex::new(Some(spec.rect))),
            watcher: None,
        }))
    }
}

impl VirtualMonitor for WlrootsMonitor {
    fn rect(&self) -> Rect {
        self.rect
    }

    fn resize(&mut self, spec: &MonitorSpec) -> Result<(), VdisplayError> {
        // The watcher must expect the new rectangle before sway reports it.
        self.expect(spec.rect);
        if let Err(error) = apply(&mut self.ipc, &self.name, spec.rect) {
            self.expect(self.rect);
            return Err(error);
        }
        self.rect = spec.rect;
        Ok(())
    }

    fn follow(&mut self, notify: DisplayEventSink) {
        self.watcher = None;
        let started = SwayOutputEvents::connect(&self.socket).and_then(|source| {
            MonitorWatcher::spawn(
                source,
                WatchTarget::Monitor(self.name.clone()),
                self.expected.clone(),
                notify,
            )
            .map_err(|e| VdisplayError::Request(e.to_string()))
        });
        match started {
            Ok(watcher) => self.watcher = Some(watcher),
            Err(error) => tracing::warn!(%error, "not following changes to the sway output"),
        }
    }

    fn observe(&mut self, rect: Rect) {
        self.rect = rect;
        self.expect(rect);
    }
}

impl WlrootsMonitor {
    fn expect(&self, rect: Rect) {
        *self.expected.lock().unwrap_or_else(|e| e.into_inner()) = Some(rect);
    }
}

impl Drop for WlrootsMonitor {
    fn drop(&mut self) {
        // Stop watching first: unplugging the output must not look like an outside change.
        self.watcher = None;
        match SwayIpc::connect(&self.socket) {
            Ok(mut ipc) => unplug(&mut ipc, &self.name),
            Err(e) => {
                tracing::warn!(output = %self.name, error = %e, "sway is gone; output not unplugged")
            }
        }
    }
}
