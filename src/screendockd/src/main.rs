//! `screendockd` binary entry point.
//!
//! Subcommands:
//! - `discover`: resolve and print the dock's MI_00/MI_01 hidraw nodes.
//! - `replay`: Milestone 2 write-timing test against MI_01.
//! - `isolate`: Milestone 3 heartbeat/video/both isolation test.
//! - `capture`: grab N frames from the desktop and write them as PNG files.
//! - `run`: the daemon, heartbeat plus capture, encode and write to the dock.
//! - `bench`: capture plus encode CPU measurement, no dock needed.
//!
//! `docs/hardware-validation.md` describes how `replay`, `isolate` and `run` are used on the
//! dock; `docs/daemon.md` describes the daemon itself.
//!
//! Per the Design Patterns section of `docs/implementation-plan.md` (pattern 7), this is the
//! application boundary: it uses `anyhow` with context, while the library modules return typed
//! errors. Any failure here exits nonzero.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

use screendockd::adapters::capture::{create_backend, detect_session};
use screendockd::adapters::encode::{create_encoder, EncoderKind};
use screendockd::adapters::transport::discovery::{
    discover_configured, DeviceIds, Mi00Path, Mi01Path,
};
use screendockd::adapters::transport::hidraw::HidrawHandle;
use screendockd::app::daemon::{self, DaemonParams, DEFAULT_MAX_CONSECUTIVE_ERRORS};
use screendockd::config::{self, CaptureBackendChoice, Config};
#[cfg(feature = "tools")]
use screendockd::core::session::Session;
use screendockd::core::session::SessionSettings;
use screendockd::ports::encode::EncoderSettings;
#[cfg(feature = "tools")]
use screendockd::ports::transport::DockTransport;
use screendockd::protocol::annexb::messages_from_annexb;
use screendockd::protocol::display::DisplayGeometry;
use screendockd::protocol::framing::PictureSize;
use screendockd::telemetry::logging::init_logging;
#[cfg(feature = "tools")]
use screendockd::tools::bench::run_bench;
#[cfg(feature = "tools")]
use screendockd::tools::validation::{
    run_isolate, run_replay, synthetic_interframe, synthetic_keyframe_bundle, wait_until_armed,
    IsolateMode, IsolateParams,
};

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
    /// Milestone 2: replay a keyframe plus a 17-chunk message back-to-back on MI_01 and print
    /// per-chunk write timing.
    #[cfg(feature = "tools")]
    Replay(ReplayArgs),
    /// Milestone 3: run heartbeat-only, video-only or both for a fixed duration.
    #[cfg(feature = "tools")]
    Isolate(IsolateArgs),
    /// Milestone 4: capture frames from the desktop and write them as PNG files.
    Capture(CaptureArgs),
    /// Run the daemon: heartbeat on MI_00, capture, encode and write video to MI_01.
    Run(RunArgs),
    /// Measure CPU time of capture plus encode over a fixed duration (no dock needed).
    #[cfg(feature = "tools")]
    Bench(BenchArgs),
}

#[derive(Debug, Args)]
struct EncoderArgs {
    /// Annex-B H.264 file to replay instead of encoding with libx264 (test encoder; needs no
    /// libav). Without it the ffmpeg encoder is used, which needs the encode-ffmpeg feature.
    #[arg(long, value_name = "PATH")]
    h264_file: Option<PathBuf>,
}

impl EncoderArgs {
    fn kind(&self) -> EncoderKind {
        match &self.h264_file {
            Some(path) => EncoderKind::Prerecorded(path.clone()),
            None => EncoderKind::Ffmpeg,
        }
    }
}

#[derive(Debug, Args)]
struct RunArgs {
    #[command(flatten)]
    nodes: NodeArgs,
    #[command(flatten)]
    encoder: EncoderArgs,
    /// Capture backend, overriding the configured `capture.backend`.
    #[arg(long, value_enum)]
    backend: Option<BackendArg>,
    /// Stop after this many frames (dry runs and tests). Default: run until SIGINT/SIGTERM.
    #[arg(long)]
    max_frames: Option<u64>,
    #[command(flatten)]
    handshake: HandshakeArgs,
}

