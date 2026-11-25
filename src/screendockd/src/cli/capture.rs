//! The `capture` subcommand.

use anyhow::{bail, Context, Result};

use super::types::CaptureArgs;
use crate::adapters::capture::{create_backend, detect_session};
use crate::utils::config::Config;

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
