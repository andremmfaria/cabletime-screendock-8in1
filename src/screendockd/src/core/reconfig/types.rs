//! Types of the live settings model: the settings, the patch, the change classes and the plan.

use thiserror::Error;

use crate::protocol::display::{DisplayError, DisplayGeometry, Orientation};
use crate::utils::types::{DisplayMode, Placement, WaylandBackend, X264Preset};

/// Highest accepted capture frame rate.
pub const MAX_FPS: u32 = 240;

/// Highest accepted bit rate in kilobits per second.
pub const MAX_BITRATE_KBPS: u32 = 100_000;

/// Highest accepted number of frames between keyframes.
pub const MAX_KEYFRAME_INTERVAL: u32 = 3600;

/// Highest accepted VBV buffer in milliseconds.
pub const MAX_VBV_BUFFER_MS: u32 = 10_000;

/// Longest accepted monitor name or anchor name in bytes.
pub const MAX_NAME_LEN: usize = 64;

/// Largest absolute monitor coordinate (the X11 protocol limit for a signed 16 bit position).
pub const MAX_COORDINATE: i32 = 32_767;

/// Every setting that can change while the daemon runs. Built only from validated values, so
/// holding one means the picture size, rates and monitor placement are in range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSettings {
    /// Encoded picture size and orientation.
    pub geometry: DisplayGeometry,
    /// Capture and encode frame rate; 0 means as fast as the backend allows.
    pub fps: u32,
    pub preset: X264Preset,
    pub tune_zerolatency: bool,
    pub bitrate_kbps: u32,
    pub vbv_buffer_ms: u32,
    pub keyframe_interval: u32,
    pub mode: DisplayMode,
    pub placement: Placement,
    /// Anchor output name, or `primary`.
    pub anchor: String,
    /// Monitor position used with `Placement::At`.
    pub x: i32,
    pub y: i32,
    /// Monitor name; empty selects the default name.
    pub name: String,
    pub wayland_backend: WaylandBackend,
    pub follow_system_changes: bool,
}

/// A set of changes to [`LiveSettings`]. Fields left `None` keep their value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsPatch {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Option<Orientation>,
    pub fps: Option<u32>,
    pub preset: Option<X264Preset>,
    pub tune_zerolatency: Option<bool>,
    pub bitrate_kbps: Option<u32>,
    pub vbv_buffer_ms: Option<u32>,
    pub keyframe_interval: Option<u32>,
    pub mode: Option<DisplayMode>,
    pub placement: Option<Placement>,
    pub anchor: Option<String>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub name: Option<String>,
    pub wayland_backend: Option<WaylandBackend>,
    pub follow_system_changes: Option<bool>,
}

/// Errors from building or patching settings.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SettingsError {
    /// The picture size or orientation is not valid.
    #[error("invalid display settings: {0}")]
    Display(#[from] DisplayError),
    /// A number is outside its accepted range.
    #[error("{key} {value} is outside {min}..={max}")]
    OutOfRange {
        key: &'static str,
        value: i64,
        min: i64,
        max: i64,
    },
    /// The key names no setting.
    #[error("unknown setting {0:?}")]
    UnknownKey(String),
    /// The value cannot be read as the setting's type.
    #[error("{key} must be {expected}")]
    InvalidValue {
        key: &'static str,
        expected: &'static str,
    },
    /// A text value is not acceptable.
    #[error("{key} {reason}")]
    InvalidText {
        key: &'static str,
        reason: &'static str,
    },
}

/// How disruptive a change is, ordered from the least to the most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChangeClass {
    /// No rebuild and no dock cycle.
    Live,
    /// New encoder or pacer and a fresh keyframe, no dock cycle.
    Rebuild,
    /// Rebuild plus the dock's stop, launch and start cycle.
    DockCycle,
}

/// One step a change needs, in the order a plan lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReconfigAction {
    /// Change the encoder's rate in place.
    SetRate,
    /// Change the spacing of forced keyframes.
    SetKeyframeInterval,
    /// Enable or disable following changes made outside the daemon.
    SetFollow,
    /// Remove the virtual monitor.
    DestroyMonitor,
    /// Create the virtual monitor.
    CreateMonitor,
    /// Create the virtual monitor again with a new name or backend.
    RecreateMonitor,
    /// Change the size of the virtual monitor.
    ResizeMonitor,
    /// Build a new encoder or pacer.
    RebuildEncoder,
    /// Point capture at a new rectangle or source.
    RetargetCapture,
    /// Run the dock's stop, launch and start cycle at the new size.
    RunDockCycle,
}

/// The result of classifying a change. Built only by
/// [`plan_change`](crate::core::reconfig::plan_change).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconfigPlan {
    pub(super) class: ChangeClass,
    pub(super) actions: Vec<ReconfigAction>,
}

/// Facts about the running pipeline that change the classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanOptions {
    /// The encoder changes its rate without a rebuild.
    pub live_rate: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self { live_rate: true }
    }
}

/// A setting as the control interface names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingKey {
    Resolution,
    Orientation,
    Fps,
    Preset,
    Tune,
    Bitrate,
    VbvBufferMs,
    KeyframeInterval,
    Mode,
    Placement,
    Anchor,
    X,
    Y,
    Name,
    WaylandBackend,
    FollowSystemChanges,
}
