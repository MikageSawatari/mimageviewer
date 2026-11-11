//! Search confirmation must keep ownership of its Enter across egui layout replay.
use super::*;
use crate::{
    app::{GridItem, ViewerPresentation},
    keymap::KeyAction,
};

fn enter_input() -> egui::RawInput {
    egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: Some(egui::Key::Enter),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    }
}

#[cfg(windows)]
fn stage_native_enter() {
    crate::key_input::set_test_frame(vec![crate::key_input::KeyEdge {
        operation_id: crate::key_input::KeyOperationId::unassigned(),
        source_hwnd: 0,
        source_viewport: egui::ViewportId::ROOT,
        virtual_key: 0x0D,
        scan_code: 0x1C,
        extended: false,
        pressed: true,
        repeat: false,
        ctrl: false,
        shift: false,
        alt: false,
    }]);
}

fn render_search(app: &mut App, ctx: &egui::Context, kind: Option<Kind>, pinned: bool) {
    ctx.root_ui(|root| {
        if pinned {
            egui::CentralPanel::default().show(root, |ui| {
                let response = app.draw_pinned_search_section(ui, kind.unwrap());
                app.handle_pinned_search_actions(ctx, response.inner.actions);
            });
        } else {
            match kind {
                Some(Kind::Container) => app.render_favsearch_bar(root),
                Some(Kind::Local) => app.render_search_bar(root),
                Some(Kind::Metadata | Kind::Everything) => app.render_global_search_bar(root),
                None => app.render_tag_view_bar(root),
                _ => unreachable!(),
            }
        }
    });
}

fn replayed_enter(kind: Option<Kind>, pinned: bool) {
    replayed_enter_batch(kind, pinned, false);
}

