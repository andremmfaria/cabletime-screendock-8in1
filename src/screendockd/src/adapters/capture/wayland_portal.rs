//! xdg-desktop-portal ScreenCast plus PipeWire capture backend for Wayland sessions.
//!
//! The real implementation needs the `capture-wayland` cargo feature (ashpd, pipewire-rs and the
//! libpipewire development files). Without the feature the type still exists so the backend
//! selection code compiles everywhere, and [`WaylandPortalCapture::connect`] reports
//! `BackendUnavailable`.
//!
//! With the feature on, a worker thread either negotiates the portal session (the compositor
//! shows its share dialog) or attaches to a given PipeWire node without a portal, then runs a
//! PipeWire main loop that copies each buffer into a [`Frame`] and pushes it into a bounded
//! queue. Retargeting switches between the two: a `Stream` area attaches to the node, `Whole`
//! opens the portal session. The queue holds a few frames and drops the newest when the
//! consumer lags, so a slow encoder sees recent frames rather than a growing backlog.

use crate::ports::capture::{CaptureBackend, CaptureError, Frame};

#[cfg(not(feature = "capture-wayland"))]
impl WaylandPortalCapture {
    /// Always fails: this build lacks the `capture-wayland` feature.
    pub fn connect(_width: u32, _height: u32, _fps: u32) -> Result<Self, CaptureError> {
        Err(CaptureError::BackendUnavailable {
            backend: "wayland-portal",
            reason: "built without the capture-wayland feature".into(),
        })
    }

    /// Always fails: this build lacks the `capture-wayland` feature.
    pub fn connect_stream(
        _remote: Option<&str>,
        _node: u32,
        _width: u32,
        _height: u32,
        _fps: u32,
    ) -> Result<Self, CaptureError> {
        Self::connect(0, 0, 0)
    }
}

#[cfg(not(feature = "capture-wayland"))]
impl CaptureBackend for WaylandPortalCapture {
    fn name(&self) -> &'static str {
        "wayland-portal"
    }

    fn next_frame(&mut self) -> Result<Frame, CaptureError> {
        Err(CaptureError::BackendUnavailable {
            backend: "wayland-portal",
            reason: "built without the capture-wayland feature".into(),
        })
    }
}

pub use super::types::WaylandPortalCapture;

#[cfg(feature = "capture-wayland")]
mod enabled {
    use std::os::fd::OwnedFd;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
    use ashpd::desktop::PersistMode;
    use pipewire as pw;
    use pw::properties::properties;
    use pw::spa;

    use super::{CaptureBackend, CaptureError, Frame};
    use crate::adapters::capture::types::{
        Attachment, FrameMessage, StreamState, WaylandPortalCapture, Worker, MAX_DIMENSION,
        QUEUE_DEPTH, STOP_POLL,
    };
    use crate::ports::capture::{CaptureArea, CaptureTarget, PixelFormat};

    fn portal_err(e: impl std::fmt::Display) -> CaptureError {
        CaptureError::Portal(e.to_string())
    }

    fn stream_err(e: impl std::fmt::Display) -> CaptureError {
        CaptureError::Stream(e.to_string())
    }

    fn interval_us(fps: u32) -> u64 {
        if fps == 0 {
            0
        } else {
            1_000_000 / u64::from(fps)
        }
    }

    impl WaylandPortalCapture {
        /// Opens a portal session, waits for the user to pick a source and starts streaming.
        /// `width` and `height` are the preferred stream size; the compositor may pick another.
        pub fn connect(width: u32, height: u32, fps: u32) -> Result<Self, CaptureError> {
            Self::attach(Attachment::Portal, None, (width, height), fps)
        }

        /// Starts streaming from the PipeWire node `node` without a portal session, asking
        /// the node for `width` by `height`. `remote` names the PipeWire socket; `None` uses
        /// the default one of the environment.
        pub fn connect_stream(
            remote: Option<&str>,
            node: u32,
            width: u32,
            height: u32,
            fps: u32,
        ) -> Result<Self, CaptureError> {
            Self::attach(
                Attachment::Stream {
                    node,
                    width,
                    height,
                },
                remote.map(str::to_string),
                (width, height),
                fps,
            )
        }

        fn attach(
            attachment: Attachment,
            remote: Option<String>,
            portal_size: (u32, u32),
            fps: u32,
        ) -> Result<Self, CaptureError> {
            let interval = Arc::new(AtomicU64::new(interval_us(fps)));
            let worker = spawn_worker(attachment, remote.clone(), portal_size, &interval)?;
            Ok(Self {
                worker,
                interval_us: interval,
                remote,
                portal_size,
            })
        }
    }

    impl Drop for WaylandPortalCapture {
        fn drop(&mut self) {
            self.worker.stop.store(true, Ordering::Relaxed);
        }
    }

    impl CaptureBackend for WaylandPortalCapture {
        fn name(&self) -> &'static str {
            "wayland-portal"
        }

