//! Restore-start handlers must leave root tree witnesses intact until recognition.

use super::*;
use crate::spread_shift::{ShiftOperation, SpreadShiftPageRef, SpreadShiftService};
use sidecar_restore::{FirstDisplayAction, SidecarLoadContinuation};

#[derive(Clone, Copy, Debug)]
enum RestoreStart {
    Hydration,
    PageIndex,
    RetainedShift,
}

impl RestoreStart {
    fn begin(self, app: &mut App, ctx: &egui::Context) {
        match self {
            Self::Hydration => {
                let now = std::time::Instant::now();
                let (tx, _rx) = mpsc::channel();
                let continuation = SidecarLoadContinuation {
                    viewer_adoption: ViewerOpenAdoption::ContextContinuation,
                    navigation_after_hydration: None,
                    audio_art_prune: None,
                    first_display_action: FirstDisplayAction::None,
                    restore_intent: StartupListIntent::PreservePresentation,
                    auto_aspect_seed: None,
                    source_path: app.current_folder.clone().unwrap(),
                    source_is_directory: true,
                    prepared_subfolder: None,
                    prepared_page_edits: None,
                    prepared_aggregate: None,
                    catalog_existing_keys: HashSet::new(),
                    video_items: Vec::new(),
                    sli_seq: 0,
                    sli_t0: now,
                    items_len: app.items.len(),
                    detached_physical: false,
                    tx,
                    cancel: Arc::new(AtomicBool::new(false)),
                    restore_started_at: now,
                };
                assert!(
                    app.begin_sidecar_restore_with_search_index(continuation, false, None)
                        .is_ok()
                );
                assert!(app.sidecar_restore.is_some());
            }
            Self::PageIndex => {
                app.prepare_current_listing_page_identity_index(Default::default(), None);
                assert!(app.sidecar_restore.is_some());
            }
            Self::RetainedShift => {
                app.begin_retained_shift_readoption(ctx);
                assert!(app.spread_shift.retained_readoption_pending());
            }
        }
        assert!(app.sidecar_restore_blocks_projected_context());
    }
}

fn starting_pass_events(pos: egui::Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        primary_event(pos, true),
        egui::Event::Text("must not reach other consumers".into()),
        tree_key_event(egui::Key::K),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 20.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        },
        primary_event(pos, false),
    ]
}

fn assert_starting_pass_input_present(ctx: &egui::Context) {
    ctx.input(|input| {
        assert!(
            input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::Text(_)))
        );
        assert!(
            input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::PointerButton { pressed: true, .. }))
        );
        assert!(input.keys_down.contains(&egui::Key::K));
    });
}

fn assert_starting_pass_input_removed(ctx: &egui::Context) {
    ctx.input(|input| {
        assert!(!input.events.iter().any(|event| matches!(
            event,
            egui::Event::Text(_)
                | egui::Event::Key { pressed: true, .. }
                | egui::Event::PointerButton { pressed: true, .. }
                | egui::Event::MouseWheel { .. }
        )));
        assert!(
            input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::PointerButton { pressed: false, .. })),
            "release edges retain their existing contract"
        );
        assert!(input.keys_down.is_empty());
        assert_eq!(input.smooth_scroll_delta, egui::Vec2::ZERO);
        assert!(input.pointer.latest_pos().is_none());
    });
}

// The real service is paused before its read is admitted. This makes the
// retained-start handler's pending branch deterministic without changing it.
fn paused_shift_service(
    app: &mut App,
    data: PathBuf,
) -> (
    crate::spread_shift::SpreadShiftProcessOwner,
    mpsc::Sender<()>,
) {
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    app.settings.remember_spread_shift_per_book = true;
    app.settings.book_root = app
        .current_folder
        .as_ref()
        .and_then(|folder| folder.parent())
        .map(Path::to_path_buf);
    let (service, owner) = SpreadShiftService::spawn_at(data, None).unwrap();
    let pause = service.pause_for_test();
    let scope = app
        .current_spread_shift_scope()
        .expect("physical book fixture");
    service
        .set_shift(
            scope,
            SpreadShiftPageRef::RelativePath("page.png".into()),
            ShiftOperation {
                context_id: ViewerContextId::for_test(828),
                items_generation: app.items_generation,
                operation_sequence: 1,
            },
        )
        .unwrap();
    app.spread_shift_service = Some(service);
    (owner, pause)
}

