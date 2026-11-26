//! Shared Xvfb and X11 painting helpers for the integration tests.
#![allow(dead_code)]

mod xorg;
#[allow(unused_imports)]
pub use xorg::{require_xorg, xrandr, XorgDummy};

use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, CreateGCAux, Gcontext, Rectangle, Screen, Window};
use x11rb::rust_connection::RustConnection;

pub const WIDTH: u16 = 320;
pub const HEIGHT: u16 = 240;

pub struct Xvfb {
    child: Child,
    pub display: String,
    /// X servers reset when their last client disconnects; this connection stays open so the
    /// display survives between the readiness probe and the test's own connections.
    _keepalive: RustConnection,
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        remove_display_files(&self.display);
    }
}

/// Next display number to try; shared so tests running in parallel never pick the same one.
static NEXT_DISPLAY: AtomicU32 = AtomicU32::new(0);

/// A display number in 100..500 that no other helper of this process uses. A number held by a
/// foreign server makes that server's start fail, and the callers then try the next one.
pub(crate) fn next_display_number() -> u32 {
    100 + (std::process::id() + NEXT_DISPLAY.fetch_add(1, Ordering::SeqCst)) % 400
}

/// Removes the socket and lock file a killed X server leaves behind.
pub(crate) fn remove_display_files(display: &str) {
    let number = display.trim_start_matches(':');
    let _ = std::fs::remove_file(format!("/tmp/.X11-unix/X{number}"));
    let _ = std::fs::remove_file(format!("/tmp/.X{number}-lock"));
}

/// Starts Xvfb on a free display number and waits until it accepts connections. `None` means
/// Xvfb is not installed or unusable. Display numbers are probed by hand because `-displayfd` fails on hosts
/// where `/tmp/.X11-unix` is not writable. XShm needs fd passing, so the test must use the unix
/// socket; a host with an unusable `/tmp/.X11-unix` has to provide a writable one (for example
/// a user and mount namespace with a tmpfs mounted there).
fn start_xvfb() -> Option<Xvfb> {
    for _ in 0..20 {
        let number = next_display_number();
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

pub fn require_xvfb() -> Option<Xvfb> {
    let xvfb = start_xvfb();
    if xvfb.is_none() {
        if std::env::var_os("SCREENDOCK_REQUIRE_XVFB").is_some() {
            panic!("Xvfb unavailable but SCREENDOCK_REQUIRE_XVFB is set");
        }
        eprintln!("skipping: Xvfb is not installed or unusable");
    }
    xvfb
}

pub struct Painter {
    conn: RustConnection,
    root: Window,
    gc: Gcontext,
}

impl Painter {
    pub fn connect(display: &str) -> Self {
        let (conn, num) = x11rb::connect(Some(display)).expect("connect painter");
        let screen: &Screen = &conn.setup().roots[num];
        let root = screen.root;
        let gc = conn.generate_id().expect("gc id");
        conn.create_gc(gc, root, &CreateGCAux::new())
            .expect("create gc");
        Self { conn, root, gc }
    }

    pub fn fill(&self, color: u32, x: i16, y: i16, w: u16, h: u16) {
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
    pub fn sync(&self) {
        self.conn.get_input_focus().unwrap().reply().unwrap();
    }
}
