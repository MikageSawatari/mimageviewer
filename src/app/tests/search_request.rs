use super::*;
use crate::global_search_ui::{MetadataAppliedSpec, SearchAccepted, SearchProducer};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

fn pin(app: &mut App, enabled: bool) {
    app.settings.show_toolbar_search_local = enabled;
    app.settings.show_toolbar_search_metadata = enabled;
    app.settings.show_toolbar_search_everything = enabled;
}

fn input_id(kind: Kind, pinned: bool) -> egui::Id {
    if pinned {
        kind.input_id()
    } else {
        kind.temporary_id()
    }
}

fn draw(app: &mut App, ctx: &egui::Context, kind: Kind, pinned: bool) {
    ctx.root_ui(|root| {
        if pinned {
            egui::CentralPanel::default().show(root, |ui| {
                let response = app.draw_pinned_search_section(ui, kind);
                app.handle_pinned_search_actions(ctx, response.inner.actions);
            });
        } else {
            match kind {
                Kind::Local => app.render_search_bar(root),
                Kind::Metadata | Kind::Everything => app.render_global_search_bar(root),
                Kind::Container => app.render_favsearch_bar(root),
            }
        }
    });
    app.record_search_input_focus(ctx);
}

fn end(ctx: &egui::Context) {
    let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    ctx
}

fn inject(app: &mut App, ctx: &egui::Context, target: Kind, query: &str) {
    app.open_search_with_text(
        ctx,
        SearchRequest {
            target,
            query: query.into(),
        },
    );
}

fn selection(ctx: &egui::Context, id: egui::Id) -> std::ops::Range<egui::text::CharIndex> {
    egui::TextEdit::load_state(ctx, id)
        .unwrap()
        .cursor
        .char_range()
        .unwrap()
        .as_sorted_char_range()
}

#[test]
fn search_name_three_targets_pinned_and_temporary_focus_select_and_wait_for_enter() {
    let _input = crate::key_input::lock_test_input();
    for pinned in [false, true] {
        for kind in [Kind::Local, Kind::Metadata, Kind::Everything] {
            let mut app = super::super::setup_app_for_test();
            pin(&mut app, pinned);
            let ctx = context();
            ctx.begin_pass(egui::RawInput::default());
            inject(&mut app, &ctx, kind, "日本語 name");
            assert!(app.edit_owner(kind), "{kind:?}, pinned={pinned}");
            assert_eq!(app.search_draft(kind), "日本語 name");
            assert_eq!(
                crate::search_request::select_all_pending_for_test(&ctx, input_id(kind, pinned)),
                kind != Kind::Everything
            );
            for _ in 0..5 {
                app.poll_global_search_debounce(&ctx);
            }
            assert!(app.search_pending.is_none());
            assert!(app.search_filter.is_none());
            assert!(app.global_search.pending.is_none());
            if kind == Kind::Metadata {
                assert!(!app.global_search.is_searching());
            }
            draw(&mut app, &ctx, kind, pinned);
            let id = input_id(kind, pinned);
            assert!(ctx.memory(|memory| memory.has_focus(id)));
            assert_eq!(
                selection(&ctx, id),
                egui::text::CharIndex(0)..egui::text::CharIndex(8)
            );
            end(&ctx);
            ctx.begin_pass(egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: Some(egui::Key::Enter),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            });
            app.update_ime_state(&ctx);
            draw(&mut app, &ctx, kind, pinned);
            match kind {
                Kind::Local => assert!(app.search_pending.is_some() || app.search_filter.is_some()),
                Kind::Metadata => assert!(app.global_search.last_change_at.is_some()),
                Kind::Everything => assert!(matches!(
                    app.global_search.pending,
                    Some(SearchProducer::Everything { .. })
                )),
                _ => unreachable!(),
            }
            end(&ctx);
        }
    }
}

