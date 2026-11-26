//! The Xorg dummy test server: the helper starts, reports one output and RandR 1.5 or newer,
//! lets the framebuffer grow past its configured size and takes monitors, and facts gathered
//! from it match what `xrandr` reports.

use crate::support::{require_xorg, xrandr};
use screendockd::adapters::display::x11_facts;
use screendockd::ports::display::{DisplayServer, Rect};

#[test]
fn dummy_server_facts_describe_the_active_output_and_a_spare_one() {
    let Some(xorg) = require_xorg() else { return };
    let facts = x11_facts(Some(&xorg.display)).expect("facts");
    assert_eq!(facts.server, DisplayServer::X11);
    assert!(
        facts.randr_version >= Some((1, 5)),
        "{:?}",
        facts.randr_version
    );
    assert_eq!(facts.outputs.len(), 1);
    assert_eq!(facts.outputs[0].name, "DUMMY0");
    assert_eq!(facts.outputs[0].rect, Rect::new(0, 0, 1280, 720));
    assert!(facts.outputs[0].primary);
    // The other outputs of the dummy driver are disconnected and free, so one can take a mode.
    assert!(facts.spare_output.is_some());
}

#[test]
fn dummy_server_grows_its_framebuffer_past_the_configured_size() {
    let Some(xorg) = require_xorg() else { return };
    xrandr(&xorg.display, &["--fb", "6000x3000"]);
    assert!(xrandr(&xorg.display, &[]).contains("current 6000 x 3000"));
    xrandr(
        &xorg.display,
        &["--setmonitor", "VMON", "1280/300x720/170+2720+0", "none"],
    );
    assert!(xrandr(&xorg.display, &["--listmonitors"]).contains("VMON 1280/300x720/170+2720+0"));
}

#[test]
fn dummy_server_spare_output_takes_a_custom_mode_and_becomes_a_monitor() {
    let Some(xorg) = require_xorg() else { return };
    let d = &xorg.display;
    xrandr(d, &["--fb", "2560x720"]);
    xrandr(
        d,
        &[
            "--newmode",
            "1280x720t",
            "74.25",
            "1280",
            "1390",
            "1430",
            "1650",
            "720",
            "725",
            "730",
            "750",
            "+hsync",
            "+vsync",
        ],
    );
    xrandr(d, &["--addmode", "DUMMY1", "1280x720t"]);
    xrandr(
        d,
        &[
            "--output",
            "DUMMY1",
            "--mode",
            "1280x720t",
            "--pos",
            "1280x0",
        ],
    );
    let monitors = xrandr(d, &["--listmonitors"]);
    assert!(
        monitors.contains("DUMMY1 1280/339x720/190+1280+0"),
        "{monitors}"
    );
    let facts = x11_facts(Some(d)).expect("facts");
    assert_eq!(facts.outputs.len(), 2);
    assert_eq!(facts.outputs[1].rect, Rect::new(1280, 0, 1280, 720));
}
