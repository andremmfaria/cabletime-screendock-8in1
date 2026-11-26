//! The Xorg dummy test server: the helper starts, reports one output and RandR 1.5 or newer,
//! lets the framebuffer grow past its configured size and takes monitors, and facts gathered
//! from it match what `xrandr` reports.

use crate::support::{require_xorg, xrandr, Painter, XorgDummy};
use screendockd::adapters::capture::x11_shm::X11ShmCapture;
use screendockd::adapters::display::{
    recover, x11_facts, RecoveryOutcome, RecoveryStore, SystemDisplayProvider,
};
use screendockd::ports::capture::{CaptureArea, CaptureBackend, CaptureTarget};
use screendockd::ports::display::{
    DisplayProvider, DisplayServer, ExtendBackend, MonitorName, MonitorSpec, OutputId, Rect,
    VirtualMonitor, X11Tier,
};

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

const RED: u32 = 0x00ff_0000;
const GREEN: u32 = 0x0000_ff00;

struct Fixture {
    xorg: XorgDummy,
    provider: SystemDisplayProvider,
    store: RecoveryStore,
    _dir: tempfile::TempDir,
}

fn fixture() -> Option<Fixture> {
    let xorg = require_xorg()?;
    let dir = tempfile::tempdir().unwrap();
    let store = RecoveryStore::new(dir.path().join("run/vdisplay-state.json"));
    let provider = SystemDisplayProvider::for_x11_display(xorg.display.clone())
        .with_recovery_store(store.clone());
    Some(Fixture {
        xorg,
        provider,
        store,
        _dir: dir,
    })
}

fn spec(rect: Rect) -> MonitorSpec {
    MonitorSpec::new(MonitorName::new("HDMI Extender"), rect)
}

fn create(f: &Fixture, tier: X11Tier, rect: Rect) -> Box<dyn VirtualMonitor> {
    f.provider
        .virtual_display(ExtendBackend::X11Randr(tier))
        .expect("display")
        .create(&spec(rect))
        .expect("create monitor")
}

fn monitor_count(f: &Fixture) -> usize {
    xrandr(&f.xorg.display, &["--listmonitors"])
        .lines()
        .filter(|l| l.contains('/'))
        .count()
}

fn assert_restored(f: &Fixture) {
    assert_eq!(monitor_count(f), 1);
    assert!(xrandr(&f.xorg.display, &[]).contains("current 1280 x 720"));
    assert!(!f.store.path().exists());
}

struct CapturedPixels {
    top_left: Option<(u8, u8, u8)>,
    bottom_right: Option<(u8, u8, u8)>,
    size: (u32, u32),
}

fn captured_pixels(f: &Fixture, rect: Rect) -> CapturedPixels {
    let painter = Painter::connect(&f.xorg.display);
    painter.fill(
        RED,
        rect.x as i16,
        rect.y as i16,
        rect.width as u16,
        rect.height as u16,
    );
    painter.fill(GREEN, rect.x as i16, rect.y as i16, 10, 10);
    painter.sync();
    let mut capture = X11ShmCapture::connect(Some(&f.xorg.display), 0).unwrap();
    capture
        .retarget(CaptureTarget {
            area: CaptureArea::Region(rect),
            fps: 0,
        })
        .unwrap();
    let frame = capture.next_frame().unwrap();
    CapturedPixels {
        top_left: frame.pixel_rgb(0, 0),
        bottom_right: frame.pixel_rgb(rect.width - 1, rect.height - 1),
        size: (frame.width, frame.height),
    }
}

#[test]
fn bare_monitor_is_listed_captured_exactly_and_removed_on_drop() {
    let Some(f) = fixture() else { return };
    let rect = Rect::new(1280, 0, 1280, 720);
    let monitor = create(&f, X11Tier::BareMonitor, rect);
    let listed = xrandr(&f.xorg.display, &["--listmonitors"]);
    assert!(listed.contains("HDMI Extender"), "{listed}");
    assert!(listed.contains("1280/338x720/190+1280+0"), "{listed}");
    assert!(f.store.path().exists());
    assert_eq!(monitor.rect(), rect);
    let pixels = captured_pixels(&f, rect);
    assert_eq!(pixels.top_left, Some((0, 255, 0)));
    assert_eq!(pixels.bottom_right, Some((255, 0, 0)));
    assert_eq!(pixels.size, (1280, 720));
    drop(monitor);
    assert_restored(&f);
}

