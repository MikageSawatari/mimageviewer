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

**注**: v0.9.0+ 後期の CP1-8 で **「transparent overlay HWND」を選択** して実装済み
(= egui overlay は presenter HWND の DComp tree ではなく独立 top-level HWND `HudOverlayWindow`
にぶら下げる)。これは VST GUI が presenter HWND の owned + TOPMOST になっているため、
presenter HWND 内の DComp visual に描画した overlay が VST の裏に潜る問題を解消するため。
詳細は [video-architecture.md](video-architecture.md) の "HUD overlay HWND" 節と
[vst3-integration.md](vst3-integration.md) の "HUD overlay HWND と VST 前後関係" 節を参照。

```text
Fullscreen top-level HWND (= presenter)        HUD overlay HWND (= 独立 top-level)
  DirectComposition target                       DirectComposition target
    Visual 0: video swap chain (D3D11/DXGI)        Visual 0: egui overlay swap chain
    Visual 1: background visual                  owner = presenter, sibling of VST3
                                                 WS_EX_TOPMOST | NOACTIVATE
                                                 SetWindowRgn(実 UI rect only)
                                                 activation zone は region 外
                                                 (= 上下端の VST 入力を奪わない)
                                                                ↑
VST3 editor top-level owned (existing bridge   ←── 同じ owner = presenter の sibling
windows): owner = presenter, WS_EX_TOPMOST
```

最終 z-order (上から):
- HUD overlay HWND (= bars / interactive UI / hover thumbnail)
- VST GUI HWND
- Fullscreen presenter HWND (= video frame + background)

**activation zone (= 画面上下端の hover 検出帯) は region に含めない** — 含めると bar 非表示
時に VST のノブやメニューが上下端と重なったとき入力を奪う。代わりに presenter thread の
50ms 周期 `GetCursorPos` polling で synthetic pointer を `NativeEguiOverlay::push_native_event`
に流して hover 表示遷移を成立させる。同じ polling で activation zone 検知時に HUD raise
burst もエンキューする (= VST 手動クリックで HUD が裏に回ったあとの復帰経路)。

The prototype originally validated only Visual 0. Production work added input,
overlay, resize, DPI, and state-machine integration through CP1-8. フォールバック経路
として、HUD HWND 生成失敗時 / 環境変数 `MIV_HUD_OVERLAY=0` のときは egui overlay を
presenter HWND の DComp tree に attach する旧経路に戻る (= CP8 以前と等価)。

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
  native HWND changes. As of 2026-05-05, this path is default-on for Windows
  trial use; set `MIV_NATIVE_VIDEO_PRESENTER=0` to force the legacy egui
  fullscreen path.
- 2026-05-04: the native slice now hides the legacy egui fullscreen viewport
  once the native borderless HWND exists, then raises the native HWND. The
  legacy viewport is still kept available for fallback before native HWND
  creation, but it must not cover the native DComp presenter after startup.
- 2026-05-05: the native fullscreen HWND is created as an owned popup of the
  main mIV HWND when that owner is available. The presenter thread raises the
  native HWND when it creates the window; the egui UI thread does not issue
  periodic `SetWindowPos` calls against that cross-thread HWND. This avoids
  UI-thread hangs in Win32/DWM z-order mutation while HUD seek and overlay input
  are active, and relies on the owner relationship plus the black fullscreen
  backdrop to keep the thumbnail grid hidden during startup.
- 2026-05-05: while the native presenter thread is starting, before its first
  native frame has presented, or before the UI thread has raised the native HWND
  at least once, the legacy egui fullscreen viewport is shown as an opaque black
  startup curtain instead of drawing thumbnails, placeholder text, or a
  transparent window. This target check is based on the `GridItem::Video`
  itself, not on whether `VideoPlayer` has already populated `fs_cache`, so the
  legacy transparent fullscreen path is skipped even during the first
  cache-miss frames after a double-click. After the first native present
  succeeds and the native HWND has been raised, that viewport stays visible as
  the same opaque black backdrop behind the native HWND instead of being hidden. Avoiding
  `Visible(false)` during active native playback prevents the fullscreen
  viewport's hide/activation transition from briefly revealing the thumbnail
  grid if Windows reorders regular HWND composition for one frame. The backdrop
  is hidden only by the normal fullscreen-exit path. During startup the UI
  thread may raise the black egui backdrop above the main HWND by matching its
  monitor-sized window, stopping the main caption from sitting over the startup
  curtain while the presenter HWND is still hidden. The main viewport itself
  does not repaint a black panel and its DWM caption/border are not recolored
  during native-video fullscreen; if Windows transiently exposes the main HWND,
  letting the eframe render pass fall back to the normal theme fill is less
  visually disruptive than a black client area with the normal title bar on top.
  This avoids non-client
  title-bar/border flashes without toggling `ViewportCommand::Decorations`,
  which can resize the main client area and disturb the thumbnail grid when
  fullscreen exits.
