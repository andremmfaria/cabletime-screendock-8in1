//! Pure display policy: which virtual monitor mechanism to use for the facts the environment
//! reports, and where the monitor sits. No I/O; the facts come from an adapter.

pub use super::types::{
    DisplayDecision, DisplayPolicyError, ExtendBackend, VirtualSupport, X11Tier,
};
use crate::core::reconfig::LiveSettings;
use crate::ports::display::{DisplayFacts, DisplayServer, Rect};
use crate::utils::types::{
    DisplayMode, FallbackPolicy, Placement, VirtualDisplayConfig, WaylandBackend,
};

/// Bit of the portal's `AvailableSourceTypes` that marks a virtual source.
const PORTAL_SOURCE_VIRTUAL: u32 = 4;

fn desktop_has(facts: &DisplayFacts, name: &str) -> bool {
    facts
        .desktop
        .split(':')
        .any(|part| part.eq_ignore_ascii_case(name))
}

/// Whether the environment can host a virtual monitor through `backend`. On X11 the Wayland
/// backend choice does not matter.
pub fn virtual_support(facts: &DisplayFacts, backend: WaylandBackend) -> VirtualSupport {
    match facts.server {
        DisplayServer::None => VirtualSupport::Unsupported("no graphical session".into()),
        DisplayServer::X11 => match facts.randr_version {
            None => VirtualSupport::Unsupported("the X server has no RandR extension".into()),
            Some(version) if version < (1, 5) => {
                VirtualSupport::Unsupported("RandR monitors need RandR 1.5 or newer".into())
            }
            Some(_) => VirtualSupport::Supported,
        },
        DisplayServer::Wayland => match backend {
            WaylandBackend::Auto => wayland_auto(facts).0,
            WaylandBackend::Mutter => mutter_support(facts),
            WaylandBackend::Portal => portal_support(facts),
            WaylandBackend::Wlroots => wlroots_support(facts),
        },
    }
}

fn mutter_support(facts: &DisplayFacts) -> VirtualSupport {
    if facts.mutter_screencast {
        VirtualSupport::Supported
    } else {
        VirtualSupport::Unsupported("org.gnome.Mutter.ScreenCast is not on the session bus".into())
    }
}

fn portal_support(facts: &DisplayFacts) -> VirtualSupport {
    match facts.portal_source_types {
        None => VirtualSupport::Unknown,
        Some(types) if types & PORTAL_SOURCE_VIRTUAL != 0 => VirtualSupport::Supported,
        Some(_) => VirtualSupport::Unsupported("portal reports no virtual source type".into()),
    }
}

fn wlroots_support(facts: &DisplayFacts) -> VirtualSupport {
    if facts.sway_socket && desktop_has(facts, "sway") {
        VirtualSupport::Supported
    } else {
        VirtualSupport::Unsupported("no sway IPC socket".into())
    }
}

/// The automatic Wayland choice: Mutter on GNOME, a wlroots headless output on sway, the portal
/// where it offers a virtual source. Returns the support and the backend it concerns; when
/// nothing applies the support is `Unsupported` with the reasons.
fn wayland_auto(facts: &DisplayFacts) -> (VirtualSupport, Option<ExtendBackend>) {
    if desktop_has(facts, "gnome") && mutter_support(facts) == VirtualSupport::Supported {
        return (VirtualSupport::Supported, Some(ExtendBackend::Mutter));
    }
    if wlroots_support(facts) == VirtualSupport::Supported {
        return (VirtualSupport::Supported, Some(ExtendBackend::Wlroots));
    }
    if portal_support(facts) == VirtualSupport::Supported {
        return (VirtualSupport::Supported, Some(ExtendBackend::Portal));
    }
    (
        VirtualSupport::Unsupported(
            "no Wayland virtual monitor source: not GNOME with Mutter ScreenCast, not sway, and the portal offers no virtual source".into(),
        ),
        None,
    )
}

fn x11_backend(facts: &DisplayFacts) -> ExtendBackend {
    ExtendBackend::X11Randr(match facts.spare_output {
        Some(id) => X11Tier::RealOutput(id),
        None => X11Tier::BareMonitor,
    })
}