#[test]
fn output_monitor_is_a_real_output_captured_exactly_and_removed_on_drop() {
    let Some(f) = fixture() else { return };
    let facts = x11_facts(Some(&f.xorg.display)).unwrap();
    let spare = facts.spare_output.expect("a spare output");
    let rect = Rect::new(1280, 0, 1024, 600);
    let monitor = create(&f, X11Tier::RealOutput(spare), rect);
    let listed = xrandr(&f.xorg.display, &["--listmonitors"]);
    assert!(
        listed.contains("1024/") && listed.contains("x600/"),
        "{listed}"
    );
    assert!(listed.contains("+1280+0"), "{listed}");
    let after = x11_facts(Some(&f.xorg.display)).unwrap();
    assert_eq!(after.outputs.len(), 2);
    assert_eq!(after.outputs[1].rect, rect);
    let pixels = captured_pixels(&f, rect);
    assert_eq!(pixels.top_left, Some((0, 255, 0)));
    assert_eq!(pixels.bottom_right, Some((255, 0, 0)));
    assert_eq!(pixels.size, (1024, 600));
    drop(monitor);
    assert_restored(&f);
    assert!(!xrandr(&f.xorg.display, &[]).contains("screendockd-"));
}

#[test]
fn an_output_the_server_refuses_falls_back_to_a_bare_monitor() {
    let Some(f) = fixture() else { return };
    let monitor = create(
        &f,
        X11Tier::RealOutput(OutputId(1)),
        Rect::new(1280, 0, 1280, 720),
    );
    let listed = xrandr(&f.xorg.display, &["--listmonitors"]);
    assert!(listed.contains("HDMI Extender 1280/"), "{listed}");
    drop(monitor);
    assert_restored(&f);
}

#[test]
fn resizing_changes_the_monitor_on_the_server_in_both_tiers() {
    let Some(f) = fixture() else { return };
    let spare = x11_facts(Some(&f.xorg.display))
        .unwrap()
        .spare_output
        .unwrap();
    for tier in [X11Tier::BareMonitor, X11Tier::RealOutput(spare)] {
        let mut monitor = create(&f, tier, Rect::new(1280, 0, 640, 360));
        monitor
            .resize(&spec(Rect::new(1280, 0, 1600, 900)))
            .unwrap();
        assert_eq!(monitor.rect(), Rect::new(1280, 0, 1600, 900));
        let listed = xrandr(&f.xorg.display, &["--listmonitors"]);
        assert!(
            listed.contains("1600/") && listed.contains("x900/"),
            "{tier:?} {listed}"
        );
        assert!(xrandr(&f.xorg.display, &[]).contains("current 2880 x 900"));
        drop(monitor);
        assert_restored(&f);
    }
}

#[test]
fn recovery_removes_a_monitor_that_a_killed_daemon_left_behind() {
    let Some(f) = fixture() else { return };
    let spare = x11_facts(Some(&f.xorg.display))
        .unwrap()
        .spare_output
        .unwrap();
    for tier in [X11Tier::BareMonitor, X11Tier::RealOutput(spare)] {
        let monitor = create(&f, tier, Rect::new(1280, 0, 1280, 720));
        // A killed process never runs the drop code.
        std::mem::forget(monitor);
        assert_eq!(monitor_count(&f), 2);
        assert!(f.store.path().exists());
        assert_eq!(recover(&f.store), Ok(RecoveryOutcome::Restored));
        assert_restored(&f);
        assert_eq!(recover(&f.store), Ok(RecoveryOutcome::NothingToDo));
    }
}

#[test]
fn a_new_monitor_first_undoes_the_state_an_earlier_one_of_this_process_left() {
    let Some(f) = fixture() else { return };
    let first = create(&f, X11Tier::BareMonitor, Rect::new(1280, 0, 1280, 720));
    // The state file belongs to this process, so creating the second monitor first undoes the
    // first; dropping the stale handle afterwards changes nothing more.
    let second = create(&f, X11Tier::BareMonitor, Rect::new(1280, 0, 1280, 720));
    assert_eq!(monitor_count(&f), 2);
    drop(second);
    drop(first);
}
