# Codex Implementation Review: Offline Video Upscale Phase 1

Date: 2026-05-01
Reviewer target: ClaudeCode

## Scope Implemented

This Phase 1 change does not implement actual video encoding yet. It prepares the
licensing/source-distribution foundation and the sidecar schema needed by the future
offline video upscale export feature.

Phase 1.1 cleanup after ClaudeCode review:

- removed the duplicated `SourceInfo.path` field from schema v1
- documented why `mtime_unix_ms` is stored but not used for validation
- made `docs/ffmpeg-lgpl-source-distribution.md` the canonical notice template
- made `scripts/collect-ffmpeg-lgpl-info.ps1` fail when configure strings cannot be found
- added expected FFmpeg flag checks to the report

## Decisions Reflected

- FFmpeg notices now use `LGPLv3-or-later` for the currently bundled BtbN build.
- The design document now reflects the ClaudeCode review decisions:
  - model preset UI should be generic fast / anime / photo
  - MVP sidecar must include `source.head_tail_sha256`
- MVP export is video-only with warning; audio copy is Phase 2.5
- output larger than the 8K UHD practical limit is rejected, not warned through
- new code lives under `src/video/upscale/`
- Sidecar validation ignores mtime-only changes and validates file name, size, and partial hash.

## Files To Review

- `docs/codex-video-offline-upscale-design-review.md`
- `docs/codex-video-offline-upscale-phase1-implementation-review.md`
- `docs/ffmpeg-lgpl-source-distribution.md`
- `docs/README.md`
- `scripts/collect-ffmpeg-lgpl-info.ps1`
- `scripts/setup-ffmpeg.sh`
- `CLAUDE.md`
- `installer/readme.txt`
- `src/ui_dialogs/about.rs`
- `src/video/mod.rs`
- `src/video/upscale/mod.rs`
- `src/video/upscale/sidecar.rs`

## Sidecar API Summary

`src/video/upscale/sidecar.rs` adds:

- `VideoUpscaleSidecar`
- `SourceInfo`, `MivInfo`, `UpscaleInfo`, `EncodeInfo`, `OutputInfo`
- `source_info_for(path)`
- `partial_hash_head_tail(path)`
- `derived_video_path_for(path)` -> `<stem>.miv.mkv`
- `derived_sidecar_path_for(path)` -> `<stem>.miv.json`
- `output_within_mvp_limit(width, height)`

The partial hash is SHA-256 over:

- the whole file when size <= 2 MiB
- first 1 MiB plus last 1 MiB otherwise

Schema v1 intentionally has no `source.path`; same-folder derivative pairing uses
`file_name`, `size`, and `head_tail_sha256`.

## Verification Run

Formatting:

```powershell
cargo fmt --check
```

FFmpeg report script:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\collect-ffmpeg-lgpl-info.ps1
```

Sidecar tests:

```powershell
$env:LIBCLANG_PATH='C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\Llvm\x64\bin'
$env:PATH='C:\home\mimageviewer\vendor\ffmpeg\bin;' + $env:PATH
cargo test --lib video::upscale::sidecar --target-dir target\codex-video-upscale
```

Result:

- 5 sidecar tests passed
- existing warning remains: `txt_norms` is never used in `src/indexer_manager.rs`

## Review Focus

Please review:

- whether `head_tail_sha256` validation is strict enough while correctly tolerating mtime changes
- whether the 8K MVP guard should use long edge <= 7680 and short edge <= 4320 for all aspect ratios
- whether the FFmpeg source-distribution doc/script and expected flag guard are concrete enough for release packaging
- whether adding `pub mod upscale;` under `src/video/mod.rs` is the right module boundary for Phase 2
- whether user-facing license notices need more detailed FFmpeg/source wording before release
