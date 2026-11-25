//! The `run` subcommand.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;

use anyhow::{Context, Result};

use crate::adapters::capture::create_backend;
use crate::adapters::control::{socket_path, ControlServer};
use crate::adapters::encode::create_encoder;
use crate::adapters::transport::hidraw::HidrawHandle;
use crate::app::daemon::{self, DaemonParams, DEFAULT_MAX_CONSECUTIVE_ERRORS};
use crate::core::reconfig::keys::answer_read;
use crate::core::reconfig::LiveSettings;
use crate::ports::control::Command;
use crate::ports::encode::EncoderSettings;
use crate::protocol::control::ControlResponse;
use crate::utils::config::Config;

use super::support::{
    device_ids, geometry, resolve_nodes, session_settings, shutdown_flag, warn_if_not_char_device,
};
use super::types::RunArgs;

pub(super) fn run(args: RunArgs, config: &Config) -> Result<()> {
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
    let control = start_control(config, live)?;
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
    };
    let result = daemon::run(
        mi00_handle,
        mi01_handle,
        capture.as_mut(),
        encoder.as_mut(),
        &params,
        &stop,
    );
    if let Some((server, responder)) = control {
        drop(server);
        let _ = responder.join();
    }
    let report = result.context("daemon failed")?;
    println!("{report:?}");
    Ok(())
}

/// Starts the control socket and the thread that answers its requests. Returns `None` when the
/// control socket is disabled or no socket path is available.
fn start_control(
    config: &Config,
    live: LiveSettings,
) -> Result<Option<(ControlServer, JoinHandle<()>)>> {
    if !config.control.enabled {
        return Ok(None);
    }
    let Some(path) = socket_path(&config.control) else {
        tracing::warn!("control socket disabled: no control.socket and $XDG_RUNTIME_DIR is unset");
        return Ok(None);
    };
    let (commands, inbox) = mpsc::channel();
    let server = ControlServer::start(&path, commands)
        .with_context(|| format!("failed to start the control socket at {path:?}"))?;
    let responder = std::thread::Builder::new()
        .name("control-responder".into())
        .spawn(move || answer_requests(&live, &inbox))
        .context("failed to start the control responder")?;
    Ok(Some((server, responder)))
}

/// Answers control requests from fixed settings until the server is gone. Requests that change
/// state are declined.
fn answer_requests(live: &LiveSettings, inbox: &Receiver<Command>) {
    let mut extra = BTreeMap::new();
    extra.insert("version".to_string(), env!("CARGO_PKG_VERSION").to_string());
    extra.insert("pid".to_string(), std::process::id().to_string());
    while let Ok(command) = inbox.recv() {
        if let Command::Request { request, reply } = command {
            let response = answer_read(live, &request, extra.clone())
                .unwrap_or_else(|| ControlResponse::rejected("changing settings is not available"));
            let _ = reply.send(response);
        }
    }
}
