//! HUD reads job identity and the provenance of completed displayed material.
use super::tests::phase_c_support::{AppTestEnv, setup_app};
use super::*;

fn fixture(ctx: &egui::Context) -> AppTestEnv {
    fixture_with_source_size(ctx, [4, 6])
}

fn fixture_with_source_size(ctx: &egui::Context, source_size: [usize; 2]) -> AppTestEnv {
    let mut app = setup_app();
    app.items = vec![GridItem::Image(PathBuf::from("C:/hud/page.png"))];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.visible_indices = vec![0];
    app.settings.ai_feature_mode = crate::settings::AiFeatureMode::HighQuality;
    app.settings.global_preset.upscale_model = Some("realesr_general_v3".into());
    let pixels = Arc::new(egui::ColorImage::filled(source_size, egui::Color32::WHITE));
    let tex = ctx.load_texture(
        "hud-source",
        (*pixels).clone(),
        DISPLAY_IMAGE_TEXTURE_OPTIONS,
    );
    app.fs_cache.insert(
        0,
        FsCacheEntry::Static {
            pdf_rerender_failures: Vec::new(),
            pdf_metadata: None,
            tex,
            pixels,
            source_dims: Some(source_size),
            load_seq: 0,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    app
}

fn ai_key(app: &App) -> FinalAiKey {
    app.final_ai_key_for_pixels(
        app.current_edit_result_key(0),
        [4, 6],
        app.effective_params(0),
    )
    .unwrap()
}

fn pending() -> FinalAiPending {
    FinalAiPending {
        feedback_request: Default::default(),
        job_id: 1,
        cancel: Arc::new(AtomicBool::new(false)),
        retained_key: None,
        retained_epoch: 0,
        allow_retained_orphan: false,
        cancel_on_drop: true,
    }
}

fn ai_entry() -> FinalAiEntry {
    FinalAiEntry {
        diagnostic_metadata: None,
        canonical_dims: None,
        source_size: [4, 6],
        pixels: Arc::new(egui::ColorImage::filled([8, 12], egui::Color32::WHITE)),
        used_upscale: true,
    }
}

fn assert_job(app: &App, expected: FinalAiJobState, done: bool) {
    assert_eq!(app.final_ai_job_state(0), expected);
    assert_eq!(app.is_idx_final_ai_done_or_skipped(0), done);
}

#[test]
fn final_ai_status_job_states_preserve_done_or_skipped_and_exact_identity() {
    use FinalAiJobState as J;
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    assert_job(&app, J::Waiting, false);
    let key = ai_key(&app);
    let mut old_key = key;
    old_key.edit_key.source_gen = old_key.edit_key.source_gen.wrapping_add(1);
    app.final_ai_cache.insert(old_key, ai_entry());
    app.final_ai_pending.insert(old_key, pending());
    assert_job(&app, J::Waiting, false);
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        DisplayedFinalAiState::Waiting
    );
    app.final_ai_pending.insert(key, pending());
    assert_job(&app, J::Running, false);
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        DisplayedFinalAiState::Processing
    );
    app.final_ai_pending[&key]
        .cancel
        .store(true, Ordering::Relaxed);
    assert_job(&app, J::Waiting, false);
    app.final_ai_failed.insert(key);
    assert_job(&app, J::Failed, true);
    app.final_ai_cache.insert(key, ai_entry());
    assert_job(&app, J::Ready, true);
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        DisplayedFinalAiState::Processing
    );
    app.final_ai_cache.remove(&key);
    assert!(app.insert_retained_final_ai(0, key, [4, 6], ai_entry().pixels));
    assert_job(&app, J::Ready, true);
    app.settings.global_preset.upscale_model = None;
    assert_job(&app, J::NotRequested, true);
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        DisplayedFinalAiState::NotRequested
    );
    app.fs_cache.remove(&0);
    assert_job(&app, J::SourcePending, false);
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        DisplayedFinalAiState::NotRequested
    );
}

