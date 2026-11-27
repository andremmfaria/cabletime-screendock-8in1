//! A simulated dock for tests and dry runs, speaking the MI_00 session protocol over a Unix
//! datagram socket pair that stands in for the hidraw node (datagrams keep report boundaries
//! like a real hidraw read does). An optional second socket pair stands in for MI_01.
//!
//! Behavior is configurable: a fresh dock announces `notify-app-accessories-are-ready` until it
//! sees `app-launched`, answers the requests, sends its EDID passthrough and echoes heartbeats;
//! a dock that already holds a session only heartbeats and answers nothing. The fake records
//! every host message byte for byte and checks the host's report layout and the video header
//! layout, so tests can assert message exactness and that no video arrives before the sink is
//! started.
//!
//! This is test support compiled into the library so unit tests, integration tests and dry
//! runs share one implementation; nothing in the daemon uses it.

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixDatagram;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::framing::{frame_message, PictureSize, ReportId, REPORT_SIZE};
use crate::hidraw_io::HidrawHandle;
use crate::session::InboundAssembler;

/// What the fake answers to `notify-projection-started`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartedReply {
    /// `"result":"isAllowed"`.
    Allowed,
    /// `"result":"isDenied"`.
    Denied,
    /// No reply at all.
    Silent,
}

/// Fake dock behavior.
#[derive(Debug, Clone)]
pub struct FakeDockConfig {
    /// Announce ready (plus tx-data) every `ready_interval` while no session is held.
    pub announce_ready: bool,
    pub ready_interval: Duration,
    /// Dock heartbeat period; `None` makes a silent dock.
    pub heartbeat_interval: Option<Duration>,
    /// Delay before the `app-launched` result; `None` never answers it.
    pub launch_reply_delay: Option<Duration>,
    pub started_reply: StartedReply,
    /// Send the EDID passthrough (id 101) after `app-launched`, repeating until answered.
    pub send_edid: bool,
    /// Start with a projection session already held (no announcements, no replies needed).
    pub holds_session: bool,
}

impl FakeDockConfig {
    /// A freshly plugged dock: announces ready, answers everything.
    pub fn fresh() -> Self {
        Self {
            announce_ready: true,
            ready_interval: Duration::from_millis(60),
            heartbeat_interval: Some(Duration::from_millis(50)),
            launch_reply_delay: Some(Duration::from_millis(30)),
            started_reply: StartedReply::Allowed,
            send_edid: true,
            holds_session: false,
        }
    }

    /// A dock that already holds a session from an earlier host process: it heartbeats, never
    /// announces and never answers requests.
    pub fn holding_session() -> Self {
        Self {
            announce_ready: true,
            launch_reply_delay: None,
            started_reply: StartedReply::Silent,
            send_edid: false,
            holds_session: true,
            ..Self::fresh()
        }
    }
}

/// What the fake saw on its simulated MI_01.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VideoStats {
    pub reports: u64,
    pub messages: u64,
    /// Reports that arrived while the sink was not started (a real dock drops these).
    pub reports_before_sink: u64,
    /// Reports that broke the header layout.
    pub layout_violations: u64,
    /// Width and height of the first video message.
    pub first_size: Option<(u32, u32)>,
}

#[derive(Debug)]
struct DockState {
    app_known: bool,
    sink_started: bool,
    edid_pending: bool,
    next_edid: Instant,
    next_ready: Instant,
    next_heartbeat: Instant,
    seq: u64,
    scheduled: Vec<(Instant, Vec<u8>, SinkEffect)>,
    host_log: Vec<(Instant, Vec<u8>)>,
    format_violations: u64,
    allowed_sent_at: Option<Instant>,
    video: VideoStats,
    launch_error: bool,
}

/// Side effect applied when a scheduled message is sent.
#[derive(Debug, Clone, Copy)]
enum SinkEffect {
    None,
    StartSink,
}

