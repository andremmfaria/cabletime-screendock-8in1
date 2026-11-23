# Protocol implementation

This page describes how the wire protocol from the protocol specification under `research/` is implemented. The research document is the source for the wire facts.

## Report layout

Every report written to the device is exactly 4096 bytes (`REPORT_SIZE`), zero padded at the tail. Every report starts with a 16-byte header, the first report of a message and every continuation report alike.

First report of a message:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | Report ID (1 for MI_00 control, 2 for MI_01 video) |
| 1 to 4 | 4 | Total payload length of the whole message, big-endian u32 |
| 5 to 7 | 3 | Zero |
| 8 to 11 | 4 | Picture width in pixels, big-endian u32 (video; zero for control messages) |
| 12 to 15 | 4 | Picture height in pixels, big-endian u32 (video; zero for control messages) |
| 16 onward | up to 4080 | Payload (Annex B H.264 or JSON text) |

Continuation report: byte 0 is the Report ID, bytes 1 to 15 are zero, and up to 4080 payload bytes start at offset 16.

A 3577 byte keyframe bundle fits one report. A 67,666 byte inter-frame takes 17 reports: the first carries 4080 payload bytes, fifteen continuations carry 4080 each and the last carries 2386.

## Building messages

`MessageBuilder::video(PictureSize)` and `MessageBuilder::control()` fill in the Report ID, width and height (zero for control). `.payload(&[u8]).build()` returns the report list. `frame_message` is the underlying pure function and `parse_chunks` is its inverse. `PixelWidth`, `PixelHeight` and `PictureSize` are newtypes so a width cannot be passed as a height. Parsing rejects non-zero bytes 5 to 7 in a first header, a continuation whose bytes 1 to 15 are not zero, and unknown Report IDs.

## Writing: `HidrawHandle::write_chunks`

- Opens the node read and write. `open_mi00` yields the control channel and `open_mi01` the video channel.
- Each chunk is validated before any byte of the message is written: it must be 4096 bytes and its first byte must equal the handle's channel Report ID. A bad chunk rejects the whole message.
- Each chunk is one `write(2)` through `File::write`. There is no ioctl, no hidapi, no buffering and no delay between chunks.
- A short write is an error (`ShortWrite`). Write errors carry the chunk index.
- Returns `WriteTimings` with the duration of every individual write and the total for the message.

## Reading: `HidrawHandle::read_message`

Reads the first report, parses its header, reads exactly the continuation reports the header length implies, and returns a `ParsedMessage`. Reports shorter than their meaningful bytes are zero-extended before parsing. End of stream mid-message returns `UnexpectedEof` with the chunk index.

## Heartbeat

The MI_00 heartbeat is this exact text, spacing included, with no trailing newline:

```text
{"jsonrpc":"2.0", "method": "heartbeat", "params": {"session-id": "<digits>"}}
```

`SessionId` is a non-empty string of at most 20 ASCII digits, generated once per run from the clock and kept constant, because a changed id makes the dock re-announce itself. For an eight digit id the length field is 78. The message is framed with `MessageBuilder::control()`, so it is a single Report ID 1 report with zero width and height.

In the daemon the heartbeat runs on its own thread inside the session: the first heartbeat is written synchronously at startup (a failure there is fatal), then one is sent every `daemon.heartbeat_interval_ms` (default 1000) against fixed deadlines. Video is written on MI_01 by a different thread, so video writes never delay it. A failed heartbeat write is logged and retried at the next tick; three consecutive failures count as a lost session.

The dock treats any text arriving on MI_00 as host activity and tears the session down if more than about 5 seconds pass without any. The 1 second heartbeat keeps well inside that watchdog.

`run_heartbeat_loop(handle, session_id, interval, stop, max_ticks)` is the simple blocking variant used by the validation tools when no session runs. It sends the first heartbeat immediately, then one per `interval`, and stops on the stop flag, at `max_ticks` or on the first write error. It refuses a handle that is not the control channel.

## Host messages on MI_00

Every MI_00 report is padded to 4096 bytes and holds one whole message, because the dock assembles exactly 4096 bytes per message and glues a short report onto the next one. Messages other than the heartbeat are compact JSON with alphabetical keys, ending in one newline that is counted in the length field. They are built by `session::messages`:

| Message | Exact text |
|---|---|
| `notify-projection-stopped` | `{"jsonrpc":"2.0","method":"notify-projection-stopped"}` |
| `app-launched` (request) | `{"id":N,"jsonrpc":"2.0","method":"app-launched","params":{"platform":"windows","platform-machine-name":"<HOST>","product-type":"QuattroTx CT10","res-height":H,"res-width":W}}` |
| `app_tx_mouse_icon_change` | `{"jsonrpc":"2.0","method":"app_tx_mouse_icon_change","params":{"cursor_base64_data":""}}` |
| `common.data_passthrough` (EDID reply) | `{"jsonrpc":"2.0","method":"common.data_passthrough","params":{"data":"AQ==","id":103,"size":1}}` |
| `notify-projection-started` (request) | `{"id":N,"jsonrpc":"2.0","method":"notify-projection-started"}` |

