//! The `run` subcommand.

use anyhow::{Context, Result};
use clap::Args;

use crate::adapters::capture::create_backend;
use crate::adapters::encode::create_encoder;
use crate::adapters::transport::hidraw::HidrawHandle;
use crate::app::daemon::{self, DaemonParams, DEFAULT_MAX_CONSECUTIVE_ERRORS};
use crate::ports::encode::EncoderSettings;
use crate::utils::config::Config;

use super::args::{BackendArg, EncoderArgs, HandshakeArgs, NodeArgs};
use super::support::{
    device_ids, geometry, resolve_nodes, session_settings, shutdown_flag, warn_if_not_char_device,
};

#[derive(Debug, Args)]
pub(super) struct RunArgs {
    #[command(flatten)]
    pub(super) nodes: NodeArgs,
    #[command(flatten)]
    pub(super) encoder: EncoderArgs,
    /// Capture backend, overriding the configured `capture.backend`.
    #[arg(long, value_enum)]
    pub(super) backend: Option<BackendArg>,
    /// Stop after this many frames (dry runs and tests). Default: run until SIGINT/SIGTERM.
    #[arg(long)]
    pub(super) max_frames: Option<u64>,
    #[command(flatten)]
    pub(super) handshake: HandshakeArgs,
}

pub(super) fn run(args: RunArgs, config: &Config) -> Result<()> {
    let geometry = geometry(config)?;
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
    let report = daemon::run(
        mi00_handle,
        mi01_handle,
        capture.as_mut(),
        encoder.as_mut(),
        &params,
        &stop,
    )
    .context("daemon failed")?;
    println!("{report:?}");
    Ok(())
}
