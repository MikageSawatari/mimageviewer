# Codex Design Review: Offline Video Upscale Export

Date: 2026-05-01
Reviewer target: ClaudeCode

## Goal

mImageViewer already has still-image AI upscale models and FFmpeg-based video playback.
This design adds a non-realtime video upscale export feature while keeping the UI responsive,
keeping mIV itself under MIT, and keeping FFmpeg redistribution compliant with the bundled
LGPL shared build.

This document is intentionally written before implementation so ClaudeCode can review the
architecture and licensing plan first.

## Current Findings

- mIV is MIT licensed.
- The bundled FFmpeg is from BtbN `ffmpeg-n7.1*-win64-lgpl-shared-7.1.zip`.
- The current `vendor/ffmpeg/LICENSE.txt` is LGPLv3.
- The current FFmpeg DLLs contain `--enable-version3` and report `LGPL version 3 or later`.
- The current FFmpeg configure string includes AV1 encoders:
  `--enable-libaom`, `--enable-librav1e`, `--enable-libsvtav1`.
- The current FFmpeg configure string disables GPL x264/x265:
  `--disable-libx264`, `--disable-libx265`.
- Existing docs still mention LGPLv2.1 in several places. Those should be changed to
  LGPLv3-or-later for the currently bundled FFmpeg build.

Primary references:

- FFmpeg legal checklist: https://ffmpeg.org/legal.html
- GNU LGPLv3: https://www.gnu.org/licenses/lgpl-3.0.html
- AOMedia AV1 overview: https://aomedia.org/specifications/av1/
- FFmpeg codec docs for `libsvtav1`: https://ffmpeg.org/ffmpeg-codecs.html

## Licensing And Source Distribution Plan

### Documentation Updates

Update all user-facing and developer-facing FFmpeg license notices from LGPLv2.1 to
LGPLv3-or-later for the current bundled BtbN build.

Expected targets:

- `CLAUDE.md`
- `docs/video-architecture.md`
- `../../README.md` if it names FFmpeg license details
- `installer/readme.txt`
- `htdocs/mimageviewer/index.html` if download/legal text is present
- `src/ui_dialogs/about.rs`
- `scripts/setup-ffmpeg.sh`

### Source Distribution

The current docs say to host an FFmpeg source tarball on `mikage.to`. With the BtbN build,
that may not be enough because external libraries are statically included into the FFmpeg
DLLs. The implementation should add a release checklist and helper script that records:

- the BtbN asset name used by `vendor/ffmpeg/VERSION`
- the FFmpeg configure string extracted from the DLLs
- the LGPLv3 license text already copied as `vendor/ffmpeg/LICENSE.txt`
- the need to provide corresponding FFmpeg source
- the need to provide or link corresponding source/license material for bundled external
  FFmpeg dependencies enabled in the build, especially AV1/audio libraries used by encoding
  or muxing paths

Proposed artifact:

- `docs/ffmpeg-lgpl-source-distribution.md`
- `scripts/collect-ffmpeg-lgpl-info.ps1`

The helper script should not attempt a perfect legal bundle automatically in MVP. It should
produce an auditable text report from the current DLLs and `vendor/ffmpeg/VERSION`, so release
packaging can be checked consistently.

### Practical LGPLv3 Implications

For mIV's desktop distribution, the important requirements are:

- keep FFmpeg dynamically linked as DLL files at runtime
- do not use BtbN GPL or nonfree builds
- do not rename DLLs deceptively
- provide prominent notices and license texts
- provide corresponding source information for FFmpeg and relevant bundled external libraries
- do not prevent reverse engineering/debugging for FFmpeg modification
- allow users to replace compatible FFmpeg DLLs

The implementation must not make FFmpeg a statically linked part of mIV.

## ClaudeCode Review Decisions

ClaudeCode reviewed this design on 2026-05-01 and validated the important local
premises on the actual bundled binaries:

- the current BtbN FFmpeg build is LGPLv3-or-later, not LGPLv2.1
- `libsvtav1`, `libaom`, and `librav1e` are present
- `libx264` and `libx265` are disabled
- `UpscaleRealEsrGeneralV3` is present, and mIV also bundles anime/photo-oriented upscale models

The following decisions supersede the earlier open questions in this document:

- Phase 1 can proceed immediately: license notices, source distribution docs/script,
  and sidecar schema/tests.
- Phase 2 UI should expose a small model preset choice: generic fast, anime, photo.
- `source.head_tail_sha256` is required in the sidecar MVP. Pairing must tolerate
  mtime-only changes and rely on size plus the partial hash.
