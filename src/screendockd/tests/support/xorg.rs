//! A rootless Xorg server with the dummy video driver, for tests that need RandR monitors,
//! a framebuffer that grows and live mode changes (Xvfb cannot do any of these).
//!
//! The server runs as the current user with its own config and log in a temporary directory.
//! Tests skip when Xorg or the dummy driver is missing; `SCREENDOCK_REQUIRE_XORG=1` turns the
//! skip into a failure.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use x11rb::rust_connection::RustConnection;

use super::next_display_number;

/// Size of the screen the server is started with.
pub const START_WIDTH: u32 = 1280;
pub const START_HEIGHT: u32 = 720;
/// Largest framebuffer the config declares. RandR accepts a larger one at runtime.
pub const MAX_WIDTH: u32 = 4096;
pub const MAX_HEIGHT: u32 = 2160;

pub struct XorgDummy {
    child: Child,
    pub display: String,
    pub log: PathBuf,
    _dir: tempfile::TempDir,
    _keepalive: RustConnection,
}

impl XorgDummy {
    /// The server's process id, for tests that kill it.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for XorgDummy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        super::remove_display_files(&self.display);
    }
}

fn config() -> String {
    format!(
        r#"Section "Device"
  Identifier "dev0"
  Driver "dummy"
  VideoRam 256000
EndSection
Section "Monitor"
  Identifier "mon0"
  HorizSync 5-1000
  VertRefresh 5-200
  Modeline "{START_WIDTH}x{START_HEIGHT}_60" 74.25 1280 1390 1430 1650 720 725 730 750 +hsync +vsync
EndSection
Section "Screen"
  Identifier "scr0"
  Device "dev0"
  Monitor "mon0"
  DefaultDepth 24
  SubSection "Display"
    Depth 24
    Virtual {MAX_WIDTH} {MAX_HEIGHT}
    Modes "{START_WIDTH}x{START_HEIGHT}_60"
  EndSubSection
EndSection
Section "ServerFlags"
  Option "AutoAddDevices" "false"
  Option "AutoEnableDevices" "false"
  Option "AutoAddGPU" "false"
EndSection
Section "ServerLayout"
  Identifier "lay0"
  Screen "scr0"
EndSection
"#
    )
}

fn xorg_binary() -> &'static str {
    // The real server, not the wrapper: the wrapper refuses a custom config for non-root users.
    if Path::new("/usr/lib/xorg/Xorg").exists() {
        "/usr/lib/xorg/Xorg"
    } else {
        "Xorg"
    }
}

fn start_xorg() -> Option<XorgDummy> {
    let dir = tempfile::tempdir().expect("temp dir");
    let conf = dir.path().join("dummy.conf");
    let empty = dir.path().join("none");
    std::fs::create_dir(&empty).expect("empty config dir");
    std::fs::write(&conf, config()).expect("write config");
    let log = dir.path().join("xorg.log");
    for _ in 0..20 {
        let number = next_display_number();
        let display = format!(":{number}");
        let spawned = Command::new(xorg_binary())
            .arg("-config")
            .arg(&conf)
            .arg("-configdir")
            .arg(&empty)
            .args(["-noreset", "-logfile"])
            .arg(&log)
            .args([
                "-sharevts",
                "-novtswitch",
                "-keeptty",
                "-ac",
                "-nolisten",
                "tcp",
            ])
            .arg(&display)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => panic!("failed to start Xorg: {e}"),
        };
        for _ in 0..100 {
            if child.try_wait().expect("poll Xorg").is_some() {
                break;
            }
            if let Ok((keepalive, _)) = x11rb::connect(Some(&display)) {
                return Some(XorgDummy {
                    child,
                    display,
                    log,
                    _dir: dir,
                    _keepalive: keepalive,
                });
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if child.try_wait().expect("poll Xorg").is_none() {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("Xorg started but never accepted a connection");
            return None;
        }
        let log_text = std::fs::read_to_string(&log).unwrap_or_default();
        if log_text.contains("dummy") && log_text.contains("(EE) Failed to load")
            || log_text.contains("no screens found")
        {
            eprintln!("Xorg could not use the dummy driver");
            return None;
        }
    }
    eprintln!("Xorg did not start on any display number tried");
    None
}

/// Starts the dummy server, or returns `None` after printing why a test should skip.
pub fn require_xorg() -> Option<XorgDummy> {
    let server = start_xorg();
    if server.is_none() {
        if std::env::var_os("SCREENDOCK_REQUIRE_XORG").is_some() {
            panic!("Xorg with the dummy driver is unavailable but SCREENDOCK_REQUIRE_XORG is set");
        }
        eprintln!(
            "skipping: Xorg with the dummy driver is not installed or cannot start rootless \
             (packages xserver-xorg-core and xserver-xorg-video-dummy)"
        );
    }
    server
}

/// Runs `xrandr` against `display` and returns its standard output.
pub fn xrandr(display: &str, args: &[&str]) -> String {
    let out = Command::new("xrandr")
        .args(["-d", display])
        .args(args)
        .output()
        .expect("run xrandr");
    assert!(
        out.status.success(),
        "xrandr {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}
