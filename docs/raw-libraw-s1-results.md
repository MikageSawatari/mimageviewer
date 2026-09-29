# RAW via LibRaw S1 results

This document records the S1 vendor setup, isolated decoder and executor, and
sample measurements. Product loading paths and UI were not changed.

## Decision

On 2026-09-27, the user chose **match the embedded preview** as the product
brightness default, with a setting to choose **none**. The product `develop`
API now measures a cheaply decoded usable preview, develops once without
auto-bright, measures its first RGB copy, drops that copy, and re-copies from
the same LibRaw process with the clamped linear-light gain. It returns the
applied gain, clamp state, or fallback reason with the image. If no usable
preview or either median is zero/invalid, it falls back to LibRaw auto-bright
threshold 0.001. JPEG previews at or below 1024 pixels on their long edge
are measured at full size; larger JPEGs use the smallest DCT image that
retains at least 1024 pixels. BITMAP previews follow the same 1024-pixel
rule. The setting and UI connection belong to S3.

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

The original 21 S1 samples below are from the raw.pixls.us repository catalog
with its exact CC0 license URL. `scripts/setup-raw-samples.ps1` checks the recorded
SHA-256 and byte size. Times are milliseconds from
`cargo run --release -p mimageviewer --features dev-tools --bin bench_raw`
on this machine and are comparative, not throughput guarantees. Brightness
values compare each developed candidate to the embedded preview. The fourth
candidate runs the product MatchPreview executor path; the five-panel
comparison PNGs show its actual output.

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
measures the complete product MatchPreview job: preview extraction and
statistic decode, one no-auto develop, the linear-median calculation, and two
`copy_mem_image` calls from the same LibRaw instance. The first copy is
dropped before the second. The no-auto panel is measured in a separate job.

| Case | Info ms | Preview ms | Full ms | Half ms | Match ms | Cancel ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| deflate-dng | 2.8 | 57.1 | 2015.7 | 358.3 | 2287.2 | 63.0 |
| cr2 | 1.6 | 9.4 | 876.2 | 155.4 | 836.8 | 21.2 |
| lossy-dng | 1.7 | 81.9 | 538.0 | 534.7 | 636.6 | 98.7 |
| portrait-flip-5 | 1.6 | 13.5 | 908.7 | 190.0 | 951.7 | 23.0 |
| portrait-flip-6-nef | 2.3 | 78.4 | 1614.3 | 301.1 | 1743.3 | 40.9 |
| arw-compressed | 1.5 | 14.8 | 496.3 | 66.2 | 505.1 | 15.4 |
| crw | 1.1 | 1.8 | 429.7 | 85.4 | 436.1 | 13.4 |
| pef | 2.0 | 49.8 | 1147.6 | 210.0 | 1035.6 | 24.4 |
| cr3-craw | 3.7 | 84.9 | 2720.5 | 620.2 | 3245.3 | 82.5 |
| portrait-flip-6 | 0.8 | 3.3 | 1115.5 | 153.6 | 1147.8 | 24.9 |
| rwl | 1.9 | 22.4 | 877.1 | 118.4 | 933.2 | 24.7 |
| phone-dng | 1.1 | 4.0 | 1158.8 | 243.0 | 1210.6 | 25.6 |
| srw | 3.6 | 145.0 | 2687.5 | 451.4 | 3025.6 | 72.9 |
| raf-xtrans-compressed | 5.6 | 112.2 | 13785.8 | 1236.6 | 14838.0 | 115.6 |
| cr3 | 2.3 | 34.9 | 826.0 | 124.6 | 798.0 | 18.7 |
| nef | 1.1 | 2.2 | 411.8 | 74.5 | 423.0 | 6.4 |
| orf | 0.7 | 0.8 | 349.8 | 31.7 | 352.2 | 4.3 |
| nef-he-negative | 6.8 | 123.6 | — | — | — | — |
| rw2 | 1.1 | 18.3 | 181.8 | 26.4 | 232.8 | 2.1 |
| arw-lossless | 1.8 | 32.4 | 437.9 | 460.7 | 520.8 | 72.5 |
| iiq | 0.9 | 1.4 | 1163.8 | 286.6 | 1070.3 | 1.0 |

For the match candidate, the product job samples up to 100,000 pixels from the
oriented statistic preview and no-auto developed image. It converts sRGB
channels to linear light, uses Rec.709 luminance weights, and divides their
medians. Gain is clamped to [0.125, 8]. LibRaw's `copy_mem_image` computes
`gamma_curve(..., (t_white << 3) / bright)`; the curve uses
`r = linear_input / imax`, so `bright` multiplies linear input before the sRGB
curve. The shim updates `bright` after `dcraw_process` and copies again;
demosaicing is not repeated. An unusable preview or zero/invalid median falls
back to auto-bright threshold 0.001. Auto 0.01 and auto 0.001 remain
benchmark-only standalone choices; the latter is an internal product fallback.

