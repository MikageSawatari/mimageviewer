use super::*;
use crate::app::{
    CompareOriginalVariant, ComparePreparationState, ComparePrepareRequest, ComparePrepareResult,
    CompareSourceGeometry, PinnedOriginalSource,
};

impl App {
    pub(crate) fn next_compare_prepared_key(&mut self) -> u64 {
        let key = self.compare_prepared_next_key;
        self.compare_prepared_next_key = key.wrapping_add(1).max(1);
        key
    }

    pub(super) fn compare_pin_original_source(
        &self,
        idx: usize,
        job: &crate::capture::CapturePixelJob,
    ) -> PinnedOriginalSource {
        match self.current_raw_source_pixels(idx) {
            Some(pixels) => PinnedOriginalSource::Raw {
                pixels,
                processed_source_size: job.source.size,
                crop: job.crop,
                rotation: job.rotation,
            },
            None => PinnedOriginalSource::Unavailable,
        }
    }

    pub(super) fn ensure_compare_original_pair(&mut self, ctx: &egui::Context, idx: usize) {
        if matches!(self.compare_view_mode, crate::app::CompareViewMode::Off)
            || !self
                .original_preview_frame_observation(ctx, idx)
                .sample
                .active
        {
            return;
        }
        let ComparePreparationState::Ready(pair) = &self.compare_preparation else {
            return;
        };
        if pair.current_idx != idx || !matches!(pair.original, CompareOriginalVariant::Absent) {
            return;
        }
        let Some(slot) = &self.pinned_compare_slot else {
            return;
        };
        if slot.source_idx != pair.pinned_source_idx {
            return;
        }
        if matches!(slot.original_source, PinnedOriginalSource::Unavailable) {
            self.fail_compare_original(
                "比較画像の元画像がありません。比較画像を設定し直してください",
            );
            return;
        }
        let Some(current_raw) = self.current_raw_source_pixels(idx) else {
            self.ensure_original_material_for_entry(
                idx,
                crate::app::OriginalMaterialEntry::MaterialOnly,
            );
            return;
        };
        let current_geometry = pair.current_geometry;
        let expected_target_size = pair.target_size;
        let current_input_size = pair.current_input_size;
        let pinned_input_size = pair.pinned_input_size;
        let key = pair.key;
        let request = ComparePrepareRequest {
            current_idx: idx,
            pinned_source_idx: pair.pinned_source_idx,
            output: pair.pixels.output(),
        };
        let pinned_original = slot.original_source.clone();
        let pinned_processed = Arc::clone(&slot.pixels);
        let (tx, rx) = std::sync::mpsc::channel();
        let repaint_ctx = ctx.clone();
        let started_at = std::time::Instant::now();
        let thread = std::thread::Builder::new()
            .name("compare-original".into())
            .spawn(move || {
                let result = (|| {
                    let current_job = original_pixel_job(current_raw, current_geometry)?;
                    let pinned = match pinned_original {
                        PinnedOriginalSource::SameAsProcessed => pinned_processed,
                        PinnedOriginalSource::Raw {
                            pixels,
                            processed_source_size,
                            crop,
                            rotation,
                        } => {
                            let job = original_pixel_job(
                                pixels,
                                CompareSourceGeometry {
                                    processed_source_size,
                                    crop,
                                    rotation,
                                },
                            )?;
                            let (_, w, h, rgba) = crate::capture::run_compare_pixel_job(job)?;
                            Arc::new(egui::ColorImage::from_rgba_unmultiplied(
                                [w as usize, h as usize],
                                &rgba,
                            ))
                        }
                        PinnedOriginalSource::Unavailable => unreachable!(),
                    };
                    run_compare_pair_job(current_job, pinned, request, key, expected_target_size)
                })();
                let _ = tx.send(result);
                repaint_ctx.request_repaint();
            });
        match thread {
            Ok(_) => {
                let ComparePreparationState::Ready(pair) =
                    std::mem::take(&mut self.compare_preparation)
                else {
                    unreachable!();
                };
                self.compare_preparation = ComparePreparationState::PreparingOriginal {
                    pair,
                    pending: crate::app::ComparePreparePending {
                        key,
                        current_geometry,
                        request,
                        scope: crate::app::ComparePrepareScope::SinglePage,
                        source_indices: vec![idx],
                        current_input_size,
                        pinned_input_size,
                        expected_target_size,
                        started_at,
                        rx,
                    },
                };
                self.show_feedback_warning("元画像を準備中".to_string());
            }
            Err(error) => {
                self.fail_compare_original(&format!("比較元画像 worker を開始できません: {error}"))
            }
        }
    }

    fn fail_compare_original(&mut self, message: &str) {
        if let Some(pair) = self.compare_preparation.prepared_pair_mut() {
            pair.original = CompareOriginalVariant::Failed;
        }
        crate::logger::log(format!("compare original unavailable: {message}"));
        self.show_feedback_warning(message.to_string());
    }

    /// 本文・ナビゲータ・CPU fallback・インジケータが同じ frame の選択を共有する。
    pub(super) fn compare_original_for_frame(&self, ctx: &egui::Context, idx: usize) -> bool {
        if matches!(self.compare_view_mode, crate::app::CompareViewMode::Off)
            || !self.compare_prepared_pair_matches(idx)
        {
            return false;
        }
        let pair = self.compare_preparation.prepared_pair().unwrap();
        let id = egui::Id::unique(("compare_original_frame", ctx.viewport_id()));
        let identity = (ctx.cumulative_frame_nr(), pair.key, idx);
        if let Some((frame, key, index, original)) =
            ctx.data(|data| data.get_temp::<(u64, u64, usize, bool)>(id))
            && (frame, key, index) == identity
        {
            return original;
        }
        let original = self
            .original_preview_frame_observation(ctx, idx)
            .sample
            .active
            && matches!(pair.original, CompareOriginalVariant::Ready(_));
        ctx.data_mut(|data| data.insert_temp(id, (identity.0, identity.1, identity.2, original)));
        original
    }

