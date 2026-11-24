//! Daemon runs against a simulated dock: MI_00 and MI_01 are Unix datagram socket pairs, and a
//! fake dock thread speaks the session protocol on the far end (see `fake_dock.rs`). No hardware
//! and no display are involved.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use screendockd::adapters::encode::prerecorded::PrerecordedEncoder;
use screendockd::app::daemon::{run, DaemonParams, DaemonReport};
use screendockd::core::session::SessionSettings;
use screendockd::fake_dock::{FakeDock, FakeDockConfig};
use screendockd::ports::capture::{CaptureBackend, CaptureError, Frame, PixelFormat};
use screendockd::ports::encode::EncoderSettings;
use screendockd::protocol::display::{DisplayGeometry, Orientation};

struct Solid;

impl CaptureBackend for Solid {
    fn name(&self) -> &'static str {
        "solid"
    }
    fn next_frame(&mut self) -> Result<Frame, CaptureError> {
        std::thread::sleep(Duration::from_millis(5));
        Frame::new(2, 2, PixelFormat::Bgrx8888, 8, vec![0; 16])
    }
}

fn geometry() -> DisplayGeometry {
    DisplayGeometry::new(1280, 720, Orientation::Landscape).unwrap()
}

fn encoder() -> PrerecordedEncoder {
    let settings = EncoderSettings {
        geometry: geometry(),
        fps: 25,
        keyframe_interval: 3,
        bitrate_kbps: 1000,
        preset: screendockd::config::X264Preset::Ultrafast,
        zerolatency: true,
        vbv_buffer_ms: 200,
    };
    let big = vec![0xAB; 9000];
    let mut key = vec![0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65];
    key.extend(&big);
    let mut inter = vec![0, 0, 0, 1, 0x41];
    inter.extend(&big);
    PrerecordedEncoder::new(vec![key, inter], settings).unwrap()
}

fn params(max_frames: Option<u64>) -> DaemonParams {
    let mut session = SessionSettings::new(geometry().size(), Duration::from_millis(50));
    session.machine_name = "TESTHOST".into();
    session.ready_timeout = Duration::from_millis(400);
    session.reply_timeout = Duration::from_millis(400);
    session.settle_delay = Duration::from_millis(100);
    session.retry_delay = Duration::from_millis(50);
    DaemonParams {
        geometry: geometry(),
        session,
        max_frames,
        max_consecutive_errors: 50,
    }
}

fn non_heartbeat(dock: &FakeDock) -> Vec<String> {
    dock.host_texts()
        .into_iter()
        .filter(|t| !t.contains("\"heartbeat\""))
        .collect()
}

#[test]
fn fresh_dock_handshake_precedes_video_and_header_layout_is_exact() {
    let (dock, mi00, mi01) = FakeDock::spawn_with_video(FakeDockConfig::fresh());
    let stop = AtomicBool::new(false);
    let report = run(
        mi00,
        mi01,
        &mut Solid,
        &mut encoder(),
        &params(Some(8)),
        &stop,
    )
    .unwrap();
    assert_eq!(report.frames_sent, 8);
    assert_eq!(report.handshakes, 1);

    // Passthrough 103 goes out twice: the reader answers the dock's EDID, the handshake
    // sends its own. Their position relative to the other messages is not fixed.
    let all = non_heartbeat(&dock);
    assert_eq!(all.len(), 6, "{all:?}");
    let texts: Vec<String> = all
        .into_iter()
        .filter(|t| !t.contains("data_passthrough"))
        .collect();
    assert_eq!(texts.len(), 4, "{texts:?}");
    assert!(texts[1].contains("\"app-launched\"") && texts[1].contains("\"id\":1,"));
    assert!(texts[3].contains("\"notify-projection-started\"") && texts[3].contains("\"id\":2,"));

    // Give the fake a moment to drain the socket, then check what reached MI_01.
    let deadline = Instant::now() + Duration::from_secs(2);
    while dock.video_stats().messages < 8 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let video = dock.video_stats();
    assert_eq!(video.messages, 8);
    assert_eq!(
        video.reports_before_sink, 0,
        "video must not precede the sink"
    );
    assert_eq!(video.layout_violations, 0);
    assert_eq!(video.first_size, Some((1280, 720)));
    assert_eq!(dock.format_violations(), 0);
    assert!(dock.sink_started());
}

