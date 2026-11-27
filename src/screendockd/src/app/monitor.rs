//! The virtual monitor of a running daemon, kept in step with the live settings.

use std::collections::BTreeMap;
use std::sync::mpsc::Sender;

pub use super::types::{MonitorManager, MonitorPipeline};
use crate::app::supervisor::PipelineHandle;
use crate::core::display_policy::{decide, place_for_backend, place_monitor};
use crate::core::reconfig::{LiveSettings, PlanOptions, ReconfigAction, ReconfigPlan};
use crate::core::types::DisplayDecision;
use crate::ports::capture::CaptureArea;
use crate::ports::control::Command;
use crate::ports::display::{DisplayProvider, MonitorName, MonitorSpec, Rect};
use crate::utils::types::{DisplayMode, FallbackPolicy, VirtualDisplayConfig};
use tracing::warn;

use super::types::EncoderPipeline;

impl MonitorManager {
    /// A manager that builds monitors through `provider`. `base` supplies the settings that
    /// cannot change while the daemon runs.
    pub fn new(provider: Box<dyn DisplayProvider>, base: VirtualDisplayConfig) -> Self {
        Self {
            provider,
            base,
            monitor: None,
            events: None,
        }
    }

    /// Reports changes made to the monitor from outside the daemon as
    /// [`Command::Display`], for the monitor that exists now and every later one.
    pub fn set_events(&mut self, events: Sender<Command>) {
        self.events = Some(events);
        self.follow();
    }

    fn follow(&mut self) {
        if let (Some(events), Some(monitor)) = (self.events.clone(), self.monitor.as_mut()) {
            monitor.follow(Box::new(move |event| {
                let _ = events.send(Command::Display(event));
            }));
        }
    }

    /// Records that the monitor was moved or resized from outside the daemon.
    pub fn observe(&mut self, rect: Rect) {
        if let Some(monitor) = self.monitor.as_mut() {
            monitor.observe(rect);
        }
    }

    /// Removes the monitor, so the whole desktop is captured again.
    pub fn release(&mut self) {
        self.monitor = None;
    }

    /// The area to capture now: the monitor's stream when it has one, else its rectangle,
    /// or the whole desktop.
    pub fn area(&self) -> CaptureArea {
        self.monitor
            .as_ref()
            .map_or(CaptureArea::Whole, |m| match m.pipewire_node() {
                Some(node) => {
                    let rect = m.rect();
                    CaptureArea::Stream {
                        node,
                        width: rect.width,
                        height: rect.height,
                    }
                }
                None => CaptureArea::Region(m.rect()),
            })
    }

    fn config_for(&self, settings: &LiveSettings) -> VirtualDisplayConfig {
        VirtualDisplayConfig {
            mode: settings.mode,
            placement: settings.placement,
            anchor: settings.anchor.clone(),
            x: settings.x,
            y: settings.y,
            name: settings.name.clone(),
            wayland_backend: settings.wayland_backend,
            follow_system_changes: settings.follow_system_changes,
            ..self.base.clone()
        }
    }

    /// Where the monitor goes for `settings`: next to the anchor, or at the origin for the
    /// mechanisms that place their own monitors.
    fn rect_for(&self, settings: &LiveSettings) -> Result<Rect, String> {
        let facts = self.provider.facts().map_err(|e| e.to_string())?;
        let config = VirtualDisplayConfig {
            fallback: FallbackPolicy::Fail,
            ..self.config_for(settings)
        };
        let rect = match decide(&facts, &config).map_err(|e| e.to_string())? {
            DisplayDecision::Extend(backend) => place_for_backend(&facts, settings, backend),
            DisplayDecision::Mirror { .. } => place_monitor(&facts, settings),
        };
        rect.map_err(|e| e.to_string())
    }

    fn spec_for(&self, settings: &LiveSettings) -> Result<MonitorSpec, String> {
        let rect = self.rect_for(settings)?;
        Ok(MonitorSpec::new(MonitorName::new(&settings.name), rect))
    }

    /// Checks that a monitor could be made for `settings`, without making one.
    pub fn check(&self, settings: &LiveSettings) -> Result<(), String> {
        self.rect_for(settings).map(drop)
    }

