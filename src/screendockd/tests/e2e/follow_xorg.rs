//! The daemon following its virtual monitor on a rootless Xorg with the dummy driver: a mode
//! change made with `xrandr` becomes one dock cycle with a rebuilt encoder and a new keyframe,
//! the daemon's own changes do not loop, and `follow_system_changes = false` leaves the dock
//! alone.
//!
//! Skips when Xorg with the dummy driver is unavailable (`SCREENDOCK_REQUIRE_XORG=1` makes that
//! a failure).

use std::ops::Deref;
use std::time::{Duration, Instant};

use screendockd::adapters::display::{RecoveryStore, SystemDisplayProvider};
use screendockd::app::daemon::DaemonReport;
use screendockd::ports::capture::CaptureArea;
use screendockd::ports::display::Rect;
use screendockd::protocol::control::{ControlRequest, ControlResponse};
use screendockd::protocol::framing::PictureSize;
use screendockd::testing::fake_dock::FakeDock;

use crate::follow_rig::{rig_with, wait_for, Rig};
use crate::support::{require_xorg, xrandr, XorgDummy};

struct XorgRig {
    xorg: XorgDummy,
    rig: Rig,
    _dir: tempfile::TempDir,
}

impl Deref for XorgRig {
    type Target = Rig;
    fn deref(&self) -> &Rig {
        &self.rig
    }
}

fn rig(follow: bool) -> Option<XorgRig> {
    let xorg = require_xorg()?;
    let dir = tempfile::tempdir().unwrap();
    let store = RecoveryStore::new(dir.path().join("run/vdisplay-state.json"));
    let provider =
        SystemDisplayProvider::for_x11_display(xorg.display.clone()).with_recovery_store(store);
    Some(XorgRig {
        rig: rig_with(provider, follow),
        xorg,
        _dir: dir,
    })
}

impl XorgRig {
    /// Gives the monitor's output a 1600x900 mode with `xrandr`, the way a display settings
    /// dialog does.
    fn change_mode_with_xrandr(&self) {
        let d = &self.xorg.display;
        let output = &screendockd::adapters::display::x11_facts(Some(d))
            .unwrap()
            .outputs[1]
            .name;
        xrandr(d, &["--fb", "3000x1000"]);
        xrandr(
            d,
            &[
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
            ],
        );
        xrandr(d, &["--addmode", output, "1600x900t"]);
        xrandr(d, &["--output", output, "--mode", "1600x900t"]);
    }

    fn finish(self) -> (DaemonReport, FakeDock) {
        self.rig.finish()
    }
}

#[test]
fn an_xrandr_mode_change_runs_one_dock_cycle_with_a_rebuilt_encoder_and_a_new_keyframe() {
    let Some(rig) = rig(true) else { return };
    wait_for("video", || rig.dock.video_stats().messages > 5);
    assert_eq!(rig.dock.launch_sizes(), [(1280, 720)]);

    let changed = Instant::now();
    rig.change_mode_with_xrandr();
    wait_for("the dock cycle at the new size", || {
        rig.dock.launch_sizes() == [(1280, 720), (1600, 900)]
    });
    let detected_in = changed.elapsed();
    assert!(
        detected_in < Duration::from_secs(2),
        "the cycle began {detected_in:?} after the change"
    );
    wait_for("video at the new size", || {
        rig.dock.video_stats().sizes == [(1280, 720), (1600, 900)]
    });
    // Nothing feeds back: no second cycle follows.
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(rig.mode(), "extend");
    let (emitted, built, targets) = (rig.emitted.clone(), rig.built.clone(), rig.targets.clone());
    let (report, dock) = rig.finish();
    assert_eq!(report.handshakes, 2);
    assert_eq!(dock.launch_sizes(), [(1280, 720), (1600, 900)]);
    let built = built.lock().unwrap();
    assert_eq!(
        built.len(),
        3,
        "the initial encoder, the probe and the new one"
    );
    assert_eq!(built[2].geometry.size(), PictureSize::new(1600, 900));
    let emitted = emitted.lock().unwrap();
    let first_new = emitted.iter().find(|(number, ..)| *number == 2).unwrap();
    assert_eq!(
        (first_new.1, first_new.2),
        (true, 7),
        "keyframe with parameter sets"
    );
    let last = targets.lock().unwrap().last().copied().unwrap();
    assert_eq!(
        last.area,
        CaptureArea::Region(Rect::new(1280, 0, 1600, 900))
    );
    let beats = dock.heartbeat_times();
    let widest = beats.windows(2).map(|w| w[1] - w[0]).max().unwrap();
    assert!(widest < Duration::from_secs(2), "heartbeat gap {widest:?}");
}

#[test]
fn a_size_set_with_ctl_runs_one_dock_cycle_and_the_echo_is_not_followed() {
    let Some(rig) = rig(true) else { return };
    wait_for("video", || rig.dock.video_stats().messages > 5);
    let answer = rig.ask(ControlRequest::Set {
        key: "resolution".into(),
        value: "1600x900".into(),
    });
    assert!(
        matches!(answer, ControlResponse::Applied { .. }),
        "{answer:?}"
    );
    wait_for("video at the new size", || {
        rig.dock.video_stats().sizes == [(1280, 720), (1600, 900)]
    });
    let listed = xrandr(&rig.xorg.display, &["--listmonitors"]);
    assert!(listed.contains("1600/"), "{listed}");
    std::thread::sleep(Duration::from_millis(1500));
    let (report, dock) = rig.finish();
    assert_eq!(report.handshakes, 2, "the change ran one cycle");
    assert_eq!(dock.launch_sizes(), [(1280, 720), (1600, 900)]);
}

#[test]
fn with_following_off_an_xrandr_mode_change_leaves_the_dock_alone() {
    let Some(rig) = rig(false) else { return };
    wait_for("video", || rig.dock.video_stats().messages > 5);
    rig.change_mode_with_xrandr();
    std::thread::sleep(Duration::from_millis(1500));
    let (report, dock) = rig.finish();
    assert_eq!(report.handshakes, 1);
    assert_eq!(dock.launch_sizes(), [(1280, 720)]);
    assert_eq!(dock.video_stats().sizes, [(1280, 720)]);
}

#[test]
fn turning_the_monitor_off_with_xrandr_reverts_the_daemon_to_mirror() {
    let Some(rig) = rig(true) else { return };
    wait_for("video", || rig.dock.video_stats().messages > 5);
    let d = &rig.xorg.display;
    let output = screendockd::adapters::display::x11_facts(Some(d))
        .unwrap()
        .outputs[1]
        .name
        .clone();
    xrandr(d, &["--output", &output, "--off"]);
    wait_for("mirror mode", || rig.mode() == "mirror");
    let (report, _dock) = rig.finish();
    assert_eq!(report.handshakes, 1, "the picture size did not change");
}