#[test]
fn final_ai_status_displayed_uses_completed_provenance_and_actual_texture_dimensions() {
    use DisplayedFinalAiState as D;
    use NotAppliedReason as R;
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let final_key = app.final_composite_key_for_pixels(
        app.current_edit_result_key(0),
        [4, 6],
        app.effective_params(0),
    );
    let pixels = Arc::new(egui::ColorImage::filled([16, 24], egui::Color32::WHITE));
    let tex = ctx.load_texture(
        "hud-clamped-final",
        egui::ColorImage::filled([7, 9], egui::Color32::WHITE),
        DISPLAY_IMAGE_TEXTURE_OPTIONS,
    );
    app.bind_display_texture_identity(&tex, final_key.edit_key.item_id);
    let mut entry = FinalCompositeEntry::raster(
        None,
        [4, 6],
        pixels,
        tex,
        DISPLAY_IMAGE_TEXTURE_OPTIONS,
        true,
    );
    entry.ai_applied = Some(FinalAiStages {
        denoise: false,
        upscale: true,
    });
    app.final_composite_cache.insert(final_key, entry);
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        D::Applied {
            width: 7,
            height: 9,
            stages: FinalAiStages {
                denoise: false,
                upscale: true
            }
        }
    );
    let source = app.fs_cache.remove(&0).unwrap();
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        D::Applied {
            width: 7,
            height: 9,
            stages: FinalAiStages {
                denoise: false,
                upscale: true
            }
        }
    );
    app.final_composite_cache
        .get_mut(&final_key)
        .unwrap()
        .complete = false;
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        D::Waiting
    );
    let mut entry = app.final_composite_cache.remove(&final_key).unwrap();
    entry.complete = true;
    let mut stale_key = final_key;
    stale_key.params_hash ^= 1;
    app.final_composite_cache.insert(stale_key, entry);
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        D::Waiting
    );
    let entry = app.final_composite_cache.remove(&stale_key).unwrap();
    app.final_composite_cache.insert(final_key, entry);
    app.final_composite_cache
        .get_mut(&final_key)
        .unwrap()
        .ai_applied = None;
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        D::NotApplied(R::Unavailable)
    );
    // Restore the source to let the existing failed-key owner be consulted.
    app.fs_cache.insert(0, source);
    let key = ai_key(&app);
    app.final_ai_failed.insert(key);
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        D::NotApplied(R::Failed)
    );
    app.final_composite_cache.clear();
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        D::NotApplied(R::Failed)
    );
}

#[test]
fn final_ai_status_size_limit_animation_and_source_waiting() {
    use DisplayedFinalAiState as D;
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.settings.ai_upscale_size_limit = Some(crate::ai::upscale::AiProcessSizeLimit::square(1));
    assert_job(&app, FinalAiJobState::OutOfRange, true);
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        D::NotApplied(NotAppliedReason::SizeLimit)
    );
    app.fs_cache.insert(
        0,
        FsCacheEntry::Animated {
            frames: Vec::new(),
            current_frame: 0,
            frame_pixels: Vec::new(),
            playback: crate::fs_animation::AnimationPlayback::Paused,
            load_seq: 0,
        },
    );
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        D::NotApplied(NotAppliedReason::Animated)
    );
    app.settings.global_preset.upscale_model = None;
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        D::NotRequested
    );
    app.settings.global_preset.upscale_model = Some("realesr_general_v3".into());
    app.fs_cache.remove(&0);
    assert_job(&app, FinalAiJobState::SourcePending, false);
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        D::Waiting
    );
}

#[test]
fn final_ai_status_original_display_overrides_ready_and_completed_ai() {
    use DisplayedFinalAiState as D;
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let key = ai_key(&app);
    app.final_ai_cache.insert(key, ai_entry());
    assert_eq!(app.final_ai_job_state(0), FinalAiJobState::Ready);
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        D::Processing
    );
    app.analysis_mode = true;
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, true)),
        D::NotApplied(NotAppliedReason::OriginalShown)
    );
    app.analysis_mode = false;
    let _ = app.ensure_final_composite_texture(&ctx, 0).unwrap();
    assert!(matches!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
        D::Applied { .. }
    ));
    // Original comparison has the same display bypass, with no real key input.
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, true)),
        D::NotApplied(NotAppliedReason::OriginalShown)
    );
    app.settings.global_preset.upscale_model = None;
    assert_eq!(
        app.displayed_final_ai_state(0, app.fs_display_stage(0, true)),
        D::NotRequested
    );
}

#[test]
fn final_ai_status_stage_admission_is_shared_with_key_and_worker_selection() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.settings.global_preset.denoise_model =
        Some(crate::ai::ModelKind::DenoiseRealplksr.as_str().into());
    app.settings.ai_upscale_size_limit = Some(crate::ai::upscale::AiProcessSizeLimit::square(1024));
    app.settings.ai_denoise_size_limit = Some(crate::ai::upscale::AiProcessSizeLimit::square(4096));
    let params = app.effective_params(0);
    for (size, expected) in [
        (
            // Limits are exclusive (`should_process_rect`), so stay below 1024.
            [1000, 1000],
            FinalAiStages {
                denoise: true,
                upscale: true,
            },
        ),
        (
            [1200, 1600],
            FinalAiStages {
                denoise: true,
                upscale: false,
            },
        ),
        (
            [5000, 5000],
            FinalAiStages {
                denoise: false,
                upscale: false,
            },
        ),
    ] {
        assert_eq!(app.final_ai_stages_in_range(params, size), expected);
        assert_eq!(
            app.final_ai_key_for_pixels(app.current_edit_result_key(0), size, params)
                .is_some(),
            expected.denoise || expected.upscale
        );
    }
    let source = egui::ColorImage::filled([1200, 1600], egui::Color32::WHITE);
    let selected = crate::ai::final_pipeline::select_final_ai_models(
        &source,
        params,
        app.settings.ai_feature_mode,
        app.settings.ai_upscale_limit(),
        app.settings.ai_denoise_limit(),
        None,
    )
    .unwrap();
    assert_eq!(
        selected.denoise,
        Some(crate::ai::ModelKind::DenoiseRealplksr)
    );
    assert_eq!(selected.upscale, None);
    // A result that did not upscale cannot acquire an upscale label later.
    assert_eq!(
        app.final_ai_result_stages(params, [1200, 1600], false),
        FinalAiStages {
            denoise: true,
            upscale: false
        }
    );
    assert_eq!(
        app.final_ai_result_stages(params, [4, 6], false),
        FinalAiStages {
            denoise: true,
            upscale: false
        }
    );
}

