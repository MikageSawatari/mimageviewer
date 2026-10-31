//! One transaction boundary for the four toolbar inputs and one Escape consumer.
use super::App;
use crate::global_search_ui::{SearchAccepted, SearchSource};
use crate::ui_pinned_search::{
    PinnedSearchAction as Action, PinnedSearchKind as Kind, PinnedSearchState as State,
};

impl From<SearchSource> for Kind {
    fn from(source: SearchSource) -> Self {
        match source {
            SearchSource::Metadata => Self::Metadata,
            SearchSource::Everything => Self::Everything,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global_search_ui::{
        MetadataAppliedSpec, adopt_everything_for_test, everything_dataset_for_test,
        start_everything_for_test,
    };

    fn pin_all(app: &mut App) {
        app.settings.show_toolbar_search_container = true;
        app.settings.show_toolbar_search_local = true;
        app.settings.show_toolbar_search_metadata = true;
        app.settings.show_toolbar_search_everything = true;
    }
    fn escape(ctx: &egui::Context) {
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
    }
    fn end_pass(ctx: &egui::Context) {
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
    }
    fn metadata_result(app: &mut App, ctx: &egui::Context) {
        app.ensure_search_session(Kind::Metadata);
        app.global_search.metadata_draft = "accepted metadata".into();
        app.global_search.accepted = SearchAccepted::Metadata {
            applied: Some(MetadataAppliedSpec {
                query: "accepted metadata".into(),
                filters: app.global_search.filters.clone(),
            }),
        };
        app.items_are_global_search_view = true;
        app.global_search.done = true;
        app.update_global_search_address();
        let _ = ctx;
    }

    #[test]
    fn section1409_focus_and_everything_edit_do_not_activate_but_unedited_submit_does() {
        let mut app = super::super::setup_app_for_test();
        pin_all(&mut app);
        let ctx = egui::Context::default();
        app.handle_pinned_search_actions(
            &ctx,
            vec![
                Action::Focused(Kind::Metadata),
                Action::Edited(Kind::Everything),
            ],
        );
        assert!(app.closable_owner().is_none());
        // Empty Everything submission exercises entry and validation without IPC.
        app.handle_pinned_search_actions(&ctx, vec![Action::Submit(Kind::Everything)]);
        assert!(app.edit_owner(Kind::Everything));
        assert!(app.global_search.reject_message.is_some());
        assert!(app.global_search.pending.is_none());
        app.search_query = "retained".into();
        app.handle_pinned_search_actions(&ctx, vec![Action::Submit(Kind::Local)]);
        assert!(app.show_search_bar);
        assert!(!app.global_search.active);
        assert!(app.search_pending.is_some() || app.search_filter.is_some());
    }

    #[test]
    fn section1409_edits_enter_once_preserve_origin_and_use_existing_execution_timing() {
        let mut app = super::super::setup_app_for_test();
        pin_all(&mut app);
        let ctx = egui::Context::default();
        app.search_query = "local".into();
        app.handle_pinned_search_actions(&ctx, vec![Action::Edited(Kind::Local)]);
        assert!(app.show_search_bar);
        assert!(app.search_pending.is_none());
        app.favsearch.query = "container".into();
        app.handle_pinned_search_actions(&ctx, vec![Action::Edited(Kind::Container)]);
        assert_eq!(app.favsearch.last_executed, "container");
        let origin = app.favsearch.saved_folder.clone();
        app.favsearch
            .results_paths
            .push(std::path::PathBuf::from("sentinel"));
        app.handle_pinned_search_actions(&ctx, vec![Action::Edited(Kind::Container)]);
        assert_eq!(app.favsearch.saved_folder, origin);
        assert_eq!(
            app.favsearch.results_paths.len(),
            1,
            "ensure must not reopen the session"
        );
        app.global_search.metadata_draft = "metadata".into();
        app.handle_pinned_search_actions(&ctx, vec![Action::Submit(Kind::Metadata)]);
        assert!(app.edit_owner(Kind::Metadata));
        assert!(app.global_search.last_change_at.is_some());
        assert!(
            app.global_search.pending.is_none(),
            "Enter follows debounce"
        );
        assert_eq!(app.search_query, "local");
        assert_eq!(app.favsearch.query, "container");
    }

    #[test]
    fn section1409_inactive_clear_is_scoped_and_end_only_clears_unpinned_drafts() {
        let mut app = super::super::setup_app_for_test();
        pin_all(&mut app);
        let ctx = egui::Context::default();
        app.ensure_search_session(Kind::Container);
        app.favsearch.query = "keep container".into();
        app.global_search.everything_draft = "clear me".into();
        app.handle_pinned_search_actions(&ctx, vec![Action::Clear(Kind::Everything)]);
        assert!(app.favsearch.active);
        assert_eq!(app.favsearch.query, "keep container");
        assert!(app.global_search.everything_draft.is_empty());
        app.ensure_search_session(Kind::Metadata);
        app.global_search.metadata_draft = "keep metadata".into();
        app.ensure_search_session(Kind::Everything);
        assert_eq!(app.global_search.metadata_draft, "keep metadata");
        app.settings.show_toolbar_search_metadata = false;
        app.ensure_search_session(Kind::Metadata);
        app.ensure_search_session(Kind::Everything);
        assert!(app.global_search.metadata_draft.is_empty());
    }

    #[test]
    fn section1409_metadata_applied_spec_survives_source_switch_and_address_uses_it() {
        let mut app = super::super::setup_app_for_test();
        pin_all(&mut app);
        let ctx = egui::Context::default();
        metadata_result(&mut app, &ctx);
        assert_eq!(app.pinned_search_state(Kind::Metadata), State::Applied);
        app.global_search.metadata_draft = "unapplied edit".into();
        assert_eq!(app.pinned_search_state(Kind::Metadata), State::Edited);
        app.global_search.everything_draft = "different Everything".into();
        app.ensure_search_session(Kind::Everything);
        app.update_global_search_address();
        assert!(app.address.contains("accepted metadata"));
        assert!(!app.address.contains("different Everything"));
        assert!(!app.address.contains("unapplied edit"));
        assert_eq!(app.applied_owner(), Some(Kind::Metadata));
    }

    #[test]
    fn section1409_clear_and_escape_applied_owner_close_both_source_directions() {
        for metadata in [true, false] {
            for clear in [true, false] {
                let mut app = super::super::setup_app_for_test();
                pin_all(&mut app);
                let ctx = egui::Context::default();
                let (applied, editing) = if metadata {
                    metadata_result(&mut app, &ctx);
                    (Kind::Metadata, Kind::Everything)
                } else {
                    let rows = everything_dataset_for_test(Vec::new());
                    adopt_everything_for_test(&mut app, &ctx, "accepted Everything", rows);
                    (Kind::Everything, Kind::Metadata)
                };
                app.ensure_search_session(editing);
                *app.search_draft_mut(editing) = "keep other draft".into();
                if clear {
                    app.handle_pinned_search_actions(&ctx, vec![Action::Clear(applied)]);
                } else {
                    escape(&ctx);
                    app.route_search_escape(
                        &ctx,
                        Some(SearchInputId::Pinned(applied)),
                        &mut EscapeClaim::default(),
                    );
                    end_pass(&ctx);
                }
                assert!(!app.global_search.active);
                assert_eq!(app.search_draft(editing), "keep other draft");
                assert_eq!(app.search_draft(applied).is_empty(), clear);
                assert!(!app.items_are_global_search_view);
            }
        }
    }

    #[test]
    fn section1409_clear_unsent_edit_returns_to_accepted_or_closes_without_results() {
        let ctx = egui::Context::default();
        for metadata in [true, false] {
            let mut app = super::super::setup_app_for_test();
            pin_all(&mut app);
            let (accepted, editing) = if metadata {
                metadata_result(&mut app, &ctx);
                (Kind::Metadata, Kind::Everything)
            } else {
                adopt_everything_for_test(
                    &mut app,
                    &ctx,
                    "accepted Everything",
                    everything_dataset_for_test(Vec::new()),
                );
                (Kind::Everything, Kind::Metadata)
            };
            let draft = app.search_draft(accepted).to_owned();
            app.ensure_search_session(editing);
            app.handle_pinned_search_actions(&ctx, vec![Action::Clear(editing)]);
            assert!(app.edit_owner(accepted));
            assert_eq!(app.applied_owner(), Some(accepted));
            assert_eq!(app.search_draft(accepted), draft);
            assert_eq!(app.global_search.last_executed, draft);
        }
        let mut app = super::super::setup_app_for_test();
        pin_all(&mut app);
        app.ensure_search_session(Kind::Everything);
        app.handle_pinned_search_actions(&ctx, vec![Action::Clear(Kind::Everything)]);
        assert!(!app.global_search.active);
    }

    #[test]
    fn section1409_unfocused_escape_closes_unsubmitted_local_and_cancelled_everything() {
        let mut app = super::super::setup_app_for_test();
        pin_all(&mut app);
        let ctx = egui::Context::default();
        app.ensure_search_session(Kind::Local);
        assert!(app.applied_owner().is_none());
        escape(&ctx);
        app.route_search_escape(&ctx, None, &mut EscapeClaim::default());
        assert!(!app.show_search_bar);
        assert!(!app.dialog_escape_pressed(&ctx), "claim consumes Escape");
        end_pass(&ctx);
        let cancel = start_everything_for_test(&mut app, &ctx, "fake running");
        escape(&ctx);
        let mut claim = EscapeClaim::default();
        app.route_search_escape(&ctx, None, &mut claim);
        app.route_search_escape(&ctx, None, &mut claim);
        assert!(cancel.is_cancelled());
        assert!(app.global_search.active);
        end_pass(&ctx);
        escape(&ctx);
        app.route_search_escape(&ctx, None, &mut EscapeClaim::default());
        assert!(!app.global_search.active);
        end_pass(&ctx);
    }

    #[test]
    fn section1409_ctrl_g_pin_focus_claim_does_not_toggle_close_and_availability_is_existing() {
        let mut app = super::super::setup_app_for_test();
        pin_all(&mut app);
        let ctx = egui::Context::default();
        app.global_search.metadata_draft = "retained draft".into();
        app.open_search_shortcut(&ctx, Kind::Metadata);
        assert!(
            !app.global_search.is_searching(),
            "a retained draft is not a pending search"
        );
        let run = app.global_search.run_sequence;
        app.open_search_shortcut(&ctx, Kind::Metadata);
        assert!(app.global_search.active);
        assert_eq!(app.global_search.run_sequence, run);
        assert_eq!(
            app.pending_text_input_focus.get().unwrap().widget_id,
            Kind::Metadata.input_id()
        );
        assert!(
            app.pinned_search_availability(Kind::Local).is_ok(),
            "another search does not disable fields"
        );
        assert!(
            app.pinned_search_availability(Kind::Metadata).is_ok(),
            "index initialization accepts drafts"
        );
        app.show_stats_dialog = true;
        assert!(
            Kind::ALL
                .into_iter()
                .all(|kind| app.pinned_search_availability(kind).is_err())
        );
    }

    #[test]
    fn section1409_reopening_pinned_metadata_does_not_rearm_an_old_debounce() {
        let mut app = super::super::setup_app_for_test();
        pin_all(&mut app);
        let ctx = egui::Context::default();
        app.global_search.metadata_draft = "old draft".into();
        app.handle_pinned_search_actions(&ctx, vec![Action::Edited(Kind::Metadata)]);
        assert!(app.global_search.last_change_at.is_some());
        app.close_global_search();
        app.ensure_search_session(Kind::Metadata);
        app.poll_global_search_debounce(&ctx);
        assert_eq!(app.global_search.metadata_draft, "old draft");
        assert!(app.global_search.last_change_at.is_none());
        assert!(app.global_search.pending.is_none());
        assert!(!app.global_search.is_searching());
    }

    #[test]
    fn section1409_availability_preserves_each_existing_linked_viewer_boundary() {
        let mut app = super::super::setup_app_for_test();
        app.fullscreen_idx = Some(0);
        app.viewer_presentation = super::super::ViewerPresentation::Fullscreen;
        assert!(
            Kind::ALL
                .into_iter()
                .all(|kind| app.pinned_search_availability(kind).is_err())
        );
        app.viewer_presentation = super::super::ViewerPresentation::DetachedWindow;
        assert!(!app.viewer_session_blocks_main_window());
        for kind in [Kind::Local, Kind::Container, Kind::Metadata] {
            assert!(
                app.pinned_search_availability(kind).is_ok(),
                "F/S/G must retain their existing admission: {kind:?}"
            );
        }
        assert_eq!(
            app.everything_search_entry_availability(),
            crate::global_search_ui::EverythingSearchEntryAvailability::LinkedViewer
        );
        assert!(app.pinned_search_availability(Kind::Everything).is_err());
    }

    #[test]
    fn section1409_inactive_field_cannot_consume_a_stale_open_focus_request() {
        let mut app = super::super::setup_app_for_test();
        pin_all(&mut app);
        app.ensure_search_session(Kind::Container);
        app.search_focus_request = true; // Superseded Local entry in the same pass.
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        ctx.begin_pass(egui::RawInput::default());
        ctx.root_ui(|root| {
            egui::CentralPanel::default().show(root, |ui| {
                app.draw_pinned_search_section(ui, Kind::Container);
                app.draw_pinned_search_section(ui, Kind::Local);
            });
        });
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            Some(Kind::Container.input_id())
        );
        assert!(!app.search_focus_request);
        end_pass(&ctx);
    }

    #[test]
    fn section1409_temporary_escape_matches_close_button_with_all_off_or_one_global_pin() {
        for metadata in [true, false] {
            for accepted_pin in [false, true] {
                for escape_key in [false, true] {
                    let mut app = super::super::setup_app_for_test();
                    let ctx = egui::Context::default();
                    app.settings.show_toolbar_search_metadata = metadata && accepted_pin;
                    app.settings.show_toolbar_search_everything = !metadata && accepted_pin;
                    let (accepted, editing) = if metadata {
                        metadata_result(&mut app, &ctx);
                        (Kind::Metadata, Kind::Everything)
                    } else {
                        adopt_everything_for_test(
                            &mut app,
                            &ctx,
                            "accepted Everything",
                            everything_dataset_for_test(Vec::new()),
                        );
                        (Kind::Everything, Kind::Metadata)
                    };
                    app.ensure_search_session(editing);
                    assert!(!editing.pinned(&app.settings));
                    if escape_key {
                        escape(&ctx);
                        app.route_search_escape(
                            &ctx,
                            Some(SearchInputId::TemporaryBar(editing)),
                            &mut EscapeClaim::default(),
                        );
                        assert!(!app.dialog_escape_pressed(&ctx));
                        end_pass(&ctx);
                    } else {
                        app.close_global_search(); // Existing temporary × transaction.
                    }
                    assert!(
                        !app.global_search.active,
                        "{metadata}/{accepted_pin}/{escape_key}"
                    );
                    assert!(!app.items_are_global_search_view);
                    assert_eq!(!app.search_draft(accepted).is_empty(), accepted_pin);
                    assert!(app.top_level_grid_view.return_to().is_none());
                }
            }
        }
        // A pre-draw owner may have ended during this pass. It must not take the
        // new session's return origin through a stale temporary close transaction.
        for (stale, current) in [
            (Kind::Container, Kind::Metadata),
            (Kind::Metadata, Kind::Container),
        ] {
            let mut app = super::super::setup_app_for_test();
            let ctx = egui::Context::default();
            app.ensure_search_session(stale);
            app.ensure_search_session(current);
            escape(&ctx);
            app.route_search_escape(
                &ctx,
                Some(SearchInputId::TemporaryBar(stale)),
                &mut EscapeClaim::default(),
            );
            assert!(app.edit_owner(current));
            end_pass(&ctx);
        }
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;
    fn end_pass(ctx: &egui::Context) {
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
    }
    #[test]
    #[expect(deprecated, reason = "Exercise platform IME compatibility events")]
    fn section1409_predraw_escape_owner_survives_begin_pass_and_composing_escape() {
        for composing in [false, true] {
            let mut app = super::super::setup_app_for_test();
            app.settings.show_toolbar_search_everything = true;
            app.ensure_search_session(Kind::Container);
            let ctx = egui::Context::default();
            crate::ime_focus::install_ime_input_policy(&ctx);
            let draw = |app: &mut App, focus| {
                ctx.root_ui(|root| {
                    egui::CentralPanel::default().show(root, |ui| {
                        crate::ui_pinned_search::draw_pinned_search(
                            ui,
                            Kind::Everything,
                            &mut app.global_search.everything_draft,
                            State::Inactive,
                            Ok(()),
                            focus,
                            false,
                            false,
                        );
                    });
                });
                app.record_search_input_focus(&ctx);
            };
            ctx.begin_pass(egui::RawInput::default());
            draw(&mut app, true);
            assert_eq!(
                ctx.memory(|memory| memory.focused()),
                Some(Kind::Everything.input_id())
            );
            end_pass(&ctx);
            if composing {
                ctx.begin_pass(egui::RawInput {
                    events: vec![
                        egui::Event::Ime(egui::ImeEvent::Enabled),
                        egui::Event::Ime(egui::ImeEvent::Preedit {
                            text: "検索".into(),
                            active_range_chars: None,
                        }),
                    ],
                    ..Default::default()
                });
                draw(&mut app, false);
                end_pass(&ctx);
            }
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
            if !composing {
                assert!(
                    ctx.memory(|memory| memory.focused()).is_none(),
                    "egui begin_pass releases Escape focus"
                );
            }
            let owner = app.capture_search_escape_input(&ctx);
            assert_eq!(owner, Some(SearchInputId::Pinned(Kind::Everything)));
            draw(&mut app, false);
            let mut claim = EscapeClaim::default();
            app.route_search_escape(&ctx, owner, &mut claim);
            assert!(
                app.favsearch.active,
                "inactive Everything must not close Container"
            );
            assert_eq!(claim.0, !composing);
            end_pass(&ctx);
        }
    }

    #[test]
    #[expect(deprecated, reason = "Exercise composition during toolbar relocation")]
    fn section1409_reorder_pin_toggle_and_global_focus_aggregate_use_stable_ids() {
        let mut app = super::super::setup_app_for_test();
        app.settings.show_toolbar_search_metadata = true;
        app.settings.show_toolbar_search_everything = true;
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        for (index, kinds) in [
            [Kind::Metadata, Kind::Everything],
            [Kind::Everything, Kind::Metadata],
            [Kind::Metadata, Kind::Everything],
        ]
        .into_iter()
        .enumerate()
        {
            let events = if index == 1 {
                vec![
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit {
                        text: "検索".into(),
                        active_range_chars: None,
                    }),
                ]
            } else {
                Vec::new()
            };
            ctx.begin_pass(egui::RawInput {
                events,
                ..Default::default()
            });
            app.prepare_pinned_search_frame(&ctx);
            ctx.root_ui(|root| {
                egui::CentralPanel::default().show(root, |ui| {
                    for kind in kinds {
                        if index == 0 && kind == Kind::Metadata {
                            let response = crate::ui_pinned_search::draw_pinned_search(
                                ui,
                                kind,
                                app.search_draft_mut(kind),
                                State::Inactive,
                                Ok(()),
                                true,
                                false,
                                false,
                            );
                            app.global_search.has_focus |= response.inner.input.has_focus();
                        } else {
                            app.draw_pinned_search_section(ui, kind);
                        }
                    }
                });
            });
            assert_eq!(
                ctx.memory(|memory| memory.focused()),
                Some(Kind::Metadata.input_id())
            );
            assert!(
                app.global_search.has_focus,
                "later unfocused field must not overwrite focus"
            );
            end_pass(&ctx);
        }
        for visible in [false, true] {
            app.settings.show_toolbar_search_everything = visible;
            ctx.begin_pass(egui::RawInput::default());
            app.prepare_pinned_search_frame(&ctx);
            ctx.root_ui(|root| {
                egui::CentralPanel::default().show(root, |ui| {
                    app.draw_pinned_search_section(ui, Kind::Metadata);
                    if visible {
                        app.draw_pinned_search_section(ui, Kind::Everything);
                    }
                });
            });
            assert_eq!(
                ctx.memory(|memory| memory.focused()),
                Some(Kind::Metadata.input_id())
            );
            assert!(app.ime_input_active(&ctx));
            end_pass(&ctx);
        }
        app.request_pinned_search_focus(&ctx, Kind::Everything);
        app.settings.show_toolbar_search_everything = false;
        app.prepare_pinned_search_frame(&ctx);
        assert!(app.pending_text_input_focus.get().is_none());
        app.settings.show_toolbar_search_metadata = false;
        app.search_has_focus = true;
        app.favsearch.has_focus = true;
        app.global_search.has_focus = true;
        app.prepare_pinned_search_frame(&ctx);
        assert!(!app.search_has_focus && !app.favsearch.has_focus && !app.global_search.has_focus);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchInputId {
    Pinned(Kind),
    TemporaryBar(Kind),
}
impl SearchInputId {
    fn kind(self) -> Kind {
        match self {
            Self::Pinned(kind) | Self::TemporaryBar(kind) => kind,
        }
    }
    fn id(self) -> egui::Id {
        match self {
            Self::Pinned(kind) => kind.input_id(),
            Self::TemporaryBar(kind) => kind.temporary_id(),
        }
    }
}

#[derive(Default)]
pub(crate) struct EscapeClaim(bool);
impl EscapeClaim {
    pub(crate) fn for_frame(app: &App) -> Self {
        // A modal can close while being drawn; its Escape still belongs to it.
        Self(app.any_dialog_open() && !app.everything_search_progress_visible())
    }

    fn claim(&mut self, ctx: &egui::Context) {
        self.0 = true;
        ctx.input_mut(|input| {
            input.consume_key(input.modifiers, egui::Key::Escape);
        });
    }
}

#[cfg(test)]
mod production_tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };
    #[test]
    fn section1409_production_toolbar_keeps_folder_and_four_fields_inside_rows() {
        for width in [1120.0, 720.0, 360.0] {
            let mut app = super::super::setup_app_for_test();
            app.settings.show_toolbar_folder_tree_button = false;
            app.settings.show_toolbar_effetune = false;
            app.settings.show_toolbar_bookshelf = false;
            app.settings.show_toolbar_collections = false;
            app.settings.show_toolbar_cols = false;
            app.settings.show_toolbar_aspect = false;
            app.settings.show_toolbar_sort = false;
            app.settings.show_toolbar_rating = false;
            app.settings.show_toolbar_favorites = false;
            app.settings.show_toolbar_smart_folders = false;
            app.settings.show_toolbar_tags = false;
            app.settings.show_toolbar_folder = true;
            app.settings.show_toolbar_search_container = true;
            app.settings.show_toolbar_search_local = true;
            app.settings.show_toolbar_search_metadata = true;
            app.settings.show_toolbar_search_everything = true;
            let app = Rc::new(RefCell::new(app));
            let render = app.clone();
            let fonts = Cell::new(false);
            let mut harness = crate::test_ui::builder()
                .with_size(egui::vec2(width, 700.0))
                .build_ui(move |root| {
                    let ctx = root.ctx().clone();
                    if !fonts.replace(true) {
                        crate::ui_fonts::configure_fonts(&ctx);
                        ctx.request_repaint();
                        return;
                    }
                    let mut app = render.borrow_mut();
                    app.render_toolbar(root);
                    let folder = app
                        .toolbar_section_anchors
                        .iter()
                        .find(|(id, _)| *id == crate::settings::ToolbarSectionId::Folder)
                        .unwrap()
                        .1;
                    for kind in Kind::ALL {
                        let section = app
                            .toolbar_section_anchors
                            .iter()
                            .find(|(id, _)| *id == kind.section())
                            .unwrap()
                            .1;
                        let input = ctx.read_response(kind.input_id()).unwrap().rect;
                        assert!(input.width() >= 148.0, "{width} {kind:?} {input:?}");
                        assert!(
                            input.left() >= 0.0 && input.right() <= width + 0.5,
                            "{width} {kind:?} {input:?}"
                        );
                        assert!(section.top() >= folder.bottom(), "Folder owns its row");
                    }
                });
            harness.run();
        }
    }
    #[test]
    fn section1409_modal_escape_remains_claimed_after_modal_closes() {
        let mut app = super::super::setup_app_for_test();
        app.ensure_search_session(Kind::Local);
        app.show_stats_dialog = true;
        let mut claim = EscapeClaim::for_frame(&app);
        app.show_stats_dialog = false;
        let ctx = egui::Context::default();
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
        app.route_search_escape(&ctx, None, &mut claim);
        assert!(app.show_search_bar);
        assert!(!app.dialog_escape_pressed(&ctx));
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
    }
    #[test]
    fn section1409_physical_child_unfocused_escape_closes_the_existing_session() {
        let mut app = super::super::setup_app_for_test();
        app.settings.show_toolbar_search_everything = true;
        let ctx = egui::Context::default();
        crate::global_search_ui::adopt_everything_for_test(
            &mut app,
            &ctx,
            "accepted Everything",
            crate::global_search_ui::everything_dataset_for_test(Vec::new()),
        );
        let child = app.tmp.path().join("child");
        if let SearchAccepted::Everything { position, .. } = &mut app.global_search.accepted {
            *position = crate::global_search_ui::EverythingPosition::PhysicalChild {
                logical_source: child.clone(),
                return_anchor: child,
                route: Default::default(),
            };
        }
        app.items_are_global_search_view = false;
        app.ensure_search_session(Kind::Metadata);
        app.handle_pinned_search_actions(&ctx, vec![Action::Clear(Kind::Metadata)]);
        assert!(
            app.edit_owner(Kind::Everything),
            "closing an unsent editor preserves the retained child session"
        );
        assert_eq!(app.applied_owner(), None);
        assert_eq!(app.closable_owner(), Some(Kind::Everything));
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
        app.route_search_escape(&ctx, None, &mut EscapeClaim::default());
        assert!(!app.global_search.active);
        assert_eq!(app.global_search.everything_draft, "accepted Everything");
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
    }
}

impl App {
    pub(crate) fn search_draft(&self, kind: Kind) -> &str {
        match kind {
            Kind::Local => &self.search_query,
            Kind::Container => &self.favsearch.query,
            Kind::Metadata => &self.global_search.metadata_draft,
            Kind::Everything => &self.global_search.everything_draft,
        }
    }
    pub(crate) fn search_draft_mut(&mut self, kind: Kind) -> &mut String {
        match kind {
            Kind::Local => &mut self.search_query,
            Kind::Container => &mut self.favsearch.query,
            Kind::Metadata => &mut self.global_search.metadata_draft,
            Kind::Everything => &mut self.global_search.everything_draft,
        }
    }
    pub(crate) fn clear_search_draft_on_end(&mut self, kind: Kind) {
        if !kind.pinned(&self.settings) {
            self.search_draft_mut(kind).clear();
        }
    }
    pub(crate) fn edit_owner(&self, kind: Kind) -> bool {
        match kind {
            Kind::Local => self.show_search_bar,
            Kind::Container => self.favsearch.active,
            Kind::Metadata | Kind::Everything => {
                self.global_search.active && Kind::from(self.global_search.source) == kind
            }
        }
    }
    pub(crate) fn applied_owner(&self) -> Option<Kind> {
        if self.items_are_global_search_view && self.global_search.active {
            return self.retained_search_owner();
        }
        if self.search_filter.is_some() {
            return Some(Kind::Local);
        }
        if self.favsearch.active
            && self.favsearch.on_results_grid()
            && self.current_folder.as_ref().is_some_and(|folder| {
                crate::folder_tree::path_eq(folder, &super::search_results_synthetic_path())
            })
        {
            return Some(Kind::Container);
        }
        None
    }
    fn retained_search_owner(&self) -> Option<Kind> {
        if !self.global_search.active {
            return None;
        }
        match &self.global_search.accepted {
            SearchAccepted::Metadata { applied: Some(_) } => Some(Kind::Metadata),
            SearchAccepted::Metadata { applied: None } => None,
            SearchAccepted::Everything { .. } => Some(Kind::Everything),
        }
    }
    pub(crate) fn closable_owner(&self) -> Option<Kind> {
        self.applied_owner()
            .or_else(|| Kind::ALL.into_iter().find(|kind| self.edit_owner(*kind)))
    }
    pub(crate) fn pinned_search_availability(&self, kind: Kind) -> Result<(), &'static str> {
        if kind == Kind::Everything {
            self.everything_search_entry_availability()
                .reason_text()
                .map_or(Ok(()), Err)?;
        }
        if self.viewer_session_blocks_main_window() {
            return Err("閲覧中は検索できません");
        }
        if self.common_modal_dialog_open() || self.any_dialog_open() {
            return Err("ほかの操作が完了してから検索してください");
        }
        if kind == Kind::Local && self.grid_is_pdf_pages() {
            return Err("PDF のページ表示中は現在地を絞り込めません");
        }
        Ok(())
    }
    pub(crate) fn ensure_search_session(&mut self, kind: Kind) {
        if self.edit_owner(kind) {
            return;
        }
        match kind {
            Kind::Local => self.open_local_metadata_search(),
            Kind::Container => self.open_favsearch(),
            Kind::Metadata => self.open_global_search(),
            Kind::Everything => self.open_everything_search(),
        }
    }
    pub(crate) fn request_pinned_search_focus(&mut self, ctx: &egui::Context, kind: Kind) {
        self.claim_pending_text_input_focus(
            ctx.viewport_id(),
            kind.input_id(),
            ctx.cumulative_pass_nr(),
        );
        match kind {
            Kind::Local => self.search_focus_request = true,
            Kind::Container => self.favsearch.focus_request = true,
            Kind::Metadata | Kind::Everything => self.global_search.focus_request = true,
        }
        ctx.request_repaint();
    }
    pub(crate) fn open_search_shortcut(&mut self, ctx: &egui::Context, kind: Kind) {
        if kind.pinned(&self.settings) {
            self.ensure_search_session(kind);
            self.request_pinned_search_focus(ctx, kind);
        } else {
            match kind {
                Kind::Local => self.open_local_metadata_search(),
                Kind::Container => self.open_favsearch(),
                Kind::Metadata => self.toggle_global_search(),
                Kind::Everything => self.open_everything_search(),
            }
        }
    }
    pub(crate) fn pinned_search_state(&self, kind: Kind) -> State {
        let running = match kind {
            Kind::Local => self.search_pending.is_some(),
            Kind::Container => self.favsearch_pending.is_some(),
            Kind::Metadata => {
                self.global_search.source == SearchSource::Metadata
                    && self.global_search.is_searching()
            }
            Kind::Everything => self.everything_search_progress_visible(),
        };
        if running {
            return State::Running;
        }
        if self.applied_owner() != Some(kind) {
            return State::Inactive;
        }
        let same = match kind {
            Kind::Local => self.search_filter.as_ref().is_some_and(|filter| {
                filter.submitted.query == self.search_query
                    && filter.submitted.target == self.search_target
                    && filter.submitted.or_mode == self.search_or_mode
            }),
            Kind::Container => self.favsearch.query == self.favsearch.last_executed,
            Kind::Metadata => {
                matches!(&self.global_search.accepted, SearchAccepted::Metadata { applied: Some(applied) } if applied.query == self.global_search.metadata_draft && applied.filters == self.global_search.filters)
            }
            Kind::Everything => self.accepted_everything_query_matches_draft(),
        };
        if same { State::Applied } else { State::Edited }
    }
    pub(crate) fn handle_pinned_search_actions(
        &mut self,
        ctx: &egui::Context,
        actions: Vec<Action>,
    ) {
        for action in actions {
            let kind = match action {
                Action::Edited(kind)
                | Action::Submit(kind)
                | Action::Clear(kind)
                | Action::Focused(kind) => kind,
            };
            if matches!(action, Action::Focused(_)) {
                continue;
            }
            if self.pinned_search_availability(kind).is_err() {
                continue;
            }
            match action {
                Action::Clear(_) => {
                    self.end_search_input(ctx, kind);
                    self.search_draft_mut(kind).clear();
                }
                Action::Edited(_) if kind == Kind::Everything => {}
                Action::Edited(_) | Action::Submit(_) => {
                    self.ensure_search_session(kind);
                    // An edit already owns focus. Entry latches must not refocus it on redraw.
                    match kind {
                        Kind::Local => self.search_focus_request = false,
                        Kind::Container => self.favsearch.focus_request = false,
                        _ => self.global_search.focus_request = false,
                    }
                    match kind {
                        Kind::Local if matches!(action, Action::Submit(_)) => {
                            self.execute_search(ctx)
                        }
                        Kind::Local => {}
                        Kind::Container => {
                            if self.favsearch.query != self.favsearch.last_executed {
                                self.execute_favsearch();
                            }
                        }
                        Kind::Metadata => self.reset_global_search_for_query_change(ctx),
                        Kind::Everything => self.send_everything_search(ctx),
                    }
                }
                Action::Focused(_) => {}
            }
        }
    }
    pub(crate) fn close_local_search(&mut self, ctx: &egui::Context) {
        self.cancel_pending_folder_nav();
        self.show_search_bar = false;
        self.clear_search_draft_on_end(Kind::Local);
        self.search_filter = None;
        self.search_filter_origin_folder = None;
        self.search_has_focus = false;
        self.search_focus_request = false;
        self.search_tag_bridge.clear();
        self.move_grid_cursor(self.selected, super::GridCursorIntent::ExplicitKeep);
        self.cancel_search_pending();
        self.rebuild_visible_indices();
        self.refresh_color_filter_for_scope_change(ctx);
    }
    /// (a) visible results; (b) edit session, returning to retained accepted results;
    /// (c) no owned session. Draft clearing is exclusively the explicit Clear action.
    fn end_search_input(&mut self, ctx: &egui::Context, kind: Kind) {
        if self.applied_owner() == Some(kind) {
            self.close_owned_search(ctx, kind);
        } else if self.edit_owner(kind) {
            if let Some(accepted) = self
                .retained_search_owner()
                .filter(|owner| *owner != kind)
                .filter(|owner| matches!(owner, Kind::Metadata | Kind::Everything))
            {
                self.switch_search_source(match accepted {
                    Kind::Metadata => SearchSource::Metadata,
                    _ => SearchSource::Everything,
                });
                self.global_search.last_executed =
                    self.global_search.accepted.applied_query().to_owned();
                self.global_search.focus_request = false;
            } else if let Some(owner) = self.closable_owner() {
                self.close_owned_search(ctx, owner);
            }
        }
    }
    fn close_owned_search(&mut self, ctx: &egui::Context, kind: Kind) {
        match kind {
            Kind::Local => self.close_local_search(ctx),
            Kind::Container => self.close_favsearch(),
            Kind::Metadata | Kind::Everything => self.close_global_search(),
        }
    }
    pub(crate) fn capture_search_escape_input(&self, ctx: &egui::Context) -> Option<SearchInputId> {
        let widget = crate::ime_focus::focused_text_input(ctx)
            .or_else(|| crate::ime_focus::recovering_text_input(ctx))?;
        // Recovery supplies the viewport/pass validity and pointer/other-widget precedence.
        // The previous record only maps that valid widget ID to a search kind.
        ctx.data(|data| {
            data.get_temp::<SearchInputId>(ctx.viewport_id().0.with("search_input_owner"))
        })
        .filter(|owner| owner.id() == widget)
    }
    pub(crate) fn record_search_input_focus(&self, ctx: &egui::Context) {
        let widget = crate::ime_focus::focused_text_input(ctx);
        let owner = Kind::ALL
            .into_iter()
            .flat_map(|kind| {
                [
                    SearchInputId::Pinned(kind),
                    SearchInputId::TemporaryBar(kind),
                ]
            })
            .find(|owner| Some(owner.id()) == widget);
        let id = ctx.viewport_id().0.with("search_input_owner");
        ctx.data_mut(|data| {
            if let Some(owner) = owner {
                data.insert_temp(id, owner);
            } else {
                data.remove::<SearchInputId>(id);
            }
        });
    }
    pub(crate) fn route_search_escape(
        &mut self,
        ctx: &egui::Context,
        input_owner: Option<SearchInputId>,
        claim: &mut EscapeClaim,
    ) {
        if !self.dialog_escape_pressed(ctx) {
            return;
        }
        if claim.0 {
            claim.claim(ctx);
            return;
        }
        if self.everything_search_progress_visible() {
            self.cancel_everything_search();
            claim.claim(ctx);
            return;
        }
        if self.common_modal_dialog_open() || self.any_dialog_open() {
            return;
        }
        let owner = input_owner
            .map(SearchInputId::kind)
            .or_else(|| self.closable_owner());
        if let Some(kind) = owner {
            if matches!(input_owner, Some(SearchInputId::TemporaryBar(_)))
                || (input_owner.is_none() && !kind.pinned(&self.settings))
            {
                // Temporary bars retain their original ×/Esc close transaction.
                if self.edit_owner(kind) || self.applied_owner() == Some(kind) {
                    self.close_owned_search(ctx, kind);
                }
            } else {
                self.end_search_input(ctx, kind);
            }
            if let Some(input) = input_owner {
                ctx.memory_mut(|memory| memory.surrender_focus(input.id()));
            }
            claim.claim(ctx);
        }
    }
    pub(crate) fn prepare_pinned_search_frame(&mut self, ctx: &egui::Context) {
        if let Some(claim) = self.pending_text_input_focus.get() {
            if let Some(kind) = Kind::ALL
                .into_iter()
                .find(|kind| claim.widget_id == kind.input_id() && !kind.pinned(&self.settings))
            {
                self.clear_pending_text_input_focus();
                match kind {
                    Kind::Local => self.search_focus_request = false,
                    Kind::Container => self.favsearch.focus_request = false,
                    _ if Kind::from(self.global_search.source) == kind => {
                        self.global_search.focus_request = false
                    }
                    _ => {}
                }
            }
        }
        if !self.show_search_bar || Kind::Local.pinned(&self.settings) {
            self.search_has_focus = false;
        }
        if !self.favsearch.active || Kind::Container.pinned(&self.settings) {
            self.favsearch.has_focus = false;
        }
        if !self.global_search.active
            || Kind::Metadata.pinned(&self.settings)
            || Kind::Everything.pinned(&self.settings)
        {
            self.global_search.has_focus = false;
        }
        let _ = ctx;
    }
    pub(crate) fn draw_pinned_search_section(
        &mut self,
        ui: &mut egui::Ui,
        kind: Kind,
    ) -> egui::InnerResponse<crate::ui_pinned_search::SearchFieldResponse> {
        let state = self.pinned_search_state(kind);
        let enabled = self.pinned_search_availability(kind);
        let focus = match kind {
            Kind::Local => std::mem::take(&mut self.search_focus_request) && self.show_search_bar,
            Kind::Container => {
                std::mem::take(&mut self.favsearch.focus_request) && self.favsearch.active
            }
            _ if self.edit_owner(kind) => std::mem::take(&mut self.global_search.focus_request),
            _ if !self.global_search.active => {
                self.global_search.focus_request = false;
                false
            }
            _ => false,
        };
        let enter = self.dialog_enter_pressed(ui.ctx());
        let drag = self.settings.toolbar_section_drag_enabled;
        let response = crate::ui_pinned_search::draw_pinned_search(
            ui,
            kind,
            self.search_draft_mut(kind),
            state,
            enabled,
            focus,
            enter,
            drag,
        );
        let focused = response.inner.input.has_focus();
        match kind {
            Kind::Local => self.search_has_focus = focused,
            Kind::Container => self.favsearch.has_focus = focused,
            _ => self.global_search.has_focus |= focused,
        }
        response
    }
}
