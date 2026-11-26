//! The headless sway output as a virtual monitor: created at the placed rectangle, holding
//! pixels a second client can read, resized, switched with the display mode and unplugged
//! on drop.
//!
//! Skips when sway cannot run headless (`SCREENDOCK_REQUIRE_SWAY=1` makes that a failure). The
//! pixel checks read the output back with `grim`.

use crate::support::{require_sway, SwayHeadless};
use screendockd::adapters::display::{SystemDisplayProvider, WlrootsHeadlessDisplay};
use screendockd::app::monitor::MonitorManager;
use screendockd::core::display_policy::{decide, DisplayDecision};
use screendockd::core::reconfig::{
    plan_change_with, LiveSettings, PlanOptions, ReconfigPlan, SettingsPatch,
};
use screendockd::ports::capture::CaptureArea;
use screendockd::ports::display::{
    DisplayProvider, DisplayServer, ExtendBackend, MonitorName, MonitorSpec, Rect, VirtualDisplay,
};
use screendockd::utils::config::{load_from_path, VirtualDisplayConfig};
use screendockd::utils::types::DisplayMode;

fn spec(rect: Rect) -> MonitorSpec {
    MonitorSpec::new(MonitorName::new(""), rect)
}

fn display(sway: &SwayHeadless) -> WlrootsHeadlessDisplay {
    WlrootsHeadlessDisplay::new(&sway.ipc_socket)
}

fn rect_of(sway: &SwayHeadless, name: &str) -> Option<Rect> {
    sway.ipc()
        .outputs()
        .unwrap()
        .into_iter()
        .find(|o| o.name == name)
        .map(|o| o.rect)
}

/// The name of the one output that is not in `before`.
fn new_output(sway: &SwayHeadless, before: &[String]) -> String {
    let now = sway.output_names();
    let added: Vec<_> = now.iter().filter(|n| !before.contains(n)).collect();
    assert_eq!(added.len(), 1, "outputs before {before:?}, now {now:?}");
    added[0].clone()
}

#[test]
fn headless_output_is_created_at_the_requested_rectangle_and_unplugged_on_drop() {
    let Some(sway) = require_sway() else { return };
    let before = sway.output_names();
    let wanted = Rect::new(1280, 0, 1600, 900);
    let monitor = display(&sway).create(&spec(wanted)).unwrap();
    let name = new_output(&sway, &before);
    assert!(name.starts_with("HEADLESS-"), "{name}");
    assert_eq!(monitor.rect(), wanted);
    assert_eq!(rect_of(&sway, &name), Some(wanted));
    drop(monitor);
    assert_eq!(sway.output_names(), before);
}

