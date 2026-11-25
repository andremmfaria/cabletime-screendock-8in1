//! Direct `write(2)`/`read(2)` I/O against the resolved MI_00/MI_01 `/dev/hidrawN` nodes.
//!
//! This intentionally does *not* use the `hidapi` crate or any `ioctl`/"set report" call: each chunk is one plain `write(2)`
//! on the node, back-to-back, with no buffering layer in between.
//!
//! Design notes:
//! - (pattern 6, RAII) `HidrawHandle` owns a `std::fs::File`, which already closes its fd on
//!   drop; no manual `Drop` is added and no method exposes the raw fd.
//! - (pattern 5, parse at the boundary) the read side runs every inbound report through
//!   `framing::parse_chunks` and only hands back a typed `ParsedMessage`.
//! - (pattern 8, mechanism/policy) this module moves already-framed buffers and never
//!   interprets header field meanings beyond checking the Report ID matches the channel the
//!   handle was opened for.
//! - (pattern 7, errors) `TransportError` is a typed enum. Fail-fast vs recoverable is the
//!   caller's decision: every failed write is returned with its chunk index so a caller can
//!   decide whether to abort (startup, validation tools) or drop the message and continue
//!   (steady-state loops).

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use super::discovery::{Mi00Path, Mi01Path};
use crate::ports::transport::{DockTransport, TransportError, WriteTimings};
use crate::protocol::framing::{
    parse_chunks, FrameHeader, FramingError, ParsedMessage, ReportId,
    CONTINUATION_CHUNK_PAYLOAD_MAX, FIRST_CHUNK_PAYLOAD_MAX, HEADER_SIZE, REPORT_SIZE,
};

/// An open hidraw node for one of the dock's two channels.
///
/// The channel (`ReportId::Control` for MI_00, `ReportId::Video` for MI_01) is fixed at open
/// time and every written chunk is checked against it, so a video chunk can never be written to
/// the control node (or vice versa) by accident.
#[derive(Debug)]
pub struct HidrawHandle {
    file: File,
    channel: ReportId,
}

impl HidrawHandle {
    /// Opens the resolved MI_00 node (control/heartbeat channel).
    pub fn open_mi00(path: &Mi00Path) -> Result<Self, TransportError> {
        Self::open(path.as_path(), ReportId::Control)
    }

    /// Opens the resolved MI_01 node (video channel).
    pub fn open_mi01(path: &Mi01Path) -> Result<Self, TransportError> {
        Self::open(path.as_path(), ReportId::Video)
    }

