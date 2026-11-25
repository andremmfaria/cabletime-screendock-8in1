//! The names of the live settings as the control interface shows and accepts them, and the
//! read-only answers built from them.

use std::collections::BTreeMap;

use super::types::{LiveSettings, SettingKey, SettingsError, SettingsPatch};
use crate::protocol::control::{ControlRequest, ControlResponse};
use crate::protocol::display::Orientation;
use crate::utils::types::{DisplayMode, Placement, WaylandBackend, X264Preset};

impl SettingKey {
    /// Every key, in display order.
    pub const ALL: [SettingKey; 16] = [
        SettingKey::Resolution,
        SettingKey::Orientation,
        SettingKey::Fps,
        SettingKey::Preset,
        SettingKey::Tune,
        SettingKey::Bitrate,
        SettingKey::VbvBufferMs,
        SettingKey::KeyframeInterval,
        SettingKey::Mode,
        SettingKey::Placement,
        SettingKey::Anchor,
        SettingKey::X,
        SettingKey::Y,
        SettingKey::Name,
        SettingKey::WaylandBackend,
        SettingKey::FollowSystemChanges,
    ];

    /// The key's name on the control interface.
    pub fn name(self) -> &'static str {
        match self {
            Self::Resolution => "resolution",
            Self::Orientation => "orientation",
            Self::Fps => "fps",
            Self::Preset => "preset",
            Self::Tune => "tune",
            Self::Bitrate => "bitrate",
            Self::VbvBufferMs => "vbv-buffer-ms",
            Self::KeyframeInterval => "keyframe-interval",
            Self::Mode => "mode",
            Self::Placement => "placement",
            Self::Anchor => "anchor",
            Self::X => "x",
            Self::Y => "y",
            Self::Name => "name",
            Self::WaylandBackend => "wayland-backend",
            Self::FollowSystemChanges => "follow-system-changes",
        }
    }

    /// Finds a key by name.
    pub fn parse(name: &str) -> Result<Self, SettingsError> {
        Self::ALL
            .into_iter()
            .find(|key| key.name() == name)
            .ok_or_else(|| SettingsError::UnknownKey(truncated(name)))
    }

    /// The setting's current value as text.
    pub fn get(self, live: &LiveSettings) -> String {
        match self {
            Self::Resolution => format!("{}x{}", live.geometry.width().0, live.geometry.height().0),
            Self::Orientation => match live.geometry.orientation() {
                Orientation::Landscape => "landscape".into(),
                Orientation::Portrait => "portrait".into(),
            },
            Self::Fps => live.fps.to_string(),
            Self::Preset => live.preset.as_str().into(),
            Self::Tune => live.tune_zerolatency.to_string(),
            Self::Bitrate => live.bitrate_kbps.to_string(),
            Self::VbvBufferMs => live.vbv_buffer_ms.to_string(),
            Self::KeyframeInterval => live.keyframe_interval.to_string(),
            Self::Mode => live.mode.as_str().into(),
            Self::Placement => live.placement.as_str().into(),
            Self::Anchor => live.anchor.clone(),
            Self::X => live.x.to_string(),
            Self::Y => live.y.to_string(),
            Self::Name => live.name.clone(),
            Self::WaylandBackend => live.wayland_backend.as_str().into(),
            Self::FollowSystemChanges => live.follow_system_changes.to_string(),
        }
    }

    /// Reads `value` as this setting's type and returns the patch that sets it. The range is
    /// checked when the patch is applied.
    pub fn patch(self, value: &str) -> Result<SettingsPatch, SettingsError> {
        let key = self.name();
        let number = |expected: &'static str| {
            value
                .parse::<u32>()
                .map_err(|_| SettingsError::InvalidValue { key, expected })
        };
        let mut patch = SettingsPatch::default();
        match self {
            Self::Resolution => {
                let invalid = || SettingsError::InvalidValue {
                    key,
                    expected: "WIDTHxHEIGHT, for example 1600x900",
                };
                let (width, height) = value.split_once(['x', 'X']).ok_or_else(invalid)?;
                let width = width.parse::<u32>().map_err(|_| invalid())?;
                let height = height.parse::<u32>().map_err(|_| invalid())?;
                patch = SettingsPatch::resolution(width, height);
            }
            Self::Orientation => {
                patch.orientation =
                    Some(
                        Orientation::parse(value).map_err(|_| SettingsError::InvalidValue {
                            key,
                            expected: "landscape or portrait",
                        })?,
                    );
            }
            Self::Fps => patch.fps = Some(number("a whole number")?),
            Self::Preset => {
                patch.preset = Some(X264Preset::from_name(value).ok_or(
                    SettingsError::InvalidValue {
                        key,
                        expected: "ultrafast, superfast, veryfast, faster, fast or medium",
                    },
                )?);
            }
            Self::Tune => patch.tune_zerolatency = Some(boolean(key, value)?),
            Self::Bitrate => patch.bitrate_kbps = Some(number("a whole number of kbit/s")?),
            Self::VbvBufferMs => patch.vbv_buffer_ms = Some(number("a whole number of ms")?),
            Self::KeyframeInterval => patch.keyframe_interval = Some(number("a whole number")?),
            Self::Mode => {
                patch.mode = Some(DisplayMode::from_name(value).ok_or(
                    SettingsError::InvalidValue {
                        key,
                        expected: "mirror or extend",
                    },
                )?);
            }
            Self::Placement => {
                patch.placement = Some(Placement::from_name(value).ok_or(
                    SettingsError::InvalidValue {
                        key,
                        expected: "right-of, left-of, above, below or at",
                    },
                )?);
            }
            Self::Anchor => patch.anchor = Some(value.to_string()),
            Self::X => {
                patch.x = Some(value.parse().map_err(|_| SettingsError::InvalidValue {
                    key,
                    expected: "a whole number",
                })?)
            }
            Self::Y => {
                patch.y = Some(value.parse().map_err(|_| SettingsError::InvalidValue {
                    key,
                    expected: "a whole number",
                })?)
            }
            Self::Name => patch.name = Some(value.to_string()),
            Self::WaylandBackend => {
                patch.wayland_backend = Some(WaylandBackend::from_name(value).ok_or(
                    SettingsError::InvalidValue {
                        key,
                        expected: "auto, mutter, portal or wlroots",
                    },
                )?);
            }
            Self::FollowSystemChanges => patch.follow_system_changes = Some(boolean(key, value)?),
        }
        Ok(patch)
    }
}

