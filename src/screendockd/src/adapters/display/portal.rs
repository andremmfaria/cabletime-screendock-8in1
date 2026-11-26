//! The portal display: a virtual monitor from the desktop portal's virtual source type.
//!
//! The portal decides where the monitor sits and how large it is; the stream size it reports
//! is the size of the monitor. A start that does not finish within the timeout is an error,
//! which the monitor manager turns into mirroring under the `mirror` fallback. The restore
//! token of a finished start is kept below `$XDG_STATE_HOME`, so the next start can skip the
//! portal's prompt.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use super::types::{
    PortalMonitor, PortalRequest, PortalSource, PortalVirtualDisplay, TokenStore,
    DEFAULT_PORTAL_TIMEOUT, MAX_TOKEN_LEN, TOKEN_FILE,
};
use crate::ports::display::{MonitorSpec, Rect, VdisplayError, VirtualDisplay, VirtualMonitor};

impl TokenStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The token file below the user's state directory.
    pub fn default_location() -> Option<Self> {
        dirs::state_dir().map(|dir| Self::new(dir.join("screendockd").join(TOKEN_FILE)))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The stored token, or `None` when the file is missing or holds anything but a short
    /// printable token.
    pub fn load(&self) -> Option<String> {
        let text = fs::read_to_string(&self.path).ok()?;
        let token = text.trim();
        let printable = token.chars().all(|c| c.is_ascii_graphic());
        (!token.is_empty() && token.len() <= MAX_TOKEN_LEN && printable).then(|| token.to_string())
    }

    /// Replaces the stored token. The file is private to the user.
    pub fn save(&self, token: &str) -> io::Result<()> {
        let dir = self.path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "token path has no directory")
        })?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let tmp = self.path.with_extension("tmp");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(token.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, &self.path)
    }
}

impl PortalVirtualDisplay {
    pub fn new(source: Arc<dyn PortalSource>) -> Self {
        Self {
            source,
            timeout: DEFAULT_PORTAL_TIMEOUT,
            tokens: TokenStore::default_location(),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Keeps the restore token at `store`; `None` keeps none.
    pub fn with_token_store(mut self, store: Option<TokenStore>) -> Self {
        self.tokens = store;
        self
    }
}

impl VirtualDisplay for PortalVirtualDisplay {
    fn name(&self) -> &'static str {
        "portal"
    }

    fn create(&self, spec: &MonitorSpec) -> Result<Box<dyn VirtualMonitor>, VdisplayError> {
        let request = PortalRequest {
            size: (spec.rect.width, spec.rect.height),
            restore_token: self.tokens.as_ref().and_then(TokenStore::load),
        };
        let source = Arc::clone(&self.source);
        let size = request.size;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("portal-start".into())
            .spawn(move || {
                // A stream that arrives after the timeout is dropped here, which closes its
                // portal session.
                let _ = tx.send(source.start(&request));
            })
            .map_err(|e| VdisplayError::Request(e.to_string()))?;
        let stream = match rx.recv_timeout(self.timeout) {
            Ok(result) => result?,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(VdisplayError::Unsupported(format!(
                    "the portal did not start a stream within {} s",
                    self.timeout.as_secs()
                )))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(VdisplayError::Request(
                    "the portal start ended without an answer".into(),
                ))
            }
        };
        if let (Some(store), Some(token)) = (&self.tokens, &stream.restore_token) {
            if let Err(e) = store.save(token) {
                tracing::warn!(error = %e, "could not keep the portal restore token");
            }
        }
        let (width, height) = stream.size.unwrap_or(size);
        Ok(Box::new(PortalMonitor {
            rect: Rect::new(spec.rect.x, spec.rect.y, width, height),
            node_id: stream.node_id,
            _stream: stream.keepalive,
        }))
    }
}

impl VirtualMonitor for PortalMonitor {
    fn rect(&self) -> Rect {
        self.rect
    }

    /// The portal owns the size of its monitor: a different size is refused so the caller
    /// recreates the monitor instead.
    fn resize(&mut self, spec: &MonitorSpec) -> Result<(), VdisplayError> {
        if (spec.rect.width, spec.rect.height) == (self.rect.width, self.rect.height) {
            self.rect.x = spec.rect.x;
            self.rect.y = spec.rect.y;
            Ok(())
        } else {
            Err(VdisplayError::Unsupported(
                "a portal monitor cannot change size".into(),
            ))
        }
    }

    fn pipewire_node(&self) -> Option<u32> {
        Some(self.node_id)
    }
}

/// The source type bits the portal offers, or `None` when no portal answers within the
/// timeout. Bit 4 is the virtual source type.
#[cfg(feature = "capture-wayland")]
pub fn probe_source_types(timeout: Duration) -> Option<u32> {
    use super::types::AshpdPortalSource;
    AshpdPortalSource.available_source_types(timeout)
}

