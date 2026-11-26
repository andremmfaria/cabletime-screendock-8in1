//! The capture, encode, frame and write loop plus the MI_00 session.
//!
//! [`run`] takes already opened handles and already constructed backends, so everything that can
//! fail at startup (discovery, opening the nodes, creating the capture backend and encoder, the
//! first heartbeat write) happens before it and aborts the process. Once the loop runs, failures
//! are per frame and recoverable: a failed capture, encode or write is logged and counted, the
//! frame is dropped, and after a failed write the encoder is asked for a keyframe so the panel
//! can resynchronize. Only a long unbroken run of failures ends the daemon with an error, so a
//! supervisor restarts it instead of it spinning against a dead device.
//!
//! MI_00 belongs to the [`Session`](crate::core::session::Session): its heartbeat, reader and
//! handshake threads run independently of the video loop. The video loop runs on the calling
//! thread, is paced by the capture backend, and writes nothing until the session is armed. When
//! the session is lost it pauses, and when the handshake completes again it restarts from a
//! keyframe.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tracing::{debug, info, info_span, warn};

use super::pipeline::probe_build;
pub use super::types::{
    DaemonError, DaemonParams, DaemonReport, EncoderBuilder, PipelineKnobs,
    DEFAULT_MAX_CONSECUTIVE_ERRORS,
};
use super::types::{
    EncoderSlot, LoopParts, CAPTURE_RETRY_DELAY, STATS_INTERVAL, UNARMED_POLL,
    WRITE_FAILURES_BEFORE_REHANDSHAKE,
};
use crate::core::keyframe::KeyframeClock;
use crate::core::session::{Session, SessionHandle};
use crate::ports::capture::{CaptureBackend, CaptureError};
use crate::ports::encode::{Encoder, RateSettings};
use crate::ports::transport::DockTransport;
use crate::protocol::framing::{MessageBuilder, ReportId};