fn boolean(key: &'static str, value: &str) -> Result<bool, SettingsError> {
    match value {
        "true" | "on" => Ok(true),
        "false" | "off" => Ok(false),
        _ => Err(SettingsError::InvalidValue {
            key,
            expected: "true or false",
        }),
    }
}

/// A client supplied name limited to a length that is safe to log and echo.
fn truncated(text: &str) -> String {
    text.chars().take(40).collect()
}

/// The names of the settings whose values differ between `old` and `new`.
pub fn changed_keys(old: &LiveSettings, new: &LiveSettings) -> Vec<&'static str> {
    SettingKey::ALL
        .into_iter()
        .filter(|key| key.get(old) != key.get(new))
        .map(SettingKey::name)
        .collect()
}

/// Every setting as name and value, or just the one named by `key`.
pub fn setting_values(
    live: &LiveSettings,
    key: Option<&str>,
) -> Result<BTreeMap<String, String>, SettingsError> {
    let keys: Vec<SettingKey> = match key {
        Some(name) => vec![SettingKey::parse(name)?],
        None => SettingKey::ALL.to_vec(),
    };
    Ok(keys
        .into_iter()
        .map(|key| (key.name().to_string(), key.get(live)))
        .collect())
}

/// Answers `status` and `get` from the live settings. `extra` adds fields to the status
/// answer (the version, the process id and whatever the caller knows). Returns `None` for
/// requests that change state.
pub fn answer_read(
    live: &LiveSettings,
    request: &ControlRequest,
    extra: BTreeMap<String, String>,
) -> Option<ControlResponse> {
    match request {
        ControlRequest::Status {} => {
            let mut fields = setting_values(live, None).expect("all keys exist");
            fields.extend(extra);
            Some(ControlResponse::Status { fields })
        }
        ControlRequest::Get { key } => Some(match setting_values(live, key.as_deref()) {
            Ok(values) => ControlResponse::Values { values },
            Err(e) => ControlResponse::invalid(e.to_string()),
        }),
        ControlRequest::Set { .. } | ControlRequest::Reload {} => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::config::load_from_path;

    fn shipped() -> LiveSettings {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/screendock_cfg.toml");
        LiveSettings::try_from(&load_from_path(&path).unwrap()).unwrap()
    }

    #[test]
    fn every_key_name_parses_back_to_itself() {
        for key in SettingKey::ALL {
            assert_eq!(SettingKey::parse(key.name()).unwrap(), key);
        }
        assert!(matches!(
            SettingKey::parse("volume"),
            Err(SettingsError::UnknownKey(_))
        ));
    }

    #[test]
    fn unknown_key_names_are_truncated_in_the_error() {
        let error = SettingKey::parse(&"k".repeat(500)).unwrap_err();
        assert!(error.to_string().len() < 100, "{error}");
    }

    #[test]
    fn every_setting_round_trips_through_its_text_value() {
        let live = shipped();
        for key in SettingKey::ALL {
            let text = key.get(&live);
            let patch = key
                .patch(&text)
                .unwrap_or_else(|e| panic!("{}: {e}", key.name()));
            assert_eq!(patch.apply_to(&live).unwrap(), live, "{}", key.name());
        }
    }

    #[test]
    fn values_that_do_not_fit_the_type_are_invalid() {
        for (key, value) in [
            (SettingKey::Resolution, "big"),
            (SettingKey::Resolution, "1280x"),
            (SettingKey::Resolution, "x720"),
            (SettingKey::Bitrate, "fast"),
            (SettingKey::Bitrate, "-5"),
            (SettingKey::Preset, "turbo"),
            (SettingKey::Mode, "clone"),
            (SettingKey::Tune, "maybe"),
            (SettingKey::Orientation, "sideways"),
            (SettingKey::X, "1.5"),
        ] {
            assert!(
                matches!(key.patch(value), Err(SettingsError::InvalidValue { .. })),
                "{} {value}",
                key.name()
            );
        }
    }

    #[test]
    fn resolution_text_sets_size_and_orientation() {
        let live = shipped();
        let portrait = SettingKey::Resolution
            .patch("720x1280")
            .unwrap()
            .apply_to(&live)
            .unwrap();
        assert_eq!(SettingKey::Resolution.get(&portrait), "720x1280");
        assert_eq!(SettingKey::Orientation.get(&portrait), "portrait");
    }

    #[test]
    fn changed_keys_lists_only_the_differences() {
        let old = shipped();
        let new = SettingsPatch {
            bitrate_kbps: Some(1000),
            ..SettingsPatch::resolution(1600, 900)
        }
        .apply_to(&old)
        .unwrap();
        assert_eq!(changed_keys(&old, &new), ["resolution", "bitrate"]);
        assert!(changed_keys(&old, &old).is_empty());
    }

    #[test]
    fn status_and_get_are_answered_from_the_settings() {
        let live = shipped();
        let mut extra = BTreeMap::new();
        extra.insert("version".to_string(), "x".to_string());
        match answer_read(&live, &ControlRequest::Status {}, extra).unwrap() {
            ControlResponse::Status { fields } => {
                assert_eq!(fields["bitrate"], "4000");
                assert_eq!(fields["resolution"], "1280x720");
                assert_eq!(fields["version"], "x");
                assert_eq!(fields.len(), SettingKey::ALL.len() + 1);
            }
            other => panic!("{other:?}"),
        }
        let one = ControlRequest::Get {
            key: Some("mode".into()),
        };
        match answer_read(&live, &one, BTreeMap::new()).unwrap() {
            ControlResponse::Values { values } => {
                assert_eq!(values.len(), 1);
                assert_eq!(values["mode"], "mirror");
            }
            other => panic!("{other:?}"),
        }
        let bad = ControlRequest::Get {
            key: Some("volume".into()),
        };
        assert!(matches!(
            answer_read(&live, &bad, BTreeMap::new()),
            Some(ControlResponse::Invalid { .. })
        ));
        assert!(answer_read(&live, &ControlRequest::Reload {}, BTreeMap::new()).is_none());
    }
}
