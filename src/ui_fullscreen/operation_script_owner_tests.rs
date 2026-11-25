//! Stage 2b: handler requests stay with the drawing owner until its final UI.
use super::*;
use crate::operation_script::{CompiledOperationScript, OperationScriptState};
use crate::ring_shortcut::{
    RightDragCommand, RightDragContext, RingActionId, RingDirection, RingShortcutContext,
};

fn ready(ctx: &egui::Context, operations: &str) -> crate::app::AppTestEnvForTest {
    install_fs_navigator_input_tracking(ctx);
    crate::key_input::install_operation_input_plugin(ctx);
    let mut app = page_turn_submission_tests::ready_app(ctx, SpreadMode::Single);
    app.settings.operation_script_enabled = true;
    app.settings.page_turn_animation_enabled = false;
    app.settings.video_in_window_mode = true;
    app.viewer_presentation = crate::app::ViewerPresentation::MainWindow;
    #[cfg(windows)]
    {
        app.native_video_in_window_active = true;
    }
    app.keymap = crate::keymap::Keymap::from_ini_str("[Global]\nScriptSlot1 = F16\n");
    let source = format!(
        "fn slots() {{ [#{{slot:1,label:\"owner\",run:\"entry\"}}] }}\nfn entry(s) {{ {operations} }}"
    );
    app.operation_script.state =
        OperationScriptState::Ready(Box::new(CompiledOperationScript::compile(&source).unwrap()));
    for idx in 0..app.items.len() {
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
    }
    app
}

fn input(events: Vec<egui::Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(600.0, 400.0),
        )),
        focused: true,
        events,
        ..Default::default()
    }
}

fn key(repeat: bool) -> egui::Event {
    egui::Event::Key {
        key: egui::Key::F16,
        physical_key: None,
        pressed: true,
        repeat,
        modifiers: egui::Modifiers::NONE,
    }
}

fn finish(app: &mut App, ctx: &egui::Context, frame: &mut FsScriptFrame, action: &mut FsKeyAction) {
    app.finish_fs_script_frame(ctx, frame, action, &mut None, false, false);
}

fn toast(app: &App) -> &str {
    app.fs_feedback_toast.as_ref().map_or("", |t| t.0.as_str())
}

fn recognition(action: RingActionId, gesture: bool) -> FsSecondaryAction {
    let command = if gesture {
        RightDragCommand::MouseGesture {
            context: RightDragContext::ImageFullscreen,
            action,
            pattern_label: "R".into(),
            show_result_toast: false,
        }
    } else {
        RightDragCommand::RingShortcut {
            context: RingShortcutContext::ImageFullscreen,
            action,
            direction: RingDirection::Right,
            source: "script-owner-test",
        }
    };
    FsSecondaryAction::Recognized {
        context: RightDragContext::ImageFullscreen,
        pos: egui::pos2(300.0, 200.0),
        egui_click: false,
        recognition: crate::ring_shortcut::RightDragRecognition::command(command),
    }
}

#[test]
fn operation_script_owner_all_entries_defer_until_tail_and_dispatch_navigation_once() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for entry in [
        FsScriptEntry::Key,
        FsScriptEntry::ForwardedKey,
        FsScriptEntry::Pointer,
        FsScriptEntry::ContextMenu,
    ] {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, r#"["FsZoomIn","FsPageNext"]"#);
        ctx.run_pass(input(vec![]), |ctx| {
            let mut frame = FsScriptFrame::default();
            let zoom = app.fs_zoom;
            app.collect_fs_script_slot(&mut frame, Some(1), entry);
            assert_eq!(app.fs_zoom, zoom);
            assert_eq!(app.fullscreen_idx, Some(0));
            let mut action = FsKeyAction::default();
            finish(&mut app, ctx, &mut frame, &mut action);
            assert!(app.fs_zoom > zoom);
            assert_eq!(
                app.fullscreen_idx,
                Some(0),
                "navigation belongs to the outer dispatcher"
            );
            assert_eq!(action.page_nav, FsPageNav::Delta(1));
            app.finish_fullscreen_key_action(ctx, 0, action);
            assert_eq!(app.fullscreen_idx, Some(1));
            // The local request was spent; a second tail cannot run it again.
            let zoom = app.fs_zoom;
            finish(&mut app, ctx, &mut frame, &mut FsKeyAction::default());
            assert_eq!(app.fs_zoom, zoom);
        })
        .drop_without_applying_deltas();
    }
}

#[cfg(windows)]
#[test]
fn operation_script_owner_real_embedded_render_runs_key_at_tail_and_rejects_repeat() {
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, r#"["FsSpreadRtl"]"#);
    assert!(app.fullscreen_embedded_still_active());
    for repeat in [false, true] {
        if repeat {
            app.spread_mode = SpreadMode::Single;
        }
        ctx.run_ui(input(vec![key(repeat)]), |ui| {
            app.render_fullscreen_viewport(ui)
        })
        .drop_without_applying_deltas();
        assert_eq!(
            app.spread_mode,
            if repeat {
                SpreadMode::Single
            } else {
                SpreadMode::Rtl
            },
            "the tail's spread setting must survive popup restore/reapply"
        );
        crate::key_input::clear_test_frame();
    }
}

