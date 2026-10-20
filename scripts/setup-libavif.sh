#!/usr/bin/env bash
# Build the pinned libavif + dav1d + libyuv static libraries into vendor/libavif/.
#
# The versions are the set libavif 1.4.2 itself pins in cmake/Modules/Local*.cmake.
# Build tools (meson, ninja, NASM) are fetched into vendor/libavif/tools/ so the
# build machine does not need them installed globally. Agent-only (bash);
# do not hand this to the user.
#
#   bash scripts/setup-libavif.sh          # build (skips when VERSION matches)
#   bash scripts/setup-libavif.sh --force  # rebuild
#   bash scripts/setup-libavif.sh check    # compare with the latest upstream releases
set -euo pipefail
cd "$(dirname "$0")/.."

LIBAVIF_VERSION=1.4.2
LIBAVIF_SHA256=2b645287340ba5a631d268b551dc2d72bd73ac33335962dd36dcdb6d8366921d
DAV1D_VERSION=1.5.3
DAV1D_SHA256=732010aa5ef461fa93355ed2c6c5fedb48ddc4b74e697eaabe8907eaeb943011
LIBYUV_COMMIT=644251f252a84bf8ce91ff0aca86a9b16b069ab8
NASM_VERSION=3.02
NASM_SHA256=161d0bfaff53c2f9e9f3e69fd0672323ebabafd1268976a5cec11be92a19aee7
MESON_VERSION=1.12.1
NINJA_VERSION=1.13.2

VENDOR=vendor/libavif
STAMP="libavif=$LIBAVIF_VERSION dav1d=$DAV1D_VERSION libyuv=$LIBYUV_COMMIT"

if [[ "${1:-}" == check ]]; then
    latest_avif=$(gh release list --repo AOMediaCodec/libavif --limit 1 --json tagName --jq '.[0].tagName')
    echo "Current: $(cat "$VENDOR/VERSION" 2>/dev/null || echo 'not installed')"
    echo "Latest libavif release: $latest_avif (check its cmake/Modules/LocalDav1d.cmake and LocalLibyuv.cmake pins)"
    exit 0
fi
if [[ "${1:-}" != --force && -f "$VENDOR/VERSION" && "$(cat "$VENDOR/VERSION")" == "$STAMP" ]]; then
    echo "vendor/libavif is up to date ($STAMP)"
    exit 0
fi

WORK=$VENDOR/_build
TOOLS=$VENDOR/tools
mkdir -p "$WORK" "$TOOLS"

fetch() { # url sha256 dest
    local url=$1 expected=$2 dest=$3
    if [[ ! -f "$dest" ]]; then
        echo "Downloading $url"
        curl -fsSL --retry 3 -o "$dest.part" "$url"
        mv "$dest.part" "$dest"
    fi
    local actual
    actual=$(sha256sum "$dest" | cut -d' ' -f1)
    if [[ "$actual" != "$expected" ]]; then
        echo "SHA-256 mismatch for $dest: $actual != $expected" >&2
        exit 1
    fi
}

# --- tools -------------------------------------------------------------------
fetch "https://www.nasm.us/pub/nasm/releasebuilds/$NASM_VERSION/win64/nasm-$NASM_VERSION-win64.zip" \
    "$NASM_SHA256" "$TOOLS/nasm-$NASM_VERSION-win64.zip"
if [[ ! -x "$TOOLS/nasm-$NASM_VERSION/nasm.exe" ]]; then
    (cd "$TOOLS" && unzip -q -o "nasm-$NASM_VERSION-win64.zip")
fi
if [[ ! -x "$TOOLS/venv/Scripts/meson.exe" ]]; then
    python -m venv "$TOOLS/venv"
    "$TOOLS/venv/Scripts/python" -m pip install -q "meson==$MESON_VERSION" "ninja==$NINJA_VERSION"
fi
TOOLS_ABS=$(cd "$TOOLS" && pwd -W)
export PATH="$(cd "$TOOLS" && pwd)/nasm-$NASM_VERSION:$(cd "$TOOLS" && pwd)/venv/Scripts:$PATH"
nasm -v
meson --version
ninja --version

# --- sources -----------------------------------------------------------------
fetch "https://github.com/AOMediaCodec/libavif/archive/refs/tags/v$LIBAVIF_VERSION.tar.gz" \
    "$LIBAVIF_SHA256" "$WORK/libavif-$LIBAVIF_VERSION.tar.gz"
fetch "https://downloads.videolan.org/pub/videolan/dav1d/$DAV1D_VERSION/dav1d-$DAV1D_VERSION.tar.xz" \
    "$DAV1D_SHA256" "$WORK/dav1d-$DAV1D_VERSION.tar.xz"