- 2026-05-13: the native fullscreen presenter HWND is now created hidden and
  shown only after the initial DirectComposition tree and black backbuffer have
  been committed and flushed. This removes the short `WS_EX_NOREDIRECTIONBITMAP`
  transparent-HWND window where the blackened main HWND's DWM caption buttons
  could show through before the presenter had real content.
- 2026-05-13: native-video fullscreen now disables DWM window transitions on the
  black egui fullscreen backdrop, the native presenter HWND, and the HUD overlay
  HWND before those windows are shown. Foreground reclaim after closing native
  video is also decoupled from the delayed main-chrome restore, so the main HWND
  is claimed as soon as the presenter HWND is destroyed instead of waiting for
  the 80ms chrome settling window.
- 2026-05-13: the main HWND caption/border recolor during native-video
  fullscreen is disabled again. The black backdrop/presenter now masks the
  client area without forcing DWM to recalculate title-bar glyph contrast, which
  avoids a visible light/dark caption flip just before the black viewport
  appears.
- 2026-05-13: while the native presenter HWND is still hidden, the UI thread now
  enumerates its own top-level HWNDs, finds the visible fullscreen-sized black
  egui backdrop, and raises it above the main HWND. This keeps the main caption
  from momentarily sitting above the black startup curtain before native video
  starts.
- 2026-05-13: the main viewport no longer draws a black `CentralPanel` during
  native-video fullscreen. That fallback masked thumbnails, but when the main
  HWND leaked above the fullscreen backdrop it produced a high-contrast "black
  client plus normal title bar" frame. The dedicated fullscreen backdrop remains
  responsible for the black curtain.
- 2026-05-13: the native-video black egui fullscreen backdrop is created hidden
  on its first frame. After the HWND exists, the UI thread applies
  `DWMWA_TRANSITIONS_FORCEDISABLED` and only then sends `Visible(true)`, so the
  first show uses the no-transition path instead of Windows' default zoom/fade.
- 2026-05-05: the Rust panic hook is complemented by a Windows native exception
  handler and a UI heartbeat watchdog. Native access violations are appended to
  `panic.log`, while an `App::update` heartbeat gap of more than five seconds is
  logged as a suspected UI hang so AppHang reports have a local breadcrumb even
  when no Rust panic occurs.
- 2026-05-05: while a native presenter HWND is active, the UI thread keeps a
  lightweight 16ms repaint pump alive even though video frames are presented on
  the native thread. This prevents native-window shortcut events such as Escape
  from sitting in the UI event queue when the hidden egui fullscreen viewport
  has no other reason to repaint.
- 2026-05-04: production GPU frames initially followed the decoder's keyed-mutex
  protocol (`ReleaseSync(1)` on the producer side, `AcquireSync(1)` /
  `ReleaseSync(0)` on the presenter side) before copying the shared texture.
  On 2026-05-05 Phase D found that D3D12/wgpu consumers can be fence-only, but
  the native D3D11 presenter must still acquire key=1 before copying or the
  shared texture can read back as black on production hardware. The producer
  also recovers key=1 pooled slots before reuse so non-D3D11 consumers and
  discarded unpresented frames do not leave the pool locked. Unpresented frame
  cleanup must not call `AcquireSync(1)` directly; driver timeouts are not
  reliable enough for seek/close/drop paths. The video swap chain is
  resized to the source frame size and the video visual is aspect-fit to the
  native fullscreen HWND with a DirectComposition transform, so a 1080p clip can
  fill a 4K fullscreen window without coupling the video copy path to the window
  backbuffer size.
- 2026-05-05: the fullscreen DComp tree now keeps an opaque black background
  visual behind the aspect-fit video visual. Letterbox/pillarbox regions are
  therefore filled by the native presenter instead of showing the desktop behind
  the borderless HWND.
- 2026-05-05: the native presenter's software-decoded fallback keeps the
  decoder's shared CPU frame contract as RGBA8, then converts to BGRA only at
  the DComp swap-chain upload boundary. This preserves the legacy egui/wgpu CPU
  path while matching the presenter's `B8G8R8A8_UNORM` video surface.
  `--dcomp-presenter-test --dcomp-force-sw --dcomp-pixel-probe-strict` now
  exercises that fallback by comparing the CPU RGBA source pixel with the
  D3D11 BGRA backbuffer pixel; `scripts/video_color_probe.py` wraps the check
  for repeatable smoke runs without screenshot capture.

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
- Native egui overlay parity is in progress: edge-hover top/left/right chrome
  is now synchronized, the left jump panel remains visible in the empty state,
  wheel navigation and S-mode column adjustment are routed through overlay
  commands, and the native overlay installs Japanese-capable fonts independently
  from the main egui context.
