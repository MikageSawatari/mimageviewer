#[cfg(windows)]
mod compare_original_tests {
    use super::*;
    use crate::app::{
        CompareOriginalVariant, ComparePreparationState, ComparePrepareOutput,
        ComparePreparePending, ComparePrepareRequest, ComparePrepareResult, ComparePreparedPair,
        ComparePreparedPixels, CompareSourceGeometry, PinnedOriginalSource,
    };

    fn geometry(size: [usize; 2]) -> CompareSourceGeometry {
        CompareSourceGeometry {
            processed_source_size: size,
            crop: None,
            rotation: crate::rotation_db::Rotation::None,
        }
    }

    fn ready_app(
        ctx: &egui::Context,
        mode: crate::app::CompareViewMode,
    ) -> crate::app::AppTestEnvForTest {
        let mut app = setup_navigation_readiness_app(2);
        app.settings.global_preset.colorize.mode = crate::colorize::ColorizeMode::Disabled;
        insert_navigation_original_page(&mut app, ctx, 0);
        insert_navigation_original_page(&mut app, ctx, 1);
        install_ready_pair(&mut app, mode);
        app
    }

    fn install_ready_pair(app: &mut App, mode: crate::app::CompareViewMode) {
        app.compare_view_mode = mode;
        let source = Arc::new(egui::ColorImage::filled([2, 2], egui::Color32::WHITE));
        app.pinned_compare_slot = Some(crate::app::PinnedCompareSlot {
            original_source: PinnedOriginalSource::Raw {
                pixels: app.current_raw_source_pixels(1).unwrap(),
                processed_source_size: [2, 2],
                crop: None,
                rotation: crate::rotation_db::Rotation::None,
            },
            pixels: source.clone(),
            indicator_pixels: source.clone(),
            indicator_texture: None,
            display_name: "original-test".into(),
            source_idx: 1,
            external_item_key: None,
            source_size: [2, 2],
        });
        let request = ComparePrepareRequest {
            current_idx: 0,
            pinned_source_idx: 1,
            output: ComparePrepareOutput::for_mode(mode, false).unwrap(),
        };
        let key = app.next_compare_prepared_key();
        let pixels = compare_original::run_compare_pair_job(
            crate::capture::CapturePixelJob::already_adjusted(String::new(), source.clone()),
            source,
            request,
            key,
            [2, 2],
        )
        .unwrap()
        .pixels;
        app.compare_preparation = ComparePreparationState::Ready(ComparePreparedPair {
            key,
            current_idx: 0,
            pinned_source_idx: 1,
            current_input_size: [2, 2],
            pinned_input_size: [2, 2],
            target_size: [2, 2],
            pixels,
            pinned_texture: None,
            current_texture: None,
            diff_texture: None,
            current_geometry: geometry([2, 2]),
            original: CompareOriginalVariant::Absent,
        });
    }

    fn hold(app: &mut crate::app::AppTestEnvForTest) {
        arm_reassigned_original_preview_hold(app, crate::key_input::SyntheticNavigationKey::Home);
    }

    fn next_frame(ctx: &egui::Context) {
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
    }

