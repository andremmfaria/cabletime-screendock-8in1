//! XShm (MIT-SHM) capture backend for X11 sessions.
//!
//! Talks to the X server through `x11rb`, a pure-Rust client, so no X client libraries are
//! needed at build time. A memfd-backed segment is created once, passed to the server with
//! `ShmAttachFd`, and every frame is one `ShmGetImage` of the root window into that segment.
//! The whole root window is captured; scaling to the panel size is the encoder's job.

use std::fs::File;
use std::time::{Duration, Instant};

use memmap2::{MmapMut, MmapOptions};
use rustix::fs::{ftruncate, memfd_create, MemfdFlags};
use x11rb::connection::Connection;
use x11rb::protocol::shm::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder, Window};
use x11rb::rust_connection::RustConnection;

use crate::ports::capture::{CaptureBackend, CaptureError, Frame, PixelFormat};

/// Bytes per pixel the backend accepts (24-bit depth stored in 32-bit pixels).
const BYTES_PER_PIXEL: usize = 4;

/// Captures the root window of an X display through shared memory.
pub struct X11ShmCapture {
    conn: RustConnection,
    root: Window,
    seg: shm::Seg,
    map: MmapMut,
    width: u16,
    height: u16,
    interval: Duration,
    next_due: Option<Instant>,
}

fn conn_err(e: impl std::fmt::Display) -> CaptureError {
    CaptureError::Connection(e.to_string())
}

fn shm_err(e: impl std::fmt::Display) -> CaptureError {
    CaptureError::Shm(e.to_string())
}

impl X11ShmCapture {
    /// Connects to `display` (or `$DISPLAY` when `None`) and sets up the shared segment.
    /// `fps` paces [`CaptureBackend::next_frame`]; zero disables pacing.
    pub fn connect(display: Option<&str>, fps: u32) -> Result<Self, CaptureError> {
        let (conn, screen_num) = x11rb::connect(display).map_err(conn_err)?;
        let setup = conn.setup();
        let screen = &setup.roots[screen_num];

        if setup.image_byte_order != ImageOrder::LSB_FIRST {
            return Err(CaptureError::BackendUnavailable {
                backend: "x11-shm",
                reason: "server uses MSB-first image byte order".into(),
            });
        }
        let depth = screen
            .allowed_depths
            .iter()
            .find(|d| d.depth == screen.root_depth)
            .ok_or_else(|| conn_err("root depth missing from allowed depths"))?;
        let visual = depth
            .visuals
            .iter()
            .find(|v| v.visual_id == screen.root_visual)
            .ok_or_else(|| conn_err("root visual missing from allowed depths"))?;
        let bpp = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == screen.root_depth)
            .map(|f| f.bits_per_pixel);
        if bpp != Some(32)
            || visual.red_mask != 0x00ff_0000
            || visual.green_mask != 0x0000_ff00
            || visual.blue_mask != 0x0000_00ff
        {
            return Err(CaptureError::BackendUnavailable {
                backend: "x11-shm",
                reason: format!(
                    "root visual must be 32 bits per pixel with 0xff0000/0xff00/0xff masks \
                     (depth {}, bpp {:?}, masks {:#x}/{:#x}/{:#x})",
                    screen.root_depth, bpp, visual.red_mask, visual.green_mask, visual.blue_mask
                ),
            });
        }

        let version = conn
            .shm_query_version()
            .map_err(shm_err)?
            .reply()
            .map_err(|e| shm_err(format!("MIT-SHM extension unavailable: {e}")))?;
        if (version.major_version, version.minor_version) < (1, 2) {
            return Err(shm_err(format!(
                "MIT-SHM {}.{} lacks fd passing (1.2 needed)",
                version.major_version, version.minor_version
            )));
        }

        let root = screen.root;
        let geometry = conn
            .get_geometry(root)
            .map_err(conn_err)?
            .reply()
            .map_err(conn_err)?;
        let (width, height) = (geometry.width, geometry.height);
        let size = width as usize * height as usize * BYTES_PER_PIXEL;

        let fd = memfd_create("screendockd-shm", MemfdFlags::CLOEXEC).map_err(shm_err)?;
        ftruncate(&fd, size as u64).map_err(shm_err)?;
        let server_fd = fd.try_clone()?;
        let file = File::from(fd);
        // SAFETY: the mapping is of a private memfd created above. Only the X server shares it,
        // and every read happens after the reply to the request that filled it.
        let map = unsafe { MmapOptions::new().len(size).map_mut(&file)? };

        let seg = conn.generate_id().map_err(conn_err)?;
        conn.shm_attach_fd(seg, server_fd, false)
            .map_err(shm_err)?
            .check()
            .map_err(shm_err)?;

        Ok(Self {
            conn,
            root,
            seg,
            map,
            width,
            height,
            interval: if fps == 0 {
                Duration::ZERO
            } else {
                Duration::from_secs_f64(1.0 / f64::from(fps))
            },
            next_due: None,
        })
    }

    fn pace(&mut self) {
        if self.interval.is_zero() {
            return;
        }
        if let Some(due) = self.next_due {
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
        }
        // Anchor the next deadline to the actual grab time so a slow consumer does not cause a
        // burst of catch-up frames.
        self.next_due = Some(Instant::now() + self.interval);
    }
}

impl CaptureBackend for X11ShmCapture {
    fn name(&self) -> &'static str {
        "x11-shm"
    }

    fn next_frame(&mut self) -> Result<Frame, CaptureError> {
        self.pace();
        let reply = self
            .conn
            .shm_get_image(
                self.root,
                0,
                0,
                self.width,
                self.height,
                u32::MAX,
                ImageFormat::Z_PIXMAP.into(),
                self.seg,
                0,
            )
            .map_err(shm_err)?
            .reply()
            .map_err(shm_err)?;
        let stride = self.width as usize * BYTES_PER_PIXEL;
        let needed = stride * self.height as usize;
        if reply.size as usize != needed {
            return Err(CaptureError::InvalidFrame(format!(
                "server wrote {} bytes, expected {needed}",
                reply.size
            )));
        }
        Frame::new(
            u32::from(self.width),
            u32::from(self.height),
            PixelFormat::Bgrx8888,
            stride,
            self.map[..needed].to_vec(),
        )
    }
}

impl Drop for X11ShmCapture {
    fn drop(&mut self) {
        // Best effort: the server also releases the segment when the connection closes.
        let _ = self.conn.shm_detach(self.seg);
        let _ = self.conn.flush();
    }
}
