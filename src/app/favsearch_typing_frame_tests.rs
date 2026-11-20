//! Ctrl+S (container search) typing across actual main-frame phases.
//!
//! Investigation for the report that the Ctrl+S field cannot take a second
//! character. Each case types into the real editor, lets the search worker
//! and the item loader finish between keystrokes, and checks that the editor
//! keeps keyboard focus, its text and its IME composition.
#![cfg(windows)]

use super::*;
use crate::ui_pinned_search::PinnedSearchKind as Kind;

struct Fixture {
    app: AppTestEnvForTest,
    ctx: egui::Context,
    frame: u64,
    field: egui::Id,
    shortcut_leaks: Vec<KeyAction>,
}

fn fixture(pinned: bool) -> Fixture {
    let mut app = setup_app_for_test();
    if matches!(app.indexer_init, IndexerInit::NotStarted) {
        app.indexer_init = IndexerInit::Unavailable;
    }
    let root = app.tmp.path().join("fav-root");
    let origin = app.tmp.path().join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    let names = ["alpha", "alps", "beta", "かな本", "かきくけ"];
    let entries = names
        .iter()
        .map(|name| {
            let path = root.join(name);
            std::fs::create_dir_all(&path).unwrap();
            crate::search_index_db::IndexEntry {
                display_name: (*name).into(),
                path,
                kind: crate::search_index_db::IndexKind::Folder,
                mtime: 0,
            }
        })
        .collect::<Vec<_>>();
    let db =
        crate::search_index_db::SearchIndexDb::open_at(&app.tmp.path().join("index.db")).unwrap();
    db.upsert_children(&root, &root, &entries).unwrap();
    app.search_index_db = Some(std::sync::Arc::new(db));
    let mut favorite = crate::settings::FavoriteEntry::new("fav".into(), root);
    favorite.auto_index_structure = true;
    app.settings.favorites.push(favorite);
    app.current_folder = Some(origin);
    app.top_level_grid_view
        .replace_surface(top_level_grid_view::TopLevelGridSurface::Folder);
    app.settings.show_toolbar_search_metadata = false;
    app.settings.show_toolbar_search_everything = false;
    app.settings.show_toolbar_search_local = false;
    app.settings.show_toolbar_search_container = pinned;
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    crate::ui_fonts::configure_fonts(&ctx);
    ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
    let field = if pinned {
        Kind::Container.input_id()
    } else {
        Kind::Container.temporary_id()
    };
    let mut fixture = Fixture {
        app,
        ctx,
        frame: 0,
        field,
        shortcut_leaks: Vec::new(),
    };
    // Open as the Ctrl+S shortcut does.
    let ctx = fixture.ctx.clone();
    fixture.app.open_search_shortcut(&ctx, Kind::Container);
    fixture.run(vec![]);
    fixture.run(vec![]);
    fixture.run(vec![]);
    fixture
}

impl Fixture {
    fn run(&mut self, events: Vec<egui::Event>) {
        self.frame += 1;
        let app = &mut self.app;
        let ctx = &self.ctx;
        let shortcut_leaks = &mut self.shortcut_leaks;
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 700.0),
                )),
                time: Some(self.frame as f64 / 60.0),
                events,
                ..Default::default()
            },
            |ui| {
                app.input_update_begin(ctx);
                app.input_main_focus_resume(ctx);
                app.input_pointer_and_ime(ctx);
                app.render_search_and_metadata_results(ctx);
                app.render_other_worker_results(ctx);
                app.keyboard_owner_for_pass(ctx);
                for action in [KeyAction::GridRotateCcw, KeyAction::GridPin] {
                    if app.keymap.pressed_action(ctx, action) {
                        shortcut_leaks.push(action);
                    }
                }
                let (keyboard_nav, keyboard_nav_from_fullscreen_close, _) =
                    app.input_root_keyboard(ctx);
                let (toolbar_fav_nav, address_nav) = app.render_toolbar(ui);
                app.input_main_grid_shortcuts(ctx, false);
                let (folder_pane_nav, _) = app.render_main_bars(ui, &mut None);
                app.record_search_input_focus(ctx);
                let grid_nav = app.render_grid(ui);
                app.handle_delete_key(ctx);
                app.post_render_main_navigation(
                    ctx,
                    update_phases::MainFrameNavigation {
                        fav_nav: None,
                        toolbar_fav_nav,
                        fullscreen_close_nav: None,
                        keyboard_nav,
                        keyboard_nav_from_fullscreen_close,
                        gamepad_nav: None,
                        address_nav,
                        open_folder_nav: None,
                        context_nav: None,
                        folder_pane_nav,
                        grid_nav,
                    },
                );
                app.post_render_sidecar_restore(ctx);
            },
        )
        .drop_without_applying_deltas();
        eprintln!(
            "frame {:>3}: query={:?} last_executed={:?} pending={} focused={:?} field_focus={} has_focus={} active={} items={} results={} hydrating={} surface={:?}",
            self.frame,
            self.app.favsearch.query,
            self.app.favsearch.last_executed,
            self.app.favsearch_pending.is_some(),
            self.ctx.memory(|m| m.focused()),
            self.ctx.memory(|m| m.has_focus(self.field)),
            self.app.favsearch.has_focus,
            self.app.favsearch.active,
            self.app.items.len(),
            self.app.favsearch.results_paths.len(),
            self.app.sidecar_restore_active(),
            self.app.top_level_grid_view.surface(),
        );
    }

    /// Let the search worker and the list load settle, drawing frames as the app does.
    fn settle(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        for _ in 0..4 {
            self.run(vec![]);
        }
        while self.app.favsearch_pending.is_some() || self.app.tag_view_pending.is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "favsearch never finished"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.run(vec![]);
        }
        for _ in 0..6 {
            self.run(vec![]);
        }
    }

    fn field_focused(&self) -> bool {
        self.ctx.memory(|m| m.has_focus(self.field))
    }
}

