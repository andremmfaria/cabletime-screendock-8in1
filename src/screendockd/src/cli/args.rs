//! Argument groups shared by several subcommands.

use std::path::PathBuf;

use clap::{Args, ValueEnum};

use crate::adapters::encode::EncoderKind;
use crate::utils::config::CaptureBackendChoice;

#[derive(Debug, Args)]
pub(super) struct EncoderArgs {
    /// Annex-B H.264 file to replay instead of encoding with libx264 (test encoder; needs no
    /// libav). Without it the ffmpeg encoder is used, which needs the encode-ffmpeg feature.
    #[arg(long, value_name = "PATH")]
    pub(super) h264_file: Option<PathBuf>,
}

impl EncoderArgs {
    pub(super) fn kind(&self) -> EncoderKind {
        match &self.h264_file {
            Some(path) => EncoderKind::Prerecorded(path.clone()),
            None => EncoderKind::Ffmpeg,
        }
    }
}

#[derive(Debug, Args)]
pub(super) struct HandshakeArgs {
    /// Skip the dock session handshake and the MI_00 reader; only heartbeat and send video
    /// immediately. For regular files standing in for the nodes. A real dock discards video
    /// until the handshake has run.
    #[arg(long)]
    pub(super) no_handshake: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(super) enum BackendArg {
    Auto,
    X11,
    Wayland,
}

impl From<BackendArg> for CaptureBackendChoice {
    fn from(arg: BackendArg) -> Self {
        match arg {
            BackendArg::Auto => CaptureBackendChoice::Auto,
            BackendArg::X11 => CaptureBackendChoice::X11,
            BackendArg::Wayland => CaptureBackendChoice::Wayland,
        }
    }
}

/// Where the hidraw nodes come from.
#[derive(Debug, Args)]
pub(super) struct NodeArgs {
    /// MI_00 (control) node, overriding discovery. A regular file works for dry runs.
    #[arg(long, value_name = "PATH")]
    pub(super) mi00: Option<PathBuf>,
    /// MI_01 (video) node, overriding discovery. A regular file works for dry runs.
    #[arg(long, value_name = "PATH")]
    pub(super) mi01: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(super) struct StreamArgs {
    /// Picture width in pixels for the video header. Defaults to the configured display width.
    #[arg(long)]
    pub(super) width: Option<u32>,
    /// Picture height in pixels for the video header. Defaults to the configured display
    /// height.
    #[arg(long)]
    pub(super) height: Option<u32>,
    /// Annex-B H.264 file (for example from ffmpeg `-f h264`) to send instead of synthetic
    /// filler messages. Required for the panel to show a real picture.
    #[arg(long, value_name = "PATH")]
    pub(super) h264_file: Option<PathBuf>,
}