#[cfg(windows)]
#[test]
fn operation_script_owner_real_child_render_runs_in_owning_pass_and_navigates_once() {
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    let child_id = std::rc::Rc::new(std::cell::Cell::new(egui::ViewportId::ROOT));
    let observed = child_id.clone();
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        observed.set(viewport.ids.this);
        let mut raw = input(vec![key(false)]);
        raw.viewport_id = viewport.ids.this;
        raw.viewports.insert(
            viewport.ids.this,
            egui::ViewportInfo {
                parent: Some(egui::ViewportId::ROOT),
                focused: Some(true),
                ..Default::default()
            },
        );
        ctx.run_ui(raw, |ui| (viewport.viewport_ui_cb)(ui))
            .drop_without_applying_deltas();
    });
    let mut app = ready(&ctx, r#"["FsPageNext"]"#);
    app.settings.video_in_window_mode = false;
    app.native_video_in_window_active = false;
    app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
    assert!(!app.fullscreen_embedded_still_active());
    ctx.run_ui(input(vec![]), |ui| app.render_fullscreen_viewport(ui))
        .drop_without_applying_deltas();
    assert_ne!(child_id.get(), egui::ViewportId::ROOT);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(ctx.viewport_id(), egui::ViewportId::ROOT);
    crate::key_input::clear_test_frame();
}

#[test]
fn operation_script_owner_ring_gesture_and_middle_return_requests_without_execution() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for gesture in [false, true] {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, r#"["FsZoomIn"]"#);
        ctx.run_pass(input(vec![]), |ctx| {
            let zoom = app.fs_zoom;
            let outcome = app.apply_fs_secondary_input(
                ctx,
                recognition(RingActionId::ScriptSlot1, gesture),
                FullscreenSidePanelClickChrome::default(),
            );
            let FsSecondaryInputOutcome::ScriptSlot(request) = outcome else {
                panic!("slot request")
            };
            assert_eq!(request.slot, 1);
            assert_eq!(app.fs_zoom, zoom);
            let mut frame = FsScriptFrame::default();
            app.collect_fs_script_request_value(&mut frame, request);
            finish(&mut app, ctx, &mut frame, &mut FsKeyAction::default());
            assert!(app.fs_zoom > zoom);
        })
        .drop_without_applying_deltas();
    }
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, r#"["FsZoomIn"]"#);
    app.settings.ring_shortcuts.mouse_buttons_image.middle = RingActionId::ScriptSlot1;
    let zoom = app.fs_zoom;
    ctx.run_pass(input(vec![]), |ctx| {
        assert_eq!(app.dispatch_viewer_middle_short_click(ctx), Some(1));
        assert_eq!(app.fs_zoom, zoom);
    })
    .drop_without_applying_deltas();
}

#[test]
fn operation_script_owner_same_list_home_invalidates_earlier_request() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, r#"["FsZoomIn"]"#);
    app.fullscreen_idx = Some(2);
    app.settings.ring_shortcuts.mouse_buttons_image.middle = RingActionId::ImageHome;
    let generation = app.items_generation;
    let destination = app.page_turn_destination();
    ctx.run_pass(input(vec![]), |ctx| {
        let mut frame = FsScriptFrame::default();
        app.collect_fs_script_slot(&mut frame, Some(1), FsScriptEntry::Key);
        assert_eq!(app.dispatch_viewer_middle_short_click(ctx), None);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.page_turn_destination(), destination);
        let zoom = app.fs_zoom;
        finish(&mut app, ctx, &mut frame, &mut FsKeyAction::default());
        assert_eq!(app.fs_zoom, zoom);
        app.notify_fs_script_frame(&mut frame);
        assert!(toast(&app).contains("表示対象が変わった"));
    })
    .drop_without_applying_deltas();
}