fn text(s: &str) -> egui::Event {
    egui::Event::Text(s.into())
}

#[expect(
    deprecated,
    reason = "winit still sends Enabled before a composition on Windows"
)]
fn preedit(s: &str) -> Vec<egui::Event> {
    vec![
        egui::Event::Ime(egui::ImeEvent::Enabled),
        egui::Event::Ime(egui::ImeEvent::Preedit {
            text: s.into(),
            active_range_chars: None,
        }),
    ]
}

fn typing_case(pinned: bool) {
    let _input = crate::key_input::lock_test_input();
    let mut f = fixture(pinned);
    assert!(f.app.favsearch.active, "Ctrl+S opened the search");
    assert!(
        f.field_focused(),
        "pinned={pinned}: Ctrl+S focused the field"
    );
    f.run(vec![text("a")]);
    assert_eq!(f.app.favsearch.query, "a");
    f.settle();
    assert!(
        f.field_focused(),
        "pinned={pinned}: the field keeps focus after the first search"
    );
    f.run(vec![text("l")]);
    f.settle();
    f.run(vec![text("p")]);
    f.settle();
    assert_eq!(f.app.favsearch.query, "alp", "pinned={pinned}");
    assert!(f.field_focused(), "pinned={pinned}");
    let mut results = f.app.favsearch.results_paths.clone();
    results.sort();
    assert_eq!(results.len(), 2, "alpha and alps: {results:?}");
}

fn ime_case(pinned: bool) {
    let _input = crate::key_input::lock_test_input();
    let mut f = fixture(pinned);
    assert!(
        f.field_focused(),
        "pinned={pinned}: Ctrl+S focused the field"
    );
    f.run(preedit("k"));
    f.settle();
    assert!(
        f.field_focused(),
        "pinned={pinned}: the field keeps focus during a composition"
    );
    f.run(vec![egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "か".into(),
        active_range_chars: None,
    })]);
    f.settle();
    f.run(vec![egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "かn".into(),
        active_range_chars: None,
    })]);
    f.settle();
    f.run(vec![egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "かな".into(),
        active_range_chars: None,
    })]);
    f.settle();
    assert_eq!(
        f.app.favsearch.query, "かな",
        "pinned={pinned}: the composition replaced its own preedit"
    );
    f.run(vec![egui::Event::Ime(egui::ImeEvent::Commit(
        "かな".into(),
    ))]);
    f.settle();
    assert_eq!(f.app.favsearch.query, "かな", "pinned={pinned}");
    assert!(f.field_focused(), "pinned={pinned}");
    assert_eq!(f.app.favsearch.results_paths.len(), 1);
}

#[test]
fn favsearch_typing_frame_unpinned_ascii() {
    typing_case(false);
}

#[test]
fn favsearch_typing_frame_pinned_ascii() {
    typing_case(true);
}

#[test]
fn favsearch_typing_frame_unpinned_ime() {
    ime_case(false);
}

#[test]
fn favsearch_typing_frame_pinned_ime() {
    ime_case(true);
}

fn tag_fixture() -> Fixture {
    let mut f = fixture(false);
    let mut db = crate::tags_db::TagsDb::open_at(&crate::data_dir::get().join("tags.db")).unwrap();
    for (name, tags) in [
        ("one.jpg", ["alpha", "\u{304b}\u{306a}"]),
        ("two.jpg", ["alps", "\u{304b}\u{304d}"]),
    ] {
        let path = f.app.tmp.path().join(name);
        image::RgbImage::new(2, 2).save(&path).unwrap();
        db.set_item_tags(&crate::tags_db::item_key_for_path(&path), tags, "test")
            .unwrap();
    }
    drop(db);
    f.app.open_tag_view();
    f.field = App::tag_search_input_id();
    f.settle();
    assert!(f.field_focused());
    f
}