- Native egui overlay parity now also covers the legacy pause/loading/error
  affordances and feedback paths: centered pause controls, "preparing video" and
  playback error status, native feedback toasts, boundary hints, Shift+Enter
  external-player launch, J/K marker jumps, and a top-bar VST3 GUI toggle.
- Left/right native side-panel entry zones use the same x range as the visible
  panel bodies, excluding the bottom seek zone. Showing the side/top panel
  chrome also keeps the bottom seek HUD visible so the fullscreen HUD does not
  leave an empty lower band; seek-bar hover alone still keeps the side/top
  panels hidden.
- S-mode video-to-video navigation keeps a native tile preparing overlay active
  across the handoff, so the next video cannot briefly show at normal brightness
  before its tile thumbnails arrive.
- When S-mode navigation is restoring the tile view, the new native presenter
  is created with an initial opaque tile curtain and renders it before the first
  video present. This prevents the first decoded frame or the underlying
  thumbnail grid from winning the frame race before UI-thread tile commands
  arrive.
- The seek/jump thumbnail worker has no fixed entry-count cap; generated
  thumbnails are retained for the lifetime of the current `VideoPlayer` so
  unusually large chapter lists can continue filling instead of cycling older
  thumbnails out of cache.
- S-mode's native tile curtain is opaque while preparing or navigating between
  videos, and top-bar controls now use icon-style S/P buttons plus a wider
  `VST` toggle that resynchronizes plugin GUI ownership/topmost state to the
  native fullscreen HWND.
- GPU frames are copied into a source-sized presenter swap chain after keyed
  mutex acquisition, then scaled by DirectComposition. HDR display output is
  intentionally unsupported; high-bit-depth inputs are converted by the D3D11
  video processor into the same SDR BGRA8 display texture used for normal
  8-bit sources.
- 2026-05-05: the production native presenter is now the default Windows
  fullscreen video path for trial use. Set `MIV_NATIVE_VIDEO_PRESENTER=0` to
  return to the legacy egui fullscreen presenter. The egui DComp overlay HUD is
  also default-on after the attach/detach visual gate was verified on the
  affected production machine; set `MIV_NATIVE_VIDEO_EGUI_OVERLAY=0` to run the
  native presenter without the HUD overlay.

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
- 2026-05-04: the CompositionVisual/egui-wgpu spike was implemented behind
  `MIV_NATIVE_VIDEO_EGUI_OVERLAY=1`. It uses a standalone egui context and
  renderer to draw into the second DComp visual, with video-only fail-closed
  behavior if the overlay cannot initialize. It was briefly returned to opt-in
  during the 2026-05-05 trial because an empty overlay surface could appear as
  an opaque black visual on at least one production setup.
- 2026-05-05: the blank-overlay failure mode is mitigated by keeping the egui
  overlay visual detached from the root DComp visual tree while the HUD is
  hidden. The presenter renders the HUD into the wgpu CompositionVisual surface
  first, then attaches the visual above the video visual; on mouse leave or any
  hidden-HUD redraw it removes the visual again. This avoids letting an empty
  or transparent-cleared surface cover the video if the underlying DComp/wgpu
  path treats it as opaque. Manual trial on the affected machine confirmed that
  visible HUD pixels blend correctly, so the env flag is default-on again with
  `MIV_NATIVE_VIDEO_EGUI_OVERLAY=0` kept as the overlay rollback.
- 2026-05-05: native-presenter `FirstFrameReady` delivery now retries when the
  engine event channel is temporarily full. HUD seek bursts can coincide with
  stale-packet and audio events; if the first presented frame notification is
  dropped, the engine can remain in `Buffering` even though the native presenter
  has already displayed a post-seek frame. The presenter only marks the epoch's
  first-frame event delivered after `try_send` succeeds, and discards pending
  retries when a newer seek serial appears.
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
- 2026-05-05: the native egui overlay HUD now carries the first legacy-control
  follow-up slice on top of the seek bar: seek-to-start/play, play/pause,
  add-bookmark, mute, and volume controls. The overlay still emits commands
  from the presenter thread and lets the UI thread call the existing
  `VideoPlayer` and video bookmark DB paths, so playback state changes,
  click/drag volume persistence, and bookmark writes stay on the same side of
  the thread boundary as the legacy fullscreen HUD. Thumbnail preview, bookmark
  pinning, and side panels remain staged Phase C production work.
