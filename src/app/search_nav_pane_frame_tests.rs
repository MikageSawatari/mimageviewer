//! Pane keyboard ownership across the actual main-frame input/drawing phases.
//!
//! The native queue and egui events describe the same Enter. Drawing only the
//! pane, or supplying only egui input, misses a fixed-key/native-keymap leak.
#![cfg(windows)]

use super::*;
use crate::folder_pane::FolderPaneState;

#[test]
fn search_nav_pane_frame_metadata_enter_opens_tree_target() {
    pane_enter_case(Some(0));
}

#[test]
fn search_nav_pane_frame_everything_enter_opens_tree_target() {
    pane_enter_case(Some(1));
}

#[test]
fn search_nav_pane_frame_favorite_search_enter_opens_tree_target() {
    pane_enter_case(Some(2));
}

#[test]
fn search_nav_pane_frame_tag_enter_opens_tree_target() {
    pane_enter_case(Some(3));
}

#[test]
fn search_nav_pane_frame_plain_enter_opens_tree_target() {
    pane_enter_case(None);
}

#[test]
fn search_nav_pane_frame_enter_preserves_modifiers_keypad_and_coalescing() {
    let _input = crate::key_input::lock_test_input();
    for (modifiers, numpad, presses) in [
        (egui::Modifiers::SHIFT, false, 1),
        (egui::Modifiers::ALT, false, 1),
        (egui::Modifiers::NONE, true, 1),
        (egui::Modifiers::SHIFT | egui::Modifiers::ALT, true, 2),
    ] {
        pane_enter_with_input(None, modifiers, numpad, presses);
    }
}

fn pane_enter_case(kind: Option<usize>) {
    let _input = crate::key_input::lock_test_input();
    pane_enter_with_input(kind, egui::Modifiers::NONE, false, 1);
}

