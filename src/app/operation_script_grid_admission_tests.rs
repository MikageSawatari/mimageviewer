use super::operation_script_runtime::{ScriptInputSurface, ScriptRunResult};
use super::tests::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use crate::operation_script::{CompiledOperationScript, OperationScriptState};
use std::ops::ControlFlow;

const NOTICE: &str = "script-open-success";

fn fixture(operation: &str) -> AppTestEnv {
    let mut app = setup_app();
    let folder = app.tmp.path().join("script-grid-admission");
    app.current_folder = Some(folder.clone());
    app.items = vec![GridItem::Image(folder.join("page.png"))];
    app.visible_indices = vec![0];
    app.selected = Some(0);
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.operation_script_enabled = true;
    let source = format!(
        "fn slots() {{ [#{{slot:1,label:\"open\",run:\"entry\"}}] }} \
         fn entry(s) {{ #{{run:[\"{operation}\"],notice:\"{NOTICE}\"}} }}"
    );
    app.operation_script.state =
        OperationScriptState::Ready(Box::new(CompiledOperationScript::compile(&source).unwrap()));
    app
}

fn placeholder(app: &AppTestEnv) -> GridItem {
    GridItem::CollectionPlaceholder {
        path: app.tmp.path().join("missing.png"),
        last_known_kind: crate::collection_store::CollectionResolvedKind::Image,
        reason: crate::grid_item::CollectionPlaceholderReason::Missing,
    }
}

fn run(app: &mut AppTestEnv) -> ScriptRunResult {
    app.run_script_slot(
        &egui::Context::default(),
        1,
        ScriptInputSurface::Grid,
        false,
    )
}

fn assert_rejected_notice(app: &AppTestEnv, result: ScriptRunResult) {
    assert!(matches!(
        result,
        ScriptRunResult::Grid {
            nav: None,
            rejected: true
        }
    ));
    let feedback = &app
        .fs_feedback_toast
        .as_ref()
        .expect("rejection feedback")
        .0;
    assert!(feedback.contains("GridOpenSelected"), "{feedback}");
    assert!(!feedback.contains(NOTICE), "{feedback}");
}

#[test]
fn script_grid_open_selected_placeholder_and_history_guard_reject_success_notice() {
    for history_guard in [false, true] {
        let mut app = fixture("GridOpenSelected");
        if history_guard {
            app.items[0] = GridItem::Folder(app.tmp.path().join("missing-folder"));
            app.items_are_reading_history_view = true;
        } else {
            app.items[0] = placeholder(&app);
        }
        let result = run(&mut app);
        assert_rejected_notice(&app, result);
        assert!(app.fullscreen_idx.is_none());
        assert!(app.top_level_grid_view.open_path_classification().is_none());
    }
}

#[test]
fn script_grid_open_selected_flow_is_independent_of_admission() {
    let ctx = egui::Context::default();
    let mut app = fixture("GridOpenSelected");
    app.items[0] = placeholder(&app);
    let result = app.execute_grid_open_selected(&ctx, false);
    assert_eq!(result.admission, ActionAdmission::Rejected);
    assert!(matches!(result.flow, ControlFlow::Continue(())));

    app.items[0] = GridItem::Folder(app.tmp.path().join("missing-folder"));
    app.items_are_reading_history_view = true;
    let result = app.execute_grid_open_selected(&ctx, false);
    assert_eq!(result.admission, ActionAdmission::Rejected);
    assert!(matches!(result.flow, ControlFlow::Break(None)));

    app.items_are_reading_history_view = false;
    app.settings.auto_fullscreen_zip_pdf = false;
    let result = app.execute_grid_open_selected(&ctx, false);
    assert_eq!(result.admission, ActionAdmission::Accepted);
    assert!(matches!(result.flow, ControlFlow::Break(Some(_))));
}

#[test]
fn script_grid_open_selected_classification_acceptance_keeps_enter_continue_flow() {
    for script in [false, true] {
        let mut app = fixture("GridOpenSelected");
        app.items[0] = GridItem::ConvertibleArchive {
            path: app.tmp.path().join("book.rar"),
            format: crate::archive_converter::ArchiveFormat::Rar,
        };
        if script {
            assert!(matches!(
                run(&mut app),
                ScriptRunResult::Grid {
                    nav: None,
                    rejected: false
                }
            ));
            assert_eq!(app.fs_feedback_toast.as_ref().unwrap().0, NOTICE);
        } else {
            let result = app.execute_grid_open_selected(&egui::Context::default(), false);
            assert_eq!(result.admission, ActionAdmission::Accepted);
            assert!(matches!(result.flow, ControlFlow::Continue(())));
        }
        assert!(app.top_level_grid_view.open_path_classification().is_some());
        assert!(
            app.fullscreen_idx.is_none(),
            "classification has not completed"
        );
    }
}

#[test]
fn script_grid_open_selected_missing_bookmark_rejects_and_preserves_enter_break() {
    let mut app = fixture("GridOpenSelected");
    app.items_are_bookmark_view = true;
    app.bookmark_browser_rows = vec![crate::bookmark_browser::BookmarkBrowserRow {
        source: crate::bookmark_browser::BookmarkRowSource::Media {
            id: 1,
            path: app.tmp.path().join("missing.mp4"),
            pts_secs: 0.0,
            title: None,
            is_audio: false,
        },
        item: app.items[0].clone(),
        relative_page_provenance: None,
        image_meta: None,
        marker_thumbnail: None,
        created_at_ms: 0,
        missing: true,
    }];
    let result = app.execute_grid_open_selected(&egui::Context::default(), false);
    assert_eq!(result.admission, ActionAdmission::Rejected);
    assert!(matches!(result.flow, ControlFlow::Break(None)));
    let result = run(&mut app);
    assert_rejected_notice(&app, result);
}

