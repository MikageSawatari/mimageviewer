use super::*;
use crate::fs_exclusive_view::FsExclusiveView;

fn assert_new_fields(snapshot: &StateSnapshot, viewer: bool, video: bool) {
    let value = snapshot.to_dynamic().cast::<rhai::Map>();
    let view = value["view"].clone().cast::<rhai::Map>();
    let media = value["video"].clone().cast::<rhai::Map>();
    let grid = value["grid"].clone().cast::<rhai::Map>();
    for key in ["depth", "slideshow"] {
        assert_eq!(view[key].is_unit(), !viewer, "view.{key}");
    }
    for key in ["muted", "volume_db"] {
        assert_eq!(media[key].is_unit(), !video, "video.{key}");
    }
    for key in ["columns", "details"] {
        assert_eq!(grid[key].is_unit(), viewer, "grid.{key}");
    }
    assert_eq!(value["api"].as_int().unwrap(), 1);
}

#[test]
fn script_363_snapshot_scopes_depth_slideshow_and_grid_to_the_input_face() {
    let mut app = grid_app("[]");
    app.settings.grid_cols = 8;
    app.settings.grid_view_mode = crate::settings::GridViewMode::Details;
    app.fullscreen_idx = Some(0);
    app.fs_exclusive_view = FsExclusiveView::Depth(crate::depth_parallax::DepthViewState::new(
        app.edit_request_owner_context(),
    ));
    app.slideshow_playing = true;
    let grid = app.collect_operation_script_snapshot(ScriptInputSurface::Grid);
    assert_new_fields(&grid, false, false);
    assert_eq!(grid.grid_columns, Some(8));
    assert_eq!(grid.grid_details, Some(true));
    for presentation in [
        ViewerPresentation::Fullscreen,
        ViewerPresentation::DetachedWindow,
    ] {
        // The snapshot surface, rather than the window's decoration, owns these values.
        app.viewer_presentation = presentation;
        let viewer = app.collect_operation_script_snapshot(ScriptInputSurface::Viewer);
        assert_new_fields(&viewer, true, false);
        assert_eq!(viewer.view.depth, Some(true));
        assert_eq!(viewer.view.slideshow, Some(true));
    }
    app.fs_exclusive_view = FsExclusiveView::None;
    app.slideshow_playing = false;
    let viewer = app.collect_operation_script_snapshot(ScriptInputSurface::Viewer);
    assert_eq!(viewer.view.depth, Some(false));
    assert_eq!(viewer.view.slideshow, Some(false));
    app.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
    let grid = app.collect_operation_script_snapshot(ScriptInputSurface::Grid);
    assert_eq!(grid.grid_details, Some(false));
    assert_eq!(grid.grid_columns, Some(8));
}

#[cfg(windows)]
#[test]
fn script_363_snapshot_cached_video_volume_defaults_minimum_and_muted_are_independent() {
    for music in [false, true] {
        let mut app = video_app("[]", music);
        let snapshot = app.collect_operation_script_snapshot(ScriptInputSurface::Viewer);
        assert_new_fields(&snapshot, true, true);
        assert_eq!(snapshot.video.volume_db, Some(0.0));
        assert_eq!(snapshot.video.muted, Some(false));
        let crate::app::FsCacheEntry::Video { player, .. } = app.fs_cache.get_mut(&0).unwrap()
        else {
            unreachable!()
        };
        player.set_volume(0.0);
        player.set_muted(true);
        let snapshot = app.collect_operation_script_snapshot(ScriptInputSurface::Viewer);
        assert_eq!(snapshot.video.volume_db, Some(-80.0));
        assert_eq!(snapshot.video.muted, Some(true));
        video_player(&app, 0).set_volume(crate::settings::VIDEO_VOLUME_MAX);
        let snapshot = app.collect_operation_script_snapshot(ScriptInputSurface::Viewer);
        assert!((snapshot.video.volume_db.unwrap() - 18.0).abs() < 1.0e-9);
        assert_eq!(snapshot.video.muted, Some(true));
        let grid = app.collect_operation_script_snapshot(ScriptInputSurface::Grid);
        assert_new_fields(&grid, false, false);
        app.fs_cache.clear();
        let snapshot = app.collect_operation_script_snapshot(ScriptInputSurface::Viewer);
        assert_new_fields(&snapshot, true, false);
    }
}

