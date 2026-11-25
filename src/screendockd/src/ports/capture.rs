//! Capture port: the [`CaptureBackend`] trait, the [`Frame`] it produces and the error type.
//!
//! Implementations live in `adapters::capture`.

use std::fmt;
use std::path::Path;

pub use super::types::{CaptureBackend, CaptureError, Frame, PixelFormat};

impl PixelFormat {
    /// Bytes occupied by one pixel.
    pub const fn bytes_per_pixel(self) -> usize {
        4
    }

    /// Index of the (red, green, blue) bytes within a pixel.
    const fn rgb_offsets(self) -> (usize, usize, usize) {
        match self {
            PixelFormat::Bgrx8888 | PixelFormat::Bgra8888 => (2, 1, 0),
            PixelFormat::Rgbx8888 | PixelFormat::Rgba8888 => (0, 1, 2),
        }
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("stride", &self.stride)
            .field("data_len", &self.data.len())
            .finish()
    }
}

impl Frame {
    /// Builds a frame, checking that `stride` covers a full row and `data` covers every row.
    pub fn new(
        width: u32,
        height: u32,
        format: PixelFormat,
        stride: usize,
        data: Vec<u8>,
    ) -> Result<Self, CaptureError> {
        let row_bytes = width as usize * format.bytes_per_pixel();
        if width == 0 || height == 0 {
            return Err(CaptureError::InvalidFrame(format!(
                "empty frame {width}x{height}"
            )));
        }
        if stride < row_bytes {
            return Err(CaptureError::InvalidFrame(format!(
                "stride {stride} is smaller than a row of {row_bytes} bytes"
            )));
        }
        // The last row does not need trailing padding.
        let needed = stride * (height as usize - 1) + row_bytes;
        if data.len() < needed {
            return Err(CaptureError::InvalidFrame(format!(
                "buffer holds {} bytes, {needed} needed for {width}x{height} at stride {stride}",
                data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            format,
            stride,
            data,
        })
    }

    /// Returns the (r, g, b) value of the pixel at (`x`, `y`), or `None` when out of range.
    pub fn pixel_rgb(&self, x: u32, y: u32) -> Option<(u8, u8, u8)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let bpp = self.format.bytes_per_pixel();
        let at = y as usize * self.stride + x as usize * bpp;
        let (r, g, b) = self.format.rgb_offsets();
        let px = self.data.get(at..at + bpp)?;
        Some((px[r], px[g], px[b]))
    }

    /// Packs the frame into tightly packed 8-bit RGB, dropping stride padding and alpha.
    pub fn to_rgb8(&self) -> Vec<u8> {
        let (r, g, b) = self.format.rgb_offsets();
        let bpp = self.format.bytes_per_pixel();
        let mut out = Vec::with_capacity(self.width as usize * self.height as usize * 3);
        for y in 0..self.height as usize {
            let row = &self.data[y * self.stride..y * self.stride + self.width as usize * bpp];
            for px in row.chunks_exact(bpp) {
                out.extend_from_slice(&[px[r], px[g], px[b]]);
            }
        }
        out
    }

    /// Writes the frame to `path` as an 8-bit RGB PNG.
    pub fn write_png(&self, path: &Path) -> Result<(), CaptureError> {
        let file = std::fs::File::create(path)?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| CaptureError::Png(e.to_string()))?;
        writer
            .write_image_data(&self.to_rgb8())
            .map_err(|e| CaptureError::Png(e.to_string()))?;
        writer
            .finish()
            .map_err(|e| CaptureError::Png(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_validates_geometry() {
        assert!(Frame::new(2, 2, PixelFormat::Bgrx8888, 8, vec![0; 16]).is_ok());
        assert!(Frame::new(2, 2, PixelFormat::Bgrx8888, 4, vec![0; 16]).is_err());
        assert!(Frame::new(2, 2, PixelFormat::Bgrx8888, 8, vec![0; 15]).is_err());
        assert!(Frame::new(0, 2, PixelFormat::Bgrx8888, 8, vec![0; 16]).is_err());
    }

    #[test]
    fn pixel_access_honours_format_and_stride() {
        // 1x2 image, stride 8 (4 bytes padding per row).
        let data = vec![1, 2, 3, 0, 9, 9, 9, 9, 10, 20, 30, 0, 9, 9, 9, 9];
        let bgrx = Frame::new(1, 2, PixelFormat::Bgrx8888, 8, data.clone()).unwrap();
        assert_eq!(bgrx.pixel_rgb(0, 0), Some((3, 2, 1)));
        assert_eq!(bgrx.pixel_rgb(0, 1), Some((30, 20, 10)));
        assert_eq!(bgrx.pixel_rgb(1, 0), None);
        assert_eq!(bgrx.to_rgb8(), vec![3, 2, 1, 30, 20, 10]);
        let rgba = Frame::new(1, 2, PixelFormat::Rgba8888, 8, data).unwrap();
        assert_eq!(rgba.pixel_rgb(0, 0), Some((1, 2, 3)));
    }

    #[test]
    fn png_round_trips() {
        let frame = Frame::new(
            2,
            1,
            PixelFormat::Bgrx8888,
            8,
            vec![0, 0, 255, 0, 255, 0, 0, 0],
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.png");
        frame.write_png(&path).unwrap();
        let decoder = png::Decoder::new(std::fs::File::open(&path).unwrap());
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (2, 1));
        assert_eq!(&buf[..6], &[255, 0, 0, 0, 0, 255]);
    }

    /// The trait must stay usable as a boxed trait object.
    #[test]
    fn backend_trait_is_object_safe() {
        struct Fixed;
        impl CaptureBackend for Fixed {
            fn name(&self) -> &'static str {
                "fixed"
            }
            fn next_frame(&mut self) -> Result<Frame, CaptureError> {
                Frame::new(1, 1, PixelFormat::Rgba8888, 4, vec![1, 2, 3, 4])
            }
        }
        let mut b: Box<dyn CaptureBackend> = Box::new(Fixed);
        assert_eq!(b.name(), "fixed");
        assert_eq!(b.next_frame().unwrap().pixel_rgb(0, 0), Some((1, 2, 3)));
    }
}
