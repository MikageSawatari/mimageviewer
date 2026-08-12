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

/// Select the deepest ancestor favorite that owns adjustment defaults.
///
/// App supplies an index-derived path; remote supplies the request path.
/// `has_default` keeps storage ownership in each adapter.
pub(crate) fn active_favorite_default_id_for_path(
    path: &std::path::Path,
    favorites: &[crate::settings::FavoriteEntry],
    excluded_id: Option<uuid::Uuid>,
    has_default: impl Fn(uuid::Uuid) -> bool,
) -> Option<uuid::Uuid> {
    let mut best: Option<uuid::Uuid> = None;
    let mut best_len = 0usize;
    for favorite in favorites {
        if excluded_id == Some(favorite.id)
            || !has_default(favorite.id)
            || !crate::search_index_db::is_under(path, &favorite.path)
        {
            continue;
        }
        let len = favorite.path.as_os_str().len();
        if best.is_none() || len > best_len {
            best = Some(favorite.id);
            best_len = len;
        }
    }
    best
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
    /// 直前の rendition が出した適用可否。`MonochromeOnly` の判定は走査が重いので、
    /// 同じ画像で答えが出ているときは再計算しない (master の final effect と同じ規則)。
    pub(crate) colorize_applicable_override: Option<bool>,
    /// 選択 id ではなく、adapter が解決済みの immutable LUT と強度。
    pub(crate) creative_lut: Option<(crate::creative_lut::SharedCreativeLut, f32)>,
    pub(crate) post_filter: crate::adjustment::PostFilter,
}

impl FinalCompositePlan {
    pub(crate) fn needs_nearest_sampler(&self) -> bool {
        self.post_filter.needs_nearest_sampler()
    }
}

/// Build the final CPU plan when the adapter does not execute final AI.
///
/// App uses this for its non-AI route and remote always uses it in stage 1.
/// Smart sharpen therefore follows `effective_smart_sharpen(false)`.
/// `MonochromeOnly` の適用可否だけは呼び出し側の答えを優先する。走査が重く、同じ画像に対して
/// 直前の rendition が既に出しているため。他のモードは常にこの場で判定する。
pub(crate) fn final_composite_colorize_applies(
    colorize: &crate::colorize::ColorizeParams,
    applicable_override: Option<bool>,
    source: &egui::ColorImage,
) -> bool {
    match colorize.mode {
        crate::colorize::ColorizeMode::MonochromeOnly => {
            applicable_override.unwrap_or_else(|| crate::colorize::should_apply(source, colorize))
        }
        _ => crate::colorize::should_apply(source, colorize),
    }
}

pub(crate) fn build_final_composite_plan_without_ai(
    params: &crate::adjustment::AdjustParams,
    creative_lut: Option<(crate::creative_lut::SharedCreativeLut, f32)>,
) -> FinalCompositePlan {
    FinalCompositePlan {
        adjust_before_effect: (!params.is_color_identity()).then(|| params.clone()),
        smart_sharpen: params.effective_smart_sharpen(false),
        colorize: params.colorize.clone(),
        colorize_applicable_override: None,
        creative_lut,
        post_filter: params.post_filter,
    }
}

/// Build the remaining final CPU plan after final AI has consumed the tone stage.
/// `used_upscale` is part of smart-sharpen's effective-value rule and therefore must be
/// supplied from the actual executor result, not inferred again from requested settings.
pub(crate) fn build_final_composite_plan_after_ai(
    params: &crate::adjustment::AdjustParams,
    creative_lut: Option<(crate::creative_lut::SharedCreativeLut, f32)>,
    used_upscale: bool,
) -> FinalCompositePlan {
    FinalCompositePlan {
        adjust_before_effect: None,
        smart_sharpen: params.effective_smart_sharpen(used_upscale),
        colorize: params.colorize.clone(),
        colorize_applicable_override: None,
        creative_lut,
        post_filter: params.post_filter,
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
    timing.colorize_applied = final_composite_colorize_applies(
        &plan.colorize,
        plan.colorize_applicable_override,
        &sharpened,
    );
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
    fn plan_without_ai_matches_the_app_non_ai_formula() {
        let mut params = params_with_brightness(12.0);
        params.smart_sharpen = 63;
        params.post_filter = crate::adjustment::PostFilter::WarmTone;
        params.colorize = crate::colorize::ColorizeParams::legacy_all_images(
            crate::colorize::ColorizePalette::LegacySkin,
        );
        let lut = Arc::new(
            local_adjust_core::parse_cube_lut(
                "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
                "identity",
            )
            .expect("valid test LUT"),
        );

        let plan = build_final_composite_plan_without_ai(&params, Some((Arc::clone(&lut), 0.4)));

        assert_eq!(plan.adjust_before_effect.as_ref(), Some(&params));
        assert_eq!(
            plan.smart_sharpen,
            params.effective_smart_sharpen(false),
            "remote without AI must match App with used_upscale=false"
        );
        assert_eq!(plan.colorize, params.colorize);
        let (actual_lut, strength) = plan.creative_lut.expect("resolved LUT");
        assert!(Arc::ptr_eq(&actual_lut, &lut));
        assert_eq!(strength, 0.4);
        assert_eq!(plan.post_filter, params.post_filter);
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
            colorize_applicable_override: None,
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
