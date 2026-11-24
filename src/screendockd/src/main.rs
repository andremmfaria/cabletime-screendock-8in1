//! `screendockd` binary entry point.
//!
//! Subcommands:
//! - `discover`: resolve and print the dock's MI_00/MI_01 hidraw nodes.
//! - `replay`: Milestone 2 write-timing test against MI_01.
//! - `isolate`: Milestone 3 heartbeat/video/both isolation test.
//! - `capture`: grab N frames from the desktop and write them as PNG files.
//!
//! The capture/encode daemon is not wired yet. `docs/hardware-validation.md` describes how the
//! `replay` and `isolate` subcommands are used on the dock.
//!
//! Per the Design Patterns section of `docs/implementation-plan.md` (pattern 7), this is the
//! application boundary: it uses `anyhow` with context, while the library modules return typed
//! errors. Any failure here exits nonzero.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

use screendockd::annexb::messages_from_annexb;
use screendockd::capture::{create_backend, detect_session};
use screendockd::discovery::{discover, Mi00Path, Mi01Path};
use screendockd::framing::PixelWidth;
use screendockd::hidraw_io::HidrawHandle;
use screendockd::utils::config::{self, CaptureBackendChoice, Config};
use screendockd::utils::logging::init_logging;
use screendockd::validation::{
    run_isolate, run_replay, synthetic_interframe, synthetic_keyframe_bundle, IsolateMode,
    IsolateParams,
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
    Replay(ReplayArgs),
    /// Milestone 3: run heartbeat-only, video-only or both for a fixed duration.
    Isolate(IsolateArgs),
    /// Milestone 4: capture frames from the desktop and write them as PNG files.
    Capture(CaptureArgs),
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
    /// Target width in pixels for header field B. Defaults to the configured display width.
    #[arg(long)]
    width: Option<u32>,
    /// Annex-B H.264 file (for example from ffmpeg `-f h264`) to send instead of synthetic
    /// filler messages. Required for the panel to show a real picture.
    #[arg(long, value_name = "PATH")]
    h264_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ReplayArgs {
    #[command(flatten)]
    nodes: NodeArgs,
    #[command(flatten)]
    stream: StreamArgs,
    /// How many times to replay the message set.
    #[arg(long, default_value_t = 20)]
    iterations: usize,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ModeArg {
    Heartbeat,
    Video,
    Both,
}

impl From<ModeArg> for IsolateMode {
    fn from(mode: ModeArg) -> Self {
        match mode {
            ModeArg::Heartbeat => IsolateMode::Heartbeat,
            ModeArg::Video => IsolateMode::Video,
            ModeArg::Both => IsolateMode::Both,
        }
    }
}

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
    nodes: &NodeArgs,
    need_mi00: bool,
    need_mi01: bool,
) -> Result<(Option<Mi00Path>, Option<Mi01Path>)> {
    let missing = (need_mi00 && nodes.mi00.is_none()) || (need_mi01 && nodes.mi01.is_none());
    let discovered = if missing {
        Some(discover().context("failed to resolve dock hidraw nodes")?)
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

fn width_for(stream: &StreamArgs, config: &Config) -> PixelWidth {
    PixelWidth(stream.width.unwrap_or(config.display.width))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = load_config(cli.config.as_ref())?;
    // A second init in the same process is a caller bug but not fatal for a CLI.
    let _ = init_logging(&config);

    match cli.command {
        Command::Discover => {
            let paths = discover().context("failed to resolve dock hidraw nodes")?;
            println!("MI_00 (control):  {}", paths.mi00.display());
            println!("MI_01 (video):    {}", paths.mi01.display());
        }
        Command::Replay(args) => {
            let (_, mi01) = resolve_nodes(&args.nodes, false, true)?;
            let mi01 = mi01.expect("MI_01 was requested");
            let messages = load_messages(&args.stream)?;
            warn_if_not_char_device(&mi01);
            let mut handle = HidrawHandle::open_mi01(&mi01).context("failed to open MI_01")?;
            let width = width_for(&args.stream, &config);
            println!(
                "replaying {} message(s) x {} iteration(s) to {}",
                messages.len(),
                args.iterations,
                mi01.display()
            );
            let report = run_replay(&mut handle, width, &messages, args.iterations)
                .context("replay failed")?;
            println!("{report}");
        }
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
            let (mi00, mi01) = resolve_nodes(&args.nodes, need_mi00, need_mi01)?;
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
            let params = IsolateParams {
                mode,
                duration: Duration::from_secs(args.duration_secs),
                heartbeat_interval: Duration::from_millis(config.daemon.heartbeat_interval_ms),
                width: width_for(&args.stream, &config),
                fps: args.fps,
                gop,
            };
            println!(
                "running {:?} for {}s (watch the panel and record what it shows)",
                mode, args.duration_secs
            );
            let report = run_isolate(&params, mi00_handle.as_mut(), mi01_handle.as_mut())
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
    }
    Ok(())
}