#[test]
fn dock_holding_a_session_gets_video_after_the_bounded_wait() {
    let (dock, mi00, mi01) = FakeDock::spawn_with_video(FakeDockConfig::holding_session());
    let stop = AtomicBool::new(false);
    let started = Instant::now();
    let report = run(
        mi00,
        mi01,
        &mut Solid,
        &mut encoder(),
        &params(Some(5)),
        &stop,
    )
    .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(350));
    assert_eq!(report.frames_sent, 5);
    assert_eq!(
        non_heartbeat(&dock).len(),
        4,
        "full sequence without stopped"
    );
    assert!(report.dock_heartbeats_seen > 0);
    let deadline = Instant::now() + Duration::from_secs(2);
    while dock.video_stats().messages < 5 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(dock.video_stats().messages, 5);
    assert_eq!(dock.video_stats().reports_before_sink, 0);
}

#[test]
fn fully_silent_dock_gets_video_in_assumed_active() {
    // No ready announcement, no heartbeat echo and no replies.
    let mut config = FakeDockConfig::holding_session();
    config.heartbeat_interval = None;
    let (dock, mi00, mi01) = FakeDock::spawn_with_video(config);
    let stop = AtomicBool::new(false);
    let report = run(
        mi00,
        mi01,
        &mut Solid,
        &mut encoder(),
        &params(Some(5)),
        &stop,
    )
    .unwrap();
    assert_eq!(report.frames_sent, 5);
    assert_eq!(report.dock_heartbeats_seen, 0);
    let deadline = Instant::now() + Duration::from_secs(2);
    while dock.video_stats().messages < 5 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(dock.video_stats().messages, 5);

    // The silent path still sends app-launched, cursor, passthrough 103 and
    // projection-started, in that order, all before the first video report.
    let texts = non_heartbeat(&dock);
    assert_eq!(texts.len(), 4, "{texts:?}");
    assert!(texts[0].contains("\"app-launched\"") && texts[0].contains("\"id\":1,"));
    assert!(texts[1].contains("app_tx_mouse_icon_change"));
    assert!(texts[2].contains("common.data_passthrough") && texts[2].contains("\"id\":103"));
    assert!(texts[3].contains("\"notify-projection-started\"") && texts[3].contains("\"id\":2,"));
    assert!(!texts
        .iter()
        .any(|t| t.contains("notify-projection-stopped")));
    let beats_and_control = dock.video_stats().control_messages_before_video.unwrap();
    assert!(beats_and_control >= 4);
}

#[test]
fn lost_session_pauses_video_and_is_redone_without_restarting() {
    let (dock, mi00, mi01) = FakeDock::spawn_with_video(FakeDockConfig::fresh());
    let stop = AtomicBool::new(false);
    let params = params(None);
    let report: DaemonReport = std::thread::scope(|scope| {
        let worker =
            scope.spawn(|| run(mi00, mi01, &mut Solid, &mut encoder(), &params, &stop).unwrap());
        let wait = |what: &str, cond: &dyn Fn() -> bool| {
            let start = Instant::now();
            while !cond() {
                assert!(
                    start.elapsed() < Duration::from_secs(10),
                    "timed out: {what}"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        wait("first video", &|| dock.video_stats().messages >= 3);
        dock.drop_session();
        wait("second handshake", &|| {
            dock.host_texts()
                .iter()
                .filter(|t| t.contains("\"app-launched\""))
                .count()
                == 2
        });
        let before = dock.video_stats().messages;
        wait("video resumes", &|| {
            dock.video_stats().messages >= before + 3
        });
        stop.store(true, Ordering::Relaxed);
        worker.join().unwrap()
    });
    assert_eq!(report.handshakes, 2);
    assert_eq!(report.session_losses, 1);
    assert_eq!(dock.video_stats().layout_violations, 0);
    assert_eq!(dock.format_violations(), 0);
}
