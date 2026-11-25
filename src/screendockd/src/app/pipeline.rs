//! Hand-over of live encoder settings from the supervisor to the video loop.

use std::sync::{Arc, MutexGuard};
use std::time::{Duration, Instant};

pub use super::types::{EncoderBuilder, EncoderPipeline, PipelineKnobs, PipelineState};
use super::types::{PipelineShared, APPLY_TIMEOUT, CYCLE_APPLY_TIMEOUT};
use crate::app::supervisor::PipelineHandle;
use crate::core::reconfig::{ChangeClass, LiveSettings, PlanOptions, ReconfigAction, ReconfigPlan};
use crate::ports::capture::{CaptureArea, CaptureTarget};
use crate::ports::encode::{EncodeError, Encoder, EncoderSettings, DEFAULT_FPS};
use crate::protocol::display::DisplayGeometry;
use crate::utils::config::X264Preset;

impl EncoderBuilder {
    pub fn new(
        build: impl Fn(EncoderSettings) -> Result<Box<dyn Encoder>, EncodeError> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(build))
    }

    pub fn build(&self, settings: EncoderSettings) -> Result<Box<dyn Encoder>, EncodeError> {
        (self.0)(settings)
    }
}

impl PipelineKnobs {
    /// The knobs a set of live settings asks for. The captured area is the whole source.
    pub fn from_live(settings: &LiveSettings) -> Self {
        Self {
            bitrate_kbps: settings.bitrate_kbps,
            vbv_buffer_ms: settings.vbv_buffer_ms,
            keyframe_interval: settings.keyframe_interval,
            geometry: settings.geometry,
            fps: settings.fps,
            preset: settings.preset,
            tune_zerolatency: settings.tune_zerolatency,
            area: CaptureArea::Whole,
        }
    }

    /// Knobs with the daemon's usual values for `geometry`.
    pub fn for_geometry(geometry: DisplayGeometry) -> Self {
        Self {
            bitrate_kbps: 4000,
            vbv_buffer_ms: 100,
            keyframe_interval: 50,
            geometry,
            fps: DEFAULT_FPS,
            preset: X264Preset::Ultrafast,
            tune_zerolatency: true,
            area: CaptureArea::Whole,
        }
    }

    /// The encoder settings these knobs describe.
    pub fn encoder_settings(&self) -> EncoderSettings {
        EncoderSettings {
            geometry: self.geometry,
            fps: if self.fps == 0 { DEFAULT_FPS } else { self.fps },
            keyframe_interval: self.keyframe_interval.max(1),
            bitrate_kbps: self.bitrate_kbps.max(1),
            preset: self.preset,
            zerolatency: self.tune_zerolatency,
            vbv_buffer_ms: self.vbv_buffer_ms,
        }
    }

    /// Where capture has to look for these knobs.
    pub fn capture_target(&self) -> CaptureTarget {
        CaptureTarget {
            area: self.area,
            fps: self.fps,
        }
    }
}

impl PipelineState {
    /// A state holding `knobs`. `live_rate` says whether the encoder changes its rate in place.
    pub fn new(knobs: PipelineKnobs, live_rate: bool) -> Arc<Self> {
        Self::build(knobs, live_rate, None)
    }

    /// Like [`PipelineState::new`], and the video loop may replace its encoder using `builder`.
    pub fn with_builder(
        knobs: PipelineKnobs,
        live_rate: bool,
        builder: EncoderBuilder,
    ) -> Arc<Self> {
        Self::build(knobs, live_rate, Some(builder))
    }

    fn build(knobs: PipelineKnobs, live_rate: bool, builder: Option<EncoderBuilder>) -> Arc<Self> {
        Arc::new(Self {
            shared: std::sync::Mutex::new(PipelineShared {
                published: knobs,
                applied: knobs,
                version: 0,
                applied_version: 0,
                failure: None,
            }),
            done: std::sync::Condvar::new(),
            live_rate,
            builder,
        })
    }

    /// For the video loop: how to build a new encoder, when the pipeline can.
    pub fn builder(&self) -> Option<&EncoderBuilder> {
        self.builder.as_ref()
    }

    fn lock(&self) -> MutexGuard<'_, PipelineShared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// True when the encoder changes its rate in place.
    pub fn live_rate(&self) -> bool {
        self.live_rate
    }

    /// The knobs the video loop is running with.
    pub fn knobs(&self) -> PipelineKnobs {
        self.lock().applied
    }

    /// Publishes new knobs for the video loop and returns the version to wait for.
    pub fn publish(&self, knobs: PipelineKnobs) -> u64 {
        let mut shared = self.lock();
        shared.published = knobs;
        shared.version += 1;
        shared.version
    }

    /// Waits until the video loop has handled `version`. The error is the encoder's refusal,
    /// or a note that the loop did not answer in time.
    pub fn wait_applied(&self, version: u64, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut shared = self.lock();
        while shared.applied_version < version {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("the video loop did not apply the change in time".into());
            }
            shared = self
                .done
                .wait_timeout(shared, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        match &shared.failure {
            Some((failed, reason)) if *failed == version => Err(reason.clone()),
            _ => Ok(()),
        }
    }

    /// For the video loop: the newest published knobs when they are newer than `seen`.
    pub fn pending(&self, seen: u64) -> Option<(u64, PipelineKnobs)> {
        let shared = self.lock();
        (shared.version > seen).then_some((shared.version, shared.published))
    }

    /// For the video loop: reports the result of applying `version`.
    pub fn complete(&self, version: u64, result: Result<(), String>) {
        let mut shared = self.lock();
        match result {
            Ok(()) => shared.applied = shared.published,
            Err(reason) => shared.failure = Some((version, reason)),
        }
        shared.applied_version = version;
        self.done.notify_all();
    }
}

impl EncoderPipeline {
    pub fn new(state: Arc<PipelineState>) -> Self {
        Self { state }
    }

