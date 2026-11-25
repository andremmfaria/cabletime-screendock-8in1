//! Types of the transport adapters: hidraw node paths, device ids, discovery errors and the
//! hidraw handle.

use std::fs::File;
use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::protocol::framing::ReportId;

/// The dock's USB vendor ID, lowercase hex, as it appears in sysfs `idVendor`.
pub const VENDOR_ID_HEX: &str = "1de1";

/// The dock's USB product ID, lowercase hex, as it appears in sysfs `idProduct`.
pub const PRODUCT_ID_HEX: &str = "f201";

/// MI_00's USB interface number (control/heartbeat, HID Report ID 1).
pub const MI00_INTERFACE_NUMBER: u8 = 0;

/// MI_01's USB interface number (video, HID Report ID 2).
pub const MI01_INTERFACE_NUMBER: u8 = 1;

/// Default sysfs directory to walk for hidraw class devices.
pub const DEFAULT_SYSFS_HIDRAW_BASE: &str = "/sys/class/hidraw";

/// Default directory the resolved device-node paths are constructed under.
pub const DEFAULT_DEV_DIR: &str = "/dev";

/// The resolved `/dev/hidrawN` path for MI_00 (control/heartbeat channel, Report ID 1).
///
/// A distinct newtype rather than a bare `PathBuf` so this path can never be passed where a
/// `Mi01Path` is expected (or vice versa) without an explicit, deliberate conversion — the
/// compiler catches a swapped-channel bug that would otherwise only surface as a silently
/// miswired heartbeat/video channel on real hardware.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mi00Path(pub(super) PathBuf);

/// The resolved `/dev/hidrawN` path for MI_01 (video channel, Report ID 2). See `Mi00Path` for
/// why this is a distinct newtype rather than a bare `PathBuf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mi01Path(pub(super) PathBuf);

impl Deref for Mi00Path {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Deref for Mi01Path {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Mi00Path {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Mi01Path {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

/// Resolved `/dev/hidrawN` paths for the dock's two HID interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockHidrawPaths {
    /// MI_00: control/heartbeat channel (Report ID 1).
    pub mi00: Mi00Path,
    /// MI_01: video channel (Report ID 2).
    pub mi01: Mi01Path,
}

/// Errors from resolving the dock's hidraw nodes.
///
/// Uses `thiserror`'s derive rather than hand-written `Display`/`Error` impls, the de facto
/// convention for a module exposing matchable error variants to its callers.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    /// The sysfs hidraw class directory couldn't be read at all (e.g. doesn't exist, no
    /// permission) — distinct from "no matching device found".
    #[error("couldn't read hidraw class dir {path:?}: {source}")]
    HidrawClassDirUnreadable { path: PathBuf, source: io::Error },
    /// Walked every hidraw entry and found neither interface 0 nor interface 1 on a device
    /// matching the dock's VID/PID.
    #[error("no hidraw device matching the configured VID:PID found")]
    NoDockInterfacesFound,
    /// A configured vendor or product ID is not four hex digits.
    #[error("device.{field} {value:?} is not a four digit hex number")]
    InvalidDeviceId { field: &'static str, value: String },
    /// Found MI_01 (interface 1) on a matching device, but not MI_00 (interface 0).
    #[error("found the dock's MI_01 (video) but not its MI_00 (control/heartbeat) interface")]
    Mi00NotFound,
    /// Found MI_00 (interface 0) on a matching device, but not MI_01 (interface 1).
    #[error("found the dock's MI_00 (control/heartbeat) but not its MI_01 (video) interface")]
    Mi01NotFound,
}

/// USB vendor and product ID to match, as lowercase four digit hex strings. Defaults to the
/// dock's real IDs; the `[device]` config section can override them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIds {
    pub(super) vendor: String,
    pub(super) product: String,
}

impl Default for DeviceIds {
    fn default() -> Self {
        Self {
            vendor: VENDOR_ID_HEX.to_string(),
            product: PRODUCT_ID_HEX.to_string(),
        }
    }
}

/// An open hidraw node for one of the dock's two channels.
///
/// The channel (`ReportId::Control` for MI_00, `ReportId::Video` for MI_01) is fixed at open
/// time and every written chunk is checked against it, so a video chunk can never be written to
/// the control node (or vice versa) by accident.
#[derive(Debug)]
pub struct HidrawHandle {
    pub(super) file: File,
    pub(super) channel: ReportId,
}
