# RAW via LibRaw S1 results

This document records the S1 vendor setup, isolated decoder and executor, and
sample measurements. Product loading paths and UI were not changed.

## Codec and build decisions

LibRaw 0.22.2 is built from its `Makefile.msvc` `LIB_OBJECTS` source list. The
build defines `LIBRAW_NODLL`, `LIBRAW_BUILDLIB`, `USE_ZLIB`, and `USE_JPEG`.
Official zlib 1.3.1 source is pinned and compiled statically with `cc`.
The existing `turbojpeg-sys` build installs `turbojpeg-static.lib`, which
contains the libjpeg API and exposes its headers. LibRaw links that archive
directly; linking `jpeg-static.lib` as well would duplicate JPEG symbols.
The project target's `+crt-static` and the `cc` toolchain select `/MT`.
Neither OpenMP nor X3FTOOLS is enabled.

## Samples and measurements

All 20 samples below are from the raw.pixls.us repository catalog with its
exact CC0 license URL. `scripts/setup-raw-samples.ps1` checks the recorded
SHA-256 and byte size. Times are milliseconds on this machine and are
comparative, not throughput guarantees. Brightness values are absolute mean
luminance differences from the embedded preview (lower is closer). Comparison
PNGs contain preview, auto-bright threshold 0.01, threshold 0.001, and no
auto-bright in that order.

<!-- S1 measurements inserted after benchmark completion. -->

## Additional format survey

The LibRaw 0.22.2 source contains decoder paths or supported-camera entries
for 3FR (Hasselblad), ERF (Epson), KDC/DCR (Kodak), MRW (Minolta), MOS
(Leaf), and MEF (Mamiya). This is source-level support evidence; S1 did not
exercise these formats. Strict CC0 sample counts in the raw.pixls.us catalog
were 3FR 9, ERF 3, KDC 5, DCR 5, MRW 8, MOS 3, and MEF 0. MEF remains
unverified by a CC0 sample.

## Plan observations

<!-- S1 observations inserted after benchmark completion. -->
