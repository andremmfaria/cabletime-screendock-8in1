//! Hand-over of live encoder settings from the supervisor to the video loop.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, MutexGuard};
use std::time::{Duration, Instant};

use super::types::{
    DisplayState, PipelineShared, StreamWatch, APPLY_TIMEOUT, CYCLE_APPLY_TIMEOUT,
    STREAM_MISMATCH_FRAMES, STREAM_SETTLE, VERIFY_MESSAGES, VERIFY_WINDOW,
};
pub use super::types::{EncoderBuilder, EncoderPipeline, PipelineKnobs, PipelineState};
use crate::app::supervisor::PipelineHandle;
use crate::core::display_info::DisplayInfo;
use crate::core::reconfig::{ChangeClass, LiveSettings, PlanOptions, ReconfigAction, ReconfigPlan};
use crate::core::session::SessionHandle;
use crate::ports::capture::{CaptureArea, CaptureTarget, Frame, PixelFormat};
use crate::ports::control::Command;
use crate::ports::display::{DisplayEvent, Rect};
use crate::ports::encode::{EncodeError, Encoder, EncoderSettings, DEFAULT_FPS};
use crate::ports::transport::EdidStore;
use crate::protocol::display::DisplayGeometry;
use crate::utils::config::X264Preset;
use crate::utils::types::EdidSource;
use tracing::warn;

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
                force_cycle: false,
            }),
            done: std::sync::Condvar::new(),
            live_rate,
            builder,
            session: std::sync::Mutex::new(None),
            written: std::sync::atomic::AtomicU64::new(0),
            display: std::sync::Mutex::new(DisplayState {
                store: None,
                prefer: EdidSource::Dock,
                fallback: (0, 0),
                cached: None,
                dock: None,
            }),
            stream: std::sync::Mutex::new(StreamWatch {
                area: knobs.area,
                since: Instant::now(),
                mismatched: 0,
                reported: None,
                settle: STREAM_SETTLE,
            }),
            events: std::sync::Mutex::new(None),
        })
    }

    /// Reports a captured stream that changes its size to `events` as a display event.
    pub fn set_events(&self, events: std::sync::mpsc::Sender<Command>) {
        *self.events.lock().unwrap_or_else(|e| e.into_inner()) = Some(events);
    }

    /// Changes how long frames at the old size are ignored after the capture area changed.
    pub fn set_stream_settle(&self, settle: Duration) {
        self.stream.lock().unwrap_or_else(|e| e.into_inner()).settle = settle;
    }

    /// For the video loop: a frame of `width` by `height` arrived. When the capture area is
    /// a stream and the frames keep another size than the one asked for, the compositor
    /// renegotiated the stream, and the new size is reported once as a display event. Frames
    /// still at the old size right after the daemon changed the area are not a change.
    pub fn note_frame_size(&self, width: u32, height: u32) {
        let area = self.knobs().area;
        let mut watch = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        if watch.area != area {
            *watch = StreamWatch {
                area,
                since: Instant::now(),
                mismatched: 0,
                reported: None,
                settle: watch.settle,
            };
        }
        let CaptureArea::Stream {
            width: asked_width,
            height: asked_height,
            ..
        } = area
        else {
            return;
        };
        if (width, height) == (asked_width, asked_height) {
            watch.mismatched = 0;
            watch.reported = None;
            return;
        }
        if watch.since.elapsed() < watch.settle {
            return;
        }
        watch.mismatched += 1;
        if watch.mismatched < STREAM_MISMATCH_FRAMES || watch.reported == Some((width, height)) {
            return;
        }
        watch.reported = Some((width, height));
        drop(watch);
        let events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(events) = events.as_ref() {
            let rect = Rect::new(0, 0, width, height);
            let _ = events.send(Command::Display(DisplayEvent::Changed {
                rect,
                resized: true,
                moved: false,
            }));
        }
    }

    /// Sets where the dock's EDID is cached and what to report when none is known, and loads
    /// the cached bytes.
    pub fn configure_display(
        &self,
        store: Option<Arc<dyn EdidStore>>,
        prefer: EdidSource,
        fallback: (u16, u16),
    ) {
        let cached = store.as_ref().and_then(|s| s.load());
        let mut display = self.display.lock().unwrap_or_else(|e| e.into_inner());
        *display = DisplayState {
            store,
            prefer,
            fallback,
            cached,
            dock: display.dock.take(),
        };
    }

    /// For the video loop: the dock sent its EDID. A copy that differs from the cache is
    /// stored.
    pub fn note_dock_edid(&self, bytes: Vec<u8>) {
        let mut display = self.display.lock().unwrap_or_else(|e| e.into_inner());
        if display.dock.as_ref() == Some(&bytes) {
            return;
        }
        if display.cached.as_ref() != Some(&bytes) {
            if let Some(store) = &display.store {
                if let Err(e) = store.save(&bytes) {
                    warn!(error = %e, "couldn't cache the dock's EDID");
                }
            }
            display.cached = Some(bytes.clone());
        }
        display.dock = Some(bytes);
    }

    /// The read-only display facts: source, name, native mode and mode list.
    pub fn display_fields(&self) -> BTreeMap<String, String> {
        let display = self.display.lock().unwrap_or_else(|e| e.into_inner());
        DisplayInfo::resolve(
            display.dock.as_deref(),
            display.cached.as_deref(),
            display.prefer,
            display.fallback,
        )
        .fields()
    }

    /// For the video loop: the session it runs against.
    pub fn attach_session(&self, session: SessionHandle) {
        *self.session.lock().unwrap_or_else(|e| e.into_inner()) = Some(session);
    }

    fn session(&self) -> Option<SessionHandle> {
        self.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// For the video loop: counts one video message that reached the dock.
    pub fn note_written(&self) {
        self.written.fetch_add(1, Ordering::Relaxed);
    }

    fn written(&self) -> u64 {
        self.written.load(Ordering::Relaxed)
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
        shared.force_cycle = false;
        shared.version += 1;
        shared.version
    }

    /// Like [`publish`](Self::publish), and the video loop runs the dock cycle at the
    /// published size even when the size is the one it already has.
    pub fn publish_with_cycle(&self, knobs: PipelineKnobs) -> u64 {
        let mut shared = self.lock();
        shared.published = knobs;
        shared.force_cycle = true;
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

    /// For the video loop: the newest published knobs when they are newer than `seen`, and
    /// whether the dock cycle is demanded.
    pub fn pending(&self, seen: u64) -> Option<(u64, PipelineKnobs, bool)> {
        let shared = self.lock();
        (shared.version > seen).then_some((shared.version, shared.published, shared.force_cycle))
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
        Self {
            state,
            baseline: None,
            area: None,
        }
    }

    /// Sets the capture area later changes carry, until another one is set.
    pub fn set_area(&mut self, area: CaptureArea) {
        self.area = Some(area);
    }

    /// Publishes `knobs`, waits for the video loop to apply them and, when that fails, puts
    /// the loop's previous knobs back.
    fn publish_and_wait(
        &self,
        knobs: PipelineKnobs,
        cycle: bool,
        timeout: Duration,
    ) -> Result<(), String> {
        let previous = self.state.knobs();
        let version = if cycle {
            self.state.publish_with_cycle(knobs)
        } else {
            self.state.publish(knobs)
        };
        if let Err(reason) = self.state.wait_applied(version, timeout) {
            // Put the old values back so the loop and the supervisor agree again.
            let back = self.state.publish(previous);
            let _ = self.state.wait_applied(back, timeout);
            return Err(reason);
        }
        Ok(())
    }

    fn knobs_of(&self, settings: &LiveSettings) -> PipelineKnobs {
        PipelineKnobs {
            area: self.area.unwrap_or_else(|| self.state.knobs().area),
            ..PipelineKnobs::from_live(settings)
        }
    }
}

impl PipelineHandle for EncoderPipeline {
    fn display_fields(&self) -> BTreeMap<String, String> {
        self.state.display_fields()
    }

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
        if let Some(session) = self.state.session() {
            self.baseline = Some((session.losses(), self.state.written()));
        }
        let timeout = if plan.class() == ChangeClass::Live {
            APPLY_TIMEOUT
        } else {
            CYCLE_APPLY_TIMEOUT
        };
        self.publish_and_wait(self.knobs_of(settings), false, timeout)?;
        Ok(())
    }

    fn prepare(&mut self, plan: &ReconfigPlan, settings: &LiveSettings) -> Result<(), String> {
        let no_cycle = plan.actions().contains(&ReconfigAction::RunDockCycle)
            && self
                .state
                .session()
                .is_some_and(|session| !session.can_renegotiate());
        if no_cycle {
            return Err(
                "the session runs without a handshake, so the dock cycle is unavailable".into(),
            );
        }
        if !plan.actions().contains(&ReconfigAction::RebuildEncoder) {
            return Ok(());
        }
        let builder = self
            .state
            .builder()
            .ok_or("this pipeline cannot rebuild the encoder")?;
        let knobs = self.knobs_of(settings);
        probe_build(builder, &knobs).map(drop)
    }

    fn verify(&mut self, plan: &ReconfigPlan, _settings: &LiveSettings) -> Result<(), String> {
        let Some(session) = self.state.session() else {
            return Ok(());
        };
        if plan.class() == ChangeClass::Live {
            return Ok(());
        }
        let (losses, written) = self
            .baseline
            .take()
            .unwrap_or((session.losses(), self.state.written()));
        let deadline = Instant::now() + VERIFY_WINDOW;
        loop {
            if session.losses() > losses {
                return Err("the dock lost the session after the change".into());
            }
            if let Some(reason) = session.fatal() {
                return Err(format!("the session ended after the change: {reason}"));
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if !session.is_armed() {
            return Err("the session is not armed after the change".into());
        }
        if self.state.written().saturating_sub(written) < VERIFY_MESSAGES {
            return Err("video did not flow after the change".into());
        }
        Ok(())
    }

    fn restore(&mut self, plan: &ReconfigPlan, settings: &LiveSettings) -> Result<(), String> {
        let cycle = plan.actions().contains(&ReconfigAction::RunDockCycle);
        self.publish_and_wait(self.knobs_of(settings), cycle, CYCLE_APPLY_TIMEOUT)
    }

    fn resync(&mut self, settings: &LiveSettings) -> Result<(), String> {
        self.publish_and_wait(self.knobs_of(settings), true, CYCLE_APPLY_TIMEOUT)
    }
}

/// Builds an encoder for `knobs` and encodes one black frame with it, so an encoder that
/// cannot work at that size is found before anything else changes.
pub(super) fn probe_build(
    builder: &EncoderBuilder,
    knobs: &PipelineKnobs,
) -> Result<Box<dyn Encoder>, String> {
    let mut encoder = builder
        .build(knobs.encoder_settings())
        .map_err(|e| e.to_string())?;
    let size = knobs.geometry.size();
    let (width, height) = (size.width.0, size.height.0);
    let stride = width as usize * 4;
    let frame = Frame::new(
        width,
        height,
        PixelFormat::Bgrx8888,
        stride,
        vec![0; stride * height as usize],
    )
    .map_err(|e| e.to_string())?;
    encoder.encode(&frame).map_err(|e| e.to_string())?;
    Ok(encoder)
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
                if let Some((version, knobs, _)) = state.pending(seen) {
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

    fn stream_state(settle: Duration) -> (Arc<PipelineState>, std::sync::mpsc::Receiver<Command>) {
        let knobs = PipelineKnobs {
            area: CaptureArea::Stream {
                node: 7,
                width: 1280,
                height: 720,
            },
            ..PipelineKnobs::for_geometry(
                DisplayGeometry::new(1280, 720, crate::protocol::display::Orientation::Landscape)
                    .unwrap(),
            )
        };
        let state = PipelineState::new(knobs, false);
        state.set_stream_settle(settle);
        let (tx, rx) = std::sync::mpsc::channel();
        state.set_events(tx);
        (state, rx)
    }

    #[test]
    fn a_stream_that_keeps_another_size_is_reported_once() {
        let (state, rx) = stream_state(Duration::ZERO);
        for _ in 0..20 {
            state.note_frame_size(1600, 900);
        }
        let Ok(Command::Display(DisplayEvent::Changed {
            rect,
            resized,
            moved,
        })) = rx.try_recv()
        else {
            panic!("no display event");
        };
        assert_eq!(
            (rect, resized, moved),
            (Rect::new(0, 0, 1600, 900), true, false)
        );
        assert!(rx.try_recv().is_err(), "reported twice");
    }

    #[test]
    fn a_few_frames_at_another_size_are_not_a_renegotiation() {
        let (state, rx) = stream_state(Duration::ZERO);
        for _ in 0..4 {
            state.note_frame_size(1600, 900);
        }
        state.note_frame_size(1280, 720);
        for _ in 0..4 {
            state.note_frame_size(1600, 900);
        }
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn frames_at_the_old_size_after_the_daemon_changed_the_area_are_not_reported() {
        let (state, rx) = stream_state(Duration::from_secs(60));
        for _ in 0..20 {
            state.note_frame_size(1600, 900);
        }
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn sizes_are_not_watched_unless_a_stream_is_captured() {
        let state = PipelineState::new(
            PipelineKnobs::for_geometry(
                DisplayGeometry::new(1280, 720, crate::protocol::display::Orientation::Landscape)
                    .unwrap(),
            ),
            false,
        );
        let (tx, rx) = std::sync::mpsc::channel();
        state.set_events(tx);
        state.set_stream_settle(Duration::ZERO);
        for _ in 0..20 {
            state.note_frame_size(1600, 900);
        }
        assert!(rx.try_recv().is_err());
    }
}
