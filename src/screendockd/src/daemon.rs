//! The capture, encode, frame and write loop plus the MI_00 session (Milestones 6 and 7).
//!
//! [`run`] takes already opened handles and already constructed backends, so everything that can
//! fail at startup (discovery, opening the nodes, creating the capture backend and encoder, the
//! first heartbeat write) happens before it and aborts the process. Once the loop runs, failures
//! are per frame and recoverable: a failed capture, encode or write is logged and counted, the
//! frame is dropped, and after a failed write the encoder is asked for a keyframe so the panel
//! can resynchronize. Only a long unbroken run of failures ends the daemon with an error, so a
//! supervisor restarts it instead of it spinning against a dead device.
//!
//! MI_00 belongs to the [`Session`](crate::session::Session): its heartbeat, reader and
//! handshake threads run independently of the video loop. The video loop runs on the calling
//! thread, is paced by the capture backend, and writes nothing until the session is armed. When
//! the session is lost it pauses, and when the handshake completes again it restarts from a
//! keyframe.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use thiserror::Error;
use tracing::{debug, info, info_span, warn};

use crate::capture::CaptureBackend;
use crate::display::DisplayGeometry;
use crate::encode::Encoder;
use crate::framing::{MessageBuilder, ReportId};
use crate::hidraw_io::HidrawHandle;
use crate::session::{Session, SessionError, SessionHandle, SessionSettings};

/// Consecutive failed frames after which the daemon gives up.
pub const DEFAULT_MAX_CONSECUTIVE_ERRORS: u32 = 100;

/// Pause after a failed capture so a broken backend cannot spin the CPU.
const CAPTURE_RETRY_DELAY: Duration = Duration::from_millis(40);

/// Pause between checks while the session is not armed.
const UNARMED_POLL: Duration = Duration::from_millis(20);

/// How often the loop logs a progress line.
const STATS_INTERVAL: Duration = Duration::from_secs(5);

