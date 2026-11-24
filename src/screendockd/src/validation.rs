//! Hardware validation routines for Milestones 2 and 3 (`docs/hardware-validation.md`).
//!
//! - `run_replay`: replays video messages back-to-back and reports per-chunk write timing
//!   (Milestone 2, native write-timing validation).
//! - `run_isolate`: runs heartbeat-only, video-only or both for a fixed duration (Milestone 3,
//!   heartbeat-dependency isolation).
//!
//! Everything takes already-open `HidrawHandle`s, so the same code runs against the real nodes
//! on the dock and against a file/pipe/socket loopback in tests. Where video is sent together
//! with an MI_00 handle, the dock session (heartbeat, reader and handshake, see `session.rs`)
//! runs first and video starts only once it is armed. The routines only measure and
//! report; what the panel shows is observed by the person running them.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::heartbeat::{run_heartbeat_loop, HeartbeatError, HeartbeatStats};
use crate::hidraw_io::{HidrawHandle, HidrawIoError};
use crate::protocol::framing::{MessageBuilder, PictureSize, ReportId};
use crate::protocol::heartbeat::SessionId;
use crate::session::{Session, SessionError, SessionHandle, SessionSettings, SessionStats};

/// Per-chunk write latency the PowerShell/.NET prototype achieved, in milliseconds. The
/// Milestone 2 exit criterion is at or below this figure.
pub const REFERENCE_MS_PER_CHUNK: f64 = 0.77;

/// Size of the research's keyframe bundle fixture (SPS+PPS+SEI+IDR), in bytes.
pub const SYNTHETIC_KEYFRAME_LEN: usize = 3577;

/// Size of the research's 17-chunk inter-frame fixture, in bytes.
pub const SYNTHETIC_INTERFRAME_LEN: usize = 67_666;

/// Longest single sleep while waiting, so deadlines and stop requests stay responsive.
const SLEEP_SLICE: Duration = Duration::from_millis(20);

