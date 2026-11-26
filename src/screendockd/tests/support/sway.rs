//! A headless sway for tests that need a wlroots compositor: no GPU, no input devices, a
//! private runtime directory and its own IPC and Wayland sockets.
//!
//! Tests skip when sway is missing or cannot start; `SCREENDOCK_REQUIRE_SWAY=1` turns the skip
//! into a failure. `grim` reads pixels back through the screencopy protocol, independent of
//! the code under test.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use screendockd::adapters::display::SwayIpc;

pub struct SwayHeadless {
    child: Child,
    _serial: std::sync::MutexGuard<'static, ()>,
    pub runtime: tempfile::TempDir,
    pub ipc_socket: PathBuf,
    pub wayland_display: String,
    pub log: PathBuf,
}

impl Drop for SwayHeadless {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn find_in(dir: &Path, prefix: &str, suffix: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with(prefix) && n.ends_with(suffix) && !n.ends_with(".lock")
            })
        })
}

fn start_sway() -> Option<SwayHeadless> {
    let serial = super::COMPOSITOR.lock().unwrap_or_else(|e| e.into_inner());
    // The runtime directory is short and private: a unix socket path has a length limit.
    let runtime = tempfile::Builder::new()
        .prefix("sd-sway-")
        .tempdir_in("/tmp")
        .ok()?;
    let log = runtime.path().join("sway.log");
    let log_file = std::fs::File::create(&log).ok()?;
    let mut child = match Command::new("sway")
        .args(["-c", "/dev/null"])
        .env("XDG_RUNTIME_DIR", runtime.path())
        .env("WLR_BACKENDS", "headless")
        .env("WLR_LIBINPUT_NO_DEVICES", "1")
        .env("WLR_RENDERER", "pixman")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("SWAYSOCK")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log_file)
        .spawn()
    {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => panic!("failed to start sway: {e}"),
    };
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(15) {
        if child.try_wait().ok()?.is_some() {
            eprintln!("sway exited early, see {}", log.display());
            return None;
        }
        let ipc = find_in(runtime.path(), "sway-ipc.", ".sock");
        let wayland = find_in(runtime.path(), "wayland-", "").filter(|p| p.extension().is_none());
        if let (Some(ipc_socket), Some(wayland)) = (ipc, wayland) {
            if let Ok(mut ipc) = SwayIpc::connect(&ipc_socket) {
                if ipc.outputs().is_ok_and(|o| !o.is_empty()) {
                    return Some(SwayHeadless {
                        child,
                        _serial: serial,
                        wayland_display: wayland
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("wayland-1")
                            .to_string(),
                        ipc_socket,
                        runtime,
                        log,
                    });
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    eprintln!(
        "sway never answered on its IPC socket, see {}",
        log.display()
    );
    None
}

/// Starts headless sway, or returns `None` after printing why a test should skip.
pub fn require_sway() -> Option<SwayHeadless> {
    let sway = start_sway();
    if sway.is_none() {
        if std::env::var_os("SCREENDOCK_REQUIRE_SWAY").is_some() {
            panic!("headless sway is unavailable but SCREENDOCK_REQUIRE_SWAY is set");
        }
        eprintln!("skipping: sway is not installed or cannot run headless on this host");
    }
    sway
}

impl SwayHeadless {
    pub fn ipc(&self) -> SwayIpc {
        SwayIpc::connect(&self.ipc_socket).expect("connect to sway")
    }

    pub fn output_names(&self) -> Vec<String> {
        self.ipc()
            .outputs()
            .expect("list outputs")
            .into_iter()
            .map(|o| o.name)
            .collect()
    }

    /// The decoded pixels of `output` as seen by `grim`: width, height and RGBA bytes.
    /// Returns `None` after printing a skip notice when `grim` is not installed.
    pub fn grim(&self, output: &str) -> Option<(u32, u32, Vec<u8>)> {
        let result = Command::new("grim")
            .args(["-o", output, "-"])
            .env("XDG_RUNTIME_DIR", self.runtime.path())
            .env("WAYLAND_DISPLAY", &self.wayland_display)
            .output();
        let out = match result {
            Ok(out) => out,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if std::env::var_os("SCREENDOCK_REQUIRE_SWAY").is_some() {
                    panic!("grim is missing but SCREENDOCK_REQUIRE_SWAY is set");
                }
                eprintln!("skipping the pixel check: grim is not installed");
                return None;
            }
            Err(e) => panic!("failed to run grim: {e}"),
        };
        assert!(
            out.status.success(),
            "grim failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let mut decoder = png::Decoder::new(std::io::Cursor::new(out.stdout));
        decoder.set_transformations(png::Transformations::EXPAND);
        let mut reader = decoder.read_info().expect("grim output is a png");
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).expect("decode png");
        let channels = info.color_type.samples();
        assert!(channels == 3 || channels == 4, "unexpected png layout");
        let rgba: Vec<u8> = buf[..info.buffer_size()]
            .chunks(channels)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect();
        Some((info.width, info.height, rgba))
    }
}