    fn open(path: &Path, channel: ReportId) -> Result<Self, TransportError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|source| TransportError::Open {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Self { file, channel })
    }

    /// Wraps an already-open file standing in for a hidraw node (regular file, pipe, pty
    /// slave). Used by loopback tests and by the validation tools' dry-run paths.
    pub fn from_file(file: File, channel: ReportId) -> Self {
        Self { file, channel }
    }

    /// Duplicates the descriptor, so a reader thread and a writer can each own a handle to the
    /// same node. Every message on the control channel is one report, so single `write(2)`
    /// calls from different handles never interleave inside a message.
    pub fn try_clone(&self) -> Result<Self, TransportError> {
        let file = self.file.try_clone().map_err(TransportError::Clone)?;
        Ok(Self {
            file,
            channel: self.channel,
        })
    }

    /// Waits up to `timeout` for the node to become readable. Returns true when a read will
    /// not block (data, end of stream or an error condition is pending), so a reader thread can
    /// notice a stop request instead of blocking in `read(2)` forever.
    pub fn wait_readable(&self, timeout: Duration) -> Result<bool, TransportError> {
        use std::os::fd::AsRawFd;
        let mut pfd = libc::pollfd {
            fd: self.file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        loop {
            // SAFETY: `pfd` is a valid, initialized pollfd and the count passed is 1.
            let rc = unsafe { libc::poll(&mut pfd, 1, millis) };
            if rc >= 0 {
                return Ok(rc > 0);
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::Interrupted {
                return Err(TransportError::Poll(err));
            }
        }
    }

    /// Reads one raw report (one `read(2)`) without interpreting it. End of stream is an
    /// error.
    pub fn read_report(&mut self) -> Result<Vec<u8>, TransportError> {
        self.read_chunk(0)
    }

    /// Test-only access to the underlying file, for rewinding a loopback file between a write
    /// and the matching read.
    #[cfg(test)]
    pub(crate) fn file_for_test(&mut self) -> &mut File {
        &mut self.file
    }

    /// The channel this handle was opened for.
    pub fn channel(&self) -> ReportId {
        self.channel
    }

    /// Writes each chunk with one `write(2)` call, in order and back-to-back (no delay, no
    /// intermediate buffering), and returns per-chunk timing. Every chunk is validated
    /// (exactly `REPORT_SIZE` bytes, Report ID matching this handle's channel) before the
    /// first byte of the message is written, so a malformed message is rejected whole rather
    /// than half-sent.
    pub fn write_chunks(&mut self, chunks: &[Vec<u8>]) -> Result<WriteTimings, TransportError> {
        for (index, chunk) in chunks.iter().enumerate() {
            if chunk.len() != REPORT_SIZE {
                return Err(TransportError::BadChunkSize {
                    index,
                    len: chunk.len(),
                });
            }
            if chunk[0] != self.channel.as_u8() {
                return Err(TransportError::WrongChannel {
                    index,
                    channel: self.channel,
                    got: chunk[0],
                });
            }
        }

        let mut per_chunk = Vec::with_capacity(chunks.len());
        let message_start = Instant::now();
        for (index, chunk) in chunks.iter().enumerate() {
            let start = Instant::now();
            // `File::write` is a single write(2); `write_all` would hide short writes by
            // retrying, and a short write on an interrupt-OUT report is a protocol error.
            let written = self
                .file
                .write(chunk)
                .map_err(|source| TransportError::Write { index, source })?;
            per_chunk.push(start.elapsed());
            if written != chunk.len() {
                return Err(TransportError::ShortWrite {
                    index,
                    written,
                    expected: chunk.len(),
                });
            }
        }
        Ok(WriteTimings {
            per_chunk,
            total: message_start.elapsed(),
        })
    }

    /// Reads one report (one `read(2)`), truncating to the bytes returned.
    fn read_chunk(&mut self, index: usize) -> Result<Vec<u8>, TransportError> {
        let mut buf = vec![0u8; REPORT_SIZE];
        let n = loop {
            match self.file.read(&mut buf) {
                Ok(n) => break n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(TransportError::Read(e)),
            }
        };
        if n == 0 {
            return Err(TransportError::UnexpectedEof { index });
        }
        buf.truncate(n);
        Ok(buf)
    }

    /// Reads and parses one complete message: the first report's header says how many bytes
    /// follow, so exactly the continuation reports needed are read, then the whole set is
    /// handed to `parse_chunks`. Only the typed result leaves this module.
    ///
    /// Blocks until a full message arrives. A report shorter than its meaningful bytes is
    /// zero-extended before parsing, since a device (or loopback) may return only the
    /// meaningful bytes of a report.
    pub fn read_message(&mut self) -> Result<ParsedMessage, TransportError> {
        let mut first = self.read_chunk(0)?;
        if first.len() < HEADER_SIZE {
            return Err(FramingError::HeaderTooShort { len: first.len() }.into());
        }
        let header = FrameHeader::from_bytes(&first)?;
        let total_len = header.payload_len as usize;
        let first_payload = total_len.min(FIRST_CHUNK_PAYLOAD_MAX);
        if first.len() < HEADER_SIZE + first_payload {
            first.resize(HEADER_SIZE + first_payload, 0);
        }

        let mut chunks = vec![first];
        let mut remaining = total_len - first_payload;
        while remaining > 0 {
            let index = chunks.len();
            let mut chunk = self.read_chunk(index)?;
            let take = remaining.min(CONTINUATION_CHUNK_PAYLOAD_MAX);
            if chunk.len() < HEADER_SIZE + take {
                chunk.resize(HEADER_SIZE + take, 0);
            }
            remaining -= take;
            chunks.push(chunk);
        }
        Ok(parse_chunks(&chunks)?)
    }
}

impl DockTransport for HidrawHandle {
    fn channel(&self) -> ReportId {
        HidrawHandle::channel(self)
    }

    fn write_chunks(&mut self, chunks: &[Vec<u8>]) -> Result<WriteTimings, TransportError> {
        HidrawHandle::write_chunks(self, chunks)
    }

    fn read_report(&mut self) -> Result<Vec<u8>, TransportError> {
        HidrawHandle::read_report(self)
    }

    fn read_message(&mut self) -> Result<ParsedMessage, TransportError> {
        HidrawHandle::read_message(self)
    }

    fn wait_readable(&self, timeout: Duration) -> Result<bool, TransportError> {
        HidrawHandle::wait_readable(self, timeout)
    }

    fn try_clone(&self) -> Result<Box<dyn DockTransport>, TransportError> {
        Ok(Box::new(HidrawHandle::try_clone(self)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::framing::{frame_message, MessageBuilder, PictureSize};
    use std::io::{Seek, SeekFrom};
    use std::os::fd::OwnedFd;
    use std::path::PathBuf;

    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    fn scratch_file(dir: &tempfile::TempDir) -> (File, PathBuf) {
        let path = dir.path().join("loop");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        (file, path)
    }

    #[test]
    fn write_then_read_round_trips_multi_chunk_message_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let (file, _) = scratch_file(&dir);
        let mut handle = HidrawHandle::from_file(file, ReportId::Video);

        let data = payload(67_666);
        let chunks = MessageBuilder::video(PictureSize::new(1280, 720))
            .payload(&data)
            .build();
        assert_eq!(chunks.len(), 17);
        let timings = handle.write_chunks(&chunks).unwrap();
        assert_eq!(timings.per_chunk.len(), 17);

        // Rewind the shared file offset so the read side sees what was written.
        handle.file_for_test().seek(SeekFrom::Start(0)).unwrap();
        let parsed = handle.read_message().unwrap();
        assert_eq!(parsed.report_id, ReportId::Video);
        assert_eq!(parsed.size, PictureSize::new(1280, 720));
        assert_eq!(parsed.payload, data);
    }

    #[test]
    fn writes_arrive_whole_through_a_pipe() {
        let (reader, writer) = std::io::pipe().unwrap();
        let mut handle =
            HidrawHandle::from_file(File::from(OwnedFd::from(writer)), ReportId::Control);

        // 3 chunks of 4096 fit the pipe buffer, but drain concurrently like a real consumer.
        let drain = std::thread::spawn(move || {
            let mut reader = reader;
            let mut all = Vec::new();
            reader.read_to_end(&mut all).unwrap();
            all
        });

        let chunks = frame_message(ReportId::Control, PictureSize::zero(), &payload(9_000));
        handle.write_chunks(&chunks).unwrap();
        drop(handle);
        let bytes = drain.join().unwrap();
        assert_eq!(bytes.len(), chunks.len() * REPORT_SIZE);
        assert_eq!(bytes[..REPORT_SIZE], chunks[0][..]);
    }

    #[test]
    fn rejects_wrong_channel_before_writing_anything() {
        let dir = tempfile::tempdir().unwrap();
        let (file, path) = scratch_file(&dir);
        let mut handle = HidrawHandle::from_file(file, ReportId::Control);

        let mut chunks = MessageBuilder::control().payload(&payload(5_000)).build();
        // Corrupt the second chunk's Report ID: the first chunk must not be written either.
        chunks[1][0] = ReportId::Video.as_u8();
        let err = handle.write_chunks(&chunks).unwrap_err();
        assert!(matches!(err, TransportError::WrongChannel { index: 1, .. }));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
    }

    #[test]
    fn rejects_wrong_sized_chunk() {
        let dir = tempfile::tempdir().unwrap();
        let (file, _) = scratch_file(&dir);
        let mut handle = HidrawHandle::from_file(file, ReportId::Control);
        let err = handle.write_chunks(&[vec![1u8; 10]]).unwrap_err();
        assert!(matches!(
            err,
            TransportError::BadChunkSize { index: 0, len: 10 }
        ));
    }

    #[test]
    fn read_reports_eof_on_truncated_stream() {
        let dir = tempfile::tempdir().unwrap();
        let (file, _) = scratch_file(&dir);
        let mut handle = HidrawHandle::from_file(file, ReportId::Video);
        let chunks = MessageBuilder::video(PictureSize::new(1280, 720))
            .payload(&payload(9_000))
            .build();
        // Write only the first chunk of a 3-chunk message.
        handle.write_chunks(&chunks[..1]).unwrap();
        handle.file_for_test().seek(SeekFrom::Start(0)).unwrap();
        let err = handle.read_message().unwrap_err();
        assert!(matches!(err, TransportError::UnexpectedEof { index: 1 }));
    }

    #[test]
    fn open_reports_missing_node() {
        let err = HidrawHandle::open_mi01(&Mi01Path::new(PathBuf::from("/nonexistent/hidraw9")))
            .unwrap_err();
        assert!(matches!(err, TransportError::Open { .. }));
    }
}
