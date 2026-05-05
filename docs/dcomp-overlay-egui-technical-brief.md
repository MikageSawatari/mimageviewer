# DComp Overlay Egui Technical Brief

This note records the Phase C decision point for drawing mIV fullscreen HUD UI
above the native DirectComposition video presenter.

## Current Facts

- `NativeVideoPresenter` already owns a DComp target with a root visual.
- The video layer is an opaque DXGI composition swap chain.
- `MIV_NATIVE_VIDEO_TEST_OVERLAY=1` verifies that a second premultiplied-alpha
  DXGI composition swap chain can be added above the video visual without
  disturbing 1080p120 present cadence.
- The static overlay test is D3D11-only and is not connected to egui yet.

## Best Next Candidate

Use `wgpu::SurfaceTargetUnsafe::CompositionVisual` to let wgpu create and manage
a surface directly from a second `IDCompositionVisual`.

Why this is the first candidate:

- It keeps video and overlay inside one DComp visual tree.
- It avoids transparent child HWND airspace and Z-order problems.
- It lets `egui_wgpu::Renderer` render normally to a wgpu surface texture.
- wgpu 27 exposes this target on the DX12 backend, and mIV already prefers DX12
  for the egui/wgpu path.

Target shape:

```text
Native fullscreen HWND
  DComp target
    root visual
      video visual   -> D3D11/DXGI swap chain, 120fps source cadence
      overlay visual -> wgpu DX12 surface from IDCompositionVisual, 30-60fps UI cadence
```

## Spike Plan

Start with an isolated overlay spike before wiring the production HUD:

1. Create a second `IDCompositionVisual` with no manual DXGI swap chain.
2. Create a `wgpu::Surface` from that visual using
   `SurfaceTargetUnsafe::CompositionVisual`.
3. Configure the surface with `Bgra8Unorm` or another DComp-compatible alpha
   format if supported.
4. Render a tiny egui UI using a standalone `egui::Context` and
   `egui_wgpu::Renderer`.
5. Present the overlay at 60Hz or only when dirty, while the native video
   presenter keeps presenting at the source frame rate.
6. Run the existing 1080p120 soak with the overlay spike enabled and require
   `late_drop=0`.

Status:

- 2026-05-04: the first spike is implemented behind
  `MIV_NATIVE_VIDEO_EGUI_OVERLAY=1`. It creates a wgpu DX12 surface from a
  second `IDCompositionVisual`, renders a tiny standalone egui label with
  `egui_wgpu::Renderer`, and fails closed to video-only presentation if the
  CompositionVisual surface path is unavailable.
- 2026-05-04: the 1080p120 soak with the spike enabled presented 601 frames in
  5 seconds with `late_drop=0`, max interval 9.8ms, and one static egui overlay
  render (`shapes=4`, `paint_jobs=1`). This proves the CompositionVisual route
  can draw egui primitives above video without tying overlay redraws to video
  cadence.
- 2026-05-04: native HWND input is now tee'd through the presenter thread before
  being forwarded to the UI thread. The egui overlay translates the same key and
  mouse events into `egui::Event`s and redraws only when dirty, which is the
  first production-shaped input/render loop for the overlay path.
- 2026-05-04: the input bridge now also forwards key releases and normalizes
  Win32 wheel deltas as egui line-scroll units, so future overlay widgets can
  rely on pressed/released key state and standard scroll scaling.
- 2026-05-04: the overlay now seeds egui with `GetDpiForWindow`-based
  `native_pixels_per_point` and converts Win32 physical mouse coordinates into
  egui points. This keeps future HUD hit-testing aligned on high-DPI monitors.
- 2026-05-04: hit-test routing is now wired through `egui_ctx` after each
  overlay frame. `wants_pointer_input()` gates mouse forwarding and
  `wants_keyboard_input()` gates key forwarding to the legacy native fullscreen
  shortcut path, so overlay widgets can consume their own input while video-area
  clicks keep using the existing shortcuts. A first bottom seek/hover bar has
  replaced the static spike label as the production-shaped HUD slice. Its
  click/drag seek action is sent back to the UI thread as an overlay command;
  drag seek commands are coalesced to target changes of roughly 100ms or more,
  and the presenter gives the visible HUD a 250ms playback tick so progress
  remains live even when the pointer is stationary.
