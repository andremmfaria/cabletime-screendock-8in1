//! Device discovery: resolving the dock's MI_00 and MI_01 `/dev/hidrawN` nodes by sysfs
//! interface number, rather than by hidraw enumeration order (which isn't guaranteed stable
//! across reboots or across hosts — see `docs/implementation-plan.md`'s "Device identification
//! ambiguity" risk).
//!
//! The dock enumerates as composite USB `VID_1DE1&PID_F201`, with MI_00 (HID interface number 0,
//! Report ID 1) and MI_01 (HID interface number 1, Report ID 2) sharing that VID/PID with MI_02
//! (Audio) and MI_04 (Mass Storage) — both stock-class devices this driver never touches. Per
//! `docs/implementation-plan.md`'s "Device Discovery and Permissions" section, the resolution
//! approach is: walk `/sys/class/hidraw/hidraw*/device/` symlinks back to each hidraw node's
//! parent USB interface, read that interface's `bInterfaceNumber`, and match interface number 0
//! to MI_00 and interface number 1 to MI_01 — cross-checking `idVendor`/`idProduct` on the
//! shared USB device ancestor so an unrelated vendor-HID device elsewhere on the host is never
//! mistaken for this dock.
//!
//! Sysfs layout relied on (standard Linux USB/HID sysfs structure, not device-specific):
//! `/sys/class/hidraw/hidrawN/device` is a symlink to the HID device node (e.g.
//! `.../1-2:1.0/0003:1DE1:F201.0001`); that node's parent directory is the USB interface
//! (`1-2:1.0`, which exposes `bInterfaceNumber`); the interface's parent directory is the USB
//! device (`1-2`, which exposes `idVendor`/`idProduct`).
//!
//! This module is a from-scratch reimplementation based solely on the above documented sysfs
//! layout (per `docs/implementation-plan.md`'s clean-room rule) — it does not reference, and was
//! not derived from, any decompiled vendor code; the sysfs layout itself is standard Linux
//! kernel/udev behavior, not anything vendor-specific.
//!
//! Per `docs/implementation-plan.md`'s Design Patterns section (pattern 2, newtype), `Mi00Path`
//! and `Mi01Path` are distinct types rather than two same-typed `PathBuf` fields, so a future
//! call site that accidentally swaps the control and video paths (e.g. opening MI_00 where
//! MI_01 was intended) is a compile error, not a runtime bug discovered on real hardware. Per
//! pattern 7, `DiscoveryError` uses `thiserror`'s derive instead of hand-written `Display`/
//! `Error` impls, with no change to the variant shapes or matching behavior.

use std::fs;
use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use thiserror::Error;

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
pub struct Mi00Path(PathBuf);

/// The resolved `/dev/hidrawN` path for MI_01 (video channel, Report ID 2). See `Mi00Path` for
/// why this is a distinct newtype rather than a bare `PathBuf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mi01Path(PathBuf);

impl Mi00Path {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn into_inner(self) -> PathBuf {
        self.0
    }
}

impl Mi01Path {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn into_inner(self) -> PathBuf {
        self.0
    }
}

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
/// Per `docs/implementation-plan.md`'s Design Patterns section (pattern 7), this uses
/// `thiserror`'s derive rather than hand-written `Display`/`Error` impls — the de facto
/// convention for a module exposing matchable error variants to its callers. Variant shapes are
/// unchanged from the pre-refactor version; only the boilerplate moved.
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
    vendor: String,
    product: String,
}

impl Default for DeviceIds {
    fn default() -> Self {
        Self {
            vendor: VENDOR_ID_HEX.to_string(),
            product: PRODUCT_ID_HEX.to_string(),
        }
    }
}

fn normalize_id(field: &'static str, value: &str) -> Result<String, DiscoveryError> {
    let trimmed = value.trim().trim_start_matches("0x").to_ascii_lowercase();
    if trimmed.len() == 4 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(trimmed)
    } else {
        Err(DiscoveryError::InvalidDeviceId {
            field,
            value: value.to_string(),
        })
    }
}

