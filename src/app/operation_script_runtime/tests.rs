use super::*;

use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
use crate::operation_script::CompiledOperationScript;

fn ready(body: &str) -> OperationScriptState {
    let source = format!(
        "fn slots() {{ [#{{slot:1,label:\"test\",run:\"entry\"}}] }}\nfn entry(s) {{ {body} }}"
    );
    OperationScriptState::Ready(Box::new(CompiledOperationScript::compile(&source).unwrap()))
}

fn enabled_facts() -> EligibilityFacts {
    EligibilityFacts {
        main_context: true,
        viewer_detached_or_switching: false,
        blocked_input: false,
        remote: false,
        enabled: true,
    }
}

fn grid_app(body: &str) -> AppTestEnv {
    let mut env = setup_app();
    let folder = env.tmp.path().join("script-book");
    env.current_folder = Some(folder.clone());
    env.items = vec![
        GridItem::Image(folder.join("a.jpg")),
        GridItem::Image(folder.join("b.jpg")),
    ];
    env.visible_indices = vec![0, 1];
    env.thumbnails = vec![crate::grid_item::ThumbnailState::Pending; 2];
    env.image_metas = vec![None; 2];
    env.selected = Some(0);
    env.checked.clear();
    env.rating_cache.clear();
    env.tags_cache.clear();
    env.settings.operation_script_enabled = true;
    env.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
    env.settings.grid_cols = 2;
    env.operation_script.state = ready(body);
    env.fs_feedback_toast = None;
    env
}

fn toast(app: &App) -> &str {
    &app.fs_feedback_toast.as_ref().expect("script feedback").0
}

#[test]
fn script_slot_number_covers_the_twenty_global_actions_only() {
    for (index, action) in SCRIPT_SLOT_ACTIONS.iter().copied().enumerate() {
        assert_eq!(script_slot_number(action), Some(index as u8 + 1));
    }
    for action in [
        KeyAction::FsPageNext,
        KeyAction::GridColumnCount1,
        KeyAction::RatingItem3,
    ] {
        assert_eq!(script_slot_number(action), None);
    }
}

