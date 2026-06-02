# local_adjust_lab

`local_adjust_lab` is a lightweight prototype for the v1.1.0 local adjustment
layer pipeline. It does not depend on the main mIV app state, mask DB, conceal
DB, video pipeline, or AI model runtime.

The prototype exercises the intended future interface:

```text
RGBA image + ordered local adjustment layers -> RGBA image with the same size
```

## Run

```powershell
cargo run -p local_adjust_lab -- path\to\image.png
```

You can also start the app without arguments and drop a JPEG or PNG file onto
the window.

## Current prototype scope

- Multiple adjustment layers
- Images start with no adjustment layers so the first state is unchanged
- New layers start with the selected mask source + No Effect. Pick the effect
  afterward from the selected layer's effect selector.
- mIV-like floating panel over the image instead of a docked side panel
- Left panel: an mIV-like workflow strip:
  Eraser / Adjustment / Conceal / Crop / Save. Eraser and Conceal are placeholder
  panels in the prototype so the lab stays lightweight.
- Right panel: parameters for the selected mask source, selected manual-mask
  tool, and selected effect while the Adjustment panel is active.
- Image navigation for testing large images:
  - Ctrl + mouse wheel: zoom around the pointer
  - Mouse wheel: zoom around the pointer
  - Ctrl while hovering the canvas: temporarily show the source image
  - Alt while hovering the canvas: temporarily invert the mask preview
  - Shift while hovering the canvas: show a loupe around the pointer
  - Zoom can go below fit-to-window, which is useful when editing final-stage
    crop handles near the image edge.
  - Space + drag or middle-button drag: pan
  - Arrow keys: nudge selected manual-mask object
  - `[` / `]`: rotate selected manual-mask object
- Mask sources:
  - Full image
  - Manual mask
    - Brush
    - Lasso fill
    - Editable line / vertical line / horizontal line / rectangle / ellipse objects
  - Linear gradient with canvas handles
  - Radial gradient with canvas handles
  - Luma range
  - Color range with click picker
  - Subject selection mask using an optional U²-Netp ONNX model. Creating or
    regenerating a subject layer requires the model and starts one generation
    pass automatically when available. Saved subject masks are stored in the
    sidecar and remain usable without the model. The generated soft matte can be
    refined for cutout work by thresholding it toward a binary mask, optionally
    shrinking/expanding it, and smoothing only the boundary band. The original
    generated matte is kept with the layer, and cutout refinement is enabled
    with a checkbox. When enabled, the sliders regenerate from that matte
    instead of destructively editing the current mask; turning it off restores
    the original matte.
  - Region segmentation mask with color-coded candidates that can be toggled by
    clicking or dragging on the image. Creating a region layer starts one
    default full-image segmentation pass automatically; the tool panel can
    regenerate the full image, subject interior, or background.
- Shared mask controls:
  - Invert
  - Expand / shrink
  - Feather
  - Opacity
- Mask operations:
  - Add/subtract manual masks on top of any mask source. These masks are binary,
    so gradient / subject matte softness is kept
    in the base mask while local mistakes can be patched by hand.
  - For non-manual mask sources, Add Mask / Subtract Mask tool panels are hidden
    by default. Press Add Mask or Subtract Mask to open that edit
    panel; pressing the same button again closes it without creating a mask.
  - Mask preview colors are selectable from three presets. When both edit panels
    are closed, the final mask uses the base color. While editing Add Mask, the
    base mask uses the base color and the add mask uses the edit color. While
    editing Subtract Mask, base + add uses the base color and the subtract mask
    uses the edit color.
  - Creating or editing a mask automatically turns the mask preview on.
  - Editing effect parameters automatically turns the mask preview off so the
    result is easier to inspect. Alt temporarily inverts the current mask
    preview state.
  - Duplicate a layer when reusing the same mask with a different effect.
- Effects:
  - Tone
  - Tone curve
  - Selective color with an image-click eyedropper for the target hue
  - Clarity
  - Highlights/Shadows
  - Dehaze
  - Blur
  - Motion blur
  - Tilt shift
    - New tilt-shift effects start with no focus range, so they are visually
      unchanged until the user drags on the canvas. Linear Range creation is
      active by default; press Radial Range creation before dragging when a
      radial focus area is desired. After a range is placed, creation buttons
      become inactive until the user starts a new range or clears the current
      one. Clear Range keeps the current range type active for the next drag.
  - Lens blur
  - Radial blur
  - Soft focus
  - Mosaic with long-edge-ratio / fixed-pixel tile sizing and the same three
    boundary modes as the conceal mosaic tool
  - Speed lines / radial focus lines
  - Sharpen
  - HSL / hue shift
  - Look presets
  - Bloom
  - God rays
  - Lens flare
  - Cloud/fog
  - Spotlight
  - Vignette
  - Film grain
  - Noise
  - Chromatic aberration
  - Halftone
  - Screen tone
  - Color halftone
  - Textureizer
  - Cross/star glow
  - Edge-preserving smooth
  - Despeckle
  - Median
  - Outline stroke