fn pane_enter_with_input(
    kind: Option<usize>,
    modifiers: egui::Modifiers,
    numpad: bool,
    presses: usize,
) {
    for native_twin in [false, true] {
        let mut app = match kind {
            Some(kind) => App::navigation_search_fixture_for_test(kind),
            None => setup_app_for_test(),
        };
        let root = app.tmp.path().join("pane-tree");
        let origin = app.tmp.path().join("pre-search");
        let target = root.join("pane-target");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        let result_book = app.tmp.path().join("result.zip");
        write_selected_book(&result_book);
        if kind.is_none() {
            app.current_folder = Some(origin.clone());
            app.top_level_grid_view
                .replace_surface(top_level_grid_view::TopLevelGridSurface::Folder);
            app.install_new_items(vec![GridItem::ZipFile(result_book)], vec![None]);
            app.visible_indices = vec![0];
            app.selected = Some(0);
        }
        app.settings.auto_fullscreen_zip_pdf = false;
        // A configurable grid action on the same physical chord must not see
        // the key that the fixed pane owner has already consumed.
        let mut bindings = crate::keymap::KeymapSettings::default();
        bindings.set_override_chords(
            KeyAction::GridOpenSelected,
            vec![crate::keymap::Chord::new(
                false,
                modifiers.shift,
                modifiers.alt,
                if numpad {
                    crate::keymap::KeyName::NumpadEnter
                } else {
                    crate::keymap::KeyName::Enter
                },
            )],
        );
        app.keymap = crate::keymap::Keymap::from_settings(&bindings);
        app.settings.show_toolbar_search_metadata = false;
        app.settings.show_toolbar_search_everything = false;
        app.settings.show_toolbar_search_container = false;
        app.settings.folder_tree_pane_visible = true;
        app.folder_pane = FolderPaneState::test_tree(&root, vec![target.clone()]);
        app.folder_pane.test_seed_listing(&target, vec![], false);
        // Establish the initial current location while the fixture's tree scope
        // is explicit. Later grid focus may follow a *new* location, as in the UI.
        let active = app.effective_folder();
        let options = crate::folder_pane::FolderPaneListingOptions::from_settings(&app.settings);
        app.folder_pane.sync_to_active(active.as_deref(), options);
        app.folder_pane.set_cursor(root.clone());
        app.folder_pane.set_focus_grid();
        // Model opening the real temporary editor. Its reservation is retired
        // only by drawing the editor and clicking the actual pane below.
        match kind {
            Some(0 | 1) => app.global_search.focus_request = true,
            Some(2) => app.favsearch.focus_request = true,
            Some(3) => app.tag_view.focus_request = true,
            _ => {}
        }
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        crate::ui_fonts::configure_fonts(&ctx);
        ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
        let mut frame = 0;
        main_frame(&mut app, &ctx, &mut frame, vec![]);
        main_frame(&mut app, &ctx, &mut frame, vec![]);
        if kind.is_some() {
            assert!(
                crate::ime_focus::focused_text_input(&ctx).is_some(),
                "kind={kind:?}: setup must draw and focus the search editor"
            );
        }

        // Click the root disclosure arrow: this transfers focus without opening
        // a folder (a row-label click would itself navigate before Enter).
        let label_id: egui::Id = ctx
            .data(|data| {
                data.get_temp(egui::Id::unique((
                    "folder_pane_test_label_id",
                    root.as_path(),
                )))
            })
            .unwrap_or_else(|| {
                panic!(
                    "actual main bars must render the root {root:?}; kind={kind:?}, visible={}, drive={:?}, rows={:?}",
                    app.settings.folder_tree_pane_visible,
                    app.folder_pane.selected_drive(),
                    app.folder_pane.visible_rows(),
                )
            });
        let label = ctx.read_response(label_id).unwrap().rect;
        let arrow = egui::pos2(label.left() - 14.0, label.center().y);
        for pressed in [true, false] {
            main_frame(
                &mut app,
                &ctx,
                &mut frame,
                vec![
                    egui::Event::PointerMoved(arrow),
                    egui::Event::PointerButton {
                        pos: arrow,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        main_frame(&mut app, &ctx, &mut frame, vec![]);
        assert!(app.folder_pane.has_focus, "real pane click owns the tree");
        assert!(
            crate::ime_focus::focused_text_input(&ctx).is_none(),
            "real pane click must blur the actual search editor"
        );
        // The click collapsed the root. Expand and move to its only child via
        // the same main keyboard entry that will receive Enter.
        for key in [egui::Key::ArrowRight, egui::Key::ArrowDown] {
            main_frame(&mut app, &ctx, &mut frame, vec![key_event(key)]);
        }
        assert!(
            app.folder_pane
                .visible_rows()
                .iter()
                .any(|row| row.path == target && row.is_cursor),
            "kind={kind:?}: real pane key routing selected its target"
        );
        assert_eq!(app.selected, Some(0), "selected grid book remains present");
        assert!(app.folder_pane_open_pending.is_none());
        assert!(app.top_level_grid_view.open_path_classification().is_none());
        if native_twin {
            stage_native_enter(modifiers, numpad, presses);
        }
        let mut enter = key_event(egui::Key::Enter);
        if let egui::Event::Key {
            modifiers: event_modifiers,
            ..
        } = &mut enter
        {
            *event_modifiers = modifiers;
        }
        main_frame(&mut app, &ctx, &mut frame, vec![enter; presses]);
        crate::key_input::clear_test_frame();
        assert!(
            app.folder_pane_open_pending
                .as_ref()
                .is_some_and(|pending| pending.path == target)
                || app.current_folder.as_deref() == Some(target.as_path()),
            "kind={kind:?} native_twin={native_twin}: Enter must request the pane target, not the selected result book; pending={:?}, current={:?}, book_open={}",
            app.folder_pane_open_pending
                .as_ref()
                .map(|pending| &pending.path),
            app.current_folder,
            app.top_level_grid_view.open_path_classification().is_some(),
        );
        assert!(
            app.top_level_grid_view.open_path_classification().is_none(),
            "kind={kind:?} native_twin={native_twin}: selected book must not start opening"
        );
        assert!(
            app.top_level_grid_view
                .history_navigation_transition()
                .is_none(),
            "kind={kind:?} native_twin={native_twin}: selected ZIP must not start a physical open"
        );
        assert!(app.fullscreen_idx.is_none());
        if kind.is_some() {
            app.verify_admitted_user_navigation_for_test(&target, &origin);
        } else {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while app.current_folder.as_deref() != Some(target.as_path()) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "plain pane scan adoption"
                );
                app.post_render_main_navigation(&ctx, empty_navigation());
                std::thread::yield_now();
            }
        }
    }
}

fn main_frame(app: &mut App, ctx: &egui::Context, frame: &mut u64, events: Vec<egui::Event>) {
    *frame += 1;
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 700.0),
            )),
            time: Some(*frame as f64 / 60.0),
            events,
            ..Default::default()
        },
        |ui| {
            app.update_ime_state(ctx);
            // App::update_frame runs root keyboard before toolbar and main bars;
            // main bars render all search editors before the tree, then grid.
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
                    keyboard_nav,
                    keyboard_nav_from_fullscreen_close,
                    toolbar_fav_nav,
                    address_nav,
                    folder_pane_nav,
                    grid_nav,
                    ..empty_navigation()
                },
            );
            app.finish_folder_pane_presentation(ctx, true);
        },
    )
    .drop_without_applying_deltas();
}

fn empty_navigation() -> update_phases::MainFrameNavigation {
    update_phases::MainFrameNavigation {
        fav_nav: None,
        toolbar_fav_nav: None,
        fullscreen_close_nav: None,
        keyboard_nav: None,
        keyboard_nav_from_fullscreen_close: false,
        gamepad_nav: None,
        address_nav: None,
        open_folder_nav: None,
        context_nav: None,
        folder_pane_nav: None,
        grid_nav: None,
    }
}

fn key_event(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn stage_native_enter(modifiers: egui::Modifiers, numpad: bool, presses: usize) {
    crate::key_input::set_test_frame(vec![
        crate::key_input::KeyEdge {
            operation_id: crate::key_input::KeyOperationId::unassigned(),
            source_hwnd: 0,
            source_viewport: egui::ViewportId::ROOT,
            virtual_key: 0x0D,
            scan_code: 0x1C,
            extended: numpad,
            pressed: true,
            repeat: false,
            ctrl: false,
            shift: modifiers.shift,
            alt: modifiers.alt,
        };
        presses
    ]);
}

fn write_selected_book(path: &Path) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    zip.start_file("001.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(2, 2)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    std::io::Write::write_all(&mut zip, bytes.get_ref()).unwrap();
    zip.finish().unwrap();
}
