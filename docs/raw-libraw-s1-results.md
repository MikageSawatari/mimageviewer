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
The build script watches the shim and native source/header directories so a
source change rebuilds the static archive; an initially stale shim archive
masked the unsupported-decoder guard until this was corrected.

## Samples and measurements

All 21 samples below are from the raw.pixls.us repository catalog with its
exact CC0 license URL. `scripts/setup-raw-samples.ps1` checks the recorded
SHA-256 and byte size. Times are milliseconds from
`cargo run --release -p mimageviewer --features dev-tools --bin bench_raw`
on this machine and are comparative, not throughput guarantees. Brightness
values compare each developed candidate to the embedded preview. The fourth
candidate matches the preview's median luminance in linear light; the five-panel
comparison PNGs support the user's visual decision.

### Dimensions, orientation, and comparisons

`Full` is the developed image; `info()` reports the same dimensions for every
supported sample. Aspect difference is the absolute percent difference
between the **decoded, oriented** preview and developed width/height ratios.
Every linked comparison PNG has five panels: preview | auto 0.01 | auto 0.001 |
no auto | match preview. The Nikon Z 8 JPEG advertises 8256×5504 and is decoded
at half DCT scale to 4128×2752; its HE RAW development remains unsupported.

| Case | Camera | Flip | Full dims | Info = Full | Half dims | Preview decoded | Aspect Δ % | Comparison |
| --- | --- | ---: | ---: | :---: | ---: | ---: | ---: | --- |
| deflate-dng | Canon EOS 5D Mark III | 0 | 5796×3870 | yes | 2898×1935 | 5796×3870 | 0.00 | [PNG](../target/raw-compare/885.png) |
| cr2 | Canon EOS 20D | 0 | 3522×2348 | yes | 1761×1174 | 1536×1024 | 0.00 | [PNG](../target/raw-compare/1018.png) |
| lossy-dng | Adobe DNG Converter Canon EOS 5D Mark III | 0 | 5760×3840 | yes | 5760×3840 | 5760×3840 | 0.00 | [PNG](../target/raw-compare/1023.png) |
| portrait-flip-5 | Canon EOS Digital Rebel XT | 5 | 2314×3474 | yes | 1157×1737 | 1024×1536 | 0.09 | [PNG](../target/raw-compare/1230.png) |
| portrait-flip-6-nef | Nikon Df | 6 | 3292×4940 | yes | 1646×2470 | 3280×4928 | 0.12 | [PNG](../target/raw-compare/1386.png) |
| arw-compressed | Sony ILCE-7S | 0 | 2784×1872 | yes | 1392×936 | 1616×1080 | 0.61 | [PNG](../target/raw-compare/1582.png) |
| crw | Canon PowerShot G1 | 0 | 2088×1550 | yes | 1044×775 | 640×480 | 1.02 | [PNG](../target/raw-compare/2073.png) |
| pef | Pentax K10D | 0 | 3896×2616 | yes | 1948×1308 | 3872×2592 | 0.30 | [PNG](../target/raw-compare/2239.png) |
| cr3-craw | Canon EOS M50 | 0 | 6024×4020 | yes | 3012×2010 | 6000×4000 | 0.10 | [PNG](../target/raw-compare/2663.png) |
| portrait-flip-6 | Ricoh GXR | 6 | 2748×3672 | yes | 1374×1836 | 480×640 | 0.22 | [PNG](../target/raw-compare/2756.png) |
| rwl | Leica D-LUX 5 | 0 | 2752×2754 | yes | 1376×1377 | 1920×1920 | 0.07 | [PNG](../target/raw-compare/2811.png) |
| phone-dng | Google Pixel 4 XL | 0 | 3700×2774 | yes | 1850×1387 | 672×502 | 0.36 | [PNG](../target/raw-compare/3502.png) |
| srw | Samsung NX500 | 0 | 6496×4336 | yes | 3248×2168 | 6480×4320 | 0.12 | [PNG](../target/raw-compare/3668.png) |
| raf-xtrans-compressed | Fujifilm X-T4 | 0 | 6246×4170 | yes | 3123×2085 | 4416×2944 | 0.14 | [PNG](../target/raw-compare/3914.png) |
| cr3 | Canon EOS R6 | 0 | 3407×2271 | yes | 1704×1136 | 3408×2272 | 0.01 | [PNG](../target/raw-compare/4659.png) |
| nef | Nikon D2H | 0 | 2482×1648 | yes | 1241×824 | 570×375 | 0.93 | [PNG](../target/raw-compare/5227.png) |
| orf | Olympus E-10 | 0 | 2256×1684 | yes | 1128×842 | 160×120 | 0.47 | [PNG](../target/raw-compare/5424.png) |
| nef-he-negative | Nikon Z 8 | 0 | — (unsupported) | — | — | 4128×2752 | — | — |
| rw2 | Panasonic DMC-LX7 | 0 | 1384×1384 | yes | 692×692 | 1920×1920 | 0.00 | [PNG](../target/raw-compare/7008.png) |
| arw-lossless | Sony ILCE-1M2 | 0 | 4608×3072 | yes | 4608×3072 | 4320×2880 | 0.00 | [PNG](../target/raw-compare/7834.png) |
| iiq | Phase One P40+ | 0 | 3658×2740 | yes | 1829×1370 | 304×220 | 3.50 | [PNG](../target/raw-compare/8010.png) |