#[derive(Debug)]
struct Inner {
    config: FakeDockConfig,
    state: Mutex<DockState>,
    stop: AtomicBool,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, DockState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A running fake dock.
#[derive(Debug)]
pub struct FakeDock {
    inner: Arc<Inner>,
    thread: Option<JoinHandle<()>>,
}

fn pretty(method: &str, params: Option<&str>, seq: u64) -> Vec<u8> {
    let params = params.map_or(String::new(), |p| format!("\t\"params\":\t{p},\n"));
    format!("{{\n\t\"jsonrpc\":\t\"2.0\",\n\t\"method\":\t\"{method}\",\n{params}\t\"jrpcSeq\":\t{seq}\n}}")
        .into_bytes()
}

fn pretty_result(id: u64, result: &str, seq: u64) -> Vec<u8> {
    format!("{{\n\t\"id\":\t{id},\n\t\"result\":\t{result},\n\t\"jrpcSeq\":\t{seq}\n}}")
        .into_bytes()
}

impl FakeDock {
    /// Starts a fake dock and returns the host's MI_00 handle.
    pub fn spawn(config: FakeDockConfig) -> (Self, HidrawHandle) {
        let (dock, mi00, _) = Self::start(config, false);
        (dock, mi00)
    }

    /// Starts a fake dock that also drains and checks a simulated MI_01. Returns the host's
    /// MI_00 and MI_01 handles.
    pub fn spawn_with_video(config: FakeDockConfig) -> (Self, HidrawHandle, HidrawHandle) {
        let (dock, mi00, mi01) = Self::start(config, true);
        (dock, mi00, mi01.expect("video requested"))
    }

    fn start(
        config: FakeDockConfig,
        with_video: bool,
    ) -> (Self, HidrawHandle, Option<HidrawHandle>) {
        let (host0, dock0) = UnixDatagram::pair().expect("socketpair for MI_00");
        dock0
            .set_read_timeout(Some(Duration::from_millis(10)))
            .expect("read timeout");
        let (host1, dock1) = if with_video {
            let (h, d) = UnixDatagram::pair().expect("socketpair for MI_01");
            d.set_nonblocking(true).expect("nonblocking MI_01");
            (Some(h), Some(d))
        } else {
            (None, None)
        };
        let now = Instant::now();
        let inner = Arc::new(Inner {
            state: Mutex::new(DockState {
                app_known: config.holds_session,
                sink_started: config.holds_session,
                edid_pending: false,
                next_edid: now,
                next_ready: now + Duration::from_millis(20),
                next_heartbeat: now,
                seq: 7730,
                scheduled: Vec::new(),
                host_log: Vec::new(),
                format_violations: 0,
                allowed_sent_at: None,
                video: VideoStats::default(),
                launch_error: false,
            }),
            config,
            stop: AtomicBool::new(false),
        });
        let thread_inner = Arc::clone(&inner);
        let thread = std::thread::Builder::new()
            .name("fake-dock".into())
            .spawn(move || run(&thread_inner, &dock0, dock1.as_ref()))
            .expect("spawn fake dock");
        let handle = |sock: UnixDatagram, channel| {
            HidrawHandle::from_file(std::fs::File::from(OwnedFd::from(sock)), channel)
        };
        (
            Self {
                inner,
                thread: Some(thread),
            },
            handle(host0, ReportId::Control),
            host1.map(|s| handle(s, ReportId::Video)),
        )
    }

    /// Every host message so far, as text (heartbeats included), in arrival order.
    pub fn host_texts(&self) -> Vec<String> {
        self.inner
            .lock()
            .host_log
            .iter()
            .map(|(_, t)| String::from_utf8_lossy(t).into_owned())
            .collect()
    }

    /// Arrival times of the host's heartbeats.
    pub fn heartbeat_times(&self) -> Vec<Instant> {
        self.inner
            .lock()
            .host_log
            .iter()
            .filter(|(_, t)| String::from_utf8_lossy(t).contains("\"heartbeat\""))
            .map(|(at, _)| *at)
            .collect()
    }

