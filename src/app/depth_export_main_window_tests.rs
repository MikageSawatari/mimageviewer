//! Export admission and modal input on the actual ROOT embedded canvas/panel.
use super::*;
use crate::app::depth_main_window_tests::{RootDepthViewer, wheel};
use crate::fs_exclusive_view::FsExclusiveView;
use crate::ring_shortcut::{RingActionId, RingDirection};

#[test]
fn realmachine50_forwarded_open_after_independent_export_retires_previous_live_owner() {
    realmachine50_forwarded_open_case(true);
}

#[test]
fn realmachine50_forwarded_open_without_dialog_retires_previous_live_owner() {
    realmachine50_forwarded_open_case(false);
}

fn realmachine50_forwarded_open_case(export: bool) {
    let _input = crate::key_input::lock_test_input();
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
    app.initialized = true;
    app.indexer_init.mark_unavailable_if_not_ready();
    app.settings.sidecar_backup_enabled = false;
    app.settings.tag_sidecar_backup_enabled = false;
    app.settings.detached_viewer_enabled = true;
    app.settings.detached_viewer_open_images_in_window = true;
    app.settings.page_turn_animation_enabled = false;
    crate::app::depth_view_tests::install_pages(&mut app, &ctx);
    app.fullscreen_idx = None;
    let folder = app.tmp.path().join("forwarded-depth-export");
    std::fs::create_dir_all(&folder).unwrap();
    let requested = folder.join("forwarded.jpg");
    image::RgbImage::from_pixel(8, 12, image::Rgb([90, 100, 110]))
        .save(&requested)
        .unwrap();
    let old_window = 88_997;
    let old = app.build_active_context_for_test(Some(old_window), DetachedSource::Image, |owner| {
        crate::app::depth_view_tests::install_pages(owner, &ctx);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        owner.detached_viewer_independent_active = true;
        owner.native_video_in_window_active = false;
        crate::app::depth_view_tests::install_ready_depth_for_export(owner);
    });
    let old_render = app
        .with_viewer_context(old, |owner| {
            owner.mark_active_detached_viewport_rendered(&ctx);
            let FsExclusiveView::Depth(state) = &owner.fs_exclusive_view else {
                unreachable!()
            };
            let paint_pass = ctx.cumulative_pass_nr();
            ctx.data_mut(|data| {
                data.insert_temp(
                    snapshot_id(old),
                    DisplaySnapshot {
                        resource_id: state.ready().unwrap().resource_id,
                        paint_pass,
                        page_idx: 0,
                        items_generation: owner.items_generation,
                        uv: egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                        quarter_turns: 0,
                        paint_size: egui::vec2(200., 300.),
                        underlay: crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::BLACK),
                    },
                )
            });
            if export {
                owner.open_depth_export(&ctx, 0);
                assert!(owner.depth_export_dialog_open());
            }
            owner.wgpu_render_state.clone().unwrap()
        })
        .unwrap();
    assert_eq!(app.depth_export_owner_context(), export.then_some(old));
    app.activation_open_path_tx
        .send(crate::single_instance::ActivationOpenPath::received(
            requested.clone(),
            app.remote_session_handle(),
        ))
        .unwrap();
    app.poll_activation_open_paths(&ctx);
    if export {
        assert!(app.startup_open_path_resolve_pending.is_none());
        assert_eq!(app.active_detached_window_id(), Some(old_window));
        assert!(app.document_open_modal_admission_blocked());
        // Real Escape -> Cancel; never clear the modal operation to bypass admission.
        ctx.begin_pass(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000., 800.),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        });
        app.with_viewer_context(old, |owner| owner.draw_depth_export_dialog(&ctx))
            .unwrap();
        ctx.end_pass().drop_without_applying_deltas();
        assert!(!app.depth_export_dialog_open());
        assert!(!app.document_open_modal_admission_blocked());
        app.poll_activation_open_paths(&ctx);
    }
    assert!(
        app.startup_open_path_resolve_pending.is_some(),
        "held open reaches the real resolver"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        app.poll_startup_open_path_resolve(&ctx);
        app.poll_sidecar_restore(&ctx);
        app.poll_book_composition_read(&ctx);
        let opened = app.active_detached_window_id().filter(|window| *window != old_window)
            .and_then(|window| app.with_window_viewer_context(window, |owner| {
                owner.poll_sidecar_restore(&ctx);
                owner.poll_book_composition_read(&ctx);
                owner.fullscreen_idx.and_then(|idx| owner.items.get(idx)).is_some_and(|item|
                    matches!(item, GridItem::Image(path) if crate::path_key::eq_keep_drive(path, &requested)))
            }).ok()).unwrap_or(false);
        if opened
            && app.startup_open_path_resolve_pending.is_none()
            && !app.sidecar_restore_active()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "forwarded target did not finish: session={:?}, main_idx={:?}, modal={}, pending={}",
            app.active_detached_window_id(),
            app.fullscreen_idx,
            app.document_open_modal_admission_blocked(),
            app.startup_open_path_resolve_pending.is_some()
        );
        std::thread::yield_now();
    }
    // The target was accepted and opened. The old owner must no longer hold Depth.
    app.with_viewer_context(old, |owner| {
        assert!(
            !owner.fs_exclusive_view.is_depth(),
            "forwarded open replaced the active session without retiring its old Depth owner"
        )
    })
    .unwrap();
    assert_eq!(
        app.detached_image_windows
            .iter()
            .filter(|window| window.id == old_window)
            .count(),
        1,
        "the previous independent window is preserved as one passive snapshot"
    );
    assert_eq!(
        app.detached_window_state(old_window),
        Some(DetachedWindowState::Parked)
    );
    assert!(
        old_render
            .renderer
            .read()
            .callback_resources
            .get::<crate::depth_parallax_wgpu::DepthRenderResourcesRef>()
            .is_none(),
        "the retired owner's GPU publication is removed"
    );
    let new_window = app.active_detached_window_id().unwrap();
    app.with_window_viewer_context(new_window, |owner| {
        let idx = owner.fullscreen_idx.unwrap();
        assert_ne!(
            owner.depth_entry_allowed(idx),
            Err(crate::depth_parallax::DepthEntryBlock::OtherWindow)
        );
    })
    .unwrap();
    assert!(app.activate_detached_image_window_snapshot(&ctx, old_window));
    app.with_viewer_context(old, |owner| {
        let idx = owner.fullscreen_idx.unwrap();
        assert_ne!(
            owner.depth_entry_allowed(idx),
            Err(crate::depth_parallax::DepthEntryBlock::OtherWindow)
        );
    })
    .unwrap();
}

