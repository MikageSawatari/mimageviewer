//! No application/viewport is launched. The identity collector below is a fixture
//! adapter, not the future adopted-location publication boundary.

use super::super::events::{EventAdmission, EventIdentity};
use super::*;
use crate::operation_script::{ALLOWED_OPERATIONS, ScriptEvent};
use std::time::{Duration, Instant};

fn event_script(body: &str) -> OperationScriptState {
    OperationScriptState::Ready(Box::new(
        CompiledOperationScript::compile(&format!(
            "fn slots() {{ [] }} fn on_event(s) {{ {body} }}"
        ))
        .unwrap(),
    ))
}

fn identity(app: &App, surface: ScriptInputSurface) -> EventIdentity {
    let location = app.operation_script_location();
    EventIdentity {
        location: location.clone(),
        context: app.script_context(surface),
        page: (surface == ScriptInputSurface::Viewer).then(|| app.operation_script_page_identity()),
        selected: app
            .selected
            .and_then(|idx| app.items.get(idx))
            .map(item_identity),
    }
}

#[test]
fn event_baseline_collects_all_changes_and_records_even_an_unchanged_frame() {
    let mut app = grid_app("[]");
    let mut events = OperationScriptEvents::default();
    assert!(events.begin_frame(0));
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::Open, ScriptEvent::Select]
    );
    assert!(!events.begin_frame(0));
    assert!(events.begin_frame(1));
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Grid))
            .is_empty()
    );
    // An unchanged first pass still owns the frame when the context changes.
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    assert!(!events.begin_frame(1));
    assert!(events.begin_frame(2));
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::View, ScriptEvent::Page]
    );
    app.current_folder = Some(app.tmp.path().join("next-book"));
    app.items[0] = GridItem::Image(app.current_folder.as_ref().unwrap().join("same.jpg"));
    assert!(events.begin_frame(3));
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Open, ScriptEvent::Page]
    );
}

#[test]
fn event_view_changes_grid_image_video_and_audio_shares_video_context() {
    let mut app = grid_app("[]");
    let mut events = OperationScriptEvents::default();
    events.observe(identity(&app, ScriptInputSurface::Grid));
    app.fullscreen_idx = Some(0);
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::View, ScriptEvent::Page]
    );
    app.items[0] = GridItem::Video(app.current_folder.as_ref().unwrap().join("clip.mp4"));
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::View, ScriptEvent::Page]
    );
    app.items[0] = GridItem::Audio(app.current_folder.as_ref().unwrap().join("song.mp3"));
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Page]
    );
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::View]
    );
}

#[test]
fn event_book_recipe_retains_logical_open_until_adoption_and_same_place_reload_is_quiet() {
    use crate::app::book_materialization::{BookScope, ProjectionRecipe};
    let mut app = grid_app("[]");
    let old = app.tmp.path().join("accepted.zip");
    let new = app.tmp.path().join("enumerating.zip");
    app.projection_recipe = ProjectionRecipe::Book(BookScope::Archive {
        logical_root: old,
        prefix: vec!["chapter".into()],
    });
    let mut events = OperationScriptEvents::default();
    events.observe(identity(&app, ScriptInputSurface::Grid));
    app.current_folder = Some(new.clone());
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Grid))
            .is_empty()
    );
    app.projection_recipe = ProjectionRecipe::Book(BookScope::Archive {
        logical_root: new,
        prefix: vec!["chapter".into()],
    });
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::Open, ScriptEvent::Select]
    );
    app.items_generation += 1;
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Grid))
            .is_empty()
    );
}

#[test]
fn event_baseline_uses_typed_pdf_zip_halves_and_location_not_index_or_generation() {
    let mut app = grid_app("[]");
    let mut events = OperationScriptEvents::default();
    let pdf = app.tmp.path().join("book.pdf");
    app.items = vec![
        GridItem::PdfPage {
            pdf_path: pdf.clone(),
            page_num: 0,
            content_type: None,
        },
        GridItem::PdfPage {
            pdf_path: pdf,
            page_num: 1,
            content_type: None,
        },
    ];
    app.fullscreen_idx = Some(0);
    events.observe(identity(&app, ScriptInputSurface::Viewer));
    app.fullscreen_idx = Some(1);
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Page]
    );
    app.fullscreen_page_slice = crate::page_split::PageSlice::Left;
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Page]
    );
    app.fullscreen_page_slice = crate::page_split::PageSlice::Right;
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Viewer)),
        [ScriptEvent::Page]
    );
    app.items.swap(0, 1);
    app.visible_indices = vec![1, 0];
    app.fullscreen_idx = Some(0);
    app.items_generation += 1;
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Viewer))
            .is_empty()
    );
    let zip = app.tmp.path().join("book.zip");
    for entry in ["a/page.jpg", "b/page.jpg"] {
        app.items[0] = GridItem::ZipImage {
            zip_path: zip.clone(),
            entry_name: entry.into(),
        };
        assert_eq!(
            events.observe(identity(&app, ScriptInputSurface::Viewer)),
            [ScriptEvent::Page]
        );
    }
}