#[test]
fn tag_typing_frame_ascii_keeps_focus_on_initial_and_repeated_adoption() {
    let _input = crate::key_input::lock_test_input();
    let mut f = tag_fixture();
    for (letter, query) in [("a", "a"), ("l", "al"), ("p", "alp")] {
        let was_tag = f.app.items_are_tag_view;
        f.run(vec![text(letter)]);
        f.settle();
        assert_eq!(f.app.tag_view.query, query);
        assert!(f.field_focused());
        assert!(!f.app.sidecar_restore_active());
        assert!(f.app.items_are_tag_view);
        assert_eq!(
            was_tag,
            query != "a",
            "both initial and repeat branches are exercised"
        );
        assert_eq!(f.app.tag_view.result_count, 2);
    }
}

#[test]
fn tag_typing_frame_ime_keeps_composition_across_result_adoptions() {
    let _input = crate::key_input::lock_test_input();
    let mut f = tag_fixture();
    f.run(preedit("\u{304b}"));
    f.settle();
    assert_eq!(f.app.tag_view.query, "\u{304b}");
    assert_eq!(f.app.tag_view.result_count, 2);
    assert!(f.field_focused());
    assert!(f.app.items_are_tag_view);
    f.run(vec![egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "\u{304b}\u{306a}".into(),
        active_range_chars: None,
    })]);
    f.settle();
    assert!(f.field_focused());
    assert_eq!(f.app.tag_view.query, "\u{304b}\u{306a}");
    assert_eq!(f.app.tag_view.result_count, 1);
    f.run(vec![egui::Event::Ime(egui::ImeEvent::Commit(
        "\u{304b}\u{306a}".into(),
    ))]);
    f.settle();
    assert!(f.field_focused());
    assert_eq!(f.app.tag_view.query, "\u{304b}\u{306a}");
    assert!(!f.app.sidecar_restore_active());
}

fn key(key: egui::Key, pressed: bool) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn favsearch_typing_frame_letters_do_not_trigger_grid_rotation_or_pin() {
    let _input = crate::key_input::lock_test_input();
    for pinned in [false, true] {
        let mut f = fixture(pinned);
        for (letter, code) in [
            ("a", egui::Key::A),
            ("l", egui::Key::L),
            ("p", egui::Key::P),
        ] {
            let (virtual_key, scan_code) = match code {
                egui::Key::A => (0x41, 0x1e),
                egui::Key::L => (0x4c, 0x26),
                egui::Key::P => (0x50, 0x19),
                _ => unreachable!(),
            };
            for pressed in [true, false] {
                // Inject the same native batch as the backend, without reading
                // physical key state. Both native and egui copies must stay owned.
                crate::key_input::set_test_frame(vec![crate::key_input::KeyEdge {
                    operation_id: crate::key_input::KeyOperationId::unassigned(),
                    source_hwnd: 0,
                    source_viewport: egui::ViewportId::ROOT,
                    virtual_key,
                    scan_code,
                    extended: false,
                    pressed,
                    repeat: false,
                    ctrl: false,
                    shift: false,
                    alt: false,
                }]);
                f.run(if pressed {
                    vec![key(code, true), text(letter)]
                } else {
                    vec![key(code, false)]
                });
            }
            crate::key_input::clear_test_frame();
            f.settle();
            assert!(f.field_focused());
            assert!(
                f.shortcut_leaks.is_empty(),
                "grid keymap must not consume editor keys: {:?}",
                f.shortcut_leaks
            );
        }
        assert_eq!(f.app.favsearch.query, "alp");
        assert_eq!(f.app.favsearch.results_paths.len(), 2);
    }
}

#[test]
fn favsearch_typing_frame_zero_results_and_empty_query_keep_focus() {
    let _input = crate::key_input::lock_test_input();
    for pinned in [false, true] {
        let mut f = fixture(pinned);
        for (query, count) in [("absent", 0), ("alp", 2), ("", 0)] {
            // Select the actual editor's contents, then replace them through TextEdit.
            f.run(vec![egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }]);
            f.run(vec![key(egui::Key::A, false)]);
            f.run(if query.is_empty() {
                vec![key(egui::Key::Backspace, true)]
            } else {
                vec![text(query)]
            });
            f.run(vec![key(egui::Key::Backspace, false)]);
            f.settle();
            assert_eq!(f.app.favsearch.query, query);
            assert_eq!(f.app.items.len(), count);
            assert!(f.field_focused());
            assert!(!f.app.sidecar_restore_active());
        }
    }
}
