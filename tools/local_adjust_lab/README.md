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

- Multiple local adjustment layers
- Images start with no local adjustment layers so the first state is unchanged
- New layers start with the selected mask source + No Effect. Pick the effect
  afterward from the selected layer's effect selector.
- mIV-like floating panel over the image instead of a docked side panel
- Left panel: display controls, layer list, selected-layer effect selection, and
  manual-mask tool selection
- Right panel: parameters for the selected mask source, selected manual-mask
  tool, and selected effect
- Image navigation for testing large images:
  - Ctrl + mouse wheel: zoom around the pointer
  - Mouse wheel: zoom around the pointer
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
  - Subject selection mask using an optional U²-Netp ONNX model
  - Region segmentation mask with color-coded candidates that can be toggled by
    clicking or dragging on the image
- Shared mask controls:
  - Invert
  - Expand / shrink
  - Feather
  - Opacity
- Mask operations:
  - Add/subtract manual correction masks on top of any mask source. These
    corrections are binary masks, so gradient / subject matte softness is kept
    in the base mask while local mistakes can be patched by hand.
  - Copy only the mask data and mask modifiers from another layer while keeping
    the current layer's effect and opacity
- Effects:
  - Tone
  - Tone curve
  - Clarity
  - Highlights/Shadows
  - Dehaze
  - Blur
  - Soft focus
  - Mosaic
  - Sharpen
  - HSL / hue shift
  - Look presets
  - Bloom
  - Vignette
  - Film grain
  - Chromatic aberration
  - Halftone
  - Cross/star glow
  - Edge-preserving smooth
- Effect parameters start from near-identity values. Use the per-effect preset
  buttons for practical starting points, or Reset to return the current effect
  to its default values.
- Final-stage crop preview/export. The crop rectangle stays in source-image
  coordinates and is only applied when saving the rendered result.
- Layer settings sidecar save/load. `foo.png` uses `foo.png.miv`; the sidecar is
  JSON with binary masks packed as 1-bit or 8-bit data, deflated, then base64
  encoded.
- Async recomposition with generation discard
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

Subject selection masks are implemented as an optional prototype path. Place
downloaded models under `tools/local_adjust_lab/models/`; the prototype looks
for `u2netp.onnx` as a lightweight Apache-2.0 salient-object /
foreground-background segmentation candidate. It generates a foreground/subject
matte on a worker thread. Use mask inversion, or the "background" button in the
tool panel, when applying an effect to the background instead of the subject.

Region segmentation masks are a separate mask source. They currently use a
lightweight color/connectivity/boundary based algorithm, optionally constrained
to an existing subject mask. Generated regions start unselected. Unselected
candidates are shown as animated colored boundaries only; click or drag on the
image to add/remove individual regions from the final mask. Selected regions are
shown in pink with a bright boundary. Lower color tolerance creates finer
splits; higher tolerance merges similar colors. Higher minimum area drops small fragments.
Boundary pixels and other unlabeled internal gaps are assigned to the nearest
generated region so adjacent selected regions compose into a continuous mask.
