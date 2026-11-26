//! The dock's panel as the daemon reports it: the EDID the dock sent, else the cached one, else
//! the configured size.

use std::collections::BTreeMap;

pub use super::types::{DisplayInfo, DisplayInfoSource, DEFAULT_MONITOR_NAME};
use crate::protocol::edid::{Edid, EdidMode, RefreshMilliHz};
use crate::utils::types::EdidSource;

impl DisplayInfoSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dock => "dock",
            Self::Cache => "cache",
            Self::Config => "config",
        }
    }
}

impl DisplayInfo {
    /// Picks the best source. With `EdidSource::Synthetic` only the configured size is used.
    /// Bytes that do not parse are skipped.
    pub fn resolve(
        dock: Option<&[u8]>,
        cached: Option<&[u8]>,
        prefer: EdidSource,
        fallback: (u16, u16),
    ) -> Self {
        if prefer == EdidSource::Dock {
            for (bytes, source) in [
                (dock, DisplayInfoSource::Dock),
                (cached, DisplayInfoSource::Cache),
            ] {
                if let Some(info) = bytes
                    .and_then(|b| Edid::parse(b).ok())
                    .and_then(|edid| Self::from_edid(&edid, source))
                {
                    return info;
                }
            }
        }
        Self::from_size(fallback)
    }

    fn from_edid(edid: &Edid, source: DisplayInfoSource) -> Option<Self> {
        let native = edid.preferred.or_else(|| edid.modes.first().copied())?;
        Some(Self {
            source,
            name: edid
                .name
                .as_ref()
                .map_or_else(|| DEFAULT_MONITOR_NAME.to_string(), |n| n.to_string()),
            native,
            modes: edid.modes.clone(),
        })
    }

    fn from_size((width, height): (u16, u16)) -> Self {
        let native = EdidMode {
            width,
            height,
            refresh: RefreshMilliHz(0),
        };
        Self {
            source: DisplayInfoSource::Config,
            name: DEFAULT_MONITOR_NAME.to_string(),
            native,
            modes: vec![native],
        }
    }

    /// The read-only keys of the control interface.
    pub fn fields(&self) -> BTreeMap<String, String> {
        let modes: Vec<String> = self.modes.iter().map(|m| mode_text(*m)).collect();
        BTreeMap::from([
            ("edid-source".to_string(), self.source.as_str().to_string()),
            ("edid-name".to_string(), self.name.clone()),
            ("edid-native".to_string(), mode_text(self.native)),
            ("edid-modes".to_string(), modes.join(",")),
        ])
    }
}

/// `1920x1080@60` or `1920x1080@59.94`; no rate when it is unknown.
fn mode_text(mode: EdidMode) -> String {
    let size = format!("{}x{}", mode.width, mode.height);
    match mode.refresh.0 {
        0 => size,
        milli if milli % 1000 == 0 => format!("{size}@{}", milli / 1000),
        milli => {
            let rate = format!("{}.{:03}", milli / 1000, milli % 1000);
            format!("{size}@{}", rate.trim_end_matches('0'))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCK: &[u8] = include_bytes!("../../tests/fixtures/dock_edid.bin");

    #[test]
    fn dock_edid_gives_name_native_mode_and_mode_list() {
        let info = DisplayInfo::resolve(Some(DOCK), None, EdidSource::Dock, (1280, 720));
        assert_eq!(info.source, DisplayInfoSource::Dock);
        let fields = info.fields();
        assert_eq!(fields["edid-source"], "dock");
        assert_eq!(fields["edid-name"], "HDMI Extender");
        assert_eq!(fields["edid-native"], "1920x1080@60");
        assert_eq!(
            fields["edid-modes"],
            "1920x1080@60,1280x720@60,1280x960@60,1280x800@60,1280x1024@60,1600x900@60"
        );
    }

    #[test]
    fn the_cache_is_used_when_the_dock_sent_nothing_usable() {
        let info = DisplayInfo::resolve(None, Some(DOCK), EdidSource::Dock, (1280, 720));
        assert_eq!(info.source, DisplayInfoSource::Cache);
        let broken = DisplayInfo::resolve(Some(&DOCK[..50]), Some(DOCK), EdidSource::Dock, (1, 2));
        assert_eq!(broken.source, DisplayInfoSource::Cache);
    }

    #[test]
    fn the_configured_size_is_the_fallback_and_the_synthetic_choice() {
        let none = DisplayInfo::resolve(None, None, EdidSource::Dock, (1280, 720));
        assert_eq!(none.source, DisplayInfoSource::Config);
        assert_eq!(none.fields()["edid-name"], "ScreenDock");
        assert_eq!(none.fields()["edid-native"], "1280x720");
        assert_eq!(none.fields()["edid-modes"], "1280x720");
        let synthetic =
            DisplayInfo::resolve(Some(DOCK), Some(DOCK), EdidSource::Synthetic, (800, 480));
        assert_eq!(synthetic.source, DisplayInfoSource::Config);
        assert_eq!(synthetic.fields()["edid-native"], "800x480");
    }

    #[test]
    fn fractional_refresh_rates_keep_their_decimals() {
        let mode = EdidMode {
            width: 1920,
            height: 1080,
            refresh: RefreshMilliHz(59_940),
        };
        assert_eq!(mode_text(mode), "1920x1080@59.94");
    }
}
