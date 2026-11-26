//! Capture backends.
//!
//! Two implementations of [`CaptureBackend`] exist: [`x11_shm::X11ShmCapture`] (XShm over a
//! pure-Rust X11 client) and [`wayland_portal::WaylandPortalCapture`] (xdg-desktop-portal
//! ScreenCast plus PipeWire, behind the `capture-wayland` cargo feature). [`create_backend`]
//! picks one once at startup and hands back a boxed trait object, so the rest of the daemon
//! never names a concrete backend.

pub mod types;
pub mod wayland_portal;
pub mod x11_shm;

pub use self::types::{BackendKind, SessionType};
use crate::ports::capture::{CaptureArea, CaptureBackend, CaptureError};
use crate::utils::config::{CaptureBackendChoice, Config};

/// Classifies the session from environment values read through `get`. `$XDG_SESSION_TYPE` wins
/// when it names `x11` or `wayland`; otherwise `$WAYLAND_DISPLAY` is checked before `$DISPLAY`
/// (an XWayland session sets both, and only Wayland capture sees the whole desktop).
pub fn detect_session_with(get: impl Fn(&str) -> Option<String>) -> SessionType {
    let non_empty = |name: &str| get(name).filter(|v| !v.is_empty());
    match non_empty("XDG_SESSION_TYPE")
        .map(|v| v.to_ascii_lowercase())
        .as_deref()
    {
        Some("wayland") => return SessionType::Wayland,
        Some("x11") => return SessionType::X11,
        _ => {}
    }
    if non_empty("WAYLAND_DISPLAY").is_some() {
        SessionType::Wayland
    } else if non_empty("DISPLAY").is_some() {
        SessionType::X11
    } else {
        SessionType::Unknown
    }
}

/// [`detect_session_with`] over the process environment.
pub fn detect_session() -> SessionType {
    detect_session_with(|name| std::env::var(name).ok())
}

/// Resolves the configured choice against the detected session. An explicit choice always wins;
/// `Auto` follows the session and fails when there is none.
pub fn select_backend(
    choice: CaptureBackendChoice,
    session: SessionType,
) -> Result<BackendKind, CaptureError> {
    match (choice, session) {
        (CaptureBackendChoice::X11, _) => Ok(BackendKind::X11Shm),
        (CaptureBackendChoice::Wayland, _) => Ok(BackendKind::WaylandPortal),
        (CaptureBackendChoice::Auto, SessionType::X11) => Ok(BackendKind::X11Shm),
        (CaptureBackendChoice::Auto, SessionType::Wayland) => Ok(BackendKind::WaylandPortal),
        (CaptureBackendChoice::Auto, SessionType::Unknown) => Err(CaptureError::NoSession),
    }
}

/// Selects and constructs the capture backend once at startup, pointed at the whole desktop.
pub fn create_backend(
    config: &Config,
    choice: CaptureBackendChoice,
) -> Result<Box<dyn CaptureBackend>, CaptureError> {
    create_backend_at(config, choice, CaptureArea::Whole)
}

/// Like [`create_backend`], for a capture that starts on `area`. A Wayland capture that
/// starts on a PipeWire node attaches to it directly and never opens a portal session; every
/// other combination starts on the whole desktop and leaves the move to `retarget`.
pub fn create_backend_at(
    config: &Config,
    choice: CaptureBackendChoice,
    area: CaptureArea,
) -> Result<Box<dyn CaptureBackend>, CaptureError> {
    let kind = select_backend(choice, detect_session())?;
    tracing::info!(?kind, ?choice, "selected capture backend");
    match kind {
        BackendKind::X11Shm => Ok(Box::new(x11_shm::X11ShmCapture::connect(
            config.capture.x11_display.as_deref(),
            config.capture.fps,
        )?)),
        BackendKind::WaylandPortal => Ok(Box::new(match area {
            CaptureArea::Stream {
                node,
                width,
                height,
            } => wayland_portal::WaylandPortalCapture::connect_stream(
                None,
                node,
                width,
                height,
                config.capture.fps,
            )?,
            _ => wayland_portal::WaylandPortalCapture::connect(
                config.display.width,
                config.display.height,
                config.capture.fps,
            )?,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn session(vars: &[(&str, &str)]) -> SessionType {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        detect_session_with(|name| map.get(name).cloned())
    }

    #[test]
    fn xdg_session_type_wins() {
        assert_eq!(
            session(&[("XDG_SESSION_TYPE", "wayland"), ("DISPLAY", ":0")]),
            SessionType::Wayland
        );
        assert_eq!(
            session(&[
                ("XDG_SESSION_TYPE", "X11"),
                ("WAYLAND_DISPLAY", "wayland-0")
            ]),
            SessionType::X11
        );
    }

    #[test]
    fn falls_back_to_display_variables() {
        assert_eq!(
            session(&[
                ("XDG_SESSION_TYPE", "tty"),
                ("WAYLAND_DISPLAY", "wayland-0")
            ]),
            SessionType::Wayland
        );
        assert_eq!(session(&[("DISPLAY", ":0")]), SessionType::X11);
        assert_eq!(
            session(&[("DISPLAY", ":0"), ("WAYLAND_DISPLAY", "wayland-0")]),
            SessionType::Wayland
        );
        assert_eq!(session(&[("DISPLAY", "")]), SessionType::Unknown);
        assert_eq!(session(&[]), SessionType::Unknown);
    }

    #[test]
    fn explicit_choice_overrides_session() {
        assert_eq!(
            select_backend(CaptureBackendChoice::X11, SessionType::Wayland).unwrap(),
            BackendKind::X11Shm
        );
        assert_eq!(
            select_backend(CaptureBackendChoice::Wayland, SessionType::Unknown).unwrap(),
            BackendKind::WaylandPortal
        );
        assert_eq!(
            select_backend(CaptureBackendChoice::Auto, SessionType::X11).unwrap(),
            BackendKind::X11Shm
        );
        assert!(matches!(
            select_backend(CaptureBackendChoice::Auto, SessionType::Unknown),
            Err(CaptureError::NoSession)
        ));
    }
}