    fn finish(app: &mut App, ctx: &egui::Context) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while matches!(
            app.compare_preparation,
            ComparePreparationState::PreparingOriginal { .. }
                | ComparePreparationState::Preparing(_)
                | ComparePreparationState::Draining(_)
        ) {
            app.poll_compare_prepare_pending(ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "compare worker did not complete"
            );
            std::thread::yield_now();
        }
    }

    fn controlled_pending(
        app: &mut App,
    ) -> std::sync::mpsc::Sender<Result<ComparePrepareResult, String>> {
        let ComparePreparationState::Ready(pair) = std::mem::take(&mut app.compare_preparation)
        else {
            panic!("ready fixture");
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let pending = ComparePreparePending {
            key: pair.key,
            current_geometry: pair.current_geometry,
            request: ComparePrepareRequest {
                current_idx: pair.current_idx,
                pinned_source_idx: pair.pinned_source_idx,
                output: pair.pixels.output(),
            },
            scope: crate::app::ComparePrepareScope::SinglePage,
            source_indices: vec![0],
            current_input_size: pair.current_input_size,
            pinned_input_size: pair.pinned_input_size,
            expected_target_size: pair.target_size,
            started_at: std::time::Instant::now(),
            rx,
        };
        app.compare_preparation = ComparePreparationState::PreparingOriginal { pair, pending };
        tx
    }

    fn result_for(app: &App) -> ComparePrepareResult {
        let pair = app.compare_preparation.prepared_pair().unwrap();
        compare_original::run_compare_pair_job(
            crate::capture::CapturePixelJob::already_adjusted(
                String::new(),
                Arc::new(egui::ColorImage::filled([2, 2], egui::Color32::BLUE)),
            ),
            app.pinned_compare_slot.as_ref().unwrap().pixels.clone(),
            app.compare_preparation.request().unwrap(),
            pair.key,
            pair.target_size,
        )
        .unwrap()
    }

    #[test]
    fn compare_original_hold_prepares_once_and_release_reuses_processed_in_all_modes() {
        let _serial = crate::key_input::lock_test_input();
        let _clear = ClearOriginalPreviewSyntheticInput;
        for mode in [
            crate::app::CompareViewMode::wipe(0.37),
            crate::app::CompareViewMode::Diff,
            crate::app::CompareViewMode::PinnedNormal,
        ] {
            let ctx = egui::Context::default();
            let mut app = ready_app(&ctx, mode);
            let key = app.compare_preparation.prepared_pair().unwrap().key;
            hold(&mut app);
            assert!(!app.original_preview_active(&ctx, 0));
            assert!(!app.original_preview_active_for_frame(&ctx, 0));
            assert!(!app.fs_display_bypasses_final_pipeline(
                app.original_preview_active_for_frame(&ctx, 0)
            ));
            assert!(app.ensure_compare_prepared_pair(&ctx, 0));
            assert!(matches!(
                app.compare_preparation,
                ComparePreparationState::PreparingOriginal { .. }
            ));
            assert!(app.compare_preparation.is_pending());
            assert!(!app.compare_original_for_frame(&ctx, 0));
            assert_eq!(app.compare_preparation.prepared_pair().unwrap().key, key);
            assert_eq!(app.compare_view_mode, mode);
            assert_eq!(app.feedback_toasts_for_test.len(), 1);
            assert_eq!(app.feedback_toasts_for_test[0].0, "元画像を準備中");
            finish(&mut app, &ctx);
            // The choice stays processed until the next frame, including navigator/indicator calls.
            assert!(!app.compare_original_for_frame(&ctx, 0));
            next_frame(&ctx);
            assert!(app.compare_original_for_frame(&ctx, 0));
            let pair = app.compare_preparation.prepared_pair().unwrap();
            let original_key = pair.draw_key(true);
            assert_ne!(original_key, key);
            assert!(pair.draw_pixels(true).valid_for_size(pair.target_size));
            crate::key_input::clear_test_synthetic_input();
            next_frame(&ctx);
            assert!(!app.compare_original_for_frame(&ctx, 0));
            assert_eq!(
                app.compare_preparation
                    .prepared_pair()
                    .unwrap()
                    .draw_key(false),
                key
            );
            hold(&mut app);
            next_frame(&ctx);
            assert!(app.ensure_compare_prepared_pair(&ctx, 0));
            assert!(matches!(
                app.compare_preparation,
                ComparePreparationState::Ready(_)
            ));
            assert!(app.compare_original_for_frame(&ctx, 0));
            assert_eq!(
                app.compare_preparation
                    .prepared_pair()
                    .unwrap()
                    .draw_key(true),
                original_key
            );
            assert_eq!(app.feedback_toasts_for_test.len(), 1);
            crate::key_input::clear_test_synthetic_input();
        }
    }

    #[test]
    fn compare_original_loupe_selects_current_frame_material_without_bypass_or_load() {
        let _serial = crate::key_input::lock_test_input();
        let _clear = ClearOriginalPreviewSyntheticInput;
        let ctx = egui::Context::default();
        let (mut app, _) = crate::app::composite_pixels_revision_tests::warm_final_pair(&ctx);
        app.spread_mode = SpreadMode::Single;
        app.fullscreen_idx = Some(0);
        insert_navigation_original_page(&mut app, &ctx, 0);
        insert_navigation_original_page(&mut app, &ctx, 1);
        install_ready_pair(&mut app, crate::app::CompareViewMode::wipe(0.5));
        let raw_id = app
            .resolve_original_preview_material(0)
            .unwrap()
            .source_texture_id();
        let processed_id = app
            .resolve_fs_processed_payload(&ctx, 0, false)
            .unwrap()
            .source_texture_id();
        assert_ne!(
            raw_id, processed_id,
            "fixture must distinguish raw and processed materials"
        );
        hold(&mut app);
        app.ensure_compare_prepared_pair(&ctx, 0);
        assert!(!app.compare_original_for_frame(&ctx, 0));
        assert_eq!(
            app.resolve_fs_loupe_page_payload(&ctx, 0, 0)
                .unwrap()
                .source_texture_id(),
            processed_id
        );
        finish(&mut app, &ctx);
        // Completion cannot change the material choice halfway through the frame.
        assert_eq!(
            app.resolve_fs_loupe_page_payload(&ctx, 0, 0)
                .unwrap()
                .source_texture_id(),
            processed_id
        );
        next_frame(&ctx);
        assert!(app.compare_original_for_frame(&ctx, 0));
        assert_eq!(
            app.resolve_fs_loupe_page_payload(&ctx, 0, 0)
                .unwrap()
                .source_texture_id(),
            raw_id
        );
        assert!(
            !app.fs_display_bypasses_final_pipeline(app.original_preview_active_for_frame(&ctx, 0))
        );
        assert!(app.fs_pending.is_empty());
        // Other sampled pages keep the existing processed selection during comparison.
        let other = app
            .resolve_fs_processed_payload(&ctx, 1, false)
            .map(|material| material.source_texture_id());
        assert_eq!(
            app.resolve_fs_loupe_page_payload(&ctx, 0, 1)
                .map(|material| material.source_texture_id()),
            other
        );
        app.fs_cache.remove(&0);
        assert!(app.resolve_fs_loupe_page_payload(&ctx, 0, 0).is_none());
        assert!(
            app.fs_pending.is_empty(),
            "loupe must not start MaterialOnly on eviction"
        );
        crate::key_input::clear_test_synthetic_input();
        next_frame(&ctx);
        assert!(!app.compare_original_for_frame(&ctx, 0));
        assert_eq!(
            app.resolve_fs_loupe_page_payload(&ctx, 0, 0)
                .unwrap()
                .source_texture_id(),
            processed_id
        );
    }

    #[test]
    fn compare_original_close_reopen_invalidates_both_variants_and_drains_before_next_worker() {
        let _serial = crate::key_input::lock_test_input();
        let _clear = ClearOriginalPreviewSyntheticInput;
        for mode in [
            crate::app::CompareViewMode::wipe(0.37),
            crate::app::CompareViewMode::Diff,
            crate::app::CompareViewMode::PinnedNormal,
        ] {
            // Ready before close, completion while closed, and still pending on reopen.
            for completion in 0..3 {
                let ctx = egui::Context::default();
                let (mut app, _) =
                    crate::app::composite_pixels_revision_tests::warm_final_pair(&ctx);
                app.spread_mode = SpreadMode::Single;
                app.fullscreen_idx = Some(0);
                insert_navigation_original_page(&mut app, &ctx, 0);
                insert_navigation_original_page(&mut app, &ctx, 1);
                install_ready_pair(&mut app, mode);
                let tx = controlled_pending(&mut app);
                let mut old_result = Some(result_for(&app));
                if completion == 0 {
                    tx.send(Ok(old_result.take().unwrap())).unwrap();
                    app.poll_compare_prepare_pending(&ctx);
                }
                let key = app.compare_preparation.prepared_pair().unwrap().key;
                app.close_fullscreen();
                assert_eq!(app.fullscreen_idx, None);
                assert_eq!(app.compare_view_mode, mode);
                assert_eq!(app.compare_preparation.prepared_pair().unwrap().key, key);
                assert!(app.current_raw_source_pixels(0).is_none());
                if completion != 0 {
                    assert!(matches!(
                        app.compare_preparation,
                        ComparePreparationState::PreparingOriginal { .. }
                    ));
                    if completion == 1 {
                        tx.send(Ok(old_result.take().unwrap())).unwrap();
                        app.poll_compare_prepare_pending(&ctx);
                    }
                }
                if completion != 2 {
                    assert!(matches!(
                        app.compare_preparation.prepared_pair().unwrap().original,
                        CompareOriginalVariant::Ready(_)
                    ));
                }
                let next_key = app.compare_prepared_next_key;
                app.open_fullscreen_with_materialization(
                    0,
                    crate::app::HistoryTrigger::AutoAdvance,
                    crate::app::FsOpenMaterialization::NavigationTargetMaterializationOnly,
                );
                assert_eq!(app.fullscreen_idx, Some(0));
                assert_eq!(app.compare_view_mode, mode);
                assert!(app.compare_preparation.prepared_pair().is_none());
                assert!(!app.compare_original_for_frame(&ctx, 0));
                if completion == 2 {
                    assert!(matches!(
                        app.compare_preparation,
                        ComparePreparationState::Draining(_)
                    ));
                    assert!(!app.ensure_compare_prepared_pair(&ctx, 0));
                    assert_eq!(app.compare_prepared_next_key, next_key);
                    assert!(matches!(
                        app.compare_preparation,
                        ComparePreparationState::Draining(_)
                    ));
                    tx.send(Ok(old_result.take().unwrap())).unwrap();
                    app.poll_compare_prepare_pending(&ctx);
                }
                assert!(matches!(
                    app.compare_preparation,
                    ComparePreparationState::Unprepared
                ));
                // Supply the completed current material for the new input generation.
                // The real open/close boundaries above must not be replaced by fixture resets.
                app.fs_pending.clear();
                crate::app::paged_prefetch_tests::retain_pdf_ai(&mut app, 0);
                app.ensure_final_composite_pixels_with_key(&ctx, 0);
                crate::app::paged_prefetch_tests::complete_pending_composites_for_display(
                    &mut app, &ctx,
                );
                assert!(!app.ensure_compare_prepared_pair(&ctx, 0));
                assert!(matches!(
                    app.compare_preparation,
                    ComparePreparationState::Preparing(_)
                ));
                finish(&mut app, &ctx);
                let pair = app.compare_preparation.prepared_pair().unwrap();
                assert_ne!(pair.key, key);
                assert_eq!(pair.key, next_key);
                assert!(matches!(pair.original, CompareOriginalVariant::Absent));
                hold(&mut app);
                next_frame(&ctx);
                assert!(!app.compare_original_for_frame(&ctx, 0));
                crate::key_input::clear_test_synthetic_input();
            }
        }
    }

    #[test]
    fn compare_original_pair_discard_routes_drain_before_next_worker() {
        let ctx = egui::Context::default();
        for route in 0..7 {
            let mut app = ready_app(&ctx, crate::app::CompareViewMode::wipe(0.5));
            let tx = controlled_pending(&mut app);
            let old_result = result_for(&app);
            match route {
                0 => app.compare_preparation.invalidate(),
                1 => app.invalidate_compare_prepared_for_idx(0),
                2 => app.invalidate_compare_prepared_for_idx(1),
                3 => app.deactivate_compare_view(),
                4 => app.invalidate_all_compare_prepared(),
                5 => app.clear_all_color_caches(),
                6 => {
                    app.fullscreen_idx = Some(1);
                    app.ensure_compare_prepared_pair(&ctx, 1);
                }
                _ => unreachable!(),
            }
            assert!(
                matches!(
                    app.compare_preparation,
                    ComparePreparationState::Draining(_)
                ),
                "route={route}"
            );
            assert!(app.compare_preparation.prepared_pair().is_none());
            app.compare_view_mode = crate::app::CompareViewMode::wipe(0.5);
            assert!(!app.ensure_compare_prepared_pair(&ctx, 1));
            assert!(matches!(
                app.compare_preparation,
                ComparePreparationState::Draining(_)
            ));
            tx.send(Ok(old_result)).unwrap();
            app.poll_compare_prepare_pending(&ctx);
            assert!(matches!(
                app.compare_preparation,
                ComparePreparationState::Unprepared
            ));
        }
    }

    #[test]
    fn compare_original_hidden_pinned_result_is_retained() {
        let ctx = egui::Context::default();
        let mut app = ready_app(&ctx, crate::app::CompareViewMode::PinnedNormal);
        let tx = controlled_pending(&mut app);
        let result = result_for(&app);
        app.hide_compare_pinned_view();
        assert!(matches!(
            app.compare_view_mode,
            crate::app::CompareViewMode::Off
        ));
        assert!(matches!(
            app.compare_preparation,
            ComparePreparationState::PreparingOriginal { .. }
        ));
        tx.send(Ok(result)).unwrap();
        app.poll_compare_prepare_pending(&ctx);
        assert!(matches!(
            app.compare_preparation.prepared_pair().unwrap().original,
            CompareOriginalVariant::Ready(_)
        ));
        assert_eq!(
            app.compare_preparation
                .prepared_pair()
                .unwrap()
                .managed_gpu_texture_count(),
            0
        );
        for (original, expected) in [(false, 1), (true, 2)] {
            let _ = app
                .ensure_compare_prepared_texture(&ctx, ComparePreparedTextureKind::Pinned, original)
                .unwrap();
            assert_eq!(
                app.compare_preparation
                    .prepared_pair()
                    .unwrap()
                    .managed_gpu_texture_count(),
                expected
            );
        }
        // Both hide/re-show logs use this same retained managed-texture count.
        app.toggle_compare_pinned_view(&ctx, 0);
        app.hide_compare_pinned_view();
        assert_eq!(
            app.compare_preparation
                .prepared_pair()
                .unwrap()
                .managed_gpu_texture_count(),
            2
        );
        app.toggle_compare_pinned_view(&ctx, 0);
        assert_eq!(
            app.compare_preparation
                .prepared_pair()
                .unwrap()
                .managed_gpu_texture_count(),
            2
        );
    }

    #[test]
    fn compare_original_rejects_stale_key_request_output_and_size_without_retry() {
        let ctx = egui::Context::default();
        for mismatch in 0..5 {
            let mut app = ready_app(&ctx, crate::app::CompareViewMode::wipe(0.5));
            let tx = controlled_pending(&mut app);
            let mut result = result_for(&app);
            match mismatch {
                0 => result.key += 99,
                1 => result.current_idx += 1,
                2 => result.pinned_source_idx += 1,
                3 => result.width += 1,
                4 => {
                    result.pixels = ComparePreparedPixels::DiffTexture(Arc::new(
                        egui::ColorImage::filled([2, 2], egui::Color32::BLACK),
                    ))
                }
                _ => unreachable!(),
            }
            tx.send(Ok(result)).unwrap();
            app.poll_compare_prepare_pending(&ctx);
            assert!(matches!(
                app.compare_preparation.prepared_pair().unwrap().original,
                CompareOriginalVariant::Failed
            ));
            assert_eq!(app.feedback_toasts_for_test.len(), 1);
            app.poll_compare_prepare_pending(&ctx);
            assert_eq!(app.feedback_toasts_for_test.len(), 1);
        }
    }

    #[test]
    fn compare_original_unavailable_and_failed_notify_once_and_keep_processed() {
        let _serial = crate::key_input::lock_test_input();
        let _clear = ClearOriginalPreviewSyntheticInput;
        for failure in 0..3 {
            let ctx = egui::Context::default();
            let mut app = ready_app(&ctx, crate::app::CompareViewMode::Diff);
            hold(&mut app);
            if failure == 0 {
                app.pinned_compare_slot.as_mut().unwrap().original_source =
                    PinnedOriginalSource::Unavailable;
                app.ensure_compare_prepared_pair(&ctx, 0);
            } else {
                let tx = controlled_pending(&mut app);
                if failure == 1 {
                    tx.send(Err("injected failure".into())).unwrap();
                }
                drop(tx);
                app.poll_compare_prepare_pending(&ctx);
            }
            assert!(matches!(
                app.compare_preparation.prepared_pair().unwrap().original,
                CompareOriginalVariant::Failed
            ));
            app.ensure_compare_prepared_pair(&ctx, 0);
            next_frame(&ctx);
            app.ensure_compare_prepared_pair(&ctx, 0);
            assert!(!app.compare_original_for_frame(&ctx, 0));
            assert_eq!(app.feedback_toasts_for_test.len(), 1);
            crate::key_input::clear_test_synthetic_input();
        }
    }

    #[test]
    fn compare_original_material_only_hydration_preserves_pair_and_starts_after_arrival() {
        let _serial = crate::key_input::lock_test_input();
        let _clear = ClearOriginalPreviewSyntheticInput;
        let ctx = egui::Context::default();
        let mut app = ready_app(&ctx, crate::app::CompareViewMode::wipe(0.5));
        hold(&mut app);
        app.fs_cache.remove(&0);
        let key = app.compare_preparation.prepared_pair().unwrap().key;
        assert!(app.ensure_compare_prepared_pair(&ctx, 0));
        assert!(matches!(
            app.compare_preparation,
            ComparePreparationState::Ready(_)
        ));
        assert_eq!(app.compare_preparation.prepared_pair().unwrap().key, key);
        assert_eq!(
            app.fs_pending.get(&0).unwrap().purpose,
            crate::app::FsLoadPurpose::Original {
                entry: crate::app::OriginalMaterialEntry::MaterialOnly
            }
        );
        assert!(app.feedback_toasts_for_test.is_empty());
        assert!(app.ensure_compare_prepared_pair(&ctx, 0));
        // Inject arrival through the real Original adoption / preserve_completed branch.
        app.fs_pending.remove(&0);
        let (tx, rx) = std::sync::mpsc::channel();
        app.fs_pending.insert(
            0,
            crate::app::FsPendingValue::new(
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
                rx,
                77,
                crate::app::FsLoadPurpose::Original {
                    entry: crate::app::OriginalMaterialEntry::MaterialOnly,
                },
            ),
        );
        tx.send(crate::fs_animation::FsLoadResult::Static {
            ci: egui::ColorImage::filled([2, 2], egui::Color32::GREEN),
            source_dims: [2, 2],
            animation: crate::fs_animation::StaticAnimationState::Still,
        })
        .unwrap();
        app.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
        assert_eq!(app.compare_preparation.prepared_pair().unwrap().key, key);
        assert!(app.current_raw_source_pixels(0).is_some());
        next_frame(&ctx);
        assert!(app.ensure_compare_prepared_pair(&ctx, 0));
        assert!(matches!(
            app.compare_preparation,
            ComparePreparationState::PreparingOriginal { .. }
        ));
        assert_eq!(app.compare_preparation.prepared_pair().unwrap().key, key);
        finish(&mut app, &ctx);
        assert_eq!(app.feedback_toasts_for_test.len(), 1);
    }

    #[test]
    fn compare_original_main_navigator_and_cpu_textures_share_frame_choice() {
        let _serial = crate::key_input::lock_test_input();
        let _clear = ClearOriginalPreviewSyntheticInput;
        let ctx = egui::Context::default();
        let mut app = ready_app(&ctx, crate::app::CompareViewMode::wipe(0.41));
        hold(&mut app);
        app.ensure_compare_prepared_pair(&ctx, 0);
        finish(&mut app, &ctx);
        next_frame(&ctx);
        let chosen = app.compare_original_for_frame(&ctx, 0);
        assert!(chosen);
        let original = app
            .ensure_compare_prepared_texture(&ctx, ComparePreparedTextureKind::Current, chosen)
            .unwrap();
        let processed = app
            .ensure_compare_prepared_texture(&ctx, ComparePreparedTextureKind::Current, false)
            .unwrap();
        assert_ne!(original.id(), processed.id());
        crate::key_input::clear_test_synthetic_input();
        // A later consumer in the same frame (navigator/indicator) must use the first choice.
        assert_eq!(app.compare_original_for_frame(&ctx, 0), chosen);
        next_frame(&ctx);
        assert!(!app.compare_original_for_frame(&ctx, 0));
        let again = app
            .ensure_compare_prepared_texture(&ctx, ComparePreparedTextureKind::Current, true)
            .unwrap();
        assert_eq!(again.id(), original.id());
    }

    #[test]
    fn compare_original_visual_gate_suppresses_edit_overlay_only_for_the_drawn_variant() {
        let _serial = crate::key_input::lock_test_input();
        let _clear = ClearOriginalPreviewSyntheticInput;
        let ctx = egui::Context::default();
        let mut app = ready_app(&ctx, crate::app::CompareViewMode::wipe(0.5));
        hold(&mut app);
        app.ensure_compare_prepared_pair(&ctx, 0);
        assert!(
            !app.original_preview_visual_active_for_frame(
                &ctx,
                0,
                true,
                CompareFramePrimaryDraw::PreparedCompareOnly
            ),
            "a stale ordinary flag must not label processed compare as original"
        );
        assert!(app.original_preview_visual_active_for_frame(
            &ctx,
            0,
            true,
            CompareFramePrimaryDraw::OrdinaryOnly
        ));
        assert!(!app.original_preview_visual_active_for_frame(
            &ctx,
            0,
            false,
            CompareFramePrimaryDraw::PreparedCompareOnly
        ));
        finish(&mut app, &ctx);
        next_frame(&ctx);
        assert!(app.original_preview_visual_active_for_frame(
            &ctx,
            0,
            false,
            CompareFramePrimaryDraw::PreparedCompareOnly
        ));
        assert!(!app.original_preview_visual_active_for_frame(
            &ctx,
            0,
            false,
            CompareFramePrimaryDraw::OrdinaryOnly
        ));
        assert!(
            !app.fs_display_bypasses_final_pipeline(app.original_preview_active_for_frame(&ctx, 0))
        );
        crate::key_input::clear_test_synthetic_input();
        next_frame(&ctx);
        assert!(!app.original_preview_visual_active_for_frame(
            &ctx,
            0,
            false,
            CompareFramePrimaryDraw::PreparedCompareOnly
        ));
    }

    #[test]
    fn compare_original_geometry_resamples_before_fractional_crop_and_rotation_without_adjustment()
    {
        let raw = Arc::new(egui::ColorImage::filled(
            [3, 5],
            egui::Color32::from_gray(40),
        ));
        let mut g = geometry([6, 10]);
        g.crop = Some(crate::export_crop::CropRect {
            min_x: 1.19,
            min_y: 1.7,
            max_x: 4.79,
            max_y: 8.6,
        });
        g.rotation = crate::rotation_db::Rotation::Cw90;
        let mut processed = compare_original::original_pixel_job(
            Arc::new(egui::ColorImage::filled(
                [6, 10],
                egui::Color32::from_gray(180),
            )),
            g,
        )
        .unwrap();
        processed.basename = "processed".into();
        let original = compare_original::original_pixel_job(raw, g).unwrap();
        assert_eq!(
            processed.output_size().unwrap(),
            original.output_size().unwrap()
        );
        assert_eq!(original.output_size().unwrap(), [8, 4]);
        let (_, pw, ph, p) = crate::capture::run_compare_pixel_job(processed).unwrap();
        let (_, ow, oh, o) = crate::capture::run_compare_pixel_job(original).unwrap();
        assert_eq!([pw, ph], [ow, oh]);
        assert!(o.chunks_exact(4).all(|px| px == [40, 40, 40, 255]));
        assert!(p.chunks_exact(4).all(|px| px == [180, 180, 180, 255]));
        let target = [19, 23];
        let align = |w, h, rgba: &[u8]| {
            crate::capture::align_rgba_to_canvas_lanczos(
                w,
                h,
                rgba,
                target[0],
                target[1],
                egui::Color32::BLACK,
            )
            .unwrap()
        };
        let p = align(pw, ph, &p);
        let o = align(ow, oh, &o);
        assert_eq!(p.len(), o.len());
        let occupied = |rgba: &[u8]| {
            rgba.chunks_exact(4)
                .map(|px| px[0] != 0)
                .collect::<Vec<_>>()
        };
        assert_eq!(occupied(&p), occupied(&o));
        // Pinned Raw uses the identical construction, then the shared pinned canvas alignment.
        let pinned = egui::ColorImage::from_rgba_unmultiplied(
            [ow as usize, oh as usize],
            &crate::capture::run_compare_pixel_job(
                compare_original::original_pixel_job(
                    Arc::new(egui::ColorImage::filled(
                        [3, 5],
                        egui::Color32::from_gray(40),
                    )),
                    g,
                )
                .unwrap(),
            )
            .unwrap()
            .3,
        );
        let pinned = align_compare_color_image_to_canvas(&pinned, [19, 23]).unwrap();
        assert_eq!(pinned, o);
    }

    #[test]
    fn compare_original_pin_routes_capture_raw_unavailable_and_external_sources() {
        let ctx = egui::Context::default();
        for grid in [false, true] {
            let (mut app, _) = crate::app::composite_pixels_revision_tests::warm_final_pair(&ctx);
            app.spread_mode = SpreadMode::Single;
            app.fullscreen_idx = Some(0);
            insert_navigation_original_page(&mut app, &ctx, 0);
            if grid {
                app.selected = Some(0);
                app.toggle_compare_pin_from_grid_selection(&ctx);
            } else {
                app.toggle_compare_pin_from_current(&ctx, 0);
            }
            let source = &app
                .compare_pin_pending
                .as_ref()
                .expect("pin started")
                .original_source;
            assert!(matches!(
                source,
                PinnedOriginalSource::Raw {
                    processed_source_size: [8, 12],
                    crop: None,
                    rotation: crate::rotation_db::Rotation::None,
                    ..
                }
            ));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while app.compare_pin_pending.is_some() {
                app.poll_compare_pin_pending(&ctx);
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
            assert!(matches!(
                app.pinned_compare_slot.as_ref().unwrap().original_source,
                PinnedOriginalSource::Raw { .. }
            ));
        }
        let (mut app, _) = crate::app::composite_pixels_revision_tests::warm_final_pair(&ctx);
        app.spread_mode = SpreadMode::Single;
        app.toggle_compare_pin_from_current(&ctx, 0);
        assert!(matches!(
            app.compare_pin_pending.as_ref().unwrap().original_source,
            PinnedOriginalSource::Unavailable
        ));
        drop(app);
        let mut app = ready_app(&ctx, crate::app::CompareViewMode::wipe(0.5));
        let tx = controlled_pending(&mut app);
        app.start_external_compare_pin_job(&ctx, "external-image".into(), || {
            Ok(crate::app::ComparePinResult {
                basename: "external".into(),
                width: 1,
                height: 1,
                rgba: vec![0, 0, 255, 255],
                indicator_width: 1,
                indicator_height: 1,
                indicator_rgba: vec![0, 0, 255, 255],
            })
        });
        assert!(matches!(
            app.compare_pin_pending.as_ref().unwrap().original_source,
            PinnedOriginalSource::SameAsProcessed
        ));
        assert!(matches!(
            app.compare_preparation,
            ComparePreparationState::Draining(_)
        ));
        drop(tx);
        finish(&mut app, &ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.compare_pin_pending.is_some() {
            app.poll_compare_pin_pending(&ctx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(matches!(
            app.pinned_compare_slot.as_ref().unwrap().original_source,
            PinnedOriginalSource::SameAsProcessed
        ));
    }

    #[test]
    fn compare_original_drained_old_result_cannot_enter_new_processed_pair() {
        let ctx = egui::Context::default();
        let (mut app, _) = crate::app::composite_pixels_revision_tests::warm_final_pair(&ctx);
        app.spread_mode = SpreadMode::Single;
        app.fullscreen_idx = Some(0);
        let source = Arc::new(egui::ColorImage::filled([8, 12], egui::Color32::WHITE));
        let key = app.next_compare_prepared_key();
        app.pinned_compare_slot = Some(crate::app::PinnedCompareSlot {
            original_source: PinnedOriginalSource::SameAsProcessed,
            pixels: source.clone(),
            indicator_pixels: source.clone(),
            indicator_texture: None,
            display_name: "pin".into(),
            source_idx: 2,
            external_item_key: None,
            source_size: [8, 12],
        });
        app.compare_view_mode = crate::app::CompareViewMode::PinnedNormal;
        app.compare_preparation = ComparePreparationState::Ready(ComparePreparedPair {
            key,
            current_idx: 0,
            pinned_source_idx: 2,
            current_input_size: [8, 12],
            pinned_input_size: [8, 12],
            target_size: [8, 12],
            pixels: ComparePreparedPixels::PinnedTexture(source),
            pinned_texture: None,
            current_texture: None,
            diff_texture: None,
            current_geometry: geometry([8, 12]),
            original: CompareOriginalVariant::Absent,
        });
        let tx = controlled_pending(&mut app);
        let result = result_for(&app);
        let slot = app.pinned_compare_slot.take().unwrap();
        app.invalidate_all_compare_prepared();
        assert!(!app.ensure_compare_prepared_pair(&ctx, 0));
        assert!(matches!(
            app.compare_preparation,
            ComparePreparationState::Draining(_)
        ));
        tx.send(Ok(result)).unwrap();
        app.poll_compare_prepare_pending(&ctx);
        app.pinned_compare_slot = Some(slot);
        app.compare_view_mode = crate::app::CompareViewMode::PinnedNormal;
        app.ensure_compare_prepared_pair(&ctx, 0);
        assert!(matches!(
            app.compare_preparation,
            ComparePreparationState::Preparing(_)
        ));
        finish(&mut app, &ctx);
        let pair = app.compare_preparation.prepared_pair().unwrap();
        assert_ne!(pair.key, key);
        assert!(matches!(pair.original, CompareOriginalVariant::Absent));
    }
}
