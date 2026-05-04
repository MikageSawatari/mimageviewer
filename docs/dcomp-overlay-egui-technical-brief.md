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

Acceptance for the spike:

- The overlay appears above video with transparency.
- Video cadence stays stable at 1080p120.
- Overlay redraws can be throttled independently from video presents.
- Shutdown does not block the presenter thread or the eframe UI thread.

## Risks To Validate

- `SurfaceTargetUnsafe::CompositionVisual` is DX12-only. If wgpu chooses another
  backend, overlay creation must fail closed and the native presenter should
  keep video-only mode.
- Alpha support depends on the surface format and DComp surface path. The spike
  must prove transparent pixels actually compose correctly.
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