#[derive(Debug, Args)]
struct HandshakeArgs {
    /// Skip the dock session handshake and the MI_00 reader; only heartbeat and send video
    /// immediately. For regular files standing in for the nodes. A real dock discards video
    /// until the handshake has run.
    #[arg(long)]
    no_handshake: bool,
}

#[cfg(feature = "tools")]
#[derive(Debug, Args)]
struct BenchArgs {
    #[command(flatten)]
    encoder: EncoderArgs,
    /// Capture backend, overriding the configured `capture.backend`.
    #[arg(long, value_enum)]
    backend: Option<BackendArg>,
    /// Measurement duration in seconds.
    #[arg(long, default_value_t = 30)]
    duration_secs: u64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BackendArg {
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

#[derive(Debug, Args)]
struct CaptureArgs {
    /// Number of frames to capture.
    #[arg(long, default_value_t = 5)]
    frames: u32,
    /// Directory the `frame-NNNN.png` files are written to (created if missing).
    #[arg(long, value_name = "DIR", default_value = ".")]
    out_dir: PathBuf,
    /// Backend, overriding the configured `capture.backend`.
    #[arg(long, value_enum)]
    backend: Option<BackendArg>,
}

/// Where the hidraw nodes come from.
#[derive(Debug, Args)]
struct NodeArgs {
    /// MI_00 (control) node, overriding discovery. A regular file works for dry runs.
    #[arg(long, value_name = "PATH")]
    mi00: Option<PathBuf>,
    /// MI_01 (video) node, overriding discovery. A regular file works for dry runs.
    #[arg(long, value_name = "PATH")]
    mi01: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct StreamArgs {
    /// Picture width in pixels for the video header. Defaults to the configured display width.
    #[arg(long)]
    width: Option<u32>,
    /// Picture height in pixels for the video header. Defaults to the configured display
    /// height.
    #[arg(long)]
    height: Option<u32>,
    /// Annex-B H.264 file (for example from ffmpeg `-f h264`) to send instead of synthetic
    /// filler messages. Required for the panel to show a real picture.
    #[arg(long, value_name = "PATH")]
    h264_file: Option<PathBuf>,
}

#[cfg(feature = "tools")]
#[derive(Debug, Args)]
struct ReplayArgs {
    #[command(flatten)]
    nodes: NodeArgs,
    #[command(flatten)]
    stream: StreamArgs,
    /// How many times to replay the message set.
    #[arg(long, default_value_t = 20)]
    iterations: usize,
    #[command(flatten)]
    handshake: HandshakeArgs,
}

#[cfg(feature = "tools")]
#[derive(Debug, Clone, Copy, ValueEnum)]
enum ModeArg {
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
#[derive(Debug, Args)]
struct IsolateArgs {
    #[command(flatten)]
    nodes: NodeArgs,
    #[command(flatten)]
    stream: StreamArgs,
    /// Which condition to run.
    #[arg(long, value_enum)]
    mode: ModeArg,
    /// How long to run, in seconds.
    #[arg(long, default_value_t = 30)]
    duration_secs: u64,
    /// Video frame rate.
    #[arg(long, default_value_t = 25)]
    fps: u32,
    #[command(flatten)]
    handshake: HandshakeArgs,
}

fn load_config(path: Option<&PathBuf>) -> Result<Config> {
    match path {
        Some(p) => {
            config::load_from_path(p).with_context(|| format!("failed to load config {p:?}"))
        }
        None => config::load().context("failed to load config"),
    }
}

/// Resolves the node paths a command needs: explicit overrides win, discovery fills in the rest.
/// Discovery is only run when a needed node was not given.
fn resolve_nodes(
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
fn load_messages(stream: &StreamArgs) -> Result<Vec<Vec<u8>>> {
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
fn warn_if_not_char_device(path: &std::path::Path) {
    use std::os::unix::fs::FileTypeExt;
    let is_char = std::fs::metadata(path).is_ok_and(|m| m.file_type().is_char_device());
    if !is_char {
        eprintln!("note: {path:?} is not a character device; timings are not hardware timings");
    }
}

fn geometry(config: &Config) -> Result<DisplayGeometry> {
    DisplayGeometry::from_config(&config.display).context("invalid [display] settings")
}

fn size_for(stream: &StreamArgs, config: &Config) -> Result<PictureSize> {
    let configured = geometry(config)?.size();
    Ok(PictureSize::new(
        stream.width.unwrap_or(configured.width.0),
        stream.height.unwrap_or(configured.height.0),
    ))
}

fn device_ids(config: &Config) -> Result<DeviceIds> {
    DeviceIds::from_config(&config.device).context("invalid [device] settings")
}

/// Longest wait for the session to arm in `replay`, covering the ready wait, both replies and
/// the settle delay with margin.
const REPLAY_ARM_LIMIT: Duration = Duration::from_secs(30);

fn session_settings(config: &Config, size: PictureSize, args: &HandshakeArgs) -> SessionSettings {
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
fn shutdown_flag() -> Result<Arc<AtomicBool>> {
    let flag = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(&flag))
            .context("failed to install signal handler")?;
    }
    Ok(flag)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = load_config(cli.config.as_ref())?;
    // A second init in the same process is a caller bug but not fatal for a CLI.
    let _ = init_logging(&config);

    match cli.command {
        Command::Discover => {
            let paths = discover_configured(&device_ids(&config)?)
                .context("failed to resolve dock hidraw nodes")?;
            println!("MI_00 (control):  {}", paths.mi00.display());
            println!("MI_01 (video):    {}", paths.mi01.display());
        }
        #[cfg(feature = "tools")]
        Command::Replay(args) => {
            let handshake = !args.handshake.no_handshake;
            let (mi00, mi01) = resolve_nodes(&device_ids(&config)?, &args.nodes, handshake, true)?;
            let mi01 = mi01.expect("MI_01 was requested");
            let messages = load_messages(&args.stream)?;
            warn_if_not_char_device(&mi01);
            let mut handle = HidrawHandle::open_mi01(&mi01).context("failed to open MI_01")?;
            let size = size_for(&args.stream, &config)?;
            let session = match mi00 {
                Some(mi00) => {
                    warn_if_not_char_device(&mi00);
                    let mi00_handle =
                        HidrawHandle::open_mi00(&mi00).context("failed to open MI_00")?;
                    let settings = session_settings(&config, size, &args.handshake);
                    let session =
                        Session::start(mi00_handle, settings).context("session failed to start")?;
                    println!("waiting for the dock session handshake");
                    wait_until_armed(&session.handle(), REPLAY_ARM_LIMIT)
                        .context("handshake did not complete")?;
                    Some(session)
                }
                None => None,
            };
            println!(
                "replaying {} message(s) x {} iteration(s) to {}",
                messages.len(),
                args.iterations,
                mi01.display()
            );
            let result = run_replay(&mut handle, size, &messages, args.iterations);
            if let Some(session) = session {
                session.shutdown().context("session shutdown failed")?;
            }
            println!("{}", result.context("replay failed")?);
        }
        #[cfg(feature = "tools")]
        Command::Isolate(args) => {
            let mode = IsolateMode::from(args.mode);
            let (need_mi00, need_mi01) = match mode {
                IsolateMode::Heartbeat => (true, false),
                IsolateMode::Video => (false, true),
                IsolateMode::Both => (true, true),
            };
            if args.duration_secs == 0 {
                bail!("--duration-secs must be greater than zero");
            }
            let (mi00, mi01) =
                resolve_nodes(&device_ids(&config)?, &args.nodes, need_mi00, need_mi01)?;
            let gop = if need_mi01 {
                load_messages(&args.stream)?
            } else {
                Vec::new()
            };
            for path in mi00
                .iter()
                .map(|p| p.as_path())
                .chain(mi01.iter().map(|p| p.as_path()))
            {
                warn_if_not_char_device(path);
            }
            let mut mi00_handle = mi00
                .as_ref()
                .map(HidrawHandle::open_mi00)
                .transpose()
                .context("failed to open MI_00")?;
            let mut mi01_handle = mi01
                .as_ref()
                .map(HidrawHandle::open_mi01)
                .transpose()
                .context("failed to open MI_01")?;
            let size = size_for(&args.stream, &config)?;
            if mode == IsolateMode::Video {
                println!("note: video-only runs have no MI_00 session, so a dock that has not been handshaken discards the video");
            }
            let params = IsolateParams {
                mode,
                duration: Duration::from_secs(args.duration_secs),
                heartbeat_interval: Duration::from_millis(config.daemon.heartbeat_interval_ms),
                size,
                session: (!args.handshake.no_handshake)
                    .then(|| session_settings(&config, size, &args.handshake)),
                fps: args.fps,
                gop,
            };
            println!(
                "running {:?} for {}s (watch the panel and record what it shows)",
                mode, args.duration_secs
            );
            let report = run_isolate(
                &params,
                mi00_handle.as_mut().map(|h| h as &mut dyn DockTransport),
                mi01_handle.as_mut().map(|h| h as &mut dyn DockTransport),
            )
            .context("isolation run failed")?;
            println!("{report}");
        }
        Command::Capture(args) => {
            if args.frames == 0 {
                bail!("--frames must be greater than zero");
            }
            std::fs::create_dir_all(&args.out_dir)
                .with_context(|| format!("failed to create {:?}", args.out_dir))?;
            let choice = args.backend.map_or(config.capture.backend, Into::into);
            println!("detected session: {:?}", detect_session());
            let mut backend = create_backend(&config, choice).context("failed to start capture")?;
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
        }
        Command::Run(args) => {
            let geometry = geometry(&config)?;
            let (mi00, mi01) = resolve_nodes(&device_ids(&config)?, &args.nodes, true, true)?;
            let mi00 = mi00.expect("MI_00 was requested");
            let mi01 = mi01.expect("MI_01 was requested");
            warn_if_not_char_device(&mi00);
            warn_if_not_char_device(&mi01);
            let mi00_handle = HidrawHandle::open_mi00(&mi00).context("failed to open MI_00")?;
            let mi01_handle = HidrawHandle::open_mi01(&mi01).context("failed to open MI_01")?;
            let choice = args.backend.map_or(config.capture.backend, Into::into);
            let mut capture = create_backend(&config, choice).context("failed to start capture")?;
            let settings = EncoderSettings::from_config(&config, geometry);
            let mut encoder = create_encoder(&args.encoder.kind(), settings)
                .context("failed to start encoder")?;
            let stop = shutdown_flag()?;
            tracing::info!(
                capture = capture.name(),
                encoder = encoder.name(),
                width = geometry.width().0,
                height = geometry.height().0,
                "starting daemon"
            );
            let params = DaemonParams {
                geometry,
                session: session_settings(&config, geometry.size(), &args.handshake),
                max_frames: args.max_frames,
                max_consecutive_errors: DEFAULT_MAX_CONSECUTIVE_ERRORS,
            };
            let report = daemon::run(
                mi00_handle,
                mi01_handle,
                capture.as_mut(),
                encoder.as_mut(),
                &params,
                &stop,
            )
            .context("daemon failed")?;
            println!("{report:?}");
        }
        #[cfg(feature = "tools")]
        Command::Bench(args) => {
            if args.duration_secs == 0 {
                bail!("--duration-secs must be greater than zero");
            }
            let geometry = geometry(&config)?;
            let choice = args.backend.map_or(config.capture.backend, Into::into);
            let mut capture = create_backend(&config, choice).context("failed to start capture")?;
            let settings = EncoderSettings::from_config(&config, geometry);
            let mut encoder = create_encoder(&args.encoder.kind(), settings)
                .context("failed to start encoder")?;
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
                Duration::from_secs(args.duration_secs),
                &stop,
            )
            .context("benchmark failed")?;
            println!("{report}");
        }
    }
    Ok(())
}
