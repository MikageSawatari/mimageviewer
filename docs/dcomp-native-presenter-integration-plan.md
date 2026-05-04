# DirectComposition Native Presenter Integration Plan

This document turns the successful `--dcomp-presenter-test` prototype into a
production integration plan for fullscreen video playback.

## Why

The egui/wgpu fullscreen path tops out below stable 1080p120 playback on the
test machine even when VST3 is skipped. The DirectComposition prototype reuses
the existing decoder and `GpuVideoDevice`, but presents decoded D3D11 frames via
a native HWND + DComp visual + flip-model DXGI swap chain. The 1080p120 smoke
run presented 360/360 frames in 3 seconds with no late drops and sub-millisecond
present work.

The production goal is to split frame-rate-critical video presentation from the
egui UI rate:

- video: native DComp/DXGI presenter at the source frame rate (120fps+)
- HUD, seek bar, panels, dialogs: egui, allowed to update at a lower UI cadence
- VST3 editor windows: existing cross-process HWND path, owned by the fullscreen
  parent HWND as today

## Target Shape

```text
Fullscreen top-level HWND
  DirectComposition target
    Visual 0: video swap chain (native presenter, D3D11/DXGI)
    Visual 1: egui overlay swap chain or transparent overlay HWND
  VST3 editor top-level owned popups (existing bridge windows)
```

The prototype currently validates only Visual 0. Production work must add input,
overlay, resize, DPI, and state-machine integration.

## Phase A: Reusable Native Presenter Module

Move prototype-only code from `src/dcomp_presenter_test.rs` into a reusable
Windows module, for example `src/video/native_presenter.rs`.

Required API sketch:

```rust
pub struct NativeVideoPresenter { ... }

pub struct NativePresenterConfig {
    pub hwnd: HWND,
    pub width: u32,
    pub height: u32,
    pub sync_interval: u32,
}

impl NativeVideoPresenter {
    pub fn new(config: NativePresenterConfig) -> Result<Self, NativePresenterError>;
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), NativePresenterError>;
    pub fn present(&mut self, frame: &VideoFrame) -> Result<PresentStats, NativePresenterError>;
}
```

Keep prototype CLI as the first caller, so the module remains testable before
the fullscreen viewer uses it.

Acceptance:

- `--dcomp-presenter-test` still reaches 1080p120 with no late drops.
- No behavior change in normal fullscreen playback.

Status:

- 2026-05-04: `NativeVideoPresenter` was extracted into
  `src/video/native_presenter.rs`. The prototype CLI remains the first caller,
  so the native presenter can be regression-tested with `--dcomp-presenter-test`
  while production fullscreen integration is still pending.

## Phase B: Fullscreen HWND Ownership

The current fullscreen path is an egui viewport. Native presentation needs the
native HWND that owns:

- the DComp video visual
- the egui overlay
- VST3 editor windows

Two possible approaches:

1. Keep eframe's fullscreen viewport HWND and attach the native presenter to it.
2. Create a dedicated Win32 fullscreen HWND and embed/overlay egui separately.

The prototype already validates approach 2, and it avoids possible conflicts
between an eframe/wgpu swap chain and a DComp target on the same HWND. Start
Phase B by evaluating approach 2 first. Approach 1 remains available only if
embedding into the eframe fullscreen viewport proves clearly simpler and does
not interfere with wgpu presentation.

Acceptance:

- Alt-tab, Escape close, multi-monitor placement, and DPI changes keep the same
  user-visible behavior as the current fullscreen path.
- VST3 owner switching still uses the fullscreen parent HWND and does not bring
  the thumbnail grid window forward.

Status:

- 2026-05-04: the prototype HWND wrapper was extracted into
  `src/video/native_window.rs` as `NativeVideoWindow`. It supports the existing
  windowed test mode plus a borderless `WS_POPUP` mode for the future fullscreen
  parent HWND. Its WndProc keeps `PostQuitMessage` configurable so production
  integration can destroy the native video HWND without accidentally terminating
  the eframe app loop. `NativeVideoPresenter::resize()` was also added as the
  swap-chain side of future `WM_SIZE` / DPI handling.
- 2026-05-04: an experimental production slice was added behind
  `MIV_NATIVE_VIDEO_PRESENTER=1`. It keeps the existing `VideoPlayer` decoder,
  audio, VST3, and clock paths, but clones the video frame receiver into a
  dedicated native presenter thread with its own borderless HWND. VST editor
  owner sync is guarded so the bridge receives `set_chain_owner` only when the
  native HWND changes.
- 2026-05-04: the opt-in slice now hides the legacy egui fullscreen viewport
  once the native borderless HWND exists, then raises the native HWND. The
  legacy viewport is still kept available for fallback before native HWND
  creation, but it must not cover the native DComp presenter after startup.
