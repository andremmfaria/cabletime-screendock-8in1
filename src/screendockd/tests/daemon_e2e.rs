//! End to end daemon test: the `run` subcommand against a real Xvfb display, the pre-recorded
//! test encoder, and regular files standing in for the two hidraw nodes.
//!
//! Skips when Xvfb is unavailable (`SCREENDOCK_REQUIRE_XVFB=1` makes that a failure).

mod common;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::{require_xvfb, Painter, HEIGHT, WIDTH};
use screendockd::annexb::messages_from_annexb;
use screendockd::framing::{
    FieldB, ParsedMessage, ReportId, CONTINUATION_CHUNK_PAYLOAD_MAX, FIRST_CHUNK_PAYLOAD_MAX,
    REPORT_SIZE,
};
use screendockd::hidraw_io::HidrawHandle;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.h264")
}

fn config() -> String {
    format!(
        "{}/../../config/screendock_cfg.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Creates empty regular files standing in for the hidraw nodes (a real node always exists).
fn make_nodes(dir: &Path) {
    for name in ["mi00", "mi01"] {
        std::fs::write(dir.join(name), b"").unwrap();
    }
}

fn spawn_daemon(display: &str, dir: &Path, extra: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_screendockd"))
        .args([
            "--config",
            &config(),
            "run",
            "--backend",
            "x11",
            "--h264-file",
        ])
        .arg(fixture())
        .arg("--mi00")
        .arg(dir.join("mi00"))
        .arg("--mi01")
        .arg(dir.join("mi01"))
        .args(extra)
        .env("DISPLAY", display)
        .env_remove("WAYLAND_DISPLAY")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn screendockd run")
}

fn wait_exit(child: &mut Child, limit: Duration) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            return status;
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            panic!("daemon did not exit within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn read_all(path: &Path, channel: ReportId) -> Vec<ParsedMessage> {
    let len = std::fs::metadata(path).unwrap().len() as usize;
    // A stopped daemon never leaves a torn message: whole reports are written one by one, but
    // be strict about report alignment so a partial write is caught.
    assert_eq!(len % REPORT_SIZE, 0, "{path:?} holds a partial report");
    let mut handle = HidrawHandle::from_file(File::open(path).unwrap(), channel);
    let mut out = Vec::new();
    let mut consumed = 0;
    while consumed < len {
        let msg = handle.read_message().expect("parse message");
        let extra = msg.payload.len().saturating_sub(FIRST_CHUNK_PAYLOAD_MAX);
        consumed += REPORT_SIZE * (1 + extra.div_ceil(CONTINUATION_CHUNK_PAYLOAD_MAX));
        out.push(msg);
    }
    out
}

#[test]
fn run_writes_expected_video_and_heartbeats_then_exits() {
    let Some(xvfb) = require_xvfb() else { return };
    let painter = Painter::connect(&xvfb.display);
    painter.fill(0x0000_00ff, 0, 0, WIDTH, HEIGHT);
    painter.sync();

    let dir = tempfile::tempdir().unwrap();
    make_nodes(dir.path());
    let mut child = spawn_daemon(&xvfb.display, dir.path(), &["--max-frames", "12"]);
    let status = wait_exit(&mut child, Duration::from_secs(30));
    assert!(status.success(), "daemon exited with {status}");

    let expected = messages_from_annexb(&std::fs::read(fixture()).unwrap()).unwrap();
    let video = read_all(&dir.path().join("mi01"), ReportId::Video);
    assert_eq!(video.len(), 12);
    for (i, msg) in video.iter().enumerate() {
        assert_eq!(msg.field_b, FieldB(327_680));
        assert_eq!(msg.payload, expected[i % expected.len()], "message {i}");
    }

    let beats = read_all(&dir.path().join("mi00"), ReportId::Control);
    assert!(!beats.is_empty());
    let first: serde_json::Value = serde_json::from_slice(&beats[0].payload).unwrap();
    assert_eq!(first["method"], "heartbeat");
    assert_eq!(first["params"]["session-id"], "1");
}

#[test]
fn sigterm_and_sigint_stop_the_daemon_cleanly() {
    let Some(xvfb) = require_xvfb() else { return };
    for signal in ["TERM", "INT"] {
        let dir = tempfile::tempdir().unwrap();
        make_nodes(dir.path());
        let mut child = spawn_daemon(&xvfb.display, dir.path(), &[]);
        let video = dir.path().join("mi01");
        let start = Instant::now();
        while std::fs::metadata(&video).map_or(0, |m| m.len()) == 0 {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "daemon never wrote video"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        let killed = Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(child.id().to_string())
            .status()
            .unwrap();
        assert!(killed.success());
        let status = wait_exit(&mut child, Duration::from_secs(10));
        assert!(status.success(), "SIG{signal}: daemon exited with {status}");
        // Whatever was written before the signal must still parse.
        assert!(!read_all(&video, ReportId::Video).is_empty());
    }
}

#[test]
fn startup_fails_fast_when_a_node_cannot_be_opened() {
    let Some(xvfb) = require_xvfb() else { return };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("mi01"), b"").unwrap();
    // mi00 is missing, so opening it must fail before any capture starts.
    let mut child = spawn_daemon(&xvfb.display, dir.path(), &["--max-frames", "1"]);
    let status = wait_exit(&mut child, Duration::from_secs(10));
    assert!(!status.success());
    assert_eq!(std::fs::metadata(dir.path().join("mi01")).unwrap().len(), 0);
}
