//! Display adapters: facts about the desktop and the implementations of the virtual display
//! port.
//!
//! [`SystemDisplayProvider`] gathers facts and builds the display a policy decision names.

mod facts;
mod mirror;
mod mutter;
mod portal;
mod randr;
mod recovery;
pub mod sway_ipc;
pub mod types;
mod wlroots;
mod x11_randr;

pub use self::facts::{session_facts, x11_facts};
pub use self::mirror::{MirrorDisplay, MirrorMonitor};
#[cfg(feature = "capture-wayland")]
pub use self::mutter::ZbusMutterConnector;
pub use self::mutter::{
    probe_screencast, MutterCalls, MutterConnector, MutterMonitor, MutterVirtualDisplay,
    DEFAULT_MUTTER_TIMEOUT,
};
pub use self::portal::probe_source_types;
pub use self::recovery::recover;
pub use self::sway_ipc::sway_output_facts;
#[cfg(feature = "capture-wayland")]
pub use self::types::AshpdPortalSource;
pub use self::types::{
    PortalMonitor, PortalRequest, PortalSource, PortalStream, PortalVirtualDisplay, TokenStore,
    DEFAULT_PORTAL_TIMEOUT,
};
pub use self::types::{
    RandrBackend, RecoveryOutcome, RecoveryState, RecoveryStore, ScreenSize, SystemDisplayProvider,
    X11MonitorGuard, X11RandrDisplay, XRandr,
};
pub use self::types::{SwayIpc, SwayOutput, WlrootsHeadlessDisplay, WlrootsMonitor};
use crate::ports::display::{
    DisplayFacts, DisplayProvider, DisplayServer, ExtendBackend, VdisplayError, VirtualDisplay,
};

/// How long the provider waits for the portal to name its source types.
const PORTAL_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

impl SystemDisplayProvider {
    /// A provider for the session described by the environment.
    pub fn new() -> Self {
        Self::default()
    }

    /// A provider for a given X display, whatever the environment says.
    pub fn for_x11_display(display: impl Into<String>) -> Self {
        Self {
            x11_display: Some(display.into()),
            recovery: None,
            sway_socket: None,
            mutter_bus: None,
        }
    }

    /// A provider for the sway whose IPC socket is at `socket`, whatever the environment
    /// says.
    pub fn for_sway(socket: impl Into<std::path::PathBuf>) -> Self {
        Self {
            sway_socket: Some(socket.into()),
            ..Self::default()
        }
    }

    /// A provider for the GNOME Shell whose session bus is at `address`, whatever the
    /// environment says.
    pub fn for_mutter(address: impl Into<String>) -> Self {
        Self {
            mutter_bus: Some(address.into()),
            ..Self::default()
        }
    }

    /// Keeps the recovery file at `store` instead of under `$XDG_RUNTIME_DIR`.
    pub fn with_recovery_store(mut self, store: RecoveryStore) -> Self {
        self.recovery = Some(store);
        self
    }

    /// The recovery store in use: the configured one, else the default location.
    pub fn recovery_store(&self) -> Option<RecoveryStore> {
        self.recovery
            .clone()
            .or_else(RecoveryStore::default_location)
    }
}

impl DisplayProvider for SystemDisplayProvider {
    fn facts(&self) -> Result<DisplayFacts, VdisplayError> {
        if let Some(socket) = &self.sway_socket {
            return Ok(DisplayFacts {
                server: DisplayServer::Wayland,
                // The socket belongs to sway, whatever the environment says.
                desktop: "sway".into(),
                outputs: sway_output_facts(socket)?,
                sway_socket: true,
                ..DisplayFacts::default()
            });
        }
        if let Some(address) = &self.mutter_bus {
            return Ok(DisplayFacts {
                server: DisplayServer::Wayland,
                desktop: "GNOME".into(),
                mutter_screencast: probe_screencast(Some(address), PORTAL_PROBE_TIMEOUT),
                ..DisplayFacts::default()
            });
        }
        match &self.x11_display {
            Some(display) => x11_facts(Some(display)),
            None => {
                let mut facts = session_facts(|name| std::env::var(name).ok())?;
                if facts.server == DisplayServer::Wayland {
                    facts.portal_source_types = probe_source_types(PORTAL_PROBE_TIMEOUT);
                    facts.mutter_screencast = probe_screencast(None, PORTAL_PROBE_TIMEOUT);
                    if let Some(socket) = std::env::var_os("SWAYSOCK").filter(|v| !v.is_empty()) {
                        match sway_output_facts(std::path::Path::new(&socket)) {
                            Ok(outputs) => facts.outputs = outputs,
                            Err(e) => tracing::warn!(error = %e, "could not list the sway outputs"),
                        }
                    }
                }
                Ok(facts)
            }
        }
    }

    fn virtual_display(
        &self,
        backend: ExtendBackend,
    ) -> Result<Box<dyn VirtualDisplay>, VdisplayError> {
        match backend {
            ExtendBackend::X11Randr(tier) => {
                let store = self.recovery_store().ok_or_else(|| {
                    VdisplayError::Unsupported(
                        "no place for the recovery file: $XDG_RUNTIME_DIR is unset".into(),
                    )
                })?;
                Ok(Box::new(X11RandrDisplay::new(
                    self.x11_display.clone(),
                    tier,
                    store,
                )))
            }
            ExtendBackend::Wlroots => Ok(Box::new(match &self.sway_socket {
                Some(socket) => WlrootsHeadlessDisplay::new(socket),
                None => WlrootsHeadlessDisplay::from_env()?,
            })),
            #[cfg(feature = "capture-wayland")]
            ExtendBackend::Mutter => Ok(Box::new(MutterVirtualDisplay::new(std::sync::Arc::new(
                match &self.mutter_bus {
                    Some(address) => ZbusMutterConnector::with_address(address.clone()),
                    None => ZbusMutterConnector::new(),
                },
            )))),
            #[cfg(feature = "capture-wayland")]
            ExtendBackend::Portal => Ok(Box::new(PortalVirtualDisplay::new(std::sync::Arc::new(
                AshpdPortalSource,
            )))),
            #[cfg(not(feature = "capture-wayland"))]
            other => Err(VdisplayError::Unsupported(format!(
                "{other:?} is not available in this build"
            ))),
        }
    }

    fn mirror(&self, anchor: &str) -> Result<Box<dyn VirtualDisplay>, VdisplayError> {
        let facts = self.facts()?;
        let output = facts
            .find_output(anchor)
            .ok_or_else(|| VdisplayError::Unsupported(format!("no output {anchor:?} to mirror")))?;
        Ok(Box::new(MirrorDisplay::new(output.rect)))
    }
}