    /// Host reports that broke the MI_00 report layout (size, Report ID, zero header bytes).
    pub fn format_violations(&self) -> u64 {
        self.inner.lock().format_violations
    }

    /// When the fake sent `isAllowed`, if it did.
    pub fn allowed_sent_at(&self) -> Option<Instant> {
        self.inner.lock().allowed_sent_at
    }

    /// Whether the simulated sink is started.
    pub fn sink_started(&self) -> bool {
        self.inner.lock().sink_started
    }

    /// Video statistics (needs `spawn_with_video`).
    pub fn video_stats(&self) -> VideoStats {
        self.inner.lock().video.clone()
    }

    /// Makes the dock answer `app-launched` with an error object instead of a result.
    pub fn set_launch_error(&self, on: bool) {
        self.inner.lock().launch_error = on;
    }

    /// The dock loses its session: the sink stops and ready announcements restart.
    pub fn drop_session(&self) {
        let mut state = self.inner.lock();
        state.app_known = false;
        state.sink_started = false;
        state.next_ready = Instant::now();
    }

    /// Stops the fake and joins its thread.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.inner.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for FakeDock {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn send(sock: &UnixDatagram, state: &mut DockState, payload: &[u8]) {
    let chunks = frame_message(ReportId::Control, PictureSize::zero(), payload);
    for chunk in chunks {
        // The host may have gone away; that ends the test, not the fake.
        let _ = sock.send(&chunk);
    }
    state.seq += 1;
}

fn run(inner: &Inner, sock: &UnixDatagram, video: Option<&UnixDatagram>) {
    let mut assembler = InboundAssembler::new();
    let mut buf = vec![0u8; REPORT_SIZE * 2];
    while !inner.stop.load(Ordering::Relaxed) {
        let received = sock.recv(&mut buf).ok();
        let mut state = inner.lock();
        if let Some(n) = received {
            handle_host_report(inner, sock, &mut state, &mut assembler, &buf[..n]);
        }
        tick(inner, sock, &mut state);
        if let Some(video) = video {
            drain_video(video, &mut state, &mut buf);
        }
    }
}

fn handle_host_report(
    inner: &Inner,
    sock: &UnixDatagram,
    state: &mut DockState,
    assembler: &mut InboundAssembler,
    report: &[u8],
) {
    let layout_ok = report.len() == REPORT_SIZE
        && report[0] == 1
        && report[5..16].iter().all(|&b| b == 0)
        && report[16..].len() == REPORT_SIZE - 16;
    if !layout_ok {
        state.format_violations += 1;
    }
    let Some(text) = assembler.feed(report) else {
        return;
    };
    state.host_log.push((Instant::now(), text.clone()));
    let Ok(Value::Object(msg)) = serde_json::from_slice::<Value>(&text) else {
        return;
    };
    let id = msg.get("id").and_then(Value::as_u64);
    let params = msg.get("params");
    match msg.get("method").and_then(Value::as_str) {
        Some("app-launched") => {
            let platform = params
                .and_then(|p| p.get("platform"))
                .and_then(Value::as_str);
            if platform.is_some() {
                state.app_known = true;
            }
            if let (Some(id), Some(delay)) = (id, inner.config.launch_reply_delay) {
                let seq = state.seq + 1000;
                let reply = if state.launch_error {
                    format!(
                        "{{\n\t\"id\":\t{id},\n\t\"error\":\t{{\"code\":-32000}},\n\t\"jrpcSeq\":\t{seq}\n}}"
                    )
                    .into_bytes()
                } else {
                    pretty_result(id, r#"{"res-width":1920,"res-height":1080}"#, seq)
                };
                state
                    .scheduled
                    .push((Instant::now() + delay, reply, SinkEffect::None));
            }
            if inner.config.send_edid {
                state.edid_pending = true;
                state.next_edid = Instant::now() + Duration::from_millis(10);
            }
        }
        Some("notify-projection-started") => {
            let Some(id) = id else { return };
            if !state.app_known {
                return;
            }
            let (text, effect) = match inner.config.started_reply {
                StartedReply::Silent => return,
                StartedReply::Allowed => ("\"isAllowed\"", SinkEffect::StartSink),
                StartedReply::Denied => ("\"isDenied\"", SinkEffect::None),
            };
            let seq = state.seq + 1000;
            state.scheduled.push((
                Instant::now() + Duration::from_millis(40),
                pretty_result(id, text, seq),
                effect,
            ));
        }
        Some("notify-projection-stopped") => state.sink_started = false,
        Some("common.data_passthrough")
            if params.and_then(|p| p.get("id")).and_then(Value::as_u64) == Some(103) =>
        {
            state.edid_pending = false;
        }
        _ => {}
    }
    let _ = sock;
}

fn tick(inner: &Inner, sock: &UnixDatagram, state: &mut DockState) {
    let now = Instant::now();
    if let Some(interval) = inner.config.heartbeat_interval {
        if now >= state.next_heartbeat {
            state.next_heartbeat = now + interval;
            let seq = state.seq;
            let text = pretty(
                "heartbeat",
                Some("{\n\t\t\"session-id\":\t\"25282281\"\n\t}"),
                seq,
            );
            send(sock, state, &text);
        }
    }
    if inner.config.announce_ready && !state.app_known && now >= state.next_ready {
        state.next_ready = now + inner.config.ready_interval;
        let seq = state.seq;
        let ready = pretty(
            "notify-app-accessories-are-ready",
            Some("{\n\t\t\"res-width\":\t1920,\n\t\t\"res-height\":\t1080\n\t}"),
            seq,
        );
        send(sock, state, &ready);
        let seq = state.seq;
        let tx = pretty(
            "tx-data-updated",
            Some("{\n\t\t\"role\":\t\"host\"\n\t}"),
            seq,
        );
        send(sock, state, &tx);
    }
    if state.edid_pending && now >= state.next_edid {
        state.next_edid = now + Duration::from_millis(150);
        let seq = state.seq;
        let edid = pretty(
            "common.data_passthrough",
            Some("{\n\t\t\"id\":\t101,\n\t\t\"size\":\t258,\n\t\t\"data\":\t\"AAAA\"\n\t}"),
            seq,
        );
        send(sock, state, &edid);
    }
    let mut due = Vec::new();
    state.scheduled.retain(|item| {
        if item.0 <= now {
            due.push((item.1.clone(), item.2));
            false
        } else {
            true
        }
    });
    for (payload, effect) in due {
        if matches!(effect, SinkEffect::StartSink) {
            state.sink_started = true;
            state.allowed_sent_at = Some(Instant::now());
        }
        send(sock, state, &payload);
    }
}

fn drain_video(video: &UnixDatagram, state: &mut DockState, buf: &mut [u8]) {
    while let Ok(n) = video.recv(buf) {
        let report = &buf[..n];
        state.video.reports += 1;
        if n < 16 {
            state.video.layout_violations += 1;
            continue;
        }
        if !state.sink_started {
            state.video.reports_before_sink += 1;
        }
        let well_formed = n == REPORT_SIZE && report[0] == 2;
        let announced = u32::from_be_bytes(report[1..5].try_into().unwrap_or([0; 4]));
        if !well_formed {
            state.video.layout_violations += 1;
        } else if announced != 0 {
            state.video.messages += 1;
            let width = u32::from_be_bytes(report[8..12].try_into().unwrap_or([0; 4]));
            let height = u32::from_be_bytes(report[12..16].try_into().unwrap_or([0; 4]));
            if report[5..8] != [0, 0, 0] {
                state.video.layout_violations += 1;
            }
            state.video.first_size.get_or_insert((width, height));
        } else if report[1..16].iter().any(|&b| b != 0) {
            state.video.layout_violations += 1;
        }
    }
}