- MVP video export is video-only with a clear warning. Audio copy/remux is a later
  Phase 2.5 task.
- The dialog must show output resolution, estimated processing time, and estimated
  output size before starting.
- Output beyond the 8K UHD practical limit is not allowed in MVP. Use a conservative
  guard of long edge <= 7680 and short edge <= 4320 after scaling.
- Quality levels 1-2 should use 10-bit output (`yuv420p10le`) unless implementation
  testing proves it problematic; levels 3-5 can use `yuv420p`.
- New code should live under `src/video/upscale/`, not a top-level `src/video_upscale/`.

## Feature Scope

### MVP User Experience

Add a context-menu action for video items:

- `AI動画アップスケール...` or similar

The dialog should expose only:

- scale: `2x` or `4x`
- model preset: generic fast, anime, photo
- quality: 5 levels
- output resolution preview
- estimated processing time and output size
- output path preview
- overwrite behavior if a `.miv.mkv` already exists
- start/cancel

MVP should not expose codec internals such as CRF, preset, GOP, tiles, bitrate, pixel format,
or audio codec. Those values are internal presets. MVP must warn that exported video has
no audio; audio copy/remux belongs to Phase 2.5.

### Defaults

- Model presets:
  - generic fast: `ModelKind::UpscaleRealEsrGeneralV3`
  - anime: `ModelKind::UpscaleRealEsrganAnime6B`
  - photo: `ModelKind::UpscaleRealEsrganX4Plus` or `ModelKind::UpscaleNmkdSiax4x`
- Output container: Matroska, `.miv.mkv`
- Video codec: AV1 via `libsvtav1`
- Audio: none in MVP, with a clear warning before start
- Subtitles/chapters/metadata: copy when practical, otherwise preserve only essentials
- Default scale: `2x`
- Default quality: level 3
- Temp output: `<stem>.miv.mkv.part`
- Final output: `<stem>.miv.mkv`
- Metadata sidecar: `<stem>.miv.json`

Quality mapping proposal:

| Level | Label | SVT-AV1 CRF | SVT-AV1 preset |
| --- | --- | ---: | ---: |
| 1 | Highest quality | 20 | 7 |
| 2 | High quality | 24 | 8 |
| 3 | Standard | 28 | 8 |
| 4 | Smaller | 32 | 9 |
| 5 | Smallest | 36 | 9 |

Pixel format proposal:

- quality 1-2: `yuv420p10le`
- quality 3-5: `yuv420p`

Scale behavior:

- `4x`: use the model output directly.
- `2x`: run the 4x model, then high-quality downscale to 2x before encoding.

This keeps model choice simple while giving a meaningful disk/decode-load reduction.
The UI should note that 2x still has roughly the same AI inference cost as 4x.

Output size guard:

- allow output only when `max(width, height) <= 7680` and `min(width, height) <= 4320`
- reject larger outputs before starting the job

## Sidecar Metadata

The sidecar is the source of truth for matching an upscaled derivative to its original video.

Example:

```json
{
  "schema": 1,
  "source": {
    "file_name": "movie.mp4",
    "size": 123456789,
    "mtime_unix_ms": 1777561200000,
    "head_tail_sha256": "0123456789abcdef..."
  },
  "miv": {
    "version": "0.9.0"
  },
  "upscale": {
    "scale": 2,
    "model": "realesr_general_v3"
  },
  "encode": {
    "container": "mkv",
    "video_codec": "av1",
    "encoder": "libsvtav1",
    "quality_level": 3,
    "crf": 28,
    "preset": 8,
    "audio": "copy"
  },
  "output": {
    "path": "movie.miv.mkv",
    "width": 3840,
    "height": 2160
  }
}
```

The partial hash is SHA-256 of the first 1 MiB plus the last 1 MiB of the source file.
For files smaller than 2 MiB, hash the whole file once. Pairing should treat mtime as
informational and must remain valid when only mtime changes. `source.path` is intentionally
not part of schema v1; file name, size, and partial hash are sufficient for same-folder
derived file pairing.

## Folder Listing And Playback Behavior

When both `movie.mp4` and `movie.miv.mkv` exist:

- show only the original `movie.mp4` in the grid by default
- attach derived-file state to the original video item
- show a small "upscaled available" indicator in the info/hover UI
- fullscreen playback should prefer the upscaled derivative when the setting is enabled
- provide a manual toggle/open action to play the original

Derived file recognition should be conservative:

- require `.miv.mkv` plus valid `.miv.json`
- verify source file name, size, and `head_tail_sha256`
- if metadata is stale, show both files as normal videos or ignore the derivative pairing

