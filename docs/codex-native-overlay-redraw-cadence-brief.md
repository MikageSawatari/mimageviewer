# Native Overlay Redraw Cadence Brief

## Context

Phase C now has the native DComp video presenter, an egui/wgpu overlay visual,
hit-test routing, and a first production-shaped seek HUD slice. The next
validation step is to prove that the overlay redraw cadence stays independent
from 1080p120 video presentation during idle HUD ticks and drag scrubbing.

## Instrumentation

`native_presenter/egui_overlay_present` perf events already include:

- `input_events`
- `native_events`
- `render_ms`
- `wants_pointer`
- `wants_keyboard`

`scripts/video_soak.py` now summarizes those events in `report.md`:

- `overlay_present`: number of egui overlay presents
- `overlay_max_render_ms`: slowest overlay render in the run
- `overlay_max_interval_ms`: largest interval between overlay presents

The raw JSONL remains the source of truth when investigating a specific spike.
The production native fullscreen path also emits `native_presenter/summary`, so
`scripts/video_soak.py` can distinguish native-presenter cadence from legacy
egui fullscreen viewport hitches while the old path still coexists. Production
summary events are emitted periodically as well as during orderly shutdown, so
short play-test runs still leave native-presenter status in the raw log.

## Suggested Soak

Build release first:

```powershell
$env:LIBCLANG_PATH='C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\Llvm\x64\bin'
cargo build --release --bin mimageviewer-core
```

Run the native production path with the egui overlay enabled:

```powershell
python scripts\video_soak.py `
  --exe target\release\mimageviewer-core.exe `
  --duration 30 `
  --start 0 `
  --skip-vst3 `
  --window-size 1920x1080 `
  --out-dir video-soak-results\native-overlay-cadence `
  --mode native-overlay:MIV_NATIVE_VIDEO_PRESENTER=1,MIV_NATIVE_VIDEO_EGUI_OVERLAY=1 `
  H:\home\mimageviewer_old\testimage\movie\test_120fps_1080p_sync.mp4
```

During one manual pass, keep the pointer over the bottom HUD strip for several
seconds to exercise the 250ms visible-HUD tick. During another pass, drag the
seek bar for several seconds to exercise 100ms target coalescing.
This soak requires manual interaction during the run window; a headless run can
only verify startup stability and HUD-hidden quietness.

## Acceptance

- `status` stays `OK`.
- `native_late_drop` remains `0` in the raw log or `frame_gap` remains `0` in
  the native-presenter report row.
- `overlay_present` increases while the HUD is visible or being dragged, but is
  far below the video present count during idle playback.
- `overlay_max_render_ms` stays comfortably below the video frame budget; any
  spike above 8ms should be inspected in the raw JSONL.
- `overlay_max_interval_ms` is expected to be near 250ms during stationary
  visible-HUD playback. It can be lower while pointer input or drag scrubbing is
  active.
  Note: overlay events do not fire while the HUD is hidden, so a long
  HUD-hidden gap can inflate `overlay_max_interval_ms`. Interpret this metric
  only across spans where the HUD remained visible.

## Follow-Up

If the measured overlay cadence is stable, continue Phase C by adding the next
production HUD widget on the same `NativeOverlayCommand` path. If overlay render
spikes coincide with native video late drops, keep feature work paused and
investigate the raw `egui_overlay_present` events first.
