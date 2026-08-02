//! 静止画の最終合成で App / remote adapter が共有する純粋な解決・CPU 実行層。
//!
//! source の選択・編集結果の materialize・final AI・cache / worker ownership・GPU upload は
//! adapter 側の責務とし、この module は解決済みの CPU 段だけを受け持つ。

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use eframe::egui;

/// ページ個別、現在地の標準、global の順で有効パラメータを解決する。
///
/// App の index・お気に入り探索・DB には依存せず、各 adapter が取得済みの値だけを渡す。
///
/// 現在地の標準は closure で受ける。ページ個別があるときに祖先お気に入りを探索しないのは
/// 呼び出し側の性能契約であり、この関数が毎フレーム呼ばれる経路にいる以上ここで守る。
pub(crate) fn resolve_effective_params<'a>(
    page: Option<&'a crate::adjustment::AdjustParams>,
    location_default: impl FnOnce() -> Option<&'a crate::adjustment::AdjustParams>,
    global_default: &'a crate::adjustment::AdjustParams,
) -> &'a crate::adjustment::AdjustParams {
    page.or_else(location_default).unwrap_or(global_default)
}

/// 選択・materialize 済み source に残っている最終 CPU 段の実行計画。
///
/// 適用順は `tone -> smart sharpen -> colorize -> Creative LUT -> post_filter`。
/// final AI は tone と smart sharpen の間にある独立 worker/cache 層なので含めない。
/// source が raw か edit result かという選択も adapter が executor 呼び出し前に確定する。
#[derive(Clone)]
pub(crate) struct FinalCompositePlan {
    /// AI を使わない先読みなど、source がまだ色調補正前の経路だけ `Some`。
    pub(crate) adjust_before_effect: Option<crate::adjustment::AdjustParams>,
    /// AI の実行結果 (`used_upscale`) まで解決した有効強度。
    pub(crate) smart_sharpen: u8,
    pub(crate) colorize: crate::colorize::ColorizeParams,
    /// 選択 id ではなく、adapter が解決済みの immutable LUT と強度。
    pub(crate) creative_lut: Option<(crate::creative_lut::SharedCreativeLut, f32)>,
    pub(crate) post_filter: crate::adjustment::PostFilter,
}

impl FinalCompositePlan {
    pub(crate) fn needs_nearest_sampler(&self) -> bool {
        self.post_filter.needs_nearest_sampler()
    }
}

