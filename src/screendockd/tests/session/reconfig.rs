//! Live encoder changes against a simulated dock: a bit rate or keyframe interval change goes
//! through the supervisor to the running video loop without another dock handshake.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use screendockd::adapters::encode::prerecorded::PrerecordedEncoder;
use screendockd::app::daemon::{run, DaemonParams, PipelineKnobs};
use screendockd::app::pipeline::{EncoderPipeline, PipelineState};
use screendockd::app::supervisor::Supervisor;
use screendockd::core::reconfig::LiveSettings;
use screendockd::core::session::{Session, SessionError, SessionSettings, SessionState};
use screendockd::ports::capture::{CaptureBackend, CaptureError, Frame, PixelFormat};
use screendockd::ports::control::Command;
use screendockd::ports::encode::{
    EncodeError, Encoder, EncoderSettings, RateSettings, VideoMessage,
};
use screendockd::protocol::control::{ControlRequest, ControlResponse};
use screendockd::protocol::display::{DisplayGeometry, Orientation};
use screendockd::protocol::framing::{MessageBuilder, PictureSize};
use screendockd::testing::fake_dock::{FakeDock, FakeDockConfig};
use screendockd::utils::config::load_from_path;

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

/// Replays a stream that is long enough for the keyframe clock to decide the spacing, and
/// records what the loop did to it.
struct Recorder {
    inner: PrerecordedEncoder,
    keyframes: Arc<Mutex<Vec<bool>>>,
    rates: Arc<Mutex<Vec<RateSettings>>>,
}

impl Encoder for Recorder {
    fn name(&self) -> &'static str {
        "recorder"
    }
    fn encode(&mut self, frame: &Frame) -> Result<Vec<VideoMessage>, EncodeError> {
        let messages = self.inner.encode(frame)?;
        let mut keyframes = self.keyframes.lock().unwrap();
        keyframes.extend(messages.iter().map(VideoMessage::is_keyframe));
        Ok(messages)
    }
    fn request_keyframe(&mut self) {
        self.inner.request_keyframe();
    }
    fn set_rate(&mut self, rate: RateSettings) -> Result<(), EncodeError> {
        self.rates.lock().unwrap().push(rate);
        Ok(())
    }
    fn supports_live_rate(&self) -> bool {
        true
    }
}

fn geometry() -> DisplayGeometry {
    DisplayGeometry::new(1280, 720, Orientation::Landscape).unwrap()
}

fn stream() -> PrerecordedEncoder {
    let settings = EncoderSettings {
        geometry: geometry(),
        fps: 25,
        keyframe_interval: 50,
        bitrate_kbps: 4000,
        preset: screendockd::utils::config::X264Preset::Ultrafast,
        zerolatency: true,
        vbv_buffer_ms: 100,
    };
    let mut messages = vec![vec![
        0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65, 9,
    ]];
    for _ in 0..199 {
        messages.push(vec![0, 0, 0, 1, 0x41, 4]);
    }
    PrerecordedEncoder::new(messages, settings).unwrap()
}