#[test]
fn operation_script_owner_shift_reset_changes_unit_identity_without_moving_anchor() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, r#"["FsZoomIn"]"#);
    app.items.truncate(3);
    app.visible_indices = vec![0, 1, 2];
    app.spread_mode = SpreadMode::Ltr;
    app.reading_direction = ReadingDirection::Ltr;
    app.fullscreen_idx = Some(1);
    app.set_spread_shift_anchor_for_test(Some(1));
    ctx.run_pass(input(vec![]), |ctx| {
        let pages = |app: &mut App| {
            app.spread_display_composition_for_anchor(1)
                .pages_in_reading_order()
                .iter()
                .filter(|page| page.role == SpreadPageRole::Navigation)
                .map(|page| page.idx)
                .collect::<Vec<_>>()
        };
        assert_eq!(pages(&mut app), vec![1, 2]);
        let before = app.operation_script_page_identity();
        let mut frame = FsScriptFrame::default();
        app.collect_fs_script_slot(&mut frame, Some(1), FsScriptEntry::Key);
        let (_, rejected, delegated) =
            app.execute_fullscreen_script_action(ctx, KeyAction::FsReadingDirectionToggle);
        assert!(!rejected && !delegated);
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_eq!(app.spread_shift.anchor_idx(), None);
        assert_eq!(pages(&mut app), vec![0, 1]);
        assert_ne!(app.operation_script_page_identity(), before);
        let zoom = app.fs_zoom;
        finish(&mut app, ctx, &mut frame, &mut FsKeyAction::default());
        assert_eq!(app.fs_zoom, zoom);
        app.notify_fs_script_frame(&mut frame);
        assert!(toast(&app).contains("表示対象が変わった"));
    })
    .drop_without_applying_deltas();
}

#[test]
fn operation_script_page_composition_inputs_share_arc_and_ignore_publication_and_geometry() {
    let book = crate::book_composition::fixtures::four_page_book(
        crate::book_composition::ReadingDirection::Ltr,
    );
    let mut app = book_composition_tests::composed_app(&book);
    app.fullscreen_idx = Some(1);
    let before = app.operation_script_page_identity();
    let original = Arc::clone(app.retained_composed_projection_input().unwrap());
    let mut republished = original.as_ref().clone();
    republished.token.service_epoch += 1;
    republished.token.publication_sequence += 1;
    republished.resolved.revision += 1;
    republished.prepared_rotations = Arc::new(HashMap::from([(
        "late rotation".into(),
        crate::rotation_db::Rotation::Cw90,
    )]));
    app.install_composition_for_test(Arc::new(republished.clone()));
    assert_eq!(
        app.operation_script_page_identity(),
        before,
        "semantic inputs survive republishing and late rotation"
    );
    let mut overridden = republished.clone();
    overridden
        .resolved
        .page_overrides
        .insert(0, crate::book_composition::PageOverride::Alone);
    app.install_composition_for_test(Arc::new(overridden));
    assert_ne!(
        app.operation_script_page_identity(),
        before,
        "a preceding directive can change pairing at the same anchor"
    );
    app.install_composition_for_test(Arc::new(republished.clone()));
    let mut reordered = republished.clone();
    reordered.resolved.visible_pages.swap(0, 1);
    app.install_composition_for_test(Arc::new(reordered));
    assert_ne!(app.operation_script_page_identity(), before);
    let mut renamed_slots = republished.clone();
    for page in &mut renamed_slots.resolved.visible_pages {
        page.item_index += 1;
    }
    renamed_slots.resolved.page_overrides = renamed_slots
        .resolved
        .page_overrides
        .into_iter()
        .map(|(idx, value)| (idx + 1, value))
        .collect();
    app.items
        .insert(0, GridItem::Folder("C:/non-reader-row".into()));
    app.visible_indices = vec![1, 2, 3, 4];
    app.details_order = app.visible_indices.clone();
    app.fullscreen_idx = Some(2);
    app.install_composition_for_test(Arc::new(renamed_slots));
    assert_eq!(
        app.operation_script_page_identity(),
        before,
        "typed page order and directives ignore row reindexing"
    );
}

#[test]
fn operation_script_page_composition_identity_retains_input_through_pending_refresh() {
    use crate::app::book_composition_runtime::{
        BookCompositionSource, ProjectionOrigin, ViewerBookComposition,
    };
    let book = crate::book_composition::fixtures::four_page_book(
        crate::book_composition::ReadingDirection::Ltr,
    );
    let mut app = book_composition_tests::composed_app(&book);
    app.fullscreen_idx = Some(1);
    let before = app.operation_script_page_identity();
    let projection = Arc::clone(app.retained_composed_projection_input().unwrap());
    let (_sender, receiver) = std::sync::mpsc::channel();
    app.book_composition = ViewerBookComposition::Preparing {
        scope: projection.scope.clone(),
        receiver,
        origin: Box::new(ProjectionOrigin::Refresh {
            source: BookCompositionSource::Unavailable,
            previous: Some(Arc::clone(&projection)),
            anchor: None,
            previous_order: Arc::from(Vec::new()),
        }),
    };
    assert!(Arc::ptr_eq(
        app.retained_composed_projection_input().unwrap(),
        &projection
    ));
    assert_eq!(
        app.operation_script_page_identity(),
        before,
        "preparation state alone is not a page change"
    );
}

