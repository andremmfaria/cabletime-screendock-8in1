//! [`RandrBackend`] over a real X connection, with x11rb.

use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as _, ModeFlag, ModeInfo, MonitorInfo, Rotation};
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::CURRENT_TIME;

use super::facts::connect;
use super::types::{RandrBackend, ScreenSize, XRandr};
use crate::ports::display::{OutputId, Rect, VdisplayError};

fn request_err(e: impl std::fmt::Display) -> VdisplayError {
    VdisplayError::Request(e.to_string())
}

/// Horizontal and vertical blanking of the modes the daemon creates, reduced blanking style.
const H_BLANK: u32 = 160;
const V_BLANK: u32 = 30;
const REFRESH_HZ: u32 = 60;

impl XRandr {
    /// Connects to `display` (or `$DISPLAY`) and checks for RandR 1.5.
    pub fn connect(display: Option<&str>) -> Result<Self, VdisplayError> {
        let (conn, root) = connect(display)?;
        let version = conn
            .randr_query_version(1, 5)
            .map_err(request_err)?
            .reply()
            .map_err(|e| VdisplayError::Unsupported(format!("no RandR extension: {e}")))?;
        if (version.major_version, version.minor_version) < (1, 5) {
            return Err(VdisplayError::Unsupported(
                "RandR monitors need RandR 1.5 or newer".into(),
            ));
        }
        Ok(Self { conn, root })
    }

    fn resources(&self) -> Result<randr::GetScreenResourcesCurrentReply, VdisplayError> {
        self.conn
            .randr_get_screen_resources_current(self.root)
            .map_err(request_err)?
            .reply()
            .map_err(request_err)
    }

    fn mode_named(
        resources: &randr::GetScreenResourcesCurrentReply,
        name: &str,
    ) -> Option<ModeInfo> {
        let mut offset = 0usize;
        for mode in &resources.modes {
            let end = offset + usize::from(mode.name_len);
            if resources.names.get(offset..end) == Some(name.as_bytes()) {
                return Some(*mode);
            }
            offset = end;
        }
        None
    }

    fn atom(&self, name: &str) -> Result<u32, VdisplayError> {
        Ok(self
            .conn
            .intern_atom(false, name.as_bytes())
            .map_err(request_err)?
            .reply()
            .map_err(request_err)?
            .atom)
    }

    fn crtc_of(&self, output: OutputId) -> Result<u32, VdisplayError> {
        Ok(self
            .conn
            .randr_get_output_info(output.0, 0)
            .map_err(request_err)?
            .reply()
            .map_err(request_err)?
            .crtc)
    }
}

impl RandrBackend for XRandr {
    fn screen_size(&mut self) -> Result<ScreenSize, VdisplayError> {
        let geometry = self
            .conn
            .get_geometry(self.root)
            .map_err(request_err)?
            .reply()
            .map_err(request_err)?;
        let (width, height) = (u32::from(geometry.width), u32::from(geometry.height));
        let setup = self.conn.setup();
        let screen = setup.roots.iter().find(|s| s.root == self.root);
        let (width_mm, height_mm) = match screen {
            Some(s)
                if u32::from(s.width_in_pixels) == width
                    && u32::from(s.height_in_pixels) == height =>
            {
                (
                    u32::from(s.width_in_millimeters),
                    u32::from(s.height_in_millimeters),
                )
            }
            _ => (width * 254 / 960, height * 254 / 960),
        };
        Ok(ScreenSize {
            width,
            height,
            width_mm,
            height_mm,
        })
    }

    fn set_screen_size(&mut self, size: ScreenSize) -> Result<(), VdisplayError> {
        let narrow = |v: u32| u16::try_from(v).map_err(|_| request_err("screen size too large"));
        self.conn
            .randr_set_screen_size(
                self.root,
                narrow(size.width)?,
                narrow(size.height)?,
                size.width_mm,
                size.height_mm,
            )
            .map_err(request_err)?
            .check()
            .map_err(request_err)
    }

    fn monitor_names(&mut self) -> Result<Vec<String>, VdisplayError> {
        let monitors = self
            .conn
            .randr_get_monitors(self.root, true)
            .map_err(request_err)?
            .reply()
            .map_err(request_err)?
            .monitors;
        let mut names = Vec::new();
        for monitor in monitors {
            let name = self
                .conn
                .get_atom_name(monitor.name)
                .map_err(request_err)?
                .reply()
                .map_err(request_err)?
                .name;
            names.push(String::from_utf8_lossy(&name).into_owned());
        }
        Ok(names)
    }

