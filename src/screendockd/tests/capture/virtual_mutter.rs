//! The Mutter virtual monitor of a headless GNOME Shell: a `RecordVirtual` stream whose
//! PipeWire node delivers frames to the Wayland capture at the size the capture asks for,
//! and goes away when the monitor is dropped.
//!
//! Skips when gnome-shell or pipewire cannot run here (`SCREENDOCK_REQUIRE_MUTTER=1` makes
//! that a failure). A headless shell has no client on the virtual monitor, so the frames
//! show the shell's own background; `gst-launch-1.0` reads the same node as a second opinion
//! when it is installed.

use std::sync::{mpsc, Arc};
use std::time::Duration;

use crate::support::{require_mutter, MutterHeadless};
use screendockd::adapters::capture::wayland_portal::WaylandPortalCapture;
use screendockd::adapters::display::{MutterVirtualDisplay, ZbusMutterConnector};
use screendockd::ports::capture::{CaptureArea, CaptureBackend, CaptureTarget, Frame, PixelFormat};
use screendockd::ports::display::{MonitorName, MonitorSpec, Rect, VirtualDisplay};

const FRAME_WAIT: Duration = Duration::from_secs(60);

fn display(shell: &MutterHeadless) -> MutterVirtualDisplay {
    MutterVirtualDisplay::new(Arc::new(ZbusMutterConnector::with_address(
        shell.bus_address.clone(),
    )))
}

fn spec(width: u32, height: u32) -> MonitorSpec {
    MonitorSpec::new(MonitorName::new(""), Rect::new(0, 0, width, height))
}

/// Reads one frame on a helper thread, so a stream that never delivers fails the test
/// instead of hanging it.
fn frame_within(
    mut capture: Box<dyn CaptureBackend>,
    wait: Duration,
) -> (Box<dyn CaptureBackend>, Option<Frame>) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let frame = capture.next_frame();
        let _ = tx.send((capture, frame));
    });
    match rx.recv_timeout(wait) {
        Ok((capture, frame)) => (capture, Some(frame.expect("a frame, not a stream error"))),
        Err(_) => panic!("no frame within {wait:?}"),
    }
}

fn first_pixel(frame: &Frame) -> [u8; 4] {
    let data = &frame.data;
    [data[0], data[1], data[2], data[3]]
}

fn is_uniform(frame: &Frame) -> bool {
    let first = first_pixel(frame);
    let row_bytes = frame.width as usize * 4;
    (0..frame.height as usize).all(|y| {
        let row = &frame.data[y * frame.stride..y * frame.stride + row_bytes];
        row.chunks(4).all(|p| p == first)
    })
}

#[test]
fn mutter_monitor_streams_frames_at_the_requested_size() {
    let Some(shell) = require_mutter() else {
        return;
    };
    let monitor = display(&shell).create(&spec(1280, 720)).unwrap();
    let node = monitor.pipewire_node().expect("a stream backed monitor");
    assert_eq!(monitor.rect(), Rect::new(0, 0, 1280, 720));

    let capture =
        WaylandPortalCapture::connect_stream(Some(&shell.pipewire_remote), node, 1280, 720, 25)
            .unwrap();
    let (capture, frame) = frame_within(Box::new(capture), FRAME_WAIT);
    let frame = frame.unwrap();
    assert_eq!((frame.width, frame.height), (1280, 720));
    assert_eq!(frame.format, PixelFormat::Bgrx8888);
    // Not black: the shell paints its background on the monitor.
    let pixel = first_pixel(&frame);
    assert!(pixel[..3].iter().any(|&c| c != 0), "all black: {pixel:?}");
    eprintln!(
        "mutter frame: {}x{} {:?} first pixel {pixel:?} uniform {}",
        frame.width,
        frame.height,
        frame.format,
        is_uniform(&frame)
    );

    // A second monitor read by GStreamer shows the same size and the same pixels. A node
    // delivers its first frame to the first consumer, so the reference gets its own node.
    drop(capture);
    let reference_monitor = display(&shell).create(&spec(1280, 720)).unwrap();
    let reference_node = reference_monitor.pipewire_node().unwrap();
    if let Some(reference) = shell.gst_frame(reference_node, 1280, 720) {
        assert_eq!(reference.len(), 1280 * 720 * 4, "gst frame size");
        assert_eq!(
            &reference[..4],
            &pixel,
            "gst and capture disagree on pixel 0"
        );
    } else if std::env::var_os("SCREENDOCK_REQUIRE_MUTTER").is_some() {
        panic!("gst-launch-1.0 produced no frame");
    } else {
        eprintln!("no gst reference: pipewiresrc is not available");
    }
}

#[test]
fn mutter_stream_takes_the_size_the_capture_asks_for() {
    let Some(shell) = require_mutter() else {
        return;
    };
    let monitor = display(&shell).create(&spec(1600, 900)).unwrap();
    let node = monitor.pipewire_node().unwrap();
    let capture =
        WaylandPortalCapture::connect_stream(Some(&shell.pipewire_remote), node, 1600, 900, 25)
            .unwrap();
    let (_capture, frame) = frame_within(Box::new(capture), FRAME_WAIT);
    let frame = frame.unwrap();
    assert_eq!((frame.width, frame.height), (1600, 900));
}

#[test]
fn retargeting_to_another_stream_switches_the_frames() {
    let Some(shell) = require_mutter() else {
        return;
    };
    let first = display(&shell).create(&spec(1280, 720)).unwrap();
    let capture = WaylandPortalCapture::connect_stream(
        Some(&shell.pipewire_remote),
        first.pipewire_node().unwrap(),
        1280,
        720,
        25,
    )
    .unwrap();
    let (mut capture, frame) = frame_within(Box::new(capture), FRAME_WAIT);
    assert_eq!(frame.unwrap().width, 1280);

    let second = display(&shell).create(&spec(800, 600)).unwrap();
    capture
        .retarget(CaptureTarget {
            area: CaptureArea::Stream {
                node: second.pipewire_node().unwrap(),
                width: 800,
                height: 600,
            },
            fps: 25,
        })
        .unwrap();
    // Frames of the first stream may still sit in the queue.
    loop {
        let (back, frame) = frame_within(capture, FRAME_WAIT);
        capture = back;
        let frame = frame.unwrap();
        if frame.width == 800 {
            assert_eq!(frame.height, 600);
            break;
        }
    }
}

#[test]
fn mutter_stream_node_is_removed_when_the_monitor_drops() {
    let Some(shell) = require_mutter() else {
        return;
    };
    let monitor = display(&shell).create(&spec(1280, 720)).unwrap();
    let node = u64::from(monitor.pipewire_node().unwrap());
    let Some(before) = shell.pipewire_node_ids() else {
        eprintln!("skipping the node check: pw-dump is not installed");
        return;
    };
    assert!(before.contains(&node), "node {node} not in {before:?}");
    drop(monitor);
    let mut gone = false;
    for _ in 0..50 {
        if !shell.pipewire_node_ids().unwrap().contains(&node) {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(gone, "node {node} still exists after the monitor dropped");
}