pub(crate) enum FinalCompositeResult {
    Ready {
        pixels: Arc<egui::ColorImage>,
        elapsed_ms: f64,
        timing: FinalCompositeTiming,
    },
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FinalCompositeTiming {
    pub(crate) adjust_ms: f64,
    pub(crate) sharpen_ms: f64,
    pub(crate) colorize_check_ms: f64,
    pub(crate) colorize_apply_ms: f64,
    pub(crate) creative_lut_ms: f64,
    pub(crate) post_filter_ms: f64,
    pub(crate) colorize_applied: bool,
    pub(crate) colorize_mode: crate::colorize::ColorizeMode,
    pub(crate) tone_method: crate::colorize::ToneDensityMethod,
    pub(crate) tone_radius: f32,
}

/// `FinalCompositePlan` を source pixels へ適用する共有 CPU executor。
///
/// この関数内の段順、cancel 境界、`should_apply` 判定は表示互換性の一部である。
pub(crate) fn execute_final_composite(
    source: Arc<egui::ColorImage>,
    plan: FinalCompositePlan,
    cancel: &AtomicBool,
) -> FinalCompositeResult {
    let started = std::time::Instant::now();
    let mut timing = FinalCompositeTiming {
        colorize_mode: plan.colorize.mode,
        tone_method: plan.colorize.tone_method,
        tone_radius: plan.colorize.tone_radius,
        ..FinalCompositeTiming::default()
    };
    if cancel.load(Ordering::Relaxed) {
        return FinalCompositeResult::Cancelled;
    }
    let stage_started = std::time::Instant::now();
    let adjusted = if let Some(params) = plan.adjust_before_effect.as_ref() {
        Arc::new(crate::adjustment::apply_adjustments_fast(&source, params))
    } else {
        source
    };
    timing.adjust_ms = stage_started.elapsed().as_secs_f64() * 1000.0;
    if cancel.load(Ordering::Relaxed) {
        return FinalCompositeResult::Cancelled;
    }
    let stage_started = std::time::Instant::now();
    let sharpened = if plan.smart_sharpen == 0 {
        adjusted
    } else {
        Arc::new(crate::adjustment::apply_final_smart_sharpen(
            &adjusted,
            plan.smart_sharpen,
        ))
    };
    timing.sharpen_ms = stage_started.elapsed().as_secs_f64() * 1000.0;
    if cancel.load(Ordering::Relaxed) {
        return FinalCompositeResult::Cancelled;
    }
    let stage_started = std::time::Instant::now();
    timing.colorize_applied = crate::colorize::should_apply(&sharpened, &plan.colorize);
    timing.colorize_check_ms = stage_started.elapsed().as_secs_f64() * 1000.0;
    let stage_started = std::time::Instant::now();
    let colorized = if timing.colorize_applied {
        match crate::colorize::apply_applicable_with_cancel(&sharpened, &plan.colorize, cancel) {
            Some(image) => Arc::new(image),
            None => return FinalCompositeResult::Cancelled,
        }
    } else {
        sharpened
    };
    timing.colorize_apply_ms = stage_started.elapsed().as_secs_f64() * 1000.0;
    if cancel.load(Ordering::Relaxed) {
        return FinalCompositeResult::Cancelled;
    }
    let stage_started = std::time::Instant::now();
    let lut_applied = if let Some((lut, strength)) = plan.creative_lut {
        Arc::new(crate::creative_lut::apply_to_color_image(
            &colorized, &lut, strength,
        ))
    } else {
        colorized
    };
    timing.creative_lut_ms = stage_started.elapsed().as_secs_f64() * 1000.0;
    if cancel.load(Ordering::Relaxed) {
        return FinalCompositeResult::Cancelled;
    }
    let stage_started = std::time::Instant::now();
    let pixels = if plan.post_filter != crate::adjustment::PostFilter::None {
        Arc::new(crate::post_filter::apply(&lut_applied, plan.post_filter))
    } else {
        lut_applied
    };
    timing.post_filter_ms = stage_started.elapsed().as_secs_f64() * 1000.0;
    if cancel.load(Ordering::Relaxed) {
        FinalCompositeResult::Cancelled
    } else {
        FinalCompositeResult::Ready {
            pixels,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            timing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params_with_brightness(brightness: f32) -> crate::adjustment::AdjustParams {
        crate::adjustment::AdjustParams {
            brightness,
            ..crate::adjustment::AdjustParams::default()
        }
    }

    #[test]
    fn resolver_prefers_page_then_location_then_global() {
        let global = params_with_brightness(5.0);
        let location = params_with_brightness(20.0);
        let page = params_with_brightness(50.0);

        assert!(std::ptr::eq(
            resolve_effective_params(None, || None, &global),
            &global
        ));
        assert!(std::ptr::eq(
            resolve_effective_params(None, || Some(&location), &global),
            &location
        ));
        assert!(std::ptr::eq(
            resolve_effective_params(Some(&page), || Some(&location), &global),
            &page
        ));
    }

    /// ページ個別がある idx では、現在地の標準を引く仕事そのものを起こさない。
    /// App 側の探索は `PathBuf` 確保と祖先お気に入り走査を伴い、毎フレーム走る。
    #[test]
    fn resolver_does_not_look_up_the_location_default_when_the_page_has_its_own() {
        let global = params_with_brightness(5.0);
        let page = params_with_brightness(50.0);
        let mut looked_up = false;

        let resolved = resolve_effective_params(
            Some(&page),
            || {
                looked_up = true;
                None
            },
            &global,
        );

        assert!(std::ptr::eq(resolved, &page));
        assert!(!looked_up);
    }

    #[test]
    fn executor_matches_the_pre_move_stage_order_pixel_for_pixel() {
        let source = Arc::new(egui::ColorImage::new(
            [4, 3],
            (0..12)
                .map(|i| {
                    egui::Color32::from_rgb(
                        (17 + i * 19) as u8,
                        (231 - i * 13) as u8,
                        (43 + i * 11) as u8,
                    )
                })
                .collect(),
        ));
        let adjust = crate::adjustment::AdjustParams {
            brightness: 11.0,
            contrast: 17.0,
            gamma: 0.85,
            saturation: 23.0,
            temperature: -9.0,
            black_point: 4,
            white_point: 244,
            midtone: 1.15,
            ..crate::adjustment::AdjustParams::default()
        };
        let colorize = crate::colorize::ColorizeParams::legacy_all_images(
            crate::colorize::ColorizePalette::LegacySkin,
        );
        let lut = Arc::new(
            local_adjust_core::parse_cube_lut(
                "LUT_3D_SIZE 2\n1 1 1\n0 1 1\n1 0 1\n0 0 1\n1 1 0\n0 1 0\n1 0 0\n0 0 0\n",
                "stage-order-invert",
            )
            .expect("valid test LUT"),
        );
        let plan = FinalCompositePlan {
            adjust_before_effect: Some(adjust.clone()),
            smart_sharpen: 37,
            colorize: colorize.clone(),
            creative_lut: Some((Arc::clone(&lut), 0.65)),
            post_filter: crate::adjustment::PostFilter::WarmTone,
        };
        // 移動前の `run_final_effect_job` と同じ逐次式を test oracle として固定する。
        let adjusted = Arc::new(crate::adjustment::apply_adjustments_fast(&source, &adjust));
        let sharpened = Arc::new(crate::adjustment::apply_final_smart_sharpen(&adjusted, 37));
        assert!(crate::colorize::should_apply(&sharpened, &colorize));
        let colorized = Arc::new(
            crate::colorize::apply_applicable_with_cancel(
                &sharpened,
                &colorize,
                &AtomicBool::new(false),
            )
            .expect("uncancelled colorize"),
        );
        let lut_applied = Arc::new(crate::creative_lut::apply_to_color_image(
            &colorized, &lut, 0.65,
        ));
        let expected =
            crate::post_filter::apply(&lut_applied, crate::adjustment::PostFilter::WarmTone);
        let result = execute_final_composite(source, plan, &AtomicBool::new(false));
        let FinalCompositeResult::Ready { pixels, timing, .. } = result else {
            panic!("uncancelled executor must complete");
        };
        assert!(timing.colorize_applied);
        assert_eq!(pixels.size, expected.size);
        assert_eq!(pixels.pixels, expected.pixels);
    }
}