/// Chooses how the dock gets its picture for the configured mode. Mirror mode always mirrors.
/// Extend mode picks the first mechanism the facts support; when none does, the fallback
/// policy decides between mirroring with the reason and an error.
pub fn decide(
    facts: &DisplayFacts,
    config: &VirtualDisplayConfig,
) -> Result<DisplayDecision, DisplayPolicyError> {
    if config.mode == DisplayMode::Mirror {
        return Ok(DisplayDecision::Mirror {
            reason: "mode is mirror".into(),
        });
    }
    let chosen = match facts.server {
        DisplayServer::None => Err("no graphical session".to_string()),
        DisplayServer::X11 => match virtual_support(facts, config.wayland_backend) {
            VirtualSupport::Supported => Ok(x11_backend(facts)),
            VirtualSupport::Unsupported(reason) => Err(reason),
            VirtualSupport::Unknown => Err("RandR support is unknown".to_string()),
        },
        DisplayServer::Wayland => match config.wayland_backend {
            WaylandBackend::Auto => {
                wayland_auto(facts)
                    .1
                    .ok_or_else(|| match wayland_auto(facts).0 {
                        VirtualSupport::Unsupported(reason) => reason,
                        _ => "no Wayland virtual monitor source".to_string(),
                    })
            }
            forced => {
                let (support, backend) = match forced {
                    WaylandBackend::Mutter => (mutter_support(facts), ExtendBackend::Mutter),
                    WaylandBackend::Portal => (portal_support(facts), ExtendBackend::Portal),
                    _ => (wlroots_support(facts), ExtendBackend::Wlroots),
                };
                match support {
                    // A forced portal may still work when it could not be probed.
                    VirtualSupport::Supported | VirtualSupport::Unknown => Ok(backend),
                    VirtualSupport::Unsupported(reason) => Err(reason),
                }
            }
        },
    };
    match chosen {
        Ok(backend) => Ok(DisplayDecision::Extend(backend)),
        Err(reason) => match config.fallback {
            FallbackPolicy::Mirror => Ok(DisplayDecision::Mirror { reason }),
            FallbackPolicy::Fail => Err(DisplayPolicyError::NoVirtualDisplay(reason)),
        },
    }
}

