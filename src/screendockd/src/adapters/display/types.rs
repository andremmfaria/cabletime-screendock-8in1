//! Types of the display adapters.

use crate::ports::display::Rect;

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
}