/// Fatal daemon errors.
#[derive(Debug, Error)]
pub enum DaemonError {
    /// A handle was opened for the wrong channel.
    #[error("{what} handle is the {got:?} channel")]
    WrongChannel { what: &'static str, got: ReportId },
    /// The session could not start (for example the first heartbeat write failed).
    #[error("session failed: {0}")]
    Session(#[from] SessionError),
    /// The session ended for good while the daemon ran (the MI_00 reader died).
    #[error("session ended: {0}")]
    SessionEnded(String),
    /// Too many frames in a row failed.
    #[error("{count} consecutive frame failures, last: {last}")]
    TooManyErrors { count: u32, last: String },
}

/// Loop settings.
#[derive(Debug, Clone)]
pub struct DaemonParams {
    pub geometry: DisplayGeometry,
    /// MI_00 session settings: session id, handshake switch and timeouts.
    pub session: SessionSettings,
    /// Stop after this many frames were sent (tests and dry runs); `None` runs until stopped.
    pub max_frames: Option<u64>,
    pub max_consecutive_errors: u32,
}

/// Counters from a finished run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonReport {
    pub frames_sent: u64,
    pub messages_sent: u64,
    pub keyframes_sent: u64,
    pub capture_errors: u64,
    pub encode_errors: u64,
    pub write_errors: u64,
    pub heartbeats_sent: u64,
    pub heartbeat_errors: u64,
    /// Times the session became armed (first handshake plus every redo).
    pub handshakes: u64,
    /// Times an armed session was lost and the handshake redone.
    pub session_losses: u64,
    /// Heartbeats the dock sent that the reader saw.
    pub dock_heartbeats_seen: u64,
    pub elapsed: Duration,
}

/// Runs the daemon until `stop` is set, `params.max_frames` frames were sent, or too many frames
/// in a row failed.
pub fn run(
    mi00: HidrawHandle,
    mut mi01: HidrawHandle,
    capture: &mut dyn CaptureBackend,
    encoder: &mut dyn Encoder,
    params: &DaemonParams,
    stop: &AtomicBool,
) -> Result<DaemonReport, DaemonError> {
    if mi00.channel() != ReportId::Control {
        return Err(DaemonError::WrongChannel {
            what: "MI_00",
            got: mi00.channel(),
        });
    }
    if mi01.channel() != ReportId::Video {
        return Err(DaemonError::WrongChannel {
            what: "MI_01",
            got: mi01.channel(),
        });
    }

    // Fail fast: a dead control channel at startup means nothing else will work either. The
    // first heartbeat is written before any thread starts.
    let started = Instant::now();
    let session = Session::start(mi00, params.session.clone())?;
    let handle = session.handle();
    let mut report = DaemonReport::default();

    let loop_result = video_loop(
        &mut mi01,
        capture,
        encoder,
        params,
        &handle,
        stop,
        &mut report,
    );
    let stats = session.shutdown()?;
    report.heartbeats_sent = stats.heartbeats_sent;
    report.heartbeat_errors = stats.heartbeat_errors;
    report.handshakes = stats.handshakes;
    report.session_losses = stats.losses;
    report.dock_heartbeats_seen = stats.dock_heartbeats_seen;
    report.elapsed = started.elapsed();
    loop_result?;
    info!(?report, "daemon stopped");
    Ok(report)
}

fn video_loop(
    mi01: &mut HidrawHandle,
    capture: &mut dyn CaptureBackend,
    encoder: &mut dyn Encoder,
    params: &DaemonParams,
    session: &SessionHandle,
    stop: &AtomicBool,
    report: &mut DaemonReport,
) -> Result<(), DaemonError> {
    let span = info_span!(
        "video_loop",
        capture = capture.name(),
        encoder = encoder.name()
    );
    let _guard = span.enter();
    info!("video loop started");

    let mut consecutive: u32 = 0;
    let mut generation = 0u64;
    let mut last_stats = Instant::now();
    let fail = |consecutive: &mut u32, what: String| -> Result<(), DaemonError> {
        *consecutive += 1;
        if *consecutive >= params.max_consecutive_errors {
            return Err(DaemonError::TooManyErrors {
                count: *consecutive,
                last: what,
            });
        }
        Ok(())
    };

    while !stop.load(Ordering::Relaxed) && params.max_frames.is_none_or(|m| report.frames_sent < m)
    {
        if !session.is_armed() {
            // Nothing is captured or written until the dock accepts video; the session thread
            // owns the handshake and its timeouts.
            if let Some(reason) = session.fatal() {
                return Err(DaemonError::SessionEnded(reason));
            }
            std::thread::sleep(UNARMED_POLL);
            continue;
        }
        if session.generation() != generation {
            generation = session.generation();
            encoder.request_keyframe();
        }
        let frame = match capture.next_frame() {
            Ok(frame) => frame,
            Err(e) => {
                report.capture_errors += 1;
                warn!(error = %e, "capture failed, dropping frame");
                fail(&mut consecutive, e.to_string())?;
                std::thread::sleep(CAPTURE_RETRY_DELAY);
                continue;
            }
        };
        let messages = match encoder.encode(&frame) {
            Ok(messages) => messages,
            Err(e) => {
                report.encode_errors += 1;
                warn!(error = %e, "encode failed, dropping frame");
                encoder.request_keyframe();
                fail(&mut consecutive, e.to_string())?;
                continue;
            }
        };

        let mut frame_ok = true;
        for message in &messages {
            if !session.is_armed() {
                // Lost mid-frame: the rest of this frame would be dropped by the dock.
                encoder.request_keyframe();
                frame_ok = false;
                break;
            }
            let chunks = MessageBuilder::video(params.geometry.size())
                .payload(message.as_bytes())
                .build();
            match mi01.write_chunks(&chunks) {
                Ok(_) => {
                    report.messages_sent += 1;
                    report.keyframes_sent += u64::from(message.is_keyframe());
                    debug!(
                        bytes = message.as_bytes().len(),
                        chunks = chunks.len(),
                        keyframe = message.is_keyframe(),
                        "message written"
                    );
                }
                Err(e) => {
                    report.write_errors += 1;
                    warn!(error = %e, "video write failed, dropping message");
                    session.report_failure("video write failed");
                    // The panel may now lack a reference frame; restart from a keyframe.
                    encoder.request_keyframe();
                    fail(&mut consecutive, e.to_string())?;
                    frame_ok = false;
                    break;
                }
            }
        }
        if frame_ok {
            consecutive = 0;
            report.frames_sent += 1;
        }

        if last_stats.elapsed() >= STATS_INTERVAL {
            last_stats = Instant::now();
            info!(
                frames = report.frames_sent,
                messages = report.messages_sent,
                write_errors = report.write_errors,
                "progress"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{CaptureError, Frame, PixelFormat};
    use crate::display::Orientation;
    use crate::encode::prerecorded::PrerecordedEncoder;
    use crate::encode::EncoderSettings;
    use crate::framing::{parse_chunks, PictureSize};
    use std::fs::OpenOptions;
    use std::io::{Read, Seek, SeekFrom};

    struct Solid {
        fail_first: u32,
    }

    impl CaptureBackend for Solid {
        fn name(&self) -> &'static str {
            "solid"
        }
        fn next_frame(&mut self) -> Result<Frame, CaptureError> {
            if self.fail_first > 0 {
                self.fail_first -= 1;
                return Err(CaptureError::Connection("synthetic".into()));
            }
            std::thread::sleep(Duration::from_millis(2));
            Frame::new(2, 2, PixelFormat::Bgrx8888, 8, vec![0; 16])
        }
    }

    fn geometry() -> DisplayGeometry {
        DisplayGeometry::new(1280, 720, Orientation::Landscape).unwrap()
    }

    fn encoder() -> PrerecordedEncoder {
        let settings = EncoderSettings {
            geometry: geometry(),
            fps: 25,
            keyframe_interval: 2,
            bitrate_kbps: 1000,
        };
        let big = vec![0xAB; 9000];
        let mut key = vec![0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65];
        key.extend(&big);
        let mut inter = vec![0, 0, 0, 1, 0x41];
        inter.extend(&big);
        PrerecordedEncoder::new(vec![key, inter], settings).unwrap()
    }

    fn loopback(dir: &tempfile::TempDir, name: &str, channel: ReportId) -> HidrawHandle {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(dir.path().join(name))
            .unwrap();
        HidrawHandle::from_file(file, channel)
    }

    fn params(max_frames: u64) -> DaemonParams {
        DaemonParams {
            geometry: geometry(),
            session: SessionSettings::new(geometry().size(), Duration::from_millis(20))
                .without_handshake(),
            max_frames: Some(max_frames),
            max_consecutive_errors: 5,
        }
    }

    #[test]
    fn writes_keyframe_bundle_then_single_nal_messages_and_heartbeats() {
        let dir = tempfile::tempdir().unwrap();
        let mi00 = loopback(&dir, "mi00", ReportId::Control);
        let mi01 = loopback(&dir, "mi01", ReportId::Video);
        // Separate descriptors on the same files read back what the daemon wrote.
        let mut video_read = OpenOptions::new()
            .read(true)
            .open(dir.path().join("mi01"))
            .unwrap();
        let mut heartbeat_read = OpenOptions::new()
            .read(true)
            .open(dir.path().join("mi00"))
            .unwrap();

        let stop = AtomicBool::new(false);
        let report = run(
            mi00,
            mi01,
            &mut Solid { fail_first: 0 },
            &mut encoder(),
            &params(4),
            &stop,
        )
        .unwrap();
        assert_eq!(report.frames_sent, 4);
        assert_eq!(report.keyframes_sent, 2);
        assert_eq!(report.write_errors, 0);
        assert!(report.heartbeats_sent >= 1);

        video_read.seek(SeekFrom::Start(0)).unwrap();
        let mut reader = HidrawHandle::from_file(video_read, ReportId::Video);
        let mut kinds = Vec::new();
        for _ in 0..4 {
            let msg = reader.read_message().unwrap();
            assert_eq!(msg.size, PictureSize::new(1280, 720));
            kinds.push(msg.payload[4] & 0x1F);
        }
        assert_eq!(kinds, [7, 1, 7, 1]);

        let mut hb = Vec::new();
        heartbeat_read.read_to_end(&mut hb).unwrap();
        let first = parse_chunks(&[hb[..crate::framing::REPORT_SIZE].to_vec()]).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&first.payload).unwrap();
        assert_eq!(value["method"], "heartbeat");
        assert!(value["params"]["session-id"]
            .as_str()
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())));
    }