- 2026-05-04: production GPU frames now follow the decoder's keyed-mutex
  protocol (`ReleaseSync(1)` on the producer side, `AcquireSync(1)` /
  `ReleaseSync(0)` on the presenter side) before copying the shared texture.
  The video swap chain is resized to the source frame size and the video visual
  is aspect-fit to the native fullscreen HWND with a DirectComposition
  transform, so a 1080p clip can fill a 4K fullscreen window without coupling
  the video copy path to the window backbuffer size.

Current limitations of the experimental slice:

- Escape closes the native video HWND and then the existing fullscreen state.
  A minimal native key bridge forwards core video shortcuts (Enter, W,
  Left/Right seek, Shift+Up/Down volume, M/L/P/S/B) back to the UI thread, but
  full file navigation and overlay hit-testing remain Phase C work.
- Native mouse messages are now forwarded to the UI thread as Phase C
  scaffolding. Mouse movement only wakes future HUD state, and left-click
  toggles play/pause when VST3 GUI windows are not visible; full overlay
  hit-testing and seek-bar interaction remain Phase C work. The bridge already
  normalizes wheel coordinates to client space, includes Shift/Ctrl flags, and
  tracks mouse leave/capture so the later overlay hit-test can reuse the same
  event path.
- The native HWND can also handle the basic non-overlay fullscreen actions that
  do not need egui hit-testing: plain Up/Down navigates to adjacent items,
  Home/End jumps to the first/last navigable item, Space toggles the current
  checkmark, and a short right-click closes fullscreen.
- GPU frames are copied into a source-sized presenter swap chain after keyed
  mutex acquisition, then scaled by DirectComposition. 10-bit/HDR GPU frames
  still need a fallback or a dedicated presentation path.
- it remains opt-in via environment variable and the egui fullscreen path is
  still the default

## Phase C: Overlay Strategy

The video visual can present independently, but HUD and seek UI still need to
draw above it. Evaluate in this order:

1. Egui overlay as a second DComp visual backed by its own swap chain.
2. Egui overlay as a transparent child/top-level overlay HWND.
3. Minimal native HUD for the hottest controls, with egui panels shown only
   while interaction is active.

The second DComp visual is the preferred production direction because it keeps
video and overlay composition inside the same visual tree. Transparent overlay
HWND experiments are still useful as a fallback, but they carry more Z-order and
airspace risk.

Status:

- 2026-05-04: the native presenter can optionally create a second premultiplied
  DComp visual backed by its own DXGI composition swap chain when
  `MIV_NATIVE_VIDEO_TEST_OVERLAY=1` is set. The overlay currently draws only a
  static translucent test marker and is intentionally not wired to egui yet; its
  purpose is to verify DComp layering and alpha composition while keeping the
  120fps video visual independent.
- 2026-05-04: the next overlay technical choice is documented in
  `dcomp-overlay-egui-technical-brief.md`. The preferred spike is a wgpu DX12
  surface created from `SurfaceTargetUnsafe::CompositionVisual`, so egui-wgpu can
  render to the second DComp visual without introducing a transparent overlay
  HWND.
- 2026-05-04: the CompositionVisual/egui-wgpu spike is implemented behind
  `MIV_NATIVE_VIDEO_EGUI_OVERLAY=1`. It uses a standalone egui context and
  renderer to draw a tiny static label into the second DComp visual, with
  video-only fail-closed behavior if the overlay cannot initialize.
  The 1080p120 soak kept `late_drop=0` for 601 frames over 5 seconds with max
  interval 9.8ms, so the egui overlay surface can coexist with the native video
  visual without coupling redraw cadence.
- 2026-05-04: native key and mouse events are now translated to `egui::Event`s
  on the presenter thread before being forwarded to the existing UI-thread
  shortcut path. The overlay redraw path is dirty-driven, so input updates can
  refresh the HUD without tying the overlay to every video present.
- 2026-05-04: key release events and normalized line-scroll wheel events are now
  part of the native-to-egui bridge. Hit-test routing and DPI-aware coordinates
  remain Phase C production work.
- 2026-05-04: the overlay now derives `pixels_per_point` from
  `GetDpiForWindow`, sends it through egui's viewport input, and scales native
  mouse coordinates from physical pixels to egui points. Dynamic DPI-change
  handling is still a Phase E production gap.