fn transparent_source(app: &mut App) {
    let Some(FsCacheEntry::Static { pixels, tex, .. }) = app.fs_cache.get_mut(&0) else {
        unreachable!()
    };
    *pixels = Arc::new(egui::ColorImage::filled([4, 6], egui::Color32::TRANSPARENT));
    tex.set((**pixels).clone(), Default::default());
    app.clear_adjustment_caches(0);
}

/// Deliver an actual configured gamepad ring through its admission owner. The
/// focus fixture is thread-local and never reads or changes the desktop HWND.
fn ring(viewer: &mut RootDepthViewer, app: &mut App, action: RingActionId, allowed: bool) {
    use crate::gamepad::{PadButton, PadEvent};
    app.settings.gamepad_enabled = true;
    app.detached_window_manager
        .note_input_surface(crate::app::ActionSurface::Viewer);
    app.settings.ring_shortcuts.image.slots[RingDirection::Up.slot_index()] = action;
    for events in [
        vec![],
        vec![
            PadEvent::ButtonPressed(PadButton::West),
            PadEvent::ButtonPressed(PadButton::DPadUp),
        ],
        vec![
            PadEvent::ButtonReleased(PadButton::West),
            PadEvent::ButtonReleased(PadButton::DPadUp),
        ],
    ] {
        for event in events {
            app.gamepad.push_for_test(event);
        }
        viewer.frame_with_before(app, vec![], |app, ctx| {
            let batch = app.sample_gamepad_input_with_logic_for_test(ctx);
            assert_eq!(!app.any_modal_dialog_open_for_fullscreen_keys(), allowed);
            let outcome = app.dispatch_gamepad_batch(ctx, batch);
            if !allowed {
                assert!(
                    outcome.nav.is_none(),
                    "modal ring must not publish navigation"
                );
            }
        });
    }
}