        fn next_frame(&mut self) -> Result<Frame, CaptureError> {
            self.worker
                .frames
                .recv()
                .map_err(|_| CaptureError::Stream("pipewire stream ended".into()))?
        }

        fn retarget(&mut self, target: CaptureTarget) -> Result<(), CaptureError> {
            let wanted = match target.area {
                CaptureArea::Whole => Attachment::Portal,
                CaptureArea::Stream {
                    node,
                    width,
                    height,
                } => Attachment::Stream {
                    node,
                    width,
                    height,
                },
                CaptureArea::Region(_) => {
                    return Err(CaptureError::Unsupported(
                        "capturing a rectangle of a Wayland source",
                    ))
                }
            };
            self.interval_us
                .store(interval_us(target.fps), Ordering::Relaxed);
            if wanted == self.worker.attachment {
                return Ok(());
            }
            // The new consumer runs before the old one stops, so a failed attach leaves the
            // capture where it was.
            let worker = spawn_worker(
                wanted,
                self.remote.clone(),
                self.portal_size,
                &self.interval_us,
            )?;
            self.worker.stop.store(true, Ordering::Relaxed);
            self.worker = worker;
            Ok(())
        }
    }

    fn spawn_worker(
        attachment: Attachment,
        remote: Option<String>,
        portal_size: (u32, u32),
        interval: &Arc<AtomicU64>,
    ) -> Result<Worker, CaptureError> {
        let (ready_tx, ready_rx) = sync_channel::<Result<(), CaptureError>>(1);
        let (frame_tx, frame_rx) = sync_channel::<FrameMessage>(QUEUE_DEPTH);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let interval = Arc::clone(interval);
        std::thread::Builder::new()
            .name("wayland-capture".into())
            .spawn(move || {
                let outcome = run(
                    attachment,
                    remote.as_deref(),
                    portal_size,
                    &interval,
                    &thread_stop,
                    &ready_tx,
                    &frame_tx,
                );
                if let Err(e) = outcome {
                    // During setup the ready channel carries the error. After setup the
                    // receiver is gone and the consumer gets it through the frame queue.
                    if let Err(TrySendError::Disconnected(Err(e)) | TrySendError::Full(Err(e))) =
                        ready_tx.try_send(Err(e))
                    {
                        let _ = frame_tx.try_send(Err(e));
                    }
                }
            })
            .map_err(CaptureError::Io)?;
        ready_rx
            .recv()
            .map_err(|_| CaptureError::Portal("capture thread exited during setup".into()))??;
        Ok(Worker {
            frames: frame_rx,
            stop,
            attachment,
        })
    }

    /// Runs the portal handshake and returns the PipeWire node id and the remote fd.
    async fn open_portal() -> Result<(u32, OwnedFd), CaptureError> {
        let proxy = Screencast::new().await.map_err(portal_err)?;
        let session = proxy
            .create_session(Default::default())
            .await
            .map_err(portal_err)?;
        proxy
            .select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_cursor_mode(CursorMode::EMBEDDED)
                    .set_sources(SourceType::MONITOR)
                    .set_multiple(false)
                    .set_persist_mode(PersistMode::DoNot),
            )
            .await
            .map_err(portal_err)?;
        let response = proxy
            .start(&session, None, Default::default())
            .await
            .map_err(portal_err)?
            .response()
            .map_err(portal_err)?;
        let stream = response
            .streams()
            .first()
            .ok_or_else(|| CaptureError::Portal("portal returned no stream".into()))?;
        let node_id = stream.pipe_wire_node_id();
        let fd = proxy
            .open_pipe_wire_remote(&session, Default::default())
            .await
            .map_err(portal_err)?;
        Ok((node_id, fd))
    }

    fn map_format(format: spa::param::video::VideoFormat) -> Option<PixelFormat> {
        use spa::param::video::VideoFormat as V;
        if format == V::BGRx {
            Some(PixelFormat::Bgrx8888)
        } else if format == V::BGRA {
            Some(PixelFormat::Bgra8888)
        } else if format == V::RGBx {
            Some(PixelFormat::Rgbx8888)
        } else if format == V::RGBA {
            Some(PixelFormat::Rgba8888)
        } else {
            None
        }
    }

    fn run(
        attachment: Attachment,
        remote: Option<&str>,
        portal_size: (u32, u32),
        interval: &Arc<AtomicU64>,
        stop: &Arc<AtomicBool>,
        ready: &SyncSender<Result<(), CaptureError>>,
        frames: &SyncSender<FrameMessage>,
    ) -> Result<(), CaptureError> {
        let (node_id, fd, (width, height)) = match attachment {
            Attachment::Portal => {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let (node_id, fd) = runtime.block_on(open_portal())?;
                (node_id, Some(fd), portal_size)
            }
            Attachment::Stream {
                node,
                width,
                height,
            } => (node, None, (width, height)),
        };

        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(stream_err)?;
        let context = pw::context::ContextRc::new(&mainloop, None).map_err(stream_err)?;
        let core = match fd {
            Some(fd) => context.connect_fd_rc(fd, None).map_err(stream_err)?,
            None => {
                let props = remote.map(|name| properties! { *pw::keys::REMOTE_NAME => name });
                context.connect_rc(props).map_err(stream_err)?
            }
        };
        let stream = pw::stream::StreamBox::new(
            &core,
            "screendockd-capture",
            properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen",
            },
        )
        .map_err(stream_err)?;

        let state = StreamState {
            format: Default::default(),
            tx: frames.clone(),
            mainloop: mainloop.clone(),
            interval_us: Arc::clone(interval),
            last_sent: None,
        };

        let _listener = stream
            .add_local_listener_with_user_data(state)
            .param_changed(|_, state, id, param| {
                let Some(param) = param else { return };
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(param)
                else {
                    return;
                };
                if media_type != spa::param::format::MediaType::Video
                    || media_subtype != spa::param::format::MediaSubtype::Raw
                {
                    return;
                }
                if state.format.parse(param).is_err() {
                    let _ = state
                        .tx
                        .try_send(Err(CaptureError::Stream("unparseable video format".into())));
                    state.mainloop.quit();
                }
            })
            .process(|stream, state| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let interval = Duration::from_micros(state.interval_us.load(Ordering::Relaxed));
                if let Some(last) = state.last_sent {
                    if last.elapsed() < interval {
                        return;
                    }
                }
                let Some(data) = buffer.datas_mut().first_mut() else {
                    return;
                };
                let size = state.format.size();
                let Some(pixel_format) = map_format(state.format.format()) else {
                    let _ = state.tx.try_send(Err(CaptureError::InvalidFrame(
                        "negotiated pixel format is not supported".into(),
                    )));
                    state.mainloop.quit();
                    return;
                };
                let chunk_stride = data.chunk().stride();
                let chunk_offset = data.chunk().offset() as usize;
                let chunk_size = data.chunk().size() as usize;
                // A buffer without pixels (a cursor only update) carries nothing to encode.
                if chunk_size == 0 {
                    return;
                }
                let stride = if chunk_stride > 0 {
                    chunk_stride as usize
                } else {
                    size.width as usize * pixel_format.bytes_per_pixel()
                };
                let Some(bytes) = data.data() else { return };
                let Some(slice) = bytes.get(chunk_offset..chunk_offset + chunk_size) else {
                    return;
                };
                let message = Frame::new(
                    size.width,
                    size.height,
                    pixel_format,
                    stride,
                    slice.to_vec(),
                );
                match state.tx.try_send(message) {
                    Ok(()) => state.last_sent = Some(Instant::now()),
                    Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => state.mainloop.quit(),
                }
            })
            .register()
            .map_err(stream_err)?;

        let wanted = spa::pod::object!(
            spa::utils::SpaTypes::ObjectParamFormat,
            spa::param::ParamType::EnumFormat,
            spa::pod::property!(
                spa::param::format::FormatProperties::MediaType,
                Id,
                spa::param::format::MediaType::Video
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::MediaSubtype,
                Id,
                spa::param::format::MediaSubtype::Raw
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::VideoFormat,
                Choice,
                Enum,
                Id,
                spa::param::video::VideoFormat::BGRx,
                spa::param::video::VideoFormat::BGRx,
                spa::param::video::VideoFormat::BGRA,
                spa::param::video::VideoFormat::RGBx,
                spa::param::video::VideoFormat::RGBA,
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::VideoSize,
                Choice,
                Range,
                Rectangle,
                spa::utils::Rectangle { width, height },
                spa::utils::Rectangle {
                    width: 1,
                    height: 1
                },
                spa::utils::Rectangle {
                    width: MAX_DIMENSION,
                    height: MAX_DIMENSION
                }
            ),
        );
        let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &spa::pod::Value::Object(wanted),
        )
        .map_err(stream_err)?
        .0
        .into_inner();
        let pod = spa::pod::Pod::from_bytes(&values)
            .ok_or_else(|| CaptureError::Stream("could not build format pod".into()))?;
        let mut params = [pod];

        stream
            .connect(
                spa::utils::Direction::Input,
                Some(node_id),
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut params,
            )
            .map_err(stream_err)?;

        // The capture asks the loop to stop through a flag, because the loop runs on this
        // thread and the owner of the capture does not.
        let quitter = mainloop.clone();
        let stop = Arc::clone(stop);
        let timer = mainloop.loop_().add_timer(move |_| {
            if stop.load(Ordering::Relaxed) {
                quitter.quit();
            }
        });
        timer
            .update_timer(Some(STOP_POLL), Some(STOP_POLL))
            .into_result()
            .map_err(stream_err)?;

        let _ = ready.try_send(Ok(()));
        mainloop.run();
        Err(CaptureError::Stream("pipewire main loop exited".into()))
    }
}
