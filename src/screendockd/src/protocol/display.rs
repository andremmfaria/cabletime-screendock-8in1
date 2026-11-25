//! Resolution picker: validated panel geometry that drives the encode size, capture scaling and
//! the video header's width and height.
//!
//! `display.width`, `display.height` and `display.orientation` from the config are the encoded
//! picture size exactly as written. Orientation is a consistency check on those numbers, not a
//! rotation: a landscape picture is wider than tall, a portrait picture is taller than wide.
//! The video header carries the encoded picture's width and height unchanged.

use thiserror::Error;

use crate::protocol::framing::{PictureSize, PixelHeight, PixelWidth};
use crate::utils::config::DisplayConfig;

/// Smallest accepted edge length in pixels (one H.264 macroblock).
pub const MIN_DIMENSION: u32 = 16;

/// Largest accepted edge length in pixels, within what Constrained Baseline encoders handle in
/// practice.
pub const MAX_DIMENSION: u32 = 4096;

/// Orientation of the encoded picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Landscape,
    Portrait,
}

/// Errors from validating the display settings.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DisplayError {
    /// The orientation string is neither `landscape` nor `portrait`.
    #[error("display.orientation must be \"landscape\" or \"portrait\", got {0:?}")]
    UnknownOrientation(String),
    /// A dimension is outside `MIN_DIMENSION..=MAX_DIMENSION`.
    #[error("display {axis} {value} is outside {MIN_DIMENSION}..={MAX_DIMENSION}")]
    OutOfRange { axis: &'static str, value: u32 },
    /// A dimension is odd; yuv420p chroma subsampling needs even sizes.
    #[error("display {axis} {value} must be even")]
    Odd { axis: &'static str, value: u32 },
    /// The width and height disagree with the stated orientation.
    #[error("{width}x{height} is not a {orientation:?} size")]
    OrientationMismatch {
        width: u32,
        height: u32,
        orientation: Orientation,
    },
}

/// Validated encoded-picture geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayGeometry {
    width: PixelWidth,
    height: PixelHeight,
    orientation: Orientation,
}

impl Orientation {
    /// Parses the config string, case-insensitively.
    pub fn parse(s: &str) -> Result<Self, DisplayError> {
        match s.to_ascii_lowercase().as_str() {
            "landscape" => Ok(Orientation::Landscape),
            "portrait" => Ok(Orientation::Portrait),
            _ => Err(DisplayError::UnknownOrientation(s.to_string())),
        }
    }
}

fn check_axis(axis: &'static str, value: u32) -> Result<(), DisplayError> {
    if !(MIN_DIMENSION..=MAX_DIMENSION).contains(&value) {
        return Err(DisplayError::OutOfRange { axis, value });
    }
    if !value.is_multiple_of(2) {
        return Err(DisplayError::Odd { axis, value });
    }
    Ok(())
}

impl DisplayGeometry {
    /// Validates a size. A square size is accepted for either orientation.
    pub fn new(width: u32, height: u32, orientation: Orientation) -> Result<Self, DisplayError> {
        check_axis("width", width)?;
        check_axis("height", height)?;
        let consistent = match orientation {
            Orientation::Landscape => width >= height,
            Orientation::Portrait => height >= width,
        };
        if !consistent {
            return Err(DisplayError::OrientationMismatch {
                width,
                height,
                orientation,
            });
        }
        Ok(Self {
            width: PixelWidth(width),
            height: PixelHeight(height),
            orientation,
        })
    }

    /// Validates the `[display]` config section.
    pub fn from_config(display: &DisplayConfig) -> Result<Self, DisplayError> {
        Self::new(
            display.width,
            display.height,
            Orientation::parse(&display.orientation)?,
        )
    }

    pub fn width(&self) -> PixelWidth {
        self.width
    }

    pub fn height(&self) -> PixelHeight {
        self.height
    }

    pub fn orientation(&self) -> Orientation {
        self.orientation
    }

    /// The width and height carried in the header of every video message.
    pub fn size(&self) -> PictureSize {
        PictureSize {
            width: self.width,
            height: self.height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_size_is_width_and_height_for_native_900_and_portrait_sizes() {
        let native = DisplayGeometry::new(1280, 720, Orientation::Landscape).unwrap();
        assert_eq!(native.size(), PictureSize::new(1280, 720));
        let narrow = DisplayGeometry::new(900, 600, Orientation::Landscape).unwrap();
        assert_eq!(narrow.size(), PictureSize::new(900, 600));
        let portrait = DisplayGeometry::new(720, 1280, Orientation::Portrait).unwrap();
        assert_eq!(portrait.size(), PictureSize::new(720, 1280));
        assert_eq!(portrait.width(), PixelWidth(720));
        assert_eq!(portrait.height(), PixelHeight(1280));
    }

    #[test]
    fn orientation_must_match_shape() {
        assert!(matches!(
            DisplayGeometry::new(720, 1280, Orientation::Landscape),
            Err(DisplayError::OrientationMismatch { .. })
        ));
        assert!(matches!(
            DisplayGeometry::new(1280, 720, Orientation::Portrait),
            Err(DisplayError::OrientationMismatch { .. })
        ));
        assert!(DisplayGeometry::new(720, 720, Orientation::Portrait).is_ok());
    }

    #[test]
    fn rejects_bad_sizes() {
        assert_eq!(
            DisplayGeometry::new(0, 720, Orientation::Landscape),
            Err(DisplayError::OutOfRange {
                axis: "width",
                value: 0
            })
        );
        assert!(matches!(
            DisplayGeometry::new(1280, 8192, Orientation::Portrait),
            Err(DisplayError::OutOfRange { axis: "height", .. })
        ));
        assert_eq!(
            DisplayGeometry::new(1281, 720, Orientation::Landscape),
            Err(DisplayError::Odd {
                axis: "width",
                value: 1281
            })
        );
    }

    #[test]
    fn parses_orientation_and_config() {
        assert_eq!(Orientation::parse("Portrait"), Ok(Orientation::Portrait));
        assert_eq!(
            Orientation::parse("sideways"),
            Err(DisplayError::UnknownOrientation("sideways".into()))
        );
        let cfg = DisplayConfig {
            width: 1280,
            height: 720,
            orientation: "landscape".into(),
        };
        assert_eq!(
            DisplayGeometry::from_config(&cfg).unwrap().size(),
            PictureSize::new(1280, 720)
        );
    }
}
