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
use screendockd::core::session::SessionSettings;
use screendockd::ports::capture::{CaptureBackend, CaptureError, Frame, PixelFormat};
use screendockd::ports::control::Command;
use screendockd::ports::encode::{
    EncodeError, Encoder, EncoderSettings, RateSettings, VideoMessage,
};
use screendockd::protocol::control::{ControlRequest, ControlResponse};
use screendockd::protocol::display::{DisplayGeometry, Orientation};
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