#[test]
fn event_select_tracks_cursor_location_and_ignores_checks_and_same_place_reload() {
    let mut app = grid_app("[]");
    let mut events = OperationScriptEvents::default();
    events.observe(identity(&app, ScriptInputSurface::Grid));
    app.checked.insert(1);
    app.items_generation += 1;
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Grid))
            .is_empty()
    );
    app.selected = Some(1);
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::Select]
    );
    app.selected = None;
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::Select]
    );
    app.selected = Some(1);
    events.observe(identity(&app, ScriptInputSurface::Grid));
    let first = app.current_folder.clone();
    app.current_folder = Some(app.tmp.path().join("different-location"));
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::Open, ScriptEvent::Select]
    );
    app.current_folder = first;
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::Open, ScriptEvent::Select]
    );
}

#[test]
fn event_empty_grid_select_keeps_location_but_initial_empty_has_only_open() {
    let mut app = grid_app("[]");
    let mut events = OperationScriptEvents::default();
    app.selected = None;
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::Open]
    );
    app.current_folder = Some(app.tmp.path().join("another-empty-place"));
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::Open, ScriptEvent::Select]
    );
}

#[test]
fn event_logical_location_ratings_and_collections_are_distinct_and_reload_is_not_open() {
    let app = grid_app("[]");
    let mut current = identity(&app, ScriptInputSurface::Grid);
    let mut events = OperationScriptEvents::default();
    current.selected = None;
    current.location = ScriptLocation::Rating(1);
    assert_eq!(events.observe(current.clone()), [ScriptEvent::Open]);
    assert!(events.observe(current.clone()).is_empty());
    current.location = ScriptLocation::Rating(2);
    assert_eq!(
        events.observe(current.clone()),
        [ScriptEvent::Open, ScriptEvent::Select]
    );
    // The identity type preserves a collection's visit-independent logical ID.
    for collection in [
        uuid::Uuid::from_u128(363_401),
        uuid::Uuid::from_u128(363_402),
    ] {
        current.location = ScriptLocation::Collection {
            id: super::super::super::top_level_grid_view::CollectionGridIdentity {
                collection_id: crate::collection_store::CollectionId::from_uuid(collection),
            },
            child: None,
            prefix: Vec::new(),
        };
        assert_eq!(
            events.observe(current.clone()),
            [ScriptEvent::Open, ScriptEvent::Select]
        );
        assert!(events.observe(current.clone()).is_empty());
    }
}

#[test]
fn event_unmarked_direction_and_animation_actions_really_change_page_identity() {
    let mut app = grid_app("[]");
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.spread_mode = crate::settings::SpreadMode::Single;
    let ctx = egui::Context::default();
    for action in [
        KeyAction::FsReadingDirectionToggle,
        KeyAction::FsPageTurnAnimationToggle,
    ] {
        let before = identity(&app, ScriptInputSurface::Viewer);
        let (_, rejected, _) = app.execute_fullscreen_script_action(&ctx, action);
        assert!(!rejected, "{action:?}");
        assert_ne!(
            identity(&app, ScriptInputSurface::Viewer),
            before,
            "{action:?}"
        );
    }
}

#[test]
fn event_unmarked_fit_cycle_reenables_split_after_free_rotation() {
    let mut app = grid_app("[]");
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.spread_mode = crate::settings::SpreadMode::SplitLtr;
    app.fs_free_rotation = 15.0;
    assert!(app.split_direction_now().is_none());
    let before = identity(&app, ScriptInputSurface::Viewer);
    let (_, rejected, _) =
        app.execute_fullscreen_script_action(&egui::Context::default(), KeyAction::FsFitModeCycle);
    assert!(!rejected);
    assert!(app.split_direction_now().is_some());
    assert_ne!(identity(&app, ScriptInputSurface::Viewer), before);
}

