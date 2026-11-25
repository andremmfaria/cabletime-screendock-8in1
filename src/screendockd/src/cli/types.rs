//! Types of the command line: argument structs and enums and the parsed command.

use std::path::PathBuf;
#[cfg(feature = "tools")]
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[cfg(feature = "tools")]
use crate::tools::validation::IsolateMode;
use crate::utils::config::CaptureBackendChoice;

/// Independent, unofficial Linux userspace driver for the CABLETIME ScreenDock (CB09W).
#[derive(Debug, Parser)]
#[command(name = "screendockd", version, arg_required_else_help = true)]
pub(super) struct Cli {
    /// Config file path. Defaults to the resolution order in docs/configuration.md.
    #[arg(long, global = true, value_name = "PATH")]
    pub(super) config: Option<PathBuf>,

    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Debug, Subcommand)]
pub(super) enum Command {
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

#[derive(Debug, Args)]
pub(super) struct EncoderArgs {
    /// Annex-B H.264 file to replay instead of encoding with libx264 (test encoder; needs no
    /// libav). Without it the ffmpeg encoder is used, which needs the encode-ffmpeg feature.
    #[arg(long, value_name = "PATH")]
    pub(super) h264_file: Option<PathBuf>,
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

#[cfg(feature = "tools")]
#[derive(Debug, Args)]
pub(super) struct BenchArgs {
    #[command(flatten)]
    pub(super) encoder: EncoderArgs,
    /// Capture backend, overriding the configured `capture.backend`.
    #[arg(long, value_enum)]
    pub(super) backend: Option<BackendArg>,
    /// Measurement duration in seconds.
    #[arg(long, default_value_t = 30)]
    pub(super) duration_secs: u64,
}

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

#[cfg(feature = "tools")]
#[derive(Debug, Args)]
pub(super) struct IsolateArgs {
    #[command(flatten)]
    pub(super) nodes: NodeArgs,
    #[command(flatten)]
    pub(super) stream: StreamArgs,
    /// Which condition to run.
    #[arg(long, value_enum)]
    pub(super) mode: ModeArg,
    /// How long to run, in seconds.
    #[arg(long, default_value_t = 30)]
    pub(super) duration_secs: u64,
    /// Video frame rate.
    #[arg(long, default_value_t = 25)]
    pub(super) fps: u32,
    #[command(flatten)]
    pub(super) handshake: HandshakeArgs,
}

#[cfg(feature = "tools")]
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(super) enum ModeArg {
    Heartbeat,
    Video,
    Both,
}

#[cfg(feature = "tools")]
impl From<ModeArg> for IsolateMode {
    fn from(mode: ModeArg) -> Self {
        match mode {
            ModeArg::Heartbeat => IsolateMode::Heartbeat,
            ModeArg::Video => IsolateMode::Video,
            ModeArg::Both => IsolateMode::Both,
        }
    }
}

#[cfg(feature = "tools")]
/// Longest wait for the session to arm in `replay`, covering the ready wait, both replies and
/// the settle delay with margin.
pub(super) const REPLAY_ARM_LIMIT: Duration = Duration::from_secs(30);

#[cfg(feature = "tools")]
#[derive(Debug, Args)]
pub(super) struct ReplayArgs {
    #[command(flatten)]
    pub(super) nodes: NodeArgs,
    #[command(flatten)]
    pub(super) stream: StreamArgs,
    /// How many times to replay the message set.
    #[arg(long, default_value_t = 20)]
    pub(super) iterations: usize,
    #[command(flatten)]
    pub(super) handshake: HandshakeArgs,
}

#[derive(Debug, Args)]
pub(super) struct RunArgs {
    #[command(flatten)]
    pub(super) nodes: NodeArgs,
    #[command(flatten)]
    pub(super) encoder: EncoderArgs,
    /// Capture backend, overriding the configured `capture.backend`.
    #[arg(long, value_enum)]
    pub(super) backend: Option<BackendArg>,
    /// Stop after this many frames (dry runs and tests). Default: run until SIGINT/SIGTERM.
    #[arg(long)]
    pub(super) max_frames: Option<u64>,
    #[command(flatten)]
    pub(super) handshake: HandshakeArgs,
}
