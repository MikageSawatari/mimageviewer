//! End-to-end input ownership and material adoption in real headless viewer passes.
use super::*;
use crate::fs_exclusive_view::FsExclusiveView;
use crate::plane_view::*;

mod gpu_tests {
    mod additional_coverage;
    mod transition_and_click_tests;
    use super::*;
    use crate::app::depth_main_window_tests::RootDepthViewer;
    fn viewer(app: &mut App) -> RootDepthViewer {
        let mut viewer = RootDepthViewer::new(app);
        viewer.ready(app);
        app.set_fs_exclusive_view(FsExclusiveView::None, "plane_fixture");
        app.settings.fullscreen_navigator_visible = false;
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
        app.settings.fullscreen_wheel_action = crate::settings::FullscreenWheelAction::PageTurn;
        app.settings.fullscreen_wheel_zoom_step =
            crate::settings::FullscreenWheelZoomStep::Percent10;
        viewer.frame(app, vec![]);
        viewer
    }
    fn wheel(x: f32, y: f32, mods: egui::Modifiers) -> egui::Event {
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(x, y),
            modifiers: mods,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        }
    }
    fn enter(app: &mut App, viewer: &mut RootDepthViewer) {
        viewer.frame(
            app,
            vec![
                egui::Event::PointerMoved(viewer.size.to_pos2() * 0.5),
                wheel(0., 40., egui::Modifiers::CTRL | egui::Modifiers::SHIFT),
            ],
        );
        assert_eq!(app.plane_view_pose().unwrap().strength, 0.125);
    }
    #[test]
    fn plane_export_size_choices_snapshot_dark() {
        let mut ready = false;
        let mut scale = crate::export_dialog::ExportScale::Full;
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(460., 230.))
            .build_ui(move |root| {
                crate::test_ui::full_viewport_ui(root, |root| {
                    crate::os_theme::apply_resolved(
                        root.ctx(),
                        crate::os_theme::ResolvedTheme::Dark,
                    );
                    if !ready {
                        crate::ui_fonts::configure_fonts(root.ctx());
                        ready = true;
                        root.ctx().request_repaint();
                        return;
                    }
                    egui::CentralPanel::default().show(root, |ui| {
                        ui.label("魚眼の見た目: 写真");
                        ui.label("出力サイズ");
                        draw_export_size_options(ui, &mut scale, |choice| {
                            let size = crate::plane_view_wgpu::resolve_plane_output_size(
                                egui::vec2(1200., 900.),
                                crate::plane_view_wgpu::PlaneOutputChoice::Export(choice),
                                &wgpu::Limits {
                                    max_texture_dimension_2d: 1024,
                                    ..Default::default()
                                },
                            );
                            [size[0] as usize, size[1] as usize]
                        });
                        ui.separator();
                        ui.label("魚眼の見た目を書き出します");
                    });
                });
            });
        harness.run();
        harness.snapshot("plane_export_size_choices_dark");
    }

    fn key(key: egui::Key) -> Vec<egui::Event> {
        [true, false]
            .into_iter()
            .map(|pressed| egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            })
            .collect()
    }

    #[test]
    fn plane_key_stage_precedes_wheel_regardless_of_raw_order() {
        let _keys = crate::key_input::lock_test_input();
        for wheel_first in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            let mut viewer = viewer(&mut app);
            app.keymap = crate::keymap::Keymap::from_ini_str(
                "[FsImage]\nFsFisheyeIn = F13\nFsFisheyeOut = F14\n",
            );
            let mut events = key(egui::Key::F13);
            let page = wheel(0., -40., egui::Modifiers::NONE);
            if wheel_first {
                events.insert(0, page);
            } else {
                events.push(page);
            }
            viewer.frame(&mut app, events);
            assert_eq!(app.fullscreen_idx, Some(1));
            let entered = app
                .plane_view_pose()
                .expect("key stage ran on the old still page");
            assert_eq!(entered.anchor.fs_idx, 0);
            assert_eq!(entered.strength, 0.125);
            // Navigation commits at frame tail, after the old page was painted. The
            // next pass must retire its anchor before painting the newly selected video.
            viewer.frame(&mut app, vec![]);
            assert_eq!(app.fullscreen_idx, Some(1));
            assert!(!app.fs_exclusive_view.is_plane_view());
            app.fullscreen_idx = Some(0);
            app.fs_zoom = 1.;
            app.fs_pan = egui::Vec2::ZERO;
            let mut events = key(egui::Key::F13);
            let out = wheel(0., -80., egui::Modifiers::CTRL | egui::Modifiers::SHIFT);
            if wheel_first {
                events.insert(0, out);
            } else {
                events.push(out);
            }
            viewer.frame(&mut app, events);
            assert!(
                !app.fs_exclusive_view.is_plane_view(),
                "the key enters first and the downward wheel exits it"
            );
            assert!((app.fs_zoom - 1.).abs() < 1.0e-5);
        }
    }

    #[test]
    fn plane_real_fractional_segments_match_and_downward_key_only_spends_strength() {
        let _keys = crate::key_input::lock_test_input();
        let run = |split: bool| {
            let mut app = crate::app::setup_app_for_test();
            let mut viewer = viewer(&mut app);
            let mods = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
            for _ in 0..if split { 4 } else { 1 } {
                viewer.frame(
                    &mut app,
                    vec![
                        egui::Event::PointerMoved(egui::pos2(500., 300.)),
                        wheel(0., if split { 10. } else { 40. }, mods),
                    ],
                );
            }
            (
                app.fs_zoom,
                app.fs_pan,
                app.plane_view_pose().unwrap().strength,
            )
        };
        let a = run(false);
        let b = run(true);
        assert!((a.0 - b.0).abs() < 1.0e-5);
        assert!((a.1 - b.1).length() < 0.01);
        assert_eq!(a.2, b.2);
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        app.keymap = crate::keymap::Keymap::from_ini_str(
            "[FsImage]\nFsFisheyeIn = F13\nFsFisheyeOut = F14\n",
        );
        viewer.frame(&mut app, key(egui::Key::F13));
        assert_eq!(
            app.plane_view_pose().unwrap().lens_center,
            egui::Vec2::splat(0.5)
        );
        viewer.frame(&mut app, key(egui::Key::F14));
        assert!(!app.fs_exclusive_view.is_plane_view());
        let zoom = app.fs_zoom;
        viewer.frame(&mut app, key(egui::Key::F14));
        assert_eq!(app.fs_zoom, zoom);
    }

    #[test]
    fn plane_wheel_respects_popup_modal_and_panel_admission() {
        let _keys = crate::key_input::lock_test_input();
        for case in ["popup", "modal", "panel"] {
            let mut app = crate::app::setup_app_for_test();
            let mut viewer = viewer(&mut app);
            let pos = if case == "panel" {
                egui::pos2(2., 300.)
            } else {
                egui::pos2(500., 300.)
            };
            match case {
                "popup" => app.fit_popup_open = true,
                "modal" => app.show_about_dialog = true,
                _ => {}
            }
            viewer.frame(
                &mut app,
                vec![
                    egui::Event::PointerMoved(pos),
                    wheel(0., 40., egui::Modifiers::CTRL | egui::Modifiers::SHIFT),
                ],
            );
            assert!(!app.fs_exclusive_view.is_plane_view(), "{case}");
        }
    }

    #[test]
    fn plane_actual_entry_checks_reject_incompatible_modes_without_clearing_them() {
        let _keys = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        assert_eq!(app.plane_view_entry_allowed(0), Ok(()));
        app.view_trim_mode = true;
        assert_eq!(
            app.plane_view_entry_allowed(0),
            Err(PlaneViewEntryBlock::Trim)
        );
        assert!(app.view_trim_mode);
        app.view_trim_mode = false;
        app.analysis_mode = true;
        assert_eq!(
            app.plane_view_entry_allowed(0),
            Err(PlaneViewEntryBlock::Analysis)
        );
        app.analysis_mode = false;
        app.compare_view_mode = crate::app::CompareViewMode::PinnedNormal;
        assert_eq!(
            app.plane_view_entry_allowed(0),
            Err(PlaneViewEntryBlock::Compare)
        );
        app.compare_view_mode = crate::app::CompareViewMode::Off;
        app.reading_flow = crate::settings::ReadingFlow::Vertical;
        assert_eq!(
            app.plane_view_entry_allowed(0),
            Err(PlaneViewEntryBlock::Continuous)
        );
        app.reading_flow = crate::settings::ReadingFlow::Paged;
        app.fullscreen_idx = Some(1);
        assert_eq!(
            app.plane_view_entry_allowed(1),
            Err(PlaneViewEntryBlock::NotImage)
        );
        app.fullscreen_idx = Some(0);
        enter(&mut app, &mut viewer);
        assert!(app.plane_view_may_continue(0));
        app.view_trim_mode = true;
        assert!(!app.plane_view_may_continue(0));
        assert!(!app.fs_exclusive_view.is_plane_view());
        assert!(app.view_trim_mode);
    }

    #[test]
    fn plane_real_handler_receives_pre_shift_vertical_and_ignores_physical_tilt() {
        let _keys = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        let mods = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
        viewer.frame(&mut app, vec![wheel(40., 0., mods)]);
        assert!(!app.fs_exclusive_view.is_plane_view());
        enter(&mut app, &mut viewer);
        assert!((app.fs_zoom - 1.1).abs() < 1.0e-5);
        viewer.frame(&mut app, vec![wheel(0., -80., mods)]);
        assert!(!app.fs_exclusive_view.is_plane_view());
        assert!((app.fs_zoom - 1.).abs() < 1.0e-5);
    }
    #[test]
    fn plane_real_frame_drops_fisheye_after_wheel_page_request_and_does_not_rebind_click() {
        let _keys = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        app.fs_focus_regained_at = None;
        let mut events = vec![
            egui::Event::PointerMoved(egui::pos2(500., 300.)),
            wheel(0., -40., egui::Modifiers::NONE),
            wheel(0., 40., egui::Modifiers::CTRL | egui::Modifiers::SHIFT),
        ];
        events.extend(transition_and_click_tests::click(
            egui::pos2(700., 300.),
            false,
        ));
        viewer.frame(&mut app, events);
        assert!(!app.fs_exclusive_view.is_plane_view());
        assert_eq!(
            app.fullscreen_idx,
            Some(1),
            "the mixed frame turns exactly one page; the later fisheye segment is spent"
        );
    }
    #[test]
    fn plane_same_pass_prediction_and_application_use_the_same_pose_and_click_classification() {
        let _keys = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, viewer.size);
        app.fs_focus_regained_at = None;
        let mut events = vec![
            egui::Event::PointerMoved(rect.center()),
            wheel(0., 40., egui::Modifiers::CTRL | egui::Modifiers::SHIFT),
        ];
        events.extend(transition_and_click_tests::click(rect.center(), false));
        let mut expected = None;
        viewer.frame_with_before(&mut app, events, |app, ctx| {
            let wheel_options = ctx.options(|o| o.input_options);
            let actions = ctx
                .input(|i| crate::wheel_input::remaining_wheel_segments(i, wheel_options))
                .into_iter()
                .map(|segment| FsWheelAction {
                    input: segment.input,
                    segment,
                    operation: None,
                    class: FsWheelInPageClass::PlaneView,
                    scroll_origin: None,
                    page_owner: (
                        app.items_generation,
                        app.fullscreen_idx,
                        app.fullscreen_page_slice,
                    ),
                    seek_hover: false,
                })
                .collect::<Vec<_>>();
            let idx = app.fullscreen_idx.unwrap();
            let canvas = app.fullscreen_media_rect(rect, idx, false);
            expected = Some(app.predict_fs_wheel_frame(ctx, canvas, &actions));
        });
        let expected = expected.unwrap();
        assert!(expected.3);
        assert_eq!(app.fs_zoom, expected.0);
        assert_eq!(app.fs_pan, expected.1);
        assert!(app.fs_exclusive_view.is_plane_view());
        assert_eq!(
            app.fullscreen_idx,
            Some(0),
            "the entering click cannot turn a page"
        );
    }
    #[test]
    fn plane_deferred_segment_cannot_enter_a_page_other_than_its_planned_owner() {
        let _keys = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        viewer.frame_with_before(
            &mut app,
            vec![wheel(
                0.,
                40.,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            )],
            |app, ctx| {
                let wheel_options = ctx.options(|o| o.input_options);
                let segment = ctx.input(|input| {
                    crate::wheel_input::remaining_wheel_segments(input, wheel_options)
                })[0];
                let action = FsWheelAction {
                    input: segment.input,
                    segment,
                    operation: None,
                    class: FsWheelInPageClass::PlaneView,
                    scroll_origin: None,
                    page_owner: (
                        app.items_generation,
                        app.fullscreen_idx,
                        app.fullscreen_page_slice,
                    ),
                    seek_hover: false,
                };
                assert!(app.plane_wheel_action_matches_page(&action));
                app.fullscreen_idx = Some(3);
                let rect = app.fullscreen_media_rect(ctx.content_rect(), 3, false);
                assert!(!app.plane_wheel_action_matches_page(&action));
                assert!(!app.predict_fs_wheel_frame(ctx, rect, &[action]).3);
                crate::wheel_input::consume_wheel_segment(ctx, segment);
            },
        );
        assert!(!app.fs_exclusive_view.is_plane_view());
        assert_eq!(app.fullscreen_idx, Some(3));
    }

    #[test]
    fn plane_page_slice_reset_fit_and_owner_change_retire_but_scoped_mount_preserves() {
        let _keys = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        for route in 0..5 {
            enter(&mut app, &mut viewer);
            match route {
                0 => {
                    app.fullscreen_page_slice = crate::page_split::PageSlice::Left;
                    assert!(!app.plane_view_may_continue(0));
                    app.fullscreen_page_slice = crate::page_split::PageSlice::Full;
                }
                1 => {
                    app.reset_fullscreen_fit_transform(0);
                }
                2 => {
                    app.apply_fs_canvas_click(
                        FsCanvasClickAction::ResetTransform,
                        &mut FsPageNavRequest::default(),
                        &mut None,
                        None,
                    );
                }
                3 => {
                    app.retire_exclusive_view_for_owner_change("test_owner_change");
                }
                _ => {
                    let main = app.edit_request_owner_context();
                    app.with_window_context_for_test(main, |owner| {
                        assert!(owner.fs_exclusive_view.is_plane_view())
                    });
                    assert!(app.fs_exclusive_view.is_plane_view());
                    app.retire_exclusive_view_for_owner_change("test_end");
                }
            }
            assert!(!app.fs_exclusive_view.is_plane_view());
        }
    }
    #[test]
    fn plane_lens_owner_survives_modifiers_and_updates_only_center_until_release() {
        let _keys = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        enter(&mut app, &mut viewer);
        let pose = app.plane_view_pose().unwrap();
        let pan = app.fs_pan;
        let start = egui::pos2(500., 300.);
        // This is an already-focused canvas press, not the focus-restoring click
        // armed by the fixture's first pass (same setup as the drag handler tests).
        app.fs_focus_regained_at = None;
        assert!(app.fs_prev_focused);
        crate::key_input::arm_test_synthetic_input(0xE917, egui::ViewportId::ROOT);
        let now = std::time::Instant::now();
        crate::key_input::enqueue_test_synthetic_command(
            crate::key_input::SyntheticKeyCommand::down(
                now,
                crate::key_input::SyntheticNavigationKey::Tab,
                crate::key_input::SyntheticModifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
        );
        crate::key_input::advance_test_synthetic_input(now);
        viewer.frame_with_before(
            &mut app,
            vec![
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            |_, ctx| {
                let permit = crate::keyboard_input::focused_key_state_permit(ctx).unwrap();
                assert!(crate::keymap::modifier_held_via_os(
                    permit,
                    crate::keymap::ModKind::Shift
                ));
                assert!(!crate::keymap::modifier_held_via_os(
                    permit,
                    crate::keymap::ModKind::Ctrl
                ));
                assert!(ctx.input(|input| input.pointer.primary_pressed()));
            },
        );
        assert!(
            app.fs_canvas_drag.is_lens(),
            "the physical Shift level owns the primary press"
        );
        crate::key_input::clear_test_synthetic_input();
        viewer.frame(
            &mut app,
            vec![
                egui::Event::PointerMoved(start + egui::vec2(20., 30.)),
                egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
            ],
        );
        assert_ne!(app.plane_view_pose().unwrap().lens_center, pose.lens_center);
        assert_eq!(app.fs_pan, pan);
        assert_eq!(app.fs_free_rotation, 0.);
        assert!(
            app.fs_canvas_drag.is_lens(),
            "changing modifiers cannot transfer a held lens to pan/rotation"
        );
        viewer.frame(
            &mut app,
            vec![egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::CTRL,
            }],
        );
        assert!(
            !app.fs_canvas_drag.is_lens(),
            "a released primary ends the owner"
        );
    }

    #[test]
    fn plane_lens_focus_loss_modal_and_projection_replacement_end_owner_without_pan() {
        let _keys = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        enter(&mut app, &mut viewer);
        let pose = app.plane_view_pose().unwrap();
        let pan = app.fs_pan;
        let owner = FsCanvasDrag::PlaneLens {
            anchor: pose.anchor,
            start: egui::pos2(500., 300.),
            center: pose.lens_center,
        };
        app.fs_canvas_drag = owner;
        viewer.frame_with_before(&mut app, vec![], |_, ctx| {
            ctx.input_mut(|input| {
                input
                    .raw
                    .viewports
                    .get_mut(&egui::ViewportId::ROOT)
                    .unwrap()
                    .focused = Some(false)
            })
        });
        assert!(!app.fs_canvas_drag.is_lens());
        app.fs_canvas_drag = owner;
        app.show_about_dialog = true;
        viewer.frame(&mut app, vec![]);
        assert!(!app.fs_canvas_drag.is_lens());
        app.show_about_dialog = false;
        app.fs_canvas_drag = owner;
        let context = app.edit_request_owner_context();
        app.set_fs_exclusive_view(
            FsExclusiveView::Depth(crate::depth_parallax::DepthViewState::new(context)),
            "plane_replace_by_depth_test",
        );
        assert!(!app.fs_canvas_drag.is_lens());
        assert!(app.fs_exclusive_view.is_depth());
        assert_eq!(app.fs_pan, pan);
    }

    #[test]
    fn plane_occupancy_keeps_image_zoom_but_suppresses_navigation_tools_and_range_copy() {
        let _keys = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        enter(&mut app, &mut viewer);
        assert_eq!(app.fs_zoom_owner(), FsZoomOwner::Image);
        app.settings.page_turn_animation_enabled = true;
        assert!(!app.page_turn_view_eligible(0));
        app.settings.fullscreen_navigator_visible = true;
        assert!(!app.fs_navigator_allowed_for_display_unit(&viewer.ctx, 0, false));
        app.begin_capture_region_selection(&viewer.ctx, 0);
        assert!(app.capture_region_selection.is_none());
        assert!(app.fs_exclusive_view.is_plane_view());
        let zoom = app.fs_zoom;
        let strength = app.plane_view_pose().unwrap().strength;
        app.fs_zoom = zoom * 1.2;
        viewer.frame(&mut app, vec![]);
        assert_eq!(app.plane_view_pose().unwrap().strength, strength);
        // Plain clicks at the side and outside the 2D image are transformed-view clicks.
        for pos in [egui::pos2(300., 300.), egui::pos2(800., 300.)] {
            for pressed in [true, false] {
                viewer.frame(
                    &mut app,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            assert_eq!(app.fullscreen_idx, Some(0));
            assert!(app.fs_exclusive_view.is_plane_view());
        }
    }

    #[test]
    fn plane_material_pending_keeps_previous_description_and_same_id_metadata_shares_copy() {
        let _keys = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        enter(&mut app, &mut viewer);
        let render = app.wgpu_render_state.as_ref().unwrap().clone();
        let pixels = egui::ColorImage::filled([32, 48], egui::Color32::RED);
        let texture = viewer
            .ctx
            .load_texture("plane adoption", pixels.clone(), Default::default());
        render.renderer.write().update_texture(
            &render.device,
            &render.queue,
            texture.id(),
            &egui::epaint::ImageDelta::full(pixels, Default::default()),
        );
        let resource = app.fixture_raster_paint_resource(0, texture);
        let payload = FullscreenPaintPayload::from(resource.clone());
        let first = app
            .select_plane_material(
                &viewer.ctx,
                &payload,
                egui::vec2(32., 48.),
                egui::vec2(32., 48.),
            )
            .unwrap();
        let basis = crate::material_display_basis::MaterialDisplayBasis::unavailable(
            crate::material_display_basis::MaterialBasisUnavailable::PdfMetadataUnavailable,
        );
        let changed = FullscreenPaintPayload::from(resource.with_display_basis(basis));
        let second = app
            .select_plane_material(
                &viewer.ctx,
                &changed,
                egui::vec2(32., 48.),
                egui::vec2(32., 48.),
            )
            .unwrap();
        assert!(Arc::ptr_eq(&first.copy, &second.copy));
        assert_ne!(first.description, second.description);
        let pending = viewer.ctx.load_texture(
            "plane not registered",
            egui::ColorImage::filled([64, 96], egui::Color32::BLUE),
            Default::default(),
        );
        let pending = FullscreenPaintPayload::from(app.fixture_raster_paint_resource(0, pending));
        let previous = app
            .select_plane_material(
                &viewer.ctx,
                &pending,
                egui::vec2(64., 96.),
                egui::vec2(64., 96.),
            )
            .unwrap();
        assert!(Arc::ptr_eq(&previous, &second));
        assert!(
            matches!(&app.fs_exclusive_view,FsExclusiveView::PlaneView(state) if matches!(state.color,PlaneColorPhase::Pending{..}) && state.last_drawn.is_none())
        );
    }
    #[test]
    fn plane_pending_pdf_reraster_draws_previous_geometry_and_keeps_current_demand_separate() {
        use crate::material_display_basis::{MaterialDisplayBasis, MaterialLegacyBasis};
        let _keys = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        enter(&mut app, &mut viewer);
        app.fullscreen_idx = Some(0);
        let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200., 900.));
        let render = app.wgpu_render_state.clone().unwrap();
        let make = |app: &App, size: [usize; 2], registered: bool| {
            let pixels = egui::ColorImage::filled(size, egui::Color32::WHITE);
            let texture = viewer.ctx.load_texture(
                format!("plane-pdf-{}", size[1]),
                pixels.clone(),
                Default::default(),
            );
            if registered {
                render.renderer.write().update_texture(
                    &render.device,
                    &render.queue,
                    texture.id(),
                    &egui::epaint::ImageDelta::full(pixels, Default::default()),
                );
            }
            let legacy = MaterialLegacyBasis::SourceLayout {
                basis: crate::fullscreen_specified_scale::LegacySourceLayoutBasis {
                    layout_size: egui::vec2(613125., 791875.),
                    reference_size: egui::vec2(size[0] as f32, size[1] as f32),
                },
            };
            let basis = MaterialDisplayBasis::new(
                legacy,
                legacy,
                legacy,
                crate::fullscreen_specified_scale::FullscreenFitBasis::new(
                    egui::vec2(613125., 791875.),
                    egui::vec2(2400., 3100.),
                )
                .ok_or(crate::material_display_basis::MaterialBasisUnavailable::InvalidDimensions),
            );
            FullscreenPaintPayload::from(
                app.fixture_raster_paint_resource(0, texture)
                    .with_display_basis(basis),
            )
        };
        let old = make(&app, [317, 409], true);
        let now = make(&app, [634, 819], false);
        let draw = |app: &mut App, payload: &FullscreenPaintPayload, size: egui::Vec2| {
            let mut result = None;
            viewer
                .ctx
                .run_pass(
                    egui::RawInput {
                        screen_rect: Some(canvas),
                        ..Default::default()
                    },
                    |ctx| {
                        ctx.root_ui(|ui| {
                            app.fullscreen_page_layout
                                .begin(FullscreenPageLayoutKind::Single);
                            result = app.draw_fs_image_with_initial_placement(
                                ui,
                                canvas,
                                0,
                                Some(size),
                                FsPageLayoutSource::CurrentItem,
                                None,
                                None,
                                SingletonSpreadPlacement::Center,
                                Some(payload),
                                None,
                                false,
                                false,
                                false,
                                crate::rotation_db::Rotation::None,
                                app.fs_zoom_pan(),
                                0.,
                                &FsBgStyle::Solid(egui::Color32::BLACK),
                                "",
                                false,
                                app.effective_fullscreen_fit_mode(),
                                app.fullscreen_fit_scale_limits(ctx.pixels_per_point()),
                                None,
                                None,
                                false,
                                None,
                                page_turn::FlatCropPaint::WithPage,
                                true,
                                false,
                            );
                            if let Some(d) = &result {
                                app.fullscreen_page_layout.push(d.transform);
                            }
                        })
                    },
                )
                .drop_without_applying_deltas();
            result.unwrap()
        };
        // Independent current-material geometry: the draw result must report its
        // demand even while a different (previous) material supplies the pixels.
        let current_geometry = |app: &App, payload: &FullscreenPaintPayload, size| {
            let input = DisplayedImageTransformInput {
                page_idx: 0,
                viewport_rect: canvas,
                source_size: size,
                texture_size: payload.full_source_size_vec2(),
                rotation: crate::rotation_db::Rotation::None,
                free_rotation_rad: 0.,
                content_bbox: None,
                fit_mode: app
                    .fs_fit_input_for_payload(0, app.effective_fullscreen_fit_mode(), payload)
                    .unwrap(),
                fit_scale_limits: app.fullscreen_fit_scale_limits(1.),
                pixels_per_point: 1.,
                placement: ResolvedDisplayPlacement::Normal {
                    zoom_pan: app.fs_zoom_pan(),
                },
                pixel_fit: RectPixelFit::Texels,
            };
            resolve_fs_image_transform(input, None).unwrap()
        };
        let provisional = match &old {
            FullscreenPaintPayload::NormalResource(resource) => FullscreenPaintPayload::from(
                resource
                    .clone()
                    .with_page_material_kind(FullscreenPageMaterialKind::Provisional),
            ),
            _ => panic!("fixture resource"),
        };
        app.request_fs_initial_placement_with_kind(
            0,
            initial_placement::FsInitialPlacementKind::LandingStart,
        );
        let first = draw(&mut app, &provisional, egui::vec2(317., 409.));
        assert!(app.fs_initial_placement_request.is_some());
        let first_material = match &app.fs_exclusive_view {
            FsExclusiveView::PlaneView(state) => {
                state.last_drawn.as_ref().unwrap().material.clone()
            }
            _ => panic!("plane"),
        };
        let second = draw(&mut app, &now, egui::vec2(634., 819.));
        assert_eq!(second.transform.source_size, first.transform.source_size);
        assert!(
            app.fs_initial_placement_request.is_some(),
            "pending current complete must not consume the previous provisional's placement"
        );
        assert_eq!(
            first_material.description.material_kind,
            FullscreenPageMaterialKind::Provisional
        );
        let current = current_geometry(&app, &now, egui::vec2(634., 819.));
        assert_eq!(second.pdf_demand_rect, current.layout_full_image_rect);
        assert_ne!(
            second.pdf_demand_rect, second.transform.layout_full_image_rect,
            "this raster-rounding fixture must distinguish the current demand from the previous paint"
        );
        assert!(placement_frames_match(
            current.placement_frame,
            second.transform.placement_frame,
            1.
        ));
        let FsExclusiveView::PlaneView(state) = &app.fs_exclusive_view else {
            panic!("plane")
        };
        assert!(matches!(state.color, PlaneColorPhase::Pending { .. }));
        assert!(Arc::ptr_eq(
            &state.last_drawn.as_ref().unwrap().material,
            &first_material
        ));
        assert_eq!(
            app.fullscreen_page_layout
                .displayed_pages()
                .next()
                .unwrap()
                .transform,
            second.transform
        );
        app.clear_fs_initial_placement_request();
        app.settings.fullscreen_fit_mode = FullscreenFitMode::SpecifiedScale;
        app.settings.fullscreen_specified_scale = crate::settings::FullscreenSpecifiedScale {
            width: crate::settings::SpecifiedScaleAxisSetting::disabled(
                crate::settings::SpecifiedScaleAxis::PercentTenths(800),
            ),
            height: crate::settings::SpecifiedScaleAxisSetting::enabled(
                crate::settings::SpecifiedScaleAxis::LogicalPixels(1200),
            ),
        };
        let specified_old = draw(&mut app, &old, egui::vec2(317., 409.));
        let specified_now = draw(&mut app, &now, egui::vec2(634., 819.));
        assert_eq!(
            specified_now.transform.source_size,
            specified_old.transform.source_size
        );
        let current = current_geometry(&app, &now, egui::vec2(634., 819.));
        assert_eq!(
            specified_now.pdf_demand_rect,
            current.layout_full_image_rect
        );
        assert!(placement_frames_match(
            current.placement_frame,
            specified_now.transform.placement_frame,
            1.
        ));
        let FsExclusiveView::PlaneView(state) = &app.fs_exclusive_view else {
            panic!("plane")
        };
        assert!(state.last_drawn.is_some());
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
        draw(&mut app, &now, egui::vec2(634., 819.));
        let FsExclusiveView::PlaneView(state) = &app.fs_exclusive_view else {
            panic!("plane")
        };
        assert!(
            state.last_drawn.is_none(),
            "a changed placement draws 2D and invalidates output"
        );
    }

    #[test]
    fn plane_capture_copy_and_export_require_the_last_drawn_matching_snapshot() {
        let _keys = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        enter(&mut app, &mut viewer);
        // Registration belongs to the renderer after UI; manually flush generated fixture deltas.
        let render = app.wgpu_render_state.as_ref().unwrap().clone();
        let mut delta = viewer.ctx.tex_manager().write().take_delta();
        for (id, deltas) in std::mem::take(&mut delta.set) {
            for delta in deltas {
                render
                    .renderer
                    .write()
                    .update_texture(&render.device, &render.queue, id, &delta);
            }
        }
        viewer.frame(&mut app, vec![]);
        assert!(
            app.plane_frame_request(crate::plane_view_wgpu::PlaneOutputChoice::Canvas)
                .is_ok()
        );
        assert!(matches!(
            app.prepare_capture_pixel_work(&viewer.ctx, 0).unwrap(),
            crate::capture::CapturePixelWork::PlaneView { .. }
        ));
        assert!(matches!(
            app.prepare_export_dialog_target(&viewer.ctx, 0)
                .unwrap()
                .composite,
            crate::export_dialog::ExportComposite::PlaneView(_)
        ));
        let frozen = app
            .plane_frame_request(crate::plane_view_wgpu::PlaneOutputChoice::Canvas)
            .unwrap()
            .snapshot;
        app.clear_plane_last_drawn();
        assert!(app.prepare_capture_pixel_work(&viewer.ctx, 0).is_err());
        if let FsExclusiveView::PlaneView(state) = &mut app.fs_exclusive_view {
            state.last_drawn = Some(frozen.clone());
        }
        app.fullscreen_page_slice = crate::page_split::PageSlice::Left;
        assert!(
            app.plane_frame_request(crate::plane_view_wgpu::PlaneOutputChoice::Canvas)
                .is_err()
        );
        app.fullscreen_page_slice = crate::page_split::PageSlice::Full;
        if let FsExclusiveView::PlaneView(state) = &mut app.fs_exclusive_view {
            state.last_drawn = Some(Arc::new(crate::plane_view_wgpu::PlaneFrameSnapshot {
                context: crate::app::ViewerContextId::for_test(917_099),
                anchor: frozen.anchor,
                material: frozen.material.clone(),
                params: frozen.params,
            }));
        }
        assert!(
            app.plane_frame_request(crate::plane_view_wgpu::PlaneOutputChoice::Canvas)
                .is_err(),
            "a sibling context snapshot cannot be exported"
        );
    }
}