/// Without the `capture-wayland` feature no portal is asked.
#[cfg(not(feature = "capture-wayland"))]
pub fn probe_source_types(_timeout: Duration) -> Option<u32> {
    None
}

#[cfg(feature = "capture-wayland")]
mod ashpd_source {
    use std::sync::mpsc;
    use std::time::Duration;

    use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
    use ashpd::desktop::PersistMode;

    use super::super::types::AshpdPortalSource;
    use super::super::types::PortalStream;
    use super::{PortalRequest, PortalSource, VdisplayError};

    fn request_err(e: impl std::fmt::Display) -> VdisplayError {
        VdisplayError::Request(format!("portal: {e}"))
    }

    /// Closes the portal session when dropped: the worker thread that owns the session
    /// leaves its wait and closes it.
    struct SessionGuard(mpsc::Sender<()>);

    impl Drop for SessionGuard {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    impl AshpdPortalSource {
        /// The source types the portal offers, asked on a worker thread so a missing bus
        /// or portal cannot hold the caller longer than `timeout`.
        pub fn available_source_types(&self, timeout: Duration) -> Option<u32> {
            let (tx, rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("portal-probe".into())
                .spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .ok()?;
                    let types = runtime.block_on(async {
                        let proxy = Screencast::new().await.ok()?;
                        proxy.available_source_types().await.ok()
                    });
                    let _ = tx.send(types.map(|t| t.bits()));
                    Some(())
                })
                .ok()?;
            rx.recv_timeout(timeout).ok().flatten()
        }
    }