### Timings and brightness

All times are milliseconds from the release-profile benchmark. `Match ms`
includes one no-auto develop, the linear-median calculation, and two
`copy_mem_image` calls from the same LibRaw instance. The first copy supplies
the no-auto panel; the second uses the matched gain or fallback brightness.

| Case | Info ms | Preview ms | Full ms | Half ms | Match ms | Cancel ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| deflate-dng | 4.6 | 72.2 | 2200.1 | 422.7 | 2353.9 | 56.3 |
| cr2 | 2.1 | 10.4 | 867.5 | 170.0 | 813.6 | 17.5 |
| lossy-dng | 2.2 | 90.3 | 568.2 | 580.4 | 662.2 | 112.1 |
| portrait-flip-5 | 2.2 | 13.5 | 973.5 | 211.3 | 1037.6 | 18.9 |
| portrait-flip-6-nef | 2.9 | 84.2 | 1786.1 | 341.6 | 1876.6 | 50.5 |
| arw-compressed | 2.2 | 15.0 | 513.0 | 50.3 | 484.0 | 7.1 |
| crw | 0.9 | 2.5 | 376.5 | 85.2 | 398.6 | 4.1 |
| pef | 2.3 | 48.3 | 991.6 | 195.4 | 1198.7 | 27.5 |
| cr3-craw | 3.4 | 96.1 | 2527.8 | 381.8 | 2526.5 | 81.2 |
| portrait-flip-6 | 0.9 | 2.8 | 1193.6 | 188.5 | 995.3 | 28.3 |
| rwl | 1.3 | 21.8 | 708.6 | 100.2 | 715.3 | 13.7 |
| phone-dng | 1.1 | 3.5 | 997.1 | 204.5 | 977.9 | 30.4 |
| srw | 4.3 | 120.5 | 2641.7 | 445.7 | 2779.4 | 86.3 |
| raf-xtrans-compressed | 4.3 | 89.3 | 13115.1 | 1409.2 | 13513.5 | 106.7 |
| cr3 | 2.0 | 29.8 | 746.1 | 144.9 | 752.6 | 16.1 |
| nef | 1.1 | 3.2 | 431.7 | 81.9 | 410.6 | 5.0 |
| orf | 0.6 | 1.1 | 321.6 | 24.6 | 339.7 | 1.8 |
| nef-he-negative | 8.1 | 126.6 | — | — | — | — |
| rw2 | 1.2 | 19.7 | 185.1 | 26.6 | 198.3 | 4.6 |
| arw-lossless | 1.3 | 30.4 | 445.0 | 480.5 | 536.1 | 89.3 |
| iiq | 0.9 | 1.1 | 1074.0 | 245.6 | 1103.5 | 0.9 |

For the match candidate, the benchmark samples up to 100,000 pixels from the
oriented embedded preview and no-auto developed image. It converts sRGB
channels to linear light, uses Rec.709 luminance weights, and divides their
medians. Gain is clamped to [0.125, 8]. LibRaw's `copy_mem_image` computes
`gamma_curve(..., (t_white << 3) / bright)`; the curve uses
`r = linear_input / imax`, so `bright` multiplies linear input before the sRGB
curve. The shim updates `bright` after `dcraw_process` and copies again;
demosaicing is not repeated. An unusable preview or zero/invalid median falls
back to auto-bright threshold 0.001. These are measurement rules only;
product brightness defaults are unchanged.

