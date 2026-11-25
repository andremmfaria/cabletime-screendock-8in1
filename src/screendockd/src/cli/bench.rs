//! The `bench` subcommand.

use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::adapters::capture::create_backend;
use crate::adapters::encode::create_encoder;
use crate::ports::encode::EncoderSettings;
use crate::tools::bench::run_bench;
use crate::utils::config::Config;

#[allow(unused_imports)]
use super::support::load_messages;
use super::support::{geometry, shutdown_flag};
use super::types::BenchArgs;

pub(super) fn run(args: BenchArgs, config: &Config) -> Result<()> {
    if args.duration_secs == 0 {
        bail!("--duration-secs must be greater than zero");
    }
    let geometry = geometry(config)?;
    let choice = args.backend.map_or(config.capture.backend, Into::into);
    let mut capture = create_backend(config, choice).context("failed to start capture")?;
    let settings = EncoderSettings::from_config(config, geometry);
    let mut encoder =
        create_encoder(&args.encoder.kind(), settings).context("failed to start encoder")?;
    let stop = shutdown_flag()?;
    println!(
        "benchmarking {} + {} at {}x{} for {}s",
        capture.name(),
        encoder.name(),
        geometry.width().0,
        geometry.height().0,
        args.duration_secs
    );
    let report = run_bench(
        capture.as_mut(),
        encoder.as_mut(),
        config.encode.keyframe_interval,
        Duration::from_secs(args.duration_secs),
        &stop,
    )
    .context("benchmark failed")?;
    println!("{report}");
    Ok(())
}