The first four ΔL columns compare sampled **mean sRGB-encoded** luminance to
the full decoded preview. `Median ΔL` is the absolute difference in **linear**
luminance medians between the full preview and product output. `Product gain`
comes from the returned product decision; `Full-preview gain` uses the earlier
full-preview statistic with the separate no-auto panel. `Decision` records
clamping or fallback; plain `gain` means neither.

| Case | ΔL auto 0.01 | ΔL auto 0.001 | ΔL no auto | ΔL match mean | Median ΔL linear | Product gain | Full-preview gain | Decision |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| deflate-dng | 0.0522 | 0.0023 | 0.0516 | 0.0039 | 0.0006 | 1.336 | 1.330 | gain |
| cr2 | 0.1743 | 0.1555 | 0.1401 | 0.0266 | 0.0016 | 2.457 | 2.453 | gain |
| lossy-dng | 0.0281 | 0.0785 | 0.1265 | 0.0128 | 0.0011 | 2.130 | 2.126 | gain |
| portrait-flip-5 | 0.0019 | 0.0148 | 0.1124 | 0.0010 | 0.0006 | 2.070 | 2.069 | gain |
| portrait-flip-6-nef | 0.1860 | 0.1860 | 0.1860 | 0.0408 | 0.0005 | 2.606 | 2.603 | gain |
| arw-compressed | 0.0731 | 0.1549 | 0.1598 | 0.0513 | 0.0002 | 2.459 | 2.465 | gain |
| crw | 0.0649 | 0.0075 | 0.0075 | 0.0075 | 0.0000 | 1.000 | 1.000 | gain |
| pef | 0.0076 | 0.0268 | 0.0406 | 0.0223 | 0.0007 | 1.511 | 1.503 | gain |
| cr3-craw | 0.0914 | 0.0272 | 0.2304 | 0.0239 | 0.0053 | 3.220 | 3.185 | gain |
| portrait-flip-6 | 0.0451 | 0.0785 | 0.0932 | 0.0998 | 0.0012 | 2.392 | 2.392 | gain |
| rwl | 0.0700 | 0.0700 | 0.0700 | 0.0071 | 0.0007 | 1.455 | 1.452 | gain |
| phone-dng | 0.0440 | 0.0268 | 0.0993 | 0.0586 | 0.0008 | 2.169 | 2.169 | gain |
| srw | 0.0511 | 0.0433 | 0.1024 | 0.0180 | 0.0004 | 1.787 | 1.789 | gain |
| raf-xtrans-compressed | 0.0466 | 0.0566 | 0.0566 | 0.0011 | 0.0005 | 1.512 | 1.506 | gain |
| cr3 | 0.0746 | 0.0095 | 0.1355 | 0.0175 | 0.0007 | 2.397 | 2.397 | gain |
| nef | 0.0329 | 0.0329 | 0.0329 | 0.0320 | 0.0002 | 1.279 | 1.279 | gain |
| orf | 0.0731 | 0.0407 | 0.0290 | 0.0367 | 0.0004 | 1.348 | 1.348 | gain |
| nef-he-negative | — | — | — | — | — | — | — | — |
| rw2 | 0.0116 | 0.0007 | 0.0842 | 0.0093 | 0.0002 | 1.698 | 1.699 | gain |
| arw-lossless | 0.1391 | 0.1391 | 0.1392 | 0.0295 | 0.0017 | 2.648 | 2.648 | gain |
| iiq | 0.1385 | 0.1869 | 0.2123 | 0.0121 | 0.0008 | 3.476 | 3.476 | gain |

Across the 20 developed samples, product gain ranged from 1.000 to 3.476
(median 2.100, mean 2.047); 0 were clamped and 0 fell back. The largest
absolute difference from the full-preview gain was 0.035 on CR3 CRAW. The
median linear ΔL against the full preview averaged 0.0009 and peaked at
0.0053. Excluding the Ricoh file with its striped JPEG, the mean-luminance
ΔL averages were 0.0716 (auto 0.01), 0.0663 (auto 0.001), 0.1061 (no auto),
and 0.0217 (product match preview). Full development ranged from 181.8 to
13785.8 ms; cancel-to-exit latency ranged from 1.0 to 115.6 ms.

### Additional formats (added 2026-09-29)

Six further strict-CC0 raw.pixls.us samples were added after the original S1
measurements. Each file's recorded SHA-256 and byte size was verified by
`scripts/setup-raw-samples.ps1`. `info()` succeeded, and full development
matched `info().developed_dims` for all six. Preview orientation and usability
were checked by the per-sample decoder test. Leaf MOS reports flip 6, but its
embedded preview fails the orientation consistency check; `preview()` returns
`NoUsablePreview(OrientationMismatch)`, and MatchPreview uses its documented
auto-bright fallback. The comparison PNGs for the other five formats contain
the same five panels as the original S1 rows.