impl DeviceIds {
    /// Validates and normalizes the `[device]` config section.
    pub fn from_config(device: &crate::config::DeviceConfig) -> Result<Self, DiscoveryError> {
        Ok(Self {
            vendor: normalize_id("vendor_id_hex", &device.vendor_id_hex)?,
            product: normalize_id("product_id_hex", &device.product_id_hex)?,
        })
    }
}

/// Reads a sysfs attribute file and returns its trimmed contents, or `None` on any I/O/parse
/// problem. Discovery treats a missing/unreadable attribute on a given hidraw entry as "this
/// entry isn't a usable match", not as a hard error for the whole scan — a malformed or
/// unrelated entry elsewhere on the system shouldn't abort discovery of the real device.
fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Resolves the dock's MI_00/MI_01 hidraw nodes by walking `sysfs_hidraw_base` (normally
/// `/sys/class/hidraw`) and constructing final device paths under `dev_dir` (normally `/dev`).
///
/// Both base directories are parameters (rather than hardcoded `/sys`/`/dev` literals) so the
/// matching logic can be exercised against a synthetic sysfs tree in tests, without needing a
/// real kernel or real hardware. See this module's `tests` for a constructed example tree.
pub fn discover_dock_hidraw(
    sysfs_hidraw_base: &Path,
    dev_dir: &Path,
) -> Result<DockHidrawPaths, DiscoveryError> {
    discover_with_ids(sysfs_hidraw_base, dev_dir, &DeviceIds::default())
}

