//! Shared edit-result materialization for desktop and remote rendering.
//!
//! Adapters own persistence, caches, worker queues and generations. This module
//! owns the typed source selection and pure CPU order:
//! raw -> erase -> local-adjust -> conceal.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;

#[derive(Clone)]
pub(crate) struct MaskSnapshot {
    pub(crate) bitmap: Vec<bool>,
    pub(crate) shapes: Vec<crate::mask_db::Shape>,
    pub(crate) size: [usize; 2],
}

pub(crate) struct EraseMaterialize {
    pub(crate) mask: MaskSnapshot,
    pub(crate) runtime: Option<Arc<crate::ai::runtime::AiRuntime>>,
    pub(crate) manager: Arc<crate::ai::model_manager::ModelManager>,
    pub(crate) log_prefix: String,
}

pub(crate) struct LocalAdjustMaterialize {
    pub(crate) layers: Vec<local_adjust_core::LocalAdjustmentLayer>,
}

pub(crate) struct ConcealMaterialize {
    pub(crate) mask: MaskSnapshot,
    pub(crate) preset: crate::conceal::ConcealPreset,
}

pub(crate) enum EditLayer<T> {
    Absent,
    Pending,
    Pixels(Arc<egui::ColorImage>),
    Materialize(T),
}

pub(crate) struct EditSourceRequest {
    pub(crate) raw: Arc<egui::ColorImage>,
    pub(crate) erase: EditLayer<EraseMaterialize>,
    pub(crate) local_adjust: EditLayer<LocalAdjustMaterialize>,
    pub(crate) conceal: EditLayer<ConcealMaterialize>,
}

pub(crate) enum EditSourceResult {
    Ready(EditSourceOutput),
    Pending,
    Cancelled,
}

pub(crate) struct EditSourceOutput {
    pub(crate) pixels: Arc<egui::ColorImage>,
    pub(crate) timing: EditSourceTiming,
    pub(crate) used_diffusion_fallback: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct EditSourceTiming {
    pub(crate) erase_ms: f64,
    pub(crate) local_adjust_ms: f64,
    pub(crate) conceal_ms: f64,
}

#[derive(Debug)]
pub(crate) enum EditSourceError {
    Cancelled,
    InvalidMask(String),
    Erase(String),
    LocalAdjust(String),
}

impl std::fmt::Display for EditSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::InvalidMask(message) | Self::Erase(message) | Self::LocalAdjust(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for EditSourceError {}

pub(crate) fn execute_edit_source(
    request: EditSourceRequest,
    cancel: &Arc<AtomicBool>,
) -> Result<EditSourceResult, EditSourceError> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(EditSourceResult::Cancelled);
    }
    let mut timing = EditSourceTiming::default();
    let mut used_diffusion_fallback = false;
    let mut pixels = request.raw;