| Case | Camera | Flip | Full dims | Info = Full | Half dims | Preview decoded | Aspect Δ % | Comparison |
| --- | --- | ---: | ---: | :---: | ---: | ---: | ---: | --- |
| 3fr | Hasselblad H3D | 0 | 7247×5444 | yes | 3624×2722 | 320×240 | 0.16 | [PNG](../target/raw-compare/2851.png) |
| erf | Epson R-D1 | 0 | 3040×2024 | yes | 1520×1012 | 640×424 | 0.50 | [PNG](../target/raw-compare/2680.png) |
| kdc | Kodak DC120 | 0 | 1301×976 | yes | 651×488 | 80×60 | 0.03 | [PNG](../target/raw-compare/2338.png) |
| dcr | Kodak DCS760C | 0 | 3040×2016 | yes | 1520×1008 | 760×504 | 0.00 | [PNG](../target/raw-compare/1347.png) |
| mrw | Minolta DiMAGE 5 | 0 | 2056×1544 | yes | 1028×772 | 640×480 | 0.13 | [PNG](../target/raw-compare/7795.png) |
| mos | Leaf Aptus 22 | 6 | 5344×4008 | yes | 2672×2004 | unusable: orientation mismatch | — | — |

Times below are milliseconds from release-profile `bench_raw`, filtered to
these six IDs with `RAW_BENCH_ONLY`. `Match ms` includes the product brightness
path, including its fallback for MOS.

| Case | Info ms | Preview ms | Full ms | Half ms | Match ms | Cancel ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 3fr | 1.3 | 0.7 | 2702.3 | 624.8 | 3486.1 | 123.8 |
| erf | 1.2 | 1.8 | 445.8 | 50.1 | 509.6 | 7.8 |
| kdc | 1.2 | 0.5 | 73.4 | 9.8 | 74.4 | 24.3 |
| dcr | 17.2 | 27.2 | 534.9 | 76.4 | 509.1 | 9.7 |
| mrw | 1.3 | 2.3 | 251.9 | 25.6 | 279.1 | 8.8 |
| mos | 1.1 | 1.4 | 1985.1 | 446.7 | 2106.8 | 58.4 |

The first four ΔL columns use the same sampled mean sRGB comparison as the
original table. The median column compares linear-light medians.

| Case | ΔL auto 0.01 | ΔL auto 0.001 | ΔL no auto | ΔL match mean | Median ΔL linear | Product gain | Full-preview gain | Decision |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 3fr | 0.0016 | 0.0155 | 0.0016 | 0.0177 | 0.0000 | 1.079 | 1.079 | gain |
| erf | 0.0274 | 0.0357 | 0.0351 | 0.0040 | 0.0002 | 1.214 | 1.214 | gain |
| kdc | 0.2050 | 0.2050 | 0.2049 | 0.0151 | 0.0005 | 0.448 | 0.448 | gain |
| dcr | 0.0473 | 0.0631 | 0.2070 | 0.0021 | 0.0013 | 3.129 | 3.129 | gain |
| mrw | 0.0502 | 0.0927 | 0.0501 | 0.0099 | 0.0002 | 0.820 | 0.820 | gain |
| mos | — | — | — | — | — | — | — | NoPreview fallback |

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

- `cargo test -j 1 -p mimageviewer --lib raw:: --offline`: 60 passed, 0 failed,
  including all 21 manifest samples, small-preview statistic scaling,
  cancellation before RGB allocation, gain/fallback, and executor fake-job tests.
- `cargo test -j 1 -p libraw-sys --offline`: 3 passed, 0 failed.
- Release-profile `bench_raw` rerun: 21 results, no benchmark errors, 20
  five-panel PNGs showing the product MatchPreview output, and every supported
  developed dimension matched `info()`.
- Fresh `cargo build --release --bin mimageviewer-core -j 1 --offline`: passed
  (28m 41s). `check-vcrt-pe-dependencies.ps1 -InputPaths
  target\release\mimageviewer-core.exe` passed (`runtime=4 pe=1`). The x64 core
  has zero direct `msvcp*`, `vcruntime*`, or `concrt*` imports; SHA-256 is
  `646976860BB56BD9DF95B9EF01205F0364749B103D24AEA82DB752EA58FAFABB`.
- `cargo fmt --all` and `cargo fmt --all --check`: passed.
- `cargo clippy -p libraw-sys -j 1 --offline` and `cargo clippy -p
  mimageviewer --lib --bin bench_raw --features dev-tools -j 1 --offline`:
  passed. The main crate still reports 1,535 broad warnings, with none in the
  changed RAW decoder or benchmark paths.
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