/// Like [`discover_dock_hidraw`], matching the given vendor/product IDs instead of the dock's.
pub fn discover_with_ids(
    sysfs_hidraw_base: &Path,
    dev_dir: &Path,
    ids: &DeviceIds,
) -> Result<DockHidrawPaths, DiscoveryError> {
    let entries = fs::read_dir(sysfs_hidraw_base).map_err(|source| {
        DiscoveryError::HidrawClassDirUnreadable {
            path: sysfs_hidraw_base.to_path_buf(),
            source,
        }
    })?;

    let mut mi00: Option<PathBuf> = None;
    let mut mi01: Option<PathBuf> = None;

    for entry in entries.flatten() {
        let hidraw_name = entry.file_name();
        let hidraw_name = hidraw_name.to_string_lossy();

        // "device" is a symlink from the hidraw class entry to the underlying HID device node.
        // fs::canonicalize resolves it to an absolute, symlink-free path we can walk parents of.
        let hid_device_path = match fs::canonicalize(entry.path().join("device")) {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Parent of the HID device node is the USB interface directory (exposes
        // bInterfaceNumber); its parent in turn is the USB device directory (exposes
        // idVendor/idProduct).
        let interface_dir = match hid_device_path.parent() {
            Some(p) => p,
            None => continue,
        };
        let usb_device_dir = match interface_dir.parent() {
            Some(p) => p,
            None => continue,
        };

        let interface_number_raw = match read_trimmed(&interface_dir.join("bInterfaceNumber")) {
            Some(s) => s,
            None => continue,
        };
        let interface_number = match u8::from_str_radix(&interface_number_raw, 16) {
            Ok(n) => n,
            Err(_) => continue,
        };

        let vendor_id = match read_trimmed(&usb_device_dir.join("idVendor")) {
            Some(s) => s.to_lowercase(),
            None => continue,
        };
        let product_id = match read_trimmed(&usb_device_dir.join("idProduct")) {
            Some(s) => s.to_lowercase(),
            None => continue,
        };

        if vendor_id != ids.vendor || product_id != ids.product {
            // A real device on this interface number, just not this one — e.g. an unrelated
            // vendor-HID device on the system. Cross-checking VID/PID here is exactly what
            // prevents that from being mismatched onto MI_00/MI_01.
            continue;
        }

        let device_node_path = dev_dir.join(hidraw_name.as_ref());
        match interface_number {
            n if n == MI00_INTERFACE_NUMBER => mi00 = Some(device_node_path),
            n if n == MI01_INTERFACE_NUMBER => mi01 = Some(device_node_path),
            _ => {} // MI_02 (Audio), MI_04 (Mass Storage), or anything else: not our concern.
        }
    }

    match (mi00, mi01) {
        (Some(mi00), Some(mi01)) => Ok(DockHidrawPaths {
            mi00: Mi00Path::new(mi00),
            mi01: Mi01Path::new(mi01),
        }),
        (None, Some(_)) => Err(DiscoveryError::Mi00NotFound),
        (Some(_), None) => Err(DiscoveryError::Mi01NotFound),
        (None, None) => Err(DiscoveryError::NoDockInterfacesFound),
    }
}

/// Resolves the dock's hidraw nodes against the real system's sysfs/`/dev`. This is what
/// `main.rs`'s future startup sequence (Milestone 2+) will call; it has no test coverage of its
/// own beyond `discover_dock_hidraw`'s logic, since exercising this specific function needs the
/// real kernel and the real device attached.
pub fn discover() -> Result<DockHidrawPaths, DiscoveryError> {
    discover_dock_hidraw(
        Path::new(DEFAULT_SYSFS_HIDRAW_BASE),
        Path::new(DEFAULT_DEV_DIR),
    )
}

/// [`discover`] with the vendor/product IDs from the `[device]` config section.
pub fn discover_configured(ids: &DeviceIds) -> Result<DockHidrawPaths, DiscoveryError> {
    discover_with_ids(
        Path::new(DEFAULT_SYSFS_HIDRAW_BASE),
        Path::new(DEFAULT_DEV_DIR),
        ids,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// Builds a synthetic sysfs tree mimicking the real `/sys/class/hidraw` + USB device
    /// layout, under a fresh temp directory, so `discover_dock_hidraw`'s matching logic can be
    /// exercised without a real kernel or real hardware. Returns (sysfs_hidraw_base, dev_dir).
    ///
    /// Tree built:
    /// - hidraw0: dock's own MI_00 (interface 0, VID 1de1/PID f201)   -> should match
    /// - hidraw1: dock's own MI_01 (interface 1, VID 1de1/PID f201)   -> should match
    /// - hidraw2: dock's own MI_02 (interface 2, VID 1de1/PID f201)   -> present but irrelevant
    /// - hidraw3: unrelated vendor HID device, interface 0, different VID/PID -> must NOT match
    ///   as MI_00, proving the VID/PID cross-check actually matters and isn't a no-op.
    fn build_test_tree(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "screendockd_discovery_test_{name}_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base); // best-effort cleanup from a prior run

        let devices = base.join("devices");
        let class_hidraw = base.join("class/hidraw");
        fs::create_dir_all(&class_hidraw).unwrap();

        // Dock's own USB device, with three HID interfaces (0, 1, 2) sharing one VID/PID.
        let dock_usb_dev = devices.join("usb1/1-2");
        fs::create_dir_all(&dock_usb_dev).unwrap();
        fs::write(dock_usb_dev.join("idVendor"), "1de1\n").unwrap();
        fs::write(dock_usb_dev.join("idProduct"), "f201\n").unwrap();

        for (iface_num, hid_suffix, hidraw_name) in [
            (0u8, "0001", "hidraw0"),
            (1u8, "0002", "hidraw1"),
            (2u8, "0003", "hidraw2"),
        ] {
            let iface_dir = dock_usb_dev.join(format!("1-2:1.{iface_num}"));
            fs::create_dir_all(&iface_dir).unwrap();
            fs::write(
                iface_dir.join("bInterfaceNumber"),
                format!("{iface_num:02}\n"),
            )
            .unwrap();
            let hid_device_dir = iface_dir.join(format!("0003:1DE1:F201.{hid_suffix}"));
            fs::create_dir_all(&hid_device_dir).unwrap();

            let hidraw_entry = class_hidraw.join(hidraw_name);
            fs::create_dir_all(&hidraw_entry).unwrap();
            symlink(&hid_device_dir, hidraw_entry.join("device")).unwrap();
        }

        // An unrelated vendor HID device elsewhere on the (synthetic) system, also on
        // interface 0, but with a different VID/PID — must not be mistaken for MI_00.
        let other_usb_dev = devices.join("usb2/3-4");
        fs::create_dir_all(&other_usb_dev).unwrap();
        fs::write(other_usb_dev.join("idVendor"), "046d\n").unwrap();
        fs::write(other_usb_dev.join("idProduct"), "c52b\n").unwrap();
        let other_iface_dir = other_usb_dev.join("3-4:1.0");
        fs::create_dir_all(&other_iface_dir).unwrap();
        fs::write(other_iface_dir.join("bInterfaceNumber"), "00\n").unwrap();
        let other_hid_device_dir = other_iface_dir.join("0003:046D:C52B.0004");
        fs::create_dir_all(&other_hid_device_dir).unwrap();
        let other_hidraw_entry = class_hidraw.join("hidraw3");
        fs::create_dir_all(&other_hidraw_entry).unwrap();
        symlink(&other_hid_device_dir, other_hidraw_entry.join("device")).unwrap();

        base
    }

    #[test]
    fn resolves_mi00_and_mi01_and_ignores_unrelated_vendor_device() {
        let base = build_test_tree("resolves_both");
        let sysfs_hidraw_base = base.join("class/hidraw");
        let dev_dir = base.join("dev");

        let result = discover_dock_hidraw(&sysfs_hidraw_base, &dev_dir).unwrap();

        assert_eq!(result.mi00.as_path(), dev_dir.join("hidraw0"));
        assert_eq!(result.mi01.as_path(), dev_dir.join("hidraw1"));
        // hidraw3 (unrelated vendor, interface 0) must never have been picked as mi00.
        assert_ne!(result.mi00.as_path(), dev_dir.join("hidraw3"));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn reports_mi01_not_found_when_only_mi00_present() {
        let base = build_test_tree("mi01_missing");
        let sysfs_hidraw_base = base.join("class/hidraw");
        let dev_dir = base.join("dev");

        // Remove the MI_01 hidraw entry to simulate it not being present.
        fs::remove_dir_all(sysfs_hidraw_base.join("hidraw1")).unwrap();

        let err = discover_dock_hidraw(&sysfs_hidraw_base, &dev_dir).unwrap_err();
        match err {
            DiscoveryError::Mi01NotFound => {}
            other => panic!("expected Mi01NotFound, got {other:?}"),
        }

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn reports_no_dock_interfaces_found_on_empty_tree() {
        let base = std::env::temp_dir().join(format!(
            "screendockd_discovery_test_empty_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let sysfs_hidraw_base = base.join("class/hidraw");
        fs::create_dir_all(&sysfs_hidraw_base).unwrap();
        let dev_dir = base.join("dev");

        let err = discover_dock_hidraw(&sysfs_hidraw_base, &dev_dir).unwrap_err();
        match err {
            DiscoveryError::NoDockInterfacesFound => {}
            other => panic!("expected NoDockInterfacesFound, got {other:?}"),
        }

        let _ = fs::remove_dir_all(&base);
    }
    #[test]
    fn id_override_selects_a_different_device() {
        let base = build_test_tree("id_override");
        let sysfs_hidraw_base = base.join("class/hidraw");
        let dev_dir = base.join("dev");
        let ids = DeviceIds::from_config(&crate::config::DeviceConfig {
            vendor_id_hex: "046D".into(),
            product_id_hex: "0xc52b".into(),
        })
        .unwrap();

        // The unrelated device only has interface 0, so matching it must report a missing MI_01
        // instead of resolving the dock.
        let err = discover_with_ids(&sysfs_hidraw_base, &dev_dir, &ids).unwrap_err();
        assert!(matches!(err, DiscoveryError::Mi01NotFound));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_malformed_device_ids() {
        for bad in ["", "1de", "1de1f", "zzzz"] {
            let result = DeviceIds::from_config(&crate::config::DeviceConfig {
                vendor_id_hex: bad.into(),
                product_id_hex: "f201".into(),
            });
            assert!(
                matches!(result, Err(DiscoveryError::InvalidDeviceId { .. })),
                "{bad:?}"
            );
        }
    }
}