#[test]
fn operation_script_page_identity_tracks_hidden_row_removal_that_changes_complete_book() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, "[]");
    app.current_folder = Some(app.tmp.path().join("complete-book"));
    app.spread_mode = SpreadMode::LtrCover;
    app.settings.page_after_cover_alone_enabled = false;
    app.settings.last_page_alone_enabled = true;
    app.settings.final_cover_spread_enabled = false;
    app.settings.singleton_spread_first_enabled = false;
    app.settings.singleton_spread_last_enabled = false;
    app.visible_indices = vec![0, 1, 2];
    app.fullscreen_idx = Some(1);
    let pages = |app: &mut App| {
        app.spread_display_composition_for_anchor(1)
            .pages_in_reading_order()
            .iter()
            .filter(|page| page.role == SpreadPageRole::Navigation)
            .map(|page| page.idx)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        pages(&mut app),
        vec![1, 2],
        "a hidden fourth row makes the reader incomplete"
    );
    let before = app.operation_script_page_identity();
    app.items.pop();
    app.items_generation += 1;
    app.viewer_navigation_caches.invalidate();
    assert_eq!(
        pages(&mut app),
        vec![1],
        "the complete three-page book has a forced last singleton"
    );
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_ne!(app.operation_script_page_identity(), before);
}

#[test]
fn operation_script_owner_discard_conditions_do_not_run_or_retain_requests() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for reason in 0..7 {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, r#"["FsZoomIn"]"#);
        if reason == 2 {
            app.page_identity_index = Arc::new(
                crate::app::file_type_resume::PreparedPageIdentityIndex::prepare(
                    &app.items,
                    None,
                    app.settings.file_type_visibility.clone(),
                ),
            );
        }
        ctx.run_pass(input(vec![]), |ctx| {
            let mut frame = FsScriptFrame::default();
            app.collect_fs_script_slot(&mut frame, Some(1), FsScriptEntry::Key);
            match reason {
                0 => app.items_generation += 1,
                1 => app.fullscreen_page_slice = crate::page_split::PageSlice::Left,
                2 => {
                    assert!(app.open_page_overview_for_viewer(ctx));
                    assert!(app.page_overview.session().is_some());
                }
                3 => app.discard_fs_script_request(&mut frame, "ViewerReplaced"),
                4 => app.discard_fs_script_request(&mut frame, "frame_exited"),
                5 => app.discard_fs_script_request(&mut frame, "frame_path_changed"),
                _ => {}
            }
            let zoom = app.fs_zoom;
            app.finish_fs_script_frame(
                ctx,
                &mut frame,
                &mut FsKeyAction::default(),
                &mut None,
                reason == 6,
                false,
            );
            assert_eq!(app.fs_zoom, zoom, "reason={reason}");
            assert!(!matches!(frame.request, FsScriptRequestState::Pending(_)));
            app.notify_fs_script_frame(&mut frame);
            assert!(!toast(&app).is_empty(), "reason={reason}");
        })
        .drop_without_applying_deltas();
    }
}

#[test]
fn operation_script_owner_duplicates_keep_first_and_notify_second() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, r#"#{run:[],notice:"done"}"#);
    let mut frame = FsScriptFrame::default();
    app.collect_fs_script_slot(&mut frame, Some(1), FsScriptEntry::Key);
    app.collect_fs_script_slot(&mut frame, Some(9), FsScriptEntry::ContextMenu);
    assert!(matches!(&frame.request, FsScriptRequestState::Pending(request) if request.slot == 1));
    finish(&mut app, &ctx, &mut frame, &mut FsKeyAction::default());
    app.notify_fs_script_frame(&mut frame);
    assert!(toast(&app).contains("スクリプト操作 9"));
    let first = FsScriptRequest {
        slot: 1,
        entry: FsScriptEntry::Pointer,
        identity: app.fs_script_display_identity(),
        duplicate_slots: Vec::new(),
    };
    let second = FsScriptRequest {
        slot: 2,
        entry: FsScriptEntry::Pointer,
        identity: app.fs_script_display_identity(),
        duplicate_slots: Vec::new(),
    };
    let request = app
        .merge_fs_pointer_script_slots(Some(first), Some(second))
        .unwrap();
    assert_eq!(request.slot, 1);
    let mut frame = FsScriptFrame::default();
    app.collect_fs_script_request_value(&mut frame, request);
    finish(&mut app, &ctx, &mut frame, &mut FsKeyAction::default());
    app.notify_fs_script_frame(&mut frame);
    assert!(toast(&app).contains("スクリプト操作 2"));
}