#[cfg(windows)]
#[test]
fn browser_repeat_main_grid_consumer_rejects_slots_but_keeps_normal_navigation() {
    let _serial = crate::key_input::lock_test_input();
    for forward in [false, true] {
        for (script, repeat) in [(true, false), (true, true), (false, true)] {
            let ctx = egui::Context::default();
            let mut app = grid_app(r#"["GridColumnCount4"]"#);
            let assignment = if script {
                crate::ring_shortcut::RingActionId::ScriptSlot1
            } else {
                crate::ring_shortcut::RingActionId::OpenLocationDriveList
            };
            app.settings.ring_shortcuts.mouse_buttons_grid.back = assignment.clone();
            app.settings.ring_shortcuts.mouse_buttons_grid.forward = assignment;
            crate::take_pending_mouse_nav();
            crate::inject_browser_keydown_for_test(forward, repeat);
            let _ = ctx
                .run_pass(egui::RawInput::default(), |ctx| {
                    let nav = app.handle_keyboard(ctx);
                    if script {
                        assert!(nav.is_none());
                    } else {
                        assert!(matches!(
                            nav,
                            Some(AddressBarNav::LocationJump(
                                crate::ui_main::LocationJumpTarget::DriveList
                            ))
                        ));
                    }
                })
                .drop_without_applying_deltas();
            assert_eq!(
                app.settings.grid_cols,
                if script && !repeat { 4 } else { 2 }
            );
            crate::key_input::clear_test_frame();
        }
    }
}

#[cfg(windows)]
#[test]
fn browser_mixed_batch_runs_script_once_and_later_repeats_do_not_run_it() {
    let _serial = crate::key_input::lock_test_input();
    for forward in [false, true] {
        let ctx = egui::Context::default();
        let mut app = grid_app(r#"["GridToggleDetailsView"]"#);
        app.settings.ring_shortcuts.mouse_buttons_grid.back =
            crate::ring_shortcut::RingActionId::ScriptSlot1;
        app.settings.ring_shortcuts.mouse_buttons_grid.forward =
            crate::ring_shortcut::RingActionId::ScriptSlot1;
        crate::take_pending_mouse_nav();
        for repeats_only in [false, true] {
            crate::inject_browser_keydown_for_test(forward, repeats_only);
            crate::inject_browser_keydown_for_test(forward, true);
            let _ = ctx
                .run_pass(egui::RawInput::default(), |ctx| {
                    assert!(app.handle_keyboard(ctx).is_none());
                })
                .drop_without_applying_deltas();
            assert_eq!(
                app.settings.grid_view_mode,
                crate::settings::GridViewMode::Details,
                "mixed input toggles exactly once; a later repeat-only batch never toggles"
            );
            crate::key_input::clear_test_frame();
        }
    }
}

#[test]
fn eligibility_prioritizes_owner_then_input_remote_and_enabled_before_loader_state() {
    let ready = ready("[]");
    assert_eq!(
        eligibility_from_facts(
            enabled_facts(),
            ScriptInputSurface::Grid,
            ScriptContext::Grid,
            &ready
        ),
        ScriptEligibility::Available(ScriptContext::Grid),
    );
    let failed = OperationScriptState::Failed("compile failure".into());
    assert_eq!(
        eligibility_from_facts(
            enabled_facts(),
            ScriptInputSurface::Grid,
            ScriptContext::Grid,
            &failed
        ),
        ScriptEligibility::Notify("compile failure".into()),
    );
    for blocked in 0..3 {
        let mut facts = enabled_facts();
        match blocked {
            0 => facts.blocked_input = true,
            1 => facts.remote = true,
            _ => facts.enabled = false,
        }
        assert_eq!(
            eligibility_from_facts(
                facts,
                ScriptInputSurface::Grid,
                ScriptContext::Grid,
                &failed
            ),
            ScriptEligibility::Silent,
        );
    }
    let mut facts = enabled_facts();
    facts.main_context = false;
    facts.blocked_input = true;
    facts.remote = true;
    facts.enabled = false;
    assert_eq!(
        eligibility_from_facts(
            facts,
            ScriptInputSurface::Grid,
            ScriptContext::Grid,
            &failed
        ),
        ScriptEligibility::Notify(MAIN_WINDOW_ONLY_NOTICE.into()),
    );
    let mut facts = enabled_facts();
    facts.viewer_detached_or_switching = true;
    assert_eq!(
        eligibility_from_facts(
            facts,
            ScriptInputSurface::Viewer,
            ScriptContext::FsImage,
            &ready
        ),
        ScriptEligibility::Notify(MAIN_WINDOW_ONLY_NOTICE.into()),
    );
    let mut facts = enabled_facts();
    facts.viewer_detached_or_switching = true;
    assert_eq!(
        eligibility_from_facts(facts, ScriptInputSurface::Grid, ScriptContext::Grid, &ready),
        ScriptEligibility::Available(ScriptContext::Grid),
    );
    assert_eq!(
        eligibility_from_facts(
            enabled_facts(),
            ScriptInputSurface::Grid,
            ScriptContext::Grid,
            &OperationScriptState::Disabled
        ),
        ScriptEligibility::Silent,
    );

    let temp = tempfile::tempdir().unwrap();
    let mut loader = crate::operation_script::OperationScriptLoader::default();
    loader.sync_enabled(true, temp.path(), &egui::Context::default());
    assert!(matches!(loader.state, OperationScriptState::Loading(_)));
    assert_eq!(
        eligibility_from_facts(
            enabled_facts(),
            ScriptInputSurface::Grid,
            ScriptContext::Grid,
            &loader.state
        ),
        ScriptEligibility::Notify("スクリプトを読み込み中です".into()),
    );
    let mut facts = enabled_facts();
    facts.remote = true;
    assert_eq!(
        eligibility_from_facts(
            facts,
            ScriptInputSurface::Grid,
            ScriptContext::Grid,
            &loader.state
        ),
        ScriptEligibility::Silent,
    );
}

#[test]
fn snapshot_reads_only_cached_ratings_tags_and_canonical_dimensions() {
    let mut app = grid_app("[]");
    let key = app.rating_path_key(0).unwrap();
    app.rating_db.as_ref().unwrap().set(&key, 5).unwrap();
    let rating_reads = crate::rating_db::read_operations_for_test();
    let tag_reads = crate::tags_db::read_operations_for_test();
    let absent = app.collect_operation_script_snapshot(ScriptInputSurface::Grid);
    assert_eq!(absent.context, ScriptContext::Grid);
    assert_eq!(
        absent.item.rating, None,
        "a DB value must not fill a cache miss"
    );
    assert_eq!(absent.item.pinned_tags, None);
    assert_eq!((absent.item.width, absent.item.height), (None, None));
    assert_eq!(absent.page.index, Some(0));
    assert_eq!(absent.page.count, Some(2));
    assert_eq!(absent.grid_selected_count, Some(1));
    let dynamic = absent.to_dynamic().cast::<rhai::Map>();
    let item = dynamic["item"].clone().cast::<rhai::Map>();
    assert!(item["rating"].is_unit());
    assert!(item["pinned_tags"].is_unit());

    app.rating_cache.insert(0, 3);
    app.settings.tags = vec![crate::settings::TagDef::new("Pinned".into())];
    let path = app.items[0].drag_source_path().unwrap().to_path_buf();
    app.tags_cache.insert(
        crate::tags_db::item_key_for_path(&path),
        vec!["Pinned".into()],
    );
    let generation = app.items_generation;
    app.page_dims_cache.record(
        generation,
        0,
        crate::page_dims::PagePixelDimensions::Thumbnail((100, 200)),
    );
    let thumbnail_only = app.collect_operation_script_snapshot(ScriptInputSurface::Grid);
    assert_eq!(
        (thumbnail_only.item.width, thumbnail_only.item.height),
        (None, None)
    );
    app.page_dims_cache.record(
        generation,
        0,
        crate::page_dims::PagePixelDimensions::Canonical(
            crate::page_dims::CanonicalPageDimensions::new(1200, 1800),
        ),
    );
    let cached = app.collect_operation_script_snapshot(ScriptInputSurface::Grid);
    assert_eq!(cached.item.rating, Some(3));
    assert_eq!(
        (cached.item.width, cached.item.height),
        (Some(1200), Some(1800))
    );
    let pinned = cached.item.pinned_tags.unwrap();
    assert!(pinned[0]);
    assert!(pinned[1..].iter().all(|value| !value));
    assert_eq!(crate::rating_db::read_operations_for_test(), rating_reads);
    assert_eq!(crate::tags_db::read_operations_for_test(), tag_reads);
}

#[test]
fn input_surface_controls_snapshot_context_and_target_includes_pdf_page_and_checks() {
    let mut app = grid_app("[]");
    let pdf_path = app.tmp.path().join("book.pdf");
    app.items = vec![
        GridItem::PdfPage {
            pdf_path: pdf_path.clone(),
            page_num: 0,
            content_type: None,
        },
        GridItem::PdfPage {
            pdf_path: pdf_path.clone(),
            page_num: 1,
            content_type: None,
        },
    ];
    app.fullscreen_idx = Some(1);
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    let grid = app.collect_operation_script_snapshot(ScriptInputSurface::Grid);
    assert_eq!(grid.context, ScriptContext::Grid);
    assert_eq!(grid.page.index, Some(0));
    assert_eq!(grid.window.fullscreen, Some(false));
    let first = app.operation_script_target(ScriptInputSurface::Grid);
    assert_eq!(
        first.pages,
        vec![ScriptItemIdentity::PdfPage(pdf_path.clone(), 0)]
    );
    app.selected = Some(1);
    let second = app.operation_script_target(ScriptInputSurface::Grid);
    assert_ne!(
        first, second,
        "PDF pages sharing a path have different identities"
    );
    app.checked.insert(0);
    let checked = app.operation_script_target(ScriptInputSurface::Grid);
    assert_ne!(
        checked, second,
        "checked targets are part of continuation identity"
    );
    assert_eq!(
        checked.checked,
        vec![(0, ScriptItemIdentity::PdfPage(pdf_path, 0))]
    );
}

#[test]
fn accepted_book_recipe_keeps_logical_location_while_a_new_folder_is_pending() {
    use crate::app::book_materialization::{BookScope, ProjectionRecipe};

    let mut app = grid_app("[]");
    let accepted = app.tmp.path().join("accepted.EPUB");
    app.projection_recipe = ProjectionRecipe::Book(BookScope::Document(accepted.clone()));
    app.current_folder = Some(app.tmp.path().join("not-yet-adopted.zip"));
    assert_eq!(
        app.operation_script_location(),
        ScriptLocation::Physical {
            path: Some(accepted.clone()),
            prefix: Vec::new(),
        }
    );
    let snapshot = app.collect_operation_script_snapshot(ScriptInputSurface::Grid);
    assert_eq!(snapshot.book.path.as_deref(), accepted.to_str());
    assert_eq!(snapshot.book.kind.as_deref(), Some("epub"));

    let archive = app.tmp.path().join("accepted.zip");
    let prefix = vec!["chapter".to_owned()];
    app.projection_recipe = ProjectionRecipe::Book(BookScope::Archive {
        logical_root: archive.clone(),
        prefix: prefix.clone(),
    });
    assert_eq!(
        app.operation_script_location(),
        ScriptLocation::Physical {
            path: Some(archive.clone()),
            prefix,
        }
    );
    let snapshot = app.collect_operation_script_snapshot(ScriptInputSurface::Grid);
    assert_eq!(snapshot.book.path.as_deref(), archive.to_str());
    assert_eq!(snapshot.book.kind.as_deref(), Some("zip"));
}

#[test]
fn run_executes_in_order_and_returns_notice_without_retained_work() {
    let mut app = grid_app(r#"#{run:["GridColumnCount3","GridColumnCount7"],notice:"done"}"#);
    let ctx = egui::Context::default();
    assert!(matches!(
        app.run_script_slot(&ctx, 1, ScriptInputSurface::Grid, false),
        ScriptRunResult::Grid {
            nav: None,
            rejected: false
        }
    ));
    assert_eq!(app.settings.grid_cols, 7);
    assert_eq!(toast(&app), "done");
    app.settings.grid_cols = 2;
    app.fs_feedback_toast = None;
    assert!(matches!(
        app.run_script_slot(&ctx, 1, ScriptInputSurface::Grid, false),
        ScriptRunResult::Grid {
            nav: None,
            rejected: false
        }
    ));
    assert_eq!(app.settings.grid_cols, 7);
    assert_eq!(toast(&app), "done");
}

#[test]
fn image_target_ignores_cache_availability_but_tracks_pairing_origin_reset() {
    let mut app = grid_app(r#"["FsBgCycle","FsBgCycle"]"#);
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    app.ai_upscale_enabled = false;
    let ctx = egui::Context::default();
    let result = app.run_script_slot(&ctx, 1, ScriptInputSurface::Viewer, false);
    assert!(matches!(
        result,
        ScriptRunResult::Fullscreen {
            rejected: false,
            ..
        }
    ));
    assert_eq!(app.fs_transparent_bg_mode, 2);

    app.operation_script.state = ready(r#"["FsReadingDirectionToggle","FsBgCycle"]"#);
    app.set_spread_shift_anchor_for_test(Some(1));
    let before = app.operation_script_target(ScriptInputSurface::Viewer);
    app.run_script_slot(&ctx, 1, ScriptInputSurface::Viewer, false);
    assert_ne!(
        app.operation_script_target(ScriptInputSurface::Viewer),
        before
    );
    assert_eq!(
        app.fs_transparent_bg_mode, 2,
        "remaining background action must not run after pairing changes"
    );
    assert!(toast(&app).contains("対象が変わった"));
}

#[test]
fn run_validates_the_whole_sequence_before_first_side_effect() {
    let mut app = grid_app(r#"["GridColumnCount7","FsPageNext"]"#);
    let result = app.run_script_slot(
        &egui::Context::default(),
        1,
        ScriptInputSurface::Grid,
        false,
    );
    assert!(matches!(
        result,
        ScriptRunResult::Grid {
            nav: None,
            rejected: true
        }
    ));
    assert_eq!(app.settings.grid_cols, 2);
    assert!(toast(&app).contains("操作の指定"));
}

#[test]
fn rebase_returns_to_caller_and_keeps_interruption_notice_over_script_notice() {
    let mut app =
        grid_app(r#"#{run:["FsSpreadLtr","FsBgCycle"],notice:"must not replace warning"}"#);
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.spread_mode = crate::settings::SpreadMode::Single;
    let (result, feedback) = app.run_script_slot_with_feedback(
        &egui::Context::default(),
        1,
        ScriptInputSurface::Viewer,
        false,
    );
    assert!(matches!(
        result,
        ScriptRunResult::Fullscreen {
            action: FsKeyAction {
                page_transition: FsKeyPageTransition::Rebased { .. },
                ..
            },
            rejected: false,
            ..
        }
    ));
    assert_eq!(app.fs_transparent_bg_mode, 0);
    assert!(toast(&app).contains("残りの操作を中止"));
    assert!(matches!(feedback, Some(ScriptFeedback::Warning(text)) if text == toast(&app)));
}

#[test]
fn delegated_book_permission_returns_and_discards_remaining_script_actions() {
    let mut app =
        grid_app(r#"#{run:["FsSpreadLtr","FsBgCycle"],notice:"must not replace warning"}"#);
    let pdf = app.tmp.path().join("permission-book.pdf");
    app.items = (0..2)
        .map(|page_num| GridItem::PdfPage {
            pdf_path: pdf.clone(),
            page_num,
            content_type: None,
        })
        .collect();
    app.fullscreen_idx = Some(1);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.spread_mode = crate::settings::SpreadMode::Single;
    let (service, owner) = crate::book_composition::BookCompositionService::spawn_at(
        app.tmp.path().join("script-permission-data"),
        None,
    )
    .unwrap();
    let scope = crate::book_scope::BookScopeKey::document(&pdf).unwrap();
    service
        .prepare(scope.clone(), crate::book_composition::PageDomain::Document)
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    let candidate = app.presentation_candidate(0).unwrap();
    assert!(
        crate::book_composition::permission_without_composition(&service.snapshot(), &candidate,)
            .is_some()
    );
    let record = crate::book_composition::BookComposition::new(
        crate::book_composition::ReadingDirection::Ltr,
        (0..2)
            .map(crate::book_composition::fixtures::document_page)
            .collect(),
    );
    service
        .save(
            scope.clone(),
            crate::book_composition::PageDomain::Document,
            record,
        )
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    // Ordinary lists can prove an absent composition synchronously. A stored
    // record requires the real candidate check instead of that snapshot shortcut.
    assert!(service.snapshot().rows.contains_key(&scope.storage_key()));
    assert!(
        crate::book_composition::permission_without_composition(&service.snapshot(), &candidate,)
            .is_none()
    );
    app.install_book_composition_runtime(service);
    let (result, feedback) = app.run_script_slot_with_feedback(
        &egui::Context::default(),
        1,
        ScriptInputSurface::Viewer,
        false,
    );
    assert!(matches!(feedback, Some(ScriptFeedback::Warning(text)) if text == toast(&app)));
    assert!(
        matches!(
            result,
            ScriptRunResult::Fullscreen {
                rejected: false,
                delegated: true,
                ..
            }
        ),
        "pending={}, idx={:?}, spread={:?}, feedback={:?}",
        app.operation_script_fullscreen_transition_pending(),
        app.fullscreen_idx,
        app.spread_mode,
        app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
    );
    assert!(app.operation_script_fullscreen_transition_pending());
    assert!(!app.pending_book_target_demand_for_test().is_empty());
    assert_eq!(
        app.fullscreen_idx,
        Some(1),
        "permission owns the pending open"
    );
    assert_eq!(app.fs_transparent_bg_mode, 0);
    assert!(toast(&app).contains("残りの操作を中止"));
    assert!(!toast(&app).contains("must not replace warning"));
    owner.shutdown_and_join();
}

#[test]
fn rejected_operation_keeps_completed_actions_and_discards_the_remainder() {
    let mut app = grid_app(
        r#"#{run:["GridColumnCount4","GridCopyImage","GridColumnCount9"],notice:"must not be shown"}"#,
    );
    let folder = app.current_folder.clone().unwrap();
    app.items[0] = GridItem::Folder(folder);
    let result = app.run_script_slot(
        &egui::Context::default(),
        1,
        ScriptInputSurface::Grid,
        false,
    );
    assert!(matches!(
        result,
        ScriptRunResult::Grid {
            nav: None,
            rejected: true
        }
    ));
    assert_eq!(app.settings.grid_cols, 4);
    assert!(toast(&app).contains("GridCopyImage"));
    assert!(!toast(&app).contains("must not be shown"));
}

#[test]
fn rating_that_changes_checked_targets_stops_before_following_operation() {
    let mut app = grid_app(r#"["RatingItem3","GridColumnCount9"]"#);
    app.checked.extend([0, 1]);
    let before = app.operation_script_target(ScriptInputSurface::Grid);
    let result = app.run_script_slot(
        &egui::Context::default(),
        1,
        ScriptInputSurface::Grid,
        false,
    );
    assert!(matches!(result, ScriptRunResult::Grid { nav: None, .. }));
    assert_eq!(app.rating_cache.get(&0), Some(&3));
    assert_eq!(app.rating_cache.get(&1), Some(&3));
    assert!(app.checked.is_empty());
    assert_ne!(
        app.operation_script_target(ScriptInputSurface::Grid),
        before
    );
    assert_eq!(app.settings.grid_cols, 2);
    assert!(toast(&app).contains("対象が変わった"));
}

#[test]
fn run_returns_navigation_for_the_existing_caller_dispatcher() {
    let mut app = grid_app(r#"["GridColumnCount4","GridOpenFavorite1"]"#);
    let destination = app.tmp.path().join("destination");
    app.settings.favorites = vec![crate::settings::FavoriteEntry::new(
        "target".into(),
        destination.clone(),
    )];
    let folder_before = app.current_folder.clone();
    let result = app.run_script_slot(
        &egui::Context::default(),
        1,
        ScriptInputSurface::Grid,
        false,
    );
    assert!(matches!(result, ScriptRunResult::Grid {
        nav: Some(AddressBarNav::WorkerFavorite(path)), rejected: false,
    } if path == destination));
    assert_eq!(app.settings.grid_cols, 4);
    assert_eq!(
        app.current_folder, folder_before,
        "the caller still owns navigation adoption"
    );
}

#[test]
fn unavailable_input_rejects_before_evaluation_without_a_notice() {
    let mut app = grid_app("missing_function_must_not_run();");
    let ctx = egui::Context::default();
    for blocked in 0..3 {
        app.address_has_focus = blocked == 0;
        app.erase_mode = blocked == 1;
        app.show_open_folder_dialog = blocked == 2;
        assert!(matches!(
            app.run_script_slot(&ctx, 1, ScriptInputSurface::Grid, false),
            ScriptRunResult::Grid {
                nav: None,
                rejected: true
            }
        ));
        assert!(
            app.fs_feedback_toast.is_none(),
            "blocked input must not evaluate Rhai"
        );
    }
    app.address_has_focus = false;
    app.erase_mode = false;
    app.show_open_folder_dialog = false;
    let metadata = std::sync::Arc::new(
        crate::page_overview::PageOverviewMetadata::new([
            crate::page_overview::PageOverviewEntry {
                items_idx: 0,
                page: std::sync::Arc::new(crate::page_overview::PageOverviewPage {
                    identity: crate::book_resume_db::PageIdentity::for_item(&app.items[0], None)
                        .unwrap(),
                    label: "first".into(),
                    white_companion: None,
                }),
            },
        ])
        .unwrap(),
    );
    let projection = std::sync::Arc::new(
        crate::page_overview::PageOverviewProjection::new(metadata, std::sync::Arc::new(vec![0]))
            .unwrap(),
    );
    app.page_overview = crate::page_overview::PageOverviewState::open(
        projection,
        crate::page_overview::PageOverviewStamp {
            context_serial: 1,
            items_generation: app.items_generation,
            reader_order_revision: 1,
            incarnation: 1,
        },
        0,
        4,
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0)),
        Default::default(),
    )
    .unwrap();
    assert!(matches!(
        app.run_script_slot(&ctx, 1, ScriptInputSurface::Grid, false),
        ScriptRunResult::Grid {
            nav: None,
            rejected: true
        }
    ));
    assert!(
        app.fs_feedback_toast.is_none(),
        "page overview must reject before evaluating Rhai"
    );
}

#[test]
fn missing_slot_and_evaluation_failure_notify_without_applying_an_operation() {
    let mut app = grid_app(r#"["GridColumnCount9"]"#);
    let ctx = egui::Context::default();
    assert!(
        app.run_script_slot(&ctx, 2, ScriptInputSurface::Grid, false)
            .rejected()
    );
    assert!(toast(&app).contains("未登録"));
    assert_eq!(app.settings.grid_cols, 2);
    app.operation_script.state = ready("unknown_function();");
    app.fs_feedback_toast = None;
    assert!(
        app.run_script_slot(&ctx, 1, ScriptInputSurface::Grid, false)
            .rejected()
    );
    assert!(toast(&app).contains("実行に失敗"));
    assert_eq!(app.settings.grid_cols, 2);
}

#[cfg(windows)]
#[test]
fn linked_detached_viewer_and_transition_reject_viewer_but_keep_main_grid_available() {
    let mut app = grid_app(r#"if s.context == "grid" { ["GridColumnCount3"] } else { [] }"#);
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    let ctx = egui::Context::default();
    assert_eq!(app.projected_viewer_context_id(), app.viewer_context_main());
    assert_eq!(
        app.operation_script_eligibility(&ctx, ScriptInputSurface::Viewer),
        ScriptEligibility::Notify(MAIN_WINDOW_ONLY_NOTICE.into())
    );
    assert_eq!(
        app.operation_script_eligibility(&ctx, ScriptInputSurface::Grid),
        ScriptEligibility::Available(ScriptContext::Grid)
    );
    assert!(matches!(
        app.run_script_slot(&ctx, 1, ScriptInputSurface::Grid, false),
        ScriptRunResult::Grid {
            nav: None,
            rejected: false
        }
    ));
    assert_eq!(app.settings.grid_cols, 3);

    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.video_presentation_transition
        .reset_stable(ViewerPresentation::Fullscreen);
    app.video_presentation_transition.request_transition(
        ViewerPresentation::DetachedWindow,
        false,
        false,
        0,
        None,
        None,
    );
    assert!(app.video_presentation_transition.is_transitioning());
    assert_eq!(
        app.operation_script_eligibility(&ctx, ScriptInputSurface::Viewer),
        ScriptEligibility::Notify(MAIN_WINDOW_ONLY_NOTICE.into())
    );
    assert_eq!(
        app.operation_script_eligibility(&ctx, ScriptInputSurface::Grid),
        ScriptEligibility::Available(ScriptContext::Grid)
    );
    app.video_presentation_transition
        .reset_stable(ViewerPresentation::Fullscreen);
}

#[cfg(windows)]
#[test]
fn mounted_other_context_is_rejected_without_mutating_main_or_parked_targets() {
    let mut app = grid_app(r#"["GridColumnCount9"]"#);
    app.fullscreen_idx = Some(0);
    let main_target = app.operation_script_target(ScriptInputSurface::Viewer);
    let other = app.build_window_context_for_test(363_201, |mounted| {
        crate::app::seed_snapshot_context_for_test(
            mounted,
            r"C:\script-other",
            &[r"C:\script-other\other.jpg"],
        );
        mounted.fullscreen_idx = Some(0);
        mounted.viewer_presentation = ViewerPresentation::DetachedWindow;
    });
    let ctx = egui::Context::default();
    assert_eq!(
        app.operation_script_eligibility(&ctx, ScriptInputSurface::Viewer),
        ScriptEligibility::Available(ScriptContext::FsImage),
        "a parked sibling must not block main"
    );
    app.with_viewer_context(other, |mounted| {
        let before = mounted.operation_script_target(ScriptInputSurface::Viewer);
        for surface in [ScriptInputSurface::Grid, ScriptInputSurface::Viewer] {
            assert_eq!(
                mounted.operation_script_eligibility(&ctx, surface),
                ScriptEligibility::Notify(MAIN_WINDOW_ONLY_NOTICE.into())
            );
            assert!(mounted.run_script_slot(&ctx, 1, surface, false).rejected());
            assert!(toast(mounted).contains(MAIN_WINDOW_ONLY_NOTICE));
        }
        assert_eq!(
            mounted.operation_script_target(ScriptInputSurface::Viewer),
            before
        );
    })
    .expect("independent context mounts");
    assert_eq!(
        app.operation_script_target(ScriptInputSurface::Viewer),
        main_target
    );
    assert_eq!(app.settings.grid_cols, 2);
}