    #[test]
    fn recovers_from_capture_errors_but_gives_up_after_too_many() {
        let dir = tempfile::tempdir().unwrap();
        let stop = AtomicBool::new(false);
        let report = run(
            loopback(&dir, "a0", ReportId::Control),
            loopback(&dir, "a1", ReportId::Video),
            &mut Solid { fail_first: 3 },
            &mut encoder(),
            &params(2),
            &stop,
        )
        .unwrap();
        assert_eq!(report.capture_errors, 3);
        assert_eq!(report.frames_sent, 2);

        let err = run(
            loopback(&dir, "b0", ReportId::Control),
            loopback(&dir, "b1", ReportId::Video),
            &mut Solid { fail_first: 100 },
            &mut encoder(),
            &params(2),
            &stop,
        )
        .unwrap_err();
        assert!(matches!(err, DaemonError::TooManyErrors { count: 5, .. }));
    }

    #[test]
    fn persistent_write_errors_end_the_run() {
        let dir = tempfile::tempdir().unwrap();
        // A read-only video file makes every write fail.
        let path = dir.path().join("ro");
        std::fs::write(&path, b"").unwrap();
        let ro = HidrawHandle::from_file(
            OpenOptions::new().read(true).open(&path).unwrap(),
            ReportId::Video,
        );
        let stop = AtomicBool::new(false);
        let err = run(
            loopback(&dir, "c0", ReportId::Control),
            ro,
            &mut Solid { fail_first: 0 },
            &mut encoder(),
            &params(3),
            &stop,
        )
        .unwrap_err();
        assert!(matches!(err, DaemonError::TooManyErrors { .. }));
    }

    #[test]
    fn startup_heartbeat_failure_is_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ro");
        std::fs::write(&path, b"").unwrap();
        let ro = HidrawHandle::from_file(
            OpenOptions::new().read(true).open(&path).unwrap(),
            ReportId::Control,
        );
        let stop = AtomicBool::new(false);
        let err = run(
            ro,
            loopback(&dir, "v", ReportId::Video),
            &mut Solid { fail_first: 0 },
            &mut encoder(),
            &params(1),
            &stop,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            DaemonError::Session(SessionError::StartupHeartbeat(_))
        ));
    }

    #[test]
    fn swapped_handles_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let stop = AtomicBool::new(false);
        let err = run(
            loopback(&dir, "x", ReportId::Video),
            loopback(&dir, "y", ReportId::Control),
            &mut Solid { fail_first: 0 },
            &mut encoder(),
            &params(1),
            &stop,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            DaemonError::WrongChannel { what: "MI_00", .. }
        ));
    }
}