/// Errors from the validation routines.
#[derive(Debug, Error)]
pub enum ValidationError {
    /// A hidraw write failed.
    #[error("hidraw I/O failed: {0}")]
    Io(#[from] HidrawIoError),
    /// The heartbeat loop failed.
    #[error("heartbeat failed: {0}")]
    Heartbeat(#[from] HeartbeatError),
    /// The caller passed unusable parameters.
    #[error("invalid validation input: {0}")]
    InvalidInput(String),
    /// The heartbeat thread panicked.
    #[error("heartbeat thread panicked")]
    HeartbeatPanicked,
    /// The dock session failed to start or ended.
    #[error("session failed: {0}")]
    Session(#[from] SessionError),
    /// The session did not become armed in time.
    #[error("dock session not armed within {0:?}")]
    NotArmed(Duration),
    /// The session ended for good.
    #[error("dock session ended: {0}")]
    SessionEnded(String),
}

/// Waits until `session` is armed for video, for at most `limit`.
pub fn wait_until_armed(session: &SessionHandle, limit: Duration) -> Result<(), ValidationError> {
    let deadline = Instant::now() + limit;
    while !session.is_armed() {
        if let Some(reason) = session.fatal() {
            return Err(ValidationError::SessionEnded(reason));
        }
        if Instant::now() >= deadline {
            return Err(ValidationError::NotArmed(limit));
        }
        std::thread::sleep(SLEEP_SLICE);
    }
    Ok(())
}

fn nal(nal_type: u8, nal_ref_idc: u8, body_len: usize) -> Vec<u8> {
    let mut v = vec![0x00, 0x00, 0x00, 0x01, (nal_ref_idc << 5) | nal_type];
    v.resize(5 + body_len, 0xAA);
    v
}

/// A keyframe-bundle-shaped message (SPS + PPS + SEI + IDR) of exactly 3577 bytes, with filler
/// bodies. It has the right size and NAL layout for timing tests but is not decodable video.
pub fn synthetic_keyframe_bundle() -> Vec<u8> {
    let mut v = Vec::with_capacity(SYNTHETIC_KEYFRAME_LEN);
    v.extend(nal(7, 3, 25));
    v.extend(nal(8, 3, 7));
    v.extend(nal(6, 0, 15));
    v.extend(nal(5, 3, 3510));
    debug_assert_eq!(v.len(), SYNTHETIC_KEYFRAME_LEN);
    v
}

/// A single-NAL inter-frame-shaped message of exactly 67,666 bytes (17 chunks), filler body.
pub fn synthetic_interframe() -> Vec<u8> {
    nal(1, 2, SYNTHETIC_INTERFRAME_LEN - 5)
}

/// Summary statistics over a set of durations.
#[derive(Debug, Clone, PartialEq)]
pub struct TimingStats {
    pub count: usize,
    pub min: Duration,
    pub max: Duration,
    pub mean: Duration,
    pub p50: Duration,
    pub p95: Duration,
    pub p99: Duration,
}

impl TimingStats {
    /// Computes stats; `None` when `samples` is empty.
    pub fn from_durations(samples: &[Duration]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.to_vec();
        sorted.sort();
        let count = sorted.len();
        let total: Duration = sorted.iter().sum();
        // Nearest-rank percentile.
        let pct = |p: usize| sorted[((count * p).div_ceil(100)).clamp(1, count) - 1];
        Some(Self {
            count,
            min: sorted[0],
            max: sorted[count - 1],
            mean: total / count as u32,
            p50: pct(50),
            p95: pct(95),
            p99: pct(99),
        })
    }

    /// Mean in milliseconds.
    pub fn mean_ms(&self) -> f64 {
        self.mean.as_secs_f64() * 1000.0
    }
}

impl fmt::Display for TimingStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let us = |d: Duration| d.as_secs_f64() * 1_000_000.0;
        write!(
            f,
            "n={} min={:.0}us mean={:.0}us p50={:.0}us p95={:.0}us p99={:.0}us max={:.0}us",
            self.count,
            us(self.min),
            us(self.mean),
            us(self.p50),
            us(self.p95),
            us(self.p99),
            us(self.max),
        )
    }
}

/// Result of a replay run.
#[derive(Debug, Clone)]
pub struct ReplayReport {
    pub messages_sent: usize,
    pub chunks_sent: usize,
    /// Stats over every individual `write(2)` call; `None` if nothing was sent.
    pub chunk_stats: Option<TimingStats>,
    /// Stats over per-message total write time (first write start to last write end).
    pub message_stats: Option<TimingStats>,
    /// Wall time per message divided by its chunk count, averaged, in milliseconds. This is
    /// the figure comparable to `REFERENCE_MS_PER_CHUNK`, since it includes inter-write gaps.
    pub mean_ms_per_chunk_wall: f64,
}

impl ReplayReport {
    /// True when the mean wall time per chunk is at or below the reference figure.
    pub fn meets_reference(&self) -> bool {
        self.mean_ms_per_chunk_wall <= REFERENCE_MS_PER_CHUNK
    }
}

impl fmt::Display for ReplayReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "messages sent: {}  chunks sent: {}",
            self.messages_sent, self.chunks_sent
        )?;
        if let Some(s) = &self.chunk_stats {
            writeln!(f, "per-chunk write(2) latency: {s}")?;
        }
        if let Some(s) = &self.message_stats {
            writeln!(f, "per-message total write time: {s}")?;
        }
        write!(
            f,
            "mean wall time per chunk: {:.3} ms (reference {:.2} ms): {}",
            self.mean_ms_per_chunk_wall,
            REFERENCE_MS_PER_CHUNK,
            if self.meets_reference() {
                "at or below reference"
            } else {
                "above reference"
            }
        )
    }
}

fn video_chunks(size: PictureSize, message: &[u8]) -> Vec<Vec<u8>> {
    MessageBuilder::video(size).payload(message).build()
}

fn require_video(handle: &HidrawHandle) -> Result<(), ValidationError> {
    if handle.channel() != ReportId::Video {
        return Err(ValidationError::InvalidInput(format!(
            "video writes need the MI_01 (video) channel, got {:?}",
            handle.channel()
        )));
    }
    Ok(())
}

