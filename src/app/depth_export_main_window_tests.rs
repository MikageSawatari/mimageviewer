//! Export admission and modal input on the actual ROOT embedded canvas/panel.
use super::*;
use crate::app::depth_main_window_tests::{RootDepthViewer, wheel};
use crate::fs_exclusive_view::FsExclusiveView;
use crate::ring_shortcut::{RingActionId, RingDirection};

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
    viewer.ctx.memory(|memory| {
        let layers: Vec<_> = memory.layer_ids().collect();
        let at = |layer| {
            layers
                .iter()
                .position(|candidate| *candidate == layer)
                .unwrap()
        };
        assert!(at(panel) < at(backdrop));
        assert_eq!(at(backdrop) + 1, at(dialog));
        assert_eq!(memory.top_modal_layer(), Some(backdrop));
        assert!(!memory.allows_interaction(panel));
        assert!(memory.allows_interaction(dialog));
    });
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
    for batched in [false, true] {
        let mut app = setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
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