fn replayed_enter_batch(kind: Option<Kind>, pinned: bool, with_text: bool) {
    let _input = crate::key_input::lock_test_input();
    let mut app = super::super::setup_app_for_test();
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    crate::ui_fonts::configure_fonts(&ctx);
    if let Some(kind) = kind {
        match kind {
            Kind::Container => app.settings.show_toolbar_search_container = pinned,
            Kind::Local => app.settings.show_toolbar_search_local = pinned,
            Kind::Metadata => app.settings.show_toolbar_search_metadata = pinned,
            Kind::Everything => app.settings.show_toolbar_search_everything = pinned,
        }
        app.ensure_search_session(kind);
    } else {
        app.tag_view.active = true;
        app.tag_view.focus_request = true;
    }
    // A selected result makes a leaked GridOpenSelected observable without I/O.
    app.items = vec![GridItem::Folder(app.tmp.path().join("result"))];
    app.visible_indices = vec![0];
    app.selected = Some(0);
    let output = ctx.run_pass(egui::RawInput::default(), |ctx| {
        app.update_ime_state(ctx);
        render_search(&mut app, ctx, kind, pinned);
    });
    let _ = crate::test_ui::headless_paint_output(output);
    assert!(ctx.memory(|memory| memory.focused()).is_some());
    let mut raw = egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: Some(egui::Key::Enter),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    };
    if with_text {
        raw.events.insert(
            0,
            egui::Event::Key {
                key: egui::Key::R,
                physical_key: Some(egui::Key::R),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        );
        raw.events.insert(1, egui::Event::Text("r".into()));
    }
    #[cfg(windows)]
    {
        stage_native_enter();
        if with_text {
            stage_native_text_and_enter();
        }
    }
    let mut observed = Vec::new();
    let mut rotations = Vec::new();
    let output = ctx.run_pass(raw.clone(), |ctx| {
        app.update_ime_state(ctx);
        app.keyboard_owner_for_pass(ctx);
        observed.push(app.keymap.pressed_action(ctx, KeyAction::GridOpenSelected));
        rotations.push(app.keymap.pressed_action(ctx, KeyAction::GridRotateCw));
        render_search(&mut app, ctx, kind, pinned);
        app.record_search_input_focus(ctx);
        if ctx.current_pass_index() == 0 {
            ctx.request_discard("exercise search Enter ownership on layout replay");
        }
    });
    let _ = crate::test_ui::headless_paint_output(output);
    crate::key_input::clear_test_frame();
    assert!(
        observed.len() >= 2,
        "the regression must exercise layout replay"
    );
    assert!(
        observed.iter().all(|opened| !opened),
        "search Enter must not become GridOpenSelected: {kind:?}, pinned={pinned}, {observed:?}"
    );
    assert!(
        rotations.iter().all(|rotated| !rotated),
        "text-input keys must not become grid rotation: {rotations:?}"
    );
    assert!(ctx.memory(|memory| memory.focused()).is_none());
    if kind == Some(Kind::Everything) {
        assert_eq!(
            app.global_search.reject_message.as_deref(),
            Some(crate::everything_ipc::EverythingError::EmptyQuery.user_message())
        );
        assert!(app.global_search.pending.is_none());
        assert!(!app.everything_search_progress_visible());
    }

    // A later, distinct Enter remains the normal grid-open command.
    let mut release = enter_input();
    if let egui::Event::Key { pressed, .. } = &mut release.events[0] {
        *pressed = false;
    }
    if with_text {
        release.events.push(egui::Event::Key {
            key: egui::Key::R,
            physical_key: Some(egui::Key::R),
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        assert_eq!(app.favsearch.query, "r");
    }
    let output = ctx.run_pass(release, |ctx| {
        app.update_ime_state(ctx);
        render_search(&mut app, ctx, kind, pinned);
    });
    let _ = crate::test_ui::headless_paint_output(output);
    ctx.begin_pass(enter_input());
    app.update_ime_state(&ctx);
    app.keyboard_owner_for_pass(&ctx);
    assert!(app.keymap.pressed_action(&ctx, KeyAction::GridOpenSelected));
    let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
    if with_text {
        let mut next = enter_input();
        if let egui::Event::Key {
            key, physical_key, ..
        } = &mut next.events[0]
        {
            *key = egui::Key::R;
            *physical_key = Some(egui::Key::R);
        }
        ctx.begin_pass(next);
        app.update_ime_state(&ctx);
        app.keyboard_owner_for_pass(&ctx);
        assert!(app.keymap.pressed_action(&ctx, KeyAction::GridRotateCw));
        ctx.end_pass().drop_without_applying_deltas();
    }
}

#[cfg(windows)]
fn stage_native_text_and_enter() {
    let mut edges = Vec::new();
    for (virtual_key, scan_code) in [(0x52, 0x13), (0x0D, 0x1C)] {
        edges.push(crate::key_input::KeyEdge {
            operation_id: crate::key_input::KeyOperationId::unassigned(),
            source_hwnd: 0,
            source_viewport: egui::ViewportId::ROOT,
            virtual_key,
            scan_code,
            extended: false,
            pressed: true,
            repeat: false,
            ctrl: false,
            shift: false,
            alt: false,
        });
    }
    crate::key_input::set_test_frame(edges);
}

#[test]
fn section1708_character_and_enter_batch_does_not_rotate_on_layout_replay() {
    replayed_enter_batch(Some(Kind::Container), false, true);
}

#[test]
fn section1708_container_enter_replay_stays_in_results() {
    replayed_enter(Some(Kind::Container), false);
}

#[test]
fn section1708_pinned_container_enter_replay_stays_in_results() {
    replayed_enter(Some(Kind::Container), true);
}

#[test]
fn section1708_metadata_enter_replay_stays_in_results() {
    replayed_enter(Some(Kind::Metadata), false);
}

#[test]
fn section1708_tag_enter_replay_stays_in_results() {
    replayed_enter(None, false);
}

#[test]
fn section1708_local_enter_replay_stays_in_results() {
    replayed_enter(Some(Kind::Local), false);
}

#[test]
fn section1708_pinned_local_enter_replay_stays_in_results() {
    replayed_enter(Some(Kind::Local), true);
}

#[test]
fn section1708_pinned_metadata_enter_replay_stays_in_results() {
    replayed_enter(Some(Kind::Metadata), true);
}

#[test]
fn section1708_everything_rejected_enter_replay_stays_in_results() {
    replayed_enter(Some(Kind::Everything), false);
}

#[test]
fn section1708_pinned_everything_rejected_enter_replay_stays_in_results() {
    replayed_enter(Some(Kind::Everything), true);
}

#[test]
fn section1708_everything_submit_claims_enter_before_validation_rejection() {
    for (draft, error) in [
        ("", crate::everything_ipc::EverythingError::EmptyQuery),
        (
            "bad\0query",
            crate::everything_ipc::EverythingError::EmbeddedNul,
        ),
    ] {
        let _input = crate::key_input::lock_test_input();
        let mut app = super::super::setup_app_for_test();
        app.global_search.everything_draft = draft.into();
        let ctx = egui::Context::default();
        ctx.begin_pass(enter_input());
        #[cfg(windows)]
        stage_native_enter();
        assert!(app.submit_search_input(&ctx, Kind::Everything, Kind::Everything.temporary_id()));
        assert_eq!(
            app.global_search.reject_message.as_deref(),
            Some(error.user_message())
        );
        assert!(app.global_search.pending.is_none());
        assert!(!app.everything_search_progress_visible());
        assert!(!app.dialog_enter_pressed(&ctx));
        #[cfg(windows)]
        assert!(!crate::key_input::pressed_key_down(
            ctx.viewport_id(),
            |edge| edge.virtual_key == 0x0D
        ));
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
    }
}

#[test]
fn section1708_pinned_container_same_query_submit_returns_to_result_root() {
    let mut app = super::super::setup_app_for_test();
    app.settings.show_toolbar_search_container = true;
    app.ensure_search_session(Kind::Container);
    // Empty query avoids a database worker; submission still owns the root transition.
    app.favsearch.nav_stack = vec![app.tmp.path().join("opened-result.zip")];
    app.handle_pinned_search_actions(
        &egui::Context::default(),
        vec![Action::Submit(Kind::Container)],
    );
    assert!(app.favsearch.nav_stack.is_empty());
    assert!(app.favsearch.active);
    assert_eq!(
        app.current_folder.as_ref(),
        Some(&crate::app::search_results_synthetic_path())
    );
    assert!(!app.favsearch.focus_request);
    assert!(!app.favsearch.has_focus);
}

#[test]
fn section1708_pending_search_enter_is_submitted_before_the_grid_handler() {
    for (kind, pinned) in [
        (Some(Kind::Container), false),
        (Some(Kind::Container), true),
        (Some(Kind::Local), false),
        (Some(Kind::Metadata), false),
        (None, false),
    ] {
        let _input = crate::key_input::lock_test_input();
        let mut app = super::super::setup_app_for_test();
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        crate::ui_fonts::configure_fonts(&ctx);
        if let Some(kind) = kind {
            app.settings.show_toolbar_search_container = pinned;
            app.ensure_search_session(kind);
            if kind == Kind::Container {
                app.favsearch.nav_stack = vec![app.tmp.path().join("opened-result")];
            }
        } else {
            app.tag_view.active = true;
            app.tag_view.focus_request = true;
        }
        let result = app.tmp.path().join("result");
        std::fs::create_dir(&result).unwrap();
        app.items = vec![GridItem::Folder(result)];
        app.visible_indices = vec![0];
        app.selected = Some(0);
        ctx.begin_pass(enter_input());
        app.update_ime_state(&ctx);
        if pinned {
            app.request_pinned_search_focus(&ctx, kind.unwrap());
        }
        #[cfg(windows)]
        stage_native_text_and_enter();
        assert!(
            app.handle_keyboard(&ctx).is_none(),
            "the pending search must own Enter before grid navigation: {kind:?}, pinned={pinned}"
        );
        assert!(!app.dialog_enter_pressed(&ctx));
        #[cfg(windows)]
        assert!(!crate::key_input::pressed_key_down(
            ctx.viewport_id(),
            |edge| edge.virtual_key == 0x52
        ));
        #[cfg(windows)]
        assert!(!crate::key_input::pressed_key_down(
            ctx.viewport_id(),
            |edge| { edge.virtual_key == 0x0D }
        ));
        assert!(app.pending_text_input_focus.get().is_none());
        if kind == Some(Kind::Container) {
            assert!(app.favsearch.nav_stack.is_empty());
        }
        render_search(&mut app, &ctx, kind, pinned);
        assert!(ctx.memory(|memory| memory.focused()).is_none());
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
    }
}

#[test]
fn section1708_pending_search_respects_existing_keyboard_owners() {
    use crate::keyboard_input::{KeyboardOwner, TextInputPhase, cache_keyboard_owner};
    for blocker in [
        "other-focus",
        "recovery",
        "modal",
        "ime",
        "other-pending",
        "inactive",
    ] {
        let _input = crate::key_input::lock_test_input();
        let mut app = super::super::setup_app_for_test();
        app.ensure_search_session(Kind::Container);
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        let mut raw = enter_input();
        if blocker == "ime" {
            raw.events.insert(
                0,
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "検索".into(),
                    active_range_chars: None,
                }),
            );
        }
        ctx.begin_pass(raw);
        app.update_ime_state(&ctx);
        let other = egui::Id::unique("other-input-owner");
        match blocker {
            "other-focus" => ctx.memory_mut(|memory| memory.request_focus(other)),
            "recovery" => cache_keyboard_owner(
                &ctx,
                KeyboardOwner::TextInput {
                    viewport: ctx.viewport_id(),
                    widget_id: other,
                    phase: TextInputPhase::FocusRecovery,
                },
            ),
            "modal" => app.show_stats_dialog = true,
            "other-pending" => app.claim_pending_text_input_focus(
                ctx.viewport_id(),
                other,
                ctx.cumulative_pass_nr(),
            ),
            "inactive" => app.favsearch.active = false,
            _ => {}
        }
        assert!(!app.route_pending_search_submit(&ctx), "{blocker}");
        assert!(
            ctx.input(|input| input.key_pressed(egui::Key::Enter)),
            "{blocker}"
        );
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
    }
}

