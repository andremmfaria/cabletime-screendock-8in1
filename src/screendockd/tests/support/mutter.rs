//! A headless GNOME Shell for tests that need Mutter's ScreenCast service: its own session
//! bus, a private runtime directory with its own PipeWire and WirePlumber, and a software
//! renderer (no GPU).
//!
//! Tests skip when gnome-shell, pipewire, wireplumber or dbus-daemon are missing or the shell
//! cannot start; `SCREENDOCK_REQUIRE_MUTTER=1` turns the skip into a failure. Every process
//! the helper starts belongs to one process group that is signalled when the helper drops.
#![allow(dead_code)]

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::MutexGuard;
use std::time::{Duration, Instant};

use screendockd::adapters::display::probe_screencast;

pub struct MutterHeadless {
    children: Vec<Child>,
    group: i32,
    pub runtime: tempfile::TempDir,
    pub bus_address: String,
    pub pipewire_remote: String,
    pub log_dir: PathBuf,
    serial: Option<MutexGuard<'static, ()>>,
}

impl Drop for MutterHeadless {
    fn drop(&mut self) {
        signal_group(self.group, libc::SIGTERM);
        let deadline = Instant::now() + Duration::from_secs(5);
        for child in &mut self.children {
            while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        signal_group(self.group, libc::SIGKILL);
        for child in &mut self.children {
            let _ = child.wait();
        }
        self.serial.take();
    }
}

fn signal_group(group: i32, signal: i32) {
    // SAFETY: kill(2) with a negative pid signals the process group; the group is the one the
    // helper created for its own children.
    unsafe {
        libc::kill(-group, signal);
    }
}

fn spawn(
    program: &str,
    args: &[&str],
    group: i32,
    runtime: &Path,
    bus: Option<&str>,
    log: &Path,
) -> Option<Child> {
    let log_file = std::fs::File::create(log).ok()?;
    let mut command = Command::new(program);
    command
        .args(args)
        .env("XDG_RUNTIME_DIR", runtime)
        .env("XDG_CONFIG_HOME", runtime.join("config"))
        .env("XDG_CACHE_HOME", runtime.join("cache"))
        .env("XDG_STATE_HOME", runtime.join("state"))
        .env("XDG_SESSION_TYPE", "wayland")
        .env("XDG_CURRENT_DESKTOP", "GNOME")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file.try_clone().ok()?))
        .stderr(Stdio::from(log_file))
        .process_group(group);
    if let Some(bus) = bus {
        command.env("DBUS_SESSION_BUS_ADDRESS", bus);
    }
    match command.spawn() {
        Ok(child) => Some(child),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => panic!("failed to start {program}: {e}"),
    }
}

fn start_mutter() -> Option<MutterHeadless> {
    // The runtime directory is short and private: a unix socket path has a length limit.
    let runtime = tempfile::Builder::new()
        .prefix("sd-gnome-")
        .tempdir_in("/tmp")
        .ok()?;
    std::fs::set_permissions(
        runtime.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .ok()?;
    let log_dir = runtime.path().join("logs");
    std::fs::create_dir_all(&log_dir).ok()?;
    let bus_path = runtime.path().join("bus");
    let bus_address = format!("unix:path={}", bus_path.display());

    let mut helper = MutterHeadless {
        children: Vec::new(),
        group: 0,
        pipewire_remote: runtime.path().join("pipewire-0").display().to_string(),
        runtime,
        bus_address: bus_address.clone(),
        log_dir: log_dir.clone(),
        serial: Some(super::COMPOSITOR.lock().unwrap_or_else(|e| e.into_inner())),
    };
    let rt = helper.runtime.path().to_path_buf();

    let bus = spawn(
        "dbus-daemon",
        &[
            "--session",
            "--nofork",
            "--nopidfile",
            &format!("--address={bus_address}"),
        ],
        0,
        &rt,
        None,
        &log_dir.join("dbus.log"),
    )?;
    helper.group = i32::try_from(bus.id()).ok()?;
    helper.children.push(bus);
    let group = helper.group;

    helper.children.push(spawn(
        "pipewire",
        &[],
        group,
        &rt,
        Some(&bus_address),
        &log_dir.join("pipewire.log"),
    )?);
    wait_for(Duration::from_secs(10), || rt.join("pipewire-0").exists())?;
    helper.children.push(spawn(
        "wireplumber",
        &[],
        group,
        &rt,
        Some(&bus_address),
        &log_dir.join("wireplumber.log"),
    )?);
    std::thread::sleep(Duration::from_secs(1));
    helper.children.push(spawn(
        "gnome-shell",
        &[
            "--headless",
            "--wayland",
            "--no-x11",
            "--virtual-monitor",
            "1280x720",
        ],
        group,
        &rt,
        Some(&bus_address),
        &log_dir.join("gnome-shell.log"),
    )?);
    let ready = wait_for(Duration::from_secs(40), || {
        probe_screencast(Some(&bus_address), Duration::from_secs(1))
    });
    if ready.is_none() {
        eprintln!(
            "gnome-shell never offered ScreenCast, see {}",
            log_dir.display()
        );
        return None;
    }
    // The shell takes a moment after it owns the name before it accepts a session.
    std::thread::sleep(Duration::from_secs(2));
    Some(helper)
}

fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> Option<()> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if done() {
            return Some(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

/// Starts headless GNOME Shell, or returns `None` after printing why a test should skip.
pub fn require_mutter() -> Option<MutterHeadless> {
    let mutter = start_mutter();
    if mutter.is_none() {
        if std::env::var_os("SCREENDOCK_REQUIRE_MUTTER").is_some() {
            panic!("headless GNOME Shell is unavailable but SCREENDOCK_REQUIRE_MUTTER is set");
        }
        eprintln!("skipping: gnome-shell or pipewire is missing or cannot run headless here");
    }
    mutter
}

impl MutterHeadless {
    /// The ids of the PipeWire nodes the private PipeWire holds, from `pw-dump`. `None` when
    /// `pw-dump` is missing.
    pub fn pipewire_node_ids(&self) -> Option<Vec<u64>> {
        let out = Command::new("pw-dump")
            .env("XDG_RUNTIME_DIR", self.runtime.path())
            .env("PIPEWIRE_RUNTIME_DIR", self.runtime.path())
            .output()
            .ok()?;
        let nodes: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
        Some(
            nodes
                .as_array()?
                .iter()
                .filter(|o| o["type"] == "PipeWire:Interface:Node")
                .filter_map(|o| o["id"].as_u64())
                .collect(),
        )
    }

    /// Raw BGRx pixels of the node as `gst-launch-1.0` reads them, independent of the code
    /// under test. `None` when GStreamer's PipeWire source is not installed or yields nothing.
    pub fn gst_frame(&self, node: u32, width: u32, height: u32) -> Option<Vec<u8>> {
        let out = self.runtime.path().join("gst-frame.raw");
        let caps = format!("video/x-raw,format=BGRx,width={width},height={height}");
        let status = Command::new("timeout")
            .args(["40", "gst-launch-1.0", "-q", "pipewiresrc"])
            .arg(format!("path={node}"))
            .args(["num-buffers=1", "!", &caps, "!", "filesink"])
            .arg(format!("location={}", out.display()))
            .env("XDG_RUNTIME_DIR", self.runtime.path())
            .env("PIPEWIRE_RUNTIME_DIR", self.runtime.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        status.success().then(|| std::fs::read(&out).ok()).flatten()
    }
}