- 2026-05-05: the egui overlay was briefly default-on with the native presenter
  for Windows fullscreen video trial use, then temporarily returned to opt-in
  while an opaque-black blank overlay visual was investigated on production
  machines.
- 2026-05-05: blank overlay frames no longer rely solely on transparent
  surface composition. The egui overlay DComp visual is detached from the root
  visual tree while the HUD is hidden and reattached only after a visible HUD
  frame has been rendered, so a driver path that treats the blank wgpu surface
  as opaque cannot cover the video. Manual trial on the affected machine
  confirmed the HUD blends correctly, so `MIV_NATIVE_VIDEO_EGUI_OVERLAY` is
  default-on again; set it to `0` to run the native presenter without the HUD
  overlay.
- 2026-05-05: native `FirstFrameReady` delivery is retried if the engine event
  channel is temporarily full. This protects HUD seek bursts from leaving the
  engine in `Buffering` after the native presenter has already displayed the
  first post-seek frame; pending retries are cleared when a newer seek epoch
  arrives.
- 2026-05-05: the production-shaped HUD slice now extends beyond seeking with
  native egui controls for seek-to-start/play, play/pause, loop, add-bookmark,
  mute, and volume. These controls are still command-only on the presenter thread;
  the UI thread executes them through the existing `VideoPlayer` and video
  bookmark DB paths, with volume saved on click/drag commit instead of every
  drag tick. This keeps the overlay path responsive while the later thumbnail
  hover preview, pinned bookmarks, and left/right panels are rebuilt in smaller
  slices.
- 2026-05-05: the overlay now has an early native perf graph toggled by the
  existing P shortcut. It shows presenter-thread FPS, drop/timeout counters,
  and a 6-second graph of present interval, total presenter cost, and copy cost.
  The samples are collected on every native present, but visible redraws are
  dirtied at a coarse cadence so the graph can validate native benefits without
  coupling the overlay to the video frame rate.
- 2026-05-05: seek-hover preview scaffolding now owns bookmark and pin actions,
  matching the legacy HUD placement more closely than a standalone transport
  bookmark button. The presenter sends target-seconds commands for thumbnail
  warming, bookmark add, and pin toggle; the UI thread still performs DB writes
  and thumbnail-cache requests. The native overlay currently draws a preview
  placeholder and action icons, with real thumbnail image upload left as the
  next texture-sharing slice.
- 2026-05-05: real hover thumbnails now use a narrow UI-to-presenter command
  channel rather than sharing egui textures across threads. The UI thread asks
  the existing thumbnail worker/cache for the hover target and sends a cloned
  RGBA `Arc<Vec<u8>>` when available; the presenter thread converts that image
  into its own egui texture before rendering. The perf graph keeps a larger
  sample history and decimates only at draw time so 60/120fps graphs occupy the
  full six-second panel.
- 2026-05-05: seek-hover previews now keep a low-rate periodic overlay render
  active while the hover target is alive, even when playback is paused or the
  video clock has not advanced. This lets the presenter re-request the warmed
  thumbnail and swap from `loading` to the decoded image without requiring a
  mouse move. Bookmark/pin actions and the time label live in a black action
  bar below the image instead of being drawn over thumbnail pixels.
- 2026-05-05: the UI side now also remembers the native hover thumbnail target
  inside `VideoPlayer` and pumps completed worker-cache thumbnails to the
  native presenter from `VideoPlayer::tick`. This covers the case where the
  thumbnail worker completes after the presenter's original request event. The
  hover pin icon receives current DB pin state from the UI thread and draws as
  active after a set-pin action or when the video already has a pin.
