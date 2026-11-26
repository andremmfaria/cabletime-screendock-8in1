//! Gathers [`DisplayFacts`] from the X server through RandR, and from the environment for
//! Wayland sessions.

use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::xproto::Window;
use x11rb::rust_connection::RustConnection;

use crate::ports::display::{
    DisplayFacts, DisplayServer, OutputFacts, OutputId, Rect, VdisplayError,
};

fn connection_err(e: impl std::fmt::Display) -> VdisplayError {
    VdisplayError::Connection(e.to_string())
}

fn request_err(e: impl std::fmt::Display) -> VdisplayError {
    VdisplayError::Request(e.to_string())
}

/// Opens the X display for the display adapters.
pub(super) fn connect(display: Option<&str>) -> Result<(RustConnection, Window), VdisplayError> {
    let (conn, screen) = x11rb::connect(display).map_err(connection_err)?;
    let root = conn.setup().roots[screen].root;
    Ok((conn, root))
}

/// The facts of the X server behind `display` (or `$DISPLAY`): its outputs with their
/// rectangles, the RandR version and an output that could take a custom mode.
pub fn x11_facts(display: Option<&str>) -> Result<DisplayFacts, VdisplayError> {
    let (conn, root) = connect(display)?;
    let mut facts = DisplayFacts {
        server: DisplayServer::X11,
        desktop: std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
        ..DisplayFacts::default()
    };
    let version = conn
        .randr_query_version(1, 5)
        .map_err(request_err)?
        .reply()
        .ok();
    let Some(version) = version else {
        return Ok(facts);
    };
    facts.randr_version = Some((version.major_version, version.minor_version));
    let resources = conn
        .randr_get_screen_resources_current(root)
        .map_err(request_err)?
        .reply()
        .map_err(request_err)?;
    let primary = conn
        .randr_get_output_primary(root)
        .map_err(request_err)?
        .reply()
        .map_err(request_err)?
        .output;
    for &output in &resources.outputs {
        let info = conn
            .randr_get_output_info(output, resources.config_timestamp)
            .map_err(request_err)?
            .reply()
            .map_err(request_err)?;
        let id = OutputId(output);
        if info.crtc == 0 {
            let free_and_unplugged = info.connection == randr::Connection::DISCONNECTED;
            if free_and_unplugged && facts.spare_output.is_none() && !info.crtcs.is_empty() {
                facts.spare_output = Some(id);
            }
            continue;
        }
        let crtc = conn
            .randr_get_crtc_info(info.crtc, resources.config_timestamp)
            .map_err(request_err)?
            .reply()
            .map_err(request_err)?;
        facts.outputs.push(OutputFacts {
            id,
            name: String::from_utf8_lossy(&info.name).into_owned(),
            rect: Rect::new(
                i32::from(crtc.x),
                i32::from(crtc.y),
                u32::from(crtc.width),
                u32::from(crtc.height),
            ),
            primary: output == primary,
        });
    }
    Ok(facts)
}

/// The facts of the session the process runs in: X11 when `$DISPLAY` is set and no Wayland
/// display is, Wayland (with what the environment tells) when `$WAYLAND_DISPLAY` is set, else
/// no session.
pub fn session_facts(get: impl Fn(&str) -> Option<String>) -> Result<DisplayFacts, VdisplayError> {
    let set = |name: &str| get(name).filter(|v| !v.is_empty());
    if set("WAYLAND_DISPLAY").is_some() {
        return Ok(DisplayFacts {
            server: DisplayServer::Wayland,
            desktop: set("XDG_CURRENT_DESKTOP").unwrap_or_default(),
            sway_socket: set("SWAYSOCK").is_some(),
            ..DisplayFacts::default()
        });
    }
    match set("DISPLAY") {
        Some(display) => x11_facts(Some(&display)),
        None => Ok(DisplayFacts::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn no_display_variables_mean_no_session() {
        assert_eq!(session_facts(env(&[])).unwrap().server, DisplayServer::None);
    }

    #[test]
    fn wayland_session_reports_desktop_and_sway_socket() {
        let facts = session_facts(env(&[
            ("WAYLAND_DISPLAY", "wayland-1"),
            ("XDG_CURRENT_DESKTOP", "sway"),
            ("SWAYSOCK", "/run/user/1000/sway.sock"),
        ]))
        .unwrap();
        assert_eq!(facts.server, DisplayServer::Wayland);
        assert_eq!(facts.desktop, "sway");
        assert!(facts.sway_socket);
    }

    #[test]
    fn unreachable_x_display_is_a_connection_error() {
        let err = x11_facts(Some(":499")).unwrap_err();
        assert!(matches!(err, VdisplayError::Connection(_)));
    }
}
