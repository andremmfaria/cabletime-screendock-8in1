//! The daemon following its virtual monitor on a rootless Xorg with the dummy driver: a mode
//! change made with `xrandr` becomes one dock cycle with a rebuilt encoder and a new keyframe,
//! the daemon's own changes do not loop, and `follow_system_changes = false` leaves the dock
//! alone.
//!
//! Skips when Xorg with the dummy driver is unavailable (`SCREENDOCK_REQUIRE_XORG=1` makes that
//! a failure).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use screendockd::adapters::display::{RecoveryStore, SystemDisplayProvider};
use screendockd::adapters::encode::prerecorded::PrerecordedEncoder;
use screendockd::app::daemon::{run, DaemonError, DaemonParams, DaemonReport, PipelineKnobs};
use screendockd::app::monitor::{MonitorManager, MonitorPipeline};
use screendockd::app::pipeline::{EncoderBuilder, EncoderPipeline, PipelineState};
use screendockd::app::supervisor::Supervisor;
use screendockd::core::reconfig::LiveSettings;
use screendockd::core::session::SessionSettings;
use screendockd::ports::capture::{
    CaptureArea, CaptureBackend, CaptureError, CaptureTarget, Frame, PixelFormat,
};
use screendockd::ports::control::Command;
use screendockd::ports::display::Rect;
use screendockd::ports::encode::{
    EncodeError, Encoder, EncoderSettings, RateSettings, VideoMessage,
};
use screendockd::protocol::control::{ControlRequest, ControlResponse};
use screendockd::protocol::framing::PictureSize;
use screendockd::testing::fake_dock::{FakeDock, FakeDockConfig};
use screendockd::utils::config::load_from_path;
use screendockd::utils::types::DisplayMode;

use crate::support::{require_xorg, xrandr, XorgDummy};

struct Tracking {
    targets: Arc<Mutex<Vec<CaptureTarget>>>,
}

impl CaptureBackend for Tracking {
    fn name(&self) -> &'static str {
        "tracking"
    }
    fn next_frame(&mut self) -> Result<Frame, CaptureError> {
        std::thread::sleep(Duration::from_millis(5));
        Frame::new(2, 2, PixelFormat::Bgrx8888, 8, vec![0; 16])
    }
    fn retarget(&mut self, target: CaptureTarget) -> Result<(), CaptureError> {
        self.targets.lock().unwrap().push(target);
        Ok(())
    }
}

/// What an encoder produced: its build number, whether the message is a keyframe and the type
/// of the first NAL.
type Emitted = Arc<Mutex<Vec<(usize, bool, u8)>>>;

struct Logged {
    inner: PrerecordedEncoder,
    number: usize,
    emitted: Emitted,
}

impl Encoder for Logged {
    fn name(&self) -> &'static str {
        "logged"
    }
    fn encode(&mut self, frame: &Frame) -> Result<Vec<VideoMessage>, EncodeError> {
        let messages = self.inner.encode(frame)?;
        self.emitted.lock().unwrap().extend(
            messages
                .iter()
                .map(|m| (self.number, m.is_keyframe(), m.as_bytes()[4] & 0x1F)),
        );
        Ok(messages)
    }
    fn request_keyframe(&mut self) {
        self.inner.request_keyframe();
    }
    fn set_rate(&mut self, _rate: RateSettings) -> Result<(), EncodeError> {
        Ok(())
    }
    fn supports_live_rate(&self) -> bool {
        true
    }
}

