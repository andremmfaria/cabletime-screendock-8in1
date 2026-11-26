//! The `vdisplay` subcommand: `probe` shows what the desktop offers, `cleanup` undoes the
//! changes of a daemon that was killed.

use anyhow::{Context, Result};

use crate::adapters::display::{recover, RecoveryOutcome, SystemDisplayProvider};
use crate::core::display_policy::{decide, place_for_backend, place_monitor};
use crate::core::reconfig::LiveSettings;
use crate::core::types::DisplayDecision;
use crate::ports::display::DisplayProvider;
use crate::utils::config::Config;
use crate::utils::types::{DisplayMode, VirtualDisplayConfig};

use super::types::{VdisplayAction, VdisplayArgs};

pub(super) fn run(args: VdisplayArgs, config: &Config) -> Result<()> {
    match args.action {
        VdisplayAction::Probe => probe(config),
        VdisplayAction::Cleanup => cleanup(),
    }
}

fn probe(config: &Config) -> Result<()> {
    let provider = SystemDisplayProvider::new();
    let facts = provider
        .facts()
        .context("failed to read the display facts")?;
    println!("server: {:?}", facts.server);
    println!("desktop: {}", facts.desktop);
    if let Some((major, minor)) = facts.randr_version {
        println!("randr: {major}.{minor}");
    }
    for output in &facts.outputs {
        let rect = output.rect;
        println!(
            "output: {} {}x{}+{}+{}{}",
            output.name,
            rect.width,
            rect.height,
            rect.x,
            rect.y,
            if output.primary { " primary" } else { "" }
        );
    }
    match facts.spare_output {
        Some(id) => println!("spare output: {}", id.0),
        None => println!("spare output: none"),
    }
    // Probe what extend mode would do, whatever mode the config has.
    let extend = VirtualDisplayConfig {
        mode: DisplayMode::Extend,
        ..config.virtual_display.clone()
    };
    let live = LiveSettings::try_from(config).context("invalid settings")?;
    let mut placed = place_monitor(&facts, &live);
    match decide(&facts, &extend) {
        Ok(DisplayDecision::Extend(backend)) => {
            println!("extend: {backend:?}");
            placed = place_for_backend(&facts, &live, backend);
        }
        Ok(DisplayDecision::Mirror { reason }) => println!("extend: unavailable ({reason})"),
        Err(e) => println!("extend: unavailable ({e})"),
    }
    match placed {
        Ok(rect) => println!(
            "monitor: {}x{}+{}+{}",
            rect.width, rect.height, rect.x, rect.y
        ),
        Err(e) => println!("monitor: no placement ({e})"),
    }
    println!("configured mode: {:?}", config.virtual_display.mode);
    Ok(())
}

fn cleanup() -> Result<()> {
    let provider = SystemDisplayProvider::new();
    let Some(store) = provider.recovery_store() else {
        println!("nothing to clean up: $XDG_RUNTIME_DIR is not set");
        return Ok(());
    };
    match recover(&store).context("cleanup failed")? {
        RecoveryOutcome::NothingToDo => println!("nothing to clean up"),
        RecoveryOutcome::Restored => println!("restored the display layout"),
        RecoveryOutcome::StillRunning(pid) => {
            println!("daemon {pid} is still running; nothing was changed")
        }
    }
    Ok(())
}