- 2026-05-05: native seek bars can now receive lightweight timeline markers
  from the UI thread. Pin, bookmark, and chapter positions are drawn directly on
  the presenter seek bar, while bookmark hover state is derived from those
  markers so the seek-hover bookmark button can show an active state near an
  existing bookmark.
- 2026-05-05: a first native left-edge jump panel is available while the HUD is
  visible. It lists the same lightweight marker set as a compact PIN/BM/CH time
  list and emits presenter-thread seek commands when a row is clicked. It is a
  marker-only bridge for now; thumbnail rows and deletion/edit affordances stay
  in the staged side-panel parity work.
- 2026-05-05: the native left jump panel visibility is separated from the
  bottom seek HUD hover region. Like the legacy fullscreen panel, the hidden
  state uses a narrow left-edge entry zone and the visible state keeps the
  whole panel width as the hover retention zone, so moving from the edge into a
  row does not dismiss the panel before click handling.
- 2026-05-05: native side-panel marker synchronization is now triggered when
  the pointer enters the left-edge zone, not only after seek-hover thumbnail
  requests. The overlay also has first-pass top and right hover panels fed by a
  small metadata command from the UI thread, keeping top/right panel visibility
  independent from the seek bar.
- 2026-05-05: S-key video tile mode is bridged into the native overlay. The UI
  thread still owns `VideoTileState` and thumbnail extraction, then sends tile
  snapshots to the presenter as RGBA payloads. While tile mode is restored
  during video-to-video navigation, the native overlay can keep a black
  preparing screen visible until the next video's tile worker is ready.
- 2026-05-05: native HUD parity work now configures the standalone overlay
  egui context with Japanese-capable Windows fonts, shows the left/top/right
  chrome together from any edge hover zone, keeps the left jump panel visible
  even when there are no pins/bookmarks/chapters, and routes native mouse wheel
  events back to fullscreen navigation or S-mode tile column changes. The left
  jump panel now receives richer pin/bookmark/chapter entries with thumbnails
  and bookmark deletion commands from the UI thread.
- 2026-05-05: side-panel polish now keeps the top bar, left jump panel, right
  metadata panel, and perf graph from occupying the same rectangle. The right
  panel uses a scrollable wrapped layout instead of fixed-position rows so long
  title/description metadata does not overlap or disappear. Mouse wheel input
  over the left or right panel is consumed by that panel's `ScrollArea` instead
  of being converted into previous/next-video navigation.
- 2026-05-05: right-panel metadata text now preserves real and escaped line
  breaks so YouTube descriptions remain paragraph-shaped instead of being
  flattened into a single wrapped line. The standalone native overlay font list
  also includes the Windows emoji font ahead of Japanese fallback fonts so
  emoji-heavy titles/descriptions render in the side panels. Left/right panel
  entry and retention hit tests now use side strips keyed by x position across
  the usable viewport height, excluding the bottom seek-bar zone. Wheel
  scrolling is still consumed only over the visible panel body.
- 2026-05-05: the native perf graph now dirties visible redraws at a finer
  100ms cadence, scales its Y axis around the current source frame interval so
  24/60/120/240fps material remains readable, and stitches the first post-pause
  sample to the previous graph point so paused playback does not create a wall
  clock gap in the six-second history.
- 2026-05-05: the perf graph history now advances on a virtual source-frame
  timeline rather than raw wall-clock sample arrival. Paused playback therefore
  stops graph scrolling and pruning instead of creating blank history that later
  disappears from the left side.
- 2026-05-05: the native overlay restored several legacy fullscreen HUD
  affordances that were missing from the parity slice. A paused video now shows
  large centered "start over" and "continue" buttons with a short key hint,
  startup/error states are drawn in the center overlay instead of leaving a
  silent black screen, feedback toasts and boundary hints are mirrored into the
  native overlay, Shift+Enter opens the external player, J/K jumps across
  chapter/bookmark/pin markers, and the top bar exposes a VST3 GUI toggle.
