//! Search history is committed at submission or one settled, executed departure.
//! Everything uses the existing fake transport helpers; no real IPC is started.
use super::*;
use crate::app::{
    LocalSearchFilter, LocalSearchRequest, LocalSearchSubmission, SearchPending,
    SearchProgressShared,
};
use crate::global_search_ui::{
    MetadataAppliedSpec, SearchProducer, adopt_everything_for_test, everything_dataset_for_test,
    start_everything_for_test,
};
use std::{
    collections::HashSet,
    sync::{Arc, atomic::AtomicBool},
};

fn configure_surface(app: &mut App, pinned: bool) {
    app.settings.show_toolbar_search_container = pinned;
    app.settings.show_toolbar_search_local = pinned;
    app.settings.show_toolbar_search_metadata = pinned;
    app.settings.show_toolbar_search_everything = pinned;
}

fn input(kind: Kind, pinned: bool) -> SearchInputId {
    if pinned {
        SearchInputId::Pinned(kind)
    } else {
        SearchInputId::TemporaryBar(kind)
    }
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    ctx
}

fn key_input(key: egui::Key) -> egui::RawInput {
    egui::RawInput {
        events: vec![egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    }
}

fn begin(app: &mut App, ctx: &egui::Context, raw: egui::RawInput) {
    ctx.begin_pass(raw);
    app.update_ime_state(ctx);
}

fn end(ctx: &egui::Context) {
    ctx.end_pass().drop_without_applying_deltas();
}

/// Draw actual helper-managed inputs so record_search_input_focus observes the
/// same widget registry as production, including fields that are currently idle.
fn draw_fields(
    app: &mut App,
    ctx: &egui::Context,
    fields: &[SearchInputId],
    focus: Option<SearchInputId>,
) {
    ctx.root_ui(|root| {
        egui::CentralPanel::default().show(root, |ui| {
            for &field in fields {
                let mut request = focus == Some(field);
                crate::ime_focus::show_singleline(
                    ui,
                    app.search_draft_mut(field.kind()),
                    Some(&mut request),
                    |edit| edit.id(field.id()),
                );
            }
        });
    });
}

fn remember_focused(app: &mut App, ctx: &egui::Context, field: SearchInputId) {
    begin(app, ctx, Default::default());
    draw_fields(app, ctx, &[field], Some(field));
    assert!(ctx.memory(|memory| memory.has_focus(field.id())));
    app.settle_search_input_departure(ctx, false);
    app.record_search_input_focus(ctx);
    let owner_key = ctx.viewport_id().0.with("search_input_owner");
    assert_eq!(
        ctx.data(|data| data.get_temp::<(SearchInputId, u64)>(owner_key)),
        Some((field, ctx.cumulative_pass_nr()))
    );
    end(ctx);
}

fn depart(app: &mut App, ctx: &egui::Context, field: SearchInputId, history_escape_taken: bool) {
    begin(app, ctx, Default::default());
    // A result receives focus before settle, as it does when the field loses
    // focus to a result click. Another widget also excludes IME focus recovery.
    ctx.memory_mut(|memory| memory.request_focus(egui::Id::unique("history-result")));
    draw_fields(app, ctx, &[field], None);
    assert!(!ctx.memory(|memory| memory.has_focus(field.id())));
    app.settle_search_input_departure(ctx, history_escape_taken);
    app.record_search_input_focus(ctx);
    end(ctx);
}

fn local_submission(query: &str) -> Arc<LocalSearchSubmission> {
    Arc::new(LocalSearchSubmission {
        query: query.into(),
        target: Default::default(),
        or_mode: false,
    })
}

fn executed(app: &mut App, ctx: &egui::Context, kind: Kind, query: &str) {
    app.ensure_search_session(kind);
    *app.search_draft_mut(kind) = query.into();
    match kind {
        Kind::Container => app.favsearch.last_executed = query.into(),
        Kind::Local => {
            app.search_filter = Some(LocalSearchFilter {
                matches: HashSet::new(),
                submitted: local_submission(query),
            })
        }
        Kind::Metadata => {
            app.global_search.last_executed = query.into();
            app.global_search.accepted = SearchAccepted::Metadata {
                applied: Some(MetadataAppliedSpec {
                    query: query.into(),
                    filters: app.global_search.filters.clone(),
                }),
            };
            app.global_search.done = true;
        }
        Kind::Everything => {
            // Empty immutable results keep the existing prepare/adoption owner
            // while avoiding filesystem scans, decoding and the IPC transport.
            adopt_everything_for_test(app, ctx, query, everything_dataset_for_test(Vec::new()));
        }
    }
    assert!(app.edit_owner(kind));
}

fn render_search(app: &mut App, ctx: &egui::Context, kind: Kind, pinned: bool) {
    ctx.root_ui(|root| {
        if pinned {
            egui::CentralPanel::default().show(root, |ui| {
                let output = app.draw_pinned_search_section(ui, kind);
                app.handle_pinned_search_actions(ctx, output.inner.actions);
            });
        } else {
            match kind {
                Kind::Container => app.render_favsearch_bar(root),
                Kind::Local => app.render_search_bar(root),
                Kind::Metadata | Kind::Everything => app.render_global_search_bar(root),
            }
        }
    });
}

fn render_pinned_search_actions(app: &mut App, ctx: &egui::Context, kind: Kind) -> Vec<Action> {
    let mut actions = Vec::new();
    ctx.root_ui(|root| {
        egui::CentralPanel::default().show(root, |ui| {
            let output = app.draw_pinned_search_section(ui, kind);
            actions = output.inner.actions;
            app.handle_pinned_search_actions(ctx, actions.clone());
        });
    });
    actions
}

#[test]
fn section1756_search_input_history_keyboard_pick_uses_each_actual_search_renderer_once() {
    for pinned in [false, true] {
        for kind in Kind::ALL {
            let _input = crate::key_input::lock_test_input();
            let mut app = crate::app::setup_app_for_test();
            configure_surface(&mut app, pinned);
            let ctx = context();
            app.ensure_search_session(kind);
            let chosen = if kind == Kind::Everything {
                "picked history query\0"
            } else {
                "picked history query"
            };
            app.settings
                .search_input_history
                .record(kind, "older history query");
            app.settings.search_input_history.record(kind, chosen);
            *app.search_draft_mut(kind) = "different unsubmitted draft".into();
            app.items.clear();
            app.visible_indices.clear();
            if kind == Kind::Container {
                app.favsearch.last_executed = chosen.into();
            }
            let field = input(kind, pinned);
            begin(&mut app, &ctx, Default::default());
            render_search(&mut app, &ctx, kind, pinned);
            assert!(ctx.memory(|memory| memory.has_focus(field.id())));
            end(&ctx);

            begin(&mut app, &ctx, key_input(egui::Key::ArrowDown));
            render_search(&mut app, &ctx, kind, pinned);
            assert!(ctx.memory(|memory| memory.has_focus(field.id())));
            assert_eq!(app.search_draft(kind), "different unsubmitted draft");
            assert!(!ctx.input(|input| input.key_pressed(egui::Key::ArrowDown)));
            end(&ctx);

            let local_sequence = app.input_seq;
            let global_sequence = app.global_search.run_sequence;
            begin(&mut app, &ctx, key_input(egui::Key::Enter));
            if pinned {
                let actions = render_pinned_search_actions(&mut app, &ctx, kind);
                assert_eq!(
                    actions
                        .iter()
                        .filter(|action| matches!(action, Action::Picked(_, _)))
                        .count(),
                    1
                );
                assert!(actions.contains(&Action::Picked(kind, 0)));
                assert!(
                    !actions
                        .iter()
                        .any(|action| matches!(action, Action::Submit(_))),
                    "a pick replaces the renderer's ordinary Enter submit"
                );
            } else {
                render_search(&mut app, &ctx, kind, pinned);
            }
            assert_eq!(app.search_draft(kind), chosen, "{kind:?}/{pinned}");
            assert_eq!(
                app.settings.search_input_history.entries(kind),
                [chosen, "older history query"]
            );
            assert!(
                !ctx.memory(|memory| memory.has_focus(field.id())),
                "pick must surrender focus: {kind:?}/{pinned}"
            );
            assert!(!ctx.input(|input| input.key_pressed(egui::Key::Enter)));
            assert!(app.pending_text_input_focus.get().is_none());
            assert!(
                !app.route_pending_search_submit(&ctx),
                "consumed pick must not reach reserved submit"
            );
            match kind {
                Kind::Local => {
                    assert_eq!(app.submitted_local_search().unwrap().query, chosen);
                    assert_eq!(
                        app.input_seq,
                        local_sequence + 1,
                        "pick must execute exactly one local request"
                    );
                }
                Kind::Metadata => assert_eq!(
                    app.global_search.run_sequence,
                    global_sequence + 1,
                    "pick must schedule exactly one metadata request"
                ),
                Kind::Everything => {
                    assert!(app.global_search.reject_message.is_some());
                    assert!(
                        app.global_search.pending.is_none(),
                        "invalid query avoids real IPC"
                    );
                }
                Kind::Container => {
                    assert!(!app.favsearch.focus_request);
                    assert!(app.favsearch_pending.is_none());
                }
            }
            let accepted_sequences = (app.input_seq, app.global_search.run_sequence);
            end(&ctx);
            begin(&mut app, &ctx, Default::default());
            render_search(&mut app, &ctx, kind, pinned);
            assert_eq!(
                (app.input_seq, app.global_search.run_sequence),
                accepted_sequences,
                "the next pass must not repeat the selection"
            );
            assert!(!ctx.memory(|memory| memory.has_focus(field.id())));
            end(&ctx);
        }
    }
}

#[test]
fn section1756_search_input_history_suffix_and_mouse_pick_use_the_app_submit_boundary() {
    use egui_kittest::kittest::Queryable;
    let _input = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    configure_surface(&mut app, true);
    app.ensure_search_session(Kind::Container);
    app.favsearch.query = "different unsubmitted draft".into();
    app.favsearch.last_executed = "picked container query".into();
    app.settings
        .search_input_history
        .record(Kind::Container, "picked container query");
    app.settings
        .search_input_history
        .record(Kind::Container, "newer container query");
    let field = Kind::Container.input_id();
    let mut harness = crate::test_ui::builder()
        .with_context_setup(|ctx| {
            crate::ime_focus::install_ime_input_policy(ctx);
            crate::ui_fonts::configure_fonts(ctx);
        })
        .with_size(egui::vec2(900.0, 360.0))
        .build_ui_state(
            |ui, app| {
                let ctx = ui.ctx().clone();
                app.update_ime_state(&ctx);
                let output = app.draw_pinned_search_section(ui, Kind::Container);
                app.handle_pinned_search_actions(&ctx, output.inner.actions);
            },
            app,
        );
    harness.run_steps(3);
    assert!(harness.ctx.memory(|memory| memory.has_focus(field)));
    let arrow_key = field.with(("history_field_arrow_rect", harness.ctx.viewport_id()));
    let arrow = harness
        .ctx
        .data(|data| data.get_temp::<egui::Rect>(arrow_key))
        .unwrap()
        .center();
    for pressed in [true, false] {
        harness.input_mut().events.extend([
            egui::Event::PointerMoved(arrow),
            egui::Event::PointerButton {
                pos: arrow,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        harness.step();
        assert!(harness.ctx.memory(|memory| memory.has_focus(field)));
    }
    harness.run_steps(3);
    let item = harness
        .get_by_label("picked container query")
        .rect()
        .center();
    harness.input_mut().events.extend([
        egui::Event::PointerMoved(item),
        egui::Event::PointerButton {
            pos: item,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    harness.step();
    assert!(harness.ctx.memory(|memory| memory.has_focus(field)));
    assert_eq!(
        harness.state().favsearch.query,
        "different unsubmitted draft"
    );
    harness.input_mut().events.push(egui::Event::PointerButton {
        pos: item,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    harness.step();
    harness.run_steps(2);
    assert_eq!(harness.state().favsearch.query, "picked container query");
    assert_eq!(
        harness
            .state()
            .settings
            .search_input_history
            .entries(Kind::Container),
        ["picked container query", "newer container query"]
    );
    assert!(!harness.ctx.memory(|memory| memory.has_focus(field)));
    assert!(!harness.state().favsearch.focus_request);
    assert!(harness.state().favsearch_pending.is_none());
}

#[test]
fn section1756_search_input_history_submit_records_button_enter_and_reserved_routes() {
    for pinned in [false, true] {
        for kind in Kind::ALL {
            for route in ["button", "enter", "reserved"] {
                let _input = crate::key_input::lock_test_input();
                let mut app = crate::app::setup_app_for_test();
                configure_surface(&mut app, pinned);
                let ctx = context();
                app.ensure_search_session(kind);
                // Everything validation rejects NUL before creating any real run.
                // Failed searches still belong in the history by specification.
                let query = if kind == Kind::Everything {
                    "history-query\0"
                } else {
                    "history-query"
                };
                *app.search_draft_mut(kind) = query.into();
                app.items.clear();
                app.visible_indices.clear();
                if kind == Kind::Container {
                    app.favsearch.last_executed = query.into(); // No scan is needed.
                }
                let field = input(kind, pinned);
                if route == "enter" {
                    begin(&mut app, &ctx, Default::default());
                    render_search(&mut app, &ctx, kind, pinned);
                    assert!(ctx.memory(|memory| memory.has_focus(field.id())));
                    end(&ctx);
                }
                begin(
                    &mut app,
                    &ctx,
                    if route == "button" {
                        Default::default()
                    } else {
                        key_input(egui::Key::Enter)
                    },
                );
                match route {
                    "button" => assert!(app.submit_search_input(&ctx, kind, field.id())),
                    "reserved" => {
                        app.claim_pending_text_input_focus(
                            ctx.viewport_id(),
                            field.id(),
                            ctx.cumulative_pass_nr(),
                        );
                        assert!(app.route_pending_search_submit(&ctx), "{kind:?}/{pinned}");
                    }
                    _ => render_search(&mut app, &ctx, kind, pinned),
                }
                assert_eq!(
                    app.settings.search_input_history.entries(kind),
                    [query],
                    "{kind:?}/{pinned}/{route}"
                );
                assert_eq!(app.settings.search_input_history.len(), 1);
                if kind == Kind::Everything {
                    assert!(app.global_search.reject_message.is_some());
                    assert!(
                        app.global_search.pending.is_none(),
                        "validation must avoid real IPC"
                    );
                }
                end(&ctx);
            }
        }
    }
}

#[test]
fn section1756_search_input_history_executed_departure_records_all_kinds_and_surfaces() {
    for pinned in [false, true] {
        for kind in Kind::ALL {
            let mut app = crate::app::setup_app_for_test();
            configure_surface(&mut app, pinned);
            let ctx = context();
            executed(&mut app, &ctx, kind, "executed query");
            let field = input(kind, pinned);
            remember_focused(&mut app, &ctx, field);
            assert_eq!(
                app.settings.search_input_history.len(),
                0,
                "focused input is not a departure"
            );
            depart(&mut app, &ctx, field, false);
            assert_eq!(
                app.settings.search_input_history.entries(kind),
                ["executed query"],
                "{kind:?}/{pinned}"
            );
        }
    }
}

#[test]
fn section1756_search_input_history_unexecuted_draft_departure_is_not_recorded() {
    for pinned in [false, true] {
        for kind in Kind::ALL {
            let mut app = crate::app::setup_app_for_test();
            configure_surface(&mut app, pinned);
            let ctx = context();
            executed(&mut app, &ctx, kind, "previous execution");
            *app.search_draft_mut(kind) = "still waiting for debounce".into();
            let field = input(kind, pinned);
            remember_focused(&mut app, &ctx, field);
            depart(&mut app, &ctx, field, false);
            assert_eq!(
                app.settings.search_input_history.len(),
                0,
                "{kind:?}/{pinned}"
            );
        }
    }
}

#[test]
fn section1756_search_input_history_ended_by_escape_or_clear_does_not_record_departure() {
    for pinned in [false, true] {
        for kind in Kind::ALL {
            for escape in [false, true] {
                let _input = crate::key_input::lock_test_input();
                let mut app = crate::app::setup_app_for_test();
                configure_surface(&mut app, pinned);
                let ctx = context();
                executed(&mut app, &ctx, kind, "executed but cancelled");
                let field = input(kind, pinned);
                remember_focused(&mut app, &ctx, field);
                begin(
                    &mut app,
                    &ctx,
                    if escape {
                        key_input(egui::Key::Escape)
                    } else {
                        Default::default()
                    },
                );
                ctx.memory_mut(|memory| memory.surrender_focus(field.id()));
                if escape {
                    app.route_search_escape(&ctx, Some(field), &mut EscapeClaim::default());
                } else if pinned {
                    app.handle_pinned_search_actions(&ctx, vec![Action::Clear(kind)]);
                } else {
                    app.close_owned_search(&ctx, kind); // Existing temporary-bar × path.
                }
                assert!(!app.edit_owner(kind), "{kind:?}/{pinned}/{escape}");
                app.settle_search_input_departure(&ctx, false);
                assert_eq!(app.settings.search_input_history.len(), 0);
                end(&ctx);
            }
        }
    }
}

#[test]
fn section1756_search_input_history_popup_escape_and_disabled_saving_skip_departure() {
    for pinned in [false, true] {
        for disabled in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            configure_surface(&mut app, pinned);
            let ctx = context();
            executed(&mut app, &ctx, Kind::Container, "executed query");
            let field = input(Kind::Container, pinned);
            remember_focused(&mut app, &ctx, field);
            app.settings.save_search_input_history = !disabled;
            depart(&mut app, &ctx, field, !disabled);
            assert!(app.edit_owner(Kind::Container));
            assert_eq!(app.settings.search_input_history.len(), 0);
        }
    }
}

#[test]
fn section1756_search_input_history_skipped_pass_cannot_replay_an_old_departure() {
    for pinned in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        configure_surface(&mut app, pinned);
        let ctx = context();
        executed(&mut app, &ctx, Kind::Container, "executed query");
        let field = input(Kind::Container, pinned);
        remember_focused(&mut app, &ctx, field);
        begin(&mut app, &ctx, Default::default());
        ctx.memory_mut(|memory| memory.surrender_focus(field.id()));
        // Simulate an update return before settle and record_search_input_focus.
        end(&ctx);
        depart(&mut app, &ctx, field, false);
        assert_eq!(app.settings.search_input_history.len(), 0);
        let owner_key = ctx.viewport_id().0.with("search_input_owner");
        assert!(
            ctx.data(|data| data.get_temp::<(SearchInputId, u64)>(owner_key))
                .is_none()
        );
    }
}

#[test]
fn section1756_search_input_history_a_to_b_to_results_is_independent_of_field_draw_order() {
    for reversed in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        configure_surface(&mut app, true);
        let ctx = context();
        let a = input(Kind::Container, true);
        let b = input(Kind::Local, true);
        let fields = if reversed { [b, a] } else { [a, b] };
        executed(&mut app, &ctx, Kind::Container, "container query");
        begin(&mut app, &ctx, Default::default());
        draw_fields(&mut app, &ctx, &fields, Some(a));
        app.record_search_input_focus(&ctx);
        end(&ctx);
        begin(&mut app, &ctx, Default::default());
        // Merely focusing another pinned field does not start its search.
        draw_fields(&mut app, &ctx, &fields, Some(b));
        app.settle_search_input_departure(&ctx, false);
        assert_eq!(
            app.settings.search_input_history.entries(Kind::Container),
            ["container query"]
        );
        // B is executed before the following pass, while B now owns focus.
        executed(&mut app, &ctx, Kind::Local, "local query");
        app.record_search_input_focus(&ctx);
        end(&ctx);
        depart(&mut app, &ctx, b, false);
        assert_eq!(
            app.settings.search_input_history.entries(Kind::Local),
            ["local query"]
        );
        assert_eq!(
            app.settings.search_input_history.entries(Kind::Container),
            ["container query"]
        );
    }
}

#[test]
fn section1756_search_input_history_local_departure_uses_current_pending_request() {
    for pinned in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        configure_surface(&mut app, pinned);
        let ctx = context();
        executed(&mut app, &ctx, Kind::Local, "old accepted query");
        let submitted = Arc::new(LocalSearchSubmission {
            query: "current pending query".into(),
            target: crate::fts_index::SearchTarget::Only(vec![
                crate::fts_index::SourceKind::Filename,
            ]),
            or_mode: true,
        });
        let (_stream, rx) = std::sync::mpsc::channel();
        app.search_pending = Some(SearchPending {
            request: Arc::new(LocalSearchRequest {
                submitted: submitted.clone(),
                list_identity: app.local_search_list_identity(),
            }),
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(SearchProgressShared::new(0)),
            rx,
        });
        app.search_query = submitted.query.clone();
        app.search_target = submitted.target.clone();
        app.search_or_mode = submitted.or_mode;
        let field = input(Kind::Local, pinned);
        remember_focused(&mut app, &ctx, field);
        depart(&mut app, &ctx, field, false);
        assert!(app.search_pending.is_some());
        assert_eq!(
            app.settings.search_input_history.entries(Kind::Local),
            ["current pending query"]
        );
    }
}

#[test]
fn section1756_search_input_history_metadata_running_result_departure_records_without_completion() {
    for pinned in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        configure_surface(&mut app, pinned);
        let ctx = context();
        executed(&mut app, &ctx, Kind::Metadata, "running metadata query");
        let (_stream, rx) = crossbeam_channel::unbounded();
        app.global_search.pending = Some(SearchProducer::Metadata(
            crate::indexer_manager::SearchHandle {
                cancel: Arc::new(AtomicBool::new(false)),
                rx,
            },
        ));
        app.global_search.done = false;
        assert_eq!(app.pinned_search_state(Kind::Metadata), State::Running);
        let field = input(Kind::Metadata, pinned);
        remember_focused(&mut app, &ctx, field);
        depart(&mut app, &ctx, field, false);
        assert!(app.global_search.pending.is_some());
        assert!(!app.global_search.done);
        assert_eq!(
            app.settings.search_input_history.entries(Kind::Metadata),
            ["running metadata query"]
        );
    }
}

#[test]
fn section1756_search_input_history_everything_pending_query_precedes_retained_results() {
    for pinned in [false, true] {
        for current_draft in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            configure_surface(&mut app, pinned);
            let ctx = context();
            executed(&mut app, &ctx, Kind::Everything, "old accepted query");
            let _cancel = start_everything_for_test(&mut app, &ctx, "current pending query");
            app.global_search.everything_draft = if current_draft {
                "current pending query"
            } else {
                "old accepted query"
            }
            .into();
            let field = input(Kind::Everything, pinned);
            remember_focused(&mut app, &ctx, field);
            depart(&mut app, &ctx, field, false);
            assert_eq!(
                app.settings.search_input_history.len(),
                usize::from(current_draft)
            );
            if current_draft {
                assert_eq!(
                    app.settings.search_input_history.entries(Kind::Everything),
                    ["current pending query"]
                );
            }
        }
    }
}

#[test]
fn section1756_search_input_history_rejected_or_disabled_submissions_do_not_record() {
    for pinned in [false, true] {
        for kind in Kind::ALL {
            for disabled in [false, true] {
                let _input = crate::key_input::lock_test_input();
                let mut app = crate::app::setup_app_for_test();
                configure_surface(&mut app, pinned);
                app.ensure_search_session(kind);
                let ctx = context();
                let query = if kind == Kind::Everything {
                    "history-query\0"
                } else {
                    "history-query"
                };
                *app.search_draft_mut(kind) = query.into();
                app.favsearch.last_executed = query.into();
                app.settings.save_search_input_history = !disabled;
                app.show_stats_dialog = !disabled;
                begin(&mut app, &ctx, Default::default());
                assert_eq!(
                    app.submit_search_input(&ctx, kind, input(kind, pinned).id()),
                    disabled
                );
                assert_eq!(
                    app.settings.search_input_history.len(),
                    0,
                    "{kind:?}/{pinned}/{disabled}"
                );
                end(&ctx);
            }
        }
    }
}

fn history_keys(keys: &[egui::Key]) -> egui::RawInput {
    #[cfg(windows)]
    crate::key_input::set_test_frame(
        keys.iter()
            .map(|key| crate::key_input::KeyEdge {
                operation_id: crate::key_input::KeyOperationId::unassigned(),
                source_hwnd: 0,
                source_viewport: egui::ViewportId::ROOT,
                virtual_key: match key {
                    egui::Key::ArrowDown => 0x28,
                    egui::Key::ArrowUp => 0x26,
                    egui::Key::Enter => 0x0D,
                    _ => unreachable!(),
                },
                scan_code: 0,
                extended: false,
                pressed: true,
                repeat: false,
                ctrl: false,
                shift: false,
                alt: false,
            })
            .collect(),
    );
    egui::RawInput {
        events: keys
            .iter()
            .map(|&key| egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            })
            .collect(),
        ..Default::default()
    }
}

fn history_pending_grid_case(
    search: Option<(Kind, bool)>,
    keys: &[egui::Key],
    edit: Option<egui::Event>,
) {
    let _input = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let ctx = context();
    crate::ui_fonts::configure_fonts(&ctx);
    let current = app.tmp.path().join("current");
    let target = app.tmp.path().join("target");
    let after = app.tmp.path().join("after");
    std::fs::create_dir(&current).unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::create_dir(&after).unwrap();
    let draft = if search.is_some_and(|(kind, _)| kind == Kind::Everything) {
        "draft\0"
    } else {
        "draft"
    };
    let chosen = if search.is_some_and(|(kind, _)| kind == Kind::Everything) {
        "chosen\0"
    } else {
        "chosen"
    };
    let field = if let Some((kind, pinned)) = search {
        configure_surface(&mut app, pinned);
        app.ensure_search_session(kind);
        app.search_focus_request = false;
        app.favsearch.focus_request = false;
        app.global_search.focus_request = false;
        *app.search_draft_mut(kind) = draft.into();
        app.favsearch.last_executed = draft.into();
        app.settings.search_input_history.record(kind, chosen);
        input(kind, pinned).id()
    } else {
        app.current_folder = Some(current.clone());
        app.address = current.to_string_lossy().into_owned();
        app.active_quick_folder_slot = None;
        app.recent_folders = vec![target.clone()];
        assert_eq!(app.recent_folder_entries(), &[target.clone()]);
        app.settings.toolbar_section_order = vec![crate::settings::ToolbarSectionId::Folder];
        app.settings.show_address_bar_history_menu = true;
        egui::Id::unique("folder_address_input")
    };
    app.items = vec![
        crate::app::GridItem::Folder(target.clone()),
        crate::app::GridItem::Folder(current.clone()),
        crate::app::GridItem::Folder(after),
    ];
    // A middle cursor in one column must move for either leaked vertical arrow.
    app.visible_indices = vec![0, 1, 2];
    app.settings.grid_cols = 1;
    app.selected = Some(1);
    let draw = |app: &mut App| {
        if let Some((kind, pinned)) = search {
            render_search(app, &ctx, kind, pinned);
            None
        } else {
            ctx.root_ui(|ui| app.render_toolbar(ui).1)
        }
    };
    let mut initial = history_keys(&[]);
    initial.screen_rect = Some(egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(1600.0, 500.0),
    ));
    begin(&mut app, &ctx, initial);
    assert!(draw(&mut app).is_none());
    end(&ctx);
    assert!(!ctx.memory(|m| m.has_focus(field)));
    let pos = crate::ui_history_field::arrow_rect_for_test(&ctx, field).center();
    let click = egui::RawInput {
        events: vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        ..Default::default()
    };
    begin(&mut app, &ctx, click);
    assert!(draw(&mut app).is_none());
    assert!(
        !ctx.memory(|m| m.has_focus(field)),
        "test must observe the reservation before helper focus"
    );
    assert_eq!(app.pending_text_input_focus.get().unwrap().widget_id, field);
    assert!(crate::ui_history_field::is_open_for(&ctx, field));
    end(&ctx);
    let mut raw = history_keys(keys);
    if let Some(event) = edit.clone() {
        raw.events.insert(0, event);
    }
    begin(&mut app, &ctx, raw);
    assert!(
        matches!(app.keyboard_owner_for_pass(&ctx), crate::keyboard_input::KeyboardOwner::TextInput { widget_id, phase: crate::keyboard_input::TextInputPhase::PendingFocus, .. } if widget_id == field)
    );
    assert!(
        app.handle_keyboard(&ctx).is_none(),
        "{search:?}/{keys:?}: grid must not open a result"
    );
    assert_eq!(
        app.selected,
        Some(1),
        "{search:?}/{keys:?}: grid must not move selection"
    );
    let nav = draw(&mut app);
    assert!(
        matches!(crate::keyboard_input::cached_keyboard_owner(&ctx), Some(crate::keyboard_input::KeyboardOwner::TextInput {
        widget_id, phase: crate::keyboard_input::TextInputPhase::Focused, ..
    }) if widget_id == field),
        "the same owner must receive actual focus for the rest of this pass"
    );
    assert!(
        app.handle_keyboard(&ctx).is_none(),
        "post-render grid handler must remain blocked"
    );

    for key in keys {
        assert!(
            !ctx.input(|i| i.key_pressed(*key)),
            "history must consume {key:?}"
        );
    }
    #[cfg(windows)]
    assert!(!crate::key_input::pressed_key_down(
        ctx.viewport_id(),
        |edge| matches!(edge.virtual_key, 0x0D | 0x26 | 0x28)
    ));
    if keys.contains(&egui::Key::Enter) {
        assert!(app.pending_text_input_focus.get().is_none());
        assert!(!crate::ui_history_field::is_open_for(&ctx, field));
        if let Some((kind, _)) = search {
            let expected = if keys.contains(&egui::Key::ArrowDown) {
                chosen
            } else {
                draft
            };
            if edit.is_some() {
                assert!(
                    app.search_draft(kind).contains("edit-marker"),
                    "pending input must reach TextEdit before submit"
                );
                assert_eq!(
                    app.settings.search_input_history.entries(kind)[0],
                    app.search_draft(kind)
                );
            } else {
                assert_eq!(app.search_draft(kind), expected);
                assert_eq!(app.settings.search_input_history.entries(kind)[0], expected);
            }
        } else {
            let expected = if keys.contains(&egui::Key::ArrowDown) {
                &target
            } else {
                &current
            };
            assert!(
                matches!(nav,Some(crate::ui_main::AddressBarNav::Direct(ref path,_)) if crate::folder_tree::path_eq(path, expected)),
                "{keys:?}: normal address/history navigation: {nav:?} expected {expected:?}"
            );
        }
    } else {
        assert!(nav.is_none());
        assert!(ctx.memory(|m| m.has_focus(field)));
        assert!(crate::ui_history_field::is_open_for(&ctx, field));
    }
    end(&ctx);
    if !keys.contains(&egui::Key::Enter) {
        begin(&mut app, &ctx, history_keys(&[]));
        assert!(
            ctx.memory(|m| m.has_focus(field)),
            "reserved arrows must not transfer focus at end_pass"
        );
        assert!(crate::ui_history_field::is_open_for(&ctx, field));
        assert!(draw(&mut app).is_none());
        end(&ctx);
    } else if search.is_none() {
        begin(&mut app, &ctx, history_keys(&[egui::Key::ArrowUp]));
        assert!(!app.address_has_focus);
        assert!(
            !app.shortcuts_blocked_by_text_input(&ctx),
            "address submit must release the next pass"
        );
        assert!(app.handle_keyboard(&ctx).is_none());
        assert_eq!(
            app.selected,
            Some(0),
            "grid keys must resume after address submission"
        );
        end(&ctx);
    }
}

#[test]
fn section1756_history_same_pass_arrow_click_reserves_next_pass_search_keys_before_grid() {
    for pinned in [false, true] {
        for kind in Kind::ALL {
            for keys in [
                vec![egui::Key::ArrowDown],
                vec![egui::Key::ArrowUp],
                vec![egui::Key::Enter],
                vec![egui::Key::ArrowDown, egui::Key::Enter],
            ] {
                history_pending_grid_case(Some((kind, pinned)), &keys, None);
            }
        }
    }
}

#[test]
fn section1756_history_same_pass_arrow_click_reserves_next_pass_folder_keys_before_grid() {
    for keys in [
        vec![egui::Key::ArrowDown],
        vec![egui::Key::ArrowUp],
        vec![egui::Key::Enter],
        vec![egui::Key::ArrowDown, egui::Key::Enter],
    ] {
        history_pending_grid_case(None, &keys, None);
    }
}

#[test]
fn section1756_pending_history_text_and_paste_enter_reach_the_editor_before_submit() {
    for pinned in [false, true] {
        for kind in Kind::ALL {
            for edit in [
                egui::Event::Text("edit-marker".into()),
                egui::Event::Paste("edit-marker".into()),
            ] {
                history_pending_grid_case(Some((kind, pinned)), &[egui::Key::Enter], Some(edit));
            }
        }
    }
}

#[test]
fn section1756_history_focus_reservation_yields_to_another_fields_arrow_in_both_draw_orders() {
    for reversed in [false, true] {
        for (first, second) in [
            (Kind::Metadata, Kind::Everything),
            (Kind::Everything, Kind::Metadata),
        ] {
            let _input = crate::key_input::lock_test_input();
            let mut app = crate::app::setup_app_for_test();
            configure_surface(&mut app, true);
            let ctx = context();
            crate::ui_fonts::configure_fonts(&ctx);
            app.ensure_search_session(Kind::Everything);
            app.global_search.focus_request = false;
            for kind in [first, second] {
                app.settings.search_input_history.record(kind, "history");
            }
            let order = if reversed {
                [second, first]
            } else {
                [first, second]
            };
            let draw = |app: &mut App| {
                ctx.root_ui(|root| {
                    egui::CentralPanel::default().show(root, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for kind in order {
                                let response = app.draw_pinned_search_section(ui, kind);
                                app.handle_pinned_search_actions(&ctx, response.inner.actions);
                            }
                        });
                    });
                })
            };
            let mut raw = history_keys(&[]);
            raw.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 400.0),
            ));
            begin(&mut app, &ctx, raw);
            draw(&mut app);
            end(&ctx);
            for kind in [first, second] {
                let pos =
                    crate::ui_history_field::arrow_rect_for_test(&ctx, kind.input_id()).center();
                begin(
                    &mut app,
                    &ctx,
                    egui::RawInput {
                        events: vec![
                            egui::Event::PointerMoved(pos),
                            egui::Event::PointerButton {
                                pos,
                                button: egui::PointerButton::Primary,
                                pressed: true,
                                modifiers: egui::Modifiers::NONE,
                            },
                            egui::Event::PointerButton {
                                pos,
                                button: egui::PointerButton::Primary,
                                pressed: false,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                        ..Default::default()
                    },
                );
                app.keyboard_owner_for_pass(&ctx);
                draw(&mut app);
                assert_eq!(
                    app.pending_text_input_focus.get().unwrap().widget_id,
                    kind.input_id()
                );
                assert!(!ctx.memory(|m| m.has_focus(first.input_id()) || m.has_focus(second.input_id())), "outside pointer must supersede pending focus");
                end(&ctx);
            }
            begin(&mut app, &ctx, history_keys(&[egui::Key::ArrowDown]));
            assert!(app.handle_keyboard(&ctx).is_none());
            draw(&mut app);
            assert!(ctx.memory(|m| m.has_focus(second.input_id())));
            assert!(crate::ui_history_field::is_open_for(
                &ctx,
                second.input_id()
            ));
            end(&ctx);
            begin(&mut app, &ctx, history_keys(&[]));
            assert!(ctx.memory(|m| m.has_focus(second.input_id())));
            end(&ctx);
        }
    }
}