fn non_heartbeat(dock: &FakeDock) -> usize {
    dock.host_texts()
        .iter()
        .filter(|t| !t.contains("\"heartbeat\""))
        .count()
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn ask(supervisor: &mut Supervisor, key: &str, value: &str) -> ControlResponse {
    let (reply, answer) = mpsc::channel();
    supervisor.handle(Command::Request {
        request: ControlRequest::Set {
            key: key.into(),
            value: value.into(),
        },
        reply,
    });
    answer.recv().unwrap()
}

#[test]
fn live_bitrate_and_keyframe_interval_changes_need_no_new_handshake() {
    let (dock, mi00, mi01) = FakeDock::spawn_with_video(FakeDockConfig::fresh());
    let state = PipelineState::new(
        PipelineKnobs {
            bitrate_kbps: 4000,
            vbv_buffer_ms: 100,
            keyframe_interval: 50,
        },
        true,
    );
    let mut session = SessionSettings::new(geometry().size(), Duration::from_millis(50));
    session.machine_name = "TESTHOST".into();
    session.ready_timeout = Duration::from_millis(400);
    session.reply_timeout = Duration::from_millis(400);
    session.settle_delay = Duration::from_millis(100);
    session.retry_delay = Duration::from_millis(50);
    let params = DaemonParams {
        geometry: geometry(),
        session,
        max_frames: None,
        max_consecutive_errors: 50,
        pipeline: Arc::clone(&state),
    };
    let keyframes = Arc::new(Mutex::new(Vec::new()));
    let rates = Arc::new(Mutex::new(Vec::new()));
    let mut recorder = Recorder {
        inner: stream(),
        keyframes: Arc::clone(&keyframes),
        rates: Arc::clone(&rates),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let daemon = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || run(mi00, mi01, &mut Solid, &mut recorder, &params, &stop))
    };

    wait_for("video", || keyframes.lock().unwrap().len() > 20);
    let handshake_messages = non_heartbeat(&dock);

    let config =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/screendock_cfg.toml");
    let live = LiveSettings::try_from(&load_from_path(&config).unwrap()).unwrap();
    let mut supervisor = Supervisor::new(
        live,
        Box::new(|| Err("no file".into())),
        Box::new(EncoderPipeline::new(Arc::clone(&state))),
    );
    for (key, value) in [("bitrate", "2000"), ("keyframe-interval", "5")] {
        match ask(&mut supervisor, key, value) {
            ControlResponse::Applied { class, .. } => assert_eq!(class, "live"),
            other => panic!("{key}: {other:?}"),
        }
    }
    let changed_at = keyframes.lock().unwrap().len();
    wait_for("frames after the change", || {
        keyframes.lock().unwrap().len() > changed_at + 60
    });
    stop.store(true, Ordering::Relaxed);
    let report = daemon.join().unwrap().unwrap();

    assert_eq!(report.handshakes, 1, "no second handshake");
    assert_eq!(
        non_heartbeat(&dock),
        handshake_messages,
        "no new host messages"
    );
    assert_eq!(
        rates.lock().unwrap().as_slice(),
        [RateSettings {
            bitrate_kbps: 2000,
            vbv_buffer_ms: 100
        }]
    );
    let keyframes = keyframes.lock().unwrap();
    let positions: Vec<usize> = keyframes
        .iter()
        .enumerate()
        .filter(|(n, key)| **key && *n >= changed_at + 10)
        .map(|(n, _)| n)
        .collect();
    assert!(positions.len() >= 5, "{positions:?}");
    assert!(
        positions.windows(2).all(|w| w[1] - w[0] == 5),
        "keyframe spacing after the change: {positions:?}"
    );
    let beats = dock.heartbeat_times();
    let widest = beats.windows(2).map(|w| w[1] - w[0]).max().unwrap();
    assert!(widest < Duration::from_secs(2), "heartbeat gap {widest:?}");
}

fn fast_settings(size: PictureSize) -> SessionSettings {
    let mut settings = SessionSettings::new(size, Duration::from_millis(50));
    settings.machine_name = "TESTHOST".into();
    settings.ready_timeout = Duration::from_millis(400);
    settings.reply_timeout = Duration::from_millis(400);
    settings.settle_delay = Duration::from_millis(100);
    settings.retry_delay = Duration::from_millis(50);
    settings
}

fn old() -> PictureSize {
    PictureSize::new(1280, 720)
}

fn new_size() -> PictureSize {
    PictureSize::new(1600, 900)
}

fn armed_session(config: FakeDockConfig) -> (FakeDock, Session) {
    let (dock, mi00) = FakeDock::spawn(config);
    let session = Session::start(mi00, fast_settings(old())).unwrap();
    let handle = session.handle();
    wait_for("the first handshake", || handle.is_armed());
    (dock, session)
}

#[test]
fn renegotiation_runs_stop_launch_at_the_new_size_and_start_in_order() {
    let (dock, session) = armed_session(FakeDockConfig::fresh());
    let handle = session.handle();
    let before = handle.generation();
    let first = dock.host_methods().len();

    session
        .renegotiate(new_size(), Duration::from_secs(5))
        .unwrap();

    let cycle: Vec<String> = dock.host_methods().split_off(first);
    let launch_at = cycle.iter().position(|m| m == "app-launched").unwrap();
    let stopped_at = cycle
        .iter()
        .position(|m| m == "notify-projection-stopped")
        .unwrap();
    let started_at = cycle
        .iter()
        .position(|m| m == "notify-projection-started")
        .unwrap();
    assert!(
        stopped_at < launch_at && launch_at < started_at,
        "{cycle:?}"
    );
    assert_eq!(dock.launch_sizes(), [(1280, 720), (1600, 900)]);
    assert!(handle.is_armed());
    assert_eq!(handle.state(), SessionState::Active);
    assert_eq!(handle.size(), new_size());
    assert_eq!(handle.generation(), before + 1);
    let beats = dock.heartbeat_times();
    let widest = beats.windows(2).map(|w| w[1] - w[0]).max().unwrap();
    assert!(widest < Duration::from_secs(2), "heartbeat gap {widest:?}");
}