/// Runs the daemon until `stop` is set, `params.max_frames` frames were sent, or too many frames
/// in a row failed.
pub fn run(
    mi00: impl DockTransport,
    mut mi01: impl DockTransport,
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
    params.pipeline.attach_session(handle.clone());
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
    mi01: &mut impl DockTransport,
    capture: &mut dyn CaptureBackend,
    base_encoder: &mut dyn Encoder,
    params: &DaemonParams,
    session: &SessionHandle,
    stop: &AtomicBool,
    report: &mut DaemonReport,
) -> Result<(), DaemonError> {
    let span = info_span!(
        "video_loop",
        capture = capture.name(),
        encoder = base_encoder.name()
    );
    let _guard = span.enter();
    info!("video loop started");

    let mut consecutive: u32 = 0;
    let mut write_failures: u32 = 0;
    let mut generation = 0u64;
    let mut last_stats = Instant::now();
    let mut clock = KeyframeClock::new(params.pipeline.knobs().keyframe_interval);
    let mut applied = params.pipeline.knobs();
    let mut seen = 0u64;
    let mut slot = EncoderSlot {
        base: base_encoder,
        replacement: None,
    };
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
        if let Some((version, knobs, cycle)) = params.pipeline.pending(seen) {
            seen = version;
            // The change runs here, on the only thread that writes video, so nothing is
            // written while the encoder is swapped or the dock cycle runs.
            let mut parts = LoopParts {
                slot: &mut slot,
                capture: &mut *capture,
                session,
                pipeline: &params.pipeline,
                cycle_timeout: params.session.cycle_timeout(),
            };
            let result = apply_knobs(&mut parts, &mut clock, &mut applied, knobs, cycle);
            if let Err(reason) = &result {
                warn!(%reason, "pipeline change refused");
            }
            params.pipeline.complete(version, result);
        }
        if !session.is_armed() {
            // Nothing is captured or written until the dock accepts video; the session thread
            // owns the handshake and its timeouts.
            if let Some(reason) = session.fatal() {
                return Err(DaemonError::SessionEnded(reason));
            }
            std::thread::sleep(UNARMED_POLL);
            continue;
        }
        let encoder = slot.active();
        if session.generation() != generation {
            generation = session.generation();
            force_keyframe(encoder, &mut clock);
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
        if clock.next_frame() {
            encoder.request_keyframe();
        }
        let messages = match encoder.encode(&frame) {
            Ok(messages) => messages,
            Err(e) => {
                report.encode_errors += 1;
                warn!(error = %e, "encode failed, dropping frame");
                force_keyframe(encoder, &mut clock);
                fail(&mut consecutive, e.to_string())?;
                continue;
            }
        };

        let mut frame_ok = true;
        for message in &messages {
            if !session.is_armed() {
                // Lost mid-frame: the rest of this frame would be dropped by the dock.
                force_keyframe(encoder, &mut clock);
                frame_ok = false;
                break;
            }
            let chunks = MessageBuilder::video(applied.geometry.size())
                .payload(message.as_bytes())
                .build();
            match mi01.write_chunks(&chunks) {
                Ok(_) => {
                    write_failures = 0;
                    params.pipeline.note_written();
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
                    write_failures += 1;
                    if write_failures >= WRITE_FAILURES_BEFORE_REHANDSHAKE {
                        session.report_failure("video writes keep failing");
                        write_failures = 0;
                    }
                    // The panel may now lack a reference frame; restart from a keyframe.
                    force_keyframe(encoder, &mut clock);
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

impl EncoderSlot<'_> {
    /// The encoder that produces the next frame.
    fn active(&mut self) -> &mut dyn Encoder {
        match self.replacement.as_mut() {
            Some(encoder) => encoder.as_mut(),
            None => &mut *self.base,
        }
    }
}

/// Asks for a keyframe on the next frame and restarts the keyframe count.
fn force_keyframe(encoder: &mut dyn Encoder, clock: &mut KeyframeClock) {
    encoder.request_keyframe();
    clock.keyframe_forced();
}

/// Applies the published knobs. A change that needs a new encoder, another capture area or
/// the dock cycle is done in one piece: the new encoder is built and probed first, then
/// capture is retargeted, then the dock cycle runs, and only when all of that worked the new
/// encoder replaces the old one. On an error nothing has changed.
fn apply_knobs(
    parts: &mut LoopParts<'_, '_>,
    clock: &mut KeyframeClock,
    current: &mut PipelineKnobs,
    new: PipelineKnobs,
    cycle: bool,
) -> Result<(), String> {
    let rate_changed =
        new.bitrate_kbps != current.bitrate_kbps || new.vbv_buffer_ms != current.vbv_buffer_ms;
    let rebuild = new.geometry != current.geometry
        || new.fps != current.fps
        || new.preset != current.preset
        || new.tune_zerolatency != current.tune_zerolatency
        || (rate_changed && !parts.pipeline.live_rate())
        || cycle;
    if rebuild {
        rebuild_pipeline(parts, current, &new, cycle)?;
        clock.set_interval(new.keyframe_interval);
        clock.keyframe_forced();
    } else {
        if rate_changed {
            parts
                .slot
                .active()
                .set_rate(RateSettings {
                    bitrate_kbps: new.bitrate_kbps,
                    vbv_buffer_ms: new.vbv_buffer_ms,
                })
                .map_err(|e| e.to_string())?;
        }
        if new.area != current.area {
            parts
                .capture
                .retarget(new.capture_target())
                .map_err(|e| e.to_string())?;
        }
        clock.set_interval(new.keyframe_interval);
    }
    *current = new;
    Ok(())
}

fn rebuild_pipeline(
    parts: &mut LoopParts<'_, '_>,
    current: &PipelineKnobs,
    new: &PipelineKnobs,
    cycle: bool,
) -> Result<(), String> {
    let builder = parts
        .pipeline
        .builder()
        .ok_or("this pipeline cannot rebuild the encoder")?;
    let mut next = probe_build(builder, new)?;

    let retargeted = new.area != current.area || new.fps != current.fps;
    if retargeted {
        match parts.capture.retarget(new.capture_target()) {
            Ok(()) => {}
            // A backend that cannot change its rate still serves a new encoder rate.
            Err(CaptureError::Unsupported(_)) if new.area == current.area => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    let size = new.geometry.size();
    if cycle || size != current.geometry.size() {
        if let Err(e) = parts.session.renegotiate(size, parts.cycle_timeout) {
            if retargeted {
                let _ = parts.capture.retarget(current.capture_target());
            }
            return Err(e.to_string());
        }
    }
    next.request_keyframe();
    parts.slot.replacement = Some(next);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::encode::prerecorded::PrerecordedEncoder;
    use crate::adapters::transport::hidraw::HidrawHandle;
    use crate::app::types::PipelineState;
    use crate::core::session::{SessionError, SessionSettings};
    use crate::ports::capture::{CaptureError, Frame, PixelFormat};
    use crate::ports::encode::EncoderSettings;
    use crate::protocol::display::DisplayGeometry;
    use crate::protocol::display::Orientation;
    use crate::protocol::framing::{parse_chunks, PictureSize};
    use std::fs::OpenOptions;
    use std::io::{Read, Seek, SeekFrom};
    use std::time::Duration;

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
            preset: crate::utils::config::X264Preset::Ultrafast,
            zerolatency: true,
            vbv_buffer_ms: 200,
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
            pipeline: PipelineState::new(
                PipelineKnobs {
                    bitrate_kbps: 1000,
                    vbv_buffer_ms: 200,
                    keyframe_interval: 2,
                    ..PipelineKnobs::for_geometry(geometry())
                },
                true,
            ),
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
        let first = parse_chunks(&[hb[..crate::protocol::framing::REPORT_SIZE].to_vec()]).unwrap();
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