    match request.erase {
        EditLayer::Absent => {}
        EditLayer::Pending => return Ok(EditSourceResult::Pending),
        EditLayer::Pixels(ready) => pixels = ready,
        EditLayer::Materialize(edit) => {
            let started = std::time::Instant::now();
            let outcome = materialize_erase(&pixels, edit, cancel)?;
            timing.erase_ms = started.elapsed().as_secs_f64() * 1000.0;
            used_diffusion_fallback = outcome.used_diffusion_fallback;
            pixels = Arc::new(outcome.image);
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(EditSourceResult::Cancelled);
    }

    match request.local_adjust {
        EditLayer::Absent => {}
        EditLayer::Pending => return Ok(EditSourceResult::Pending),
        EditLayer::Pixels(ready) => pixels = ready,
        EditLayer::Materialize(edit) => {
            let started = std::time::Instant::now();
            let outcome = materialize_local_adjust(&pixels, edit.layers, cancel)?;
            timing.local_adjust_ms = started.elapsed().as_secs_f64() * 1000.0;
            pixels = outcome.pixels;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(EditSourceResult::Cancelled);
    }

    match request.conceal {
        EditLayer::Absent => {}
        EditLayer::Pending => return Ok(EditSourceResult::Pending),
        EditLayer::Pixels(ready) => pixels = ready,
        EditLayer::Materialize(edit) => {
            let started = std::time::Instant::now();
            pixels = materialize_conceal(&pixels, edit, cancel)?;
            timing.conceal_ms = started.elapsed().as_secs_f64() * 1000.0;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        Ok(EditSourceResult::Cancelled)
    } else {
        Ok(EditSourceResult::Ready(EditSourceOutput {
            pixels,
            timing,
            used_diffusion_fallback,
        }))
    }
}

pub(crate) struct EraseMaterializeOutput {
    pub(crate) image: egui::ColorImage,
    pub(crate) used_diffusion_fallback: bool,
}

pub(crate) fn run_inpaint_pure(
    runtime: Option<&Arc<crate::ai::runtime::AiRuntime>>,
    manager: &Arc<crate::ai::model_manager::ModelManager>,
    original: &egui::ColorImage,
    composite: &[bool],
    w: usize,
    h: usize,
    cancel: &Arc<AtomicBool>,
    log_prefix: &str,
    progress_tx: Option<&std::sync::mpsc::Sender<crate::ui_erase::EraseInpaintProgress>>,
) -> crate::ui_erase::InpaintOutcome {
    use crate::ui_erase::EraseInpaintProgress;
    crate::ui_erase::report_inpaint_progress(progress_tx, EraseInpaintProgress::Preparing);
    if let Some(runtime) = runtime {
        let kind = crate::ai::ModelKind::InpaintMiGan;
        match manager.model_path(kind) {
            Some(model_path) => {
                if !runtime.is_loaded(kind)
                    && let Err(error) = runtime.load_model(kind, &model_path)
                {
                    crate::logger::log(format!(
                        "[erase] {log_prefix} MI-GAN load failed: {error}, falling back to diffusion"
                    ));
                    crate::ui_erase::report_inpaint_progress(
                        progress_tx,
                        EraseInpaintProgress::DiffusionFallback,
                    );
                    return crate::ui_erase::InpaintOutcome {
                        image: crate::ui_erase::inpaint_diffuse(original, composite, w, h),
                        used_diffusion_fallback: true,
                    };
                }
                match crate::ui_erase::inpaint_migan(
                    runtime,
                    original,
                    composite,
                    w,
                    h,
                    cancel,
                    progress_tx,
                ) {
                    Ok(image) => {
                        return crate::ui_erase::InpaintOutcome {
                            image,
                            used_diffusion_fallback: false,
                        };
                    }
                    Err(error) => crate::logger::log(format!(
                        "[erase] {log_prefix} MI-GAN failed: {error}, falling back to diffusion"
                    )),
                }
            }
            None => crate::logger::log(format!(
                "[erase] {log_prefix} MI-GAN model not found, falling back to diffusion"
            )),
        }
    } else {
        crate::logger::log(format!(
            "[erase] {log_prefix} AI runtime not available, falling back to diffusion"
        ));
    }
    crate::ui_erase::report_inpaint_progress(progress_tx, EraseInpaintProgress::DiffusionFallback);
    crate::ui_erase::InpaintOutcome {
        image: crate::ui_erase::inpaint_diffuse(original, composite, w, h),
        used_diffusion_fallback: true,
    }
}

pub(crate) fn materialize_erase(
    source: &egui::ColorImage,
    edit: EraseMaterialize,
    cancel: &Arc<AtomicBool>,
) -> Result<EraseMaterializeOutput, EditSourceError> {
    let mask = resize_mask_snapshot(edit.mask, source.size, cancel)?;
    let outcome = crate::ui_erase::erase_from_saved_mask(
        edit.runtime.as_ref(),
        &edit.manager,
        source,
        &mask.bitmap,
        &mask.shapes,
        cancel,
        &edit.log_prefix,
    )
    .map_err(EditSourceError::Erase)?;
    Ok(EraseMaterializeOutput {
        image: outcome.image,
        used_diffusion_fallback: outcome.used_diffusion_fallback,
    })
}

pub(crate) struct LocalAdjustMaterializeOutput {
    pub(crate) pixels: Arc<egui::ColorImage>,
    pub(crate) layers: Vec<local_adjust_core::LocalAdjustmentLayer>,
    pub(crate) active: bool,
}

pub(crate) fn materialize_local_adjust(
    source: &Arc<egui::ColorImage>,
    mut layers: Vec<local_adjust_core::LocalAdjustmentLayer>,
    cancel: &Arc<AtomicBool>,
) -> Result<LocalAdjustMaterializeOutput, EditSourceError> {
    let [width, height] = source.size;
    for layer in &mut layers {
        if !layer.masks_match_dims(width.max(1), height.max(1)) {
            layer.resize_masks_to(width.max(1), height.max(1));
        }
    }
    let active = layers
        .iter()
        .any(|layer| layer.enabled && layer.opacity > 0.0);
    if !active || cancel.load(Ordering::Relaxed) {
        return Ok(LocalAdjustMaterializeOutput {
            pixels: Arc::clone(source),
            layers,
            active,
        });
    }
    let rgba = crate::capture::color_image_to_rgba(source);
    let src = local_adjust_core::RgbaImageRef {
        width,
        height,
        pixels: &rgba,
    };
    let rendered =
        local_adjust_core::apply_layers_with_progress(src, &layers, Some(cancel), |_| {}).map_err(
            |error| match error {
                local_adjust_core::LocalAdjustError::Cancelled => EditSourceError::Cancelled,
                other => EditSourceError::LocalAdjust(other.to_string()),
            },
        )?;
    Ok(LocalAdjustMaterializeOutput {
        pixels: Arc::new(egui::ColorImage::from_rgba_unmultiplied(
            [rendered.width, rendered.height],
            &rendered.pixels,
        )),
        layers,
        active,
    })
}

pub(crate) fn materialize_conceal(
    source: &Arc<egui::ColorImage>,
    edit: ConcealMaterialize,
    cancel: &Arc<AtomicBool>,
) -> Result<Arc<egui::ColorImage>, EditSourceError> {
    let mut mask = resize_mask_snapshot(edit.mask, source.size, cancel)?;
    if !crate::mask_db::rasterize_shapes_into_cancel(
        &mut mask.bitmap,
        &mask.shapes,
        source.size[0],
        source.size[1],
        cancel,
    ) {
        return Ok(Arc::clone(source));
    }
    if !mask.bitmap.iter().any(|&bit| bit) {
        return Ok(Arc::clone(source));
    }
    Ok(crate::conceal_compose::compose_with_preset_cancel(
        source,
        &mask.bitmap,
        &edit.preset,
        cancel,
    )
    .map(Arc::new)
    .unwrap_or_else(|| Arc::clone(source)))
}

pub(crate) fn resize_mask_snapshot(
    mut mask: MaskSnapshot,
    target: [usize; 2],
    cancel: &AtomicBool,
) -> Result<MaskSnapshot, EditSourceError> {
    let [source_w, source_h] = mask.size;
    let [target_w, target_h] = target;
    if mask.bitmap.len() != source_w.saturating_mul(source_h) {
        return Err(EditSourceError::InvalidMask(
            "mask size mismatch".to_string(),
        ));
    }
    if cancel.load(Ordering::Relaxed) || mask.size == target {
        return Ok(mask);
    }
    mask.bitmap =
        crate::mask_db::rescale_mask(&mask.bitmap, source_w, source_h, target_w, target_h);
    let sx = target_w as f32 / source_w.max(1) as f32;
    let sy = target_h as f32 / source_h.max(1) as f32;
    for shape in &mut mask.shapes {
        shape.scale_xy(sx, sy);
    }
    mask.size = target;
    Ok(mask)
}

pub(crate) fn page_key_for_grid_item(item: &crate::grid_item::GridItem) -> Option<String> {
    match item {
        crate::grid_item::GridItem::Image(path) => Some(crate::adjustment_db::normalize_path(path)),
        crate::grid_item::GridItem::ZipImage {
            zip_path,
            entry_name,
        } => Some(crate::adjustment_db::zip_entry_key(zip_path, entry_name)),
        crate::grid_item::GridItem::PdfPage {
            pdf_path, page_num, ..
        } => Some(page_key_for_pdf(pdf_path, *page_num)),
        _ => None,
    }
}

pub(crate) fn page_key_for_remote(
    logical_path: &Path,
    subresource: &mimageviewer_ipc::RemoteSubresource,
) -> Option<String> {
    match subresource {
        mimageviewer_ipc::RemoteSubresource::File => {
            Some(crate::adjustment_db::normalize_path(logical_path))
        }
        mimageviewer_ipc::RemoteSubresource::ZipEntry { entry_name } => Some(
            crate::adjustment_db::zip_entry_key(logical_path, entry_name),
        ),
        mimageviewer_ipc::RemoteSubresource::PdfPage { page_number } => {
            Some(page_key_for_pdf(logical_path, *page_number))
        }
        mimageviewer_ipc::RemoteSubresource::ZipDirectory { .. } => None,
    }
}

fn page_key_for_pdf(path: &Path, page_number: u32) -> String {
    crate::adjustment_db::zip_entry_key(path, &format!("page_{page_number}"))
}

pub(crate) fn comic_composite(
    base: &Arc<egui::ColorImage>,
    objects: &[comic_core::AnnotationObject],
    stored_edit_space: [usize; 2],
    fonts: &comic_core::FontSet,
    stamp_cache: &mut std::collections::HashMap<String, Option<Arc<comic_core::RgbaOverlay>>>,
    cancel: &AtomicBool,
) -> Arc<egui::ColorImage> {
    if objects.is_empty() || cancel.load(Ordering::Relaxed) {
        return Arc::clone(base);
    }
    // Saved annotation coordinates use the original raster's pixel space. Catalog layout/aspect
    // dimensions (notably PDF page-box fixed-point values) must never be passed here.
    let scale = base.size[0].max(base.size[1]) as f32
        / stored_edit_space[0].max(stored_edit_space[1]).max(1) as f32;
    let scaled = if (scale - 1.0).abs() > 1e-4 {
        comic_core::scale_scene(objects, scale)
    } else {
        objects.to_vec()
    };
    let (stamps, _, _) =
        crate::comic_stamp::build_stamp_images_from_cache_snapshot(&scaled, stamp_cache, cancel);
    let layers =
        comic_core::bake_annotation_layers(&scaled, base.size[0], base.size[1], fonts, &stamps);
    Arc::new(crate::comic_overlay::composite_annotation_layers(
        base, &layers,
    ))
}

pub(crate) fn export_crop_rect_for_pixels(
    settings: crate::export_crop::CropSettings,
    stored_edit_space: [usize; 2],
    pixel_dims: [usize; 2],
) -> crate::export_crop::CropRect {
    // Crop rectangles are persisted in original-raster pixel coordinates, not catalog
    // layout/aspect coordinates. Keep the name explicit at this shared boundary.
    if stored_edit_space == pixel_dims {
        return settings.rect;
    }
    let sx = pixel_dims[0] as f32 / stored_edit_space[0].max(1) as f32;
    let sy = pixel_dims[1] as f32 / stored_edit_space[1].max(1) as f32;
    crate::export_crop::CropRect {
        min_x: settings.rect.min_x * sx,
        min_y: settings.rect.min_y * sy,
        max_x: settings.rect.max_x * sx,
        max_y: settings.rect.max_y * sy,
    }
    .sanitized(pixel_dims[0], pixel_dims[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_image() -> Arc<egui::ColorImage> {
        Arc::new(egui::ColorImage::new(
            [3, 2],
            vec![
                egui::Color32::RED,
                egui::Color32::GREEN,
                egui::Color32::BLUE,
                egui::Color32::WHITE,
                egui::Color32::BLACK,
                egui::Color32::GRAY,
            ],
        ))
    }

    #[test]
    fn no_edits_preserve_the_exact_source_pixels_and_arc() {
        let raw = test_image();
        let result = execute_edit_source(
            EditSourceRequest {
                raw: Arc::clone(&raw),
                erase: EditLayer::Absent,
                local_adjust: EditLayer::Absent,
                conceal: EditLayer::Absent,
            },
            &Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let EditSourceResult::Ready(output) = result else {
            panic!("expected ready");
        };
        assert!(Arc::ptr_eq(&raw, &output.pixels));
        assert_eq!(raw.pixels, output.pixels.pixels);
    }

    #[test]
    fn unfinished_upper_layer_never_completes_from_lower_pixels() {
        let result = execute_edit_source(
            EditSourceRequest {
                raw: test_image(),
                erase: EditLayer::Pending,
                local_adjust: EditLayer::Absent,
                conceal: EditLayer::Absent,
            },
            &Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(matches!(result, EditSourceResult::Pending));
    }

    #[test]
    fn remote_materialize_request_applies_conceal() {
        let raw = test_image();
        let mut preset = crate::conceal::ConcealPreset::default();
        preset.conceal_type = crate::conceal::ConcealType::BlackFill;
        preset.fill_opacity_percent = 100;
        let result = execute_edit_source(
            EditSourceRequest {
                raw,
                erase: EditLayer::Absent,
                local_adjust: EditLayer::Absent,
                conceal: EditLayer::Materialize(ConcealMaterialize {
                    mask: MaskSnapshot {
                        bitmap: vec![true, false, false, false, false, false],
                        shapes: Vec::new(),
                        size: [3, 2],
                    },
                    preset,
                }),
            },
            &Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let EditSourceResult::Ready(output) = result else {
            panic!("expected ready");
        };
        assert_eq!(output.pixels.pixels[0], egui::Color32::BLACK);
        assert_eq!(output.pixels.pixels[1], egui::Color32::GREEN);
    }

    #[test]
    fn moved_local_executor_matches_the_previous_formula_pixel_for_pixel() {
        let source = test_image();
        let layers = vec![local_adjust_core::LocalAdjustmentLayer::new(
            "invert",
            local_adjust_core::LocalMask::Full,
            local_adjust_core::LocalEffect::Invert(local_adjust_core::InvertParams::default()),
        )];
        let rgba = crate::capture::color_image_to_rgba(&source);
        let expected = local_adjust_core::apply_layers_with_progress(
            local_adjust_core::RgbaImageRef {
                width: source.size[0],
                height: source.size[1],
                pixels: &rgba,
            },
            &layers,
            None,
            |_| {},
        )
        .unwrap();
        let actual =
            materialize_local_adjust(&source, layers, &Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(actual.pixels.size, [expected.width, expected.height]);
        assert_eq!(
            crate::capture::color_image_to_rgba(&actual.pixels),
            expected.pixels
        );
    }

    #[test]
    fn app_and_remote_page_keys_match_for_image_zip_and_pdf() {
        use crate::grid_item::GridItem;
        use mimageviewer_ipc::RemoteSubresource;
        let image = std::path::PathBuf::from(r"C:\pictures\page.png");
        let zip = std::path::PathBuf::from(r"C:\pictures\book.zip");
        let pdf = std::path::PathBuf::from(r"C:\pictures\book.pdf");
        assert_eq!(
            page_key_for_grid_item(&GridItem::Image(image.clone())),
            page_key_for_remote(&image, &RemoteSubresource::File)
        );
        assert_eq!(
            page_key_for_grid_item(&GridItem::ZipImage {
                zip_path: zip.clone(),
                entry_name: "chapter/page.png".to_string(),
            }),
            page_key_for_remote(
                &zip,
                &RemoteSubresource::ZipEntry {
                    entry_name: "chapter/page.png".to_string(),
                },
            )
        );
        assert_eq!(
            page_key_for_grid_item(&GridItem::PdfPage {
                pdf_path: pdf.clone(),
                page_num: 7,
                content_type: None,
            }),
            page_key_for_remote(&pdf, &RemoteSubresource::PdfPage { page_number: 7 },)
        );
    }
}