    fn create(&mut self, settings: &LiveSettings) -> Result<(), String> {
        let facts = self.provider.facts().map_err(|e| e.to_string())?;
        let config = VirtualDisplayConfig {
            fallback: FallbackPolicy::Fail,
            ..self.config_for(settings)
        };
        let DisplayDecision::Extend(backend) =
            decide(&facts, &config).map_err(|e| e.to_string())?
        else {
            return Err("the mode is mirror".into());
        };
        let rect = place_for_backend(&facts, settings, backend).map_err(|e| e.to_string())?;
        let spec = MonitorSpec::new(MonitorName::new(&settings.name), rect);
        let display = self
            .provider
            .virtual_display(backend)
            .map_err(|e| e.to_string())?;
        self.monitor = Some(display.create(&spec).map_err(|e| e.to_string())?);
        self.follow();
        Ok(())
    }

    /// Makes the monitor the settings ask for at daemon start. A desktop that cannot host
    /// one mirrors when the fallback policy says so.
    pub fn start(&mut self, settings: &LiveSettings) -> Result<CaptureArea, String> {
        if settings.mode == DisplayMode::Extend {
            if let Err(reason) = self.create(settings) {
                if self.base.fallback == FallbackPolicy::Fail {
                    return Err(reason);
                }
                warn!(%reason, "no virtual monitor; mirroring the desktop instead");
            }
        }
        Ok(self.area())
    }

    /// Makes the monitor match `settings`: created or removed with the mode, moved or
    /// resized when the rectangle differs.
    pub fn reconcile(&mut self, settings: &LiveSettings) -> Result<CaptureArea, String> {
        match (settings.mode, self.monitor.is_some()) {
            (DisplayMode::Mirror, true) => self.monitor = None,
            (DisplayMode::Extend, false) => self.create(settings)?,
            (DisplayMode::Extend, true) => self.fit(settings)?,
            (DisplayMode::Mirror, false) => {}
        }
        Ok(self.area())
    }