- 2026-05-05: side-panel hit zones now match the visible left/right panel
  widths across the usable viewport height, while still excluding the bottom
  seek-bar zone. When any edge panel is visible the bottom seek HUD is kept
  visible as part of the same HUD chrome; pure seek-bar hover still shows only
  the seek HUD. Left/right panels start below the top bar to avoid overlap, and
  the hover seek thumbnail is displayed closer to the legacy size.
- 2026-05-05: S-mode video-to-video wheel navigation now installs the native
  tile "preparing video" overlay before leaving the current video and again
  immediately after opening the next cached video. This keeps the dark tile
  curtain alive until the next tile worker snapshot is ready, avoiding a
  one-frame flash of the raw video surface.
- 2026-05-05: the next native presenter now receives an `initial_tile_overlay`
  flag in its startup config when S-mode video navigation is being restored.
  The presenter renders the opaque tile curtain before its first video present,
  closing the command-delivery race where the first frame could otherwise beat
  the follow-up `SetTileOverlay` message.
- 2026-05-05: the top hover toolbar was narrowed to overlay-only actions:
  VST3, perf graph, S-mode thumbnails, and close fullscreen. Playback,
  loop, mute, and bookmark actions remain on the bottom seek HUD or side
  panels, avoiding duplicate controls and top-bar button overlap. S-mode tile
  view also has a top-right close button that returns directly to normal video.
- 2026-05-05: native fullscreen now mirrors the file checked state into the
  overlay and draws the green check badge after Space toggles, matching the
  legacy fullscreen feedback. The VST3-available and checked sync commands are
  also change-filtered before they enter the presenter command queue, so steady
  playback no longer sends redundant no-op state updates every UI tick.
- 2026-05-05: the seek/jump thumbnail worker no longer has a fixed entry-count
  cap. Generated thumbnails are retained for the lifetime of the current
  `VideoPlayer`, so videos with hundreds of chapters can keep their left-panel
  thumbnails once decoded; memory is released when the player is dropped.
- 2026-05-05: S-mode's native tile curtain is now fully opaque black during
  preparing/navigation states, so the raw video surface cannot flash through at
  normal brightness. The top hover bar also replaced the S/P/V text buttons
  with a 2x2 tile icon, a small line-graph icon, and a wider `VST` button; the
  VST button resynchronizes plugin GUI ownership/topmost state to the native
  fullscreen HWND before toggling visibility.

Acceptance for the spike:

- The overlay appears above video with transparency.
- Video cadence stays stable at 1080p120.
- Overlay redraws can be throttled independently from video presents.
- Shutdown does not block the presenter thread or the eframe UI thread.

## Risks To Validate

- `SurfaceTargetUnsafe::CompositionVisual` is DX12-only. If wgpu chooses another
  backend, overlay creation must fail closed and the native presenter should
  keep video-only mode.
- Alpha support depends on the surface format and DComp surface path. The
  overlay now detaches its visual when no HUD is visible, but visible HUD
  pixels still need transparent composition around the drawn controls.
- The overlay wgpu device may be separate from the existing eframe render state.
  That is acceptable for a first spike, but production should avoid duplicating
  large GPU resources if it becomes expensive.
- Egui input must come from the existing native HWND event bridge. The overlay
  surface itself should not own input timing.

## Fallbacks

If the `CompositionVisual` surface path fails:

1. Transparent overlay HWND with its own wgpu surface.
   - Easier to create with standard wgpu/winit style APIs.
   - Higher risk of airspace, focus, and Z-order glitches.
2. Minimal D3D11 native HUD.
   - Most deterministic and fastest.
   - Requires reimplementing the hot HUD widgets instead of reusing egui.
3. Full custom egui D3D11 renderer.
   - Avoids wgpu surface issues.
   - Highest implementation cost and maintenance burden.

## Production Notes

- The overlay should be dirty-driven, not tied to video frame rate.
- UI stalls must never block `NativeVideoPresenter::present`.
- If overlay initialization fails, keep the native video path alive and fall
  back to native key/mouse shortcuts plus video-only presentation.
- Keep the old egui fullscreen path available until the native overlay reaches
  feature parity for seek bar, hover bar, metadata, and VST focus behavior.