#[test]
fn search_name_reinjects_into_open_unfocused_fields_and_selects_each_time() {
    for pinned in [false, true] {
        for kind in [Kind::Local, Kind::Metadata, Kind::Everything] {
            let mut app = super::super::setup_app_for_test();
            pin(&mut app, pinned);
            let ctx = context();
            ctx.begin_pass(egui::RawInput::default());
            app.ensure_search_session(kind);
            *app.search_draft_mut(kind) = "previous".into();
            draw(&mut app, &ctx, kind, pinned);
            end(&ctx);
            let id = input_id(kind, pinned);
            ctx.memory_mut(|memory| memory.surrender_focus(id));
            app.search_has_focus = false;
            app.global_search.has_focus = false;
            for query in ["new name", "再注入"] {
                ctx.begin_pass(egui::RawInput::default());
                inject(&mut app, &ctx, kind, query);
                draw(&mut app, &ctx, kind, pinned);
                assert!(ctx.memory(|memory| memory.has_focus(id)));
                assert_eq!(
                    selection(&ctx, id),
                    egui::text::CharIndex(0)..egui::text::CharIndex(query.chars().count())
                );
                end(&ctx);
                ctx.memory_mut(|memory| memory.surrender_focus(id));
                app.search_has_focus = false;
                app.global_search.has_focus = false;
            }
        }
    }
}

#[test]
fn search_name_normal_local_and_metadata_open_preserve_collapsed_selection() {
    for pinned in [false, true] {
        for kind in [Kind::Local, Kind::Metadata] {
            let mut app = super::super::setup_app_for_test();
            pin(&mut app, pinned);
            let ctx = context();
            ctx.begin_pass(egui::RawInput::default());
            app.open_search_shortcut(&ctx, kind);
            *app.search_draft_mut(kind) = "previous".into();
            draw(&mut app, &ctx, kind, pinned);
            let id = input_id(kind, pinned);
            let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(3),
                )));
            state.store(&ctx, id);
            end(&ctx);
            ctx.memory_mut(|memory| memory.surrender_focus(id));
            app.search_has_focus = false;
            app.global_search.has_focus = false;
            ctx.begin_pass(egui::RawInput::default());
            // The temporary Ctrl+G shortcut remains a toggle; the retained input
            // is reopened using its existing close/open transaction.
            if !pinned && kind == Kind::Metadata {
                app.toggle_global_search();
                app.global_search.metadata_draft = "previous".into();
            }
            app.open_search_shortcut(&ctx, kind);
            draw(&mut app, &ctx, kind, pinned);
            let range = selection(&ctx, id);
            assert_eq!(range.start, range.end, "{kind:?}, pinned={pinned}");
            end(&ctx);
        }
    }
}

#[test]
fn search_name_metadata_injection_disarms_completed_waiting_and_running_queries() {
    for phase in 0..3 {
        let mut app = super::super::setup_app_for_test();
        let ctx = context();
        app.ensure_search_session(Kind::Metadata);
        app.begin_search_page_edit_prepare_for_test(&ctx);
        app.global_search.metadata_draft = "old executed".into();
        app.global_search.last_executed = "old executed".into();
        app.global_search.accepted = SearchAccepted::Metadata {
            applied: Some(MetadataAppliedSpec {
                query: "old executed".into(),
                filters: app.global_search.filters.clone(),
            }),
        };
        let SearchAccepted::Metadata {
            applied: applied_before,
        } = &app.global_search.accepted
        else {
            unreachable!()
        };
        let applied_before = applied_before.clone();
        let items_before = app.items.clone();
        app.global_search.done = phase == 0;
        app.global_search.last_change_at =
            (phase != 0).then(|| Instant::now() - Duration::from_secs(2));
        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        if phase == 2 {
            app.global_search.pending = Some(SearchProducer::Metadata(
                crate::indexer_manager::SearchHandle {
                    cancel: cancel.clone(),
                    rx,
                },
            ));
        }
        ctx.begin_pass(egui::RawInput::default());
        inject(&mut app, &ctx, Kind::Metadata, "new draft");
        assert_eq!(app.global_search.last_executed, "old executed");
        assert!(
            matches!(&app.global_search.accepted, SearchAccepted::Metadata { applied: Some(applied) } if applied.query == "old executed")
        );
        assert!(app.global_search.last_change_at.is_none());
        assert!(
            matches!(&app.global_search.accepted, SearchAccepted::Metadata { applied } if applied == &applied_before)
        );
        assert_eq!(app.items, items_before);
        assert!(app.global_search.done);
        assert!(app.global_search.pending.is_none());
        assert!(app.global_search.page_edit_prepare.is_none());
        for _ in 0..8 {
            app.poll_global_search_debounce(&ctx);
            app.poll_global_search_events(&ctx);
        }
        assert_eq!(cancel.load(Ordering::Relaxed), phase == 2);
        if phase == 2 {
            assert!(
                tx.send(crate::global_search::SearchStreamEvent::Done {
                    truncated: false,
                    reason: crate::global_search::DoneReason::Complete,
                })
                .is_err(),
                "retired query no longer has a receiver"
            );
            app.poll_global_search_events(&ctx);
            assert!(app.global_search.done);
        }
        let run = app.global_search.run_sequence;
        for _ in 0..8 {
            app.poll_global_search_debounce(&ctx);
        }
        assert_eq!(app.global_search.run_sequence, run);
        assert_eq!(app.global_search.last_executed, "old executed");
        assert!(
            matches!(&app.global_search.accepted, SearchAccepted::Metadata { applied } if applied == &applied_before)
        );
        assert!(app.global_search.page_edit_prepare.is_none());
        assert!(app.submit_search_input(&ctx, Kind::Metadata, Kind::Metadata.temporary_id()));
        assert!(app.global_search.last_change_at.is_some());
        assert_ne!(app.global_search.run_sequence, run);
        app.global_search.last_change_at = Some(Instant::now() - Duration::from_secs(2));
        app.poll_global_search_debounce(&ctx);
        assert_eq!(app.global_search.last_executed, "new draft");
        end(&ctx);
    }
}

