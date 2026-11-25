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

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::utils::logging::init_logging;

use self::capture::CaptureArgs;
use self::run::RunArgs;
#[cfg(feature = "tools")]
use self::{bench::BenchArgs, isolate::IsolateArgs, replay::ReplayArgs};

/// Independent, unofficial Linux userspace driver for the CABLETIME ScreenDock (CB09W).
#[derive(Debug, Parser)]
#[command(name = "screendockd", version, arg_required_else_help = true)]
struct Cli {
    /// Config file path. Defaults to the resolution order in docs/configuration.md.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Resolve and print the dock's MI_00 and MI_01 hidraw nodes.
    Discover,
    /// Replay a keyframe plus a 17-chunk message back-to-back on MI_01 and print
    /// per-chunk write timing.
    #[cfg(feature = "tools")]
    Replay(ReplayArgs),
    /// Run heartbeat-only, video-only or both for a fixed duration.
    #[cfg(feature = "tools")]
    Isolate(IsolateArgs),
    /// Capture frames from the desktop and write them as PNG files.
    Capture(CaptureArgs),
    /// Run the daemon: heartbeat on MI_00, capture, encode and write video to MI_01.
    Run(RunArgs),
    /// Measure CPU time of capture plus encode over a fixed duration (no dock needed).
    #[cfg(feature = "tools")]
    Bench(BenchArgs),
}

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