    fn knobs_of(&self, settings: &LiveSettings) -> PipelineKnobs {
        PipelineKnobs {
            // Placement is not part of the live settings yet; the area stays as it is.
            area: self.state.knobs().area,
            ..PipelineKnobs::from_live(settings)
        }
    }
}

impl PipelineHandle for EncoderPipeline {
    fn supported(&self) -> Vec<ReconfigAction> {
        let mut actions = vec![
            ReconfigAction::SetKeyframeInterval,
            ReconfigAction::SetFollow,
        ];
        if self.state.live_rate() {
            actions.push(ReconfigAction::SetRate);
        }
        if self.state.builder().is_some() {
            actions.extend([
                ReconfigAction::RebuildEncoder,
                ReconfigAction::RetargetCapture,
                ReconfigAction::RunDockCycle,
            ]);
        }
        actions
    }

    fn options(&self) -> PlanOptions {
        PlanOptions {
            live_rate: self.state.live_rate(),
        }
    }

    fn apply(&mut self, plan: &ReconfigPlan, settings: &LiveSettings) -> Result<(), String> {
        let touches_encoder = plan.actions().iter().any(|a| {
            matches!(
                a,
                ReconfigAction::SetRate
                    | ReconfigAction::SetKeyframeInterval
                    | ReconfigAction::RebuildEncoder
                    | ReconfigAction::RetargetCapture
                    | ReconfigAction::RunDockCycle
            )
        });
        if !touches_encoder {
            return Ok(());
        }
        let previous = self.state.knobs();
        let version = self.state.publish(self.knobs_of(settings));
        let timeout = if plan.class() == ChangeClass::Live {
            APPLY_TIMEOUT
        } else {
            CYCLE_APPLY_TIMEOUT
        };
        if let Err(reason) = self.state.wait_applied(version, timeout) {
            // Put the old values back so the loop and the supervisor agree again.
            let back = self.state.publish(previous);
            let _ = self.state.wait_applied(back, timeout);
            return Err(reason);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::reconfig::plan_change;
    use crate::core::reconfig::SettingsPatch;
    use crate::utils::config::load_from_path;

    fn knobs() -> PipelineKnobs {
        PipelineKnobs::for_geometry(
            DisplayGeometry::new(1280, 720, crate::protocol::display::Orientation::Landscape)
                .unwrap(),
        )
    }

    fn shipped() -> LiveSettings {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/screendock_cfg.toml");
        LiveSettings::try_from(&load_from_path(&path).unwrap()).unwrap()
    }

    /// A stand-in video loop: applies every published change, failing when told to.
    fn video_loop(
        state: Arc<PipelineState>,
        refuse_bitrate_below: u32,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut seen = 0;
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if let Some((version, knobs)) = state.pending(seen) {
                    seen = version;
                    let result = if knobs.bitrate_kbps < refuse_bitrate_below {
                        Err("encoder refused".to_string())
                    } else {
                        Ok(())
                    };
                    state.complete(version, result);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    }

    #[test]
    fn published_knobs_reach_the_loop_and_the_waiter_sees_the_result() {
        let state = PipelineState::new(knobs(), true);
        let looper = video_loop(Arc::clone(&state), 0);
        let version = state.publish(PipelineKnobs {
            bitrate_kbps: 2000,
            ..knobs()
        });
        assert_eq!(state.wait_applied(version, Duration::from_secs(2)), Ok(()));
        assert_eq!(state.knobs().bitrate_kbps, 2000);
        drop(looper);
    }

    #[test]
    fn waiting_without_a_loop_times_out() {
        let state = PipelineState::new(knobs(), true);
        let version = state.publish(knobs());
        assert!(state
            .wait_applied(version, Duration::from_millis(50))
            .is_err());
    }

    #[test]
    fn rate_support_decides_which_steps_the_pipeline_offers() {
        let live = EncoderPipeline::new(PipelineState::new(knobs(), true));
        assert!(live.supported().contains(&ReconfigAction::SetRate));
        assert!(live.options().live_rate);
        let fixed = EncoderPipeline::new(PipelineState::new(knobs(), false));
        assert!(!fixed.supported().contains(&ReconfigAction::SetRate));
        assert!(!fixed.options().live_rate);
    }

    #[test]
    fn encoder_refusal_is_reported_and_the_old_knobs_come_back() {
        let state = PipelineState::new(knobs(), true);
        let looper = video_loop(Arc::clone(&state), 1000);
        let mut pipeline = EncoderPipeline::new(Arc::clone(&state));
        let old = shipped();
        let new = SettingsPatch {
            bitrate_kbps: Some(500),
            ..SettingsPatch::default()
        }
        .apply_to(&old)
        .unwrap();
        let plan = plan_change(&old, &new);
        assert_eq!(pipeline.apply(&plan, &new), Err("encoder refused".into()));
        assert_eq!(state.knobs(), knobs());
        drop(looper);
    }

    #[test]
    fn follow_only_change_does_not_touch_the_loop() {
        let state = PipelineState::new(knobs(), true);
        let mut pipeline = EncoderPipeline::new(Arc::clone(&state));
        let old = shipped();
        let new = SettingsPatch {
            follow_system_changes: Some(false),
            ..SettingsPatch::default()
        }
        .apply_to(&old)
        .unwrap();
        assert_eq!(pipeline.apply(&plan_change(&old, &new), &new), Ok(()));
        assert!(state.pending(0).is_none());
    }
}
