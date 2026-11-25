//! The `capture` subcommand.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Args;

use crate::adapters::capture::{create_backend, detect_session};
use crate::utils::config::Config;

use super::args::BackendArg;

#[derive(Debug, Args)]
pub(super) struct CaptureArgs {
    /// Number of frames to capture.
    #[arg(long, default_value_t = 5)]
    pub(super) frames: u32,
    /// Directory the `frame-NNNN.png` files are written to (created if missing).
    #[arg(long, value_name = "DIR", default_value = ".")]
    pub(super) out_dir: PathBuf,
    /// Backend, overriding the configured `capture.backend`.
    #[arg(long, value_enum)]
    pub(super) backend: Option<BackendArg>,
}

pub(super) fn run(args: CaptureArgs, config: &Config) -> Result<()> {
    if args.frames == 0 {
        bail!("--frames must be greater than zero");
    }
    std::fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("failed to create {:?}", args.out_dir))?;
    let choice = args.backend.map_or(config.capture.backend, Into::into);
    println!("detected session: {:?}", detect_session());
    let mut backend = create_backend(config, choice).context("failed to start capture")?;
    println!(
        "backend: {} (panel target {}x{})",
        backend.name(),
        config.display.width,
        config.display.height
    );
    for i in 0..args.frames {
        let frame = backend.next_frame().context("capture failed")?;
        let path = args.out_dir.join(format!("frame-{i:04}.png"));
        frame
            .write_png(&path)
            .with_context(|| format!("failed to write {path:?}"))?;
        println!(
            "{}: {}x{} {:?}",
            path.display(),
            frame.width,
            frame.height,
            frame.format
        );
    }
    Ok(())
}