    /// 実際に元画像を描いた frame だけ、編集overlayとその入力を抑止する。
    /// producer / readiness の通常bypass判定へ比較の描画版を逆流させない。
    pub(super) fn original_preview_visual_active_for_frame(
        &self,
        ctx: &egui::Context,
        idx: usize,
        ordinary_original: bool,
        primary_draw: CompareFramePrimaryDraw,
    ) -> bool {
        (primary_draw.draws_ordinary() && ordinary_original)
            || (primary_draw.draws_prepared_compare() && self.compare_original_for_frame(ctx, idx))
    }
}

/// 再標本化から crop / rotation まで worker 専用。色補正・conceal は持ち込まない。
pub(super) fn original_pixel_job(
    pixels: Arc<egui::ColorImage>,
    geometry: CompareSourceGeometry,
) -> Result<crate::capture::CapturePixelJob, String> {
    let [width, height] = geometry.processed_source_size;
    if width == 0
        || height == 0
        || pixels.size.contains(&0)
        || pixels.pixels.len() != pixels.size[0].saturating_mul(pixels.size[1])
    {
        return Err("比較元画像のサイズが不正です".to_string());
    }
    let source = if pixels.size == geometry.processed_source_size {
        pixels
    } else {
        let rgba = crate::capture::color_image_to_rgba(pixels.as_ref());
        let source = image::RgbaImage::from_raw(pixels.size[0] as u32, pixels.size[1] as u32, rgba)
            .ok_or_else(|| "比較元画像の RGBA サイズが不正です".to_string())?;
        let resized = crate::fast_resize::resize_rgba8_exact(
            &source,
            width as u32,
            height as u32,
            crate::fast_resize::Quality::Lanczos3,
        );
        Arc::new(egui::ColorImage::from_rgba_unmultiplied(
            [width, height],
            resized.as_raw(),
        ))
    };
    Ok(crate::capture::CapturePixelJob {
        basename: String::new(),
        source,
        conceal: None,
        crop: geometry.crop,
        rotation: geometry.rotation,
    })
}

pub(super) fn run_compare_pair_job(
    current_job: crate::capture::CapturePixelJob,
    pinned_pixels: Arc<egui::ColorImage>,
    request: ComparePrepareRequest,
    key: u64,
    expected_target_size: [usize; 2],
) -> Result<ComparePrepareResult, String> {
    let fs_idx = request.current_idx;
    let pinned_source_idx = request.pinned_source_idx;
    let output = request.output;
    if output == crate::app::ComparePrepareOutput::PinnedTexture {
        let width = expected_target_size[0] as u32;
        let height = expected_target_size[1] as u32;
        align_compare_color_image_to_canvas(pinned_pixels.as_ref(), expected_target_size).map(
            |pinned_rgba| crate::app::ComparePrepareResult {
                key,
                current_idx: fs_idx,
                pinned_source_idx,
                width,
                height,
                pixels: crate::app::ComparePreparedPixels::PinnedTexture(Arc::new(
                    egui::ColorImage::from_rgba_unmultiplied(expected_target_size, &pinned_rgba),
                )),
            },
        )
    } else {
        crate::capture::run_compare_pixel_job(current_job).and_then(
            |(_basename, current_width, current_height, current_source_rgba)| {
                let width = expected_target_size[0] as u32;
                let height = expected_target_size[1] as u32;
                let current_rgba = crate::capture::align_rgba_to_canvas_lanczos(
                    current_width,
                    current_height,
                    &current_source_rgba,
                    width,
                    height,
                    egui::Color32::BLACK,
                )?;
                let pinned_rgba = align_compare_color_image_to_canvas(
                    pinned_pixels.as_ref(),
                    expected_target_size,
                )?;
                let size = [width as usize, height as usize];
                let pixels = match output {
                    crate::app::ComparePrepareOutput::ShaderPair => {
                        crate::app::ComparePreparedPixels::ShaderPair {
                            pinned_rgba: Arc::new(pinned_rgba),
                            current_rgba: Arc::new(current_rgba),
                        }
                    }
                    crate::app::ComparePrepareOutput::PinnedTexture => {
                        crate::app::ComparePreparedPixels::PinnedTexture(Arc::new(
                            egui::ColorImage::from_rgba_unmultiplied(size, &pinned_rgba),
                        ))
                    }
                    crate::app::ComparePrepareOutput::WipeTextures => {
                        crate::app::ComparePreparedPixels::WipeTextures {
                            pinned: Arc::new(egui::ColorImage::from_rgba_unmultiplied(
                                size,
                                &pinned_rgba,
                            )),
                            current: Arc::new(egui::ColorImage::from_rgba_unmultiplied(
                                size,
                                &current_rgba,
                            )),
                        }
                    }
                    crate::app::ComparePrepareOutput::DiffTexture => {
                        let diff_rgba = crate::capture::diff_rgba_color(
                            width,
                            height,
                            &pinned_rgba,
                            &current_rgba,
                        )?;
                        crate::app::ComparePreparedPixels::DiffTexture(Arc::new(
                            egui::ColorImage::from_rgba_unmultiplied(size, &diff_rgba),
                        ))
                    }
                };
                Ok(crate::app::ComparePrepareResult {
                    key,
                    current_idx: fs_idx,
                    pinned_source_idx,
                    width,
                    height,
                    pixels,
                })
            },
        )
    }
}
