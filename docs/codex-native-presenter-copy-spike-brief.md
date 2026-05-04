# Native Presenter Copy Spike Brief

## Context

The production DirectComposition presenter now displays real decoder output:
the egui fullscreen viewport is hidden behind the native HWND, D3D11 shared
textures are synchronized with the decoder's keyed mutex, and the source-sized
video swap chain is aspect-fit by a DComp transform. With the black-screen issue
resolved, `copy_ms` and `fence_wait_ms` measurements are meaningful again.

Earlier native-overlay soaks saw isolated fullscreen present spikes around
25-35ms in the copy/total path. This brief is for validating whether those are
startup noise, rare driver stalls, or a sustained production-path problem.

## Instrumentation

`scripts/video_soak.py` now reports native fullscreen-present timing from
`native_presenter/fullscreen_present` events:

- `native_present_samples`
- `native_copy_p95_ms`
- `native_copy_max_ms`
- `native_fence_max_ms`

By default the app logs slow presents and roughly one present per second. For a
full per-present distribution, run with:

```text
MIV_NATIVE_VIDEO_PRESENT_TRACE=1
```

The existing `MIV_NATIVE_VIDEO_PIXEL_PROBE=1` remains available when a run needs
to prove that source and backbuffer pixels are non-zero.

## Suggested Soak

Build release first:

```powershell
$env:LIBCLANG_PATH='C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\Llvm\x64\bin'
cargo build --release --bin mimageviewer-core
```

Run the production native presenter with full present tracing:

```powershell
python scripts\video_soak.py `
  --exe target\release\mimageviewer-core.exe `
  --duration 30 `
  --start 0 `
  --skip-vst3 `
  --window-size 1920x1080 `
  --out-dir video-soak-results\native-present-copy-trace `
  --mode native-trace:MIV_NATIVE_VIDEO_PRESENTER=1,MIV_NATIVE_VIDEO_PRESENT_TRACE=1 `
  H:\home\mimageviewer_old\testimage\movie\test_120fps_1080p_sync.mp4
```

Optionally repeat with the egui overlay enabled:

```powershell
--mode native-overlay-trace:MIV_NATIVE_VIDEO_PRESENTER=1,MIV_NATIVE_VIDEO_EGUI_OVERLAY=1,MIV_NATIVE_VIDEO_PRESENT_TRACE=1
```

## Acceptance

- `status` stays `OK`.
- `native_late_drop` remains `0` in raw `native_presenter/summary` events.
- `native_present_samples` is close to the native present count for traced runs.
- `native_copy_p95_ms` stays comfortably below the 120fps frame budget.
- Any `native_copy_max_ms` spike above the frame budget is inspected in the raw
  JSONL and classified as startup-only, rare isolated, or sustained.
- If a copy spike coincides with a high `native_fence_max_ms`, treat it as a
  producer/fence wait issue before investigating the copy itself.

## Follow-Up

If copy spikes are rare and do not produce native late drops, continue Phase D
timing work. If spikes are sustained or correlate with late drops, pause HUD
feature work and compare the production path against `--dcomp-presenter-test`
with the same clip and duration.
