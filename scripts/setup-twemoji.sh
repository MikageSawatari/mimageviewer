#!/usr/bin/env bash
# Download the curated emoji SVGs used by the comic_lab stamp picker.
#
# Source of truth for the key list is tools/comic_lab/src/stamp.rs (EMOJI_CATALOG).
# This script greps the `key: "..."` entries out of it and fetches each SVG into
# vendor/twemoji/svg/<key>.svg, so the catalog and the assets never drift.
#
# Assets: Twemoji (maintained jdecked fork). Graphics are CC-BY 4.0 — attribution
# is required when distributing (see docs/archive/comic/stamp-feature-design.md). The stamp
# picker degrades to user-image-only if these assets are absent, so this is
# optional for development.
#
# Usage:
#   bash scripts/setup-twemoji.sh          # fetch missing SVGs
#   bash scripts/setup-twemoji.sh --force  # re-fetch all (overwrite)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CATALOG="$REPO_ROOT/tools/comic_lab/src/stamp.rs"
OUT_DIR="$REPO_ROOT/vendor/twemoji/svg"
BASE_URL="https://raw.githubusercontent.com/jdecked/twemoji/main/assets/svg"

FORCE=0
if [ "${1:-}" = "--force" ]; then
    FORCE=1
fi

if [ ! -f "$CATALOG" ]; then
    echo "error: catalog not found: $CATALOG" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"

# Extract unique, non-empty emoji keys from EMOJI_CATALOG.
keys=$(grep -oE 'key: "[0-9a-f-]+"' "$CATALOG" \
    | sed -E 's/key: "([0-9a-f-]+)"/\1/' \
    | sort -u)

total=0
fetched=0
skipped=0
failed=0
for key in $keys; do
    total=$((total + 1))
    dest="$OUT_DIR/$key.svg"
    if [ "$FORCE" -eq 0 ] && [ -s "$dest" ]; then
        skipped=$((skipped + 1))
        continue
    fi
    if curl -fsSL "$BASE_URL/$key.svg" -o "$dest.tmp" 2>/dev/null && [ -s "$dest.tmp" ]; then
        mv -f "$dest.tmp" "$dest"
        fetched=$((fetched + 1))
    else
        rm -f "$dest.tmp"
        failed=$((failed + 1))
        echo "  ! failed: $key" >&2
    fi
done

echo "twemoji: $total keys -> fetched $fetched, skipped $skipped, failed $failed"
echo "out: $OUT_DIR"
if [ "$failed" -gt 0 ]; then
    echo "note: some keys may not exist in this Twemoji version (catalog drift)." >&2
fi
echo "Attribution required on distribution: Twemoji graphics (c) Twitter/jdecked, CC-BY 4.0."
