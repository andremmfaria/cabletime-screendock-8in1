//! Types of the utils layer: the configuration structs and errors, plus the plain value types
//! shared by several layers (picture size newtypes).

use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

/// Picture width in pixels, as carried in header bytes 8..11.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelWidth(pub u32);

/// Picture height in pixels, as carried in header bytes 12..15.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelHeight(pub u32);

/// Picture size carried in the header of a video message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureSize {
    pub width: PixelWidth,
    pub height: PixelHeight,
}

impl PictureSize {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: PixelWidth(width),
            height: PixelHeight(height),
        }
    }

    /// The size field values of a control message: both zero.
    pub fn zero() -> Self {
        Self::new(0, 0)
    }
}

/// Subdirectory name used under both `$XDG_CONFIG_HOME` and `/etc` for this daemon's config.
pub(super) const CONFIG_DIR_NAME: &str = "screendockd";

/// Config file name, consistent across every resolved location.
pub(super) const CONFIG_FILE_NAME: &str = "screendock_cfg.toml";

/// Environment variable that, if set, overrides every other resolution step.
pub(super) const CONFIG_PATH_ENV_VAR: &str = "SCREENDOCK_CONFIG_PATH";

/// System-wide fallback config path, as shipped by the `.deb`/`.rpm` packaging asset lists.
pub(super) const SYSTEM_CONFIG_PATH: &str = "/etc/screendockd/screendock_cfg.toml";

/// Display/capture target settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DisplayConfig {
    /// Target capture/output width in pixels.
    pub width: u32,
    /// Target capture/output height in pixels.
    pub height: u32,
    /// `"landscape"` or `"portrait"`.
    pub orientation: String,
}

/// Whether the dock shows a copy of an existing screen or its own monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayMode {
    /// Capture an existing screen or region and scale it to the dock.
    #[default]
    Mirror,
    /// Create a virtual monitor for the dock and capture exactly that monitor.
    Extend,
}

/// Where the virtual monitor sits relative to its anchor output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub enum Placement {
    #[default]
    #[serde(rename = "right-of")]
    RightOf,
    #[serde(rename = "left-of")]
    LeftOf,
    #[serde(rename = "above")]
    Above,
    #[serde(rename = "below")]
    Below,
    /// At the configured `x` and `y`.
    #[serde(rename = "at")]
    At,
}

/// Which Wayland mechanism provides the virtual monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WaylandBackend {
    /// Pick by the compositor: Mutter on GNOME, wlroots headless output on sway, else the portal.
    #[default]
    Auto,
    Mutter,
    Portal,
    Wlroots,
}

/// Which capture backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureBackendChoice {
    /// Follow the detected session type (`$XDG_SESSION_TYPE`, `$WAYLAND_DISPLAY`, `$DISPLAY`).
    #[default]
    Auto,
    /// XShm capture of an X11 display.
    X11,
    /// xdg-desktop-portal ScreenCast over PipeWire (needs the `capture-wayland` build feature).
    Wayland,
}

/// Screen capture settings. Every field is optional in the file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    /// Backend selection; `auto` follows the detected session.
    pub backend: CaptureBackendChoice,
    /// X11 display to capture, for example `":0"`. Unset means `$DISPLAY`.
    pub x11_display: Option<String>,
    /// Capture frame rate. Zero captures as fast as the backend allows.
    pub fps: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            backend: CaptureBackendChoice::Auto,
            x11_display: None,
            fps: 25,
        }
    }
}

/// libx264 speed preset. Slower presets spend more CPU for better quality at one bit rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum X264Preset {
    Ultrafast,
    Superfast,
    Veryfast,
    Faster,
    Fast,
    Medium,
}

/// H.264 encoder settings. Every field is optional in the file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct EncodeConfig {
    /// libx264 preset.
    pub preset: X264Preset,
    /// Apply libx264's `zerolatency` tune (no lookahead, no frame delay).
    pub tune_zerolatency: bool,
    /// Target bit rate in kilobits per second. It is also the VBV maximum rate.
    pub bitrate_kbps: u32,
    /// Frames between keyframes.
    pub keyframe_interval: u32,
    /// VBV buffer size in milliseconds of `bitrate_kbps`. It bounds how large one frame can
    /// get, keyframes included. 0 turns the VBV cap off.
    pub vbv_buffer_ms: u32,
}

impl Default for EncodeConfig {
    fn default() -> Self {
        Self {
            preset: X264Preset::Ultrafast,
            tune_zerolatency: true,
            bitrate_kbps: 4000,
            keyframe_interval: 50,
            vbv_buffer_ms: 100,
        }
    }
}

