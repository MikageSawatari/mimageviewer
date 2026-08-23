#!/usr/bin/env bash
# Encode the video scaling test clips from the charts.
#
# Both clips are deliberately frozen: every frame is identical, so anything
# that shimmers on screen is the display scaler and not the material.  Playback
# is still visible on the seek bar.  They are encoded losslessly (CRF 0) so the
# thing being compared is the scaler rather than the encoder -- the fine
# gratings in the 4K chart are close to the worst case H.264 has, and at any
# normal CRF the encoder would soften exactly the detail the chart exists to
# test.
#
# Usage: bash scripts/gen-scaling-test-videos.sh [out-dir]

set -euo pipefail

OUT_DIR="${1:-.}"
DURATION=20
FPS=30
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$OUT_DIR"
python "$(dirname "$0")/gen_scaling_test_charts.py" --out-dir "$WORK"

encode() {
    local src="$1" dst="$2"
    ffmpeg -hide_banner -loglevel error -y \
        -loop 1 -framerate "$FPS" -i "$src" \
        -t "$DURATION" \
        -c:v libx264 -preset veryslow -crf 0 -pix_fmt yuv420p \
        -g "$FPS" -movflags +faststart \
        "$dst"
    printf '%s  %s\n' "$(du -h "$dst" | cut -f1)" "$dst"
}

encode "$WORK/chart_downscale_4k.png"  "$OUT_DIR/scaling_downscale_2160p.mp4"
encode "$WORK/chart_upscale_540p.png"  "$OUT_DIR/scaling_upscale_540p.mp4"
encode "$WORK/chart_edges_270p.png"    "$OUT_DIR/scaling_edges_270p.mp4"