#[test]
fn operation_script_owner_duplicate_notice_survives_navigation_destination_toast() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, r#"["FsPageNext"]"#);
    app.adjustment_page_params
        .insert(1, crate::adjustment::AdjustParams::default());
    ctx.run_pass(input(vec![]), |ctx| {
        let mut frame = FsScriptFrame::default();
        app.collect_fs_script_slot(&mut frame, Some(1), FsScriptEntry::Key);
        app.collect_fs_script_slot(&mut frame, Some(9), FsScriptEntry::ContextMenu);
        let mut action = FsKeyAction::default();
        finish(&mut app, ctx, &mut frame, &mut action);
        app.finish_fullscreen_key_action(ctx, 0, action);
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_eq!(
            toast(&app),
            "ページ補正適用",
            "fixture must exercise the normal destination notification"
        );
        app.notify_fs_script_frame(&mut frame);
        assert!(toast(&app).contains("スクリプト操作 9"));
    })
    .drop_without_applying_deltas();
}

#[test]
fn operation_script_owner_discard_and_rejected_notices_survive_existing_navigation() {
    for rejected in [false, true] {
        let ctx = egui::Context::default();
        let mut app = ready(
            &ctx,
            if rejected {
                r#"["FsZoomIn","FsClose"]"#
            } else {
                r#"["FsZoomIn"]"#
            },
        );
        app.adjustment_page_params
            .insert(1, crate::adjustment::AdjustParams::default());
        ctx.run_pass(input(vec![]), |ctx| {
            let mut frame = FsScriptFrame::default();
            app.collect_fs_script_slot(&mut frame, Some(1), FsScriptEntry::Key);
            if !rejected {
                app.fullscreen_idx = Some(2);
            }
            let mut action = FsKeyAction {
                page_nav: FsPageNav::Delta(if rejected { 1 } else { -1 }).into(),
                ..Default::default()
            };
            let zoom = app.fs_zoom;
            finish(&mut app, ctx, &mut frame, &mut action);
            assert_eq!(app.fs_zoom, zoom);
            let idx = app.fullscreen_idx.unwrap();
            app.finish_fullscreen_key_action(ctx, idx, action);
            assert_eq!(app.fullscreen_idx, Some(1));
            assert_eq!(toast(&app), "ページ補正適用");
            app.notify_fs_script_frame(&mut frame);
            assert!(toast(&app).contains(if rejected {
                "操作の指定"
            } else {
                "表示対象が変わった"
            }));
        })
        .drop_without_applying_deltas();
    }
}

#[test]
fn operation_script_owner_rebased_interruption_warning_survives_existing_navigation() {
    // Both requests were accepted before the script changes the spread mode.
    for (accepted_nav, destination) in [(FsPageNav::Delta(1), 1), (FsPageNav::Target(2), 2)] {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, r#"["FsSpreadRtl","FsZoomIn"]"#);
        app.adjustment_page_params
            .insert(destination, crate::adjustment::AdjustParams::default());
        ctx.run_pass(input(vec![]), |ctx| {
            let mut frame = FsScriptFrame::default();
            app.collect_fs_script_slot(&mut frame, Some(1), FsScriptEntry::ContextMenu);
            let mut action = FsKeyAction {
                page_nav: accepted_nav.into(),
                ..Default::default()
            };
            let zoom = app.fs_zoom;
            finish(&mut app, ctx, &mut frame, &mut action);
            assert!(matches!(
                action.page_transition,
                FsKeyPageTransition::Rebased { .. }
            ));
            assert_eq!(app.spread_mode, SpreadMode::Rtl);
            assert_eq!(
                action.page_nav, accepted_nav,
                "Rebased must retain the already accepted navigation"
            );
            assert_eq!(
                app.fs_zoom, zoom,
                "Rebased ends the script before its zoom action"
            );
            assert!(frame.notices.iter().any(|feedback| matches!(feedback,
                ScriptFeedback::Warning(text) if text.contains("残りの操作を中止"))));
            app.finish_fullscreen_key_action(ctx, 0, action);
            assert_eq!(app.fullscreen_idx, Some(destination));
            assert_eq!(toast(&app), "ページ補正適用");
            app.notify_fs_script_frame(&mut frame);
            assert!(toast(&app).contains("残りの操作を中止"));
            assert_eq!(
                app.feedback_toasts_for_test.last().map(|(_, kind)| *kind),
                Some(crate::feedback::FeedbackKind::Warning)
            );
        })
        .drop_without_applying_deltas();
    }
}

#[test]
fn operation_script_owner_success_notice_keeps_info_kind_after_navigation() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, r#"#{run:["FsPageNext"],notice:"done"}"#);
    app.adjustment_page_params
        .insert(1, crate::adjustment::AdjustParams::default());
    ctx.run_pass(input(vec![]), |ctx| {
        let mut frame = FsScriptFrame::default();
        app.collect_fs_script_slot(&mut frame, Some(1), FsScriptEntry::ContextMenu);
        let mut action = FsKeyAction::default();
        finish(&mut app, ctx, &mut frame, &mut action);
        assert!(
            frame
                .notices
                .iter()
                .any(|feedback| matches!(feedback, ScriptFeedback::Notice(text) if text == "done"))
        );
        app.finish_fullscreen_key_action(ctx, 0, action);
        assert_eq!(toast(&app), "ページ補正適用");
        app.notify_fs_script_frame(&mut frame);
        assert_eq!(toast(&app), "done");
        assert_eq!(
            app.feedback_toasts_for_test.last().map(|(_, kind)| *kind),
            Some(crate::feedback::FeedbackKind::Info)
        );
    })
    .drop_without_applying_deltas();
}

