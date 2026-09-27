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
values are absolute mean luminance differences from the embedded preview
(lower is closer). Comparison
PNGs contain preview, auto-bright threshold 0.01, threshold 0.001, and no
auto-bright in that order.

### Dimensions, orientation, and comparisons

`Full` is the developed image; `info()` reports the same dimensions for every
supported sample. Aspect difference is the absolute percent difference
between preview and developed width/height ratios, after preview orientation.

| Case | Format · camera | Flip | Full dims | Info = Full | Half dims | Preview dims | Aspect Δ % | Comparison |
| --- | --- | ---: | ---: | :---: | ---: | ---: | ---: | --- |
| deflate-dng | DNG · Canon EOS 5D Mark III | 0 | 5796×3870 | yes | 2898×1935 | 5796×3870 | 0.00 | [PNG](../target/raw-compare/885.png) |
| cr2 | CR2 · Canon EOS 20D | 0 | 3522×2348 | yes | 1761×1174 | 1536×1024 | 0.00 | [PNG](../target/raw-compare/1018.png) |
| lossy-dng | DNG · Adobe DNG Converter Canon EOS 5D Mark III | 0 | 5760×3840 | yes | 5760×3840 | 5760×3840 | 0.00 | [PNG](../target/raw-compare/1023.png) |
| portrait-flip-5 | CR2 · Canon EOS Digital Rebel XT | 5 | 2314×3474 | yes | 1157×1737 | 1024×1536 | 0.09 | [PNG](../target/raw-compare/1230.png) |
| portrait-flip-6-nef | NEF · Nikon Df | 6 | 3292×4940 | yes | 1646×2470 | 3280×4928 | 0.12 | [PNG](../target/raw-compare/1386.png) |
| arw-compressed | ARW · Sony ILCE-7S | 0 | 2784×1872 | yes | 1392×936 | 1616×1080 | 0.61 | [PNG](../target/raw-compare/1582.png) |
| crw | CRW · Canon PowerShot G1 | 0 | 2088×1550 | yes | 1044×775 | 640×480 | 1.02 | [PNG](../target/raw-compare/2073.png) |
| pef | PEF · Pentax K10D | 0 | 3896×2616 | yes | 1948×1308 | 3872×2592 | 0.30 | [PNG](../target/raw-compare/2239.png) |
| cr3-craw | CR3 · Canon EOS M50 | 0 | 6024×4020 | yes | 3012×2010 | 6000×4000 | 0.10 | [PNG](../target/raw-compare/2663.png) |
| portrait-flip-6 | DNG · Ricoh GXR | 6 | 2748×3672 | yes | 1374×1836 | 480×640 | 0.22 | [PNG](../target/raw-compare/2756.png) |
| rwl | RWL · Leica D-LUX 5 | 0 | 2752×2754 | yes | 1376×1377 | 1920×1920 | 0.07 | [PNG](../target/raw-compare/2811.png) |
| phone-dng | DNG · Google Pixel 4 XL | 0 | 3700×2774 | yes | 1850×1387 | 672×502 | 0.36 | [PNG](../target/raw-compare/3502.png) |
| srw | SRW · Samsung NX500 | 0 | 6496×4336 | yes | 3248×2168 | 6480×4320 | 0.12 | [PNG](../target/raw-compare/3668.png) |
| raf-xtrans-compressed | RAF · Fujifilm X-T4 | 0 | 6246×4170 | yes | 3123×2085 | 4416×2944 | 0.14 | [PNG](../target/raw-compare/3914.png) |
| cr3 | CR3 · Canon EOS R6 | 0 | 3407×2271 | yes | 1704×1136 | 3408×2272 | 0.01 | [PNG](../target/raw-compare/4659.png) |
| nef | NEF · Nikon D2H | 0 | 2482×1648 | yes | 1241×824 | 570×375 | 0.93 | [PNG](../target/raw-compare/5227.png) |
| orf | ORF · Olympus E-10 | 0 | 2256×1684 | yes | 1128×842 | 160×120 | 0.47 | [PNG](../target/raw-compare/5424.png) |
| nef-he-negative | NEF · Nikon Z 8 | 0 | — (unsupported) | — | — | 8256×5504 | — | — |
| rw2 | RW2 · Panasonic DMC-LX7 | 0 | 1384×1384 | yes | 692×692 | 1920×1920 | 0.00 | [PNG](../target/raw-compare/7008.png) |
| arw-lossless | ARW · Sony ILCE-1M2 | 0 | 4608×3072 | yes | 4608×3072 | 4320×2880 | 0.00 | [PNG](../target/raw-compare/7834.png) |
| iiq | IIQ · Phase One P40+ | 0 | 3658×2740 | yes | 1829×1370 | 304×220 | 3.50 | [PNG](../target/raw-compare/8010.png) |

### Timings and brightness