#[test]
fn script_grid_parent_and_history_guards_do_not_show_success_notice() {
    for operation in ["GridParentFolder", "GridHistoryBack", "GridHistoryForward"] {
        let mut app = fixture(operation);
        if operation == "GridParentFolder" {
            app.show_search_bar = true;
            app.search_filter = Some([0].into_iter().collect());
            app.search_filter_origin_folder = app.current_folder.clone();
            assert!(app.local_search_blocks_parent_nav());
        } else {
            app.global_search.active = true;
        }
        assert!(matches!(
            run(&mut app),
            ScriptRunResult::Grid {
                nav: None,
                rejected: true
            }
        ));
        assert!(!app.fs_feedback_toast.as_ref().unwrap().0.contains(NOTICE));
    }
}

#[test]
fn browser_repeat_stack_refresh_consumer_preserves_script_and_navigation_rules() {
    use crate::ring_shortcut::RingActionId;
    for forward in [false, true] {
        for repeat in [false, true] {
            let mut app = fixture("GridColumnCount3");
            app.settings.grid_cols = 2;
            let profile = &mut app.settings.ring_shortcuts.mouse_buttons_grid;
            profile.back = RingActionId::ScriptSlot1;
            profile.forward = RingActionId::ScriptSlot1;
            let ctx = egui::Context::default();
            ctx.begin_pass(Default::default());
            assert!(!app.shortcuts_blocked_by_text_input(&ctx));
            let press = crate::MouseNavPresses {
                count: 1,
                has_non_repeat: !repeat,
            };
            let empty = crate::MouseNavPresses::default();
            let (back, next) = if forward {
                (empty, press)
            } else {
                (press, empty)
            };
            let nav = app.handle_stack_refresh_keyboard(
                &ctx,
                true,
                back,
                next,
                crate::filename_stack_ui::GridItemInputBlock::StackRefresh,
            );
            assert!(nav.is_none());
            assert_eq!(app.settings.grid_cols, if repeat { 2 } else { 3 });

            app.settings.ring_shortcuts.mouse_buttons_grid.back =
                RingActionId::OpenLocationDriveList;
            app.settings.ring_shortcuts.mouse_buttons_grid.forward =
                RingActionId::OpenLocationDriveList;
            let nav = app.handle_stack_refresh_keyboard(
                &ctx,
                true,
                back,
                next,
                crate::filename_stack_ui::GridItemInputBlock::StackRefresh,
            );
            assert!(matches!(
                nav,
                Some(crate::ui_main::AddressBarNav::LocationJump(
                    crate::ui_main::LocationJumpTarget::DriveList
                ))
            ));
            ctx.end_pass().drop_without_applying_deltas();
        }
    }
}

#[test]
#[cfg(windows)]
fn script_grid_open_zipdir_refusal_preserves_legacy_outcome_and_rejects_notice() {
    let mut app = fixture("GridOpenSelected");
    let backing = app.tmp.path().join("backing.zip");
    let source = app.tmp.path().join("source.rar");
    let tree = crate::zip_tree::ZipTree::build(
        backing.clone(),
        ["A/001.png", "B/001.png"]
            .into_iter()
            .map(|name| crate::zip_loader::ZipImageEntry {
                entry_name: name.into(),
                uncompressed_size: 1,
                mtime: 0,
                container_ancestry: Vec::new(),
            })
            .collect(),
    );
    app.current_folder = Some(backing.clone());
    app.zip_nav = Some(crate::zip_tree::ZipNavState::new(std::sync::Arc::new(tree)));
    app.archive_source_override = Some(source);
    app.items[0] = GridItem::ZipDir {
        zip_path: backing,
        dir_prefix: "A/".into(),
        is_archive: false,
        representative: Some("A/001.png".into()),
    };
    app.settings.detached_viewer_open_images_in_window = true;
    app.settings.archive_file_handling = crate::settings::ArchiveFileHandling::Ignore;
    let (legacy, admission) = app.activate_main_zipdir_with_admission(0, &egui::Context::default());
    assert_eq!(legacy, ZipDirActivationOutcome::BookRequested);
    assert_eq!(admission, ActionAdmission::Rejected);
    assert!(
        !app.fs_feedback_toast
            .as_ref()
            .unwrap()
            .0
            .contains("内部の本を別ウィンドウ")
    );
    let result = run(&mut app);
    assert_rejected_notice(&app, result);
    assert!(app.active_detached_window_id().is_none());
}

#[test]
#[cfg(windows)]
fn script_grid_open_same_media_acceptance_is_distinct_from_presentation_guard() {
    let mut app = fixture("GridOpenSelected");
    let ctx = egui::Context::default();
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    app.last_viewer_sync_stamp = app.viewer_sync_stamp_for_idx(0);
    assert!(app.last_viewer_sync_stamp.is_some());
    assert!(matches!(
        app.prepare_detached_context_for_grid_open_with_admission(&ctx, 0),
        ControlFlow::Break(ActionAdmission::Accepted)
    ));
    assert!(!app.prepare_detached_context_for_grid_open(&ctx, 0));

    app.video_presentation_transition
        .reset_stable(ViewerPresentation::DetachedWindow);
    app.video_presentation_transition.request_transition(
        ViewerPresentation::MainWindow,
        false,
        false,
        0,
        None,
        None,
    );
    assert!(
        !app.video_presentation_transition
            .z_order_recovery_permitted()
    );
    assert!(matches!(
        app.prepare_detached_context_for_grid_open_with_admission(&ctx, 0),
        ControlFlow::Break(ActionAdmission::Rejected)
    ));
    assert!(!app.prepare_detached_context_for_grid_open(&ctx, 0));
}
