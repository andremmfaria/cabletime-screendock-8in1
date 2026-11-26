//! The mirror display: the dock shows an existing output.

pub use super::types::{MirrorDisplay, MirrorMonitor};
use crate::ports::display::{MonitorSpec, Rect, VdisplayError, VirtualDisplay, VirtualMonitor};

impl MirrorDisplay {
    /// A display that mirrors the output occupying `rect`.
    pub fn new(rect: Rect) -> Self {
        Self { rect }
    }
}

impl VirtualDisplay for MirrorDisplay {
    fn name(&self) -> &'static str {
        "mirror"
    }

    fn create(&self, _spec: &MonitorSpec) -> Result<Box<dyn VirtualMonitor>, VdisplayError> {
        Ok(Box::new(MirrorMonitor { rect: self.rect }))
    }
}

impl VirtualMonitor for MirrorMonitor {
    fn rect(&self) -> Rect {
        self.rect
    }

    /// A mirrored output keeps its own size; only the dock picture changes.
    fn resize(&mut self, _spec: &MonitorSpec) -> Result<(), VdisplayError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::display::MonitorName;

    #[test]
    fn mirror_monitor_keeps_the_rectangle_of_the_mirrored_output() {
        let rect = Rect::new(0, 0, 1920, 1080);
        let display = MirrorDisplay::new(rect);
        let spec = MonitorSpec::new(MonitorName::new(""), Rect::new(1920, 0, 1280, 720));
        let mut monitor = display.create(&spec).unwrap();
        assert_eq!(display.name(), "mirror");
        assert_eq!(monitor.rect(), rect);
        monitor.resize(&spec).unwrap();
        assert_eq!(monitor.rect(), rect);
    }
}