#[test]
fn operation_script_owner_conflicting_navigation_rejects_whole_bc_but_allows_a() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for kind in 0..12 {
        for operations in [
            r#"["FsZoomIn","FsPageNext"]"#,
            r#"["FsZoomIn","FsClose"]"#,
            r#"["FsZoomIn"]"#,
        ] {
            let ctx = egui::Context::default();
            let mut app = ready(&ctx, operations);
            ctx.run_pass(input(vec![]), |ctx| {
                let mut action = FsKeyAction::default();
                let mut frame = FsScriptFrame::default();
                app.collect_fs_script_slot(&mut frame, Some(1), FsScriptEntry::Key);
                match kind {
                    0 => action.close = true,
                    1 => action.close_to_page_list = true,
                    2 => action.page_nav = FsPageNav::Delta(-1).into(),
                    3 => action.ctrl_nav = Some(-1),
                    4 => action.list_order_nav = Some(false),
                    5 => action.sibling_nav = Some(-1),
                    6 => action.mouse_nav = Some(crate::ui_main::AddressBarNav::HistoryBack),
                    7 => action.jump_to = Some(3),
                    8 => frame.navigation_accepted = true,
                    9 => app.mouse_ring_nav = Some(crate::ui_main::AddressBarNav::HistoryBack),
                    11 => app.pending_return_to_parent.request_live(),
                    _ => {}
                }
                let zoom = app.fs_zoom;
                let original = Some(ViewerOperationId::Pad {
                    batch: 363,
                    ordinal: 1,
                });
                let mut operation = original;
                app.finish_fs_script_frame(
                    ctx,
                    &mut frame,
                    &mut action,
                    &mut operation,
                    false,
                    kind == 10,
                );
                assert_eq!(
                    app.fs_zoom > zoom,
                    operations == r#"["FsZoomIn"]"#,
                    "kind={kind} {operations}"
                );
                if kind <= 7 {
                    assert_eq!(operation, original);
                }
                assert_eq!(action.close, kind == 0);
                assert_eq!(action.close_to_page_list, kind == 1);
                assert_eq!(
                    action.page_nav,
                    if kind == 2 {
                        FsPageNav::Delta(-1)
                    } else {
                        FsPageNav::None
                    }
                );
                assert_eq!(action.ctrl_nav, (kind == 3).then_some(-1));
                assert_eq!(action.list_order_nav, (kind == 4).then_some(false));
                assert_eq!(action.sibling_nav, (kind == 5).then_some(-1));
                assert_eq!(action.mouse_nav.is_some(), kind == 6);
                assert_eq!(action.jump_to, (kind == 7).then_some(3));
            })
            .drop_without_applying_deltas();
        }
    }
}

#[test]
fn operation_script_owner_rebased_result_is_returned_without_preparing_again() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, r#"["FsSpreadRtl","FsZoomIn"]"#);
    ctx.run_pass(input(vec![]), |ctx| {
        let mut frame = FsScriptFrame::default();
        app.collect_fs_script_slot(&mut frame, Some(1), FsScriptEntry::Key);
        let zoom = app.fs_zoom;
        let mut action = FsKeyAction::default();
        finish(&mut app, ctx, &mut frame, &mut action);
        assert!(matches!(
            action.page_transition,
            FsKeyPageTransition::Rebased { .. }
        ));
        assert_eq!(app.fs_zoom, zoom, "remaining actions stop at Rebased");
        assert_eq!(app.spread_mode, SpreadMode::Rtl);
    })
    .drop_without_applying_deltas();
}

#[test]
fn operation_script_owner_closed_is_terminal_and_keeps_rating_undo() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, r#"["FsZoomIn"]"#);
    let changes = vec![crate::undo_stack::RatingChange {
        path_key: "c:/page-turn/0.png".into(),
        source_path: PathBuf::from("c:/page-turn/0.png"),
        meta: None,
        xmp_target: None,
        before: 3,
        after: 0,
    }];
    app.meta_undo.push(crate::undo_stack::UndoEntry::Rating {
        changes: changes.clone(),
        origin: None,
        summary: "rating close".into(),
    });
    assert_eq!(app.meta_undo.undo_len(), 1);
    let mut frame = FsScriptFrame::default();
    app.collect_fs_script_slot(&mut frame, Some(1), FsScriptEntry::Key);
    app.fullscreen_idx = None;
    let mut action = FsKeyAction {
        page_transition: FsKeyPageTransition::Closed,
        ..Default::default()
    };
    let zoom = app.fs_zoom;
    finish(&mut app, &ctx, &mut frame, &mut action);
    assert_eq!(app.fs_zoom, zoom);
    assert_eq!(action.page_transition, FsKeyPageTransition::Closed);
    assert!(!action.close);
    assert_eq!(app.meta_undo.undo_len(), 1);
    assert!(matches!(app.meta_undo.peek_undo(),
        Some(crate::undo_stack::UndoEntry::Rating { changes: actual, origin: None, summary })
            if actual == &changes && summary == "rating close"));
}