fn root_start_scenario(start: RestoreStart, after_gate: bool) {
    let (mut app, ctx, _, a, _) = reclick_stage2::reclick_stage2_fixture();
    let pos = visible_label_click_rect(&ctx, &a).center();
    sidecar_tree_frame(
        &mut app,
        &ctx,
        10.0,
        vec![
            egui::Event::PointerMoved(pos),
            primary_event(pos, true),
            primary_event(pos, false),
        ],
        None,
    );
    // Keep the first click's real refresh from adopting: this scenario drives
    // each start handler explicitly at the selected input checkpoint instead.
    let (_worker, _sender) = reclick::reclick_hold_preflight(&mut app);
    assert_label_expanded(&app, &a, true);
    let data = app.tmp.path().join("input-gate-shift");
    let shift =
        matches!(start, RestoreStart::RetainedShift).then(|| paused_shift_service(&mut app, data));
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 600.0),
            )),
            time: Some(10.04),
            events: starting_pass_events(pos),
            ..Default::default()
        },
        |ui| {
            app.begin_root_input_gate_pass(&ctx);
            app.edit_preview_repaint_ctx = Some(ctx.clone());
            if after_gate {
                app.input_update_begin(&ctx);
            }
            assert_starting_pass_input_present(&ctx);
            app.fs_seek_drag_active = true;
            app.analysis_guide_drag = Some((pos, pos, 1));
            start.begin(&mut app, &ctx);
            assert!(!app.fs_seek_drag_active, "context gesture ends at start");
            assert!(app.analysis_guide_drag.is_none());
            if !after_gate {
                assert_starting_pass_input_present(&ctx);
                app.input_update_begin(&ctx);
            }
            assert_starting_pass_input_removed(&ctx);
            // Recognition must have produced the same Toggle despite recovery
            // disabling the newly drawn widgets. Do not bypass the apply handler.
            app.render_toolbar(ui);
            assert!(app.render_folder_pane(ui).0.is_none());
            assert_label_expanded(&app, &a, false);
            app.finish_folder_pane_presentation(&ctx, true);
            app.end_root_input_gate_pass();
        },
    )
    .drop_without_applying_deltas();
    if let Some((owner, pause)) = shift {
        pause.send(()).unwrap();
        app.finish_page_identity_preparation_for_test();
        owner.shutdown_and_join();
    } else {
        app.finish_page_identity_preparation_for_test();
    }
}

#[test]
fn folder_pane_hydration_start_before_input_gate_preserves_witnesses_and_blocks_input() {
    root_start_scenario(RestoreStart::Hydration, false);
}

#[test]
fn folder_pane_hydration_start_after_input_gate_sanitizes_immediately() {
    root_start_scenario(RestoreStart::Hydration, true);
}

#[test]
fn folder_pane_page_index_start_before_input_gate_preserves_witnesses_and_blocks_input() {
    root_start_scenario(RestoreStart::PageIndex, false);
}

#[test]
fn folder_pane_page_index_start_after_input_gate_sanitizes_immediately() {
    root_start_scenario(RestoreStart::PageIndex, true);
}

#[test]
fn folder_pane_retained_shift_start_before_input_gate_preserves_witnesses_and_blocks_input() {
    root_start_scenario(RestoreStart::RetainedShift, false);
}

#[test]
fn folder_pane_retained_shift_start_after_input_gate_sanitizes_immediately() {
    root_start_scenario(RestoreStart::RetainedShift, true);
}

