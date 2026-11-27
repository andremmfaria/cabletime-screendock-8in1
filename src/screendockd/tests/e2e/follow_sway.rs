//! The daemon following its virtual monitor on headless sway: `swaymsg output <name> mode`
//! becomes one dock cycle with a rebuilt encoder and a new keyframe, the daemon's own changes
//! do not loop, and an unplugged output reverts to mirror.
//!
//! Skips when sway is unavailable (`SCREENDOCK_REQUIRE_SWAY=1` makes that a failure).

use std::time::{Duration, Instant};

use screendockd::adapters::display::SystemDisplayProvider;
use screendockd::ports::capture::CaptureArea;
use screendockd::ports::display::Rect;
use screendockd::protocol::control::{ControlRequest, ControlResponse};
use screendockd::protocol::framing::PictureSize;

use crate::follow_rig::{rig_with, wait_for};
use crate::support::{require_sway, SwayHeadless};

fn monitor_name(sway: &SwayHeadless) -> String {
    sway.output_names()
        .into_iter()
        .find(|n| n.starts_with("HEADLESS-") && n != "HEADLESS-1")
        .or_else(|| sway.output_names().into_iter().last())
        .expect("the daemon's output")
}

fn swaymsg(sway: &SwayHeadless, command: &str) {
    sway.ipc().run(command).expect("swaymsg");
}

#[test]
fn a_swaymsg_mode_change_runs_one_dock_cycle_with_a_rebuilt_encoder_and_a_new_keyframe() {
    let Some(sway) = require_sway() else { return };
    let provider = SystemDisplayProvider::for_sway(&sway.ipc_socket);
    let rig = rig_with(provider, true);
    wait_for("video", || rig.dock.video_stats().messages > 5);
    assert_eq!(rig.dock.launch_sizes(), [(1280, 720)]);
    let name = monitor_name(&sway);

    let changed = Instant::now();
    swaymsg(&sway, &format!("output {name} mode 1600x900"));
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
    // sway lays outputs out itself, so only the size is fixed.
    let CaptureArea::Region(rect) = last.area else {
        panic!("{:?}", last.area);
    };
    assert_eq!((rect.width, rect.height), (1600, 900));
    let beats = dock.heartbeat_times();
    let widest = beats.windows(2).map(|w| w[1] - w[0]).max().unwrap();
    assert!(widest < Duration::from_secs(2), "heartbeat gap {widest:?}");
}

#[test]
fn a_size_set_with_ctl_on_sway_runs_one_dock_cycle_and_the_echo_is_not_followed() {
    let Some(sway) = require_sway() else { return };
    let rig = rig_with(SystemDisplayProvider::for_sway(&sway.ipc_socket), true);
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
    std::thread::sleep(Duration::from_millis(1500));
    let (report, dock) = rig.finish();
    assert_eq!(report.handshakes, 2, "the change ran one cycle");
    assert_eq!(dock.launch_sizes(), [(1280, 720), (1600, 900)]);
}

#[test]
fn a_swaymsg_position_change_retargets_capture_without_a_dock_cycle() {
    let Some(sway) = require_sway() else { return };
    let rig = rig_with(SystemDisplayProvider::for_sway(&sway.ipc_socket), true);
    wait_for("video", || rig.dock.video_stats().messages > 5);
    let name = monitor_name(&sway);
    swaymsg(&sway, &format!("output {name} pos 0 720"));
    wait_for("capture at the new position", || {
        rig.targets.lock().unwrap().last().map(|t| t.area)
            == Some(CaptureArea::Region(Rect::new(0, 720, 1280, 720)))
    });
    let (report, dock) = rig.finish();
    assert_eq!(report.handshakes, 1);
    assert_eq!(dock.launch_sizes(), [(1280, 720)]);
}

#[test]
fn with_following_off_a_swaymsg_mode_change_leaves_the_dock_alone() {
    let Some(sway) = require_sway() else { return };
    let rig = rig_with(SystemDisplayProvider::for_sway(&sway.ipc_socket), false);
    wait_for("video", || rig.dock.video_stats().messages > 5);
    let name = monitor_name(&sway);
    swaymsg(&sway, &format!("output {name} mode 1600x900"));
    std::thread::sleep(Duration::from_millis(1500));
    let (report, dock) = rig.finish();
    assert_eq!(report.handshakes, 1);
    assert_eq!(dock.launch_sizes(), [(1280, 720)]);
}

#[test]
fn unplugging_the_output_reverts_the_daemon_to_mirror() {
    let Some(sway) = require_sway() else { return };
    let rig = rig_with(SystemDisplayProvider::for_sway(&sway.ipc_socket), true);
    wait_for("video", || rig.dock.video_stats().messages > 5);
    let name = monitor_name(&sway);
    swaymsg(&sway, &format!("output {name} unplug"));
    wait_for("mirror mode", || rig.mode() == "mirror");
    let (report, _dock) = rig.finish();
    assert_eq!(report.handshakes, 1, "the picture size did not change");
}
