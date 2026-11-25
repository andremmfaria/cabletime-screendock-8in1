//! Display port: the data an adapter reports about the desktop (outputs, rectangles and the
//! facilities available for a virtual monitor).
//!
//! The display policy in `core::display_policy` reads these facts; adapters gather them.

pub use super::types::{DisplayFacts, DisplayServer, OutputFacts, OutputId, Rect};

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
