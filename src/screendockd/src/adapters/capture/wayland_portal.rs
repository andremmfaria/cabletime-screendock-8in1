//! xdg-desktop-portal ScreenCast plus PipeWire capture backend for Wayland sessions.
//!
//! The real implementation needs the `capture-wayland` cargo feature (ashpd, pipewire-rs and the
//! libpipewire development files). Without the feature the type still exists so the backend
//! selection code compiles everywhere, and [`WaylandPortalCapture::connect`] reports
//! `BackendUnavailable`.
//!
//! With the feature on, a worker thread negotiates the portal session (the compositor shows its
//! share dialog), then runs a PipeWire main loop that copies each buffer into a [`Frame`] and
//! pushes it into a bounded queue. The queue holds a few frames and drops the newest when the
//! consumer lags, so a slow encoder sees recent frames rather than a growing backlog.

use crate::ports::capture::{CaptureBackend, CaptureError, Frame};

#[cfg(not(feature = "capture-wayland"))]
pub struct WaylandPortalCapture;

#[cfg(not(feature = "capture-wayland"))]
impl WaylandPortalCapture {
    /// Always fails: this build lacks the `capture-wayland` feature.
    pub fn connect(_width: u32, _height: u32, _fps: u32) -> Result<Self, CaptureError> {
        Err(CaptureError::BackendUnavailable {
            backend: "wayland-portal",
            reason: "built without the capture-wayland feature".into(),
        })
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

#[cfg(feature = "capture-wayland")]
pub use enabled::WaylandPortalCapture;

#[cfg(feature = "capture-wayland")]
mod enabled {
    use std::os::fd::OwnedFd;
    use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
    use std::time::{Duration, Instant};

    use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
    use ashpd::desktop::PersistMode;
    use pipewire as pw;
    use pw::properties::properties;
    use pw::spa;

    use super::{CaptureBackend, CaptureError, Frame};
    use crate::ports::capture::PixelFormat;

    /// Frames buffered between the PipeWire thread and the consumer.
    const QUEUE_DEPTH: usize = 2;
    /// Upper bound advertised for the negotiated stream size.
    const MAX_DIMENSION: u32 = 8192;

    type FrameMessage = Result<Frame, CaptureError>;

    /// Receives frames from a portal ScreenCast session through PipeWire.
    pub struct WaylandPortalCapture {
        frames: Receiver<FrameMessage>,
    }

    struct StreamState {
        format: spa::param::video::VideoInfoRaw,
        tx: SyncSender<FrameMessage>,
        mainloop: pw::main_loop::MainLoopRc,
        interval: Duration,
        last_sent: Option<Instant>,
    }

    fn portal_err(e: impl std::fmt::Display) -> CaptureError {
        CaptureError::Portal(e.to_string())
    }

    fn stream_err(e: impl std::fmt::Display) -> CaptureError {
        CaptureError::Stream(e.to_string())
    }

    impl WaylandPortalCapture {
        /// Opens a portal session, waits for the user to pick a source and starts streaming.
        /// `width` and `height` are the preferred stream size; the compositor may pick another.
        pub fn connect(width: u32, height: u32, fps: u32) -> Result<Self, CaptureError> {
            let (ready_tx, ready_rx) = sync_channel::<Result<(), CaptureError>>(1);
            let (frame_tx, frame_rx) = sync_channel::<FrameMessage>(QUEUE_DEPTH);
            std::thread::Builder::new()
                .name("wayland-capture".into())
                .spawn(move || {
                    let outcome = run(width, height, fps, &ready_tx, &frame_tx);
                    if let Err(e) = outcome {
                        // During setup the ready channel carries the error. After setup the
                        // receiver is gone and the consumer gets it through the frame queue.
                        if let Err(
                            TrySendError::Disconnected(Err(e)) | TrySendError::Full(Err(e)),
                        ) = ready_tx.try_send(Err(e))
                        {
                            let _ = frame_tx.try_send(Err(e));
                        }
                    }
                })
                .map_err(CaptureError::Io)?;
            ready_rx
                .recv()
                .map_err(|_| CaptureError::Portal("capture thread exited during setup".into()))??;
            Ok(Self { frames: frame_rx })
        }
    }

    impl CaptureBackend for WaylandPortalCapture {
        fn name(&self) -> &'static str {
            "wayland-portal"
        }

        fn next_frame(&mut self) -> Result<Frame, CaptureError> {
            self.frames
                .recv()
                .map_err(|_| CaptureError::Stream("pipewire stream ended".into()))?
        }
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
        width: u32,
        height: u32,
        fps: u32,
        ready: &SyncSender<Result<(), CaptureError>>,
        frames: &SyncSender<FrameMessage>,
    ) -> Result<(), CaptureError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (node_id, fd) = runtime.block_on(open_portal())?;

        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(stream_err)?;
        let context = pw::context::ContextRc::new(&mainloop, None).map_err(stream_err)?;
        let core = context.connect_fd_rc(fd, None).map_err(stream_err)?;
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
            interval: if fps == 0 {
                Duration::ZERO
            } else {
                Duration::from_secs_f64(1.0 / f64::from(fps))
            },
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
                if let Some(last) = state.last_sent {
                    if last.elapsed() < state.interval {
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

        let _ = ready.try_send(Ok(()));
        mainloop.run();
        Err(CaptureError::Stream("pipewire main loop exited".into()))
    }
}
