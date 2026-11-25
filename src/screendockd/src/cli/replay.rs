//! The `replay` subcommand.

use std::time::Duration;

use anyhow::{Context, Result};
use clap::Args;

use crate::adapters::transport::hidraw::HidrawHandle;
use crate::core::session::Session;
use crate::tools::validation::{run_replay, wait_until_armed};
use crate::utils::config::Config;

use super::args::{HandshakeArgs, NodeArgs, StreamArgs};
#[allow(unused_imports)]
use super::support::load_messages;
use super::support::{
    device_ids, resolve_nodes, session_settings, size_for, warn_if_not_char_device,
};

/// Longest wait for the session to arm in `replay`, covering the ready wait, both replies and
/// the settle delay with margin.
const REPLAY_ARM_LIMIT: Duration = Duration::from_secs(30);

#[derive(Debug, Args)]
pub(super) struct ReplayArgs {
    #[command(flatten)]
    pub(super) nodes: NodeArgs,
    #[command(flatten)]
    pub(super) stream: StreamArgs,
    /// How many times to replay the message set.
    #[arg(long, default_value_t = 20)]
    pub(super) iterations: usize,
    #[command(flatten)]
    pub(super) handshake: HandshakeArgs,
}

pub(super) fn run(args: ReplayArgs, config: &Config) -> Result<()> {
    let handshake = !args.handshake.no_handshake;
    let (mi00, mi01) = resolve_nodes(&device_ids(config)?, &args.nodes, handshake, true)?;
    let mi01 = mi01.expect("MI_01 was requested");
    let messages = load_messages(&args.stream)?;
    warn_if_not_char_device(&mi01);
    let mut handle = HidrawHandle::open_mi01(&mi01).context("failed to open MI_01")?;
    let size = size_for(&args.stream, config)?;
    let session = match mi00 {
        Some(mi00) => {
            warn_if_not_char_device(&mi00);
            let mi00_handle = HidrawHandle::open_mi00(&mi00).context("failed to open MI_00")?;
            let settings = session_settings(config, size, &args.handshake);
            let session =
                Session::start(mi00_handle, settings).context("session failed to start")?;
            println!("waiting for the dock session handshake");
            wait_until_armed(&session.handle(), REPLAY_ARM_LIMIT)
                .context("handshake did not complete")?;
            Some(session)
        }
        None => None,
    };
    println!(
        "replaying {} message(s) x {} iteration(s) to {}",
        messages.len(),
        args.iterations,
        mi01.display()
    );
    let result = run_replay(&mut handle, size, &messages, args.iterations);
    if let Some(session) = session {
        session.shutdown().context("session shutdown failed")?;
    }
    println!("{}", result.context("replay failed")?);
    Ok(())
}
