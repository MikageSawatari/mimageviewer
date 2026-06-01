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
  - Subject selection mask using an optional U²-Netp ONNX model. Creating a
    subject layer starts one generation pass automatically when the model is
    available.
  - Region segmentation mask with color-coded candidates that can be toggled by
    clicking or dragging on the image. Creating a region layer starts one
    default full-image segmentation pass automatically; the tool panel can
    regenerate the full image, subject interior, or background.
  - Experimental SAM-candidate partitioning. Put external white/alpha candidate
    mask images in a sibling folder named `<image-stem>.sam_masks`; the region
    tool can convert overlapping masks and uncovered gaps into non-overlapping
    region candidates.
  - Experimental SAM2 candidate generation. Place `encoder.onnx` and
    `decoder.onnx` under `tools/local_adjust_lab/models/sam2_hiera_tiny\`; the
    region tool can run grid point prompts, collect candidate masks, and feed
    them into the same non-overlapping region partitioner.
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
  - Duplicate a layer when reusing the same mask with a different effect.
- Effects:
  - Tone
  - Tone curve
  - Clarity
  - Highlights/Shadows
  - Dehaze
  - Blur
  - Soft focus
  - Mosaic with long-edge-ratio / fixed-pixel tile sizing and the same three
    boundary modes as the conceal mosaic tool
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
- Final-stage crop preview/export. The Crop panel has Reset, aspect ratio
  selection (Keep, Free, 16:9 through 9:16), and X/Y/W/H numeric inputs. The
  area outside the crop is dimmed while the Crop panel is open. Drag inside the
  image to create a crop from the full-image state, or drag handles to adjust an
  existing crop. The crop rectangle stays in source-image coordinates and is
  only applied when saving the rendered result.
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

The adjustment-layer mosaic effect mirrors the conceal mosaic semantics:
long-edge ratio mode is useful for image-size-independent results, fixed-pixel
mode is available for exact tile sizes, and the boundary mode controls whether
tiles touching the mask are drawn opaque, blended by mask coverage, or clipped
to the mask shape. The opaque mode intentionally lets the mosaic tile extend
outside the mask, matching the conceal workflow.

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
shown with the selected mask color and a bright boundary. Lower color tolerance creates finer
splits; higher tolerance merges similar colors. Higher minimum area drops small fragments.
Boundary pixels and other unlabeled internal gaps are assigned to the nearest
generated region so adjacent selected regions compose into a continuous mask.

For SAM-style experiments, the lab can also read candidate masks generated by an
external tool. For `foo.png`, place mask images under `foo.sam_masks\`. Each
candidate image is treated as a soft mask, thresholded, and combined with the
other candidates by membership signature. Pixels covered by the same set of
candidate masks become one connected region; overlaps become their own regions,
and pixels not covered by any candidate remain selectable gap/background
regions. This keeps the UI partitioned even when the source AI masks overlap.

The lab can also run SAM2 Hiera Tiny ONNX directly when the model files are
present:

```text
tools/local_adjust_lab/models/sam2_hiera_tiny/encoder.onnx
tools/local_adjust_lab/models/sam2_hiera_tiny/decoder.onnx
```

The current prototype uses grid foreground point prompts. Encoder results are
computed once per run, the best decoder candidate per grid point is kept, then
stability filtering and mask-IoU NMS remove unstable or duplicate candidates.
The resulting soft masks are converted into the same partitioned `RegionMask`.
SAM2 candidate masks often overlap, so the SAM2 path gives each pixel to the
first high-score candidate instead of making every overlap into a separate
region. This keeps the result coarser and more practical for clicking. This is
meant for quality and workflow validation; model download and GPU/DirectML
packaging are still future integration work.