#[cfg(windows)]
mod gpu_tests {
    use super::*;

    fn fixture(ctx: &egui::Context) -> AppTestEnv {
        let mut app = grid_app("[]");
        crate::app::depth_view_tests::install_pages(&mut app, ctx);
        let model_path = app.tmp.path().join("not-executed.onnx");
        app.depth_setup
            .set_status_for_test(crate::addon::AddonStatus::Valid {
                model_id: "script-363-fixture".into(),
                bytes: 1,
                model_path,
                mmproj_path: None,
            });
        app
    }

    fn keys(app: &mut App, ctx: &egui::Context, keys: &[egui::Key]) {
        let events = keys
            .iter()
            .flat_map(|key| {
                [true, false].map(|pressed| egui::Event::Key {
                    key: *key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                })
            })
            .collect();
        ctx.run_pass(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                let idx = app.fullscreen_idx.unwrap();
                app.handle_fs_key_input(ctx, idx, false);
            },
        )
        .drop_without_applying_deltas();
        crate::key_input::clear_test_frame();
    }

    #[test]
    fn script_363_depth_and_slideshow_share_actual_key_admission_and_bodies() {
        let _serial = crate::key_input::lock_test_input();
        for initial_depth in [false, true] {
            for initial_slideshow in [false, true] {
                for action in [KeyAction::FsDepthParallax, KeyAction::FsSlideshow] {
                    let mut outcomes = Vec::new();
                    for from_key in [true, false] {
                        let ctx = egui::Context::default();
                        let mut app = fixture(&ctx);
                        if initial_depth {
                            app.fs_exclusive_view =
                                FsExclusiveView::Depth(crate::depth_parallax::DepthViewState::new(
                                    app.edit_request_owner_context(),
                                ));
                        }
                        app.slideshow_playing = initial_slideshow;
                        if from_key {
                            app.keymap = crate::keymap::Keymap::from_ini_str(&format!(
                                "[FsImage]\n{} = F16\n",
                                action.ini_name()
                            ));
                            keys(&mut app, &ctx, &[egui::Key::F16]);
                        } else {
                            let (result, rejected, delegated) =
                                app.execute_fullscreen_script_action(&ctx, action);
                            assert!(!rejected && !delegated);
                            assert_eq!(result.page_transition, FsKeyPageTransition::Unchanged);
                        }
                        outcomes.push((
                            app.fs_exclusive_view.is_depth(),
                            app.slideshow_playing,
                            app.depth_setup.modal_open(),
                        ));
                    }
                    assert_eq!(
                        outcomes[0], outcomes[1],
                        "{action:?}, depth={initial_depth}, slideshow={initial_slideshow}"
                    );
                    assert_eq!(
                        outcomes[0],
                        (
                            initial_depth ^ (action == KeyAction::FsDepthParallax),
                            initial_slideshow ^ (action == KeyAction::FsSlideshow),
                            false,
                        )
                    );
                }
            }
        }
    }

    #[test]
    fn script_363_slot_starts_and_stops_depth_and_slideshow_in_one_invocation() {
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        app.operation_script.state = ready(r#"["FsDepthParallax","FsSlideshow"]"#);
        let result = app.run_script_slot(&ctx, 1, ScriptInputSurface::Viewer, false);
        assert!(!result.rejected() && !result.needs_postprocessing());
        assert!(app.fs_exclusive_view.is_depth() && app.slideshow_playing);
        app.operation_script.state = ready(r#"["FsSlideshow","FsDepthParallax"]"#);
        let result = app.run_script_slot(&ctx, 1, ScriptInputSurface::Viewer, false);
        assert!(!result.rejected() && !result.needs_postprocessing());
        assert!(!app.fs_exclusive_view.is_depth() && !app.slideshow_playing);
    }

    #[test]
    fn script_363_same_frame_s_v_keeps_360_slideshow_suppressed() {
        let _serial = crate::key_input::lock_test_input();
        for reverse_events in [false, true] {
            let ctx = egui::Context::default();
            let mut app = fixture(&ctx);
            app.fullscreen_idx = Some(2);
            app.apply_panorama_mode_toggle(2);
            assert!(app.is_panorama_mode_active(2));
            let input = if reverse_events {
                [egui::Key::V, egui::Key::S]
            } else {
                [egui::Key::S, egui::Key::V]
            };
            keys(&mut app, &ctx, &input);
            assert!(!app.is_panorama_mode_active(2));
            assert!(!app.slideshow_playing, "S must use the pre-V admission");
        }
    }

    #[test]
    fn script_363_same_frame_d_s_and_following_pixel_grid_observe_d_immediately() {
        let _serial = crate::key_input::lock_test_input();
        for initial_depth in [false, true] {
            let ctx = egui::Context::default();
            let mut app = fixture(&ctx);
            if initial_depth {
                app.fs_exclusive_view = FsExclusiveView::Depth(
                    crate::depth_parallax::DepthViewState::new(app.edit_request_owner_context()),
                );
            }
            app.fs_pixel_grid_enabled = false;
            keys(&mut app, &ctx, &[egui::Key::D, egui::Key::S, egui::Key::G]);
            assert_eq!(app.fs_exclusive_view.is_depth(), !initial_depth);
            assert!(
                app.slideshow_playing,
                "D and S coexist in one handler frame"
            );
            assert_eq!(
                app.fs_pixel_grid_enabled, initial_depth,
                "G must see the post-D pano_active_now"
            );
        }
    }

    #[test]
    fn script_363_duplicate_key_assignments_keep_consume_order_even_if_s_is_suppressed() {
        let _serial = crate::key_input::lock_test_input();
        for panorama in [false, true] {
            let ctx = egui::Context::default();
            let mut app = fixture(&ctx);
            if panorama {
                app.fullscreen_idx = Some(2);
                app.apply_panorama_mode_toggle(2);
            }
            // Slideshow consumes first; suppression must not give the edge back to D.
            app.keymap = crate::keymap::Keymap::from_ini_str(
                "[FsImage]\nFsSlideshow=F16\nFsDepthParallax=F16\n",
            );
            keys(&mut app, &ctx, &[egui::Key::F16]);
            assert!(!app.fs_exclusive_view.is_depth());
            assert_eq!(app.slideshow_playing, !panorama);
            assert_eq!(app.is_panorama_mode_active(2), panorama);
        }
    }

    #[test]
    fn script_363_missing_depth_model_modal_rejects_remaining_slideshow_and_notice() {
        let ctx = egui::Context::default();
        let mut app = grid_app(
            r##"#{run:["FsDepthParallax","FsSlideshow","FsNavigatorToggle"],notice:"must not complete"}"##,
        );
        crate::app::depth_view_tests::install_pages(&mut app, &ctx);
        app.viewer_presentation = ViewerPresentation::Fullscreen;
        app.depth_setup
            .set_status_for_test(crate::addon::AddonStatus::Missing);
        let navigator = app.settings.fullscreen_navigator_visible;
        let result = app.run_script_slot(&ctx, 1, ScriptInputSurface::Viewer, false);
        assert!(app.depth_setup.confirming_for_test());
        assert!(app.common_modal_dialog_open());
        assert!(result.rejected());
        assert!(!result.needs_postprocessing());
        assert!(!app.fs_exclusive_view.is_depth());
        assert!(!app.slideshow_playing);
        assert_eq!(app.settings.fullscreen_navigator_visible, navigator);
        assert!(toast(&app).contains("FsSlideshow"));
        assert!(!toast(&app).contains("must not complete"));
        let snapshot = app.collect_operation_script_snapshot(ScriptInputSurface::Viewer);
        assert_eq!(snapshot.view.depth, Some(false));
    }

    #[cfg(windows)]
    #[test]
    fn script_363_image_actions_remain_rejected_on_video_and_music_handlers() {
        let _serial = crate::key_input::lock_test_input();
        for music in [false, true] {
            for action in [KeyAction::FsDepthParallax, KeyAction::FsSlideshow] {
                let mut app = video_app("[]", music);
                let ctx = egui::Context::default();
                app.keymap = crate::keymap::Keymap::from_ini_str(&format!(
                    "[FsImage]\n{}=F16\n",
                    action.ini_name()
                ));
                ctx.run_pass(
                    egui::RawInput {
                        events: [true, false]
                            .map(|pressed| egui::Event::Key {
                                key: egui::Key::F16,
                                physical_key: None,
                                pressed,
                                repeat: false,
                                modifiers: egui::Modifiers::NONE,
                            })
                            .to_vec(),
                        ..Default::default()
                    },
                    |ctx| {
                        app.handle_fs_key_input(ctx, 0, false);
                    },
                )
                .drop_without_applying_deltas();
                crate::key_input::clear_test_frame();
                let (_, rejected, delegated) = app.execute_fullscreen_script_action(&ctx, action);
                assert!(rejected && !delegated);
                assert!(!app.fs_exclusive_view.is_depth());
                assert!(!app.slideshow_playing);
                assert!(!app.depth_setup.modal_open());
            }
        }
    }
}

#[test]
fn script_363_panorama_refusal_keeps_completed_prefix_and_discards_remainder() {
    let ctx = egui::Context::default();
    let mut app = grid_app(r#"["FsNavigatorToggle","FsSlideshow","FsNavigatorToggle"]"#);
    app.fullscreen_idx = Some(0);
    let state =
        crate::panorama::PanoramaState::new(0.0, 0.0, crate::panorama::PanoProjection::Perspective);
    app.fs_exclusive_view = FsExclusiveView::Panorama(state);
    let source_key = app.metadata_cache_key(0).unwrap();
    app.xmp_panorama_info.insert(
        source_key,
        Some(crate::xmp_reader::XmpPanoramaInfo {
            projection_type: Some("equirectangular".into()),
            use_panorama_viewer: Some(true),
            ..Default::default()
        }),
    );
    let navigator = app.settings.fullscreen_navigator_visible;
    let result = app.run_script_slot(&ctx, 1, ScriptInputSurface::Viewer, false);
    assert!(result.rejected());
    assert_ne!(app.settings.fullscreen_navigator_visible, navigator);
    assert!(!app.slideshow_playing);
    assert!(toast(&app).contains("FsSlideshow"));
}

#[test]
fn script_363_event_return_rejects_depth_and_slideshow_before_any_dispatch() {
    use super::super::events::capture_event_executions;
    for action in ["FsDepthParallax", "FsSlideshow"] {
        let ctx = egui::Context::default();
        let mut app = grid_app("[]");
        app.fullscreen_idx = Some(0);
        app.spread_mode = crate::settings::SpreadMode::Single;
        app.operation_script.state = OperationScriptState::Ready(Box::new(
            CompiledOperationScript::compile(&format!(
                "fn slots() {{ [] }} fn on_event(s) {{ [\"FsNavigatorToggle\",\"{action}\"] }}"
            ))
            .unwrap(),
        ));
        let navigator = app.settings.fullscreen_navigator_visible;
        let (_, executions) = capture_event_executions(|| {
            ctx.run_pass(egui::RawInput::default(), |ctx| {
                app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer);
            })
            .drop_without_applying_deltas();
        });
        assert_eq!(executions.len(), 1);
        assert_eq!(executions[0].0, 0);
        assert_eq!(app.settings.fullscreen_navigator_visible, navigator);
        assert!(!app.fs_exclusive_view.is_depth());
        assert!(!app.slideshow_playing);
        assert!(toast(&app).contains(action));
    }
}
