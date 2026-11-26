//! The daemon's virtual monitor on a rootless Xorg with the dummy driver: switching between
//! mirror and extend at run time, restoring the layout at exit, and `vdisplay cleanup` after a
//! kill.
//!
//! Skips when Xorg with the dummy driver is unavailable (`SCREENDOCK_REQUIRE_XORG=1` makes that
//! a failure).

use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::daemon_xvfb::{config, make_nodes, spawn_daemon, wait_exit};
use crate::support::{require_xorg, xrandr};

fn runtime_command(display: &str, runtime: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_screendockd"))
        .args(["--config", &config()])
        .args(args)
        .env("DISPLAY", display)
        .env("XDG_RUNTIME_DIR", runtime)
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null())
        .output()
        .expect("run screendockd")
}

fn monitors(display: &str) -> usize {
    xrandr(display, &["--listmonitors"])
        .lines()
        .filter(|l| l.contains('/'))
        .count()
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_daemon(display: &str, runtime: &Path) {
    wait_for("the control socket answers", || {
        runtime_command(display, runtime, &["ctl", "status"])
            .status
            .success()
    });
}

fn set_mode(display: &str, runtime: &Path, mode: &str) {
    let out = runtime_command(display, runtime, &["ctl", "set", "mode", mode]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn signal(child: &Child, name: &str) {
    assert!(Command::new("kill")
        .arg(name)
        .arg(child.id().to_string())
        .status()
        .unwrap()
        .success());
}

#[test]
fn ctl_switches_between_mirror_and_extend_and_exit_restores_the_layout() {
    let Some(xorg) = require_xorg() else { return };
    let dir = tempfile::tempdir().unwrap();
    make_nodes(dir.path());
    let before = xrandr(&xorg.display, &["--listmonitors"]);
    let mut child = spawn_daemon(&xorg.display, dir.path(), &[]);
    wait_for_daemon(&xorg.display, dir.path());
    assert_eq!(monitors(&xorg.display), 1);

    set_mode(&xorg.display, dir.path(), "extend");
    assert_eq!(monitors(&xorg.display), 2);
    let listed = xrandr(&xorg.display, &["--listmonitors"]);
    assert!(
        listed.contains("1280/") && listed.contains("+1280+0"),
        "{listed}"
    );
    assert!(dir.path().join("screendockd/vdisplay-state.json").exists());

    set_mode(&xorg.display, dir.path(), "mirror");
    assert_eq!(monitors(&xorg.display), 1);
    assert!(!dir.path().join("screendockd/vdisplay-state.json").exists());

    set_mode(&xorg.display, dir.path(), "extend");
    assert_eq!(monitors(&xorg.display), 2);
    signal(&child, "-TERM");
    assert!(wait_exit(&mut child, Duration::from_secs(10)).success());
    assert_eq!(xrandr(&xorg.display, &["--listmonitors"]), before);
    assert!(xrandr(&xorg.display, &[]).contains("current 1280 x 720"));
}

#[test]
fn a_monitor_left_by_a_killed_daemon_is_removed_by_vdisplay_cleanup() {
    let Some(xorg) = require_xorg() else { return };
    let dir = tempfile::tempdir().unwrap();
    make_nodes(dir.path());
    let before = xrandr(&xorg.display, &["--listmonitors"]);
    let mut child = spawn_daemon(&xorg.display, dir.path(), &[]);
    wait_for_daemon(&xorg.display, dir.path());
    set_mode(&xorg.display, dir.path(), "extend");
    assert_eq!(monitors(&xorg.display), 2);

    signal(&child, "-KILL");
    let _ = child.wait();
    assert_eq!(
        monitors(&xorg.display),
        2,
        "a kill leaves the monitor behind"
    );

    let out = runtime_command(&xorg.display, dir.path(), &["vdisplay", "cleanup"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("restored"));
    assert_eq!(xrandr(&xorg.display, &["--listmonitors"]), before);
    assert!(xrandr(&xorg.display, &[]).contains("current 1280 x 720"));
    let again = runtime_command(&xorg.display, dir.path(), &["vdisplay", "cleanup"]);
    assert!(String::from_utf8_lossy(&again.stdout).contains("nothing to clean up"));
}

#[test]
fn a_new_daemon_undoes_the_monitor_a_killed_one_left_behind() {
    let Some(xorg) = require_xorg() else { return };
    let dir = tempfile::tempdir().unwrap();
    make_nodes(dir.path());
    let before = xrandr(&xorg.display, &["--listmonitors"]);
    let mut first = spawn_daemon(&xorg.display, dir.path(), &[]);
    wait_for_daemon(&xorg.display, dir.path());
    set_mode(&xorg.display, dir.path(), "extend");
    signal(&first, "-KILL");
    let _ = first.wait();
    let _ = std::fs::remove_file(dir.path().join("screendockd/control.sock"));

    // The next daemon undoes what the killed one left before it makes its own monitor.
    let mut second = spawn_daemon(&xorg.display, dir.path(), &[]);
    wait_for_daemon(&xorg.display, dir.path());
    set_mode(&xorg.display, dir.path(), "extend");
    assert_eq!(monitors(&xorg.display), 2);
    signal(&second, "-TERM");
    assert!(wait_exit(&mut second, Duration::from_secs(10)).success());
    assert_eq!(xrandr(&xorg.display, &["--listmonitors"]), before);
}

#[test]
fn vdisplay_probe_prints_the_outputs_and_the_extend_mechanism() {
    let Some(xorg) = require_xorg() else { return };
    let dir = tempfile::tempdir().unwrap();
    let out = runtime_command(&xorg.display, dir.path(), &["vdisplay", "probe"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("server: X11"), "{text}");
    assert!(
        text.contains("output: DUMMY0 1280x720+0+0 primary"),
        "{text}"
    );
    assert!(text.contains("extend: X11Randr(RealOutput("), "{text}");
    assert!(text.contains("monitor: 1280x720+1280+0"), "{text}");
}
