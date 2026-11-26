//! Display adapters: facts about the desktop and the implementations of the virtual display
//! port.
//!
//! [`SystemDisplayProvider`] gathers facts and builds the display a policy decision names.

mod facts;
mod mirror;
mod randr;
mod recovery;
pub mod types;
mod x11_randr;

pub use self::facts::{session_facts, x11_facts};
pub use self::mirror::{MirrorDisplay, MirrorMonitor};
pub use self::recovery::recover;
pub use self::types::{
    RandrBackend, RecoveryOutcome, RecoveryState, RecoveryStore, ScreenSize, SystemDisplayProvider,
    X11MonitorGuard, X11RandrDisplay, XRandr,
};
use crate::ports::display::{
    DisplayFacts, DisplayProvider, ExtendBackend, VdisplayError, VirtualDisplay,
};

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
        match &self.x11_display {
            Some(display) => x11_facts(Some(display)),
            None => session_facts(|name| std::env::var(name).ok()),
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