- Effect parameters start from near-identity values. Use the per-effect preset
  buttons for practical starting points, or Reset to return the current effect
  to its default values.
- RGB color parameters in Color Fill, Outline Stroke, Color Overlay, Neon Glow,
  Speed Lines, Cloud/Fog, and Spotlight share a
  control with a color button, HEX readout, RGB sliders, and an image-click
  eyedropper.
- Color Fill starts with a shape placeholder and full opacity. Choosing Solid,
  Linear Gradient, or Radial Gradient immediately makes the fill visible.
- Linear and radial gradients in Color Fill and Color Overlay can be adjusted
  with the same canvas drag handles used by gradient-like mask/effect controls.
- Effects with image-space center or light-source parameters can show canvas
  handles for direct dragging on the image. God Rays, Lens Flare, Spotlight,
  Speed Lines, Radial Blur, Ripple Wave Distortion, Pinch/Spherize, Twirl,
  Polar Coordinates, and Lens Correction share the `画像ハンドルを表示` toggle;
  turn it off from the effect panel when it gets in the way. Gradient-like
  effects use the gradient handle system, and Tilt Shift keeps its dedicated
  range handles.
- Each layer has `前` and `後` mask application toggles. `前` limits the effect
  input to the mask before calculation; `後` clips the calculated result by the
  mask. Existing-style local adjustments use `前` off / `後` on, while spreading
  effects such as Wind, Outline Stroke, Neon Glow, Diffuse Glow, Bloom, God
  Rays, Glowing Edges, and Cross/Star Glow default to `前` on / `後` off so the
  effect can extend past the mask.
- 3D LUT sample files are available under `tools/local_adjust_lab/sample_luts/`.
  They are small self-made `.cube` files for quick testing of the LUT loader and
  effect strength slider.
- Final-stage crop preview/export. The Crop panel has Reset, aspect ratio
  selection (Keep, Free, 16:9 through 9:16), and X/Y/W/H numeric inputs. The
  area outside the crop is dimmed while the Crop panel is open. Drag inside the
  image to create a crop from the full-image state, or drag handles to adjust an
  existing crop. The crop rectangle stays in source-image coordinates and is
  only applied when saving the rendered result.
- Layer settings sidecar save/load. `foo.png` uses `foo.png.miv`; the sidecar is
  JSON with binary masks packed as 1-bit or 8-bit data, deflated, then base64
  encoded.
- Async recomposition with generation discard, cancellation for superseded
  renders, and progress status for long-running filters such as Median.
- Manual-mask undo snapshots for layer operations, brush/lasso/object edits, and object deletion
- Japanese system font registration so labels render correctly on Windows
- Save result as `*_local_adjust.png`

Layer edits request a follow-up repaint, so non-drag operations such as delete,
duplicate, enable/disable, and layer reorder should recompose without needing
another mouse interaction.

Manual mask behavior is intentionally being kept close to the existing eraser
and conceal mask tools. The goal is to make this prototype's manual mask engine
replaceable into those existing tools later, instead of creating a separate
local-adjust-only mask implementation.

Gradients and color range are mask sources, not manual-mask tools. Linear and
radial gradients stay parametric until composition time, and color range is
created by clicking the image after selecting the Color Range mask source.

The adjustment-layer mosaic effect mirrors the conceal mosaic semantics:
long-edge ratio mode is useful for image-size-independent results, fixed-pixel
mode is available for exact tile sizes, and the boundary mode controls whether
tiles touching the mask are drawn opaque, blended by mask coverage, or clipped
to the mask shape. The opaque mode intentionally lets the mosaic tile extend
outside the mask, matching the conceal workflow.

Subject selection masks are implemented as an optional prototype path. Place
downloaded models under `tools/local_adjust_lab/models/`; the prototype looks
for `u2netp.onnx` as a lightweight Apache-2.0 salient-object /
foreground-background segmentation candidate. New subject mask generation and
regeneration are disabled when the model is missing, but saved subject masks in
`.miv` sidecars can still be loaded, edited, and applied. It generates a
foreground/subject matte on a worker thread. Use mask inversion, or the
"background" button in the tool panel, when applying an effect to the background
instead of the subject.

Region segmentation masks are a separate mask source. They currently use a
lightweight color/connectivity/boundary based algorithm, optionally constrained
to an existing subject mask. Generated regions start unselected. Unselected
candidates are shown as animated colored boundaries only; click or drag on the
image to add/remove individual regions from the final mask. Selected regions are
shown with the selected mask color and a bright boundary. Lower color tolerance creates finer
splits; higher tolerance merges similar colors. Higher minimum area drops small fragments.
Boundary pixels and other unlabeled internal gaps are assigned to the nearest
generated region so adjacent selected regions compose into a continuous mask.
