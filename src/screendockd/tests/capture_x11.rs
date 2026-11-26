//! Stage 1 capture test: a real Xvfb display with synthetic painted content, captured through the
//! XShm backend, with exact pixel assertions.
//!
//! Skips (passes with a note on stderr) when `Xvfb` is not on `PATH`. Set
//! `SCREENDOCK_REQUIRE_XVFB=1` to make a missing Xvfb a failure; CI does this.

mod common;

use std::process::Command;

use common::{require_xvfb, Painter, HEIGHT, WIDTH};
use screendockd::capture::x11_shm::X11ShmCapture;
use screendockd::capture::{CaptureBackend, PixelFormat};

#[test]
fn captures_painted_regions_with_exact_pixels() {
    let Some(xvfb) = require_xvfb() else { return };
    let painter = Painter::connect(&xvfb.display);
    painter.fill(0x00ff_0000, 0, 0, WIDTH, HEIGHT); // red background
    painter.fill(0x0000_00ff, 10, 20, 50, 30); // blue
    painter.fill(0x0000_ff00, 100, 100, 40, 40); // green
    painter.sync();

    let mut capture = X11ShmCapture::connect(Some(&xvfb.display), 0).expect("connect capture");
    let frame = capture.next_frame().expect("frame");

    assert_eq!(
        (frame.width, frame.height),
        (u32::from(WIDTH), u32::from(HEIGHT))
    );
    assert_eq!(frame.format, PixelFormat::Bgrx8888);
    assert_eq!(frame.stride, usize::from(WIDTH) * 4);
    assert_eq!(frame.pixel_rgb(0, 0), Some((255, 0, 0)));
    assert_eq!(frame.pixel_rgb(319, 239), Some((255, 0, 0)));
    assert_eq!(frame.pixel_rgb(10, 20), Some((0, 0, 255)));
    assert_eq!(frame.pixel_rgb(59, 49), Some((0, 0, 255)));
    assert_eq!(frame.pixel_rgb(60, 50), Some((255, 0, 0)));
    assert_eq!(frame.pixel_rgb(100, 100), Some((0, 255, 0)));
    assert_eq!(frame.pixel_rgb(139, 139), Some((0, 255, 0)));
    assert_eq!(frame.pixel_rgb(140, 140), Some((255, 0, 0)));
}

#[test]
fn later_frames_reflect_repainting() {
    let Some(xvfb) = require_xvfb() else { return };
    let painter = Painter::connect(&xvfb.display);
    painter.fill(0x0000_0000, 0, 0, WIDTH, HEIGHT);
    painter.sync();

    let mut capture = X11ShmCapture::connect(Some(&xvfb.display), 0).expect("connect capture");
    let before = capture.next_frame().expect("first frame");
    assert_eq!(before.pixel_rgb(5, 5), Some((0, 0, 0)));

    painter.fill(0x00ff_ffff, 0, 0, 20, 20);
    painter.sync();
    let after = capture.next_frame().expect("second frame");
    assert_eq!(after.pixel_rgb(5, 5), Some((255, 255, 255)));
    assert_eq!(after.pixel_rgb(25, 25), Some((0, 0, 0)));
}

#[test]
fn works_as_boxed_backend_and_paces_frames() {
    let Some(xvfb) = require_xvfb() else { return };
    let mut backend: Box<dyn CaptureBackend> =
        Box::new(X11ShmCapture::connect(Some(&xvfb.display), 20).expect("connect capture"));
    assert_eq!(backend.name(), "x11-shm");
    let start = std::time::Instant::now();
    for _ in 0..4 {
        backend.next_frame().expect("frame");
    }
    // Four frames at 20 fps: the first is immediate, the next three each wait about 50 ms.
    assert!(start.elapsed() >= std::time::Duration::from_millis(140));
}

#[test]
fn capture_subcommand_writes_png_files() {
    let Some(xvfb) = require_xvfb() else { return };
    let painter = Painter::connect(&xvfb.display);
    painter.fill(0x0000_00ff, 0, 0, WIDTH, HEIGHT);
    painter.sync();

    let out = tempfile::tempdir().unwrap();
    let config = format!(
        "{}/../../config/screendock_cfg.toml",
        env!("CARGO_MANIFEST_DIR")
    );
    let status = Command::new(env!("CARGO_BIN_EXE_screendockd"))
        .args([
            "--config",
            &config,
            "capture",
            "--frames",
            "2",
            "--backend",
            "x11",
            "--out-dir",
        ])
        .arg(out.path())
        .env("DISPLAY", &xvfb.display)
        .env_remove("WAYLAND_DISPLAY")
        .status()
        .expect("run screendockd capture");
    assert!(status.success());

    for name in ["frame-0000.png", "frame-0001.png"] {
        let decoder = png::Decoder::new(std::fs::File::open(out.path().join(name)).unwrap());
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!(
            (info.width, info.height),
            (u32::from(WIDTH), u32::from(HEIGHT))
        );
        assert_eq!(&buf[..3], &[0, 0, 255], "{name} first pixel should be blue");
    }
}