#[test]
fn section1708_tag_submit_preserves_modal_enter_after_losing_field_focus() {
    let _input = crate::key_input::lock_test_input();
    let mut app = super::super::setup_app_for_test();
    app.tag_view.active = true;
    app.tag_view.focus_request = true;
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    crate::ui_fonts::configure_fonts(&ctx);
    ctx.begin_pass(Default::default());
    render_search(&mut app, &ctx, None, false);
    assert!(ctx.memory(|memory| memory.has_focus(App::tag_search_input_id())));
    let _ = crate::test_ui::headless_paint_output(ctx.end_pass());

    app.show_stats_dialog = true;
    ctx.begin_pass(enter_input());
    app.update_ime_state(&ctx);
    app.keyboard_owner_for_pass(&ctx);
    render_search(&mut app, &ctx, None, false);
    assert!(ctx.input(|input| input.key_pressed(egui::Key::Enter)));
    assert!(!app.submit_tag_search_input(&ctx, App::tag_search_input_id()));
    // Closing the modal in this pass does not transfer its Enter to the old field.
    app.show_stats_dialog = false;
    assert!(!app.submit_tag_search_input(&ctx, App::tag_search_input_id()));
    assert!(ctx.input(|input| input.key_pressed(egui::Key::Enter)));
    let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
}