fn acquire_request() -> mimageviewer_ipc::SessionAcquireRequest {
    mimageviewer_ipc::SessionAcquireRequest {
        client_id: "depth-root-export-test".into(),
        peer: mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: None,
        },
    }
}

fn assert_modal_layers(viewer: &RootDepthViewer) {
    let backdrop = egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("depth_export_barrier"),
    );
    let dialog = egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("depth_export_dialog"),
    );
    let panel = egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("depth_parallax_panel"),
    );
    {
        let evidence = viewer.modal_layers();
        let layers = evidence.layers;
        let at = |layer| {
            layers
                .iter()
                .position(|candidate| *candidate == layer)
                .unwrap()
        };
        assert!(at(panel) < at(backdrop));
        assert_eq!(at(backdrop) + 1, at(dialog));
        assert_eq!(evidence.top_modal, Some(backdrop));
        assert!(!evidence.panel_interactive);
        assert!(evidence.dialog_interactive);
    }
}

fn background(
    app: &App,
) -> (
    Option<usize>,
    ViewerPresentation,
    u8,
    String,
    bool,
    f32,
    egui::Vec2,
) {
    let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
        panic!("Depth owner");
    };
    (
        app.fullscreen_idx,
        app.viewer_presentation,
        app.fs_transparent_bg_mode,
        format!("{:?}", app.settings.depth_parallax),
        state.panel_visible,
        app.fs_zoom,
        app.fs_pan,
    )
}

fn key(key: egui::Key, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    [true, false]
        .into_iter()
        .map(|pressed| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers,
        })
        .collect()
}