/// Replays `messages` in order, `iterations` times, each message back-to-back with no delay
/// between its chunks and none between messages, recording per-chunk timing.
pub fn run_replay(
    handle: &mut HidrawHandle,
    size: PictureSize,
    messages: &[Vec<u8>],
    iterations: usize,
) -> Result<ReplayReport, ValidationError> {
    require_video(handle)?;
    if messages.is_empty() || iterations == 0 {
        return Err(ValidationError::InvalidInput(
            "replay needs at least one message and one iteration".into(),
        ));
    }
    let framed: Vec<Vec<Vec<u8>>> = messages.iter().map(|m| video_chunks(size, m)).collect();

    let mut chunk_samples = Vec::new();
    let mut message_samples = Vec::new();
    let mut wall_ms_per_chunk = Vec::new();
    for _ in 0..iterations {
        for chunks in &framed {
            let timings = handle.write_chunks(chunks)?;
            wall_ms_per_chunk.push(timings.total.as_secs_f64() * 1000.0 / chunks.len() as f64);
            message_samples.push(timings.total);
            chunk_samples.extend(timings.per_chunk);
        }
    }
    Ok(ReplayReport {
        messages_sent: message_samples.len(),
        chunks_sent: chunk_samples.len(),
        chunk_stats: TimingStats::from_durations(&chunk_samples),
        message_stats: TimingStats::from_durations(&message_samples),
        mean_ms_per_chunk_wall: wall_ms_per_chunk.iter().sum::<f64>()
            / wall_ms_per_chunk.len() as f64,
    })
}

/// The three Milestone 3 conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolateMode {
    /// Heartbeat on MI_00 only, no video.
    Heartbeat,
    /// Video on MI_01 only, no heartbeat.
    Video,
    /// Both concurrently.
    Both,
}

impl IsolateMode {
    fn heartbeat(self) -> bool {
        matches!(self, Self::Heartbeat | Self::Both)
    }
    fn video(self) -> bool {
        matches!(self, Self::Video | Self::Both)
    }
}

/// Parameters for an isolation run.
#[derive(Debug, Clone)]
pub struct IsolateParams {
    pub mode: IsolateMode,
    pub duration: Duration,
    pub heartbeat_interval: Duration,
    pub size: PictureSize,
    /// MI_00 session settings. `Some` runs the dock session (handshake, reader, heartbeat) on
    /// the MI_00 handle and gates video on it; `None` sends a bare heartbeat loop and ungated
    /// video, which a dock that has not been handshaken discards.
    pub session: Option<SessionSettings>,
    pub fps: u32,
    /// One looped group of pictures: element 0 must be the keyframe bundle, the rest
    /// inter-frames. The loop restarts at the keyframe each time it wraps.
    pub gop: Vec<Vec<u8>>,
}

/// Result of an isolation run.
#[derive(Debug, Clone)]
pub struct IsolateReport {
    pub mode: IsolateMode,
    pub elapsed: Duration,
    pub heartbeat: Option<HeartbeatStats>,
    /// Session counters when the dock session ran.
    pub session: Option<SessionStats>,
    pub video_messages: usize,
    pub video_chunk_stats: Option<TimingStats>,
    /// Frames whose send finished after their scheduled slot (the stream fell behind `fps`).
    pub video_late_frames: usize,
}

impl fmt::Display for IsolateReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "mode: {:?}  elapsed: {:.1}s",
            self.mode,
            self.elapsed.as_secs_f64()
        )?;
        match &self.heartbeat {
            Some(h) => writeln!(f, "heartbeat ticks sent: {}", h.ticks_sent)?,
            None => match &self.session {
                Some(s) => writeln!(
                    f,
                    "session: heartbeats sent {}, dock heartbeats seen {}, handshakes {}, losses {}",
                    s.heartbeats_sent, s.dock_heartbeats_seen, s.handshakes, s.losses
                )?,
                None => writeln!(f, "heartbeat: not run")?,
            },
        }
        if self.mode.video() {
            writeln!(
                f,
                "video messages sent: {}  late frames: {}",
                self.video_messages, self.video_late_frames
            )?;
            if let Some(s) = &self.video_chunk_stats {
                write!(f, "video per-chunk write(2) latency: {s}")?;
            }
        } else {
            write!(f, "video: not run")?;
        }
        Ok(())
    }
}