#[test]
fn headless_output_holds_the_pixels_a_second_client_reads_at_every_size() {
    let Some(sway) = require_sway() else { return };
    let before = sway.output_names();
    let mut monitor = display(&sway)
        .create(&spec(Rect::new(1280, 0, 1280, 720)))
        .unwrap();
    let name = new_output(&sway, &before);
    sway.ipc()
        .run(&format!("output {name} bg #1060d0 solid_color"))
        .unwrap();
    // The background is painted on the next frame, so the first read may still be black.
    let mut shot = None;
    for _ in 0..30 {
        let Some(read) = sway.grim(&name) else {
            return;
        };
        let painted = read.2.get(..3) == Some(&[0x10, 0x60, 0xd0][..]);
        shot = Some(read);
        if painted {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let (width, height, rgba) = shot.unwrap();
    assert_eq!((width, height), (1280, 720));
    let at = |x: u32, y: u32| {
        let i = ((y * width + x) * 4) as usize;
        (rgba[i], rgba[i + 1], rgba[i + 2])
    };
    assert_eq!(at(640, 360), (0x10, 0x60, 0xd0));
    assert_eq!(at(0, 0), (0x10, 0x60, 0xd0));

    monitor.resize(&spec(Rect::new(1280, 0, 900, 720))).unwrap();
    let mut last_pixel = [0u8; 3];
    for _ in 0..30 {
        let (width, height, rgba) = sway.grim(&name).unwrap();
        assert_eq!((width, height), (900, 720));
        let last = ((height - 1) * width + (width - 1)) as usize * 4;
        last_pixel.copy_from_slice(&rgba[last..last + 3]);
        if last_pixel == [0x10, 0x60, 0xd0] {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(last_pixel, [0x10, 0x60, 0xd0]);
}

#[test]
fn resizing_the_headless_output_changes_size_and_position() {
    let Some(sway) = require_sway() else { return };
    let before = sway.output_names();
    let mut monitor = display(&sway)
        .create(&spec(Rect::new(1280, 0, 1280, 720)))
        .unwrap();
    let name = new_output(&sway, &before);
    let moved = Rect::new(0, 720, 1920, 1080);
    monitor.resize(&spec(moved)).unwrap();
    assert_eq!(monitor.rect(), moved);
    assert_eq!(rect_of(&sway, &name), Some(moved));
}

#[test]
fn several_headless_monitors_are_independent() {
    let Some(sway) = require_sway() else { return };
    let before = sway.output_names();
    let first = display(&sway)
        .create(&spec(Rect::new(1280, 0, 1280, 720)))
        .unwrap();
    let second = display(&sway)
        .create(&spec(Rect::new(2560, 0, 800, 600)))
        .unwrap();
    assert_eq!(sway.output_names().len(), before.len() + 2);
    drop(first);
    assert_eq!(sway.output_names().len(), before.len() + 1);
    assert_eq!(second.rect(), Rect::new(2560, 0, 800, 600));
    drop(second);
    assert_eq!(sway.output_names(), before);
}

#[test]
fn provider_reads_the_sway_outputs_and_extend_chooses_the_wlroots_display() {
    let Some(sway) = require_sway() else { return };
    let provider = SystemDisplayProvider::for_sway(&sway.ipc_socket);
    let facts = provider.facts().unwrap();
    assert_eq!(facts.server, DisplayServer::Wayland);
    assert!(facts.sway_socket);
    assert_eq!(facts.outputs[0].rect, Rect::new(0, 0, 1280, 720));
    assert!(facts.outputs[0].primary);
    let config = VirtualDisplayConfig {
        mode: DisplayMode::Extend,
        ..VirtualDisplayConfig::default()
    };
    assert_eq!(
        decide(&facts, &config).unwrap(),
        DisplayDecision::Extend(ExtendBackend::Wlroots)
    );
    let display = provider.virtual_display(ExtendBackend::Wlroots).unwrap();
    assert_eq!(display.name(), "wlroots");
}

fn shipped() -> LiveSettings {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/screendock_cfg.toml"
    );
    LiveSettings::try_from(&load_from_path(std::path::Path::new(path)).unwrap()).unwrap()
}

fn switch(old: &LiveSettings, mode: DisplayMode) -> (LiveSettings, ReconfigPlan) {
    let new = SettingsPatch {
        mode: Some(mode),
        ..SettingsPatch::default()
    }
    .apply_to(old)
    .unwrap();
    let plan = plan_change_with(old, &new, PlanOptions::default());
    (new, plan)
}

#[test]
fn switching_the_mode_creates_the_output_right_of_the_anchor_and_removes_it_again() {
    let Some(sway) = require_sway() else { return };
    let before = sway.output_names();
    let mut monitors = MonitorManager::new(
        Box::new(SystemDisplayProvider::for_sway(&sway.ipc_socket)),
        VirtualDisplayConfig::default(),
    );
    let mirror = shipped();
    assert_eq!(monitors.start(&mirror), Ok(CaptureArea::Whole));
    assert_eq!(sway.output_names(), before);

    let (extend, plan) = switch(&mirror, DisplayMode::Extend);
    let area = monitors.apply(&plan, &extend).unwrap();
    let wanted = Rect::new(1280, 0, 1280, 720);
    assert_eq!(area, CaptureArea::Region(wanted));
    let name = new_output(&sway, &before);
    assert_eq!(rect_of(&sway, &name), Some(wanted));

    let (back, plan) = switch(&extend, DisplayMode::Mirror);
    assert_eq!(monitors.apply(&plan, &back), Ok(CaptureArea::Whole));
    assert_eq!(sway.output_names(), before);
}

#[test]
fn dropping_the_manager_removes_the_output() {
    let Some(sway) = require_sway() else { return };
    let before = sway.output_names();
    let mut monitors = MonitorManager::new(
        Box::new(SystemDisplayProvider::for_sway(&sway.ipc_socket)),
        VirtualDisplayConfig::default(),
    );
    let (extend, plan) = switch(&shipped(), DisplayMode::Extend);
    monitors.apply(&plan, &extend).unwrap();
    assert_eq!(sway.output_names().len(), before.len() + 1);
    drop(monitors);
    assert_eq!(sway.output_names(), before);
}