#[test]
fn depth_main_window_export_real_root_modal_owns_all_phases_and_reopen_inputs() {
    let _input = crate::key_input::lock_test_input();
    let _foreground = crate::test_native_focus::scoped(1, 1, true);
    for (linked, batched) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut app = setup_app_for_test();
        let mut viewer = if linked {
            RootDepthViewer::linked(&mut app)
        } else {
            RootDepthViewer::new(&mut app)
        };
        transparent_source(&mut app);
        viewer.ready(&mut app);
        let handle = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(handle.clone());
        viewer.frame(&mut app, vec![]);
        viewer.frame(&mut app, vec![]);
        // Real panel admission, after a real depth callback supplied the capture.
        let export = viewer.label("書き出し…").center();
        viewer.click(&mut app, export, batched);
        assert!(app.depth_export_dialog_open());
        assert_eq!(
            app.depth_export_operation
                .as_ref()
                .unwrap()
                .display
                .page_idx,
            0
        );
        let mut running_sender = None;
        let mut running_cancel = None;
        for phase in 0..3 {
            if phase == 1 {
                let (job, events, cancel) = DepthExportJob::controlled_for_test();
                app.depth_export_operation.as_mut().unwrap().phase = Phase::Running {
                    job,
                    completed: 1,
                    total: 3,
                    status: RunStatus::Active,
                };
                running_sender = Some(events);
                running_cancel = Some(cancel);
            } else if phase == 2 {
                running_sender
                    .take()
                    .unwrap()
                    .send(DepthExportEvent::Finished(Ok(
                        crate::depth_export::DepthExportSummary {
                            output: "root_3d.mp4".into(),
                            bytes: Some(1024),
                        },
                    )))
                    .unwrap();
                app.poll_depth_export();
                assert!(matches!(
                    app.depth_export_operation.as_ref().unwrap().phase,
                    Phase::Finished { .. }
                ));
            }
            let before = background(&app);
            for pass in 0..3 {
                viewer.frame(&mut app, vec![]);
                assert!(app.common_modal_dialog_open());
                assert!(app.document_open_modal_admission_blocked());
                assert_eq!(
                    handle.acquire(acquire_request()).status,
                    mimageviewer_ipc::SessionStatus::LocalInUse
                );
                assert_modal_layers(&viewer);
                let heading = if phase == 0 {
                    "3D 写真の書き出し"
                } else {
                    "3D 写真の書き出し状況"
                };
                viewer.label(heading);
                viewer.label(if phase == 2 {
                    "閉じる"
                } else {
                    "キャンセル"
                });
                if phase == 0 {
                    viewer.label("保存先を選ぶ…");
                }
                assert_eq!(
                    viewer.callbacks().len(),
                    1,
                    "ROOT canvas still draws behind modal, phase={phase}, pass={pass}"
                );
            }
            let panel = viewer.panel();
            // Canvas, the real panel close/export region, and ordinary page wheel.
            for pos in [
                egui::pos2(30.0, 220.0),
                panel.right_top() + egui::vec2(-20.0, 20.0),
                export,
            ] {
                viewer.click(&mut app, pos, batched);
                viewer.frame(&mut app, wheel(pos));
                assert_eq!(background(&app), before, "phase={phase}, outside={pos:?}");
                assert!(app.depth_export_dialog_open());
                assert_modal_layers(&viewer);
            }
            for event in [
                key(egui::Key::D, egui::Modifiers::NONE),
                key(egui::Key::I, egui::Modifiers::NONE),
                key(egui::Key::F11, egui::Modifiers::NONE),
                key(egui::Key::F12, egui::Modifiers::NONE),
                key(egui::Key::ArrowRight, egui::Modifiers::NONE),
                key(egui::Key::B, egui::Modifiers::SHIFT),
            ] {
                viewer.frame(&mut app, event);
                assert_eq!(background(&app), before, "phase={phase}, key");
            }
            for action in [
                RingActionId::ImageBackgroundCycle,
                RingActionId::ImageEnd,
                RingActionId::ToggleWindowMode,
            ] {
                ring(&mut viewer, &mut app, action.clone(), false);
                assert_eq!(background(&app), before, "phase={phase}, ring={action:?}");
            }
            if phase == 0 {
                for (label, format) in [
                    ("GIF", DepthExportFormat::Gif),
                    ("APNG", DepthExportFormat::Apng),
                ] {
                    let pos = viewer.label(label).center();
                    viewer.click(&mut app, pos, batched);
                    let Phase::Settings(options) =
                        &app.depth_export_operation.as_ref().unwrap().phase
                    else {
                        unreachable!()
                    };
                    assert_eq!(
                        options.format, format,
                        "dialog controls remain usable after outside clicks"
                    );
                }
            } else if phase == 1 {
                let cancel = viewer.label("キャンセル").center();
                viewer.click(&mut app, cancel, batched);
                assert!(
                    running_cancel
                        .as_ref()
                        .unwrap()
                        .load(std::sync::atomic::Ordering::Relaxed)
                );
                assert!(
                    app.depth_export_dialog_open(),
                    "Running cancellation retains modal until completion"
                );
            }
        }
        let close = viewer.label("閉じる").center();
        viewer.click(&mut app, close, batched);
        assert!(!app.depth_export_dialog_open());
        viewer.frame(&mut app, vec![]);
        let export = viewer.label("書き出し…").center();
        viewer.click(&mut app, export, batched);
        assert!(
            app.depth_export_dialog_open(),
            "same ROOT dialog can reopen after Finished"
        );
        viewer.label("3D 写真の書き出し");
        viewer.label("保存先を選ぶ…");
        let cancel = viewer.label("キャンセル").center();
        viewer.click(&mut app, cancel, batched);
        assert!(!app.depth_export_dialog_open());
        viewer.frame(&mut app, vec![]);
        let before = app.fs_transparent_bg_mode;
        viewer.frame(&mut app, key(egui::Key::B, egui::Modifiers::SHIFT));
        assert_eq!(
            app.fs_transparent_bg_mode,
            (before + 1) % 3,
            "Shift+B positive control after dismissal"
        );
        let before = app.fs_transparent_bg_mode;
        ring(
            &mut viewer,
            &mut app,
            RingActionId::ImageBackgroundCycle,
            true,
        );
        assert_eq!(
            app.fs_transparent_bg_mode,
            (before + 1) % 3,
            "real ring positive control after dismissal"
        );
        assert_ne!(
            handle.acquire(acquire_request()).status,
            mimageviewer_ipc::SessionStatus::LocalInUse
        );
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_main_window_export_root_small_geometry_at_100_and_200_percent() {
    let _input = crate::key_input::lock_test_input();
    for scale in [1.0, 2.0] {
        let mut app = setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        viewer.ctx.set_zoom_factor(scale);
        viewer.ready(&mut app);
        viewer.frame(&mut app, vec![]);
        viewer.frame(&mut app, vec![]);
        assert!(viewer.ctx.content_rect().contains_rect(viewer.panel()));
        viewer.label("×");
        // At 200%, only the panel body scrolls; its fixed close remains visible.
        if viewer.text_rects("書き出し…").is_empty() {
            viewer.frame(&mut app, wheel(viewer.panel().center()));
            viewer.frame(&mut app, vec![]);
        }
        let export = viewer.label("書き出し…").center();
        viewer.click(&mut app, export, false);
        assert!(app.depth_export_dialog_open());
        for phase in 0..4 {
            if phase == 1 {
                let (job, _, _) = DepthExportJob::controlled_for_test();
                app.depth_export_operation.as_mut().unwrap().phase = Phase::Running {
                    job,
                    completed: 1,
                    total: 3,
                    status: RunStatus::Active,
                };
            } else if phase == 2 {
                app.depth_export_operation.as_mut().unwrap().phase = Phase::Finished {
                    message: "保存しました: root_3d.mp4 (1.0 KB)".into(),
                    failed: false,
                };
            } else if phase == 3 {
                app.depth_export_operation.take();
                viewer.frame(&mut app, vec![]);
                let export = viewer.label("書き出し…").center();
                viewer.click(&mut app, export, false);
                assert!(
                    app.depth_export_dialog_open(),
                    "scale={scale}: actual panel reopen"
                );
            }
            for pass in 0..3 {
                viewer.frame(&mut app, vec![]);
                let settings = phase == 0 || phase == 3;
                viewer.label(if settings {
                    "3D 写真の書き出し"
                } else {
                    "3D 写真の書き出し状況"
                });
                viewer.label(if phase == 2 {
                    "閉じる"
                } else {
                    "キャンセル"
                });
                if settings {
                    viewer.label("保存先を選ぶ…");
                }
                assert_modal_layers(&viewer);
                let rect =
                    egui::AreaState::load(&viewer.ctx, egui::Id::unique("depth_export_dialog"))
                        .unwrap()
                        .rect();
                assert!(
                    viewer.ctx.content_rect().contains_rect(rect),
                    "scale={scale}, phase={phase}, pass={pass}: {rect:?}"
                );
            }
        }
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_linked_dialogs_draw_only_in_main_context_and_return_to_root_after_close() {
    std::thread::spawn(|| {
        let _input = crate::key_input::lock_test_input();
        for addon in [false, true] {
            let mut app = setup_app_for_test();
            let mut viewer = RootDepthViewer::linked(&mut app);
            viewer.ready(&mut app);
            let render = app.wgpu_render_state.clone();
            let sibling = app.build_window_context_for_test(88_940, |owner| {
                crate::app::depth_view_tests::install_pages(owner, &viewer.ctx);
                owner.wgpu_render_state = render.clone();
                crate::app::source_texture_test_support::seed(owner, &viewer.ctx, 0);
                owner.viewer_presentation = ViewerPresentation::DetachedWindow;
                owner.detached_viewer_independent_active = true;
                owner.native_video_in_window_active = false;
                owner.set_fs_exclusive_view(FsExclusiveView::None, "independent_dialog_fixture");
            });
            viewer.frame(&mut app, vec![]);
            viewer.frame(&mut app, vec![]);
            let (heading, finished, cancel) = if addon {
                app.set_fs_exclusive_view(FsExclusiveView::None, "missing_model_linked_entry");
                app.depth_setup
                    .set_status_for_test(crate::addon::AddonStatus::Missing);
                viewer.frame(&mut app, key(egui::Key::D, egui::Modifiers::NONE));
                assert!(app.depth_setup.dialog_open_for_test());
                ("3D 写真の追加モデル", None, None)
            } else {
                let export = viewer.label("書き出し…").center();
                viewer.click(&mut app, export, false);
                assert!(app.depth_export_dialog_open());
                let (job, events, cancel) = DepthExportJob::controlled_for_test();
                app.depth_export_operation.as_mut().unwrap().phase = Phase::Running {
                    job,
                    completed: 1,
                    total: 3,
                    status: RunStatus::Active,
                };
                ("3D 写真の書き出し状況", Some(events), Some(cancel))
            };
            viewer.frame(&mut app, vec![]);
            viewer.frame(&mut app, vec![]);
            viewer.label(heading);
            app.with_viewer_context(sibling, |owner| {
                assert!(!if addon {
                    owner.depth_addon_dialog_in_mounted_viewer()
                } else {
                    owner.depth_export_dialog_in_mounted_viewer()
                });
                let before = viewer.child_passes();
                viewer.frame(owner, vec![]);
                viewer.frame(owner, vec![]);
                assert!(
                    viewer.child_passes() > before,
                    "independent body really ran"
                );
                assert!(
                    viewer.text_rects(heading).is_empty(),
                    "main dialog leaked into independent child"
                );
            })
            .unwrap();
            assert!(if addon {
                app.depth_addon_dialog_in_mounted_viewer()
            } else {
                app.depth_export_dialog_in_mounted_viewer()
            });
            viewer.frame(&mut app, vec![]);
            viewer.label(heading);
            app.close_fullscreen();
            assert!(app.fullscreen_idx.is_none());
            assert!(app.main_window_should_draw_export_dialogs());
            if let Some(cancel) = cancel {
                assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
                assert!(matches!(
                    app.depth_export_operation.as_ref().unwrap().phase,
                    Phase::Running {
                        status: RunStatus::Stopping,
                        ..
                    }
                ));
            }
            // Run the existing ROOT dialog drawing owner after its viewer is closed.
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            let draw_root = |app: &mut App| {
                let output = ctx.run_pass(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1093., 614.),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        if addon {
                            app.show_depth_addon_dialog(ctx);
                        } else {
                            app.draw_depth_export_dialog(ctx);
                        }
                    },
                );
                fn text(shape: &egui::Shape, label: &str) -> bool {
                    match shape {
                        egui::Shape::Text(t) => t.galley.text() == label,
                        egui::Shape::Vec(shapes) => shapes.iter().any(|s| text(s, label)),
                        _ => false,
                    }
                }
                let visible = output.shapes.iter().any(|s| text(&s.shape, heading));
                output.drop_without_applying_deltas();
                visible
            };
            draw_root(&mut app);
            assert!(draw_root(&mut app), "closed viewer dialog returns to ROOT");
            if let Some(events) = finished {
                events.send(DepthExportEvent::Cancelled).unwrap();
                app.poll_depth_export();
                assert!(matches!(
                    app.depth_export_operation.as_ref().unwrap().phase,
                    Phase::Finished { .. }
                ));
                assert!(draw_root(&mut app));
            }
        }
        crate::key_input::clear_test_frame();
    })
    .join()
    .unwrap();
}

#[test]
fn depth_independent_export_all_phases_protect_owner_and_allow_sibling_close() {
    for phase in 0..4 {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        crate::app::depth_view_tests::install_pages(&mut app, &ctx);
        app.push_window_context_for_test(&ctx, 88_990, |owner| {
            crate::app::depth_view_tests::install_pages(owner, &ctx);
        });
        let child =
            app.build_active_context_for_test(Some(88_991), DetachedSource::Image, |owner| {
                crate::app::depth_view_tests::install_pages(owner, &ctx);
                owner.viewer_presentation = ViewerPresentation::DetachedWindow;
                owner.detached_viewer_independent_active = true;
                crate::app::depth_view_tests::install_ready_depth_for_export(owner);
            });
        app.with_viewer_context(child, |owner| {
            owner.mark_active_detached_viewport_rendered(&ctx);
            let FsExclusiveView::Depth(state) = &owner.fs_exclusive_view else {
                unreachable!()
            };
            let paint_pass = ctx.cumulative_pass_nr();
            ctx.data_mut(|data| {
                data.insert_temp(
                    snapshot_id(child),
                    DisplaySnapshot {
                        resource_id: state.ready().unwrap().resource_id,
                        paint_pass,
                        page_idx: 0,
                        items_generation: owner.items_generation,
                        uv: egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                        quarter_turns: 0,
                        paint_size: egui::vec2(200., 300.),
                        underlay: crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::BLACK),
                    },
                )
            });
            owner.open_depth_export(&ctx, 0);
        })
        .unwrap();
        assert_eq!(app.depth_export_owner_context(), Some(child));
        let mut _events = None;
        if phase == 1 || phase == 2 {
            let (job, events, _) = DepthExportJob::controlled_for_test();
            _events = Some(events);
            app.depth_export_operation.as_mut().unwrap().phase = Phase::Running {
                job,
                completed: 0,
                total: 4,
                status: if phase == 1 {
                    RunStatus::Active
                } else {
                    RunStatus::Stopping
                },
            };
        } else if phase == 3 {
            app.depth_export_operation.as_mut().unwrap().phase = Phase::Finished {
                message: "完了".into(),
                failed: false,
            };
        }
        crate::app::depth_setup_entry_tests::assert_independent_modal_protection(
            &mut app, &ctx, child, 88_991, 88_990,
        );
    }
}