/// Sleeps until `deadline`, waking early when `stop` is set. Returns false if stopped.
fn sleep_until(deadline: Instant, stop: &AtomicBool) -> bool {
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(left.min(SLEEP_SLICE));
    }
    true
}

struct VideoOutcome {
    messages: usize,
    chunk_samples: Vec<Duration>,
    late: usize,
}

fn run_video_paced(
    handle: &mut HidrawHandle,
    size: PictureSize,
    gop: &[Vec<u8>],
    fps: u32,
    duration: Duration,
    session: Option<&SessionHandle>,
) -> Result<VideoOutcome, ValidationError> {
    let framed: Vec<Vec<Vec<u8>>> = gop.iter().map(|m| video_chunks(size, m)).collect();
    let frame_period = Duration::from_secs_f64(1.0 / f64::from(fps));
    let never_stop = AtomicBool::new(false);
    let start = Instant::now();
    let mut outcome = VideoOutcome {
        messages: 0,
        chunk_samples: Vec::new(),
        late: 0,
    };
    let mut index = 0usize;
    let mut generation = 0u64;
    let mut next_slot = start;
    while start.elapsed() < duration {
        if let Some(session) = session {
            if let Some(reason) = session.fatal() {
                return Err(ValidationError::SessionEnded(reason));
            }
            if !session.is_armed() {
                std::thread::sleep(SLEEP_SLICE);
                continue;
            }
            if session.generation() != generation {
                // The dock may have lost its decoder state: restart at the keyframe and
                // re-base the pacing so the wait is not counted as lateness.
                generation = session.generation();
                index = 0;
                next_slot = Instant::now();
            }
        }
        let slot_end = next_slot + frame_period;
        next_slot = slot_end;
        let timings = match handle.write_chunks(&framed[index % framed.len()]) {
            Ok(timings) => timings,
            Err(e) => {
                if let Some(session) = session {
                    session.report_failure("video write failed");
                }
                return Err(e.into());
            }
        };
        outcome.chunk_samples.extend(timings.per_chunk);
        outcome.messages += 1;
        index += 1;
        if Instant::now() > slot_end {
            outcome.late += 1;
        } else {
            sleep_until(slot_end, &never_stop);
        }
    }
    Ok(outcome)
}