- 2026-05-05: the native overlay gained an early P-key perf graph so native
  fullscreen A/B checks do not have to wait for the full legacy HUD parity
  pass. The graph is fed by presenter-thread present samples and the existing
  native summary counters, draws a compact 6-second interval/total/copy trace,
  and is throttled by the same dirty-driven overlay path instead of presenting
  a HUD frame for every video frame.
- 2026-05-05: bookmark and pin actions were moved toward the legacy seek-hover
  model. The bottom transport strip no longer owns a standalone bookmark
  button; hovering the seek bar now keeps a preview target alive, asks the UI
  thread to warm the seek-thumbnail cache, and shows bookmark/pin actions on
  that preview target. Actual thumbnail pixels are still a follow-up bridge
  from the UI/thumbnail cache into the native overlay texture path.
- 2026-05-05: the seek-hover preview now receives real thumbnail pixels from
  the UI thread. The native output owns a small UI-to-presenter command channel;
  when the existing `ThumbnailWorker` cache has a nearby RGBA frame, the UI
  sends an `Arc<Vec<u8>>` clone to the presenter thread, which uploads it as an
  egui texture and draws it aspect-fit in the preview. The perf graph history
  cap was also raised so 120fps runs fill the intended 6-second window instead
  of only the right edge.

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

Before making the trial default permanent:

- complete dynamic `WM_SIZE` / monitor-change coverage around the source-sized
  video surface and DComp aspect-fit transform
- handle DPI and monitor changes
- keep 10-bit/HDR sources on the GPU path by converting them to SDR BGRA8
  display textures; true HDR output remains out of scope
- decide tearing policy (`sync_interval=0`) vs vsync policy (`sync_interval=1`)
- handle fullscreen close without the known `set_gui_owner` burst stalls
- keep CPU fallback path correct for software decoded frames
- replace the temporary environment rollback with a user-facing setting if the
  native path remains default

## Test Matrix

Use `scripts/video_soak.py` for A/B comparisons:

```powershell
python scripts/video_soak.py --exe target\release\mimageviewer-core.exe `
  --duration 10 --start 0 --skip-vst3 --window-size 1920x1080 `
  --mode egui-default:MIV_NATIVE_VIDEO_PRESENTER=0 `
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
viewport. The production path emits this summary periodically as well as during
orderly shutdown because play-test runs can exit before the presenter thread's
final shutdown log is flushed.