#[test]
fn depth_first_pass_in_new_context_cannot_read_previous_panel_or_export_snapshot() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    crate::app::depth_view_tests::install_pages(&mut app, &ctx);
    crate::app::depth_view_tests::install_ready_depth_for_export(&mut app);
    let main = app.viewer_context_main();
    let resource = match &app.fs_exclusive_view {
        FsExclusiveView::Depth(state) => state.ready().unwrap().resource_id,
        _ => unreachable!(),
    };
    let panel = egui::Rect::from_min_size(egui::pos2(100., 100.), egui::vec2(300., 400.));
    let paint_pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| {
        data.insert_temp(app.depth_panel_rect_id(), panel);
        data.insert_temp(
            snapshot_id(main),
            DisplaySnapshot {
                resource_id: resource,
                paint_pass,
                page_idx: 0,
                items_generation: app.items_generation,
                uv: egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                quarter_turns: 0,
                paint_size: panel.size(),
                underlay: crate::ui_fullscreen::FsBgStyle::Solid(egui::Color32::BLACK),
            },
        );
    });
    assert!(app.capture_depth_export(&ctx, true).is_some());
    app.set_fs_exclusive_view(FsExclusiveView::None, "previous_context_finished");
    let child = app.build_active_context_for_test(Some(88_992), DetachedSource::Image, |owner| {
        crate::app::depth_view_tests::install_pages(owner, &ctx);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        owner.detached_viewer_independent_active = true;
        crate::app::depth_view_tests::install_ready_depth_for_export(owner);
    });
    app.with_viewer_context(child, |owner| {
        assert_ne!(
            owner.depth_panel_rect_id(),
            egui::Id::unique(("depth_panel_rect", main))
        );
        assert!(
            ctx.data(|data| data.get_temp::<egui::Rect>(owner.depth_panel_rect_id()))
                .is_none()
        );
        assert!(
            ctx.data(|data| data.get_temp::<DisplaySnapshot>(snapshot_id(child)))
                .is_none()
        );
        // Match the resource identity too: isolation is the id's context component,
        // not accidental rejection of a stale GPU resource id or generation.
        let child_resource = match &owner.fs_exclusive_view {
            FsExclusiveView::Depth(state) => state.ready().unwrap().resource_id,
            _ => unreachable!(),
        };
        ctx.data_mut(|data| {
            let mut previous: DisplaySnapshot = data.get_temp(snapshot_id(main)).unwrap();
            previous.resource_id = child_resource;
            previous.items_generation = owner.items_generation;
            data.insert_temp(snapshot_id(main), previous);
        });
        assert!(owner.capture_depth_export(&ctx, true).is_none());
        owner.open_depth_export(&ctx, 0);
        assert!(!owner.depth_export_dialog_open());
    })
    .unwrap();
    assert_eq!(
        ctx.data(|data| data.get_temp::<egui::Rect>(app.depth_panel_rect_id())),
        Some(panel)
    );
}