#[test]
fn event_unmarked_background_and_analysis_pixel_grid_can_resolve_pairing_origin() {
    use crate::app::spread_shift_owner::ViewerShift;
    use crate::spread_shift::{
        ShiftPersistence, ShiftResolution, SpreadShiftPageRef, SpreadShiftState,
    };
    let mut app = grid_app("[]");
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    app.ai_upscale_enabled = false;
    app.page_identity_index = Arc::new(
        crate::app::file_type_resume::PreparedPageIdentityIndex::prepare(
            &app.items,
            None,
            app.settings.file_type_visibility.clone(),
        ),
    );
    let ctx = egui::Context::default();
    for action in [KeyAction::FsBgCycle, KeyAction::FsPixelGrid] {
        app.spread_shift = Default::default();
        app.spread_shift.scope = Some(
            crate::book_scope::BookScopeKey::folder(app.current_folder.as_ref().unwrap()).unwrap(),
        );
        // A previously filtered-out anchor becomes visible before the next owner
        // resolves it. Ordinary adoption invalidates the nav cache, not this ref.
        app.spread_shift.state = ViewerShift::Book(SpreadShiftState::Adjusted {
            anchor_ref: SpreadShiftPageRef::RelativePath("b.jpg".into()),
            resolution: ShiftResolution::NotInView,
            persistence: ShiftPersistence::SessionOnly,
        });
        app.viewer_navigation_caches.invalidate();
        app.analysis_mode = action == KeyAction::FsPixelGrid;
        assert_eq!(app.spread_shift.anchor_idx(), None);
        let before = identity(&app, ScriptInputSurface::Viewer);
        let (_, rejected, _) = app.execute_fullscreen_script_action(&ctx, action);
        assert!(!rejected, "{action:?}");
        assert_eq!(app.spread_shift.anchor_idx(), Some(1), "{action:?}");
        assert_ne!(
            identity(&app, ScriptInputSurface::Viewer),
            before,
            "{action:?}"
        );
    }
}

#[test]
fn event_script_resnapshot_suppresses_sync_changes_but_input_changes_fire_next_frame() {
    let mut app = grid_app(r#"["GridMoveLast"]"#);
    let ctx = egui::Context::default();
    let mut events = OperationScriptEvents::default();
    events.observe(identity(&app, ScriptInputSurface::Grid));
    assert!(
        !app.run_script_slot(&ctx, 1, ScriptInputSurface::Grid, false)
            .rejected()
    );
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::Select]
    );
    app.selected = Some(0);
    events.finish_event(identity(&app, ScriptInputSurface::Grid));
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Grid))
            .is_empty()
    );
    // Later completion is independent of which invocation originally caused it.
    app.selected = Some(1);
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::Select]
    );
}

#[test]
fn event_limit_is_rolling_ten_attempts_and_notification_is_lifetime_once() {
    let mut events = OperationScriptEvents::default();
    let now = Instant::now();
    for index in 0..10 {
        assert_eq!(
            events.admit_execution(now + Duration::from_millis(index * 50)),
            EventAdmission::Execute
        );
    }
    assert_eq!(
        events.admit_execution(now + Duration::from_millis(999)),
        EventAdmission::Drop { notify: true }
    );
    assert_eq!(
        events.admit_execution(now + Duration::from_millis(999)),
        EventAdmission::Drop { notify: false }
    );
    assert_eq!(
        events.admit_execution(now + Duration::from_secs(1)),
        EventAdmission::Execute
    );
    assert_eq!(
        events.admit_execution(now + Duration::from_millis(1001)),
        EventAdmission::Drop { notify: false }
    );
    for index in 0..10 {
        assert_eq!(
            events.admit_execution(now + Duration::from_secs(3) + Duration::from_millis(index)),
            EventAdmission::Execute
        );
    }
    assert_eq!(
        events.admit_execution(now + Duration::from_millis(3011)),
        EventAdmission::Drop { notify: false }
    );
}

