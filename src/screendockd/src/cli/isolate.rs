//! The `isolate` subcommand.

use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::adapters::transport::hidraw::HidrawHandle;
use crate::ports::transport::DockTransport;
use crate::tools::validation::{run_isolate, IsolateMode, IsolateParams};
use crate::utils::config::Config;

#[allow(unused_imports)]
use super::support::load_messages;
use super::support::{
    device_ids, resolve_nodes, session_settings, size_for, warn_if_not_char_device,
};
use super::types::IsolateArgs;

pub(super) fn run(args: IsolateArgs, config: &Config) -> Result<()> {
    let mode = IsolateMode::from(args.mode);
    let (need_mi00, need_mi01) = match mode {
        IsolateMode::Heartbeat => (true, false),
        IsolateMode::Video => (false, true),
        IsolateMode::Both => (true, true),
    };
    if args.duration_secs == 0 {
        bail!("--duration-secs must be greater than zero");
    }
    let (mi00, mi01) = resolve_nodes(&device_ids(config)?, &args.nodes, need_mi00, need_mi01)?;
    let gop = if need_mi01 {
        load_messages(&args.stream)?
    } else {
        Vec::new()
    };
    for path in mi00
        .iter()
        .map(|p| p.as_path())
        .chain(mi01.iter().map(|p| p.as_path()))
    {
        warn_if_not_char_device(path);
    }
    let mut mi00_handle = mi00
        .as_ref()
        .map(HidrawHandle::open_mi00)
        .transpose()
        .context("failed to open MI_00")?;
    let mut mi01_handle = mi01
        .as_ref()
        .map(HidrawHandle::open_mi01)
        .transpose()
        .context("failed to open MI_01")?;
    let size = size_for(&args.stream, config)?;
    if mode == IsolateMode::Video {
        println!("note: video-only runs have no MI_00 session, so a dock that has not been handshaken discards the video");
    }
    let params = IsolateParams {
        mode,
        duration: Duration::from_secs(args.duration_secs),
        heartbeat_interval: Duration::from_millis(config.daemon.heartbeat_interval_ms),
        size,
        session: (!args.handshake.no_handshake)
            .then(|| session_settings(config, size, &args.handshake)),
        fps: args.fps,
        gop,
    };
    println!(
        "running {:?} for {}s (watch the panel and record what it shows)",
        mode, args.duration_secs
    );
    let report = run_isolate(
        &params,
        mi00_handle.as_mut().map(|h| h as &mut dyn DockTransport),
        mi01_handle.as_mut().map(|h| h as &mut dyn DockTransport),
    )
    .context("isolation run failed")?;
    println!("{report}");
    Ok(())
}
