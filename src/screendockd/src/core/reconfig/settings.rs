//! Building [`LiveSettings`] from the config and applying a [`SettingsPatch`] to them.

use super::types::{
    LiveSettings, SettingsError, SettingsPatch, MAX_BITRATE_KBPS, MAX_COORDINATE, MAX_FPS,
    MAX_KEYFRAME_INTERVAL, MAX_NAME_LEN, MAX_VBV_BUFFER_MS,
};
use crate::ports::encode::{EncoderSettings, DEFAULT_FPS};
use crate::protocol::display::{DisplayGeometry, Orientation};
use crate::utils::config::Config;
use crate::utils::types::{DisplayMode, Placement, WaylandBackend};

impl TryFrom<&Config> for LiveSettings {
    type Error = SettingsError;

    fn try_from(config: &Config) -> Result<Self, Self::Error> {
        Ok(Self {
            geometry: DisplayGeometry::from_config(&config.display)?,
            fps: config.capture.fps,
            preset: config.encode.preset,
            tune_zerolatency: config.encode.tune_zerolatency,
            bitrate_kbps: config.encode.bitrate_kbps.max(1),
            vbv_buffer_ms: config.encode.vbv_buffer_ms,
            keyframe_interval: config.encode.keyframe_interval.max(1),
            mode: DisplayMode::default(),
            placement: Placement::default(),
            anchor: "primary".into(),
            x: 0,
            y: 0,
            name: String::new(),
            wayland_backend: WaylandBackend::default(),
            follow_system_changes: true,
        })
    }
}

impl LiveSettings {
    /// The encoder settings these live settings describe.
    pub fn encoder_settings(&self) -> EncoderSettings {
        EncoderSettings {
            geometry: self.geometry,
            fps: if self.fps == 0 { DEFAULT_FPS } else { self.fps },
            keyframe_interval: self.keyframe_interval,
            bitrate_kbps: self.bitrate_kbps,
            preset: self.preset,
            zerolatency: self.tune_zerolatency,
            vbv_buffer_ms: self.vbv_buffer_ms,
        }
    }
}

fn check_range(key: &'static str, value: u32, min: u32, max: u32) -> Result<u32, SettingsError> {
    if (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(SettingsError::OutOfRange {
            key,
            value: i64::from(value),
            min: i64::from(min),
            max: i64::from(max),
        })
    }
}

fn check_coordinate(key: &'static str, value: i32) -> Result<i32, SettingsError> {
    if (-MAX_COORDINATE..=MAX_COORDINATE).contains(&value) {
        Ok(value)
    } else {
        Err(SettingsError::OutOfRange {
            key,
            value: i64::from(value),
            min: i64::from(-MAX_COORDINATE),
            max: i64::from(MAX_COORDINATE),
        })
    }
}

fn check_name(key: &'static str, value: &str, allow_empty: bool) -> Result<String, SettingsError> {
    if value.is_empty() && !allow_empty {
        return Err(SettingsError::InvalidText {
            key,
            reason: "must not be empty",
        });
    }
    if value.len() > MAX_NAME_LEN {
        return Err(SettingsError::InvalidText {
            key,
            reason: "is too long",
        });
    }
    if !value.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        return Err(SettingsError::InvalidText {
            key,
            reason: "must be printable ASCII",
        });
    }
    Ok(value.to_string())
}

impl SettingsPatch {
    /// A patch that sets the picture size and derives the orientation from its shape. A square
    /// size keeps the current orientation.
    pub fn resolution(width: u32, height: u32) -> Self {
        let orientation = match width.cmp(&height) {
            std::cmp::Ordering::Greater => Some(Orientation::Landscape),
            std::cmp::Ordering::Less => Some(Orientation::Portrait),
            std::cmp::Ordering::Equal => None,
        };
        Self {
            width: Some(width),
            height: Some(height),
            orientation,
            ..Self::default()
        }
    }

    /// The patch that turns `old` into `new`: only the fields that differ are set.
    pub fn between(old: &LiveSettings, new: &LiveSettings) -> Self {
        fn changed<T: PartialEq + Clone>(old: &T, new: &T) -> Option<T> {
            (old != new).then(|| new.clone())
        }
        Self {
            width: changed(&old.geometry.width().0, &new.geometry.width().0),
            height: changed(&old.geometry.height().0, &new.geometry.height().0),
            orientation: changed(&old.geometry.orientation(), &new.geometry.orientation()),
            fps: changed(&old.fps, &new.fps),
            preset: changed(&old.preset, &new.preset),
            tune_zerolatency: changed(&old.tune_zerolatency, &new.tune_zerolatency),
            bitrate_kbps: changed(&old.bitrate_kbps, &new.bitrate_kbps),
            vbv_buffer_ms: changed(&old.vbv_buffer_ms, &new.vbv_buffer_ms),
            keyframe_interval: changed(&old.keyframe_interval, &new.keyframe_interval),
            mode: changed(&old.mode, &new.mode),
            placement: changed(&old.placement, &new.placement),
            anchor: changed(&old.anchor, &new.anchor),
            x: changed(&old.x, &new.x),
            y: changed(&old.y, &new.y),
            name: changed(&old.name, &new.name),
            wayland_backend: changed(&old.wayland_backend, &new.wayland_backend),
            follow_system_changes: changed(&old.follow_system_changes, &new.follow_system_changes),
        }
    }