#[cfg(windows)]
#[test]
fn operation_script_owner_real_rating_closed_exits_render_without_clearing_undo() {
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, r#"["RatingItemClear"]"#);
    assert!(app.fullscreen_embedded_still_active());
    let folder = app.tmp.path().join("script-rating-close");
    std::fs::create_dir_all(&folder).unwrap();
    for idx in 0..app.items.len() {
        let path = folder.join(format!("{idx}.png"));
        image::RgbaImage::from_pixel(4, 4, image::Rgba([70, 90, 110, 255]))
            .save(&path)
            .unwrap();
        app.items[idx] = GridItem::Image(path);
        let rating = if idx == 0 { 3 } else { 4 };
        let key = app.rating_path_key(idx).unwrap();
        app.rating_db.as_ref().unwrap().set(&key, rating).unwrap();
        app.rating_cache.insert(idx, rating);
    }
    app.current_folder = Some(folder);
    app.settings.rating_filter = [false, false, false, true, false, false];
    app.rebuild_visible_indices();
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.visible_indices, vec![0]);
    ctx.run_ui(input(vec![key(false)]), |ui| {
        app.render_fullscreen_viewport(ui)
    })
    .drop_without_applying_deltas();
    assert_eq!(
        app.fullscreen_idx, None,
        "the script's rating action closes the last filtered page"
    );
    assert_eq!(
        app.meta_undo.undo_len(),
        1,
        "Closed must bypass the ordinary close dispatcher"
    );
    assert!(matches!(
        app.meta_undo.peek_undo(),
        Some(crate::undo_stack::UndoEntry::Rating { .. })
    ));
    crate::key_input::clear_test_frame();
}

#[test]
fn operation_script_owner_text_and_ime_are_checked_at_tail_after_collection() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for ime in [false, true] {
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        let mut app = ready(&ctx, r#"["FsZoomIn"]"#);
        let mut frame = FsScriptFrame::default();
        app.collect_fs_script_slot(&mut frame, Some(1), FsScriptEntry::Pointer);
        let events = if ime {
            vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "に".into(),
                    active_range_chars: None,
                }),
            ]
        } else {
            vec![]
        };
        ctx.run_ui(input(events), |ui| {
            let ctx = ui.ctx().clone();
            let mut text = String::new();
            let mut focus = true;
            crate::ime_focus::show_singleline(ui, &mut text, Some(&mut focus), |edit| edit);
            app.update_ime_state(&ctx);
            assert!(matches!(
                app.operation_script_eligibility(
                    &ctx,
                    crate::app::operation_script_runtime::ScriptInputSurface::Viewer
                ),
                crate::app::operation_script_runtime::ScriptEligibility::Silent
            ));
            let zoom = app.fs_zoom;
            finish(&mut app, &ctx, &mut frame, &mut FsKeyAction::default());
            assert_eq!(app.fs_zoom, zoom);
        })
        .drop_without_applying_deltas();
    }
}

