//! Display port: the data an adapter reports about the desktop (outputs, rectangles and the
//! facilities available for a virtual monitor).
//!
//! The display policy in `core::display_policy` reads these facts; adapters gather them.

pub use super::types::{
    DisplayEvent, DisplayEventSink, DisplayFacts, DisplayProvider, DisplayServer, ExtendBackend,
    MonitorName, MonitorSpec, OutputFacts, OutputId, Rect, VdisplayError, VirtualDisplay,
    VirtualMonitor, X11Tier, DEFAULT_MONITOR_NAME,
};

impl MonitorName {
    /// Longest name a monitor gets.
    pub const MAX_LEN: usize = 32;

    /// Keeps printable ASCII without path separators, replaces everything else with `_` and
    /// cuts the name at [`Self::MAX_LEN`]. An empty result becomes `ScreenDock`.
    pub fn new(raw: &str) -> Self {
        let clean: String = raw
            .chars()
            .map(|c| {
                if c.is_ascii_graphic() && c != '/' && c != '\\' || c == ' ' {
                    c
                } else {
                    '_'
                }
            })
            .take(Self::MAX_LEN)
            .collect();
        let clean = clean.trim().to_string();
        if clean.is_empty() {
            Self(DEFAULT_MONITOR_NAME.into())
        } else {
            Self(clean)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl MonitorSpec {
    /// A spec for `rect` with a derived physical size.
    pub fn new(name: MonitorName, rect: Rect) -> Self {
        Self {
            name,
            rect,
            size_mm: None,
        }
    }

    /// Sets the physical size in millimetres.
    pub fn with_size_mm(mut self, width: u32, height: u32) -> Self {
        self.size_mm = Some((width, height));
        self
    }

    /// The physical size: the given one, else derived from 96 dots per inch.
    pub fn physical_mm(&self) -> (u32, u32) {
        self.size_mm.unwrap_or_else(|| {
            let mm = |px: u32| (u64::from(px) * 254 / 960) as u32;
            (mm(self.rect.width), mm(self.rect.height))
        })
    }
}

impl Rect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The x coordinate just past the right edge.
    pub fn right(&self) -> i32 {
        self.x
            .saturating_add(i32::try_from(self.width).unwrap_or(i32::MAX))
    }

    /// The y coordinate just past the bottom edge.
    pub fn bottom(&self) -> i32 {
        self.y
            .saturating_add(i32::try_from(self.height).unwrap_or(i32::MAX))
    }
}

impl DisplayFacts {
    /// The output the anchor name selects: `primary` picks the primary output, else the first
    /// one; any other name must match an output name exactly.
    pub fn find_output(&self, anchor: &str) -> Option<&OutputFacts> {
        if anchor == "primary" {
            self.outputs
                .iter()
                .find(|o| o.primary)
                .or_else(|| self.outputs.first())
        } else {
            self.outputs.iter().find(|o| o.name == anchor)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_names_are_sanitized_and_never_empty() {
        assert_eq!(MonitorName::new("HDMI Extender").as_str(), "HDMI Extender");
        assert_eq!(MonitorName::new("a/b\\c\n").as_str(), "a_b_c_");
        assert_eq!(MonitorName::new("   ").as_str(), "ScreenDock");
        assert_eq!(MonitorName::new(&"x".repeat(500)).as_str().len(), 32);
    }

    #[test]
    fn physical_size_defaults_to_96_dpi() {
        let spec = MonitorSpec::new(MonitorName::new(""), Rect::new(0, 0, 960, 480));
        assert_eq!(spec.physical_mm(), (254, 127));
        assert_eq!(spec.with_size_mm(170, 95).physical_mm(), (170, 95));
    }

    fn output(id: u32, name: &str, primary: bool) -> OutputFacts {
        OutputFacts {
            id: OutputId(id),
            name: name.into(),
            rect: Rect::new(id as i32 * 1920, 0, 1920, 1080),
            primary,
        }
    }

    #[test]
    fn rect_edges_are_just_past_the_rectangle() {
        let rect = Rect::new(-100, 50, 200, 80);
        assert_eq!((rect.right(), rect.bottom()), (100, 130));
    }

    #[test]
    fn anchor_primary_prefers_the_primary_output_then_the_first() {
        let facts = DisplayFacts {
            outputs: vec![output(0, "DP-1", false), output(1, "HDMI-1", true)],
            ..DisplayFacts::default()
        };
        assert_eq!(facts.find_output("primary").unwrap().name, "HDMI-1");
        assert_eq!(facts.find_output("DP-1").unwrap().id, OutputId(0));
        assert!(facts.find_output("VGA-1").is_none());

        let none_primary = DisplayFacts {
            outputs: vec![output(0, "DP-1", false), output(1, "HDMI-1", false)],
            ..DisplayFacts::default()
        };
        assert_eq!(none_primary.find_output("primary").unwrap().name, "DP-1");
        assert!(DisplayFacts::default().find_output("primary").is_none());
    }
}