The first four ΔL columns compare sampled **mean sRGB-encoded** luminance to
the preview, preserving the earlier benchmark's visual screening measure.
`Median ΔL` is the absolute difference in **linear** luminance medians after
matching. `Decision` records clamping or fallback; plain `gain` means neither.

| Case | ΔL auto 0.01 | ΔL auto 0.001 | ΔL no auto | ΔL match mean | Median ΔL linear | Gain | Decision |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| deflate-dng | 0.0522 | 0.0023 | 0.0516 | 0.0047 | 0.0001 | 1.330 | gain |
| cr2 | 0.1743 | 0.1555 | 0.1401 | 0.0261 | 0.0011 | 2.453 | gain |
| lossy-dng | 0.0281 | 0.0785 | 0.1265 | 0.0124 | 0.0006 | 2.126 | gain |
| portrait-flip-5 | 0.0019 | 0.0148 | 0.1124 | 0.0011 | 0.0005 | 2.069 | gain |
| portrait-flip-6-nef | 0.1860 | 0.1860 | 0.1860 | 0.0406 | 0.0000 | 2.603 | gain |
| arw-compressed | 0.0731 | 0.1549 | 0.1598 | 0.0520 | 0.0013 | 2.465 | gain |
| crw | 0.0649 | 0.0075 | 0.0075 | 0.0075 | 0.0000 | 1.000 | gain |
| pef | 0.0076 | 0.0268 | 0.0406 | 0.0215 | 0.0002 | 1.503 | gain |
| cr3-craw | 0.0914 | 0.0272 | 0.2304 | 0.0209 | 0.0017 | 3.185 | gain |
| portrait-flip-6 | 0.0451 | 0.0785 | 0.0932 | 0.0998 | 0.0012 | 2.392 | gain |
| rwl | 0.0700 | 0.0700 | 0.0700 | 0.0067 | 0.0003 | 1.452 | gain |
| phone-dng | 0.0440 | 0.0268 | 0.0993 | 0.0586 | 0.0008 | 2.169 | gain |
| srw | 0.0511 | 0.0433 | 0.1024 | 0.0183 | 0.0005 | 1.789 | gain |
| raf-xtrans-compressed | 0.0466 | 0.0566 | 0.0566 | 0.0005 | 0.0002 | 1.506 | gain |
| cr3 | 0.0746 | 0.0095 | 0.1355 | 0.0175 | 0.0007 | 2.397 | gain |
| nef | 0.0329 | 0.0329 | 0.0329 | 0.0320 | 0.0002 | 1.279 | gain |
| orf | 0.0731 | 0.0407 | 0.0290 | 0.0367 | 0.0004 | 1.348 | gain |
| nef-he-negative | — | — | — | — | — | — | — |
| rw2 | 0.0116 | 0.0007 | 0.0842 | 0.0094 | 0.0003 | 1.699 | gain |
| arw-lossless | 0.1391 | 0.1391 | 0.1392 | 0.0295 | 0.0017 | 2.648 | gain |
| iiq | 0.1385 | 0.1869 | 0.2123 | 0.0121 | 0.0008 | 3.476 | gain |

Across the 20 developed samples, applied gain ranged from 1.000 to 3.476
(median 2.098, mean 2.044); 0 were clamped and 0 fell back. The median linear
ΔL averaged 0.0007 and peaked at 0.0017. Excluding the Ricoh file with its
striped JPEG, the mean-luminance ΔL averages were 0.0716 (auto 0.01), 0.0663
(auto 0.001), 0.1061 (no auto), and 0.0215 (match preview). These numbers do
not replace the user's visual choice from the PNGs. Full development ranged
from 185.1 to 13115.1 ms; cancel-to-exit latency ranged from 0.9 to 112.1 ms.

### WIC comparison for DNG

WIC's dimensions can represent a DNG preview rather than the developed image.

| Case | LibRaw developed dims | WIC decoded dims |
| --- | ---: | ---: |
| deflate-dng | 5796×3870 | 5796×3870 |
| lossy-dng | 5760×3840 | 5760×3840 |
| portrait-flip-6 | 2748×3672 | 640×480 |
| phone-dng | 3700×2774 | 672×502 |


