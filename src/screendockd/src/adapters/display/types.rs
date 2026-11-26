//! Types of the display adapters.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ports::display::{OutputId, Rect, VdisplayError, X11Tier};

/// Shows an existing output instead of creating a monitor: the monitor is the output's own
/// rectangle and nothing is created or removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirrorDisplay {
    pub(super) rect: Rect,
}

/// The monitor a [`MirrorDisplay`] hands out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirrorMonitor {
    pub(super) rect: Rect,
}

/// Builds displays for the desktop the daemon runs in.
#[derive(Debug, Clone, Default)]
pub struct SystemDisplayProvider {
    /// The X display to talk to; `None` uses `$DISPLAY`.
    pub(super) x11_display: Option<String>,
    /// Where the recovery file goes; `None` uses `$XDG_RUNTIME_DIR`.
    pub(super) recovery: Option<RecoveryStore>,
}

/// Size of the X screen (the framebuffer) in pixels and millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenSize {
    pub width: u32,
    pub height: u32,
    pub width_mm: u32,
    pub height_mm: u32,
}

/// The RandR requests the virtual monitor code needs, so the tier logic runs against a
/// recording double as well as a real X server.
pub trait RandrBackend {
    /// The current screen size.
    fn screen_size(&mut self) -> Result<ScreenSize, VdisplayError>;

    /// Changes the screen size.
    fn set_screen_size(&mut self, size: ScreenSize) -> Result<(), VdisplayError>;

    /// Names of the RandR monitors that exist.
    fn monitor_names(&mut self) -> Result<Vec<String>, VdisplayError>;

    /// Defines (or replaces) a bare monitor.
    fn set_monitor(
        &mut self,
        name: &str,
        rect: Rect,
        size_mm: (u32, u32),
    ) -> Result<(), VdisplayError>;

    /// Removes a bare monitor; a missing one is not an error.
    fn delete_monitor(&mut self, name: &str) -> Result<(), VdisplayError>;

    /// The id of the mode called `name`, created at `width` by `height` when it does not
    /// exist.
    fn ensure_mode(&mut self, name: &str, width: u32, height: u32) -> Result<u32, VdisplayError>;

    /// Offers `mode` on `output` and turns the output on at `x`, `y` through a free CRTC.
    fn show_output(
        &mut self,
        output: OutputId,
        mode: u32,
        x: i32,
        y: i32,
    ) -> Result<(), VdisplayError>;

    /// Turns `output` off.
    fn hide_output(&mut self, output: OutputId) -> Result<(), VdisplayError>;

    /// Removes the mode called `name` from `output` and from the server; missing pieces are
    /// not an error.
    fn drop_mode(&mut self, output: OutputId, name: &str) -> Result<(), VdisplayError>;
}

/// [`RandrBackend`] over a real X connection.
pub struct XRandr {
    pub(super) conn: x11rb::rust_connection::RustConnection,
    pub(super) root: x11rb::protocol::xproto::Window,
}

/// Makes virtual monitors on an X server through RandR: a real output given a custom mode
/// (tier 1) or a bare monitor over an enlarged screen (tier 2).
#[derive(Debug, Clone)]
pub struct X11RandrDisplay {
    pub(super) display: Option<String>,
    pub(super) tier: X11Tier,
    pub(super) store: RecoveryStore,
}

/// What creating a monitor changed, as the guard and the recovery file both record it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryState {
    /// The X display the changes were made on; `None` means `$DISPLAY`.
    pub display: Option<String>,
    /// The daemon that made them.
    pub pid: u32,
    /// The screen size before the first change.
    pub screen: ScreenSize,
    /// The bare monitor that was or is about to be created.
    pub monitor: Option<String>,
    /// The output that was or is about to be turned on, and the name of its mode.
    pub output: Option<u32>,
    pub mode: Option<String>,
}

/// The file that records an unfinished change, so a killed daemon's changes can be undone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryStore {
    pub(super) path: PathBuf,
}

/// What [`recover`](super::recovery::recover) found and did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryOutcome {
    /// No state file exists.
    NothingToDo,
    /// The recorded changes were undone and the file removed.
    Restored,
    /// The daemon that wrote the file is still running, so nothing was touched.
    StillRunning(u32),
}

/// A monitor made through RandR. Dropping it undoes the change.
pub struct X11MonitorGuard<B: RandrBackend + Send> {
    pub(super) backend: B,
    pub(super) state: RecoveryState,
    pub(super) store: RecoveryStore,
    pub(super) rect: Rect,
    pub(super) tier: X11Tier,
    pub(super) restore_on_drop: bool,
}

/// Directory and file name of the recovery file under `$XDG_RUNTIME_DIR`.
pub(super) const RECOVERY_DIR: &str = "screendockd";
pub(super) const RECOVERY_FILE: &str = "vdisplay-state.json";
/// Largest recovery file read back.
pub(super) const RECOVERY_MAX_BYTES: u64 = 64 * 1024;
/// Prefix of the modes the daemon creates, so they are recognised on recovery.
pub(super) const MODE_PREFIX: &str = "screendockd-";
