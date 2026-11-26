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
    /// The sway IPC socket to use; `None` uses `$SWAYSOCK`.
    pub(super) sway_socket: Option<PathBuf>,
    /// The session bus address of a Mutter to use; `None` uses `$DBUS_SESSION_BUS_ADDRESS`.
    pub(super) mutter_bus: Option<String>,
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

/// What a portal source is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalRequest {
    /// The size the virtual monitor should have; the portal may ignore it.
    pub size: (u32, u32),
    /// The token of an earlier session, so the portal can skip its prompt.
    pub restore_token: Option<String>,
}

/// A started portal stream. Dropping `keepalive` closes the portal session.
pub struct PortalStream {
    /// The PipeWire node that carries the frames.
    pub node_id: u32,
    /// The size the portal reports for the stream, when it reports one.
    pub size: Option<(u32, u32)>,
    /// The token to present next time, when the portal issued one.
    pub restore_token: Option<String>,
    pub keepalive: Box<dyn Send>,
}

/// Opens a portal ScreenCast session for a virtual source. Implementations talk to the
/// portal; the display around it adds the start timeout and the restore token handling.
pub trait PortalSource: Send + Sync {
    fn start(&self, request: &PortalRequest) -> Result<PortalStream, VdisplayError>;
}

/// The restore token file of the portal source, written with owner-only permissions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenStore {
    pub(super) path: PathBuf,
}

/// Makes virtual monitors through the portal's virtual source type.
#[derive(Clone)]
pub struct PortalVirtualDisplay {
    pub(super) source: std::sync::Arc<dyn PortalSource>,
    pub(super) timeout: std::time::Duration,
    pub(super) tokens: Option<TokenStore>,
}

/// A monitor backed by a portal stream. Dropping it closes the portal session.
pub struct PortalMonitor {
    pub(super) rect: Rect,
    pub(super) node_id: u32,
    pub(super) _stream: Box<dyn Send>,
}

/// How long the portal gets to start a stream before the display gives up.
pub const DEFAULT_PORTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// Longest restore token read back from the token file.
pub(super) const MAX_TOKEN_LEN: usize = 256;
/// File name of the restore token below the state directory.
pub(super) const TOKEN_FILE: &str = "portal-restore-token";

/// The portal source that talks to `xdg-desktop-portal` over D-Bus.
#[cfg(feature = "capture-wayland")]
#[derive(Debug, Clone, Copy, Default)]
pub struct AshpdPortalSource;

/// A connection to the sway IPC socket (the i3 IPC framing).
#[derive(Debug)]
pub struct SwayIpc {
    pub(super) stream: std::os::unix::net::UnixStream,
}

/// One output as `GET_OUTPUTS` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwayOutput {
    pub name: String,
    pub rect: Rect,
    pub active: bool,
}

/// Makes virtual monitors as headless outputs of a wlroots compositor (sway), through its IPC
/// socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WlrootsHeadlessDisplay {
    pub(super) socket: PathBuf,
}

/// A headless output. Dropping it unplugs the output.
#[derive(Debug)]
pub struct WlrootsMonitor {
    pub(super) socket: PathBuf,
    pub(super) ipc: SwayIpc,
    pub(super) name: String,
    pub(super) rect: Rect,
}

/// Magic bytes that start every IPC message.
pub(super) const IPC_MAGIC: &[u8; 6] = b"i3-ipc";
/// Message types of the IPC requests the adapter sends.
pub(super) const IPC_RUN_COMMAND: u32 = 0;
pub(super) const IPC_GET_OUTPUTS: u32 = 3;
/// Largest reply payload read.
pub(super) const IPC_MAX_PAYLOAD: usize = 1 << 20;
/// How long an IPC read or write may block.
pub(super) const IPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// How long a changed output gets to show the new geometry.
pub(super) const OUTPUT_SETTLE: std::time::Duration = std::time::Duration::from_secs(3);

/// The calls the Mutter display makes on `org.gnome.Mutter.ScreenCast`, so the order of the
/// calls can be tested against a recording double as well as a real shell.
pub trait MutterCalls: Send {
    /// `CreateSession`; returns the object path of the session.
    fn create_session(&mut self) -> Result<String, VdisplayError>;

    /// `RecordVirtual` on the session; returns the object path of the stream.
    fn record_virtual(&mut self, session: &str) -> Result<String, VdisplayError>;

    /// Listens for `PipeWireStreamAdded` on `stream`, calls `Start` on the session and waits
    /// up to `timeout` for the signal; returns the PipeWire node id it carries.
    fn start(
        &mut self,
        session: &str,
        stream: &str,
        timeout: std::time::Duration,
    ) -> Result<u32, VdisplayError>;

    /// `Stop` on the session; failures are logged, not returned.
    fn stop(&mut self, session: &str);
}

/// Opens a connection to the Mutter ScreenCast service.
pub trait MutterConnector: Send + Sync {
    fn connect(&self) -> Result<Box<dyn MutterCalls>, VdisplayError>;
}

/// Makes virtual monitors through `org.gnome.Mutter.ScreenCast` `RecordVirtual`.
///
/// The session belongs to the D-Bus connection: Mutter removes the stream when the connection
/// closes, so a killed daemon leaves nothing behind.
#[derive(Clone)]
pub struct MutterVirtualDisplay {
    pub(super) connector: std::sync::Arc<dyn MutterConnector>,
    pub(super) timeout: std::time::Duration,
}

/// A monitor backed by a Mutter stream. Dropping it stops the session.
pub struct MutterMonitor {
    pub(super) calls: Box<dyn MutterCalls>,
    pub(super) session: String,
    pub(super) node_id: u32,
    pub(super) rect: Rect,
}

/// How long Mutter gets to announce the PipeWire node after `Start`.
pub const DEFAULT_MUTTER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// The bus name, object path and interfaces of the Mutter ScreenCast service.
#[cfg(feature = "capture-wayland")]
pub(super) const MUTTER_SERVICE: &str = "org.gnome.Mutter.ScreenCast";
#[cfg(feature = "capture-wayland")]
pub(super) const MUTTER_PATH: &str = "/org/gnome/Mutter/ScreenCast";
#[cfg(feature = "capture-wayland")]
pub(super) const MUTTER_SESSION_INTERFACE: &str = "org.gnome.Mutter.ScreenCast.Session";
/// `cursor-mode` value of `RecordVirtual` that paints the cursor into the frames.
#[cfg(feature = "capture-wayland")]
pub(super) const CURSOR_EMBEDDED: u32 = 1;
#[cfg(feature = "capture-wayland")]
pub(super) const MUTTER_STREAM_INTERFACE: &str = "org.gnome.Mutter.ScreenCast.Stream";

/// Connects to the session bus that `address` names, or to `$DBUS_SESSION_BUS_ADDRESS`.
#[cfg(feature = "capture-wayland")]
#[derive(Debug, Clone, Default)]
pub struct ZbusMutterConnector {
    pub(super) address: Option<String>,
}

/// The Mutter ScreenCast calls over zbus, driven on a private single threaded runtime.
#[cfg(feature = "capture-wayland")]
pub(super) struct ZbusMutterCalls {
    pub(super) runtime: tokio::runtime::Runtime,
    pub(super) connection: zbus::Connection,
}