    fn set_monitor(
        &mut self,
        name: &str,
        rect: Rect,
        size_mm: (u32, u32),
    ) -> Result<(), VdisplayError> {
        let fits = |v: u32| u16::try_from(v).map_err(|_| request_err("monitor size too large"));
        let info = MonitorInfo {
            name: self.atom(name)?,
            primary: false,
            automatic: false,
            x: i16::try_from(rect.x).map_err(|_| request_err("monitor position too large"))?,
            y: i16::try_from(rect.y).map_err(|_| request_err("monitor position too large"))?,
            width: fits(rect.width)?,
            height: fits(rect.height)?,
            width_in_millimeters: size_mm.0,
            height_in_millimeters: size_mm.1,
            outputs: Vec::new(),
        };
        self.conn
            .randr_set_monitor(self.root, info)
            .map_err(request_err)?
            .check()
            .map_err(request_err)
    }

    fn delete_monitor(&mut self, name: &str) -> Result<(), VdisplayError> {
        let atom = self.atom(name)?;
        // A monitor that is already gone makes the server answer with an error; that is the
        // state we want.
        let _ = self
            .conn
            .randr_delete_monitor(self.root, atom)
            .map_err(request_err)?
            .check();
        Ok(())
    }

    fn ensure_mode(&mut self, name: &str, width: u32, height: u32) -> Result<u32, VdisplayError> {
        let resources = self.resources()?;
        if let Some(mode) = Self::mode_named(&resources, name) {
            if u32::from(mode.width) == width && u32::from(mode.height) == height {
                return Ok(mode.id);
            }
            return Err(request_err(format!("mode {name} exists with another size")));
        }
        let htotal = width + H_BLANK;
        let vtotal = height + V_BLANK;
        let narrow = |v: u32| u16::try_from(v).map_err(|_| request_err("mode size too large"));
        let info = ModeInfo {
            id: 0,
            width: narrow(width)?,
            height: narrow(height)?,
            dot_clock: htotal * vtotal * REFRESH_HZ,
            hsync_start: narrow(width + 48)?,
            hsync_end: narrow(width + 80)?,
            htotal: narrow(htotal)?,
            hskew: 0,
            vsync_start: narrow(height + 3)?,
            vsync_end: narrow(height + 8)?,
            vtotal: narrow(vtotal)?,
            name_len: u16::try_from(name.len()).map_err(|_| request_err("mode name too long"))?,
            mode_flags: ModeFlag::HSYNC_NEGATIVE | ModeFlag::VSYNC_POSITIVE,
        };
        Ok(self
            .conn
            .randr_create_mode(self.root, info, name.as_bytes())
            .map_err(request_err)?
            .reply()
            .map_err(request_err)?
            .mode)
    }

    fn show_output(
        &mut self,
        output: OutputId,
        mode: u32,
        x: i32,
        y: i32,
    ) -> Result<(), VdisplayError> {
        let resources = self.resources()?;
        let info = self
            .conn
            .randr_get_output_info(output.0, resources.config_timestamp)
            .map_err(request_err)?
            .reply()
            .map_err(request_err)?;
        self.conn
            .randr_add_output_mode(output.0, mode)
            .map_err(request_err)?
            .check()
            .map_err(request_err)?;
        let mut free = None;
        for &crtc in &info.crtcs {
            let state = self
                .conn
                .randr_get_crtc_info(crtc, resources.config_timestamp)
                .map_err(request_err)?
                .reply()
                .map_err(request_err)?;
            if state.outputs.is_empty() && state.mode == 0 {
                free = Some(crtc);
                break;
            }
        }
        let crtc = free.ok_or_else(|| request_err("the output has no free CRTC"))?;
        let reply = self
            .conn
            .randr_set_crtc_config(
                crtc,
                CURRENT_TIME,
                resources.config_timestamp,
                i16::try_from(x).map_err(|_| request_err("position too large"))?,
                i16::try_from(y).map_err(|_| request_err("position too large"))?,
                mode,
                Rotation::ROTATE0,
                &[output.0],
            )
            .map_err(request_err)?
            .reply()
            .map_err(request_err)?;
        if reply.status != randr::SetConfig::SUCCESS {
            return Err(request_err(format!(
                "the server refused the output configuration: {:?}",
                reply.status
            )));
        }
        Ok(())
    }

    fn hide_output(&mut self, output: OutputId) -> Result<(), VdisplayError> {
        let crtc = self.crtc_of(output)?;
        if crtc == 0 {
            return Ok(());
        }
        let resources = self.resources()?;
        self.conn
            .randr_set_crtc_config(
                crtc,
                CURRENT_TIME,
                resources.config_timestamp,
                0,
                0,
                0,
                Rotation::ROTATE0,
                &[],
            )
            .map_err(request_err)?
            .reply()
            .map_err(request_err)?;
        Ok(())
    }

    fn drop_mode(&mut self, output: OutputId, name: &str) -> Result<(), VdisplayError> {
        let resources = self.resources()?;
        let Some(mode) = Self::mode_named(&resources, name) else {
            return Ok(());
        };
        // Both requests fail when something still uses the mode; the next recovery retries.
        let _ = self
            .conn
            .randr_delete_output_mode(output.0, mode.id)
            .map_err(request_err)?
            .check();
        let _ = self
            .conn
            .randr_destroy_mode(mode.id)
            .map_err(request_err)?
            .check();
        Ok(())
    }
}
