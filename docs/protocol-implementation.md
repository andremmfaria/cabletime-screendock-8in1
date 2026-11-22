# Protocol implementation

This page describes how the wire protocol from `research/full-protocol-spec-2026-11-21.md` is implemented. The research document is the source for the wire facts.

## Chunk layout

Every chunk written to the device is exactly 4096 bytes (`REPORT_SIZE`), zero padded at the tail.

First chunk of a message:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | Report ID (1 for MI_00 control, 2 for MI_01 video) |
| 1 to 4 | 4 | Total payload length, big-endian u32 |
| 5 to 8 | 4 | Field A, big-endian u32, always 0 |
| 9 to 12 | 4 | Field B, big-endian u32: 0 for control, target width in pixels times 256 for video |
| 13 to 15 | 3 | Zero padding |
| 16 onward | up to 4080 | Payload |

Continuation chunk: 1 byte Report ID followed by up to 4095 payload bytes.

A 3577 byte keyframe bundle fits one chunk. A 67,666 byte inter-frame takes 17 chunks.

## Building messages

`MessageBuilder::video(PixelWidth)` and `MessageBuilder::control()` fill in the Report ID, field A (always 0) and field B. `.payload(&[u8]).build()` returns the chunk list. `frame_message` is the underlying pure function and `parse_chunks` is its inverse. Header parsing rejects non-zero padding and unknown Report IDs.

## Writing: `HidrawHandle::write_chunks`

- Opens the node read and write. `open_mi00` yields the control channel and `open_mi01` the video channel.
- Each chunk is validated before any byte of the message is written: it must be 4096 bytes and its first byte must equal the handle's channel Report ID. A bad chunk rejects the whole message.
- Each chunk is one `write(2)` through `File::write`. There is no ioctl, no hidapi, no buffering and no delay between chunks.
- A short write is an error (`ShortWrite`). Write errors carry the chunk index.
- Returns `WriteTimings` with the duration of every individual write and the total for the message.

## Reading: `HidrawHandle::read_message`

Reads the first report, parses its header, reads exactly the continuation reports the header length implies, and returns a `ParsedMessage`. Reports shorter than their meaningful bytes are zero-extended before parsing. End of stream mid-message returns `UnexpectedEof` with the chunk index.

## Heartbeat

The MI_00 message is JSON-RPC:

```json
{"jsonrpc":"2.0","method":"heartbeat","params":{"session-id":"<tick>"}}
```

`tick` is a decimal string counting heartbeats sent in this run, starting at 1. The message is framed with `MessageBuilder::control()`, so it is a single Report ID 1 chunk with field B 0.

`run_heartbeat_loop(handle, interval, stop, max_ticks)` sends the first tick immediately and then one per `interval`, scheduled against fixed deadlines so write time does not accumulate as drift. It stops when the stop flag is set, when `max_ticks` is reached, or on the first write error, which it returns with the tick number. It refuses a handle that is not the control channel. The default interval is 1000 ms and comes from `daemon.heartbeat_interval_ms`.

## Annex-B splitting

`annexb::messages_from_annexb` turns an Annex-B byte stream into dock messages: SPS, PPS and SEI NAL units accumulate and are bundled with the IDR slice that follows into one message; a non-IDR slice is its own message; access unit delimiters and other NAL types are dropped. Start codes are kept on every NAL. This is used by the validation tools to replay a real stream.

## Not implemented

- The `reset` JSON-RPC method (direction unresolved in the research).
- Any input or touch path.
- Capture and encode (stubs).