Times are milliseconds. The three `ΔL` columns are absolute differences in
sampled mean luminance from the selected embedded preview. `—` indicates that
the Nikon HE decoder is unsupported or a measurement did not reach its trigger.

| Case | Info ms | Preview ms | Full ms | Half ms | Cancel ms | ΔL auto 0.01 | ΔL auto 0.001 | ΔL no auto |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| deflate-dng | 4.7 | 51.7 | 2201.9 | 358.4 | 69.5 | 0.0522 | 0.0023 | 0.0516 |
| cr2 | 3.2 | 9.0 | 1045.5 | 173.7 | 19.0 | 0.1743 | 0.1555 | 0.1401 |
| lossy-dng | 3.7 | 72.8 | 597.5 | 593.2 | 114.3 | 0.0281 | 0.0785 | 0.1265 |
| portrait-flip-5 | 2.9 | 11.5 | 846.2 | 169.6 | 22.6 | 0.0019 | 0.0148 | 0.1124 |
| portrait-flip-6-nef | 4.3 | 98.3 | 1483.8 | 313.2 | 36.7 | 0.1860 | 0.1860 | 0.1860 |
| arw-compressed | 2.9 | 12.6 | 488.5 | 39.3 | 8.8 | 0.0731 | 0.1549 | 0.1598 |
| crw | 1.4 | 1.7 | 279.1 | 58.0 | 8.3 | 0.0649 | 0.0075 | 0.0075 |
| pef | 3.3 | 42.1 | 862.4 | 182.5 | 19.1 | 0.0076 | 0.0268 | 0.0406 |
| cr3-craw | 5.4 | 69.3 | 1921.9 | 272.0 | 66.3 | 0.0914 | 0.0272 | 0.2304 |
| portrait-flip-6 | 1.7 | 2.2 | 855.6 | 142.4 | 22.6 | 0.0451 | 0.0785 | 0.0932 |
| rwl | 2.4 | 17.7 | 649.2 | 91.3 | 13.4 | 0.0700 | 0.0700 | 0.0700 |
| phone-dng | 1.6 | 2.7 | 977.3 | 186.9 | 29.6 | 0.0440 | 0.0268 | 0.0993 |
| srw | 5.8 | 100.4 | 2438.4 | 394.7 | 83.2 | 0.0511 | 0.0433 | 0.1024 |
| raf-xtrans-compressed | 6.0 | 85.0 | 13220.4 | 1370.6 | 183.2 | 0.0466 | 0.0566 | 0.0566 |
| cr3 | 4.7 | 53.3 | 1054.9 | 162.0 | 40.1 | 0.0746 | 0.0095 | 0.1355 |
| nef | 2.3 | 3.2 | 636.3 | 130.9 | 18.5 | 0.0329 | 0.0329 | 0.0329 |
| orf | 1.2 | 0.9 | 447.6 | 37.9 | 14.4 | 0.0731 | 0.0407 | 0.0290 |
| nef-he-negative | 15.1 | 314.3 | — | — | — | — | — | — |
| rw2 | 2.8 | 32.1 | 275.5 | 38.6 | 5.4 | 0.0116 | 0.0007 | 0.0842 |
| arw-lossless | 3.7 | 58.6 | 703.7 | 733.8 | 123.8 | 0.1391 | 0.1391 | 0.1392 |
| iiq | 6.3 | 1.3 | 1570.0 | 425.7 | 6.4 | 0.1385 | 0.1869 | 0.2123 |

Excluding the Ricoh file with its striped JPEG from the brightness comparison:
auto 0.01: mean 0.0716, median 0.0649, best in 10/19 samples; auto 0.001:
mean 0.0663, median 0.0407, best in 10/19 samples; no auto: mean 0.1061,
median 0.1024, best in 3/19 samples. Ties count for each candidate. These
mean-luminance figures are a screening measure; the linked PNGs support the
visual brightness choice. Full development ranged from 275.5 to 13220.4 ms;
measured cancel-to-exit latency ranged from 5.4 to 183.2 ms.

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
Nikon HE (`6616`) supplied a preview but its decoder was typed unsupported, so
development was not attempted by the benchmark.

## Verification

- `cargo test -j 1 -p mimageviewer --lib raw:: --offline`: 49 passed, 0 failed,
  including all 21 manifest samples and executor fake-job tests.
- Release-profile `bench_raw`: 21 results, no benchmark errors, and every
  supported developed dimension matched `info()`.
- `cargo build --release -j 1 --bin mimageviewer-core --offline`: passed.
  `check-vcrt-pe-dependencies.ps1` passed for that executable, with zero direct
  `msvcp*`, `vcruntime*`, or `concrt*` imports.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy -j 1 -p libraw-sys -p mimageviewer --lib --offline`: passed;
  after one local lint cleanup, `cargo clippy -j 1 -p mimageviewer --lib
  --offline` also passed. The latter reported 1,582 warnings across the large
  main crate, including dead-code warnings for S1's intentionally unwired API.
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