This avoids accidentally hiding unrelated `.miv.mkv` files.

## Proposed Modules

New modules:

- `src/video/upscale/mod.rs`
- `src/video/upscale/job.rs`
- `src/video/upscale/sidecar.rs`
- `src/video/upscale/ffmpeg_encode.rs`
- `src/ui_dialogs/video_upscale.rs`

Likely touched existing files:

- `src/main.rs` / `src/lib.rs` module declarations
- `src/app.rs` for dialog state, pending job state, polling
- `src/ui_dialogs/mod.rs` for dialog module registration
- `src/ui_dialogs/context_menu.rs` for video context menu action
- `src/folder_tree.rs` or folder scan path to pair/hide derived files
- `src/grid_item.rs` if derived metadata needs to be attached to video items
- `src/video/mod.rs` / fullscreen open path to prefer derivative playback
- `src/settings.rs` for `prefer_upscaled_video_derivatives`
- docs and license files listed above

## Encoding Pipeline

MVP should use existing FFmpeg libraries, not an external `ffmpeg.exe`.

Pipeline:

1. Worker opens source via `avformat`.
2. Decode video frames in presentation order.
3. Convert to `DynamicImage`/RGB or RGBA-compatible image buffer.
4. Run existing AI upscale with the selected model preset.
5. If target scale is 2x, resize the 4x output down to 2x using existing high-quality resize helpers.
6. Convert to encoder pixel format: `yuv420p10le` for quality 1-2, `yuv420p` for quality 3-5.
7. Encode with `libsvtav1`.
8. MVP writes video only. Audio copy/remux is Phase 2.5.
9. Write to `.part`.
10. Flush encoder/muxer.
11. Write `.miv.json`.
12. Atomic rename `.part` to `.miv.mkv`.

Implementation note: audio copy and video re-encode have different timestamp domains. MVP avoids
that risk by writing video only behind an explicit warning; the target design should add audio
copy/remux later.

## Threading And Responsiveness

- All decode/upscale/encode work must run off the UI thread.
- Progress should be reported through an mpsc channel.
- Cancellation should use `Arc<AtomicBool>`.
- The worker must check cancellation between frames, between upscale tiles where possible, and
  before muxer finalization.
- Do not use `try_lock + sleep`.
- The UI should display frame count/progress when known, elapsed time, output path, and errors.
- The UI should display output resolution, estimated processing time, estimated output size,
  and an updating ETA while the job runs.
- There should be only one video upscale export job at a time in MVP.

## Risk Areas For Review

1. FFmpeg license notice currently says LGPLv2.1 in several places despite current DLLs being
   LGPLv3-or-later.
2. FFmpeg source distribution for BtbN builds may require more than an FFmpeg source tarball
   because external libraries are included.
3. `ffmpeg-the-third` Rust API may not expose every encoder/muxer operation ergonomically; some
   parts may need FFI calls.
4. Audio packet copy while video frames are re-timestamped can be error-prone.
5. Long-running AI inference plus video encoding needs cancellation and temp-file cleanup.
6. Hiding derived videos in the grid must be conservative to avoid hiding user files.
7. 4x output can become 8K for 1080p input, so 2x must remain available.

## Proposed Implementation Phases

### Phase 1: Compliance And Design Foundation

- Update FFmpeg license wording to LGPLv3-or-later.
- Add `docs/ffmpeg-lgpl-source-distribution.md`.
- Add `scripts/collect-ffmpeg-lgpl-info.ps1`.
- Add sidecar schema and tests.

### Phase 2: Export MVP

- Add video upscale dialog and job state.
- Decode frames, upscale using `realesr_general_v3`, encode AV1/MKV.
- Write `.part`, `.miv.mkv`, and `.miv.json`.
- Add progress/cancel UI.

### Phase 3: Derivative Pairing

- Detect valid `.miv.json` + `.miv.mkv`.
- Hide derived entry from grid when paired.
- Prefer derived playback when enabled.
- Add toggle/open-original path.

## Review Questions For ClaudeCode

1. Is LGPLv3-or-later the correct wording for the currently bundled BtbN FFmpeg DLLs?
2. Is the proposed source distribution doc/report sufficient as a release-process guard, or should
   implementation create a concrete source bundle manifest now?
3. Is `.miv.mkv` + `.miv.json` pairing conservative enough?
4. Should source validation include a partial hash in MVP?
5. Is `libsvtav1` + MKV the right first encoder/container combination?
6. Should audio copy be mandatory for MVP, or can initial implementation be video-only with clear UI?
7. Are the proposed module boundaries aligned with the existing video/AI architecture?
