//! The `replay` subcommand.

use anyhow::{Context, Result};

use crate::adapters::transport::hidraw::HidrawHandle;
use crate::core::session::Session;
use crate::tools::validation::{run_replay, wait_until_armed};
use crate::utils::config::Config;

#[allow(unused_imports)]
use super::support::load_messages;
use super::support::{
    device_ids, resolve_nodes, session_settings, size_for, warn_if_not_char_device,
};
use super::types::ReplayArgs;
use super::types::REPLAY_ARM_LIMIT;

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
