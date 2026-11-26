//! Types of the capture adapters: session detection and backend selection values and the
//! X11 backend.

#[cfg(feature = "capture-wayland")]
use std::sync::atomic::{AtomicBool, AtomicU64};
#[cfg(feature = "capture-wayland")]
use std::sync::mpsc::{Receiver, SyncSender};
#[cfg(feature = "capture-wayland")]
use std::sync::Arc;
use std::time::{Duration, Instant};

use memmap2::MmapMut;
#[cfg(feature = "capture-wayland")]
use pipewire as pw;
#[cfg(feature = "capture-wayland")]
use pw::spa;
use x11rb::protocol::shm;
use x11rb::protocol::xproto::Window;
use x11rb::rust_connection::RustConnection;

#[cfg(feature = "capture-wayland")]
use crate::ports::capture::{CaptureError, Frame};

/// Kind of graphical session the daemon runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionType {
    X11,
    Wayland,
    Unknown,
}

/// Concrete backend selected by [`super::select_backend`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    X11Shm,
    WaylandPortal,
}

/// Bytes per pixel the backend accepts (24-bit depth stored in 32-bit pixels).
pub(super) const BYTES_PER_PIXEL: usize = 4;

/// Captures the root window of an X display through shared memory.
pub struct X11ShmCapture {
    pub(super) conn: RustConnection,
    pub(super) root: Window,
    pub(super) seg: shm::Seg,
    pub(super) map: MmapMut,
    /// Size of the captured rectangle.
    pub(super) width: u16,
    pub(super) height: u16,
    /// Top left corner of the captured rectangle in the root window.
    pub(super) origin: (i16, i16),
    pub(super) interval: Duration,
    pub(super) next_due: Option<Instant>,
}

/// Placeholder when the `capture-wayland` feature is off: the type exists so backend selection
/// compiles everywhere, and constructing it reports `BackendUnavailable`.
#[cfg(not(feature = "capture-wayland"))]
pub struct WaylandPortalCapture;

/// Frames buffered between the PipeWire thread and the consumer.
#[cfg(feature = "capture-wayland")]
pub(super) const QUEUE_DEPTH: usize = 2;
/// How often a PipeWire consumer checks whether it was asked to stop.
#[cfg(feature = "capture-wayland")]
pub(super) const STOP_POLL: Duration = Duration::from_millis(100);
/// Upper bound advertised for the negotiated stream size.
#[cfg(feature = "capture-wayland")]
pub(super) const MAX_DIMENSION: u32 = 8192;

#[cfg(feature = "capture-wayland")]
pub(super) type FrameMessage = Result<Frame, CaptureError>;

/// Where a Wayland capture gets its frames.
#[cfg(feature = "capture-wayland")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Attachment {
    /// A portal ScreenCast session of the whole desktop.
    Portal,
    /// A PipeWire node asked for a size, with no portal involved.
    Stream { node: u32, width: u32, height: u32 },
}

/// A running PipeWire consumer thread and the means to steer and stop it.
#[cfg(feature = "capture-wayland")]
pub(super) struct Worker {
    pub(super) frames: Receiver<FrameMessage>,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) attachment: Attachment,
}

/// Receives frames from a portal ScreenCast session or from a PipeWire node through PipeWire.
#[cfg(feature = "capture-wayland")]
pub struct WaylandPortalCapture {
    pub(super) worker: Worker,
    /// Minimum time between delivered frames in microseconds; zero disables pacing.
    pub(super) interval_us: Arc<AtomicU64>,
    /// The PipeWire remote to connect nodes on; `None` uses the default one.
    pub(super) remote: Option<String>,
    /// The size asked for when a portal session is opened.
    pub(super) portal_size: (u32, u32),
}

#[cfg(feature = "capture-wayland")]
pub(super) struct StreamState {
    pub(super) format: spa::param::video::VideoInfoRaw,
    pub(super) tx: SyncSender<FrameMessage>,
    pub(super) mainloop: pw::main_loop::MainLoopRc,
    pub(super) interval_us: Arc<AtomicU64>,
    pub(super) last_sent: Option<Instant>,
}