    impl PortalSource for AshpdPortalSource {
        fn start(&self, request: &PortalRequest) -> Result<PortalStream, VdisplayError> {
            let token = request.restore_token.clone();
            let (ready_tx, ready_rx) = mpsc::channel();
            let (stop_tx, stop_rx) = mpsc::channel::<()>();
            std::thread::Builder::new()
                .name("portal-session".into())
                .spawn(move || {
                    let runtime = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(runtime) => runtime,
                        Err(e) => {
                            let _ = ready_tx.send(Err(request_err(e)));
                            return;
                        }
                    };
                    runtime.block_on(async move {
                        let opened = open(token.as_deref()).await;
                        match opened {
                            Ok((proxy, session, stream)) => {
                                let _ = ready_tx.send(Ok(stream));
                                // Blocks until the guard drops; the session lives meanwhile.
                                let _ = tokio::task::spawn_blocking(move || stop_rx.recv()).await;
                                let _ = session.close().await;
                                drop(proxy);
                            }
                            Err(e) => {
                                let _ = ready_tx.send(Err(e));
                            }
                        }
                    });
                })
                .map_err(request_err)?;
            let mut stream = ready_rx
                .recv()
                .map_err(|_| request_err("portal thread ended during setup"))??;
            stream.keepalive = Box::new(SessionGuard(stop_tx));
            Ok(stream)
        }
    }

    type Opened = (
        Screencast,
        ashpd::desktop::Session<Screencast>,
        PortalStream,
    );

    async fn open(token: Option<&str>) -> Result<Opened, VdisplayError> {
        let proxy = Screencast::new().await.map_err(request_err)?;
        let offered = proxy.available_source_types().await.map_err(request_err)?;
        if !offered.contains(SourceType::VIRTUAL) {
            return Err(VdisplayError::Unsupported(
                "portal reports no virtual source type".into(),
            ));
        }
        let session = proxy
            .create_session(Default::default())
            .await
            .map_err(request_err)?;
        proxy
            .select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_cursor_mode(CursorMode::EMBEDDED)
                    .set_sources(SourceType::VIRTUAL)
                    .set_multiple(false)
                    .set_persist_mode(PersistMode::ExplicitlyRevoked)
                    .set_restore_token(token),
            )
            .await
            .map_err(request_err)?;
        let response = proxy
            .start(&session, None, Default::default())
            .await
            .map_err(request_err)?
            .response()
            .map_err(request_err)?;
        let first = response
            .streams()
            .first()
            .ok_or_else(|| request_err("portal returned no stream"))?;
        let size = first
            .size()
            .and_then(|(w, h)| Some((u32::try_from(w).ok()?, u32::try_from(h).ok()?)));
        let stream = PortalStream {
            node_id: first.pipe_wire_node_id(),
            size,
            restore_token: response.restore_token().map(str::to_string),
            keepalive: Box::new(()),
        };
        Ok((proxy, session, stream))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    use super::super::types::PortalStream;
    use super::*;
    use crate::ports::display::MonitorName;

    struct Closed(Arc<AtomicBool>);

    impl Drop for Closed {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// A portal that answers with a fixed stream and records what it was asked.
    struct Mock {
        seen: Mutex<Vec<PortalRequest>>,
        closed: Arc<AtomicBool>,
        size: Option<(u32, u32)>,
        delay: Duration,
    }

    impl Mock {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                seen: Mutex::new(Vec::new()),
                closed: Arc::new(AtomicBool::new(false)),
                size: None,
                delay: Duration::ZERO,
            })
        }
    }

    impl PortalSource for Mock {
        fn start(&self, request: &PortalRequest) -> Result<PortalStream, VdisplayError> {
            std::thread::sleep(self.delay);
            self.seen.lock().unwrap().push(request.clone());
            Ok(PortalStream {
                node_id: 77,
                size: self.size,
                restore_token: Some("next-token".into()),
                keepalive: Box::new(Closed(Arc::clone(&self.closed))),
            })
        }
    }

    fn spec() -> MonitorSpec {
        MonitorSpec::new(MonitorName::new(""), Rect::new(1920, 0, 1280, 720))
    }

    fn display(mock: Arc<Mock>, dir: &Path) -> PortalVirtualDisplay {
        PortalVirtualDisplay::new(mock)
            .with_token_store(Some(TokenStore::new(dir.join("state").join("token"))))
    }

    #[test]
    fn portal_monitor_reports_the_stream_node_and_the_requested_place() {
        let dir = tempfile::tempdir().unwrap();
        let mock = Mock::new();
        let monitor = display(Arc::clone(&mock), dir.path())
            .create(&spec())
            .unwrap();
        assert_eq!(monitor.pipewire_node(), Some(77));
        assert_eq!(monitor.rect(), Rect::new(1920, 0, 1280, 720));
        assert_eq!(mock.seen.lock().unwrap()[0].size, (1280, 720));
    }

    #[test]
    fn portal_monitor_takes_the_size_the_stream_reports() {
        let dir = tempfile::tempdir().unwrap();
        let mock = Arc::new(Mock {
            size: Some((1024, 600)),
            ..Arc::try_unwrap(Mock::new()).ok().unwrap()
        });
        let monitor = display(mock, dir.path()).create(&spec()).unwrap();
        assert_eq!(monitor.rect(), Rect::new(1920, 0, 1024, 600));
    }

    #[test]
    fn restore_token_is_presented_again_and_replaced_after_a_start() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::new(dir.path().join("state").join("token"));
        store.save("first-token").unwrap();
        let mock = Mock::new();
        display(Arc::clone(&mock), dir.path())
            .create(&spec())
            .unwrap();
        assert_eq!(
            mock.seen.lock().unwrap()[0].restore_token.as_deref(),
            Some("first-token")
        );
        assert_eq!(store.load().as_deref(), Some("next-token"));
        let mode = fs::metadata(store.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn dropping_the_monitor_closes_the_portal_session() {
        let dir = tempfile::tempdir().unwrap();
        let mock = Mock::new();
        let monitor = display(Arc::clone(&mock), dir.path())
            .create(&spec())
            .unwrap();
        assert!(!mock.closed.load(Ordering::SeqCst));
        drop(monitor);
        assert!(mock.closed.load(Ordering::SeqCst));
    }

    #[test]
    fn a_portal_that_does_not_answer_in_time_is_unsupported_and_its_late_stream_is_closed() {
        let dir = tempfile::tempdir().unwrap();
        let mock = Arc::new(Mock {
            delay: Duration::from_millis(400),
            ..Arc::try_unwrap(Mock::new()).ok().unwrap()
        });
        let closed = Arc::clone(&mock.closed);
        let err = display(mock, dir.path())
            .with_timeout(Duration::from_millis(50))
            .create(&spec())
            .err()
            .expect("start times out");
        assert!(matches!(err, VdisplayError::Unsupported(_)), "{err}");
        let start = std::time::Instant::now();
        while !closed.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(closed.load(Ordering::SeqCst));
    }

    #[test]
    fn portal_monitor_moves_but_refuses_a_new_size() {
        let dir = tempfile::tempdir().unwrap();
        let mut monitor = display(Mock::new(), dir.path()).create(&spec()).unwrap();
        let moved = MonitorSpec::new(MonitorName::new(""), Rect::new(0, 100, 1280, 720));
        monitor.resize(&moved).unwrap();
        assert_eq!(monitor.rect(), moved.rect);
        let bigger = MonitorSpec::new(MonitorName::new(""), Rect::new(0, 100, 1600, 900));
        assert!(matches!(
            monitor.resize(&bigger),
            Err(VdisplayError::Unsupported(_))
        ));
    }

    #[test]
    fn token_file_with_unexpected_content_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        let store = TokenStore::new(&path);
        assert_eq!(store.load(), None);
        for bad in [
            "",
            "two words",
            "line\nbreak",
            &"x".repeat(1000),
            "caf\u{e9}",
        ] {
            fs::write(&path, bad).unwrap();
            assert_eq!(store.load(), None, "{bad:?}");
        }
        fs::write(&path, "abc123\n").unwrap();
        assert_eq!(store.load().as_deref(), Some("abc123"));
    }
}