fn stream_with(settings: EncoderSettings) -> PrerecordedEncoder {
    let mut messages = vec![vec![
        0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65, 9,
    ]];
    for _ in 0..199 {
        messages.push(vec![0, 0, 0, 1, 0x41, 4]);
    }
    PrerecordedEncoder::new(messages, settings).unwrap()
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

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A daemon on a simulated dock whose virtual monitor lives on the Xorg dummy server.
struct Rig {
    xorg: XorgDummy,
    dock: FakeDock,
    commands: Sender<Command>,
    stop: Arc<AtomicBool>,
    daemon: JoinHandle<Result<DaemonReport, DaemonError>>,
    supervisor: JoinHandle<()>,
    built: Arc<Mutex<Vec<EncoderSettings>>>,
    emitted: Emitted,
    targets: Arc<Mutex<Vec<CaptureTarget>>>,
    _dir: tempfile::TempDir,
}

fn rig(follow: bool) -> Option<Rig> {
    let xorg = require_xorg()?;
    let dir = tempfile::tempdir().unwrap();
    let (dock, mi00, mi01) = FakeDock::spawn_with_video(FakeDockConfig::fresh());
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/screendock_cfg.toml");
    let config = load_from_path(&path).unwrap();
    let mut live = LiveSettings::try_from(&config).unwrap();
    live.mode = DisplayMode::Extend;
    live.follow_system_changes = follow;

    let store = RecoveryStore::new(dir.path().join("run/vdisplay-state.json"));
    let provider =
        SystemDisplayProvider::for_x11_display(xorg.display.clone()).with_recovery_store(store);
    let (commands, inbox) = mpsc::channel();
    let mut monitors = MonitorManager::new(Box::new(provider), config.virtual_display.clone());
    monitors.set_events(commands.clone());
    let area = monitors.start(&live).expect("the monitor");
    assert!(matches!(area, CaptureArea::Region(_)), "{area:?}");

    let built: Arc<Mutex<Vec<EncoderSettings>>> = Arc::default();
    let emitted: Emitted = Arc::default();
    let targets: Arc<Mutex<Vec<CaptureTarget>>> = Arc::default();
    let make = {
        let built = Arc::clone(&built);
        let emitted = Arc::clone(&emitted);
        move |settings: EncoderSettings| -> Result<Box<dyn Encoder>, EncodeError> {
            let number = {
                let mut built = built.lock().unwrap();
                built.push(settings);
                built.len() - 1
            };
            Ok(Box::new(Logged {
                inner: stream_with(settings),
                number,
                emitted: Arc::clone(&emitted),
            }))
        }
    };
    let state = PipelineState::with_builder(
        PipelineKnobs {
            area,
            ..PipelineKnobs::from_live(&live)
        },
        true,
        EncoderBuilder::new(make.clone()),
    );
    let params = DaemonParams {
        geometry: live.geometry,
        session: fast_settings(live.geometry.size()),
        max_frames: None,
        max_consecutive_errors: 50,
        pipeline: Arc::clone(&state),
    };
    let first_settings = state.knobs().encoder_settings();
    let stop = Arc::new(AtomicBool::new(false));
    let daemon = {
        let stop = Arc::clone(&stop);
        let targets = Arc::clone(&targets);
        std::thread::spawn(move || {
            let mut first = make(first_settings).unwrap();
            run(
                mi00,
                mi01,
                &mut Tracking { targets },
                first.as_mut(),
                &params,
                &stop,
            )
        })
    };
    let supervisor = Supervisor::new(
        live,
        Box::new(|| Err("no file".into())),
        Box::new(MonitorPipeline::new(EncoderPipeline::new(state), monitors)),
    );
    let supervisor = std::thread::spawn(move || supervisor.run(&inbox));
    Some(Rig {
        xorg,
        dock,
        commands,
        stop,
        daemon,
        supervisor,
        built,
        emitted,
        targets,
        _dir: dir,
    })
}

impl Rig {
    fn ask(&self, request: ControlRequest) -> ControlResponse {
        let (reply, answer) = mpsc::channel();
        self.commands
            .send(Command::Request { request, reply })
            .unwrap();
        answer.recv_timeout(Duration::from_secs(15)).unwrap()
    }

    fn mode(&self) -> String {
        match self.ask(ControlRequest::Get {
            key: Some("mode".into()),
        }) {
            ControlResponse::Values { values } => values["mode"].clone(),
            other => panic!("{other:?}"),
        }
    }

    /// Gives the monitor's output a 1600x900 mode with `xrandr`, the way a display settings
    /// dialog does.
    fn change_mode_with_xrandr(&self) {
        let d = &self.xorg.display;
        let output = &screendockd::adapters::display::x11_facts(Some(d))
            .unwrap()
            .outputs[1]
            .name;
        xrandr(d, &["--fb", "3000x1000"]);
        xrandr(
            d,
            &[
                "--newmode",
                "1600x900t",
                "98.21",
                "1600",
                "1648",
                "1680",
                "1760",
                "900",
                "903",
                "906",
                "930",
                "+hsync",
                "-vsync",
            ],
        );
        xrandr(d, &["--addmode", output, "1600x900t"]);
        xrandr(d, &["--output", output, "--mode", "1600x900t"]);
    }

    fn finish(self) -> (DaemonReport, FakeDock) {
        self.stop.store(true, Ordering::Relaxed);
        let report = self.daemon.join().unwrap().unwrap();
        let _ = self.commands.send(Command::Shutdown);
        self.supervisor.join().unwrap();
        (report, self.dock)
    }
}

#[test]
fn an_xrandr_mode_change_runs_one_dock_cycle_with_a_rebuilt_encoder_and_a_new_keyframe() {
    let Some(rig) = rig(true) else { return };
    wait_for("video", || rig.dock.video_stats().messages > 5);
    assert_eq!(rig.dock.launch_sizes(), [(1280, 720)]);

    let changed = Instant::now();
    rig.change_mode_with_xrandr();
    wait_for("the dock cycle at the new size", || {
        rig.dock.launch_sizes() == [(1280, 720), (1600, 900)]
    });
    let detected_in = changed.elapsed();
    assert!(
        detected_in < Duration::from_secs(2),
        "the cycle began {detected_in:?} after the change"
    );
    wait_for("video at the new size", || {
        rig.dock.video_stats().sizes == [(1280, 720), (1600, 900)]
    });
    // Nothing feeds back: no second cycle follows.
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(rig.mode(), "extend");
    let (emitted, built, targets) = (rig.emitted.clone(), rig.built.clone(), rig.targets.clone());
    let (report, dock) = rig.finish();
    assert_eq!(report.handshakes, 2);
    assert_eq!(dock.launch_sizes(), [(1280, 720), (1600, 900)]);
    let built = built.lock().unwrap();
    assert_eq!(
        built.len(),
        3,
        "the initial encoder, the probe and the new one"
    );
    assert_eq!(built[2].geometry.size(), PictureSize::new(1600, 900));
    let emitted = emitted.lock().unwrap();
    let first_new = emitted.iter().find(|(number, ..)| *number == 2).unwrap();
    assert_eq!(
        (first_new.1, first_new.2),
        (true, 7),
        "keyframe with parameter sets"
    );
    let last = targets.lock().unwrap().last().copied().unwrap();
    assert_eq!(
        last.area,
        CaptureArea::Region(Rect::new(1280, 0, 1600, 900))
    );
    let beats = dock.heartbeat_times();
    let widest = beats.windows(2).map(|w| w[1] - w[0]).max().unwrap();
    assert!(widest < Duration::from_secs(2), "heartbeat gap {widest:?}");
}

#[test]
fn a_size_set_with_ctl_runs_one_dock_cycle_and_the_echo_is_not_followed() {
    let Some(rig) = rig(true) else { return };
    wait_for("video", || rig.dock.video_stats().messages > 5);
    let answer = rig.ask(ControlRequest::Set {
        key: "resolution".into(),
        value: "1600x900".into(),
    });
    assert!(
        matches!(answer, ControlResponse::Applied { .. }),
        "{answer:?}"
    );
    wait_for("video at the new size", || {
        rig.dock.video_stats().sizes == [(1280, 720), (1600, 900)]
    });
    let listed = xrandr(&rig.xorg.display, &["--listmonitors"]);
    assert!(listed.contains("1600/"), "{listed}");
    std::thread::sleep(Duration::from_millis(1500));
    let (report, dock) = rig.finish();
    assert_eq!(report.handshakes, 2, "the change ran one cycle");
    assert_eq!(dock.launch_sizes(), [(1280, 720), (1600, 900)]);
}

#[test]
fn with_following_off_an_xrandr_mode_change_leaves_the_dock_alone() {
    let Some(rig) = rig(false) else { return };
    wait_for("video", || rig.dock.video_stats().messages > 5);
    rig.change_mode_with_xrandr();
    std::thread::sleep(Duration::from_millis(1500));
    let (report, dock) = rig.finish();
    assert_eq!(report.handshakes, 1);
    assert_eq!(dock.launch_sizes(), [(1280, 720)]);
    assert_eq!(dock.video_stats().sizes, [(1280, 720)]);
}

#[test]
fn turning_the_monitor_off_with_xrandr_reverts_the_daemon_to_mirror() {
    let Some(rig) = rig(true) else { return };
    wait_for("video", || rig.dock.video_stats().messages > 5);
    let d = &rig.xorg.display;
    let output = screendockd::adapters::display::x11_facts(Some(d))
        .unwrap()
        .outputs[1]
        .name
        .clone();
    xrandr(d, &["--output", &output, "--off"]);
    wait_for("mirror mode", || rig.mode() == "mirror");
    let (report, _dock) = rig.finish();
    assert_eq!(report.handshakes, 1, "the picture size did not change");
}