`<HOST>` is the uppercase hostname truncated to 15 characters, and W and H are the configured picture size. Request ids come from one counter that starts at 1 and keeps counting across handshakes. The dock's own messages carry a `jrpcSeq` counter; the host never sends it.

## Reading MI_00

The dock writes JSON to MI_00 in 4096 byte reports: pretty printed with tabs, with a trailing `jrpcSeq`. If the host stops reading, the dock's writes block and it eventually re-enumerates the USB device, so a reader thread drains MI_00 for the whole life of the session. It waits for readability with `poll(2)` in 50 ms slices so it can stop, reads one report per `read(2)`, and reassembles messages with `InboundAssembler`: the first report's bytes 1 to 4 give the payload length (messages above 64 KiB are dropped), payload starts at offset 16, and continuation reports repeat the 16-byte header with a zero length. Messages are parsed as JSON into `DockMessage` values (ready, heartbeat, passthrough, reply, other). Dock text is untrusted data: it is only parsed, never executed, and only truncated method names are logged. End of stream or a read error ends the session for good.

The reader answers the dock's `common.data_passthrough` with id 101 (EDID) immediately with the id 103 message above, every time it arrives; the dock repeats it about every 5 seconds until answered. Heartbeats are counted and timestamped. Ready announcements and request replies go to the controller.

## Session handshake

The dock discards every video frame, with no error, until the host has sent `app-launched` (a request with an id and a `platform`), then `notify-projection-started` (a request with an id), and the dock has started its sink. Video sent earlier is lost. `Session` runs the handshake on its controller thread:

| State | Meaning |
|---|---|
| `Starting` | Threads started, handshake not begun |
| `WaitingForReady` | Waiting up to 3 s for `notify-app-accessories-are-ready` |
| `AppLaunchedSent` | `notify-projection-stopped` (optional) and `app-launched` sent, waiting up to 5 s for the result with the same id |
| `ProjectionStartedSent` | `app_tx_mouse_icon_change` and `notify-projection-started` sent with the next id, waiting up to 5 s for the result |
| `Settling` | The result is `isAllowed`; waiting 0.8 s |
| `Active` | Every reply was seen; video is allowed |
| `AssumedActive` | The dock was alive but silent; video is allowed |
| `Lost` | The session was lost or the handshake failed; video is held back |
| `Stopped` | Shut down, or the reader died |

Sequence on a freshly plugged dock: the dock announces ready about every 1 to 2 seconds until `app-launched` arrives; the host sends `notify-projection-stopped`, then `app-launched` with id 1; the dock sends its EDID passthrough (answered by the reader) and the `app-launched` result `{"res-width":W,"res-height":H}`; the host sends the cursor message and `notify-projection-started` with id 2; the dock answers `"isAllowed"`; after the 0.8 s settle delay video is allowed. All timeouts are `SessionSettings` fields.

Bounded waits: a dock that already holds a projection session from an earlier host process sends no ready announcement and answers neither request, yet accepts video after heartbeats alone. When the ready wait or a reply wait times out and the dock's heartbeat was seen within the last 3 seconds, the state machine logs a warning and enters `AssumedActive`. When the ready wait times out and the dock has sent no heartbeat, nothing is armed, `DockSilent` is logged and the handshake is retried after 1 s. A reply that is an error object, or a result other than `isAllowed`, fails the attempt, which is retried after 1 s. Video is never blocked forever on a reply.

Loss detection: while armed, the controller watches for a new ready announcement from the dock, three consecutive heartbeat write failures, and failures reported through `SessionHandle::report_failure` (the daemon reports a failed video write). Any of them moves the state to `Lost` and runs the handshake again without restarting the process. `SessionHandle::generation` increases each time the session becomes armed, and the video loop requests a keyframe when it changes.

Video gating: the daemon's video loop captures and writes nothing while the state is not armed, and re-checks the state before every message. The validation tools do the same: `isolate` with an MI_00 handle (`heartbeat` and `both` modes) starts a session and `both` starts video only once it is armed; `replay` opens MI_00, starts a session and waits up to 30 s for it to arm. `--no-handshake` keeps only the heartbeat thread and arms immediately, for regular files standing in for the nodes.

## Annex-B splitting

`annexb::messages_from_annexb` turns an Annex-B byte stream into dock messages: SPS, PPS and SEI NAL units accumulate and are bundled with the IDR slice that follows into one message; a non-IDR slice is its own message; access unit delimiters and other NAL types are dropped. Start codes are kept on every NAL. This is used by the validation tools to replay a real stream.

## Not implemented

- The `reset` JSON-RPC method, which makes the dock re-enumerate USB and is never sent.
- The optional `app_tx_mouse_coordinate` message.
- Any input or touch path.