- 2026-05-04: overlay input now feeds an egui hit-test routing decision back to
  the native presenter loop. When the egui overlay wants pointer or keyboard
  input, the matching native input batch is no longer forwarded to the legacy
  UI-thread fullscreen shortcut path; clicks outside the overlay still pass
  through to the existing native video shortcuts. The first production-shaped
  overlay HUD slice is a bottom seek/hover bar that reads playback position
  from the native clock and duration from a shared atomic updated on
  `InfoReceived`. Seek-bar click/drag now emits a native overlay seek command
  back to the UI thread so it uses the same `VideoPlayer::seek()` path as the
  legacy fullscreen HUD. Drag seek commands are coalesced to target changes of
  roughly 100ms or more to avoid flooding decoder seek state. While playback
  continues with the pointer resting over the HUD, the presenter ticks the
  overlay at roughly 250ms intervals so the time label and progress fill do not
  freeze.

The first production slice can accept a 60Hz overlay cadence as long as video
presentation remains independent at 120fps.

Acceptance:

- Hover bar, seek bar, metadata, and shortcuts keep working.
- UI overlay stalls do not block video present cadence.
- Click/focus behavior with visible VST3 editors remains fixed.

## Phase D: Frame Timing And Queues

The native presenter should own display timing. The decoder may continue to
produce future frames into the existing `VideoPlayer` queue, but presentation
must be based on:

- source PTS
- current audio/wall clock
- display refresh pacing from the native presenter

Avoid egui repaint scheduling as a video timing source.

Acceptance:

- 1080p120 synthetic sync video has no sustained display misses on a 165Hz
  monitor.
- 60fps AV1 files keep audio/video sync after resume, W seek-to-start, and
  repeated open/close.
- `video/display_miss` or a replacement native metric can still be graphed in
  the perf overlay.

## Phase E: Production Gaps From Prototype

Before enabling by default:

- complete dynamic `WM_SIZE` / monitor-change coverage around the source-sized
  video surface and DComp aspect-fit transform
- handle DPI and monitor changes
- support 10-bit/HDR GPU frames or fall back cleanly
- decide tearing policy (`sync_interval=0`) vs vsync policy (`sync_interval=1`)
- handle fullscreen close without the known `set_gui_owner` burst stalls
- keep CPU fallback path correct for software decoded frames
- add feature gate / setting for quick rollback

## Test Matrix

Use `scripts/video_soak.py` for A/B comparisons:

```powershell
python scripts/video_soak.py --exe target\release\mimageviewer-core.exe `
  --duration 10 --start 0 --skip-vst3 --window-size 1920x1080 `
  --mode egui-default `
  H:\home\mimageviewer_old\testimage\movie\test_120fps_1080p_sync.mp4

python scripts/video_soak.py --exe target\release\mimageviewer-core.exe `
  --duration 10 --start 0 --dcomp-presenter --window-size 1920x1080 `
  --mode dcomp `
  H:\home\mimageviewer_old\testimage\movie\test_120fps_1080p_sync.mp4
```

For the Phase C production overlay path, use
`docs/codex-native-overlay-redraw-cadence-brief.md`. The soak report includes
`overlay_present`, `overlay_max_render_ms`, and `overlay_max_interval_ms` from
`native_presenter/egui_overlay_present` events so overlay redraw cadence can be
checked separately from native video present cadence. The production native
fullscreen path also emits `native_presenter/summary` with the same core fields
as `--dcomp-presenter-test` (`presented`, `gpu_frames`, `cpu_frames`,
`late_drop`, `wait_timeout`, `actual_fps`, and max timing fields), so soak
status can key off the native presenter rather than the legacy egui fullscreen
viewport that still runs during the opt-in phase. The production path emits this
summary periodically as well as during orderly shutdown because play-test runs
can exit before the presenter thread's final shutdown log is flushed.

For production native presenter copy/fence spikes, use
`docs/codex-native-presenter-copy-spike-brief.md`. Setting
`MIV_NATIVE_VIDEO_PRESENT_TRACE=1` logs every `fullscreen_present` event so
`scripts/video_soak.py` can report `native_copy_p95_ms`,
`native_copy_max_ms`, `native_fence_max_ms`, shared handle cardinality, and
presenter shared-texture cache hits from real per-present samples. The
production decoder keeps a bounded D3D11 shared-output pool so NT shared handles
remain stable across frames; `OpenSharedResource1` should be limited to pool
warmup and source size/format changes. The presenter treats keyed-mutex read
ownership as a fail-fast check after the shared fence has completed, avoiding a
long presenter-thread stall if a pooled texture is not immediately readable.
The pool size tracks the existing video frame channel depth so startup does not
create more shared textures than the playback queue can reasonably hold.

Core clips:

- synthetic 1080p120 sync video
- strong-wind AV1 60fps file
- WMA Pro 5.1 WMV file
- old DivX/AVI file with missing PTS
- a normal H.264/AAC 30fps file

## Rollout

Keep the egui fullscreen path until the native path passes the test matrix.
Prefer an environment variable or hidden setting first:

```text
MIV_NATIVE_VIDEO_PRESENTER=1
```

After sustained testing, graduate it to a user setting or default path.
