#!/bin/bash
# Compares the picture the daemon sends with the screen it captured, for several picture sizes.
# For each size it starts a private Xvfb with an xterm, runs `screendockd run --backend x11`
# against regular files standing in for the dock nodes, saves one screenshot of the display,
# decodes a keyframe from the written video file, and prints PSNR and SSIM between the decoded
# frame and the screenshot scaled to the same size.
#
# usage: deploy/compare_sizes.sh [size ...]      (default sizes: 1280x720 900x720 640x360)
# env:   SCREENDOCKD     path of the built binary (default: src/screendockd/target/release/screendockd)
#        COMPARE_DISPLAY display number of the private Xvfb (default 150)
#        COMPARE_SECONDS seconds the daemon runs before it is stopped (default 6)
# needs: Xvfb, xterm, ffmpeg and ffprobe (with x11grab), python3; the binary built with --features encode-ffmpeg
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
bin=${SCREENDOCKD:-$root/src/screendockd/target/release/screendockd}
num=${COMPARE_DISPLAY:-150}
secs=${COMPARE_SECONDS:-6}
display=":$num"
sizes=("$@")
[ ${#sizes[@]} -gt 0 ] || sizes=(1280x720 900x720 640x360)

[ -x "$bin" ] || { echo "binary not found or not executable: $bin" >&2; exit 1; }
for tool in Xvfb xterm ffmpeg ffprobe python3; do
    command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 1; }
done
if [ -e "/tmp/.X$num-lock" ] || [ -e "/tmp/.X11-unix/X$num" ]; then
    echo "display $display is already in use; set COMPARE_DISPLAY" >&2
    exit 1
fi

work=$(mktemp -d)
pids=()
cleanup() {
    for pid in "${pids[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    for pid in "${pids[@]}"; do
        wait "$pid" 2>/dev/null || true
    done
    rm -rf "$work"
}
trap cleanup EXIT

Xvfb "$display" -screen 0 1280x720x24 -nolisten tcp >/dev/null 2>&1 &
pids+=($!)
for _ in $(seq 1 50); do
    [ -S "/tmp/.X11-unix/X$num" ] && break
    sleep 0.1
done
[ -S "/tmp/.X11-unix/X$num" ] || { echo "Xvfb did not start on $display" >&2; exit 1; }

# A fixed text pattern, so the screenshot and the decoded keyframe show the same picture.
pattern='for i in $(seq 1 80); do echo "$i The quick brown fox jumps over the lazy dog 0123456789 ABCDEFGHIJKLMNOPQRSTUVWXYZ"; done; sleep 3600'
DISPLAY=$display xterm -geometry 200x60+0+0 -fa Monospace -fs 12 -bg black -fg white \
    -e sh -c "$pattern" >/dev/null 2>&1 &
pids+=($!)
sleep 1

# Splits the written MI_01 file into video messages (16 byte header on every 4096 byte report,
# payload length big-endian at bytes 1 to 4 of the first report, 4080 payload bytes per
# report) and writes the first keyframe message from the given index on as Annex B.
extract_keyframe() { # extract_keyframe <video file> <output .h264> <first index>
    python3 - "$1" "$2" "$3" <<'PY'
import sys

REPORT, HEADER = 4096, 16
PAYLOAD = REPORT - HEADER
path, out, first = sys.argv[1], sys.argv[2], int(sys.argv[3])
data = open(path, "rb").read()
data = data[: len(data) // REPORT * REPORT]
pos, index, chosen, count = 0, 0, None, 0
while pos < len(data):
    length = int.from_bytes(data[pos + 1 : pos + 5], "big")
    reports = 1 + max(0, -(-(length - PAYLOAD) // PAYLOAD))
    if pos + reports * REPORT > len(data):
        break
    payload = b"".join(
        data[pos + i * REPORT + HEADER : pos + (i + 1) * REPORT] for i in range(reports)
    )[:length]
    has_idr = any(
        payload[i : i + 3] == b"\x00\x00\x01" and payload[i + 3] & 0x1F == 5
        for i in range(len(payload) - 3)
    )
    if has_idr:
        chosen = payload
        count += 1
        if index >= first:
            break
    pos += reports * REPORT
    index += 1
if chosen is None:
    sys.exit("no keyframe in " + path)
open(out, "wb").write(chosen)
print(f"messages={index + 1} keyframe_index={index}", file=sys.stderr)
PY
}

metric() { # metric <filter name> <decoded png> <source png> <size> -> average value
    local out
    out=$(ffmpeg -hide_banner -nostdin -i "$2" -i "$3" -lavfi \
        "[0:v]format=rgb24[a];[1:v]scale=${4/x/:}:flags=bicubic,format=rgb24[b];[a][b]$1" \
        -f null - 2>&1)
    if [ "$1" = psnr ]; then
        grep -o 'average:[0-9a-z.]*' <<<"$out" | tail -n1 | cut -d: -f2
    else
        grep -o ' All:[0-9.]*' <<<"$out" | tail -n1 | cut -d: -f2
    fi
}

rows=()
for size in "${sizes[@]}"; do
    w=${size%x*}
    h=${size#*x}
    dir="$work/$size"
    mkdir -p "$dir/run"
    : >"$dir/mi00"
    : >"$dir/mi01"
    sed -e "s/^width = .*/width = $w/" -e "s/^height = .*/height = $h/" \
        "$root/config/screendock_cfg.toml" >"$dir/cfg.toml"

    DISPLAY=$display XDG_RUNTIME_DIR="$dir/run" "$bin" --config "$dir/cfg.toml" run \
        --backend x11 --mi00 "$dir/mi00" --mi01 "$dir/mi01" --no-handshake \
        >"$dir/daemon.log" 2>&1 &
    daemon=$!
    pids+=("$daemon")
    sleep "$secs"
    ffmpeg -hide_banner -nostdin -loglevel error -f x11grab -video_size 1280x720 \
        -i "$display" -frames:v 1 -y "$dir/source.png"
    kill -TERM "$daemon" 2>/dev/null || true
    wait "$daemon" 2>/dev/null || true

    extract_keyframe "$dir/mi01" "$dir/key.h264" 40
    ffmpeg -hide_banner -nostdin -loglevel error -i "$dir/key.h264" -frames:v 1 -y "$dir/decoded.png"
    got=$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=s=x:p=0 "$dir/decoded.png")
    psnr=$(metric psnr "$dir/decoded.png" "$dir/source.png" "$size")
    ssim=$(metric ssim "$dir/decoded.png" "$dir/source.png" "$size")
    rows+=("$size|$got|$psnr|$ssim")
done

printf '%-10s %-10s %-10s %s\n' "size" "decoded" "PSNR dB" "SSIM"
for row in "${rows[@]}"; do
    IFS='|' read -r size got psnr ssim <<<"$row"
    printf '%-10s %-10s %-10s %s\n' "$size" "$got" "$psnr" "$ssim"
done