#[test]
fn event_rate_drop_advances_baseline_and_never_queues_a_late_replay() {
    let mut app = grid_app("[]");
    let mut events = OperationScriptEvents::default();
    events.observe(identity(&app, ScriptInputSurface::Grid));
    let now = Instant::now();
    for _ in 0..10 {
        assert_eq!(events.admit_execution(now), EventAdmission::Execute);
    }
    app.selected = Some(1);
    assert_eq!(
        events.observe(identity(&app, ScriptInputSurface::Grid)),
        [ScriptEvent::Select]
    );
    assert_eq!(
        events.admit_execution(now),
        EventAdmission::Drop { notify: true }
    );
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Grid))
            .is_empty()
    );
    assert_eq!(
        events.admit_execution(now + Duration::from_secs(1)),
        EventAdmission::Execute
    );
    assert!(
        events
            .observe(identity(&app, ScriptInputSurface::Grid))
            .is_empty()
    );
}

#[test]
fn event_detection_off_absent_loading_and_failed_do_not_claim_the_frame() {
    let mut app = grid_app("[]");
    let ctx = egui::Context::default();
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    app.operation_script.state = event_script("[]");
    app.settings.operation_script_enabled = false;
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    app.settings.operation_script_enabled = true;
    let data_dir = app.tmp.path().to_path_buf();
    app.operation_script.reload(&data_dir, &ctx);
    assert!(matches!(
        app.operation_script.state,
        OperationScriptState::Loading(_)
    ));
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    app.operation_script.state = OperationScriptState::Failed("test".into());
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    app.operation_script.state = event_script("[]");
    assert!(app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Viewer));
    assert!(app.fs_feedback_toast.is_none());
}

#[cfg(windows)]
#[test]
fn event_detection_mounted_context_and_linked_detached_skip_without_touching_main_owner() {
    let mut app = grid_app("[]");
    app.operation_script.state = event_script("[]");
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    let baseline = identity(&app, ScriptInputSurface::Viewer);
    app.operation_script_events.finish_event(baseline.clone());
    let other = app.build_window_context_for_test(363_401, |mounted| {
        crate::app::seed_snapshot_context_for_test(
            mounted,
            r"C:\event-other",
            &[r"C:\event-other\other.jpg"],
        );
        mounted.fullscreen_idx = Some(0);
        mounted.viewer_presentation = ViewerPresentation::DetachedWindow;
    });
    let ctx = egui::Context::default();
    app.with_viewer_context(other, |mounted| {
        for surface in [ScriptInputSurface::Grid, ScriptInputSurface::Viewer] {
            assert!(!mounted.begin_operation_script_event_detection(&ctx, surface));
        }
    })
    .unwrap();
    assert!(
        app.operation_script_events.observe(baseline).is_empty(),
        "the common owner was not swapped or rewritten"
    );
    // A parked sibling does not block the main viewer, even in this same frame.
    assert!(app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Viewer));
    assert!(app.fs_feedback_toast.is_none());
}

#[cfg(windows)]
#[test]
fn event_detection_linked_f12_and_transition_skip_viewer_but_main_list_continues() {
    let mut app = grid_app("[]");
    app.operation_script.state = event_script("[]");
    app.fullscreen_idx = Some(0);
    let ctx = egui::Context::default();
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Viewer));
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
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Viewer));
    assert!(app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    assert_eq!(
        identity(&app, ScriptInputSurface::Grid).context,
        ScriptContext::Grid
    );
    assert!(app.fs_feedback_toast.is_none());
}

#[test]
fn event_detection_edit_and_page_overview_coalesce_after_exit() {
    let mut app = grid_app("[]");
    app.operation_script.state = event_script("[]");
    let baseline = identity(&app, ScriptInputSurface::Grid);
    app.operation_script_events.finish_event(baseline);
    let ctx = egui::Context::default();
    app.erase_mode = true;
    app.selected = Some(1);
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    app.erase_mode = false;
    let metadata = Arc::new(
        crate::page_overview::PageOverviewMetadata::new([
            crate::page_overview::PageOverviewEntry {
                items_idx: 0,
                page: Arc::new(crate::page_overview::PageOverviewPage {
                    identity: crate::book_resume_db::PageIdentity::for_item(&app.items[0], None)
                        .unwrap(),
                    label: "first".into(),
                    white_companion: None,
                }),
            },
        ])
        .unwrap(),
    );
    let projection = Arc::new(
        crate::page_overview::PageOverviewProjection::new(metadata, Arc::new(vec![0])).unwrap(),
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
    assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    app.page_overview = Default::default();
    assert!(app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Grid));
    let current = identity(&app, ScriptInputSurface::Grid);
    assert_eq!(
        app.operation_script_events.observe(current),
        [ScriptEvent::Select]
    );
    assert!(app.fs_feedback_toast.is_none());
}

