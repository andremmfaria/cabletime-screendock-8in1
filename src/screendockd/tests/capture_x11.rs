//! Stage 1 capture test: a real Xvfb display with synthetic painted content, captured through the
//! XShm backend, with exact pixel assertions.
//!
//! Skips (passes with a note on stderr) when `Xvfb` is not on `PATH`. Set
//! `SCREENDOCK_REQUIRE_XVFB=1` to make a missing Xvfb a failure; CI does this.

use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use screendockd::capture::x11_shm::X11ShmCapture;
use screendockd::capture::{CaptureBackend, PixelFormat};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, CreateGCAux, Gcontext, Rectangle, Screen, Window};
use x11rb::rust_connection::RustConnection;

const WIDTH: u16 = 320;
const HEIGHT: u16 = 240;

struct Xvfb {
    child: Child,
    display: String,
    /// X servers reset when their last client disconnects; this connection stays open so the
    /// display survives between the readiness probe and the test's own connections.
    _keepalive: RustConnection,
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Next display number to try; shared so tests running in parallel never pick the same one.
static NEXT_DISPLAY: AtomicU32 = AtomicU32::new(0);

/// Starts Xvfb on a free display number and waits until it accepts connections. `None` means
/// Xvfb is not installed or unusable. Display numbers are probed by hand because `-displayfd` fails on hosts
/// where `/tmp/.X11-unix` is not writable. XShm needs fd passing, so the test must use the unix
/// socket; a host with an unusable `/tmp/.X11-unix` has to provide a writable one (for example
/// a user and mount namespace with a tmpfs mounted there).
fn start_xvfb() -> Option<Xvfb> {
    let base = 100 + std::process::id() % 400;
    for _ in 0..20 {
        let number = base + NEXT_DISPLAY.fetch_add(1, Ordering::SeqCst);
        let display = format!(":{number}");
        let spawned = Command::new("Xvfb")
            .args([
                &format!(":{number}"),
                "-screen",
                "0",
                &format!("{WIDTH}x{HEIGHT}x24"),
                "-nolisten",
                "tcp",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => panic!("failed to start Xvfb: {e}"),
        };
        for _ in 0..60 {
            if child.try_wait().expect("poll Xvfb").is_some() {
                break; // display number taken, try the next one
            }
            if let Ok((keepalive, _)) = x11rb::connect(Some(&display)) {
                return Some(Xvfb {
                    child,
                    display,
                    _keepalive: keepalive,
                });
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // Alive but never connectable: the unix socket is unusable on this host.
        if child.try_wait().expect("poll Xvfb").is_none() {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("Xvfb started but its unix socket never accepted a connection");
            return None;
        }
    }
    eprintln!("no free display number for Xvfb");
    None
}

fn require_xvfb() -> Option<Xvfb> {
    let xvfb = start_xvfb();
    if xvfb.is_none() {
        if std::env::var_os("SCREENDOCK_REQUIRE_XVFB").is_some() {
            panic!("Xvfb unavailable but SCREENDOCK_REQUIRE_XVFB is set");
        }
        eprintln!("skipping: Xvfb is not installed or unusable");
    }
    xvfb
}

struct Painter {
    conn: RustConnection,
    root: Window,
    gc: Gcontext,
}

impl Painter {
    fn connect(display: &str) -> Self {
        let (conn, num) = x11rb::connect(Some(display)).expect("connect painter");
        let screen: &Screen = &conn.setup().roots[num];
        let root = screen.root;
        let gc = conn.generate_id().expect("gc id");
        conn.create_gc(gc, root, &CreateGCAux::new())
            .expect("create gc");
        Self { conn, root, gc }
    }

    fn fill(&self, color: u32, x: i16, y: i16, w: u16, h: u16) {
        self.conn
            .change_gc(
                self.gc,
                &x11rb::protocol::xproto::ChangeGCAux::new().foreground(color),
            )
            .expect("set color");
        self.conn
            .poly_fill_rectangle(
                self.root,
                self.gc,
                &[Rectangle {
                    x,
                    y,
                    width: w,
                    height: h,
                }],
            )
            .expect("fill");
    }

    /// Round trip so the server has processed every drawing request.
    fn sync(&self) {
        self.conn.get_input_focus().unwrap().reply().unwrap();
    }
}

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