fn finish_effect_worker(app: &mut App, ctx: &egui::Context, ai_applied: bool) {
    let final_key = *app.final_effect_pending.keys().next().unwrap();
    let pending = app.final_effect_pending.get_mut(&final_key).unwrap();
    assert_eq!(pending.ai_applied.is_some(), ai_applied);
    let FinalEffectState::Computing(FinalEffectReceiver::Legacy(rx)) = &pending.state else {
        panic!("legacy worker expected")
    };
    let result = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
    pending.state = FinalEffectState::CpuReady(FinalEffectReady::Legacy(result));
    app.poll_final_effects(ctx);
}

#[test]
fn final_ai_status_composite_records_selected_ai_base_for_sync_and_worker_paths() {
    for worker in [false, true] {
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        if worker {
            app.settings.global_preset.colorize.mode = crate::colorize::ColorizeMode::AllImages;
        }
        let key = ai_key(&app);
        app.final_ai_cache.insert(key, ai_entry());
        app.ensure_final_composite_texture(&ctx, 0);
        if worker {
            finish_effect_worker(&mut app, &ctx, true);
        }
        assert!(
            app.exact_complete_final_composite_entry(0)
                .unwrap()
                .1
                .ai_applied
                .is_some()
        );
        assert!(matches!(
            app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
            DisplayedFinalAiState::Applied {
                width: 8,
                height: 12,
                stages: FinalAiStages {
                    denoise: false,
                    upscale: true
                }
            }
        ));
    }
}

#[test]
fn final_ai_status_denoise_only_result_records_input_range_through_final_effect() {
    for worker in [false, true] {
        let ctx = egui::Context::default();
        let mut app = fixture_with_source_size(&ctx, [1200, 1600]);
        app.settings.global_preset.denoise_model =
            Some(crate::ai::ModelKind::DenoiseRealplksr.as_str().into());
        app.settings.ai_upscale_size_limit =
            Some(crate::ai::upscale::AiProcessSizeLimit::square(1024));
        app.settings.ai_denoise_size_limit =
            Some(crate::ai::upscale::AiProcessSizeLimit::square(4096));
        if worker {
            app.settings.global_preset.colorize.mode = crate::colorize::ColorizeMode::AllImages;
        }
        let key = app
            .final_ai_key_for_pixels(
                app.current_edit_result_key(0),
                [1200, 1600],
                app.effective_params(0),
            )
            .unwrap();
        let expected = Some(FinalAiStages {
            denoise: true,
            upscale: false,
        });
        app.final_ai_cache.insert(
            key,
            FinalAiEntry {
                diagnostic_metadata: None,
                canonical_dims: None,
                source_size: [1200, 1600],
                pixels: ai_entry().pixels,
                used_upscale: false,
            },
        );
        app.ensure_final_composite_texture(&ctx, 0);
        if worker {
            let pending = app.final_effect_pending.values().next().unwrap();
            assert_eq!(pending.ai_applied, expected);
            finish_effect_worker(&mut app, &ctx, true);
        }
        assert_eq!(
            app.exact_complete_final_composite_entry(0)
                .unwrap()
                .1
                .ai_applied,
            expected
        );
        assert!(matches!(
            app.displayed_final_ai_state(0, app.fs_display_stage(0, false)),
            DisplayedFinalAiState::Applied {
                stages: FinalAiStages {
                    denoise: true,
                    upscale: false
                },
                ..
            }
        ));
    }
}

#[test]
fn final_ai_status_composite_failed_and_no_key_bases_are_not_ai_applied() {
    for worker in [false, true] {
        for failed in [false, true] {
            let ctx = egui::Context::default();
            let mut app = fixture(&ctx);
            if worker {
                app.settings.global_preset.colorize.mode = crate::colorize::ColorizeMode::AllImages;
            }
            if failed {
                let key = ai_key(&app);
                app.final_ai_failed.insert(key);
            } else {
                app.settings.global_preset.upscale_model = None;
            }
            app.ensure_final_composite_texture(&ctx, 0);
            if worker {
                finish_effect_worker(&mut app, &ctx, false);
            }
            assert!(
                !app.exact_complete_final_composite_entry(0)
                    .unwrap()
                    .1
                    .ai_applied
                    .is_some()
            );
        }
    }
}
