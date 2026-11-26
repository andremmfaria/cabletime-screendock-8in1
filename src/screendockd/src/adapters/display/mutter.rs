//! The Mutter display: a virtual monitor from `org.gnome.Mutter.ScreenCast` `RecordVirtual`.
//!
//! GNOME's portal backend does not offer a virtual source everywhere, so the display talks to
//! Mutter's own ScreenCast service. `RecordVirtual` makes a monitor that exists for the
//! stream alone: Mutter publishes a PipeWire node for it and the size of the frames is the size
//! the consumer asks the node for. Where the monitor sits on the desktop is Mutter's choice,
//! so the placement settings do not apply. The session lives as long as the D-Bus connection,
//! which the monitor owns.

use std::sync::Arc;
use std::time::Duration;

pub use super::types::{
    MutterCalls, MutterConnector, MutterMonitor, MutterVirtualDisplay, DEFAULT_MUTTER_TIMEOUT,
};
use crate::ports::display::{MonitorSpec, Rect, VdisplayError, VirtualDisplay, VirtualMonitor};

impl MutterVirtualDisplay {
    pub fn new(connector: Arc<dyn MutterConnector>) -> Self {
        Self {
            connector,
            timeout: DEFAULT_MUTTER_TIMEOUT,
        }
    }

    /// Sets how long Mutter gets to announce the PipeWire node.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl VirtualDisplay for MutterVirtualDisplay {
    fn name(&self) -> &'static str {
        "mutter"
    }

    fn create(&self, spec: &MonitorSpec) -> Result<Box<dyn VirtualMonitor>, VdisplayError> {
        let mut calls = self.connector.connect()?;
        let session = calls.create_session()?;
        let started = calls
            .record_virtual(&session)
            .and_then(|stream| calls.start(&session, &stream, self.timeout));
        match started {
            Ok(node_id) => Ok(Box::new(MutterMonitor {
                calls,
                session,
                node_id,
                rect: spec.rect,
            })),
            Err(e) => {
                calls.stop(&session);
                Err(e)
            }
        }
    }
}

impl VirtualMonitor for MutterMonitor {
    fn rect(&self) -> Rect {
        self.rect
    }

    /// The stream takes the size its consumer asks for, so a new size only changes the
    /// rectangle the capture is pointed at.
    fn resize(&mut self, spec: &MonitorSpec) -> Result<(), VdisplayError> {
        self.rect = spec.rect;
        Ok(())
    }

    fn pipewire_node(&self) -> Option<u32> {
        Some(self.node_id)
    }
}

impl Drop for MutterMonitor {
    fn drop(&mut self) {
        self.calls.stop(&self.session);
    }
}

#[cfg(feature = "capture-wayland")]
pub use zbus_calls::probe_screencast;

/// Without the `capture-wayland` feature no bus is asked.
#[cfg(not(feature = "capture-wayland"))]
pub fn probe_screencast(_address: Option<&str>, _timeout: Duration) -> bool {
    false
}

#[cfg(feature = "capture-wayland")]
pub use super::types::ZbusMutterConnector;

#[cfg(feature = "capture-wayland")]
mod zbus_calls {
    use std::collections::HashMap;
    use std::future::poll_fn;
    use std::pin::pin;
    use std::sync::mpsc;
    use std::time::Duration;

    use zbus::export::futures_core::Stream;
    use zbus::{OwnedObjectPath, Variant};

    use super::super::types::{
        MutterCalls, MutterConnector, ZbusMutterCalls, ZbusMutterConnector, CURSOR_EMBEDDED,
        MUTTER_PATH, MUTTER_SERVICE, MUTTER_SESSION_INTERFACE, MUTTER_STREAM_INTERFACE,
    };
    use crate::ports::display::VdisplayError;

    fn request_err(e: impl std::fmt::Display) -> VdisplayError {
        VdisplayError::Request(format!("mutter: {e}"))
    }

    fn connection_err(e: impl std::fmt::Display) -> VdisplayError {
        VdisplayError::Connection(format!("session bus: {e}"))
    }

    impl ZbusMutterConnector {
        /// Connects to the bus named by `$DBUS_SESSION_BUS_ADDRESS`.
        pub fn new() -> Self {
            Self::default()
        }

        /// Connects to the bus at `address`, whatever the environment says.
        pub fn with_address(address: impl Into<String>) -> Self {
            Self {
                address: Some(address.into()),
            }
        }
    }

    async fn connect(address: Option<String>) -> zbus::Result<zbus::Connection> {
        let builder = match address {
            Some(address) => zbus::connection::Builder::address(address.as_str()),
            None => zbus::connection::Builder::session(),
        };
        builder.build().await
    }