/// The rectangle the virtual monitor occupies: next to the anchor output as the placement
/// says, or at the configured position for `Placement::At`. The picture size is the monitor
/// size. The origin can be negative (left of or above an anchor at the desktop origin).
pub fn place_monitor(
    facts: &DisplayFacts,
    settings: &LiveSettings,
) -> Result<Rect, DisplayPolicyError> {
    let width = settings.geometry.width().0;
    let height = settings.geometry.height().0;
    if settings.placement == Placement::At {
        return Ok(Rect::new(settings.x, settings.y, width, height));
    }
    let anchor = facts
        .find_output(&settings.anchor)
        .ok_or_else(|| DisplayPolicyError::UnknownAnchor(settings.anchor.clone()))?
        .rect;
    let signed = |value: u32| i32::try_from(value).unwrap_or(i32::MAX);
    let (x, y) = match settings.placement {
        Placement::RightOf => (anchor.right(), anchor.y),
        Placement::LeftOf => (anchor.x.saturating_sub(signed(width)), anchor.y),
        Placement::Above => (anchor.x, anchor.y.saturating_sub(signed(height))),
        Placement::Below => (anchor.x, anchor.bottom()),
        Placement::At => unreachable!("handled above"),
    };
    Ok(Rect::new(x, y, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::display::{OutputFacts, OutputId};
    use crate::protocol::display::{DisplayGeometry, Orientation};
    use crate::utils::types::X264Preset;

    fn extend() -> VirtualDisplayConfig {
        VirtualDisplayConfig {
            mode: DisplayMode::Extend,
            ..VirtualDisplayConfig::default()
        }
    }

    fn output(id: u32, name: &str, rect: Rect, primary: bool) -> OutputFacts {
        OutputFacts {
            id: OutputId(id),
            name: name.into(),
            rect,
            primary,
        }
    }

    fn x11_facts() -> DisplayFacts {
        DisplayFacts {
            server: DisplayServer::X11,
            desktop: "XFCE".into(),
            outputs: vec![
                output(1, "DP-1", Rect::new(0, 0, 1920, 1080), false),
                output(2, "HDMI-1", Rect::new(1920, 0, 2560, 1440), true),
            ],
            randr_version: Some((1, 6)),
            ..DisplayFacts::default()
        }
    }

    /// The facts measured on the headless GNOME Shell session of the development host.
    fn gnome_headless_facts() -> DisplayFacts {
        DisplayFacts {
            server: DisplayServer::Wayland,
            desktop: "GNOME".into(),
            outputs: vec![output(0, "Virtual-1", Rect::new(0, 0, 1280, 720), true)],
            mutter_screencast: true,
            portal_source_types: Some(0),
            ..DisplayFacts::default()
        }
    }

    fn sway_facts() -> DisplayFacts {
        DisplayFacts {
            server: DisplayServer::Wayland,
            desktop: "sway".into(),
            outputs: vec![output(0, "HEADLESS-1", Rect::new(0, 0, 1280, 720), true)],
            sway_socket: true,
            ..DisplayFacts::default()
        }
    }

    fn settings(placement: Placement, anchor: &str) -> LiveSettings {
        LiveSettings {
            geometry: DisplayGeometry::new(1280, 720, Orientation::Landscape).unwrap(),
            fps: 25,
            preset: X264Preset::Ultrafast,
            tune_zerolatency: true,
            bitrate_kbps: 4000,
            vbv_buffer_ms: 100,
            keyframe_interval: 50,
            mode: DisplayMode::Extend,
            placement,
            anchor: anchor.into(),
            x: 40,
            y: 60,
            name: String::new(),
            wayland_backend: WaylandBackend::Auto,
            follow_system_changes: true,
        }
    }

    #[test]
    fn mirror_mode_always_mirrors() {
        let decision = decide(&x11_facts(), &VirtualDisplayConfig::default()).unwrap();
        assert!(matches!(decision, DisplayDecision::Mirror { .. }));
    }

    #[test]
    fn x11_with_randr_monitors_gets_a_bare_monitor() {
        assert_eq!(
            decide(&x11_facts(), &extend()).unwrap(),
            DisplayDecision::Extend(ExtendBackend::X11Randr(X11Tier::BareMonitor))
        );
    }

    #[test]
    fn x11_with_a_spare_output_gets_a_real_output() {
        let facts = DisplayFacts {
            spare_output: Some(OutputId(9)),
            ..x11_facts()
        };
        assert_eq!(
            decide(&facts, &extend()).unwrap(),
            DisplayDecision::Extend(ExtendBackend::X11Randr(X11Tier::RealOutput(OutputId(9))))
        );
    }

    #[test]
    fn x11_without_randr_monitors_falls_back_to_mirror_with_a_reason() {
        for version in [None, Some((1, 4))] {
            let facts = DisplayFacts {
                randr_version: version,
                ..x11_facts()
            };
            match decide(&facts, &extend()).unwrap() {
                DisplayDecision::Mirror { reason } => assert!(reason.contains("RandR"), "{reason}"),
                other => panic!("expected mirror, got {other:?}"),
            }
        }
    }

    #[test]
    fn fail_policy_turns_a_missing_mechanism_into_an_error() {
        let config = VirtualDisplayConfig {
            fallback: FallbackPolicy::Fail,
            ..extend()
        };
        let facts = DisplayFacts {
            randr_version: None,
            ..x11_facts()
        };
        assert!(matches!(
            decide(&facts, &config),
            Err(DisplayPolicyError::NoVirtualDisplay(_))
        ));
    }

    #[test]
    fn gnome_picks_mutter_even_when_the_portal_offers_nothing() {
        assert_eq!(
            decide(&gnome_headless_facts(), &extend()).unwrap(),
            DisplayDecision::Extend(ExtendBackend::Mutter)
        );
    }

    #[test]
    fn sway_picks_the_wlroots_headless_output() {
        assert_eq!(
            decide(&sway_facts(), &extend()).unwrap(),
            DisplayDecision::Extend(ExtendBackend::Wlroots)
        );
    }

    #[test]
    fn other_wayland_compositors_use_the_portal_only_when_it_offers_a_virtual_source() {
        let kde = DisplayFacts {
            server: DisplayServer::Wayland,
            desktop: "KDE".into(),
            portal_source_types: Some(7),
            ..DisplayFacts::default()
        };
        assert_eq!(
            decide(&kde, &extend()).unwrap(),
            DisplayDecision::Extend(ExtendBackend::Portal)
        );
        let no_virtual = DisplayFacts {
            portal_source_types: Some(3),
            ..kde.clone()
        };
        assert!(matches!(
            decide(&no_virtual, &extend()).unwrap(),
            DisplayDecision::Mirror { .. }
        ));
        let unprobed = DisplayFacts {
            portal_source_types: None,
            ..kde
        };
        assert!(matches!(
            decide(&unprobed, &extend()).unwrap(),
            DisplayDecision::Mirror { .. }
        ));
    }

    #[test]
    fn forced_backends_follow_their_own_support() {
        let portal = VirtualDisplayConfig {
            wayland_backend: WaylandBackend::Portal,
            ..extend()
        };
        // Headless GNOME: the portal reports no source types, so a forced portal is refused.
        assert!(matches!(
            decide(&gnome_headless_facts(), &portal).unwrap(),
            DisplayDecision::Mirror { .. }
        ));
        // An unprobed portal is tried when forced.
        let unprobed = DisplayFacts {
            portal_source_types: None,
            ..gnome_headless_facts()
        };
        assert_eq!(
            decide(&unprobed, &portal).unwrap(),
            DisplayDecision::Extend(ExtendBackend::Portal)
        );
        let mutter = VirtualDisplayConfig {
            wayland_backend: WaylandBackend::Mutter,
            ..extend()
        };
        assert_eq!(
            decide(&gnome_headless_facts(), &mutter).unwrap(),
            DisplayDecision::Extend(ExtendBackend::Mutter)
        );
        assert!(matches!(
            decide(&sway_facts(), &mutter).unwrap(),
            DisplayDecision::Mirror { .. }
        ));
    }

    #[test]
    fn no_session_cannot_host_a_monitor() {
        let facts = DisplayFacts::default();
        assert!(matches!(
            virtual_support(&facts, WaylandBackend::Auto),
            VirtualSupport::Unsupported(_)
        ));
        assert!(matches!(
            decide(&facts, &extend()).unwrap(),
            DisplayDecision::Mirror { .. }
        ));
    }

    #[test]
    fn support_reports_unknown_for_an_unprobed_portal() {
        let facts = DisplayFacts {
            server: DisplayServer::Wayland,
            desktop: "KDE".into(),
            ..DisplayFacts::default()
        };
        assert_eq!(
            virtual_support(&facts, WaylandBackend::Portal),
            VirtualSupport::Unknown
        );
    }

    #[test]
    fn placement_puts_the_monitor_beside_the_anchor() {
        let facts = x11_facts();
        let at = |placement, anchor| place_monitor(&facts, &settings(placement, anchor)).unwrap();
        assert_eq!(
            at(Placement::RightOf, "primary"),
            Rect::new(4480, 0, 1280, 720)
        );
        assert_eq!(
            at(Placement::RightOf, "DP-1"),
            Rect::new(1920, 0, 1280, 720)
        );
        assert_eq!(
            at(Placement::LeftOf, "HDMI-1"),
            Rect::new(640, 0, 1280, 720)
        );
        assert_eq!(at(Placement::Above, "DP-1"), Rect::new(0, -720, 1280, 720));
        assert_eq!(
            at(Placement::Below, "HDMI-1"),
            Rect::new(1920, 1440, 1280, 720)
        );
    }

    #[test]
    fn placement_at_uses_the_configured_position_and_needs_no_anchor() {
        let rect = place_monitor(
            &DisplayFacts::default(),
            &settings(Placement::At, "nowhere"),
        )
        .unwrap();
        assert_eq!(rect, Rect::new(40, 60, 1280, 720));
    }

    #[test]
    fn placement_with_an_unknown_anchor_is_an_error() {
        assert_eq!(
            place_monitor(&x11_facts(), &settings(Placement::RightOf, "VGA-9")),
            Err(DisplayPolicyError::UnknownAnchor("VGA-9".into()))
        );
        assert_eq!(
            place_monitor(
                &DisplayFacts::default(),
                &settings(Placement::RightOf, "primary")
            ),
            Err(DisplayPolicyError::UnknownAnchor("primary".into()))
        );
    }

    #[test]
    fn placement_uses_the_monitor_size_of_the_live_settings() {
        let mut live = settings(Placement::RightOf, "DP-1");
        live.geometry = DisplayGeometry::new(900, 1200, Orientation::Portrait).unwrap();
        assert_eq!(
            place_monitor(&x11_facts(), &live).unwrap(),
            Rect::new(1920, 0, 900, 1200)
        );
    }
}
