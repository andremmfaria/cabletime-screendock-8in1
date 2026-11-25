//! Command line: argument definitions, the subcommand handlers and the composition root that
//! builds adapters and hands them to the core and the app.
//!
//! Subcommands:
//! - `discover`: resolve and print the dock's MI_00/MI_01 hidraw nodes.
//! - `replay`: write-timing test against MI_01.
//! - `isolate`: heartbeat/video/both isolation test.
//! - `capture`: grab N frames from the desktop and write them as PNG files.
//! - `run`: the daemon, heartbeat plus capture, encode and write to the dock.
//! - `bench`: capture plus encode CPU measurement, no dock needed.
//!
//! `docs/hardware-validation.md` describes how `replay`, `isolate` and `run` are used on the
//! dock; `docs/daemon.md` describes the daemon itself.
//!
//! This is the application boundary: it uses `anyhow` with context, while the library modules return typed
//! errors. Any failure here exits nonzero.

mod args;
#[cfg(feature = "tools")]
mod bench;
mod capture;
mod discover;
#[cfg(feature = "tools")]
mod isolate;
#[cfg(feature = "tools")]
mod replay;
mod run;
mod support;
pub mod types;

use anyhow::Result;
use clap::Parser;

use crate::utils::logging::init_logging;

use self::types::{Cli, Command};

/// Parses the command line, initializes logging and runs the chosen subcommand.
pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let config = support::load_config(cli.config.as_ref())?;
    // A second init in the same process is a caller bug but not fatal for a CLI.
    let _ = init_logging(&config);

    match cli.command {
        Command::Discover => discover::run(&config),
        #[cfg(feature = "tools")]
        Command::Replay(args) => replay::run(args, &config),
        #[cfg(feature = "tools")]
        Command::Isolate(args) => isolate::run(args, &config),
        Command::Capture(args) => capture::run(args, &config),
        Command::Run(args) => run::run(args, &config),
        #[cfg(feature = "tools")]
        Command::Bench(args) => bench::run(args, &config),
    }
}
