use std::time::{Duration, Instant};

use super::*;
use crate::adapters::transport::hidraw::HidrawHandle;
use crate::protocol::framing::{PictureSize, ReportId};
use crate::protocol::heartbeat::SessionId;
use crate::protocol::messages;
use crate::testing::fake_dock::{FakeDock, FakeDockConfig, StartedReply};

fn size() -> PictureSize {
    PictureSize::new(1280, 720)
}

fn fast_settings() -> SessionSettings {
    let mut s = SessionSettings::new(size(), Duration::from_millis(50));
    s.session_id = SessionId::new("25282281").unwrap();
    s.machine_name = "VERMELHO".into();
    s.ready_timeout = Duration::from_millis(400);
    s.reply_timeout = Duration::from_millis(400);
    s.settle_delay = Duration::from_millis(60);
    s.retry_delay = Duration::from_millis(50);
    s
}

fn wait_for(what: &str, limit: Duration, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < limit, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn texts(dock: &FakeDock) -> Vec<String> {
    dock.host_texts()
}

fn non_heartbeat(dock: &FakeDock) -> Vec<String> {
    texts(dock)
        .into_iter()
        .filter(|t| !t.contains("\"heartbeat\""))
        .collect()
}

/// What a silent dock must receive, in order: no projection-stopped, since the dock never
/// announced ready.
fn silent_sequence() -> Vec<String> {
    [
        messages::app_launched(1, "VERMELHO", size()),
        messages::MOUSE_ICON_CHANGE.to_vec(),
        messages::EDID_ACK.to_vec(),
        messages::projection_started(2),
    ]
    .into_iter()
    .map(|m| String::from_utf8(m).unwrap())
    .collect()
}

#[test]
fn silent_sequence_has_the_exact_vendor_bytes() {
    assert_eq!(
            silent_sequence(),
            [
                "{\"id\":1,\"jsonrpc\":\"2.0\",\"method\":\"app-launched\",\"params\":{\"platform\":\"windows\",\"platform-machine-name\":\"VERMELHO\",\"product-type\":\"QuattroTx CT10\",\"res-height\":720,\"res-width\":1280}}\n",
                "{\"jsonrpc\":\"2.0\",\"method\":\"app_tx_mouse_icon_change\",\"params\":{\"cursor_base64_data\":\"\"}}\n",
                "{\"jsonrpc\":\"2.0\",\"method\":\"common.data_passthrough\",\"params\":{\"data\":\"AQ==\",\"id\":103,\"size\":1}}\n",
                "{\"id\":2,\"jsonrpc\":\"2.0\",\"method\":\"notify-projection-started\"}\n",
            ]
        );
}

#[test]
fn full_handshake_against_a_fresh_dock_sends_the_exact_sequence() {
    let (dock, mi00) = FakeDock::spawn(FakeDockConfig::fresh());
    let session = Session::start(mi00, fast_settings()).unwrap();
    let handle = session.handle();
    assert!(!handle.is_armed(), "not armed before the handshake runs");
    wait_for("armed", Duration::from_secs(5), || handle.is_armed());
    assert_eq!(handle.state(), SessionState::Active);

    // The passthrough answer goes out once from the reader (reply to the dock's EDID) and
    // once from the handshake; their relative order is not fixed.
    let all: Vec<String> = texts(&dock)
        .into_iter()
        .filter(|t| !t.contains("\"heartbeat\""))
        .collect();
    let passthroughs = all
        .iter()
        .filter(|t| t.contains("data_passthrough"))
        .count();
    assert_eq!(passthroughs, 2);
    let sent: Vec<String> = all
        .into_iter()
        .filter(|t| !t.contains("data_passthrough"))
        .collect();
    assert_eq!(
            sent,
            [
                "{\"jsonrpc\":\"2.0\",\"method\":\"notify-projection-stopped\"}\n",
                "{\"id\":1,\"jsonrpc\":\"2.0\",\"method\":\"app-launched\",\"params\":{\"platform\":\"windows\",\"platform-machine-name\":\"VERMELHO\",\"product-type\":\"QuattroTx CT10\",\"res-height\":720,\"res-width\":1280}}\n",
                "{\"jsonrpc\":\"2.0\",\"method\":\"app_tx_mouse_icon_change\",\"params\":{\"cursor_base64_data\":\"\"}}\n",
                "{\"id\":2,\"jsonrpc\":\"2.0\",\"method\":\"notify-projection-started\"}\n",
            ]
        );
    // Heartbeats carry the exact vendor text and the constant id.
    let beats: Vec<String> = texts(&dock)
        .into_iter()
        .filter(|t| t.contains("\"heartbeat\""))
        .collect();
    assert!(!beats.is_empty());
    assert!(beats.iter().all(|b| b
        == r#"{"jsonrpc":"2.0", "method": "heartbeat", "params": {"session-id": "25282281"}}"#));
    assert_eq!(dock.format_violations(), 0);
    let stats = session.shutdown().unwrap();
    assert_eq!(stats.handshakes, 1);
    assert!(stats.dock_heartbeats_seen > 0);
}

#[test]
fn video_is_not_armed_until_is_allowed_plus_settle_delay() {
    let mut config = FakeDockConfig::fresh();
    config.launch_reply_delay = Some(Duration::from_millis(150));
    let (dock, mi00) = FakeDock::spawn(config);
    let mut settings = fast_settings();
    settings.settle_delay = Duration::from_millis(300);
    let session = Session::start(mi00, settings).unwrap();
    let handle = session.handle();
    wait_for("armed", Duration::from_secs(5), || handle.is_armed());
    let armed_at = Instant::now();
    let allowed_at = dock.allowed_sent_at().expect("dock sent isAllowed");
    assert!(
        armed_at.duration_since(allowed_at) >= Duration::from_millis(250),
        "armed only {:?} after isAllowed",
        armed_at.duration_since(allowed_at)
    );
    session.shutdown().unwrap();
}

#[test]
fn silent_dock_with_heartbeats_ends_in_assumed_active() {
    // The dock already holds a session: heartbeats only, no ready, no replies.
    let (dock, mi00) = FakeDock::spawn(FakeDockConfig::holding_session());
    let session = Session::start(mi00, fast_settings()).unwrap();
    let handle = session.handle();
    let started = Instant::now();
    wait_for("armed", Duration::from_secs(5), || handle.is_armed());
    assert_eq!(handle.state(), SessionState::AssumedActive);
    assert!(
        started.elapsed() >= Duration::from_millis(350),
        "waited the ready timeout"
    );
    assert_eq!(non_heartbeat(&dock), silent_sequence());
    session.shutdown().unwrap();
}

#[test]
fn unanswered_requests_time_out_into_assumed_active_when_dock_is_alive() {
    let mut config = FakeDockConfig::fresh();
    config.launch_reply_delay = None;
    let (_dock, mi00) = FakeDock::spawn(config);
    let session = Session::start(mi00, fast_settings()).unwrap();
    let handle = session.handle();
    wait_for("armed", Duration::from_secs(5), || handle.is_armed());
    assert_eq!(handle.state(), SessionState::AssumedActive);
    session.shutdown().unwrap();
}

#[test]
fn fully_silent_dock_arms_as_assumed_active_and_video_flows() {
    // No ready, no heartbeat echo, no replies: the dock still renders written video.
    let mut config = FakeDockConfig::holding_session();
    config.heartbeat_interval = None;
    let (dock, mi00, mi01) = FakeDock::spawn_with_video(config);
    let session = Session::start(mi00, fast_settings()).unwrap();
    let handle = session.handle();
    wait_for("armed", Duration::from_secs(5), || handle.is_armed());
    assert_eq!(handle.state(), SessionState::AssumedActive);
    assert_eq!(handle.generation(), 1);

    assert_eq!(non_heartbeat(&dock), silent_sequence());
    let mut mi01 = mi01;
    let chunks = crate::protocol::framing::MessageBuilder::video(size())
        .payload(&[0xAB; 5000])
        .build();
    for _ in 0..3 {
        mi01.write_chunks(&chunks).unwrap();
    }
    wait_for("video seen", Duration::from_secs(3), || {
        dock.video_stats().messages == 3
    });
    let video = dock.video_stats();
    assert_eq!(video.reports_before_sink, 0);
    assert_eq!(video.layout_violations, 0);
    // The handshake messages all precede the first video report; nothing else follows.
    assert_eq!(non_heartbeat(&dock), silent_sequence());
    let before = video.control_messages_before_video.unwrap();
    assert!(before >= 4);
    let stats = session.shutdown().unwrap();
    assert_eq!(stats.handshakes, 1);
    assert_eq!(stats.dock_heartbeats_seen, 0);
    assert!(stats.heartbeats_sent >= 2);
}

#[test]
fn late_ready_after_assumed_active_runs_the_full_handshake() {
    let mut config = FakeDockConfig::holding_session();
    config.launch_reply_delay = Some(Duration::from_millis(30));
    config.started_reply = StartedReply::Allowed;
    config.send_edid = true;
    let (dock, mi00) = FakeDock::spawn(config);
    let session = Session::start(mi00, fast_settings()).unwrap();
    let handle = session.handle();
    wait_for("assumed", Duration::from_secs(5), || handle.is_armed());
    assert_eq!(handle.state(), SessionState::AssumedActive);
    let generation = handle.generation();

    // The dock loses its session and starts announcing: the handshake must follow at once.
    dock.drop_session();
    wait_for("handshake", Duration::from_secs(5), || {
        handle.generation() > generation && handle.is_armed()
    });
    assert_eq!(handle.state(), SessionState::Active);
    let launches = texts(&dock)
        .iter()
        .filter(|t| t.contains("\"app-launched\""))
        .count();
    // One from the silent first attempt, one after the dock announced ready.
    assert_eq!(launches, 2);
    assert!(dock.sink_started());
    let stats = session.shutdown().unwrap();
    assert_eq!(stats.handshakes, 2);
    assert_eq!(stats.losses, 1);
}

#[test]
fn denied_projection_does_not_arm() {
    let mut config = FakeDockConfig::fresh();
    config.started_reply = StartedReply::Denied;
    let (_dock, mi00) = FakeDock::spawn(config);
    let session = Session::start(mi00, fast_settings()).unwrap();
    let handle = session.handle();
    // The denied attempt fails fast and never arms.
    std::thread::sleep(Duration::from_millis(300));
    assert!(!handle.is_armed());
    assert_eq!(handle.generation(), 0);
    // The dock now holds app state and stops announcing, so the retry ends in the
    // bounded fallback rather than blocking video forever.
    wait_for("fallback", Duration::from_secs(5), || handle.is_armed());
    assert_eq!(handle.state(), SessionState::AssumedActive);
    assert_eq!(handle.generation(), 1);
    session.shutdown().unwrap();
}

#[test]
fn error_reply_to_app_launched_fails_the_attempt_and_retries() {
    let (dock, mi00) = FakeDock::spawn(FakeDockConfig::fresh());
    dock.set_launch_error(true);
    let session = Session::start(mi00, fast_settings()).unwrap();
    let handle = session.handle();
    std::thread::sleep(Duration::from_millis(250));
    assert!(!handle.is_armed());
    assert_eq!(handle.generation(), 0);
    wait_for("fallback", Duration::from_secs(5), || handle.is_armed());
    assert_eq!(handle.state(), SessionState::AssumedActive);
    session.shutdown().unwrap();
}

#[test]
fn re_announced_ready_redoes_the_handshake_without_restarting() {
    let (dock, mi00) = FakeDock::spawn(FakeDockConfig::fresh());
    let session = Session::start(mi00, fast_settings()).unwrap();
    let handle = session.handle();
    wait_for("armed", Duration::from_secs(5), || handle.is_armed());
    let first_generation = handle.generation();

    dock.drop_session();
    wait_for("re-armed", Duration::from_secs(5), || {
        handle.is_armed() && handle.generation() > first_generation
    });
    let launches = texts(&dock)
        .iter()
        .filter(|t| t.contains("\"app-launched\""))
        .count();
    assert_eq!(launches, 2);
    // Ids keep counting up across handshakes.
    assert!(texts(&dock)
        .iter()
        .any(|t| t.starts_with("{\"id\":3,") && t.contains("app-launched")));
    let stats = session.shutdown().unwrap();
    assert_eq!(stats.handshakes, 2);
    assert_eq!(stats.losses, 1);
}

#[test]
fn reported_write_failure_redoes_the_handshake() {
    let (dock, mi00) = FakeDock::spawn(FakeDockConfig::fresh());
    let session = Session::start(mi00, fast_settings()).unwrap();
    let handle = session.handle();
    wait_for("armed", Duration::from_secs(5), || handle.is_armed());
    let generation = handle.generation();
    // The dock still holds its session, so the redo ends without a ready announcement.
    let _ = &dock;
    handle.report_failure("video write failed");
    wait_for("re-armed", Duration::from_secs(5), || {
        handle.generation() > generation && handle.is_armed()
    });
    session.shutdown().unwrap();
}

#[test]
fn heartbeat_keeps_its_cadence_independent_of_other_traffic() {
    let (dock, mi00) = FakeDock::spawn(FakeDockConfig::holding_session());
    let mut settings = fast_settings();
    settings.heartbeat_interval = Duration::from_millis(40);
    settings.ready_timeout = Duration::from_secs(30);
    let session = Session::start(mi00, settings).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    let times = dock.heartbeat_times();
    assert!(times.len() >= 10, "{} heartbeats", times.len());
    let worst = times
        .windows(2)
        .map(|w| w[1].duration_since(w[0]))
        .max()
        .unwrap();
    assert!(worst < Duration::from_millis(200), "worst gap {worst:?}");
    session.shutdown().unwrap();
}

#[test]
fn startup_write_failure_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ro");
    std::fs::write(&path, b"").unwrap();
    let ro = HidrawHandle::from_file(
        std::fs::OpenOptions::new().read(true).open(&path).unwrap(),
        ReportId::Control,
    );
    let err = Session::start(ro, fast_settings().without_handshake()).unwrap_err();
    assert!(matches!(err, SessionError::StartupHeartbeat(_)));
    let video = HidrawHandle::from_file(std::fs::File::open(&path).unwrap(), ReportId::Video);
    assert!(matches!(
        Session::start(video, fast_settings()).unwrap_err(),
        SessionError::WrongChannel { .. }
    ));
}

#[test]
fn without_handshake_arms_immediately_and_only_heartbeats() {
    let dir = tempfile::tempdir().unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(dir.path().join("mi00"))
        .unwrap();
    let handle = HidrawHandle::from_file(file, ReportId::Control);
    let session = Session::start(handle, fast_settings().without_handshake()).unwrap();
    assert!(session.handle().is_armed());
    std::thread::sleep(Duration::from_millis(180));
    let stats = session.shutdown().unwrap();
    assert!(stats.heartbeats_sent >= 3);
    assert_eq!(stats.handshakes, 1);
}

#[test]
fn reader_end_of_stream_is_fatal() {
    // A regular file reads as end of stream immediately.
    let dir = tempfile::tempdir().unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(dir.path().join("mi00"))
        .unwrap();
    let handle = HidrawHandle::from_file(file, ReportId::Control);
    let session = Session::start(handle, fast_settings()).unwrap();
    let sh = session.handle();
    wait_for("fatal", Duration::from_secs(3), || sh.fatal().is_some());
    assert!(!sh.is_armed());
    session.shutdown().unwrap();
}