#[test]
fn section1708_stale_pinned_lost_focus_does_not_submit_another_fields_enter() {
    fn draw_pair(
        ctx: &egui::Context,
        drafts: &mut [String; 2],
        focus: Option<Kind>,
        enter: bool,
    ) -> (Vec<Action>, bool) {
        let mut actions = Vec::new();
        let mut local_lost_focus = false;
        ctx.root_ui(|root| {
            egui::CentralPanel::default().show(root, |ui| {
                for (index, kind) in [Kind::Local, Kind::Container].into_iter().enumerate() {
                    let response = crate::ui_pinned_search::draw_pinned_search(
                        ui,
                        kind,
                        &mut drafts[index],
                        State::Inactive,
                        Ok(()),
                        focus == Some(kind),
                        enter,
                        false,
                        None,
                    );
                    if kind == Kind::Local {
                        local_lost_focus = response.inner.input.lost_focus();
                    }
                    actions.extend(response.inner.actions);
                }
            });
        });
        (actions, local_lost_focus)
    }
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    crate::ui_fonts::configure_fonts(&ctx);
    let mut drafts = [String::new(), String::new()];
    for kind in [Kind::Local, Kind::Container] {
        ctx.begin_pass(Default::default());
        draw_pair(&ctx, &mut drafts, Some(kind), false);
        assert!(ctx.memory(|memory| memory.has_focus(kind.input_id())));
        let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
    }
    ctx.begin_pass(enter_input());
    let (actions, stale) = draw_pair(&ctx, &mut drafts, None, true);
    assert!(stale, "exercise egui's two-pass lost_focus history");
    let submits: Vec<_> = actions
        .into_iter()
        .filter_map(|action| match action {
            Action::Submit(kind) => Some(kind),
            _ => None,
        })
        .collect();
    assert_eq!(submits, [Kind::Container]);
    let _ = crate::test_ui::headless_paint_output(ctx.end_pass());
}

