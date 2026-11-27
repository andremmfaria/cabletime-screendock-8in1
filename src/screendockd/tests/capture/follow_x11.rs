//! Changes made to a virtual monitor from outside the daemon, with `xrandr` as the outside
//! party on the Xorg dummy server: a new mode or geometry is reported once, the daemon's own
//! resize is not, and a removed monitor is reported as removed.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use crate::support::{require_xorg, xrandr, XorgDummy};
use screendockd::adapters::display::{x11_facts, RecoveryStore, SystemDisplayProvider};
use screendockd::ports::display::{
    DisplayEvent, DisplayProvider, ExtendBackend, MonitorName, MonitorSpec, Rect, VirtualMonitor,
    X11Tier,
};

const NAME: &str = "HDMI Extender";
const WITHIN: Duration = Duration::from_secs(2);
const QUIET: Duration = Duration::from_millis(900);

struct Rig {
    xorg: XorgDummy,
    monitor: Box<dyn VirtualMonitor>,
    events: Receiver<DisplayEvent>,
    _dir: tempfile::TempDir,
}

fn spec(rect: Rect) -> MonitorSpec {
    MonitorSpec::new(MonitorName::new(NAME), rect)
}

fn rig(tier: fn(&XorgDummy) -> X11Tier) -> Option<Rig> {
    let xorg = require_xorg()?;
    let dir = tempfile::tempdir().unwrap();
    let store = RecoveryStore::new(dir.path().join("run/vdisplay-state.json"));
    let provider =
        SystemDisplayProvider::for_x11_display(xorg.display.clone()).with_recovery_store(store);
    let mut monitor = provider
        .virtual_display(ExtendBackend::X11Randr(tier(&xorg)))
        .unwrap()
        .create(&spec(Rect::new(1280, 0, 1280, 720)))
        .unwrap();
    let (tx, events) = mpsc::channel();
    monitor.follow(Box::new(move |event| {
        let _ = tx.send(event);
    }));
    Some(Rig {
        xorg,
        monitor,
        events,
        _dir: dir,
    })
}

fn bare(_: &XorgDummy) -> X11Tier {
    X11Tier::BareMonitor
}

fn output(xorg: &XorgDummy) -> X11Tier {
    X11Tier::RealOutput(
        x11_facts(Some(&xorg.display))
            .unwrap()
            .spare_output
            .expect("a spare output"),
    )
}

fn resized(rect: Rect) -> DisplayEvent {
    DisplayEvent::Changed {
        rect,
        resized: true,
        moved: false,
    }
}

#[test]
fn a_bare_monitor_redefined_with_xrandr_is_reported_as_resized() {
    let Some(rig) = rig(bare) else { return };
    let d = &rig.xorg.display;
    xrandr(d, &["--fb", "3000x1000"]);
    xrandr(d, &["--delmonitor", NAME]);
    xrandr(
        d,
        &["--setmonitor", NAME, "1600/423x900/238+1280+0", "none"],
    );
    assert_eq!(
        rig.events.recv_timeout(WITHIN),
        Ok(resized(Rect::new(1280, 0, 1600, 900)))
    );
    assert!(rig.events.recv_timeout(QUIET).is_err(), "reported twice");
}

#[test]
fn a_bare_monitor_moved_with_xrandr_is_reported_as_moved() {
    let Some(rig) = rig(bare) else { return };
    let d = &rig.xorg.display;
    xrandr(d, &["--fb", "3000x1000"]);
    xrandr(d, &["--delmonitor", NAME]);
    xrandr(
        d,
        &["--setmonitor", NAME, "1280/338x720/190+1500+100", "none"],
    );
    assert_eq!(
        rig.events.recv_timeout(WITHIN),
        Ok(DisplayEvent::Changed {
            rect: Rect::new(1500, 100, 1280, 720),
            resized: false,
            moved: true
        })
    );
}

#[test]
fn a_bare_monitor_deleted_with_xrandr_is_reported_as_removed() {
    let Some(rig) = rig(bare) else { return };
    xrandr(&rig.xorg.display, &["--delmonitor", NAME]);
    assert_eq!(rig.events.recv_timeout(WITHIN), Ok(DisplayEvent::Removed));
}

#[test]
fn an_output_given_another_mode_with_xrandr_is_reported_as_resized() {
    let Some(rig) = rig(output) else { return };
    let d = &rig.xorg.display;
    let name = x11_facts(Some(d)).unwrap().outputs[1].name.clone();
    xrandr(d, &["--fb", "3000x1000"]);
    let modeline = [
        "--newmode",
        "1600x900t",
        "98.21",
        "1600",
        "1648",
        "1680",
        "1760",
        "900",
        "903",
        "906",
        "930",
        "+hsync",
        "-vsync",
    ];
    xrandr(d, &modeline);
    xrandr(d, &["--addmode", &name, "1600x900t"]);
    xrandr(d, &["--output", &name, "--mode", "1600x900t"]);
    assert_eq!(
        rig.events.recv_timeout(WITHIN),
        Ok(resized(Rect::new(1280, 0, 1600, 900)))
    );
    assert!(rig.events.recv_timeout(QUIET).is_err(), "reported twice");
}

#[test]
fn resizing_the_monitor_ourselves_reports_nothing_for_either_tier() {
    for tier in [bare as fn(&XorgDummy) -> X11Tier, output] {
        let Some(mut rig) = rig(tier) else { return };
        rig.monitor
            .resize(&spec(Rect::new(1280, 0, 1024, 600)))
            .unwrap();
        rig.monitor
            .resize(&spec(Rect::new(1280, 0, 1280, 720)))
            .unwrap();
        assert!(
            rig.events.recv_timeout(QUIET).is_err(),
            "the daemon's own change was reported"
        );
    }
}

#[test]
fn a_change_the_daemon_observed_is_not_reported_again_and_not_undone() {
    let Some(mut rig) = rig(bare) else { return };
    let d = &rig.xorg.display;
    xrandr(d, &["--fb", "3000x1000"]);
    xrandr(d, &["--delmonitor", NAME]);
    xrandr(
        d,
        &["--setmonitor", NAME, "1600/423x900/238+1280+0", "none"],
    );
    let Ok(DisplayEvent::Changed { rect, .. }) = rig.events.recv_timeout(WITHIN) else {
        panic!("no change reported");
    };
    rig.monitor.observe(rect);
    assert_eq!(rig.monitor.rect(), rect);
    xrandr(
        &rig.xorg.display,
        &["--setmonitor", "OTHER", "100/26x100/26+0+0", "none"],
    );
    assert!(rig.events.recv_timeout(QUIET).is_err());
}
