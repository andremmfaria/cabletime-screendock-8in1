//! CPU cost measurement for the capture plus encode path (Milestone 5 CPU criterion).
//!
//! [`run_bench`] drives a capture backend and an encoder back to back for a fixed time and
//! reports the process CPU time (user plus system, from `getrusage`) consumed by the loop
//! alongside wall time. The figure is process-wide, so it includes any helper threads the
//! encoder library starts. HID writes are not part of the measurement; the dock is not needed.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::capture::{CaptureBackend, CaptureError};
use crate::encode::{EncodeError, Encoder};

/// Errors that abort a measurement.
#[derive(Debug, Error)]
pub enum BenchError {
    #[error("capture failed: {0}")]
    Capture(#[from] CaptureError),
    #[error("encode failed: {0}")]
    Encode(#[from] EncodeError),
    #[error("getrusage failed: {0}")]
    Rusage(std::io::Error),
}

/// Process CPU time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub user: Duration,
    pub system: Duration,
}

impl CpuTimes {
    pub fn total(&self) -> Duration {
        self.user + self.system
    }

    /// Time consumed between `earlier` and `self`.
    pub fn since(&self, earlier: &CpuTimes) -> CpuTimes {
        CpuTimes {
            user: self.user.saturating_sub(earlier.user),
            system: self.system.saturating_sub(earlier.system),
        }
    }
}

fn timeval(tv: libc::timeval) -> Duration {
    Duration::new(tv.tv_sec.max(0) as u64, (tv.tv_usec.max(0) as u32) * 1000)
}

/// Reads the CPU time this process has used so far.
pub fn process_cpu_times() -> Result<CpuTimes, BenchError> {
    // SAFETY: `usage` is a plain-data out parameter that getrusage fully initializes on success.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
            return Err(BenchError::Rusage(std::io::Error::last_os_error()));
        }
        usage
    };
    Ok(CpuTimes {
        user: timeval(usage.ru_utime),
        system: timeval(usage.ru_stime),
    })
}

/// Result of one measurement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchReport {
    pub frames: u64,
    pub messages: u64,
    pub encoded_bytes: u64,
    pub wall: Duration,
    pub cpu: CpuTimes,
    /// Wall time spent inside `next_frame` (includes pacing waits).
    pub capture_wall: Duration,
    /// Wall time spent inside `encode`.
    pub encode_wall: Duration,
    /// Bytes of each frame's messages, in order.
    pub frame_bytes: Vec<u64>,
    /// Bytes of each keyframe message, in order.
    pub keyframe_bytes: Vec<u64>,
    /// Bytes of each non-keyframe message, in order.
    pub inter_bytes: Vec<u64>,
}

/// Summary of a list of sizes in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeStats {
    pub count: usize,
    pub min: u64,
    pub median: u64,
    pub p95: u64,
    pub max: u64,
}

impl SizeStats {
    pub fn of(sizes: &[u64]) -> Option<Self> {
        if sizes.is_empty() {
            return None;
        }
        let mut sorted = sizes.to_vec();
        sorted.sort_unstable();
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
        Some(Self {
            count: sorted.len(),
            min: sorted[0],
            median: at(0.5),
            p95: at(0.95),
            max: sorted[sorted.len() - 1],
        })
    }
}

/// HID reports needed for a message of `bytes` bytes (4080 payload bytes per report).
pub fn reports_for(bytes: u64) -> u64 {
    bytes
        .div_ceil(crate::protocol::framing::FIRST_CHUNK_PAYLOAD_MAX as u64)
        .max(1)
}

impl BenchReport {
    /// CPU use as a percentage of one core over the wall time.
    pub fn cpu_percent_of_one_core(&self) -> f64 {
        if self.wall.is_zero() {
            return 0.0;
        }
        self.cpu.total().as_secs_f64() / self.wall.as_secs_f64() * 100.0
    }

    /// Achieved frame rate.
    pub fn fps(&self) -> f64 {
        if self.wall.is_zero() {
            return 0.0;
        }
        self.frames as f64 / self.wall.as_secs_f64()
    }
}

impl fmt::Display for BenchReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "frames: {} ({:.2} fps), messages: {}, encoded bytes: {}",
            self.frames,
            self.fps(),
            self.messages,
            self.encoded_bytes
        )?;
        writeln!(f, "wall: {:.3} s", self.wall.as_secs_f64())?;
        writeln!(
            f,
            "cpu: {:.3} s (user {:.3} s, system {:.3} s) = {:.1}% of one core",
            self.cpu.total().as_secs_f64(),
            self.cpu.user.as_secs_f64(),
            self.cpu.system.as_secs_f64(),
            self.cpu_percent_of_one_core()
        )?;
        writeln!(
            f,
            "wall in capture: {:.3} s, wall in encode: {:.3} s",
            self.capture_wall.as_secs_f64(),
            self.encode_wall.as_secs_f64()
        )?;
        writeln!(
            f,
            "bit rate: {:.0} kbit/s over the measured wall time",
            self.encoded_bytes as f64 * 8.0 / 1000.0 / self.wall.as_secs_f64().max(f64::EPSILON)
        )?;
        let line = |f: &mut fmt::Formatter<'_>, label: &str, s: Option<SizeStats>| match s {
            Some(s) => writeln!(
                f,
                "{label}: n={} min={} median={} p95={} max={} bytes (max = {} reports)",
                s.count,
                s.min,
                s.median,
                s.p95,
                s.max,
                reports_for(s.max)
            ),
            None => writeln!(f, "{label}: none"),
        };
        line(f, "keyframe size", SizeStats::of(&self.keyframe_bytes))?;
        line(f, "inter frame size", SizeStats::of(&self.inter_bytes))?;
        line(f, "all frames size", SizeStats::of(&self.frame_bytes))
    }
}