For production native presenter copy/fence spikes, use
`docs/codex-native-presenter-copy-spike-brief.md`. Setting
`MIV_NATIVE_VIDEO_PRESENT_TRACE=1` logs every `fullscreen_present` event so
`scripts/video_soak.py` can report `native_copy_p95_ms`,
`native_copy_max_ms`, `native_fence_max_ms`, shared handle cardinality, and
presenter shared-texture cache hits from real per-present samples. The
production decoder keeps a bounded D3D11 shared-output pool so NT shared handles
remain stable across frames; `OpenSharedResource1` should be limited to pool
warmup and source size/format changes. A 2026-05-05 Phase D trace showed that
consumer-side `IDXGIKeyedMutex::AcquireSync(1)` can block the native presenter
thread for 25-35ms even with a zero timeout, while the shared fence, shared
texture cache, and copy call remain sub-millisecond. Follow-up validation showed
that the native D3D11 presenter still has to acquire key=1 before copying; using
only the fence made the source texture read as black. Treat this as a D3D11
shared-resource visibility/cache-flush requirement on the tested driver, not
just keyed-mutex ownership bookkeeping. D3D12/wgpu consumers remain fence-only,
and the producer tracks slots released to readers so it can recover
key=1 back to key=0 immediately before reusing a pooled output slot. Trace runs
should keep keyed mutex acquire time small in steady state and
`native_recover_max_ms` below 1ms.
Unpresented frame drains only release the pool slot and leave
`released_to_reader=true` for the next producer reuse. Do not move
`AcquireSync(1)` back into `reset_unpresented_shared_output`; a 2026-05-05 live
hang showed the native presenter thread blocked inside NVIDIA's D3D11 keyed
mutex acquire while draining seek-era frames.
Trace events split `keyed_mutex_ms` into `keyed_mutex_cast_ms` and
`keyed_mutex_acquire_ms` so Phase D can distinguish COM interface lookup from
the `AcquireSync(1)` wait.
The pool size tracks the existing video frame channel depth so startup does not
create more shared textures than the playback queue can reasonably hold.
Because the same `GpuVideoDevice` is reused across clips, the pool may contain
idle textures for earlier source sizes. When a new size arrives and the bounded
pool is full, the producer evicts an idle slot from a different size/format
before falling back to CPU readback; otherwise a sequence of mixed-resolution
clips can exhaust the pool with unusable old slots.
The native presenter drains due frames in source PTS order rather than replacing
all due frames with the newest one; this prevents startup backlog from dropping
unpresented keyed-mutex frames and briefly forcing the decoder into CPU
readback fallback. When `MIV_NATIVE_VIDEO_PRESENT_TRACE=1` is enabled, the
`source_delta_ms` field on `fullscreen_present` helps confirm that 120fps
content advances at roughly 8.33ms per presented frame.
Phase E presentation timing traces split swap-chain throttling from the DXGI
present call itself: `present_waitable_ms` records the frame-latency waitable
object wait before copy, while `present_call_ms` records the
`IDXGISwapChain::Present` call. A spike in the latter points to vsync / DComp /
DWM present policy rather than decoder, copy, or keyed-mutex ownership.
The legacy egui/wgpu rollback path must also reset discarded pooled GPU frames
before replacing `gpu_latest` or draining seek-era queues, because D3D12 import
does not release the D3D11 keyed mutex on behalf of the producer.
Native seek-hover thumbnails are updated through a UI-to-presenter command
channel. While a hover preview target is active, the overlay performs a
low-rate dirty render even when the video clock is idle so completed thumbnail
work can replace the `loading` placeholder without a pointer move. The preview
keeps bookmark/pin actions and the time label in a black action bar below the
image to avoid burying controls in thumbnail colors.
`VideoPlayer` also stores the latest native hover target and pumps completed
thumbnail-worker cache entries to the presenter from its UI-thread tick, so
worker completion does not depend on a second native pointer event. Pin active
state remains UI-owned and is sent to the presenter as a small overlay state
command after DB lookup or toggle.
The UI thread now also sends lightweight timeline markers for pin/bookmark/
chapter positions. The presenter draws these over the native seek bar and uses
the bookmark markers to make the seek-hover bookmark icon show an active state
near existing bookmarks.
A first native left-edge jump panel consumes the same marker list and exposes a
compact PIN/BM/CH time list with click-to-seek rows. It intentionally omits
thumbnail rows and edit/delete actions until the full left/right panel parity
slice.
The left panel has its own hover lifetime separate from the bottom seek HUD:
the full visible panel width opens and retains it, while the bottom seek-bar
zone remains excluded so seek hover does not accidentally open the side panel.
This mirrors the legacy fullscreen side-panel behavior and keeps rows clickable
when the cursor leaves the seek-bar hover region.
Marker synchronization is now requested on left-edge mouse movement, so the
panel can initialize before any seek-hover thumbnail has been opened.

The native overlay now also has first-pass top and right hover panels. The
right panel receives a compact metadata snapshot from the UI thread (file/title,
codec, decoder, audio, bitrate, duration, and chapter count), while the top bar
shows title and playback context. These hover zones are independent from the
bottom seek HUD and follow the legacy direction-based panel model.
The right metadata panel now wraps and scrolls long metadata values, and wheel
input over either side panel scrolls that panel instead of navigating to another
video. The top bar, side panels, and perf graph use separate rectangles so the
chrome can be shown together without text or panels covering each other.

S-key video tile mode is now represented in the native overlay. The existing
UI-owned `VideoTileState` and thumbnail worker remain the source of truth; the
UI thread periodically sends tile progress and decoded RGBA thumbnails to the
presenter. When tile-mode navigation lands on another video, the native overlay
keeps a black preparing screen visible until the new video's tile state can be
reopened, reducing flicker back to the thumbnail grid/backdrop.
Native presentation drops stale due frames before presenting when the presenter
thread has fallen behind the audio clock, and pause/resume clears the native
source-pacing baseline. This keeps overloaded CPU fallback playback closer to
audio time; dropped video frames are preferred to sustained A/V drift.

Core clips:

- synthetic 1080p120 sync video
- strong-wind AV1 60fps file
- WMA Pro 5.1 WMV file
- old DivX/AVI file with missing PTS
- a normal H.264/AAC 30fps file

## Rollout

The native presenter is the default Windows fullscreen video path for trial use.
Keep the egui fullscreen path available as a rollback path until the native path
passes the full test matrix:

```text
MIV_NATIVE_VIDEO_PRESENTER=0
```

If sustained testing is clean, replace the environment rollback with a
user-facing setting or remove it when the legacy path is retired.