    fn fit(&mut self, settings: &LiveSettings) -> Result<(), String> {
        let spec = self.spec_for(settings)?;
        if let Some(monitor) = self.monitor.as_mut() {
            if monitor.rect() != spec.rect {
                monitor.resize(&spec).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// Carries out the monitor steps of `plan` for `settings` and returns the area to
    /// capture afterwards. Removing a monitor that does not exist is not an error.
    pub fn apply(
        &mut self,
        plan: &ReconfigPlan,
        settings: &LiveSettings,
    ) -> Result<CaptureArea, String> {
        let has = |a| plan.actions().contains(&a);
        if has(ReconfigAction::DestroyMonitor) || has(ReconfigAction::RecreateMonitor) {
            self.monitor = None;
        }
        if has(ReconfigAction::CreateMonitor) || has(ReconfigAction::RecreateMonitor) {
            self.create(settings)?;
        } else if settings.mode == DisplayMode::Extend && self.monitor.is_some() {
            self.fit(settings)?;
        }
        Ok(self.area())
    }
}

impl MonitorPipeline {
    pub fn new(inner: EncoderPipeline, monitors: MonitorManager) -> Self {
        Self { inner, monitors }
    }
}

impl PipelineHandle for MonitorPipeline {
    fn supported(&self) -> Vec<ReconfigAction> {
        let mut actions = self.inner.supported();
        actions.extend([
            ReconfigAction::CreateMonitor,
            ReconfigAction::DestroyMonitor,
            ReconfigAction::RecreateMonitor,
            ReconfigAction::ResizeMonitor,
        ]);
        actions
    }

    fn options(&self) -> PlanOptions {
        self.inner.options()
    }

    fn prepare(&mut self, plan: &ReconfigPlan, settings: &LiveSettings) -> Result<(), String> {
        if settings.mode == DisplayMode::Extend
            && plan.actions().iter().any(|a| {
                matches!(
                    a,
                    ReconfigAction::CreateMonitor | ReconfigAction::RecreateMonitor
                )
            })
        {
            self.monitors.check(settings)?;
        }
        self.inner.prepare(plan, settings)
    }

    fn apply(&mut self, plan: &ReconfigPlan, settings: &LiveSettings) -> Result<(), String> {
        let area = self.monitors.apply(plan, settings)?;
        self.inner.set_area(area);
        self.inner.apply(plan, settings)
    }

    fn verify(&mut self, plan: &ReconfigPlan, settings: &LiveSettings) -> Result<(), String> {
        self.inner.verify(plan, settings)
    }

    fn restore(&mut self, plan: &ReconfigPlan, settings: &LiveSettings) -> Result<(), String> {
        let area = self.monitors.apply(plan, settings)?;
        self.inner.set_area(area);
        self.inner.restore(plan, settings)
    }

    fn display_fields(&self) -> BTreeMap<String, String> {
        self.inner.display_fields()
    }

    fn resync(&mut self, settings: &LiveSettings) -> Result<(), String> {
        let area = self.monitors.reconcile(settings)?;
        self.inner.set_area(area);
        self.inner.resync(settings)
    }

    fn observed(&mut self, rect: Rect) {
        self.monitors.observe(rect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::reconfig::{plan_change_with, SettingsPatch};
    use crate::ports::display::{
        DisplayFacts, DisplayServer, ExtendBackend, OutputFacts, OutputId, Rect, VdisplayError,
        VirtualDisplay, VirtualMonitor, X11Tier,
    };
    use crate::utils::config::load_from_path;
    use std::sync::{Arc, Mutex};

    type Log = Arc<Mutex<Vec<String>>>;

    struct FakeMonitor {
        rect: Rect,
        log: Log,
    }

    impl VirtualMonitor for FakeMonitor {
        fn rect(&self) -> Rect {
            self.rect
        }
        fn resize(&mut self, spec: &MonitorSpec) -> Result<(), VdisplayError> {
            self.log
                .lock()
                .unwrap()
                .push(format!("resize {:?}", spec.rect));
            self.rect = spec.rect;
            Ok(())
        }
    }

    impl Drop for FakeMonitor {
        fn drop(&mut self) {
            self.log.lock().unwrap().push("drop".into());
        }
    }

    struct FakeDisplay(Log);

    impl VirtualDisplay for FakeDisplay {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn create(&self, spec: &MonitorSpec) -> Result<Box<dyn VirtualMonitor>, VdisplayError> {
            self.0
                .lock()
                .unwrap()
                .push(format!("create {:?}", spec.rect));
            Ok(Box::new(FakeMonitor {
                rect: spec.rect,
                log: self.0.clone(),
            }))
        }
    }

    struct FakeProvider {
        log: Log,
        server: DisplayServer,
    }

    impl DisplayProvider for FakeProvider {
        fn facts(&self) -> Result<DisplayFacts, VdisplayError> {
            Ok(DisplayFacts {
                server: self.server,
                randr_version: Some((1, 5)),
                outputs: vec![OutputFacts {
                    id: OutputId(1),
                    name: "DUMMY0".into(),
                    rect: Rect::new(0, 0, 1920, 1080),
                    primary: true,
                }],
                ..DisplayFacts::default()
            })
        }
        fn virtual_display(
            &self,
            backend: ExtendBackend,
        ) -> Result<Box<dyn VirtualDisplay>, VdisplayError> {
            assert_eq!(backend, ExtendBackend::X11Randr(X11Tier::BareMonitor));
            Ok(Box::new(FakeDisplay(self.log.clone())))
        }
        fn mirror(&self, _: &str) -> Result<Box<dyn VirtualDisplay>, VdisplayError> {
            unreachable!("monitor management never mirrors an output")
        }
    }

    fn manager(server: DisplayServer, base: VirtualDisplayConfig) -> (MonitorManager, Log) {
        let log = Log::default();
        let provider = FakeProvider {
            log: log.clone(),
            server,
        };
        (MonitorManager::new(Box::new(provider), base), log)
    }

    fn shipped() -> LiveSettings {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/screendock_cfg.toml"
        );
        LiveSettings::try_from(&load_from_path(std::path::Path::new(path)).unwrap()).unwrap()
    }

    fn switch(old: &LiveSettings, mode: DisplayMode) -> (LiveSettings, ReconfigPlan) {
        let new = SettingsPatch {
            mode: Some(mode),
            ..SettingsPatch::default()
        }
        .apply_to(old)
        .unwrap();
        let plan = plan_change_with(old, &new, PlanOptions::default());
        (new, plan)
    }

    #[test]
    fn switching_to_extend_creates_the_monitor_right_of_the_anchor_and_back_removes_it() {
        let (mut monitors, log) = manager(DisplayServer::X11, VirtualDisplayConfig::default());
        let mirror = shipped();
        assert_eq!(monitors.start(&mirror), Ok(CaptureArea::Whole));
        let (extend, plan) = switch(&mirror, DisplayMode::Extend);
        let area = monitors.apply(&plan, &extend).unwrap();
        assert_eq!(area, CaptureArea::Region(Rect::new(1920, 0, 1280, 720)));
        let (back, plan) = switch(&extend, DisplayMode::Mirror);
        assert_eq!(monitors.apply(&plan, &back), Ok(CaptureArea::Whole));
        assert_eq!(
            *log.lock().unwrap(),
            [
                "create Rect { x: 1920, y: 0, width: 1280, height: 720 }",
                "drop"
            ]
        );
    }

    #[test]
    fn a_size_change_in_extend_mode_resizes_the_monitor() {
        let (mut monitors, log) = manager(DisplayServer::X11, VirtualDisplayConfig::default());
        let (extend, plan) = switch(&shipped(), DisplayMode::Extend);
        monitors.apply(&plan, &extend).unwrap();
        let bigger = SettingsPatch {
            width: Some(900),
            height: Some(720),
            ..SettingsPatch::default()
        }
        .apply_to(&extend)
        .unwrap();
        let plan = plan_change_with(&extend, &bigger, PlanOptions::default());
        let area = monitors.apply(&plan, &bigger).unwrap();
        assert_eq!(area, CaptureArea::Region(Rect::new(1920, 0, 900, 720)));
        assert!(log.lock().unwrap()[1].starts_with("resize"));
    }

    #[test]
    fn a_desktop_without_a_session_refuses_extend_and_changes_nothing() {
        let (mut monitors, log) = manager(DisplayServer::None, VirtualDisplayConfig::default());
        let (extend, plan) = switch(&shipped(), DisplayMode::Extend);
        assert!(monitors.check(&extend).is_err());
        assert!(monitors.apply(&plan, &extend).is_err());
        assert_eq!(monitors.area(), CaptureArea::Whole);
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn start_in_extend_mode_falls_back_to_mirror_or_fails_by_policy() {
        let mut extend = shipped();
        extend.mode = DisplayMode::Extend;
        let (mut monitors, _) = manager(DisplayServer::None, VirtualDisplayConfig::default());
        assert_eq!(monitors.start(&extend), Ok(CaptureArea::Whole));
        let strict = VirtualDisplayConfig {
            fallback: FallbackPolicy::Fail,
            ..VirtualDisplayConfig::default()
        };
        let (mut monitors, _) = manager(DisplayServer::None, strict);
        assert!(monitors.start(&extend).is_err());
    }

    #[test]
    fn resync_brings_the_monitor_in_line_with_the_settings() {
        let (mut monitors, log) = manager(DisplayServer::X11, VirtualDisplayConfig::default());
        let (extend, _) = switch(&shipped(), DisplayMode::Extend);
        assert!(matches!(
            monitors.reconcile(&extend),
            Ok(CaptureArea::Region(_))
        ));
        assert!(matches!(
            monitors.reconcile(&extend),
            Ok(CaptureArea::Region(_))
        ));
        assert_eq!(monitors.reconcile(&shipped()), Ok(CaptureArea::Whole));
        assert_eq!(log.lock().unwrap().len(), 2, "one create, one drop");
    }

    struct StreamMonitor(Rect);

    impl VirtualMonitor for StreamMonitor {
        fn rect(&self) -> Rect {
            self.0
        }
        fn resize(&mut self, spec: &MonitorSpec) -> Result<(), VdisplayError> {
            self.0 = spec.rect;
            Ok(())
        }
        fn pipewire_node(&self) -> Option<u32> {
            Some(45)
        }
    }

    struct StreamDisplay;

    impl VirtualDisplay for StreamDisplay {
        fn name(&self) -> &'static str {
            "stream"
        }
        fn create(&self, spec: &MonitorSpec) -> Result<Box<dyn VirtualMonitor>, VdisplayError> {
            Ok(Box::new(StreamMonitor(spec.rect)))
        }
    }

    /// A GNOME session: no output list, Mutter's ScreenCast on the bus.
    struct GnomeProvider;

    impl DisplayProvider for GnomeProvider {
        fn facts(&self) -> Result<DisplayFacts, VdisplayError> {
            Ok(DisplayFacts {
                server: DisplayServer::Wayland,
                desktop: "GNOME".into(),
                mutter_screencast: true,
                ..DisplayFacts::default()
            })
        }
        fn virtual_display(
            &self,
            backend: ExtendBackend,
        ) -> Result<Box<dyn VirtualDisplay>, VdisplayError> {
            assert_eq!(backend, ExtendBackend::Mutter);
            Ok(Box::new(StreamDisplay))
        }
        fn mirror(&self, _: &str) -> Result<Box<dyn VirtualDisplay>, VdisplayError> {
            unreachable!("monitor management never mirrors an output")
        }
    }

    #[test]
    fn stream_backed_monitor_is_captured_by_node_without_an_anchor_output() {
        let mut monitors =
            MonitorManager::new(Box::new(GnomeProvider), VirtualDisplayConfig::default());
        let (extend, _) = switch(&shipped(), DisplayMode::Extend);
        assert_eq!(
            monitors.reconcile(&extend),
            Ok(CaptureArea::Stream {
                node: 45,
                width: 1280,
                height: 720
            })
        );
        assert_eq!(monitors.reconcile(&shipped()), Ok(CaptureArea::Whole));
    }
}