#[test]
fn folder_pane_restore_starts_in_child_viewport_sanitize_immediately() {
    for start in [
        RestoreStart::Hydration,
        RestoreStart::PageIndex,
        RestoreStart::RetainedShift,
    ] {
        let (mut app, ctx, _, _, _) = reclick_stage2::reclick_stage2_fixture();
        let data = app.tmp.path().join("child-input-gate-shift");
        let shift = matches!(start, RestoreStart::RetainedShift)
            .then(|| paused_shift_service(&mut app, data));
        let child = egui::ViewportId::from_hash_of("restore-start-child");
        let mut input = egui::RawInput {
            viewport_id: child,
            events: starting_pass_events(egui::pos2(20.0, 20.0)),
            ..Default::default()
        };
        input.viewports.insert(
            child,
            egui::ViewportInfo {
                parent: Some(egui::ViewportId::ROOT),
                ..Default::default()
            },
        );
        let output = ctx.run_pass(input, |ctx| {
            app.edit_preview_repaint_ctx = Some(ctx.clone());
            assert_eq!(ctx.viewport_id(), child);
            assert_starting_pass_input_present(ctx);
            start.begin(&mut app, ctx);
            assert_starting_pass_input_removed(ctx);
        });
        assert!(
            output.viewport_output[&child]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
        );
        output.drop_without_applying_deltas();
        if let Some((owner, pause)) = shift {
            pause.send(()).unwrap();
            app.finish_page_identity_preparation_for_test();
            owner.shutdown_and_join();
        } else {
            app.finish_page_identity_preparation_for_test();
        }
    }
}

#[test]
fn folder_pane_restore_starts_outside_pass_sanitize_immediately() {
    for start in [
        RestoreStart::Hydration,
        RestoreStart::PageIndex,
        RestoreStart::RetainedShift,
    ] {
        let (mut app, ctx, _, _, _) = reclick_stage2::reclick_stage2_fixture();
        let data = app.tmp.path().join("outside-input-gate-shift");
        let shift = matches!(start, RestoreStart::RetainedShift)
            .then(|| paused_shift_service(&mut app, data));
        ctx.run_pass(
            egui::RawInput {
                events: starting_pass_events(egui::pos2(20.0, 20.0)),
                ..Default::default()
            },
            |ctx| {
                app.begin_root_input_gate_pass(ctx);
                app.end_root_input_gate_pass();
            },
        )
        .drop_without_applying_deltas();
        let _output = ctx.run_logic(&egui::RawInput::default(), |ctx| {
            app.edit_preview_repaint_ctx = Some(ctx.clone());
            assert_starting_pass_input_present(ctx);
            start.begin(&mut app, ctx);
            assert_starting_pass_input_removed(ctx);
        });
        if let Some((owner, pause)) = shift {
            pause.send(()).unwrap();
            app.finish_page_identity_preparation_for_test();
            owner.shutdown_and_join();
        } else {
            app.finish_page_identity_preparation_for_test();
        }
    }
}

#[test]
fn folder_pane_started_restore_input_request_is_not_replayed_in_a_later_pass() {
    let (mut app, ctx, _, _, _) = reclick_stage2::reclick_stage2_fixture();
    let events = || starting_pass_events(egui::pos2(20.0, 20.0));
    ctx.run_pass(
        egui::RawInput {
            events: events(),
            ..Default::default()
        },
        |ctx| {
            app.begin_root_input_gate_pass(ctx);
            app.edit_preview_repaint_ctx = Some(ctx.clone());
            RestoreStart::PageIndex.begin(&mut app, ctx);
            assert_starting_pass_input_present(ctx);
            // Deliberately leave this harness pass before the input gate and its
            // normal end marker, so the gate itself must reject a stale request.
        },
    )
    .drop_without_applying_deltas();
    app.finish_page_identity_preparation_for_test();
    assert!(!app.sidecar_restore_blocks_projected_context());
    ctx.run_pass(
        egui::RawInput {
            events: events(),
            ..Default::default()
        },
        |ctx| {
            app.input_update_begin(ctx);
            assert_starting_pass_input_present(ctx);
            app.end_root_input_gate_pass();
        },
    )
    .drop_without_applying_deltas();
}