#[cfg(windows)]
#[test]
fn event_detection_native_overlay_uses_existing_snapshot_and_does_not_claim_blocked_frame() {
    let mut app = video_app("[]", false);
    app.operation_script.state = event_script("[]");
    let ctx = egui::Context::default();
    for modal in [false, true] {
        video_player(&app, 0).set_native_overlay_input_routing_for_test(
            crate::video::native_presenter::NativeOverlayInputRouting {
                text_input_active: !modal,
                modal_dialog_active: modal,
                ..Default::default()
            },
        );
        assert!(!app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Viewer));
    }
    video_player(&app, 0).set_native_overlay_input_routing_for_test(Default::default());
    assert!(app.begin_operation_script_event_detection(&ctx, ScriptInputSurface::Viewer));
    assert!(app.fs_feedback_toast.is_none());
}

#[test]
fn event_safe_image_actions_preserve_all_four_identity_components() {
    let mut app = grid_app("[]");
    let ctx = egui::Context::default();
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.spread_mode = crate::settings::SpreadMode::Single;
    app.ai_upscale_enabled = false;
    for mode in [
        crate::settings::SpreadMode::Single,
        crate::settings::SpreadMode::Ltr,
        crate::settings::SpreadMode::SplitLtr,
    ] {
        for free_rotation in [0.0, 15.0] {
            app.spread_mode = mode;
            app.fs_free_rotation = free_rotation;
            for operation in ALLOWED_OPERATIONS
                .iter()
                .filter(|op| op.event_allowed && op.contexts.contains(ScriptContext::FsImage))
            {
                let before = identity(&app, ScriptInputSurface::Viewer);
                let (result, rejected, delegated) =
                    app.execute_fullscreen_script_action(&ctx, operation.action);
                assert!(!rejected, "{:?}", operation.action);
                assert!(!delegated, "{:?}", operation.action);
                assert!(
                    !ScriptRunResult::Fullscreen {
                        action: result,
                        rejected,
                        delegated
                    }
                    .needs_postprocessing(),
                    "{:?}",
                    operation.action
                );
                assert_eq!(
                    identity(&app, ScriptInputSurface::Viewer),
                    before,
                    "{:?}",
                    operation.action
                );
            }
        }
    }
}

#[test]
fn event_safe_grid_actions_preserve_all_four_identity_components_even_from_details() {
    let mut app = grid_app("[]");
    let ctx = egui::Context::default();
    for operation in ALLOWED_OPERATIONS
        .iter()
        .filter(|op| op.event_allowed && op.contexts.contains(ScriptContext::Grid))
    {
        app.set_grid_view_mode(crate::settings::GridViewMode::Details);
        let before = identity(&app, ScriptInputSurface::Grid);
        let result = app.execute_grid_key_action(&ctx, operation.action, "script_event_audit");
        assert!(!result.rejected(), "{:?}", operation.action);
        assert!(!result.needs_postprocessing(), "{:?}", operation.action);
        assert_eq!(
            identity(&app, ScriptInputSurface::Grid),
            before,
            "{:?}",
            operation.action
        );
    }
}

#[cfg(windows)]
#[test]
fn event_safe_video_actions_preserve_all_four_identity_components_for_native_and_music() {
    use crate::app::{VideoKeyActionResult, VideoKeyActionSurface};
    for surface in [
        VideoKeyActionSurface::Native,
        VideoKeyActionSurface::EguiVideo,
        VideoKeyActionSurface::EguiMusic,
    ] {
        let music = surface == VideoKeyActionSurface::EguiMusic;
        // One live fixture per iteration, never acquire setup_app's mutex twice.
        let mut app = video_app("[]", music);
        let ctx = egui::Context::default();
        for operation in ALLOWED_OPERATIONS
            .iter()
            .filter(|op| op.event_allowed && op.contexts.contains(ScriptContext::FsVideo))
        {
            if music && operation.action == KeyAction::VideoSeekStripToggle {
                continue; // Released music keys do not expose the video strip.
            }
            let before = identity(&app, ScriptInputSurface::Viewer);
            let result = app.execute_video_key_action(&ctx, 0, operation.action, surface);
            assert!(
                matches!(result, VideoKeyActionResult::Accepted(_)),
                "{:?}, music={music}",
                operation.action
            );
            assert_eq!(
                identity(&app, ScriptInputSurface::Viewer),
                before,
                "{:?}, music={music}",
                operation.action
            );
        }
    }
}
