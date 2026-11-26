//! Display adapters: facts about the desktop and the implementations of the virtual display
//! port.
//!
//! [`SystemDisplayProvider`] gathers facts and builds the display a policy decision names.

mod facts;
mod mirror;
pub mod types;

pub use self::facts::{session_facts, x11_facts};
pub use self::mirror::{MirrorDisplay, MirrorMonitor};
pub use self::types::SystemDisplayProvider;
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
        }
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
        Err(VdisplayError::Unsupported(format!(
            "{backend:?} is not available in this build"
        )))
    }

    fn mirror(&self, anchor: &str) -> Result<Box<dyn VirtualDisplay>, VdisplayError> {
        let facts = self.facts()?;
        let output = facts
            .find_output(anchor)
            .ok_or_else(|| VdisplayError::Unsupported(format!("no output {anchor:?} to mirror")))?;
        Ok(Box::new(MirrorDisplay::new(output.rect)))
    }
}