#[test]
fn renegotiation_sends_the_stop_even_to_a_silent_dock_and_arms_assumed_active() {
    let (dock, session) = armed_session(FakeDockConfig::holding_session());
    let handle = session.handle();
    assert!(!dock
        .host_methods()
        .contains(&"notify-projection-stopped".to_string()));

    session
        .renegotiate(new_size(), Duration::from_secs(5))
        .unwrap();

    assert!(dock
        .host_methods()
        .contains(&"notify-projection-stopped".to_string()));
    assert_eq!(dock.launch_sizes().last(), Some(&(1600, 900)));
    assert_eq!(handle.state(), SessionState::AssumedActive);
}

#[test]
fn video_headers_follow_the_new_size_and_none_is_written_while_the_cycle_runs() {
    let (dock, mi00, mi01) = FakeDock::spawn_with_video(FakeDockConfig::fresh());
    let session = Session::start(mi00, fast_settings(old())).unwrap();
    let handle = session.handle();
    wait_for("the first handshake", || handle.is_armed());

    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let stop = Arc::clone(&stop);
        let handle = handle.clone();
        std::thread::spawn(move || {
            let mut mi01 = mi01;
            let mut written_unarmed = 0u32;
            while !stop.load(Ordering::Relaxed) {
                if handle.is_armed() {
                    let size = handle.size();
                    let chunks = MessageBuilder::video(size)
                        .payload(&[0, 0, 0, 1, 0x41, 4])
                        .build();
                    let _ = mi01.write_chunks(&chunks);
                } else {
                    written_unarmed += 1;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            written_unarmed
        })
    };
    wait_for("video at the old size", || {
        dock.video_stats().sizes == [(1280, 720)]
    });

    session
        .renegotiate(new_size(), Duration::from_secs(5))
        .unwrap();
    wait_for("video at the new size", || {
        dock.video_stats().sizes == [(1280, 720), (1600, 900)]
    });
    stop.store(true, Ordering::Relaxed);
    let skipped = writer.join().unwrap();
    assert!(skipped > 0, "the writer saw the session unarmed");
    assert_eq!(dock.video_stats().layout_violations, 0);
}

#[test]
fn a_size_the_dock_refuses_fails_the_cycle_and_the_old_size_comes_back() {
    let (dock, session) = armed_session(FakeDockConfig::fresh());
    let handle = session.handle();
    dock.reject_size(new_size());

    let result = session.renegotiate(new_size(), Duration::from_secs(5));
    assert!(
        matches!(result, Err(SessionError::Rejected { .. })),
        "{result:?}"
    );
    assert_eq!(handle.size(), old());

    wait_for("the old size to be armed again", || {
        handle.is_armed() && dock.launch_sizes().last() == Some(&(1280, 720))
    });
    assert_eq!(handle.size(), old());
    assert!(handle.is_armed());
}

#[test]
fn renegotiation_without_a_handshake_is_unavailable() {
    let (_dock, mi00) = FakeDock::spawn(FakeDockConfig::fresh());
    let session = Session::start(mi00, fast_settings(old()).without_handshake()).unwrap();
    assert!(matches!(
        session.renegotiate(new_size(), Duration::from_millis(100)),
        Err(SessionError::RenegotiationUnavailable)
    ));
}

#[test]
fn two_renegotiations_in_a_row_are_served_in_order() {
    let (dock, session) = armed_session(FakeDockConfig::fresh());
    let handle = session.handle();
    let other = handle.clone();
    let first = std::thread::spawn(move || {
        other.renegotiate(PictureSize::new(1600, 900), Duration::from_secs(10))
    });
    session
        .renegotiate(PictureSize::new(960, 540), Duration::from_secs(10))
        .unwrap();
    first.join().unwrap().unwrap();
    let launches = dock.launch_sizes();
    assert_eq!(launches.len(), 3, "{launches:?}");
    assert!(handle.is_armed());
}

#[test]
fn the_dock_dropping_the_session_after_some_video_is_noticed() {
    let (dock, mi00, mi01) = FakeDock::spawn_with_video(FakeDockConfig::fresh());
    let session = Session::start(mi00, fast_settings(old())).unwrap();
    let handle = session.handle();
    wait_for("the first handshake", || handle.is_armed());
    dock.drop_session_after(3);
    let mut mi01 = mi01;
    for _ in 0..3 {
        let chunks = MessageBuilder::video(old())
            .payload(&[0, 0, 0, 1, 0x41, 4])
            .build();
        mi01.write_chunks(&chunks).unwrap();
    }
    wait_for("the loss", || handle.losses() >= 1);
    wait_for("the handshake again", || {
        handle.is_armed() && handle.handshakes() >= 2
    });
}
