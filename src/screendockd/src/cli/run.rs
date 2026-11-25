//! The `run` subcommand.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result};
use signal_hook::consts::SIGHUP;
use signal_hook::iterator::Signals;

use crate::adapters::capture::create_backend;
use crate::adapters::control::{socket_path, ControlServer};
use crate::adapters::encode::create_encoder;
use crate::adapters::transport::hidraw::HidrawHandle;
use crate::app::daemon::{self, DaemonParams, PipelineKnobs, DEFAULT_MAX_CONSECUTIVE_ERRORS};
use crate::app::pipeline::{EncoderPipeline, PipelineState};
use crate::app::supervisor::{PipelineHandle, ReloadFn, Supervisor};
use crate::core::reconfig::LiveSettings;
use crate::ports::control::Command;
use crate::ports::encode::EncoderSettings;
use crate::utils::config::Config;

use super::support::{
    device_ids, geometry, load_config, resolve_nodes, session_settings, shutdown_flag,
    warn_if_not_char_device,
};
use super::types::{RunArgs, Services};

/// How often the SIGHUP thread looks for a pending signal.
const SIGHUP_POLL: Duration = Duration::from_millis(100);

pub(super) fn run(args: RunArgs, config: &Config, config_path: Option<&PathBuf>) -> Result<()> {
    let geometry = geometry(config)?;
    let live = LiveSettings::try_from(config).context("invalid settings")?;
    let (mi00, mi01) = resolve_nodes(&device_ids(config)?, &args.nodes, true, true)?;
    let mi00 = mi00.expect("MI_00 was requested");
    let mi01 = mi01.expect("MI_01 was requested");
    warn_if_not_char_device(&mi00);
    warn_if_not_char_device(&mi01);
    let mi00_handle = HidrawHandle::open_mi00(&mi00).context("failed to open MI_00")?;
    let mi01_handle = HidrawHandle::open_mi01(&mi01).context("failed to open MI_01")?;
    let choice = args.backend.map_or(config.capture.backend, Into::into);
    let mut capture = create_backend(config, choice).context("failed to start capture")?;
    let settings = EncoderSettings::from_config(config, geometry);
    let mut encoder =
        create_encoder(&args.encoder.kind(), settings).context("failed to start encoder")?;
    let stop = shutdown_flag()?;
    let pipeline = PipelineState::new(
        PipelineKnobs {
            bitrate_kbps: live.bitrate_kbps,
            vbv_buffer_ms: live.vbv_buffer_ms,
            keyframe_interval: live.keyframe_interval,
        },
        encoder.supports_live_rate(),
    );
    let services = Services::start(
        config,
        live,
        reload_source(config_path.cloned()),
        Box::new(EncoderPipeline::new(std::sync::Arc::clone(&pipeline))),
    )?;
    tracing::info!(
        capture = capture.name(),
        encoder = encoder.name(),
        width = geometry.width().0,
        height = geometry.height().0,
        "starting daemon"
    );
    let params = DaemonParams {
        geometry,
        session: session_settings(config, geometry.size(), &args.handshake),
        max_frames: args.max_frames,
        max_consecutive_errors: DEFAULT_MAX_CONSECUTIVE_ERRORS,
        pipeline,
    };
    let result = daemon::run(
        mi00_handle,
        mi01_handle,
        capture.as_mut(),
        encoder.as_mut(),
        &params,
        &stop,
    );
    services.stop();
    let report = result.context("daemon failed")?;
    println!("{report:?}");
    Ok(())
}

/// The function the supervisor calls to read the config file again: the file given with
/// `--config`, or the first file of the resolution order.
fn reload_source(path: Option<PathBuf>) -> ReloadFn {
    Box::new(move || {
        let config = load_config(path.as_ref()).map_err(|e| format!("{e:#}"))?;
        LiveSettings::try_from(&config).map_err(|e| e.to_string())
    })
}

impl Services {
    /// Starts the supervisor thread, the SIGHUP thread and, when the config enables it and a
    /// socket path exists, the control socket.
    pub(super) fn start(
        config: &Config,
        live: LiveSettings,
        reload: ReloadFn,
        pipeline: Box<dyn PipelineHandle>,
    ) -> Result<Self> {
        let (commands, inbox) = mpsc::channel();
        let halt = Arc::new(AtomicBool::new(false));

        let server = if config.control.enabled {
            match socket_path(&config.control) {
                Some(path) => Some(
                    ControlServer::start(&path, commands.clone()).with_context(|| {
                        format!("failed to start the control socket at {path:?}")
                    })?,
                ),
                None => {
                    tracing::warn!(
                        "control socket disabled: no control.socket and $XDG_RUNTIME_DIR is unset"
                    );
                    None
                }
            }
        } else {
            None
        };

        let supervisor = Supervisor::new(live, reload, pipeline)
            .with_status_field("version", env!("CARGO_PKG_VERSION"))
            .with_status_field("pid", std::process::id().to_string());
        let supervisor = std::thread::Builder::new()
            .name("supervisor".into())
            .spawn(move || supervisor.run(&inbox))
            .context("failed to start the supervisor")?;
        let sighup = spawn_sighup(commands.clone(), Arc::clone(&halt))?;
        Ok(Self {
            commands: Some(commands),
            server,
            halt,
            sighup: Some(sighup),
            supervisor: Some(supervisor),
        })
    }

    /// Stops the SIGHUP thread, closes the control socket and ends the supervisor.
    pub(super) fn stop(mut self) {
        self.halt.store(true, Ordering::Relaxed);
        if let Some(thread) = self.sighup.take() {
            let _ = thread.join();
        }
        drop(self.server.take());
        drop(self.commands.take());
        if let Some(thread) = self.supervisor.take() {
            let _ = thread.join();
        }
    }
}

/// A thread that turns SIGHUP into [`Command::Reload`] until `halt` is set.
fn spawn_sighup(commands: Sender<Command>, halt: Arc<AtomicBool>) -> Result<JoinHandle<()>> {
    let mut signals = Signals::new([SIGHUP]).context("failed to install the SIGHUP handler")?;
    std::thread::Builder::new()
        .name("sighup".into())
        .spawn(move || {
            while !halt.load(Ordering::Relaxed) {
                for _ in signals.pending() {
                    if commands.send(Command::Reload).is_err() {
                        return;
                    }
                }
                std::thread::sleep(SIGHUP_POLL);
            }
        })
        .context("failed to start the SIGHUP thread")
}
