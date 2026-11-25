//! Helpers shared by the subcommand handlers: config loading, node resolution, message loading
//! and settings assembly.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::adapters::transport::discovery::{discover_configured, DeviceIds, Mi00Path, Mi01Path};
use crate::core::session::SessionSettings;
use crate::protocol::display::DisplayGeometry;
use crate::protocol::framing::PictureSize;
use crate::utils::config::{self, Config};

#[cfg(feature = "tools")]
use super::args::StreamArgs;
use super::args::{HandshakeArgs, NodeArgs};

pub(super) fn load_config(path: Option<&PathBuf>) -> Result<Config> {
    match path {
        Some(p) => {
            config::load_from_path(p).with_context(|| format!("failed to load config {p:?}"))
        }
        None => config::load().context("failed to load config"),
    }
}

/// Resolves the node paths a command needs: explicit overrides win, discovery fills in the rest.
/// Discovery is only run when a needed node was not given.
pub(super) fn resolve_nodes(
    ids: &DeviceIds,
    nodes: &NodeArgs,
    need_mi00: bool,
    need_mi01: bool,
) -> Result<(Option<Mi00Path>, Option<Mi01Path>)> {
    let missing = (need_mi00 && nodes.mi00.is_none()) || (need_mi01 && nodes.mi01.is_none());
    let discovered = if missing {
        Some(discover_configured(ids).context("failed to resolve dock hidraw nodes")?)
    } else {
        None
    };
    let mi00 = need_mi00.then(|| match (&nodes.mi00, &discovered) {
        (Some(p), _) => Mi00Path::new(p.clone()),
        (None, Some(d)) => d.mi00.clone(),
        (None, None) => unreachable!("discovery runs whenever a needed node is missing"),
    });
    let mi01 = need_mi01.then(|| match (&nodes.mi01, &discovered) {
        (Some(p), _) => Mi01Path::new(p.clone()),
        (None, Some(d)) => d.mi01.clone(),
        (None, None) => unreachable!("discovery runs whenever a needed node is missing"),
    });
    Ok((mi00, mi01))
}

/// Messages from `--h264-file`, or the synthetic keyframe bundle plus 17-chunk inter-frame.
#[cfg(feature = "tools")]
pub(super) fn load_messages(stream: &StreamArgs) -> Result<Vec<Vec<u8>>> {
    use crate::protocol::annexb::messages_from_annexb;
    use crate::tools::validation::{synthetic_interframe, synthetic_keyframe_bundle};

    match &stream.h264_file {
        Some(path) => {
            let data = std::fs::read(path).with_context(|| format!("failed to read {path:?}"))?;
            messages_from_annexb(&data).with_context(|| format!("failed to split {path:?}"))
        }
        None => {
            eprintln!(
                "note: using synthetic filler messages (right sizes, not decodable video); \
                 pass --h264-file for a picture"
            );
            Ok(vec![synthetic_keyframe_bundle(), synthetic_interframe()])
        }
    }
}

/// Warns when a node isn't a character device (a dry-run file or pipe), since timings against
/// it say nothing about the dock.
pub(super) fn warn_if_not_char_device(path: &std::path::Path) {
    use std::os::unix::fs::FileTypeExt;
    let is_char = std::fs::metadata(path).is_ok_and(|m| m.file_type().is_char_device());
    if !is_char {
        eprintln!("note: {path:?} is not a character device; timings are not hardware timings");
    }
}

pub(super) fn geometry(config: &Config) -> Result<DisplayGeometry> {
    DisplayGeometry::from_config(&config.display).context("invalid [display] settings")
}

#[cfg(feature = "tools")]
pub(super) fn size_for(stream: &StreamArgs, config: &Config) -> Result<PictureSize> {
    let configured = geometry(config)?.size();
    Ok(PictureSize::new(
        stream.width.unwrap_or(configured.width.0),
        stream.height.unwrap_or(configured.height.0),
    ))
}

pub(super) fn device_ids(config: &Config) -> Result<DeviceIds> {
    DeviceIds::from_config(&config.device).context("invalid [device] settings")
}

pub(super) fn session_settings(
    config: &Config,
    size: PictureSize,
    args: &HandshakeArgs,
) -> SessionSettings {
    let settings = SessionSettings::new(
        size,
        Duration::from_millis(config.daemon.heartbeat_interval_ms),
    );
    if args.no_handshake {
        settings.without_handshake()
    } else {
        settings
    }
}

/// Flag set by SIGINT and SIGTERM.
pub(super) fn shutdown_flag() -> Result<Arc<AtomicBool>> {
    let flag = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(&flag))
            .context("failed to install signal handler")?;
    }
    Ok(flag)
}
