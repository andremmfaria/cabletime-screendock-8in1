//! The daemon loop on a simulated dock with a supervisor that manages a virtual monitor on a
//! real display server, shared by the tests that change the monitor from outside.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use screendockd::adapters::display::SystemDisplayProvider;
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
use screendockd::ports::encode::{
    EncodeError, Encoder, EncoderSettings, RateSettings, VideoMessage,
};
use screendockd::protocol::control::{ControlRequest, ControlResponse};
use screendockd::protocol::framing::PictureSize;
use screendockd::testing::fake_dock::{FakeDock, FakeDockConfig};
use screendockd::utils::config::load_from_path;
use screendockd::utils::types::DisplayMode;

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
pub(crate) type Emitted = Arc<Mutex<Vec<(usize, bool, u8)>>>;

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

pub(crate) fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A daemon on a simulated dock whose virtual monitor lives on the Xorg dummy server.
pub(crate) struct Rig {
    pub(crate) dock: FakeDock,
    pub(crate) commands: Sender<Command>,
    stop: Arc<AtomicBool>,
    daemon: JoinHandle<Result<DaemonReport, DaemonError>>,
    supervisor: JoinHandle<()>,
    pub(crate) built: Arc<Mutex<Vec<EncoderSettings>>>,
    pub(crate) emitted: Emitted,
    pub(crate) targets: Arc<Mutex<Vec<CaptureTarget>>>,
}

/// Builds the rig around `provider`, with the monitor created in extend mode.
pub(crate) fn rig_with(provider: SystemDisplayProvider, follow: bool) -> Rig {
    let (dock, mi00, mi01) = FakeDock::spawn_with_video(FakeDockConfig::fresh());
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/screendock_cfg.toml");
    let config = load_from_path(&path).unwrap();
    let mut live = LiveSettings::try_from(&config).unwrap();
    live.mode = DisplayMode::Extend;
    live.follow_system_changes = follow;

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
    Rig {
        dock,
        commands,
        stop,
        daemon,
        supervisor,
        built,
        emitted,
        targets,
    }
}

impl Rig {
    pub(crate) fn ask(&self, request: ControlRequest) -> ControlResponse {
        let (reply, answer) = mpsc::channel();
        self.commands
            .send(Command::Request { request, reply })
            .unwrap();
        answer.recv_timeout(Duration::from_secs(15)).unwrap()
    }

    pub(crate) fn mode(&self) -> String {
        match self.ask(ControlRequest::Get {
            key: Some("mode".into()),
        }) {
            ControlResponse::Values { values } => values["mode"].clone(),
            other => panic!("{other:?}"),
        }
    }

    pub(crate) fn finish(self) -> (DaemonReport, FakeDock) {
        self.stop.store(true, Ordering::Relaxed);
        let report = self.daemon.join().unwrap().unwrap();
        let _ = self.commands.send(Command::Shutdown);
        self.supervisor.join().unwrap();
        (report, self.dock)
    }
}
