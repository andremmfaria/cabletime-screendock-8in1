//! The control socket of the real binary: `ctl status` and `ctl get` against a running daemon,
//! exit codes, socket permissions and cleanup at exit.
//!
//! Skips when Xvfb is unavailable (`SCREENDOCK_REQUIRE_XVFB=1` makes that a failure).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::daemon_xvfb::{config, make_nodes, spawn_daemon, wait_exit};
use crate::support::require_xvfb;

fn socket(runtime: &Path) -> PathBuf {
    runtime.join("screendockd").join("control.sock")
}

fn ctl(runtime: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_screendockd"))
        .args(["--config", &config(), "ctl"])
        .args(args)
        .env("XDG_RUNTIME_DIR", runtime)
        .stdin(Stdio::null())
        .output()
        .expect("run screendockd ctl")
}

fn wait_for_socket(runtime: &Path) {
    let start = Instant::now();
    while !socket(runtime).exists() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "daemon never created its control socket"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn terminate(child: &mut std::process::Child) {
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .unwrap();
    assert!(status.success());
    let exit = wait_exit(child, Duration::from_secs(10));
    assert!(exit.success(), "daemon exited with {exit}");
}

#[test]
fn ctl_reports_the_running_daemons_settings_and_the_socket_is_private() {
    let Some(xvfb) = require_xvfb() else { return };
    let dir = tempfile::tempdir().unwrap();
    make_nodes(dir.path());
    let mut child = spawn_daemon(&xvfb.display, dir.path(), &[]);
    wait_for_socket(dir.path());

    let mode = std::fs::metadata(socket(dir.path()))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);

    let status = ctl(dir.path(), &["status"]);
    assert_eq!(status.status.code(), Some(0));
    let text = String::from_utf8(status.stdout).unwrap();
    assert!(text.contains("bitrate: 4000\n"), "{text}");
    assert!(text.contains("resolution: 1280x720\n"), "{text}");
    assert!(text.contains("mode: mirror\n"), "{text}");
    assert!(
        text.contains(&format!("version: {}\n", env!("CARGO_PKG_VERSION"))),
        "{text}"
    );
    assert!(text.contains(&format!("pid: {}\n", child.id())), "{text}");

    let one = ctl(dir.path(), &["get", "keyframe-interval"]);
    assert_eq!(one.status.code(), Some(0));
    assert_eq!(String::from_utf8(one.stdout).unwrap(), "50\n");

    let all = ctl(dir.path(), &["get"]);
    assert_eq!(all.status.code(), Some(0));
    let all = String::from_utf8(all.stdout).unwrap();
    assert!(all.contains("preset: ultrafast\n"), "{all}");
    assert!(!all.contains("pid:"), "{all}");

    let bad = ctl(dir.path(), &["get", "volume"]);
    assert_eq!(bad.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("unknown setting"));

    terminate(&mut child);
    assert!(
        !socket(dir.path()).exists(),
        "the socket file must go away at exit"
    );
}

#[test]
fn ctl_without_a_daemon_exits_with_code_3() {
    let dir = tempfile::tempdir().unwrap();
    let output = ctl(dir.path(), &["status"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no daemon"));
}

#[test]
fn disabled_control_socket_is_not_created() {
    let Some(xvfb) = require_xvfb() else { return };
    let dir = tempfile::tempdir().unwrap();
    make_nodes(dir.path());
    let config_text = std::fs::read_to_string(config())
        .unwrap()
        .replace("enabled = true", "enabled = false");
    let config_path = dir.path().join("cfg.toml");
    std::fs::write(&config_path, config_text).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_screendockd"))
        .arg("--config")
        .arg(&config_path)
        .args(["run", "--backend", "x11", "--h264-file"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.h264"))
        .arg("--mi00")
        .arg(dir.path().join("mi00"))
        .arg("--mi01")
        .arg(dir.path().join("mi01"))
        .args(["--no-handshake", "--max-frames", "5"])
        .env("DISPLAY", &xvfb.display)
        .env_remove("WAYLAND_DISPLAY")
        .env("XDG_RUNTIME_DIR", dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let status = wait_exit(&mut child, Duration::from_secs(30));
    assert!(status.success());
    assert!(!socket(dir.path()).exists());
}