rm -rf "$WORK/libavif-$LIBAVIF_VERSION" "$WORK/dav1d-$DAV1D_VERSION" "$WORK/libyuv" "$WORK/out"
tar -xzf "$WORK/libavif-$LIBAVIF_VERSION.tar.gz" -C "$WORK"
tar -xJf "$WORK/dav1d-$DAV1D_VERSION.tar.xz" -C "$WORK"
# libyuv has no release tarballs; the commit hash is the integrity check.
git clone -q https://chromium.googlesource.com/libyuv/libyuv "$WORK/libyuv"
git -C "$WORK/libyuv" -c advice.detachedHead=false checkout -q "$LIBYUV_COMMIT"
[[ "$(git -C "$WORK/libyuv" rev-parse HEAD)" == "$LIBYUV_COMMIT" ]]

OUT=$WORK/out
mkdir -p "$OUT"
OUT_ABS=$(cd "$OUT" && pwd -W)

# --- dav1d (meson, static CRT to match .cargo/config.toml +crt-static) -------
(cd "$WORK/dav1d-$DAV1D_VERSION" && meson setup --vsenv build \
    --default-library=static --buildtype=release -Db_vscrt=mt \
    -Denable_tools=false -Denable_tests=false -Denable_examples=false \
    --prefix="$OUT_ABS/dav1d" && meson compile -C build && meson install -C build)

# --- libyuv (CMake) ------------------------------------------------------------
CMAKE_COMMON=(-G "Visual Studio 18 2026" -A x64 -DCMAKE_POLICY_DEFAULT_CMP0091=NEW
    "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded" -DBUILD_SHARED_LIBS=OFF)
cmake -S "$WORK/libyuv" -B "$WORK/libyuv/build" "${CMAKE_COMMON[@]}" -DLIBYUV_ENABLE_JPEG=OFF -DJPEG_FOUND=OFF
cmake --build "$WORK/libyuv/build" --config Release --target yuv --parallel

# --- libavif -----------------------------------------------------------------
# meson names MSVC static libraries libNAME.a
DAV1D_LIB=$OUT/dav1d/lib/libdav1d.a
[[ -f "$DAV1D_LIB" ]]
cmake -S "$WORK/libavif-$LIBAVIF_VERSION" -B "$WORK/libavif-build" "${CMAKE_COMMON[@]}" \
    -DAVIF_CODEC_DAV1D=SYSTEM -DAVIF_LIBYUV=SYSTEM \
    -DDAV1D_INCLUDE_DIR="$OUT_ABS/dav1d/include" -DDAV1D_LIBRARY="$(cd "$(dirname "$DAV1D_LIB")" && pwd -W)/$(basename "$DAV1D_LIB")" \
    -DLIBYUV_INCLUDE_DIR="$(cd "$WORK/libyuv/include" && pwd -W)" \
    -DLIBYUV_LIBRARY="$(cd "$WORK/libyuv/build/Release" && pwd -W)/yuv.lib" \
    -DAVIF_BUILD_APPS=OFF -DAVIF_BUILD_TESTS=OFF -DAVIF_BUILD_EXAMPLES=OFF \
    -DAVIF_LIBSHARPYUV=OFF -DAVIF_LIBXML2=OFF -DAVIF_ZLIBPNG=OFF -DAVIF_JPEG=OFF \
    -DCMAKE_INSTALL_PREFIX="$OUT_ABS/libavif"
cmake --build "$WORK/libavif-build" --config Release --parallel
cmake --install "$WORK/libavif-build" --config Release

# --- install into vendor/libavif ----------------------------------------------
rm -rf "$VENDOR/include" "$VENDOR/lib" "$VENDOR/licenses"
mkdir -p "$VENDOR/include" "$VENDOR/lib" "$VENDOR/licenses"
cp -r "$OUT/libavif/include/avif" "$VENDOR/include/"
cp "$OUT/libavif/lib/avif.lib" "$VENDOR/lib/avif.lib"
cp "$DAV1D_LIB" "$VENDOR/lib/dav1d.lib"
cp "$WORK/libyuv/build/Release/yuv.lib" "$VENDOR/lib/yuv.lib"
cp "$WORK/libavif-$LIBAVIF_VERSION/LICENSE" "$VENDOR/licenses/libavif-LICENSE.txt"
cp "$WORK/dav1d-$DAV1D_VERSION/COPYING" "$VENDOR/licenses/dav1d-COPYING.txt"
cp "$WORK/libyuv/LICENSE" "$VENDOR/licenses/libyuv-LICENSE.txt"
printf '%s\n' "$STAMP" > "$VENDOR/VERSION"
echo "Installed $STAMP into $VENDOR (tools: $TOOLS_ABS)"