/// Runs capture and encode for `duration` (or until `stop`), measuring CPU time over the loop.
/// Any capture or encode error aborts the run: a measurement with dropped frames is not
/// comparable.
pub fn run_bench(
    capture: &mut dyn CaptureBackend,
    encoder: &mut dyn Encoder,
    duration: Duration,
    stop: &AtomicBool,
) -> Result<BenchReport, BenchError> {
    let cpu_start = process_cpu_times()?;
    let start = Instant::now();
    let mut report = BenchReport {
        frames: 0,
        messages: 0,
        encoded_bytes: 0,
        wall: Duration::ZERO,
        cpu: CpuTimes::default(),
        capture_wall: Duration::ZERO,
        encode_wall: Duration::ZERO,
        frame_bytes: Vec::new(),
        keyframe_bytes: Vec::new(),
        inter_bytes: Vec::new(),
    };
    while start.elapsed() < duration && !stop.load(Ordering::Relaxed) {
        let t = Instant::now();
        let frame = capture.next_frame()?;
        report.capture_wall += t.elapsed();

        let t = Instant::now();
        let messages = encoder.encode(&frame)?;
        report.encode_wall += t.elapsed();

        report.frames += 1;
        report.messages += messages.len() as u64;
        let bytes: u64 = messages.iter().map(|m| m.as_bytes().len() as u64).sum();
        report.encoded_bytes += bytes;
        report.frame_bytes.push(bytes);
        for m in &messages {
            let len = m.as_bytes().len() as u64;
            if m.is_keyframe() {
                report.keyframe_bytes.push(len);
            } else {
                report.inter_bytes.push(len);
            }
        }
    }
    report.wall = start.elapsed();
    report.cpu = process_cpu_times()?.since(&cpu_start);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{Frame, PixelFormat};
    use crate::encode::prerecorded::PrerecordedEncoder;
    use crate::encode::EncoderSettings;
    use crate::protocol::display::{DisplayGeometry, Orientation};

    struct Paced;
    impl CaptureBackend for Paced {
        fn name(&self) -> &'static str {
            "paced"
        }
        fn next_frame(&mut self) -> Result<Frame, CaptureError> {
            std::thread::sleep(Duration::from_millis(5));
            Frame::new(2, 2, PixelFormat::Bgrx8888, 8, vec![0; 16])
        }
    }

    #[test]
    fn measures_frames_and_cpu_over_the_window() {
        let settings = EncoderSettings {
            geometry: DisplayGeometry::new(1280, 720, Orientation::Landscape).unwrap(),
            fps: 25,
            keyframe_interval: 2,
            bitrate_kbps: 1000,
            preset: crate::config::X264Preset::Ultrafast,
            zerolatency: true,
            vbv_buffer_ms: 200,
        };
        let mut encoder = PrerecordedEncoder::new(
            vec![vec![0, 0, 0, 1, 0x65, 1, 2, 3], vec![0, 0, 0, 1, 0x41, 4]],
            settings,
        )
        .unwrap();
        let stop = AtomicBool::new(false);
        let report =
            run_bench(&mut Paced, &mut encoder, Duration::from_millis(200), &stop).unwrap();
        assert!(report.frames >= 10, "{report}");
        assert_eq!(report.messages, report.frames);
        assert!(report.encoded_bytes >= 6 * report.frames);
        assert!(report.wall >= Duration::from_millis(200));
        // Sleeping dominates, so CPU use is far below one full core.
        assert!(report.cpu_percent_of_one_core() < 100.0);
        assert!(report.capture_wall > report.encode_wall);
    }

    #[test]
    fn percent_and_fps_handle_zero_wall_time() {
        let report = BenchReport {
            frames: 5,
            messages: 5,
            encoded_bytes: 0,
            wall: Duration::ZERO,
            cpu: CpuTimes::default(),
            capture_wall: Duration::ZERO,
            encode_wall: Duration::ZERO,
            frame_bytes: Vec::new(),
            keyframe_bytes: Vec::new(),
            inter_bytes: Vec::new(),
        };
        assert_eq!(report.cpu_percent_of_one_core(), 0.0);
        assert_eq!(report.fps(), 0.0);
    }

    #[test]
    fn size_stats_and_report_counts() {
        let stats = SizeStats::of(&[10, 40, 20, 30, 50]).unwrap();
        assert_eq!((stats.min, stats.median, stats.max), (10, 30, 50));
        assert!(SizeStats::of(&[]).is_none());
        assert_eq!(reports_for(1), 1);
        assert_eq!(reports_for(4080), 1);
        assert_eq!(reports_for(4081), 2);
    }

    #[test]
    fn cpu_time_difference_saturates() {
        let a = CpuTimes {
            user: Duration::from_millis(30),
            system: Duration::from_millis(10),
        };
        let b = CpuTimes {
            user: Duration::from_millis(50),
            system: Duration::from_millis(5),
        };
        let d = b.since(&a);
        assert_eq!(d.user, Duration::from_millis(20));
        assert_eq!(d.system, Duration::ZERO);
    }
}