#[test]
fn search_name_abandoned_selection_never_revives_on_ordinary_reopen() {
    let _input = crate::key_input::lock_test_input();
    for pinned in [false, true] {
        for kind in [Kind::Local, Kind::Metadata] {
            for abandoned in ["undrawn", "escape", "switch"] {
                let mut app = super::super::setup_app_for_test();
                pin(&mut app, pinned);
                let ctx = context();
                ctx.begin_pass(egui::RawInput::default());
                inject(&mut app, &ctx, kind, "same draft");
                // No field draw: the mark has not been consumed.
                end(&ctx);
                if abandoned == "undrawn" {
                    for _ in 0..4 {
                        ctx.begin_pass(egui::RawInput::default());
                        end(&ctx);
                    }
                }
                if abandoned == "escape" {
                    ctx.begin_pass(egui::RawInput {
                        events: vec![egui::Event::Key {
                            key: egui::Key::Escape,
                            physical_key: Some(egui::Key::Escape),
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }],
                        ..Default::default()
                    });
                    let owner = app.capture_search_escape_input(&ctx);
                    app.route_search_escape(
                        &ctx,
                        owner,
                        &mut super::super::pinned_search::EscapeClaim::default(),
                    );
                    assert!(!app.edit_owner(kind));
                    end(&ctx);
                } else {
                    // Switching to Container exercises the ordinary session
                    // teardown, including a temporary draft's clearing.
                    app.ensure_search_session(Kind::Container);
                }
                ctx.begin_pass(egui::RawInput::default());
                // Keep the spelling identical even for temporary inputs so the
                // test cannot pass only because closing cleared their draft.
                *app.search_draft_mut(kind) = "same draft".into();
                app.open_search_shortcut(&ctx, kind);
                draw(&mut app, &ctx, kind, pinned);
                let range = selection(&ctx, input_id(kind, pinned));
                assert_eq!(
                    range.start, range.end,
                    "{kind:?}, pinned={pinned}, {abandoned}"
                );
                end(&ctx);
            }
        }
    }
}