#[test]
fn operation_script_owner_child_text_gate_uses_child_pass_instead_of_root() {
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    let mut app = ready(&ctx, r#"["FsZoomIn"]"#);
    app.settings.video_in_window_mode = false;
    #[cfg(windows)]
    {
        app.native_video_in_window_active = false;
    }
    app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
    let mut frame = FsScriptFrame::default();
    app.collect_fs_script_slot(&mut frame, Some(1), FsScriptEntry::Pointer);
    let child = app.fullscreen_viewport_id();
    let mut raw = input(vec![]);
    raw.viewport_id = child;
    raw.viewports.insert(
        child,
        egui::ViewportInfo {
            parent: Some(egui::ViewportId::ROOT),
            focused: Some(true),
            ..Default::default()
        },
    );
    ctx.run_ui(raw, |ui| {
        assert_eq!(ui.ctx().viewport_id(), child);
        let mut text = String::new();
        let mut focus = true;
        crate::ime_focus::show_singleline(ui, &mut text, Some(&mut focus), |edit| edit);
        let zoom = app.fs_zoom;
        finish(&mut app, ui.ctx(), &mut frame, &mut FsKeyAction::default());
        assert_eq!(app.fs_zoom, zoom);
        assert!(toast(&app).is_empty(), "text ownership refuses silently");
    })
    .drop_without_applying_deltas();
}

#[cfg(windows)]
#[test]
fn operation_script_owner_egui_media_buttons_return_slots_and_execute_at_tail() {
    let _input = crate::key_input::lock_test_input();
    use crate::ring_shortcut::MouseButtonSlot;
    for audio in [false, true] {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, r#"["VideoMute"]"#);
        let path = app
            .tmp
            .path()
            .join(if audio { "audio.mp3" } else { "video.mp4" });
        let player_path = path.clone();
        app.items[0] = if audio {
            GridItem::Audio(path)
        } else {
            GridItem::Video(path)
        };
        app.fs_cache.insert(
            0,
            FsCacheEntry::Video {
                player: Box::new(crate::video::VideoPlayer::disconnected_for_test(
                    player_path,
                    0.0,
                )),
                load_seq: 0,
            },
        );
        if !audio {
            app.video_audio_mode = Some(0);
        }
        app.settings.ring_shortcuts.mouse_buttons_video.back = RingActionId::ScriptSlot1;
        app.settings.ring_shortcuts.mouse_buttons_video.forward = RingActionId::ScriptSlot1;
        app.settings.ring_shortcuts.mouse_buttons_video.middle = RingActionId::ScriptSlot1;
        ctx.run_pass(input(vec![]), |ctx| {
            for button in [
                MouseButtonSlot::Back,
                MouseButtonSlot::Forward,
                MouseButtonSlot::Middle,
            ] {
                let result = app.dispatch_mouse_button(
                    ctx,
                    button,
                    crate::app::ActionSurface::Viewer,
                    "script-media-owner-test",
                );
                assert_eq!(result.script_slot, Some(1));
                assert!(result.navigation.is_none());
                let before = app.settings.video_muted;
                let mut frame = FsScriptFrame::default();
                app.collect_fs_script_slot(&mut frame, result.script_slot, FsScriptEntry::Pointer);
                assert_eq!(app.settings.video_muted, before);
                finish(&mut app, ctx, &mut frame, &mut FsKeyAction::default());
                assert_eq!(app.settings.video_muted, !before);
                app.notify_fs_script_frame(&mut frame);
            }
        })
        .drop_without_applying_deltas();
    }
}

#[cfg(windows)]
#[test]
fn operation_script_owner_detached_and_transition_are_rejected_by_common_eligibility() {
    let ctx = egui::Context::default();
    for transition in [false, true] {
        let mut app = ready(&ctx, r#"["FsZoomIn"]"#);
        if transition {
            app.video_presentation_transition
                .reset_stable(crate::app::ViewerPresentation::Fullscreen);
            app.video_presentation_transition.request_transition(
                crate::app::ViewerPresentation::DetachedWindow,
                false,
                false,
                0,
                None,
                None,
            );
        } else {
            app.viewer_presentation = crate::app::ViewerPresentation::DetachedWindow;
        }
        let mut frame = FsScriptFrame::default();
        app.collect_fs_script_slot(&mut frame, Some(1), FsScriptEntry::Pointer);
        let zoom = app.fs_zoom;
        finish(&mut app, &ctx, &mut frame, &mut FsKeyAction::default());
        assert_eq!(app.fs_zoom, zoom);
        app.notify_fs_script_frame(&mut frame);
        assert!(toast(&app).contains("メインウィンドウ"));
        app.video_presentation_transition
            .reset_stable(crate::app::ViewerPresentation::MainWindow);
    }
}

#[test]
fn operation_script_owner_deferred_wheel_script_is_not_run_outside_owner() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, r#"["FsNavigatorToggle"]"#);
    let before = app.settings.fullscreen_navigator_visible;
    let wheel = |y, modifiers| egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, y),
        modifiers,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    };
    ctx.run_ui(
        input(vec![
            egui::Event::PointerMoved(egui::pos2(300.0, 200.0)),
            wheel(-120.0, egui::Modifiers::NONE),
            wheel(120.0, egui::Modifiers::CTRL),
            wheel(-120.0, egui::Modifiers::NONE),
        ]),
        |ui| {
            let (mut result, mut operation) =
                wheel_frame_tests::collect_actual_merge(&mut app, ui, false);
            let mut proposal = result.deferred.take().expect("mixed wheel proposal");
            proposal.secondary_action = recognition(RingActionId::ScriptSlot1, true);
            app.claim_fs_wheel_frame_after_ui(&mut proposal);
            let result = app.apply_fs_wheel_frame_proposal(proposal, false, &mut operation);
            assert!(result.script_slot.is_none());
            assert_eq!(app.settings.fullscreen_navigator_visible, before);
            let mut frame = FsScriptFrame {
                notices: result.script_feedback,
                ..Default::default()
            };
            app.notify_fs_script_frame(&mut frame);
            assert!(toast(&app).contains("保留されたホイール"));
        },
    )
    .drop_without_applying_deltas();
}