/// What the daemon does when the virtual monitor cannot be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FallbackPolicy {
    /// Continue in mirror mode and report the reason.
    #[default]
    Mirror,
    /// End startup with an error.
    Fail,
}

/// Where the virtual monitor's identity data comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdidSource {
    /// The dock's own EDID, with synthetic values when the dock sent none.
    #[default]
    Dock,
    /// Always synthetic values.
    Synthetic,
}

/// Virtual monitor settings. Every field is optional in the file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct VirtualDisplayConfig {
    /// `mirror` captures an existing screen; `extend` creates a monitor for the dock.
    pub mode: DisplayMode,
    /// Where the monitor sits relative to `anchor`.
    pub placement: Placement,
    /// Output name the placement is relative to, or `primary`.
    pub anchor: String,
    /// Monitor position when `placement` is `at`.
    pub x: i32,
    pub y: i32,
    /// Monitor name; empty selects the name from the dock's EDID, else `ScreenDock`.
    pub name: String,
    /// What to do when the monitor cannot be created.
    pub fallback: FallbackPolicy,
    /// Wayland mechanism that provides the monitor.
    pub wayland_backend: WaylandBackend,
    /// Follow size, rotation and position changes made outside the daemon.
    pub follow_system_changes: bool,
    /// Where the monitor's identity data comes from.
    pub edid: EdidSource,
    /// Restore the previous layout when the daemon exits.
    pub restore_on_exit: bool,
}

impl Default for VirtualDisplayConfig {
    fn default() -> Self {
        Self {
            mode: DisplayMode::Mirror,
            placement: Placement::RightOf,
            anchor: "primary".into(),
            x: 0,
            y: 0,
            name: String::new(),
            fallback: FallbackPolicy::Mirror,
            wayland_backend: WaylandBackend::Auto,
            follow_system_changes: true,
            edid: EdidSource::Dock,
            restore_on_exit: true,
        }
    }
}

/// Control socket settings. Every field is optional in the file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ControlConfig {
    /// Serve the control socket while the daemon runs.
    pub enabled: bool,
    /// Socket path; empty selects `$XDG_RUNTIME_DIR/screendockd/control.sock`.
    pub socket: String,
}

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            socket: String::new(),
        }
    }
}

/// Daemon-level runtime settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DaemonConfig {
    /// Default log level (`"trace"`/`"debug"`/`"info"`/`"warn"`/`"error"`), used by
    /// `utils::logging::init_logging` unless overridden by the `RUST_LOG` environment variable.
    pub log_level: String,
    /// Heartbeat tick interval for the MI_00 control channel, in milliseconds.
    pub heartbeat_interval_ms: u64,
}

/// Device identification override, for testing against a non-standard hidraw setup without
/// recompiling.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceConfig {
    /// Lowercase hex USB vendor ID, overriding `discovery::VENDOR_ID_HEX`.
    pub vendor_id_hex: String,
    /// Lowercase hex USB product ID, overriding `discovery::PRODUCT_ID_HEX`.
    pub product_id_hex: String,
}

/// Top-level daemon configuration, deserialized directly from `screendock_cfg.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Config {
    pub display: DisplayConfig,
    #[serde(default)]
    pub capture: CaptureConfig,
    #[serde(default)]
    pub encode: EncodeConfig,
    #[serde(default)]
    pub virtual_display: VirtualDisplayConfig,
    #[serde(default)]
    pub control: ControlConfig,
    pub daemon: DaemonConfig,
    pub device: DeviceConfig,
}

/// Errors from resolving or loading the config file.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// None of the four resolution steps found an existing file.
    #[error(
        "no config file found: checked ${CONFIG_PATH_ENV_VAR}, $XDG_CONFIG_HOME/{CONFIG_DIR_NAME}/{CONFIG_FILE_NAME}, {SYSTEM_CONFIG_PATH}, and the repo-root dev path"
    )]
    NotFound,
    /// The resolved file existed but couldn't be read.
    #[error("couldn't read config file {path:?}: {source}")]
    Unreadable {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The resolved file's contents weren't valid TOML matching `Config`'s shape.
    #[error("couldn't parse config file {path:?} as TOML: {source}")]
    InvalidToml {
        path: PathBuf,
        source: toml::de::Error,
    },
}

/// Environment variable `tracing-subscriber`'s `EnvFilter` reads for a level override - the
/// same `RUST_LOG` convention `log`/`env_logger` users already expect, so this isn't a new
/// environment variable to document.
pub(super) const LOG_LEVEL_ENV_VAR: &str = "RUST_LOG";