#[test]
fn search_name_shortcut_conjunctions_preserve_local_and_metadata_admission() {
    let mut app = super::super::setup_app_for_test();
    for bits in 0..256 {
        app.address_has_focus = bits & 1 != 0;
        app.search_has_focus = bits & 2 != 0;
        app.favsearch.has_focus = bits & 4 != 0;
        app.global_search.has_focus = bits & 8 != 0;
        app.tag_view.has_focus = bits & 16 != 0;
        app.global_search.active = bits & 32 != 0;
        app.show_stats_dialog = bits & 64 != 0;
        let blocked = bits & 128 != 0;
        let base = !app.address_has_focus
            && !blocked
            && !app.any_dialog_open()
            && !app.favsearch.has_focus
            && !app.tag_view.has_focus;
        assert_eq!(
            app.search_shortcut_availability(Kind::Local, blocked)
                .is_ok(),
            base && !app.global_search.has_focus && !app.global_search.active
        );
        assert_eq!(
            app.search_shortcut_availability(Kind::Metadata, blocked)
                .is_ok(),
            base && !app.search_has_focus
        );
    }
}

#[test]
fn search_name_local_refusal_keeps_both_search_result_sources_and_physical_children() {
    for everything in [false, true] {
        for child in [false, true] {
            let mut app = super::super::setup_app_for_test();
            let ctx = context();
            ctx.begin_pass(egui::RawInput::default());
            let kind = if everything {
                Kind::Everything
            } else {
                Kind::Metadata
            };
            app.ensure_search_session(kind);
            if everything {
                crate::global_search_ui::adopt_everything_for_test(
                    &mut app,
                    &ctx,
                    "old",
                    crate::global_search_ui::everything_dataset_for_test(Vec::new()),
                );
                if child {
                    let SearchAccepted::Everything { position, .. } =
                        &mut app.global_search.accepted
                    else {
                        unreachable!()
                    };
                    *position = crate::global_search_ui::EverythingPosition::PhysicalChild {
                        logical_source: app.tmp.path().to_owned(),
                        return_anchor: app.tmp.path().to_owned(),
                        route: super::super::FacetRoute::root(super::super::FacetScope::path(
                            app.tmp.path(),
                        )),
                    };
                }
            } else if child {
                app.global_search.drill = Some(crate::global_search_ui::DrillState {
                    container_root: app.tmp.path().to_owned(),
                    current_path: app.tmp.path().to_owned(),
                    is_zip: false,
                });
            }
            app.items_are_global_search_view = !child;
            app.search_query = "keep local".into();
            assert!(
                app.search_with_text_availability(&ctx, Kind::Local)
                    .is_err()
            );
            inject(&mut app, &ctx, Kind::Local, "blocked");
            assert_eq!(app.search_query, "keep local");
            assert!(app.edit_owner(kind));
            assert!(!app.show_search_bar);
            end(&ctx);
        }
    }
}

#[test]
fn search_name_switches_existing_sessions_through_ensure_without_submitting() {
    for (from, to) in [
        (Kind::Container, Kind::Local),
        (Kind::Container, Kind::Metadata),
        (Kind::Container, Kind::Everything),
        (Kind::Metadata, Kind::Everything),
        (Kind::Everything, Kind::Metadata),
    ] {
        let mut app = super::super::setup_app_for_test();
        let ctx = context();
        ctx.begin_pass(egui::RawInput::default());
        app.ensure_search_session(from);
        draw(&mut app, &ctx, from, false);
        end(&ctx);
        ctx.memory_mut(|memory| {
            if let Some(id) = memory.focused() {
                memory.surrender_focus(id);
            }
        });
        app.search_has_focus = false;
        app.favsearch.has_focus = false;
        app.global_search.has_focus = false;
        ctx.begin_pass(egui::RawInput::default());
        inject(&mut app, &ctx, to, "switch draft");
        assert!(app.edit_owner(to), "{from:?} -> {to:?}");
        assert!(!app.edit_owner(from));
        assert_eq!(app.search_draft(to), "switch draft");
        assert!(app.global_search.pending.is_none());
        assert!(app.search_pending.is_none());
        end(&ctx);
    }
}

