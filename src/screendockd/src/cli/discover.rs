//! The `discover` subcommand.

use anyhow::{Context, Result};

use crate::adapters::transport::discovery::discover_configured;
use crate::utils::config::Config;

use super::support::device_ids;

pub(super) fn run(config: &Config) -> Result<()> {
    let paths =
        discover_configured(&device_ids(config)?).context("failed to resolve dock hidraw nodes")?;
    println!("MI_00 (control):  {}", paths.mi00.display());
    println!("MI_01 (video):    {}", paths.mi01.display());
    Ok(())
}