#[test]
#[cfg(windows)]
fn section1708_text_owner_closes_only_its_native_viewport_batch() {
    use crate::keyboard_input::{
        self, KeyboardOwner, ShortcutScope, ShortcutSurface, TextInputPhase,
    };
    let _input = crate::key_input::lock_test_input();
    for text_owner in [true, false] {
        let ctx = egui::Context::default();
        ctx.begin_pass(enter_input());
        let field = egui::Id::unique("batch-field");
        let sibling = egui::ViewportId::from_hash_of("batch-sibling");
        let edge = |viewport, virtual_key| crate::key_input::KeyEdge {
            operation_id: crate::key_input::KeyOperationId::unassigned(),
            source_hwnd: 0,
            source_viewport: viewport,
            virtual_key,
            scan_code: 0,
            extended: false,
            pressed: true,
            repeat: false,
            ctrl: false,
            shift: false,
            alt: false,
        };
        crate::key_input::set_test_routed_frame(vec![
            edge(ctx.viewport_id(), 0x52),
            edge(ctx.viewport_id(), 0x43),
            edge(ctx.viewport_id(), 0x0D),
            edge(sibling, 0x52),
        ]);
        keyboard_input::cache_keyboard_owner(
            &ctx,
            if text_owner {
                KeyboardOwner::TextInput {
                    viewport: ctx.viewport_id(),
                    widget_id: field,
                    phase: TextInputPhase::Focused,
                }
            } else {
                KeyboardOwner::ApplicationShortcut {
                    scope: ShortcutScope::new(ctx.viewport_id(), ShortcutSurface::Main),
                }
            },
        );
        // An explicit button in another search field still retires the viewport's old text delivery.
        keyboard_input::claim_text_input_submit(&ctx, field.with("submit-target"));
        assert_eq!(
            crate::key_input::pressed_key_down(ctx.viewport_id(), |edge| edge.virtual_key == 0x52),
            !text_owner
        );
        assert_eq!(
            crate::key_input::pressed_key_down(ctx.viewport_id(), |edge| edge.virtual_key == 0x43),
            !text_owner
        );
        assert!(!crate::key_input::pressed_key_down(
            ctx.viewport_id(),
            |edge| edge.virtual_key == 0x0D
        ));
        assert!(crate::key_input::pressed_key_down(sibling, |edge| edge
            .virtual_key
            == 0x52));
        crate::key_input::clear_test_frame();
        // A new backend frame's R remains a grid operation after the search ended.
        crate::key_input::set_test_frame(vec![edge(ctx.viewport_id(), 0x52)]);
        assert!(crate::key_input::pressed_key_down(
            ctx.viewport_id(),
            |edge| edge.virtual_key == 0x52
        ));
        crate::key_input::clear_test_frame();
        ctx.end_pass().drop_without_applying_deltas();
    }
}

