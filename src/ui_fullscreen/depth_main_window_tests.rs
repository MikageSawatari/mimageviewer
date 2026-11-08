//! Check the second eligibility boundary after the real ROOT viewer entry succeeded.
use super::*;
use crate::app::depth_main_window_tests::RootDepthViewer;
use crate::fs_exclusive_view::FsExclusiveView;

#[test]
fn depth_main_window_root_predraw_rechecks_eligibility_after_initial_admission() {
    let _input = crate::key_input::lock_test_input();
    for case in [
        "eligible",
        "video",
        "animation",
        "spread",
        "continuous",
        "editing",
    ] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        viewer.ready(&mut app);
        viewer.frame(&mut app, vec![]);
        assert_eq!(
            viewer.callbacks().len(),
            1,
            "actual ROOT viewer positive control"
        );
        let payload = FullscreenPaintPayload::from(
            app.ensure_final_composite_material(&viewer.ctx, 0).unwrap(),
        );
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, viewer.size);
        let mut depth_painted = false;
        let output = viewer.ctx.run_pass(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                assert_eq!(ctx.viewport_id(), egui::ViewportId::ROOT);
                assert!(
                    app.depth_view_may_continue(0),
                    "initial pass admission must succeed before {case}"
                );
                match case {
                    "video" => app.items[0] = GridItem::Video("late-video.mp4".into()),
                    "animation" => {
                        let Some(FsCacheEntry::Static { animation, .. }) = app.fs_cache.get_mut(&0)
                        else {
                            unreachable!()
                        };
                        *animation = crate::fs_animation::StaticAnimationState::FirstFrameOnly(
                            crate::canonical_image_loader::CanonicalAnimatedFormat::Gif,
                        );
                    }
                    "spread" => {
                        app.items[1] = GridItem::Image("late-spread.jpg".into());
                        crate::app::source_texture_test_support::seed(&mut app, ctx, 1);
                        app.spread_mode = crate::settings::SpreadMode::Ltr;
                        assert!(matches!(
                            app.resolve_visible_spread_pair(0),
                            SpreadPair::Double { .. }
                        ));
                    }
                    "continuous" => app.reading_flow = crate::settings::ReadingFlow::Vertical,
                    "editing" => app.export_crop_mode = true,
                    _ => {}
                }
                ctx.root_ui(|root_ui| {
                    egui::CentralPanel::default().show(root_ui, |ui| {
                        assert!(
                            app.draw_fs_image_with_receipt(
                                ui,
                                screen,
                                0,
                                Some(egui::vec2(400.0, 600.0)),
                                FsPageLayoutSource::CurrentItem,
                                None,
                                None,
                                SingletonSpreadPlacement::Center,
                                Some(&payload),
                                None,
                                false,
                                false,
                                false,
                                crate::rotation_db::Rotation::None,
                                None,
                                0.0,
                                &FsBgStyle::Solid(egui::Color32::BLACK),
                                "",
                                false,
                                FullscreenFitMode::Page,
                                FullscreenFitScaleLimits::default(),
                                None,
                                None,
                                false,
                                None,
                                page_turn::FlatCropPaint::WithPage
                            )
                            .is_some()
                        );
                    });
                });
                depth_painted = app.capture_depth_export(ctx, true).is_some();
            },
        );
        let callback = output
            .shapes
            .iter()
            .any(|s| matches!(s.shape, egui::Shape::Callback(_)));
        assert_eq!(
            depth_painted,
            case == "eligible",
            "{case}: predraw must not submit stale Depth"
        );
        if depth_painted {
            assert!(
                callback,
                "successful Depth receipt must accompany its callback"
            );
        }
        assert_eq!(
            matches!(app.fs_exclusive_view, FsExclusiveView::Depth(_)),
            case == "eligible",
            "{case}"
        );
        output.drop_without_applying_deltas();
    }
    crate::key_input::clear_test_frame();
}
