//! Stage A: actual search renderers and Escape routing, without product changes.
use super::*;
use crate::ui_pinned_search::PinnedSearchKind as Kind;
use egui::{Event, Key, Modifiers, PointerButton, Pos2};

struct SearchEscapeFixture {
    env: crate::app::AppTestEnvForTest,
    ctx: egui::Context,
    pinned: bool,
    time: f64,
}

impl SearchEscapeFixture {
    fn new(pinned: bool) -> Self {
        let mut env = crate::app::setup_app_for_test();
        env.settings.show_toolbar_search_metadata = pinned;
        env.settings.show_toolbar_search_everything = false;
        env.settings.show_toolbar_search_container = false;
        env.settings.show_toolbar_search_local = false;
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        crate::ui_fonts::configure_fonts(&ctx);
        let mut fixture = Self {
            env,
            ctx,
            pinned,
            time: 0.0,
        };
        // Exercise the Ctrl+G production entry, rather than assigning active/focus.
        fixture.frame(vec![key(Key::G, true, Modifiers::CTRL)]);
        assert!(fixture.env.global_search.active, "Ctrl+G must open search");
        fixture.frame(vec![key(Key::G, false, Modifiers::CTRL)]);
        fixture.env.global_search.metadata_draft = "keep this draft".into();
        fixture.frame(vec![]);
        assert!(fixture.ctx.memory(|memory| memory.has_focus(fixture.id())));
        fixture
    }

    fn id(&self) -> egui::Id {
        if self.pinned {
            Kind::Metadata.input_id()
        } else {
            Kind::Metadata.temporary_id()
        }
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.time += 0.05;
        let output = self.ctx.run_ui(
            egui::RawInput {
                time: Some(self.time),
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(1100.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |root| {
                let ctx = root.ctx().clone();
                let app = &mut self.env.app;
                app.update_ime_state(&ctx);
                app.keyboard_owner_for_pass(&ctx);
                // Match the production ordering around render_main_bars.
                let mut claim = pinned_search::EscapeClaim::for_frame(app);
                let history_taken = crate::ui_history_field::take_escape(
                    &ctx,
                    app.any_dialog_open() || app.common_modal_dialog_open(),
                );
                if history_taken {
                    claim.claim(&ctx);
                }
                let owner = (!history_taken)
                    .then(|| app.capture_search_escape_input(&ctx))
                    .flatten();
                if self.pinned {
                    app.prepare_pinned_search_frame(&ctx);
                    egui::CentralPanel::default().show(root, |ui| {
                        let output = app.draw_pinned_search_section(ui, Kind::Metadata);
                        app.handle_pinned_search_actions(&ctx, output.inner.actions);
                    });
                }
                app.input_main_grid_shortcuts(&ctx, false);
                if !self.pinned {
                    app.render_global_search_bar(root);
                }
                app.route_search_escape(&ctx, owner, &mut claim);
                app.settle_search_input_departure(&ctx, history_taken);
                app.record_search_input_focus(&ctx);
            },
        );
        output.drop_without_applying_deltas();
    }

    fn menu_open(&self) -> bool {
        self.ctx
            .read_response(self.id())
            .is_some_and(|response| response.context_menu_opened())
    }

    fn open_menu(&mut self) {
        let pos = self.ctx.read_response(self.id()).unwrap().rect.center();
        self.frame(vec![Event::PointerMoved(pos)]);
        self.frame(vec![pointer(pos, true)]);
        self.frame(vec![pointer(pos, false)]);
        self.frame(vec![]);
        assert!(self.menu_open(), "actual field right-click must open menu");
        assert!(self.env.global_search.active);
        assert_eq!(self.env.global_search.metadata_draft, "keep this draft");
    }

    fn escape(&mut self) {
        self.frame(vec![key(Key::Escape, true, Modifiers::NONE)]);
    }
}

fn key(key: Key, pressed: bool, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    }
}

fn pointer(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Secondary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn section1768_ctrl_g_context_menu_escape_keeps_temporary_search() {
    let _input = crate::key_input::lock_test_input();
    let mut fixture = SearchEscapeFixture::new(false);
    fixture.open_menu();
    fixture.escape();
    assert!(
        !fixture.menu_open(),
        "one Escape must dismiss the edit menu"
    );
    assert!(
        fixture.env.global_search.active,
        "the same Escape must not also close the Ctrl+G search"
    );
    assert_eq!(fixture.env.global_search.metadata_draft, "keep this draft");
}

#[test]
fn section1768_ctrl_g_context_menu_escape_keeps_pinned_draft() {
    let _input = crate::key_input::lock_test_input();
    let mut fixture = SearchEscapeFixture::new(true);
    fixture.open_menu();
    fixture.escape();
    assert!(
        !fixture.menu_open(),
        "one Escape must dismiss the edit menu"
    );
    assert!(fixture.env.global_search.active);
    assert_eq!(
        fixture.env.global_search.metadata_draft, "keep this draft",
        "the same Escape must not also cancel the pinned search draft"
    );
}

#[test]
fn section1768_ctrl_g_plain_escape_still_closes_temporary_search() {
    let _input = crate::key_input::lock_test_input();
    let mut fixture = SearchEscapeFixture::new(false);
    assert!(!fixture.menu_open());
    fixture.escape();
    assert!(!fixture.env.global_search.active);
}

#[test]
fn section1768_ctrl_g_history_escape_keeps_temporary_search() {
    let _input = crate::key_input::lock_test_input();
    let mut fixture = SearchEscapeFixture::new(false);
    fixture.frame(vec![key(Key::ArrowDown, true, Modifiers::NONE)]);
    assert!(crate::ui_history_field::is_open_for(
        &fixture.ctx,
        fixture.id()
    ));
    fixture.escape();
    assert!(!crate::ui_history_field::is_open_for(
        &fixture.ctx,
        fixture.id()
    ));
    assert!(fixture.env.global_search.active);
    assert_eq!(fixture.env.global_search.metadata_draft, "keep this draft");
}

#[test]
#[expect(
    deprecated,
    reason = "Exercise platform IME compatibility notifications"
)]
fn section1768_ctrl_g_composing_escape_keeps_temporary_search() {
    let _input = crate::key_input::lock_test_input();
    let mut fixture = SearchEscapeFixture::new(false);
    fixture.frame(vec![
        Event::Ime(egui::ImeEvent::Enabled),
        Event::Ime(egui::ImeEvent::Preedit {
            text: "検索".into(),
            active_range_chars: None,
        }),
    ]);
    assert!(crate::ime_focus::ime_input_active(&fixture.ctx));
    fixture.escape();
    assert!(fixture.env.global_search.active);
}