## Additional format survey

The LibRaw 0.22.2 source lists the corresponding cameras and contains their
decoder or metadata paths. This is source-level support evidence, not an S1
decode test. The counts use only entries with the exact CC0 license URL in the
raw.pixls.us catalog.

| Format | LibRaw 0.22.2 source evidence | CC0 samples |
| --- | --- | ---: |
| 3FR | Hasselblad camera list and 3FR unpacking changelog | 9 |
| ERF | Epson R-D1 camera list and Epson metadata path | 3 |
| KDC | Kodak camera list and Kodak metadata path | 5 |
| DCR | Kodak camera list and `Kodak_DCR_WBtags` path | 5 |
| MRW | Minolta camera list and Minolta metadata path | 8 |
| MOS | Leaf camera list and Leaf metadata path | 3 |
| MEF | Mamiya ZD camera list; extension decode unverified | 0 |

MEF has no strict-CC0 sample in that catalog, so its actual development remains
unverified.

## Plan observations

The Ricoh GXR DNG (`2756`) carries a striped preview in the file itself.
`exiftool -b -PreviewImage` extracted a 56,811-byte JPEG with SHA-256
`ef3b2ceae1ade1ba20e67c90ecdf6e1b3ade657340913f08df685ba4b117d485`.
Pillow decoded that independently extracted JPEG as 640×480; its pixel at
`(100, 0)` is magenta `(255, 121, 255)` and at `(100, 80)` is green
`(0, 135, 0)`. These bands match the benchmark image. The preview remains
decodable under the plan's definition; no special content filter was added.

The updated orientation rule uses `sizes.flip` when `tflip` is 0 or `0xffff`.
The Nikon Df (`1386`) preview is now portrait at 3280×4928, compared with
developed 3292×4940; its aspect difference is 0.12%. The Ricoh GXR (`2756`)
striped preview is also oriented portrait at 480×640, with an aspect
difference of 0.22%. The decoder rejects clear landscape/portrait mismatches
for non-square images with the typed `OrientationMismatch`
preview-unavailable reason.

LibRaw returned full-size dimensions for `Half` on lossy DNG and lossless ARW.
This is measured behavior; no half-size workaround or product behavior change
was made.

The CR3 samples `2663` and `4659` each include a JPEG thumbnail whose
`thumbs_list` dimensions are 0×0; the JPEG header supplied usable dimensions.
Nikon HE (`6616`) supplied an 8256×5504 JPEG preview. TurboJPEG DCT scaling
decoded it to 4128×2752; its RAW decoder remains typed unsupported, so
development was not attempted by the benchmark.

## Verification

- `cargo test -j 1 -p mimageviewer --lib raw:: --offline`: 56 passed, 0 failed,
  including all 21 manifest samples, Nikon Z 8 scaling, gain/fallback tests,
  and executor fake-job tests.
- `cargo test -j 1 -p libraw-sys --offline`: 3 passed, 0 failed.
- `cargo check -j 1 -p mimageviewer --features dev-tools --bin bench_raw
  --offline`: passed.
- Release-profile `bench_raw`: 21 results, no benchmark errors, 20 five-panel
  PNGs, and every supported developed dimension matched `info()`.
- The earlier S1 `cargo build --release -j 1 --bin mimageviewer-core --offline`
  passed; it was not rerun for this follow-up.
  `check-vcrt-pe-dependencies.ps1` passed for that executable, with zero direct
  `msvcp*`, `vcruntime*`, or `concrt*` imports.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy -j 1 -p libraw-sys -p mimageviewer --features dev-tools --lib
  --bin bench_raw --offline`: passed after a local lint cleanup; the main
  crate still reports 1,535 broad warnings, including S1's unwired API.
- The Windows-host non-Windows shadow script was attempted with shared and
  isolated target directories, including offline mode, but its PowerShell
  Cargo capture did not return a verdict. A direct offline check from the
  generated shadow tree first found an existing `ort-patched` dependency
  artifact of the host cfg rewrite: its Windows arm was disabled while rustc
  still targeted Windows. After enabling the Linux arm only in the disposable
  shadow copy, the sole remaining error was the script's documented
  `std::os::unix` host-standard-library false positive in `zip_loader.rs`.
  There were no other errors, including none in S1 RAW code. Ubuntu CI remains
  the final non-Windows check.