#[test]
fn plane_pdf_raster_rounding_preserves_placement_before_containment_at_all_dpi_and_zoom() {
    for ppp in [1., 1.25, 2.] {
        for zoom in [1., 1.1] {
            let input = |size: egui::Vec2, mode| DisplayedImageTransformInput {
                page_idx: 0,
                viewport_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200., 900.)),
                source_size: size,
                texture_size: size,
                rotation: crate::rotation_db::Rotation::None,
                free_rotation_rad: 0.,
                content_bbox: None,
                fit_mode: FullscreenFitInput::SourceLayout {
                    mode,
                    basis: crate::fullscreen_specified_scale::LegacySourceLayoutBasis {
                        layout_size: egui::vec2(613125., 791875.),
                        reference_size: size,
                    },
                },
                fit_scale_limits: FullscreenFitScaleLimits {
                    pixels_per_point: ppp,
                    ..Default::default()
                },
                pixels_per_point: ppp,
                placement: ResolvedDisplayPlacement::Normal {
                    zoom_pan: Some((zoom, egui::Vec2::ZERO)),
                },
                pixel_fit: RectPixelFit::Texels,
            };
            let old = resolve_fs_image_transform(
                input(egui::vec2(317., 409.), FullscreenFitMode::Page),
                None,
            )
            .unwrap();
            let now = resolve_fs_image_transform(
                input(egui::vec2(634., 819.), FullscreenFitMode::Page),
                None,
            )
            .unwrap();
            assert!(placement_frames_match(
                old.placement_frame,
                now.placement_frame,
                ppp
            ));
            let old_original = resolve_fs_image_transform(
                input(egui::vec2(317., 409.), FullscreenFitMode::Original),
                None,
            )
            .unwrap();
            let now_original = resolve_fs_image_transform(
                input(egui::vec2(634., 819.), FullscreenFitMode::Original),
                None,
            )
            .unwrap();
            assert!(!placement_frames_match(
                old_original.placement_frame,
                now_original.placement_frame,
                ppp
            ));
        }
    }
}
