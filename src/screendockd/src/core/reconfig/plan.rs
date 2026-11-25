//! Pure classification of a settings change into a change class and the steps it needs.

use std::collections::BTreeSet;

use super::types::{ChangeClass, LiveSettings, PlanOptions, ReconfigAction, ReconfigPlan};
use crate::utils::types::DisplayMode;

impl ReconfigPlan {
    /// How disruptive the change is.
    pub fn class(&self) -> ChangeClass {
        self.class
    }

    /// The steps the change needs, in execution order.
    pub fn actions(&self) -> &[ReconfigAction] {
        &self.actions
    }

    /// True when nothing has to be done to the running pipeline.
    pub fn is_noop(&self) -> bool {
        self.actions.is_empty()
    }
}

/// Classifies the change from `old` to `new`, assuming the encoder changes its rate live.
pub fn plan_change(old: &LiveSettings, new: &LiveSettings) -> ReconfigPlan {
    plan_change_with(old, new, PlanOptions::default())
}

/// Classifies the change from `old` to `new` for a pipeline described by `options`.
pub fn plan_change_with(
    old: &LiveSettings,
    new: &LiveSettings,
    options: PlanOptions,
) -> ReconfigPlan {
    use ReconfigAction as A;

    let mut class = ChangeClass::Live;
    let mut actions = BTreeSet::new();
    let mut raise = |to: ChangeClass| class = class.max(to);

    let extend_before = old.mode == DisplayMode::Extend;
    let extend_after = new.mode == DisplayMode::Extend;

    if old.geometry != new.geometry {
        raise(ChangeClass::DockCycle);
        actions.insert(A::RebuildEncoder);
        actions.insert(A::RunDockCycle);
        if extend_after {
            actions.insert(A::RetargetCapture);
        }
        if extend_before && extend_after {
            actions.insert(A::ResizeMonitor);
        }
    }

    if old.mode != new.mode {
        raise(ChangeClass::Rebuild);
        actions.insert(A::RebuildEncoder);
        actions.insert(A::RetargetCapture);
        actions.insert(if extend_after {
            A::CreateMonitor
        } else {
            A::DestroyMonitor
        });
    }

    if old.fps != new.fps
        || old.preset != new.preset
        || old.tune_zerolatency != new.tune_zerolatency
    {
        raise(ChangeClass::Rebuild);
        actions.insert(A::RebuildEncoder);
    }

    if old.bitrate_kbps != new.bitrate_kbps || old.vbv_buffer_ms != new.vbv_buffer_ms {
        if options.live_rate {
            actions.insert(A::SetRate);
        } else {
            raise(ChangeClass::Rebuild);
            actions.insert(A::RebuildEncoder);
        }
    }

    if old.keyframe_interval != new.keyframe_interval {
        actions.insert(A::SetKeyframeInterval);
    }

    let placement_changed = old.placement != new.placement
        || old.anchor != new.anchor
        || old.x != new.x
        || old.y != new.y;
    if placement_changed && extend_before && extend_after {
        actions.insert(A::RetargetCapture);
    }

    if (old.name != new.name || old.wayland_backend != new.wayland_backend)
        && extend_before
        && extend_after
    {
        raise(ChangeClass::Rebuild);
        actions.insert(A::RecreateMonitor);
        actions.insert(A::RetargetCapture);
        actions.insert(A::RebuildEncoder);
    }

    if old.follow_system_changes != new.follow_system_changes {
        actions.insert(A::SetFollow);
    }

    ReconfigPlan {
        class,
        actions: actions.into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::display::{DisplayGeometry, Orientation};
    use crate::utils::types::{Placement, WaylandBackend, X264Preset};

    fn base() -> LiveSettings {
        LiveSettings {
            geometry: DisplayGeometry::new(1280, 720, Orientation::Landscape).unwrap(),
            fps: 25,
            preset: X264Preset::Ultrafast,
            tune_zerolatency: true,
            bitrate_kbps: 4000,
            vbv_buffer_ms: 100,
            keyframe_interval: 50,
            mode: DisplayMode::Mirror,
            placement: Placement::RightOf,
            anchor: "primary".into(),
            x: 0,
            y: 0,
            name: String::new(),
            wayland_backend: WaylandBackend::Auto,
            follow_system_changes: true,
        }
    }

    fn extended() -> LiveSettings {
        LiveSettings {
            mode: DisplayMode::Extend,
            ..base()
        }
    }

    #[test]
    fn identical_settings_need_nothing() {
        let plan = plan_change(&base(), &base());
        assert!(plan.is_noop());
        assert_eq!(plan.class(), ChangeClass::Live);
    }

    #[test]
    fn bitrate_and_vbv_changes_are_live_when_the_encoder_supports_it() {
        let new = LiveSettings {
            bitrate_kbps: 2000,
            ..base()
        };
        let plan = plan_change(&base(), &new);
        assert_eq!(plan.class(), ChangeClass::Live);
        assert_eq!(plan.actions(), [ReconfigAction::SetRate]);

        let new = LiveSettings {
            vbv_buffer_ms: 50,
            ..base()
        };
        assert_eq!(
            plan_change(&base(), &new).actions(),
            [ReconfigAction::SetRate]
        );
    }

    #[test]
    fn bitrate_change_rebuilds_the_encoder_when_the_rate_is_not_live() {
        let new = LiveSettings {
            bitrate_kbps: 2000,
            ..base()
        };
        let plan = plan_change_with(&base(), &new, PlanOptions { live_rate: false });
        assert_eq!(plan.class(), ChangeClass::Rebuild);
        assert_eq!(plan.actions(), [ReconfigAction::RebuildEncoder]);
    }

    #[test]
    fn keyframe_interval_change_is_live() {
        let new = LiveSettings {
            keyframe_interval: 25,
            ..base()
        };
        let plan = plan_change(&base(), &new);
        assert_eq!(plan.class(), ChangeClass::Live);
        assert_eq!(plan.actions(), [ReconfigAction::SetKeyframeInterval]);
    }

    #[test]
    fn frame_rate_preset_and_tune_changes_rebuild_the_encoder() {
        for new in [
            LiveSettings { fps: 15, ..base() },
            LiveSettings {
                preset: X264Preset::Veryfast,
                ..base()
            },
            LiveSettings {
                tune_zerolatency: false,
                ..base()
            },
        ] {
            let plan = plan_change(&base(), &new);
            assert_eq!(plan.class(), ChangeClass::Rebuild);
            assert_eq!(plan.actions(), [ReconfigAction::RebuildEncoder]);
        }
    }

    #[test]
    fn size_change_in_mirror_mode_runs_the_dock_cycle() {
        let new = LiveSettings {
            geometry: DisplayGeometry::new(1600, 900, Orientation::Landscape).unwrap(),
            ..base()
        };
        let plan = plan_change(&base(), &new);
        assert_eq!(plan.class(), ChangeClass::DockCycle);
        assert_eq!(
            plan.actions(),
            [ReconfigAction::RebuildEncoder, ReconfigAction::RunDockCycle]
        );
    }

    #[test]
    fn orientation_change_runs_the_dock_cycle() {
        let old = LiveSettings {
            geometry: DisplayGeometry::new(720, 720, Orientation::Landscape).unwrap(),
            ..base()
        };
        let new = LiveSettings {
            geometry: DisplayGeometry::new(720, 720, Orientation::Portrait).unwrap(),
            ..base()
        };
        assert_eq!(plan_change(&old, &new).class(), ChangeClass::DockCycle);
    }

    #[test]
    fn size_change_in_extend_mode_resizes_the_monitor_and_retargets_capture() {
        let new = LiveSettings {
            geometry: DisplayGeometry::new(1600, 900, Orientation::Landscape).unwrap(),
            ..extended()
        };
        let plan = plan_change(&extended(), &new);
        assert_eq!(plan.class(), ChangeClass::DockCycle);
        assert_eq!(
            plan.actions(),
            [
                ReconfigAction::ResizeMonitor,
                ReconfigAction::RebuildEncoder,
                ReconfigAction::RetargetCapture,
                ReconfigAction::RunDockCycle
            ]
        );
    }

    #[test]
    fn switching_to_extend_at_the_same_size_rebuilds_without_a_dock_cycle() {
        let plan = plan_change(&base(), &extended());
        assert_eq!(plan.class(), ChangeClass::Rebuild);
        assert_eq!(
            plan.actions(),
            [
                ReconfigAction::CreateMonitor,
                ReconfigAction::RebuildEncoder,
                ReconfigAction::RetargetCapture
            ]
        );
    }

    #[test]
    fn switching_to_mirror_destroys_the_monitor() {
        let plan = plan_change(&extended(), &base());
        assert_eq!(plan.class(), ChangeClass::Rebuild);
        assert!(plan.actions().contains(&ReconfigAction::DestroyMonitor));
        assert!(!plan.actions().contains(&ReconfigAction::RunDockCycle));
    }

    #[test]
    fn switching_mode_and_size_together_runs_the_dock_cycle() {
        let new = LiveSettings {
            geometry: DisplayGeometry::new(1600, 900, Orientation::Landscape).unwrap(),
            ..extended()
        };
        let plan = plan_change(&base(), &new);
        assert_eq!(plan.class(), ChangeClass::DockCycle);
        assert!(plan.actions().contains(&ReconfigAction::CreateMonitor));
        assert!(plan.actions().contains(&ReconfigAction::RunDockCycle));
    }

    #[test]
    fn monitor_position_change_in_extend_mode_only_retargets_capture() {
        let new = LiveSettings {
            placement: Placement::LeftOf,
            ..extended()
        };
        let plan = plan_change(&extended(), &new);
        assert_eq!(plan.class(), ChangeClass::Live);
        assert_eq!(plan.actions(), [ReconfigAction::RetargetCapture]);

        let new = LiveSettings {
            x: 10,
            anchor: "HDMI-1".into(),
            ..extended()
        };
        assert_eq!(
            plan_change(&extended(), &new).actions(),
            [ReconfigAction::RetargetCapture]
        );
    }

    #[test]
    fn monitor_position_change_in_mirror_mode_needs_nothing() {
        let new = LiveSettings {
            placement: Placement::Below,
            ..base()
        };
        assert!(plan_change(&base(), &new).is_noop());
    }

    #[test]
    fn monitor_name_or_backend_change_in_extend_mode_recreates_the_monitor() {
        for new in [
            LiveSettings {
                name: "Dock".into(),
                ..extended()
            },
            LiveSettings {
                wayland_backend: WaylandBackend::Portal,
                ..extended()
            },
        ] {
            let plan = plan_change(&extended(), &new);
            assert_eq!(plan.class(), ChangeClass::Rebuild);
            assert!(plan.actions().contains(&ReconfigAction::RecreateMonitor));
        }
        let new = LiveSettings {
            name: "Dock".into(),
            ..base()
        };
        assert!(plan_change(&base(), &new).is_noop());
    }

    #[test]
    fn follow_system_changes_toggle_is_live() {
        let new = LiveSettings {
            follow_system_changes: false,
            ..base()
        };
        let plan = plan_change(&base(), &new);
        assert_eq!(plan.class(), ChangeClass::Live);
        assert_eq!(plan.actions(), [ReconfigAction::SetFollow]);
    }

    #[test]
    fn combined_changes_take_the_highest_class_and_list_each_step_once() {
        let new = LiveSettings {
            geometry: DisplayGeometry::new(900, 600, Orientation::Landscape).unwrap(),
            fps: 15,
            bitrate_kbps: 1000,
            keyframe_interval: 10,
            ..base()
        };
        let plan = plan_change(&base(), &new);
        assert_eq!(plan.class(), ChangeClass::DockCycle);
        assert_eq!(
            plan.actions(),
            [
                ReconfigAction::SetRate,
                ReconfigAction::SetKeyframeInterval,
                ReconfigAction::RebuildEncoder,
                ReconfigAction::RunDockCycle
            ]
        );
    }
}