#[test]
fn search_name_item_entry_excludes_wrong_surfaces_views_and_checked_selection() {
    let mut app = super::super::setup_app_for_test();
    let ctx = context();
    let item = GridItem::Image(app.tmp.path().join("name.jpg"));
    assert_eq!(
        app.grid_item_search_name(&ctx, &item).as_deref(),
        Some("name")
    );
    app.checked.insert(0);
    assert!(app.grid_item_search_name(&ctx, &item).is_none());
    app.checked.clear();
    app.items_are_bookmark_view = true;
    assert!(app.grid_item_search_name(&ctx, &item).is_none());
    app.items_are_bookmark_view = false;
    app.items_are_reading_history_view = true;
    assert!(app.grid_item_search_name(&ctx, &item).is_none());
    app.items_are_reading_history_view = false;
    app.fullscreen_idx = Some(0);
    assert!(app.grid_item_search_name(&ctx, &item).is_none());
    app.fullscreen_idx = None;
    let viewport = egui::ViewportId::from_hash_of("detached-search-name-test");
    ctx.begin_pass(egui::RawInput {
        viewport_id: viewport,
        ..Default::default()
    });
    assert!(app.grid_item_search_name(&ctx, &item).is_none());
    end(&ctx);
}

#[test]
fn search_name_rebound_grid_action_routes_after_existing_keyboard_ownership() {
    let _input = crate::key_input::lock_test_input();
    for (action, target) in [
        (KeyAction::GridSearchNameEverything, Kind::Everything),
        (KeyAction::GridSearchNameMetadata, Kind::Metadata),
        (KeyAction::GridSearchNameLocal, Kind::Local),
    ] {
        let mut app = super::super::setup_app_for_test();
        let mut settings = crate::keymap::KeymapSettings::default();
        settings.set_override_chords(
            action,
            vec![crate::keymap::Chord::ctrl_shift(crate::keymap::KeyName::F)],
        );
        app.keymap = crate::keymap::Keymap::from_settings(&settings);
        app.items = vec![GridItem::Image(app.tmp.path().join("selected name.jpg"))];
        app.selected = Some(0);
        let ctx = context();
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::F,
                physical_key: Some(egui::Key::F),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            }],
            ..Default::default()
        });
        assert!(app.handle_keyboard(&ctx).is_none());
        assert!(app.edit_owner(target));
        assert_eq!(app.search_draft(target), "selected name");
        end(&ctx);
    }
}

#[test]
fn search_name_key_respects_text_ime_modal_pane_and_entry_ownership() {
    let _input = crate::key_input::lock_test_input();
    for blocked in [
        "text",
        "ime",
        "modal",
        "checked",
        "bookmark",
        "history",
        "fullscreen",
        "pane",
    ] {
        let mut app = super::super::setup_app_for_test();
        let mut settings = crate::keymap::KeymapSettings::default();
        settings.set_override_chords(
            KeyAction::GridSearchNameMetadata,
            vec![crate::keymap::Chord::ctrl_shift(crate::keymap::KeyName::F)],
        );
        app.keymap = crate::keymap::Keymap::from_settings(&settings);
        app.items = vec![GridItem::Image(app.tmp.path().join("selected.jpg"))];
        app.selected = Some(0);
        let ctx = context();
        match blocked {
            "checked" => {
                app.checked.insert(0);
            }
            "bookmark" => app.items_are_bookmark_view = true,
            "history" => app.items_are_reading_history_view = true,
            "fullscreen" => app.fullscreen_idx = Some(0),
            "modal" => app.show_stats_dialog = true,
            "pane" => {
                app.settings.folder_tree_pane_visible = true;
            }
            _ => {}
        }
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::F,
                physical_key: Some(egui::Key::F),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            }],
            ..Default::default()
        });
        if matches!(blocked, "text" | "ime") {
            crate::keyboard_input::cache_keyboard_owner(
                &ctx,
                crate::keyboard_input::KeyboardOwner::TextInput {
                    viewport: egui::ViewportId::ROOT,
                    widget_id: egui::Id::unique("search-name-text-owner"),
                    phase: if blocked == "ime" {
                        crate::keyboard_input::TextInputPhase::ImeComposing
                    } else {
                        crate::keyboard_input::TextInputPhase::Focused
                    },
                },
            );
        }
        if blocked == "pane" {
            // The pane's existing focus owner is independent of text input.
            app.folder_pane.has_focus = true;
        }
        app.handle_keyboard(&ctx);
        assert!(!app.global_search.active, "{blocked}");
        assert!(app.global_search.metadata_draft.is_empty(), "{blocked}");
        end(&ctx);
    }
}