    fn runtime() -> Result<tokio::runtime::Runtime, VdisplayError> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(connection_err)
    }

    impl MutterConnector for ZbusMutterConnector {
        fn connect(&self) -> Result<Box<dyn MutterCalls>, VdisplayError> {
            let runtime = runtime()?;
            let connection = runtime
                .block_on(connect(self.address.clone()))
                .map_err(connection_err)?;
            Ok(Box::new(ZbusMutterCalls {
                runtime,
                connection,
            }))
        }
    }

    impl ZbusMutterCalls {
        async fn proxy<'a>(
            &'a self,
            path: &'a str,
            interface: &'a str,
        ) -> zbus::Result<zbus::Proxy<'a>> {
            zbus::Proxy::new(&self.connection, MUTTER_SERVICE, path, interface).await
        }
    }

    impl MutterCalls for ZbusMutterCalls {
        fn create_session(&mut self) -> Result<String, VdisplayError> {
            let this = &*self;
            this.runtime
                .block_on(async {
                    let proxy = this.proxy(MUTTER_PATH, MUTTER_SERVICE).await?;
                    let options: HashMap<&str, Variant> = HashMap::new();
                    let path: OwnedObjectPath = proxy.call("CreateSession", &(options,)).await?;
                    Ok::<_, zbus::Error>(path.to_string())
                })
                .map_err(request_err)
        }

        fn record_virtual(&mut self, session: &str) -> Result<String, VdisplayError> {
            let this = &*self;
            this.runtime
                .block_on(async {
                    let proxy = this.proxy(session, MUTTER_SESSION_INTERFACE).await?;
                    let mut options: HashMap<&str, Variant> = HashMap::new();
                    options.insert("cursor-mode", Variant::U32(CURSOR_EMBEDDED));
                    let path: OwnedObjectPath = proxy.call("RecordVirtual", &(options,)).await?;
                    Ok::<_, zbus::Error>(path.to_string())
                })
                .map_err(request_err)
        }

        fn start(
            &mut self,
            session: &str,
            stream: &str,
            timeout: Duration,
        ) -> Result<u32, VdisplayError> {
            let this = &*self;
            this.runtime.block_on(async {
                let stream_proxy = this
                    .proxy(stream, MUTTER_STREAM_INTERFACE)
                    .await
                    .map_err(request_err)?;
                // Subscribed before Start, so the announcement cannot be missed.
                let mut signals = stream_proxy
                    .receive_signal("PipeWireStreamAdded")
                    .await
                    .map_err(request_err)?;
                let session_proxy = this
                    .proxy(session, MUTTER_SESSION_INTERFACE)
                    .await
                    .map_err(request_err)?;
                session_proxy
                    .call::<_, _, ()>("Start", &())
                    .await
                    .map_err(request_err)?;
                let next = poll_fn(|cx| pin!(&mut signals).poll_next(cx));
                let message = tokio::time::timeout(timeout, next)
                    .await
                    .map_err(|_| {
                        VdisplayError::Unsupported(format!(
                            "mutter announced no stream within {} s",
                            timeout.as_secs()
                        ))
                    })?
                    .ok_or_else(|| request_err("the stream signal ended"))?;
                message.body().deserialize::<u32>().map_err(request_err)
            })
        }

        fn stop(&mut self, session: &str) {
            let this = &*self;
            let stopped = this.runtime.block_on(async {
                let proxy = this.proxy(session, MUTTER_SESSION_INTERFACE).await?;
                proxy.call::<_, _, ()>("Stop", &()).await
            });
            if let Err(e) = stopped {
                tracing::warn!(error = %e, "could not stop the mutter session");
            }
        }
    }

    /// True when `org.gnome.Mutter.ScreenCast` has an owner on the session bus at `address`
    /// (or `$DBUS_SESSION_BUS_ADDRESS`), asked on a worker thread so a missing bus cannot
    /// hold the caller longer than `timeout`.
    pub fn probe_screencast(address: Option<&str>, timeout: Duration) -> bool {
        let address = address.map(str::to_string);
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("mutter-probe".into())
            .spawn(move || {
                let owned = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()
                    .and_then(|runtime| {
                        runtime.block_on(async {
                            let connection = connect(address).await.ok()?;
                            let bus = zbus::fdo::DBusProxy::new(&connection).await.ok()?;
                            let name = zbus::names::BusName::try_from(MUTTER_SERVICE).ok()?;
                            bus.name_has_owner(name).await.ok()
                        })
                    })
                    .unwrap_or(false);
                let _ = tx.send(owned);
            });
        spawned.is_ok() && rx.recv_timeout(timeout).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::ports::display::{MonitorName, Rect};

    type Log = Arc<Mutex<Vec<String>>>;

    struct Recorder {
        log: Log,
        fail_at: Option<&'static str>,
    }

    impl Recorder {
        fn step(&self, what: &str) -> Result<(), VdisplayError> {
            self.log.lock().unwrap().push(what.to_string());
            if self.fail_at == Some(what_name(what)) {
                Err(VdisplayError::Request(format!("{what} refused")))
            } else {
                Ok(())
            }
        }
    }

    fn what_name(what: &str) -> &str {
        what.split(' ').next().unwrap_or(what)
    }

    impl MutterCalls for Recorder {
        fn create_session(&mut self) -> Result<String, VdisplayError> {
            self.step("create_session")?;
            Ok("/session/u1".into())
        }
        fn record_virtual(&mut self, session: &str) -> Result<String, VdisplayError> {
            self.step(&format!("record_virtual {session}"))?;
            Ok("/stream/u1".into())
        }
        fn start(
            &mut self,
            session: &str,
            stream: &str,
            _: Duration,
        ) -> Result<u32, VdisplayError> {
            self.step(&format!("start {session} {stream}"))?;
            Ok(45)
        }
        fn stop(&mut self, session: &str) {
            let _ = self.step(&format!("stop {session}"));
        }
    }

    struct Connector {
        log: Log,
        fail_at: Option<&'static str>,
    }

    impl MutterConnector for Connector {
        fn connect(&self) -> Result<Box<dyn MutterCalls>, VdisplayError> {
            self.log.lock().unwrap().push("connect".into());
            Ok(Box::new(Recorder {
                log: Arc::clone(&self.log),
                fail_at: self.fail_at,
            }))
        }
    }

    fn display(fail_at: Option<&'static str>) -> (MutterVirtualDisplay, Log) {
        let log = Log::default();
        let connector = Connector {
            log: Arc::clone(&log),
            fail_at,
        };
        (MutterVirtualDisplay::new(Arc::new(connector)), log)
    }

    fn spec() -> MonitorSpec {
        MonitorSpec::new(MonitorName::new(""), Rect::new(0, 0, 1280, 720))
    }

    #[test]
    fn mutter_mock_creates_session_records_virtual_then_starts_in_order() {
        let (display, log) = display(None);
        let monitor = display.create(&spec()).unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            [
                "connect",
                "create_session",
                "record_virtual /session/u1",
                "start /session/u1 /stream/u1"
            ]
        );
        assert_eq!(monitor.pipewire_node(), Some(45));
        assert_eq!(monitor.rect(), Rect::new(0, 0, 1280, 720));
    }

    #[test]
    fn mutter_mock_stops_the_session_when_the_monitor_drops() {
        let (display, log) = display(None);
        drop(display.create(&spec()).unwrap());
        assert_eq!(log.lock().unwrap().last().unwrap(), "stop /session/u1");
    }

    #[test]
    fn mutter_mock_stops_the_session_when_start_fails() {
        let (display, log) = display(Some("start"));
        let err = display.create(&spec()).err().expect("start fails");
        assert!(matches!(err, VdisplayError::Request(_)));
        assert_eq!(log.lock().unwrap().last().unwrap(), "stop /session/u1");
    }

    #[test]
    fn mutter_mock_stops_the_session_when_record_virtual_fails() {
        let (display, log) = display(Some("record_virtual"));
        assert!(display.create(&spec()).is_err());
        let log = log.lock().unwrap();
        assert!(!log.iter().any(|l| l.starts_with("start ")));
        assert_eq!(log.last().unwrap(), "stop /session/u1");
    }

    #[test]
    fn mutter_mock_failed_session_creation_leaves_nothing_to_stop() {
        let (display, log) = display(Some("create_session"));
        assert!(display.create(&spec()).is_err());
        assert!(!log.lock().unwrap().iter().any(|l| l.starts_with("stop")));
    }

    #[test]
    fn mutter_mock_resize_moves_the_rectangle_and_keeps_the_stream() {
        let (display, log) = display(None);
        let mut monitor = display.create(&spec()).unwrap();
        let bigger = MonitorSpec::new(MonitorName::new(""), Rect::new(0, 0, 1600, 900));
        monitor.resize(&bigger).unwrap();
        assert_eq!(monitor.rect(), Rect::new(0, 0, 1600, 900));
        assert_eq!(monitor.pipewire_node(), Some(45));
        assert!(!log.lock().unwrap().iter().any(|l| l.starts_with("stop")));
    }
}
