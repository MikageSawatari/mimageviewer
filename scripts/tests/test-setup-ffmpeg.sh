#!/usr/bin/env bash
# Offline comparison tests. Run: bash scripts/tests/test-setup-ffmpeg.sh
set -euo pipefail

# Sourcing must not query GitHub, change directory, or touch vendor files.
gh() { echo 'Unexpected gh invocation' >&2; exit 1; }
original_dir=$PWD
source "$(dirname "${BASH_SOURCE[0]}")/../setup-ffmpeg.sh"
[[ "$PWD" == "$original_dir" ]]

passed=0
assert_comparison() {
    local expected=$1 current=$2 matched=$3 actual
    if actual=$(compare_ffmpeg_assets "$current" "$matched"); then
        if [[ "$expected" == incomparable || "$actual" != "$expected" ]]; then
            echo "FAIL: expected $expected, got $actual: $current vs $matched" >&2
            exit 1
        fi
    elif [[ "$expected" != incomparable || -n "$actual" ]]; then
        echo "FAIL: unexpected comparison failure: $current vs $matched" >&2
        exit 1
    fi
    passed=$((passed + 1))
}

current='ffmpeg-n7.1.5-16-g9a4bb2c579-win64-lgpl-shared-7.1.zip'
matched='ffmpeg-n7.1.5-12-g1fdbca85aa-win64-lgpl-shared-7.1.zip'
assert_comparison newer "$current" "$matched"
assert_comparison older "$matched" "$current"
assert_comparison identical "$current" "$current"
assert_comparison older \
    'ffmpeg-n7.1-99-gaaa-win64-lgpl-shared-7.1.zip' \
    'ffmpeg-n7.1.1-0-gbbb-win64-lgpl-shared-7.1.zip'
assert_comparison incomparable \
    'ffmpeg-n7.1.5-16-gaaa-win64-lgpl-shared-7.1.zip' \
    'ffmpeg-n7.1.5-16-gbbb-win64-lgpl-shared-7.1.zip'
assert_comparison incomparable 'unparseable.zip' "$matched"
assert_comparison incomparable "$current" 'unparseable.zip'
assert_comparison incomparable '' "$matched"
# Numeric ordering, omitted patch normalization, and strict asset validation.
assert_comparison newer \
    'ffmpeg-n7.10-0-gaaa-win64-lgpl-shared-7.10.zip' "$current"
assert_comparison newer \
    'ffmpeg-n8.0-0-gaaa-win64-lgpl-shared-8.0.zip' "$current"
assert_comparison older \
    'ffmpeg-n7.1.5-9-gaaa-win64-lgpl-shared-7.1.zip' "$matched"
assert_comparison newer \
    'ffmpeg-n07.01.005-016-gaaa-win64-lgpl-shared-7.1.zip' "$matched"
assert_comparison incomparable \
    'ffmpeg-n7.1-16-gaaa-win64-lgpl-shared-7.1.zip' \
    'ffmpeg-n7.1.0-16-gaaa-win64-lgpl-shared-7.1.zip'
assert_comparison incomparable "$current" \
    'ffmpeg-n7.1.5-12-gxyz-win64-lgpl-shared-7.1.zip'
assert_comparison incomparable "$current" "${matched}.extra"
assert_comparison newer \
    'ffmpeg-n7.1.5-999999999999999999999999-gaaa-win64-lgpl-shared-7.1.zip' "$matched"

echo "FFmpeg asset comparison: $passed tests passed"