    /// True when the patch changes nothing.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Applies the patch to `base` and validates every changed value. `base` is not modified.
    pub fn apply_to(&self, base: &LiveSettings) -> Result<LiveSettings, SettingsError> {
        let mut next = base.clone();
        if self.width.is_some() || self.height.is_some() || self.orientation.is_some() {
            next.geometry = DisplayGeometry::new(
                self.width.unwrap_or(base.geometry.width().0),
                self.height.unwrap_or(base.geometry.height().0),
                self.orientation.unwrap_or(base.geometry.orientation()),
            )?;
        }
        if let Some(fps) = self.fps {
            next.fps = check_range("fps", fps, 0, MAX_FPS)?;
        }
        if let Some(preset) = self.preset {
            next.preset = preset;
        }
        if let Some(tune) = self.tune_zerolatency {
            next.tune_zerolatency = tune;
        }
        if let Some(rate) = self.bitrate_kbps {
            next.bitrate_kbps = check_range("bitrate_kbps", rate, 1, MAX_BITRATE_KBPS)?;
        }
        if let Some(ms) = self.vbv_buffer_ms {
            next.vbv_buffer_ms = check_range("vbv_buffer_ms", ms, 0, MAX_VBV_BUFFER_MS)?;
        }
        if let Some(interval) = self.keyframe_interval {
            next.keyframe_interval =
                check_range("keyframe_interval", interval, 1, MAX_KEYFRAME_INTERVAL)?;
        }
        if let Some(mode) = self.mode {
            next.mode = mode;
        }
        if let Some(placement) = self.placement {
            next.placement = placement;
        }
        if let Some(anchor) = &self.anchor {
            next.anchor = check_name("anchor", anchor, false)?;
        }
        if let Some(x) = self.x {
            next.x = check_coordinate("x", x)?;
        }
        if let Some(y) = self.y {
            next.y = check_coordinate("y", y)?;
        }
        if let Some(name) = &self.name {
            next.name = check_name("name", name, true)?;
        }
        if let Some(backend) = self.wayland_backend {
            next.wayland_backend = backend;
        }
        if let Some(follow) = self.follow_system_changes {
            next.follow_system_changes = follow;
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::display::DisplayError;
    use crate::utils::config::load_from_path;
    use crate::utils::types::{PictureSize, X264Preset};

    fn shipped() -> LiveSettings {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/screendock_cfg.toml");
        LiveSettings::try_from(&load_from_path(&path).unwrap()).unwrap()
    }

    #[test]
    fn settings_from_the_shipped_config_match_its_values() {
        let live = shipped();
        assert_eq!(live.geometry.size(), PictureSize::new(1280, 720));
        assert_eq!(live.bitrate_kbps, 4000);
        assert_eq!(live.keyframe_interval, 50);
        assert_eq!(live.fps, 25);
        assert_eq!(live.mode, DisplayMode::Mirror);
        assert!(live.follow_system_changes);
    }

    #[test]
    fn empty_patch_changes_nothing() {
        let live = shipped();
        let patch = SettingsPatch::default();
        assert!(patch.is_empty());
        assert_eq!(patch.apply_to(&live).unwrap(), live);
    }

    #[test]
    fn resolution_patch_derives_orientation_from_shape() {
        let live = shipped();
        let portrait = SettingsPatch::resolution(720, 1280)
            .apply_to(&live)
            .unwrap();
        assert_eq!(portrait.geometry.orientation(), Orientation::Portrait);
        let back = SettingsPatch::resolution(1600, 900)
            .apply_to(&portrait)
            .unwrap();
        assert_eq!(back.geometry.orientation(), Orientation::Landscape);
        let square = SettingsPatch::resolution(720, 720)
            .apply_to(&portrait)
            .unwrap();
        assert_eq!(square.geometry.orientation(), Orientation::Portrait);
    }

    #[test]
    fn size_patch_that_disagrees_with_the_orientation_is_rejected() {
        let live = shipped();
        let patch = SettingsPatch {
            width: Some(720),
            height: Some(1280),
            ..SettingsPatch::default()
        };
        assert!(matches!(
            patch.apply_to(&live),
            Err(SettingsError::Display(
                DisplayError::OrientationMismatch { .. }
            ))
        ));
    }

    #[test]
    fn odd_and_out_of_range_sizes_are_rejected() {
        let live = shipped();
        assert!(matches!(
            SettingsPatch::resolution(1281, 720).apply_to(&live),
            Err(SettingsError::Display(DisplayError::Odd { .. }))
        ));
        assert!(matches!(
            SettingsPatch::resolution(8192, 720).apply_to(&live),
            Err(SettingsError::Display(DisplayError::OutOfRange { .. }))
        ));
    }

    #[test]
    fn rate_fps_and_interval_limits_are_enforced() {
        let live = shipped();
        for patch in [
            SettingsPatch {
                bitrate_kbps: Some(0),
                ..SettingsPatch::default()
            },
            SettingsPatch {
                bitrate_kbps: Some(MAX_BITRATE_KBPS + 1),
                ..SettingsPatch::default()
            },
            SettingsPatch {
                keyframe_interval: Some(0),
                ..SettingsPatch::default()
            },
            SettingsPatch {
                fps: Some(MAX_FPS + 1),
                ..SettingsPatch::default()
            },
            SettingsPatch {
                vbv_buffer_ms: Some(MAX_VBV_BUFFER_MS + 1),
                ..SettingsPatch::default()
            },
        ] {
            assert!(
                matches!(patch.apply_to(&live), Err(SettingsError::OutOfRange { .. })),
                "{patch:?}"
            );
        }
        let ok = SettingsPatch {
            bitrate_kbps: Some(2500),
            keyframe_interval: Some(25),
            fps: Some(0),
            vbv_buffer_ms: Some(0),
            ..SettingsPatch::default()
        }
        .apply_to(&live)
        .unwrap();
        assert_eq!(ok.bitrate_kbps, 2500);
        assert_eq!(ok.keyframe_interval, 25);
        assert_eq!(ok.fps, 0);
        assert_eq!(ok.vbv_buffer_ms, 0);
    }

    #[test]
    fn monitor_text_and_coordinates_are_validated() {
        let live = shipped();
        for patch in [
            SettingsPatch {
                name: Some("bad\nname".into()),
                ..SettingsPatch::default()
            },
            SettingsPatch {
                name: Some("x".repeat(MAX_NAME_LEN + 1)),
                ..SettingsPatch::default()
            },
            SettingsPatch {
                anchor: Some(String::new()),
                ..SettingsPatch::default()
            },
        ] {
            assert!(
                matches!(
                    patch.apply_to(&live),
                    Err(SettingsError::InvalidText { .. })
                ),
                "{patch:?}"
            );
        }
        let far = SettingsPatch {
            x: Some(MAX_COORDINATE + 1),
            ..SettingsPatch::default()
        };
        assert!(matches!(
            far.apply_to(&live),
            Err(SettingsError::OutOfRange { key: "x", .. })
        ));
        let named = SettingsPatch {
            name: Some("ScreenDock".into()),
            x: Some(-100),
            ..SettingsPatch::default()
        }
        .apply_to(&live)
        .unwrap();
        assert_eq!(named.name, "ScreenDock");
        assert_eq!(named.x, -100);
    }

    #[test]
    fn failed_patch_leaves_the_base_untouched() {
        let live = shipped();
        let before = live.clone();
        let _ = SettingsPatch {
            bitrate_kbps: Some(3000),
            keyframe_interval: Some(0),
            ..SettingsPatch::default()
        }
        .apply_to(&live)
        .unwrap_err();
        assert_eq!(live, before);
    }

    #[test]
    fn between_yields_only_the_differences_and_round_trips() {
        let old = shipped();
        let new = SettingsPatch {
            bitrate_kbps: Some(1500),
            preset: Some(X264Preset::Fast),
            mode: Some(DisplayMode::Extend),
            ..SettingsPatch::resolution(1600, 900)
        }
        .apply_to(&old)
        .unwrap();
        let patch = SettingsPatch::between(&old, &new);
        assert_eq!(patch.bitrate_kbps, Some(1500));
        assert_eq!(patch.width, Some(1600));
        assert_eq!(patch.fps, None);
        assert_eq!(patch.keyframe_interval, None);
        assert_eq!(patch.apply_to(&old).unwrap(), new);
        assert!(SettingsPatch::between(&old, &old).is_empty());
    }

    #[test]
    fn encoder_settings_use_the_default_rate_when_fps_is_zero() {
        let mut live = shipped();
        live.fps = 0;
        assert_eq!(live.encoder_settings().fps, 25);
        live.fps = 10;
        assert_eq!(live.encoder_settings().fps, 10);
        assert_eq!(live.encoder_settings().bitrate_kbps, live.bitrate_kbps);
    }
}