#[test]
fn section1708_legacy_search_reservation_uses_main_shortcut_scope() {
    use crate::keyboard_input::{KeyboardOwner, ShortcutScope, ShortcutSurface, TextInputPhase};
    for presentation in [
        ViewerPresentation::MainWindow,
        ViewerPresentation::Fullscreen,
        ViewerPresentation::DetachedWindow,
    ] {
        for kind in [
            Some(Kind::Local),
            Some(Kind::Container),
            Some(Kind::Metadata),
            Some(Kind::Everything),
            None,
        ] {
            let _input = crate::key_input::lock_test_input();
            let mut app = super::super::setup_app_for_test();
            if let Some(kind) = kind {
                app.ensure_search_session(kind);
            } else {
                app.tag_view.active = true;
                app.tag_view.focus_request = true;
            }
            let widget = app
                .temporary_search_focus_reservation()
                .expect("legacy reservation");
            app.fullscreen_idx = Some(0);
            app.viewer_presentation = presentation;
            let detached = presentation == ViewerPresentation::DetachedWindow;
            let ctx = egui::Context::default();
            crate::ime_focus::install_ime_input_policy(&ctx);
            ctx.begin_pass(enter_input());
            app.update_ime_state(&ctx);
            #[cfg(windows)]
            stage_native_enter();
            let owner = app.keyboard_owner_for_pass(&ctx);
            if detached {
                // Existing detached sessions leave Main usable; fullscreen_idx alone
                // must not suppress its pending search field.
                assert_eq!(
                    owner,
                    KeyboardOwner::TextInput {
                        viewport: ctx.viewport_id(),
                        widget_id: widget,
                        phase: TextInputPhase::PendingFocus,
                    },
                    "{presentation:?}/{kind:?}"
                );
            } else {
                assert_eq!(
                    owner,
                    KeyboardOwner::ApplicationShortcut {
                        scope: ShortcutScope::new(ctx.viewport_id(), ShortcutSurface::Fullscreen),
                    },
                    "{presentation:?}/{kind:?}"
                );
                assert!(owner.fullscreen_raw_key_permit().is_some());
                assert!(!app.route_pending_search_submit(&ctx));
                assert!(app.dialog_enter_pressed(&ctx));
                #[cfg(windows)]
                assert!(crate::key_input::pressed_key_down(
                    ctx.viewport_id(),
                    |edge| edge.virtual_key == 0x0D
                ));
                assert_eq!(app.temporary_search_focus_reservation(), Some(widget));
            }
            assert_eq!(
                app.keyboard_shortcut_surface(egui::ViewportId::from_hash_of("search-sibling")),
                ShortcutSurface::Fullscreen
            );
            #[cfg(windows)]
            crate::key_input::clear_test_frame();
            ctx.end_pass().drop_without_applying_deltas();
        }
    }
}

#[test]
fn section1708_result_navigation_retires_only_its_search_focus_claim() {
    for kind in Kind::ALL {
        for claim_case in ["temporary", "pinned", "other-widget", "sibling-viewport"] {
            let _input = crate::key_input::lock_test_input();
            let mut app = super::super::setup_app_for_test();
            app.ensure_search_session(kind);
            *app.search_draft_mut(kind) = "keep this draft".into();
            let ctx = egui::Context::default();
            ctx.begin_pass(enter_input());
            #[cfg(windows)]
            stage_native_text_and_enter();
            let widget = match claim_case {
                "temporary" => kind.temporary_id(),
                "pinned" | "sibling-viewport" => kind.input_id(),
                _ => egui::Id::unique("unrelated-input"),
            };
            let viewport = if claim_case == "sibling-viewport" {
                egui::ViewportId::from_hash_of("search-navigation-sibling")
            } else {
                egui::ViewportId::ROOT
            };
            app.claim_pending_text_input_focus(viewport, widget, ctx.cumulative_pass_nr());
            let before = app.pending_text_input_focus.get();
            app.retire_search_input_focus(kind);
            assert!(app.edit_owner(kind));
            assert_eq!(app.search_draft(kind), "keep this draft");
            let request = match kind {
                Kind::Local => app.search_focus_request,
                Kind::Container => app.favsearch.focus_request,
                _ => app.global_search.focus_request,
            };
            assert!(!request, "{kind:?}/{claim_case}");
            assert_eq!(
                app.pending_text_input_focus.get(),
                if matches!(claim_case, "temporary" | "pinned") {
                    None
                } else {
                    before
                }
            );
            assert!(ctx.input(|input| input.key_pressed(egui::Key::Enter)));
            #[cfg(windows)]
            {
                assert!(crate::key_input::pressed_key_down(
                    ctx.viewport_id(),
                    |edge| edge.virtual_key == 0x52
                ));
                assert!(crate::key_input::pressed_key_down(
                    ctx.viewport_id(),
                    |edge| edge.virtual_key == 0x0D
                ));
                crate::key_input::clear_test_frame();
            }
            ctx.end_pass().drop_without_applying_deltas();
        }
    }
}