/// Runs one Milestone 3 condition for `params.duration`. Handles for channels the mode doesn't
/// use must be `None`, and handles for channels it does use must be `Some`, so a heartbeat-only
/// run provably never opens or writes to MI_01 (and vice versa).
pub fn run_isolate(
    params: &IsolateParams,
    mi00: Option<&mut HidrawHandle>,
    mi01: Option<&mut HidrawHandle>,
) -> Result<IsolateReport, ValidationError> {
    let mode = params.mode;
    if mode.heartbeat() != mi00.is_some() || mode.video() != mi01.is_some() {
        return Err(ValidationError::InvalidInput(format!(
            "mode {mode:?} needs MI_00 handle: {}, MI_01 handle: {}",
            mode.heartbeat(),
            mode.video()
        )));
    }
    if mode.video() {
        if params.fps == 0 || params.gop.is_empty() {
            return Err(ValidationError::InvalidInput(
                "video needs fps > 0 and a non-empty GOP".into(),
            ));
        }
        if let Some(h) = mi01.as_deref() {
            require_video(h)?;
        }
    }

    let stop = AtomicBool::new(false);
    let start = Instant::now();
    let deadline = start + params.duration;

    // The session needs its own descriptor, so a cloned handle goes to it and the original
    // stays with the caller. Only modes with an MI_00 handle run a session.
    let session = match (&params.session, mi00.as_deref()) {
        (Some(settings), Some(handle)) => {
            Some(Session::start(handle.try_clone()?, settings.clone())?)
        }
        _ => None,
    };
    let session_handle = session.as_ref().map(Session::handle);
    // With a session the heartbeat runs inside it; without one the bare loop runs here.
    let bare_heartbeat_handle = if session.is_some() { None } else { mi00 };

    let (heartbeat, video) = std::thread::scope(|scope| {
        let hb_thread = bare_heartbeat_handle.map(|handle| {
            let stop = &stop;
            let id = params
                .session
                .as_ref()
                .map_or_else(SessionId::generate, |s| s.session_id.clone());
            scope.spawn(move || {
                run_heartbeat_loop(handle, &id, params.heartbeat_interval, stop, None)
            })
        });

        let video = match mi01 {
            Some(handle) => run_video_paced(
                handle,
                params.size,
                &params.gop,
                params.fps,
                params.duration,
                session_handle.as_ref(),
            )
            .map(Some),
            None => {
                sleep_until(deadline, &stop);
                Ok(None)
            }
        };
        stop.store(true, Ordering::Relaxed);

        let heartbeat = match hb_thread {
            Some(t) => match t.join() {
                Ok(result) => result.map(Some).map_err(ValidationError::from),
                Err(_) => Err(ValidationError::HeartbeatPanicked),
            },
            None => Ok(None),
        };
        (heartbeat, video)
    });

    let session_stats = session.map(Session::shutdown).transpose()?;
    let video = video?;
    let heartbeat = heartbeat?;
    let (video_messages, video_chunk_stats, video_late_frames) = match video {
        Some(v) => (
            v.messages,
            TimingStats::from_durations(&v.chunk_samples),
            v.late,
        ),
        None => (0, None, 0),
    };
    Ok(IsolateReport {
        mode,
        elapsed: start.elapsed(),
        heartbeat,
        session: session_stats,
        video_messages,
        video_chunk_stats,
        video_late_frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::framing::parse_chunks;
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom};

    fn loopback(channel: ReportId) -> (HidrawHandle, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(dir.path().join("node"))
            .unwrap();
        (HidrawHandle::from_file(file, channel), dir)
    }

    #[test]
    fn synthetic_messages_have_research_sizes_and_chunk_counts() {
        assert_eq!(synthetic_keyframe_bundle().len(), 3577);
        assert_eq!(synthetic_interframe().len(), 67_666);
        let chunks = video_chunks(PictureSize::new(1280, 720), &synthetic_interframe());
        assert_eq!(chunks.len(), 17);
        assert_eq!(
            parse_chunks(&chunks).unwrap().payload,
            synthetic_interframe()
        );
    }

    #[test]
    fn stats_use_nearest_rank_percentiles() {
        let samples: Vec<Duration> = (1..=100).map(Duration::from_micros).collect();
        let s = TimingStats::from_durations(&samples).unwrap();
        assert_eq!(s.count, 100);
        assert_eq!(s.min, Duration::from_micros(1));
        assert_eq!(s.max, Duration::from_micros(100));
        assert_eq!(s.p50, Duration::from_micros(50));
        assert_eq!(s.p95, Duration::from_micros(95));
        assert_eq!(s.p99, Duration::from_micros(99));
        assert!(TimingStats::from_durations(&[]).is_none());
    }

    #[test]
    fn replay_writes_keyframe_plus_seventeen_chunks_and_reports_timing() {
        let (mut handle, dir) = loopback(ReportId::Video);
        let messages = vec![synthetic_keyframe_bundle(), synthetic_interframe()];
        let report = run_replay(&mut handle, PictureSize::new(1280, 720), &messages, 2).unwrap();
        assert_eq!(report.messages_sent, 4);
        assert_eq!(report.chunks_sent, 2 * (1 + 17));
        assert_eq!(report.chunk_stats.as_ref().unwrap().count, 36);
        let len = std::fs::metadata(dir.path().join("node")).unwrap().len();
        assert_eq!(len as usize, 36 * 4096);
        assert!(report.to_string().contains("mean wall time per chunk"));

        // The file holds a parseable keyframe message first.
        handle.file_for_test().seek(SeekFrom::Start(0)).unwrap();
        let first = handle.read_message().unwrap();
        assert_eq!(first.payload, synthetic_keyframe_bundle());
    }

    #[test]
    fn replay_rejects_control_handle_and_empty_input() {
        let (mut control, _d) = loopback(ReportId::Control);
        assert!(run_replay(&mut control, PictureSize::new(1280, 720), &[vec![1]], 1).is_err());
        let (mut video, _d2) = loopback(ReportId::Video);
        assert!(run_replay(&mut video, PictureSize::new(1280, 720), &[], 1).is_err());
        assert!(run_replay(&mut video, PictureSize::new(1280, 720), &[vec![1]], 0).is_err());
    }

    fn params(mode: IsolateMode) -> IsolateParams {
        IsolateParams {
            mode,
            duration: Duration::from_millis(300),
            heartbeat_interval: Duration::from_millis(50),
            size: PictureSize::new(1280, 720),
            session: None,
            fps: 25,
            gop: vec![synthetic_keyframe_bundle(), vec![0, 0, 0, 1, 0x41, 1, 2, 3]],
        }
    }

    fn count_reports(dir: &tempfile::TempDir) -> usize {
        let mut bytes = Vec::new();
        File::open(dir.path().join("node"))
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        bytes.len() / 4096
    }

    #[test]
    fn isolate_heartbeat_only_writes_only_to_mi00() {
        let (mut mi00, d00) = loopback(ReportId::Control);
        let report = run_isolate(&params(IsolateMode::Heartbeat), Some(&mut mi00), None).unwrap();
        let ticks = report.heartbeat.as_ref().unwrap().ticks_sent;
        assert!((4..=8).contains(&ticks), "ticks = {ticks}");
        assert_eq!(report.video_messages, 0);
        assert_eq!(count_reports(&d00) as u64, ticks);
    }

    #[test]
    fn isolate_video_only_writes_only_to_mi01() {
        let (mut mi01, d01) = loopback(ReportId::Video);
        let report = run_isolate(&params(IsolateMode::Video), None, Some(&mut mi01)).unwrap();
        assert!(report.heartbeat.is_none());
        // 25 fps for 0.3 s is about 8 frames.
        assert!(
            (6..=10).contains(&report.video_messages),
            "{}",
            report.video_messages
        );
        assert_eq!(count_reports(&d01), report.video_messages);
    }

    #[test]
    fn isolate_both_runs_both_channels() {
        let (mut mi00, d00) = loopback(ReportId::Control);
        let (mut mi01, d01) = loopback(ReportId::Video);
        let report =
            run_isolate(&params(IsolateMode::Both), Some(&mut mi00), Some(&mut mi01)).unwrap();
        assert!(report.heartbeat.as_ref().unwrap().ticks_sent >= 4);
        assert!(report.video_messages >= 6);
        assert!(count_reports(&d00) >= 4);
        assert!(count_reports(&d01) >= 6);
    }

    #[test]
    fn isolate_rejects_handles_that_do_not_match_the_mode() {
        let (mut mi00, _d) = loopback(ReportId::Control);
        let (mut mi01, _d2) = loopback(ReportId::Video);
        assert!(run_isolate(
            &params(IsolateMode::Heartbeat),
            Some(&mut mi00),
            Some(&mut mi01)
        )
        .is_err());
        assert!(run_isolate(&params(IsolateMode::Video), None, None).is_err());
    }

    #[test]
    fn isolate_both_with_session_waits_for_the_handshake_before_video() {
        use crate::fake_dock::{FakeDock, FakeDockConfig};
        let (dock, mut mi00, mut mi01) = FakeDock::spawn_with_video(FakeDockConfig::fresh());
        let mut p = params(IsolateMode::Both);
        p.duration = Duration::from_millis(1500);
        let mut settings = SessionSettings::new(p.size, Duration::from_millis(50));
        settings.settle_delay = Duration::from_millis(100);
        settings.ready_timeout = Duration::from_millis(500);
        settings.reply_timeout = Duration::from_millis(500);
        p.session = Some(settings);
        let report = run_isolate(&p, Some(&mut mi00), Some(&mut mi01)).unwrap();
        let session = report.session.as_ref().unwrap();
        assert_eq!(session.handshakes, 1);
        assert!(report.video_messages >= 3, "{}", report.video_messages);
        // Let the fake drain the socket before reading its counters.
        std::thread::sleep(Duration::from_millis(100));
        let video = dock.video_stats();
        assert_eq!(video.reports_before_sink, 0);
        assert_eq!(video.layout_violations, 0);
        assert!(report.to_string().contains("handshakes 1"));
    }
}