#[test]
fn section1708_result_navigation_keeps_the_new_shared_search_editor() {
    use crate::global_search_ui::SearchSource;
    for (old, new) in [
        (Kind::Metadata, Kind::Everything),
        (Kind::Everything, Kind::Metadata),
    ] {
        let mut app = super::super::setup_app_for_test();
        app.ensure_search_session(old);
        app.switch_search_source(if new == Kind::Everything {
            SearchSource::Everything
        } else {
            SearchSource::Metadata
        });
        assert!(app.global_search.focus_request);
        app.claim_pending_text_input_focus(egui::ViewportId::ROOT, new.input_id(), 0);
        let pending = app.pending_text_input_focus.get();
        app.retire_search_input_focus(old);
        assert!(app.global_search.focus_request);
        assert_eq!(app.pending_text_input_focus.get(), pending);
    }
}

#[test]
fn section1708_tag_navigation_preserves_foreign_focus_claim() {
    let _input = crate::key_input::lock_test_input();
    let mut app = super::super::setup_app_for_test();
    app.tag_view.active = true;
    app.tag_view.focus_request = true;
    app.tag_view.query = "keep tag".into();
    let other = egui::Id::unique("tag-unrelated-editor");
    app.claim_pending_text_input_focus(egui::ViewportId::ROOT, other, 0);
    let pending = app.pending_text_input_focus.get();
    app.retire_tag_search_input_focus();
    assert!(!app.tag_view.focus_request);
    assert!(app.tag_view.active);
    assert_eq!(app.tag_view.query, "keep tag");
    assert_eq!(app.pending_text_input_focus.get(), pending);
    app.claim_pending_text_input_focus(egui::ViewportId::ROOT, App::tag_search_input_id(), 0);
    app.retire_tag_search_input_focus();
    assert!(app.pending_text_input_focus.get().is_none());
}

#[cfg(windows)]
#[test]
fn section1708_result_viewing_in_sibling_context_keeps_main_search_reservation() {
    use crate::app::viewer_context_registry::BuildOutcome;
    let mut app = super::super::setup_app_for_test();
    let sibling = app
        .build_viewer_context("search-focus-sibling", |_, _| BuildOutcome::Commit)
        .unwrap();
    app.ensure_search_session(Kind::Metadata);
    app.claim_pending_text_input_focus(egui::ViewportId::ROOT, Kind::Metadata.temporary_id(), 0);
    let pending = app.pending_text_input_focus.get();
    app.with_viewer_context(sibling, |owner| {
        owner.global_search.active = true;
        owner.global_search.focus_request = true;
        owner.retire_result_search_input_focus();
        owner.retire_search_input_focus(Kind::Metadata);
        assert!(owner.global_search.focus_request);
        assert_eq!(owner.pending_text_input_focus.get(), pending);
    })
    .unwrap();
    assert!(app.global_search.focus_request);
    assert_eq!(app.pending_text_input_focus.get(), pending);
}

#[test]
fn section1708_existing_viewer_page_open_preserves_new_main_search_reservation() {
    let mut app = super::super::setup_app_for_test();
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.show_toolbar_search_metadata = true;
    let paths = [
        app.tmp.path().join("page0.png"),
        app.tmp.path().join("page1.png"),
    ];
    for path in &paths {
        image::RgbImage::new(2, 3).save(path).unwrap();
    }
    let ctx = egui::Context::default();
    app.adopt_c1_search_results_for_test(&ctx, &paths, false);
    app.finish_page_identity_preparation_for_test();
    app.open_fullscreen(0, super::super::HistoryTrigger::UserChosen);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert!(!app.global_search.focus_request);
    app.request_pinned_search_focus(&ctx, Kind::Metadata);
    let pending = app.pending_text_input_focus.get();
    app.open_fullscreen(1, super::super::HistoryTrigger::UserChosen);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.global_search.focus_request);
    assert_eq!(app.pending_text_input_focus.get(), pending);
}
