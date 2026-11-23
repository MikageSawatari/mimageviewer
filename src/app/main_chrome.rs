use super::App;

impl App {
    /// Dismiss background menus once when a real modal takes input ownership.
    pub(crate) fn note_main_chrome_pointer_block_transition(&mut self, ctx: &egui::Context) {
        // Yield this read-only list before any other modal can consume Esc and
        // disappear earlier in the dialog drawing order.
        self.close_omitted_entries_dialog_if_interrupted();
        let blocked = self.main_chrome_pointer_blocked();
        if blocked && !self.main_chrome_pointer_was_blocked {
            // Popups have independent Uis; disabling their anchor cannot revoke
            // an already open menu. Submenus keep a separate open-item owner.
            egui::Popup::close_all(ctx);
            crate::ui_history_field::close(ctx);
            ctx.data_mut(|data| data.remove_by_type::<egui::containers::menu::MenuState>());
        }
        self.main_chrome_pointer_was_blocked = blocked;
    }

    /// Hold background chrome input without dimming the whole main window.
    pub(crate) fn gate_main_chrome(&self, ui: &mut egui::Ui) {
        #[cfg(test)]
        crate::ime_focus::typing_probe::chrome_gate(
            ui.ctx(),
            self.modal_dialog_block_reason_including_menus(false),
        );
        if self.main_chrome_pointer_blocked() {
            // Disable at full alpha, then restore the style so genuinely disabled
            // child controls keep their own disabled appearance.
            let disabled_alpha = ui.visuals().disabled_alpha;
            ui.visuals_mut().disabled_alpha = 1.0;
            ui.disable();
            ui.visuals_mut().disabled_alpha = disabled_alpha;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::{NodeT, Queryable};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    #[derive(Clone, Copy, Debug)]
    enum Blocker {
        None,
        GridMenu,
        Delete,
        Preferences,
        Archive,
    }

    const BLOCKERS: [Blocker; 5] = [
        Blocker::None,
        Blocker::GridMenu,
        Blocker::Delete,
        Blocker::Preferences,
        Blocker::Archive,
    ];

    impl Blocker {
        fn blocked(self) -> bool {
            matches!(self, Self::Delete | Self::Preferences | Self::Archive)
        }

        fn install(self, app: &mut App) {
            use crate::ui_dialogs::archive_convert::{
                ArchiveConvertCompletionPolicy, ArchiveConvertPhase, ArchiveConvertProgressShared,
                ArchiveConvertState,
            };
            app.context_menu_idx =
                matches!(self, Self::GridMenu).then(|| app.capture_grid_context_menu_owner(0));
            app.show_delete_confirm = matches!(self, Self::Delete);
            app.show_preferences = matches!(self, Self::Preferences);
            app.archive_convert = matches!(self, Self::Archive).then(|| {
                Box::new(ArchiveConvertState {
                    src_path: "test.7z".into(),
                    input_seq: 0,
                    format: crate::archive_converter::ArchiveFormat::SevenZ,
                    password: None,
                    password_input: String::new(),
                    phase: ArchiveConvertPhase::Converting {
                        progress: std::sync::Arc::new(ArchiveConvertProgressShared::new()),
                    },
                    cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    rx: std::sync::mpsc::channel().1,
                    pending_nav: None,
                    pending_direct_nav: None,
                    allow_direct_read: false,
                    completion: ArchiveConvertCompletionPolicy::Navigation,
                    restore_intent: super::super::StartupListIntent::ExplicitList,
                    pending_sibling_output: None,
                    auto_fullscreen: false,
                    deferred_fullscreen: None,
                    suppress_confirm: false,
                    suppress_confirm_next_time: false,
                })
            });
        }
    }

    #[test]
    fn main_chrome_modal_classification_keeps_menu_tools_and_hidden_scans_distinct() {
        use crate::ui_dialogs::archive_convert::ArchiveConvertPhase;
        let mut app = crate::app::setup_app_for_test();
        Blocker::None.install(&mut app);
        assert!(!app.main_chrome_pointer_blocked());
        Blocker::GridMenu.install(&mut app);
        assert!(app.common_modal_dialog_open());
        assert!(!app.main_chrome_pointer_blocked());
        // This dialog comes after grid_context_menu in the shared reason chain.
        app.text_add_bubble_dialog = true;
        assert!(app.main_chrome_pointer_blocked());
        app.text_add_bubble_dialog = false;
        Blocker::None.install(&mut app);
        app.show_vst3_manager = true;
        assert!(app.any_dialog_open());
        assert!(!app.common_modal_dialog_open());
        assert!(!app.main_chrome_pointer_blocked());
        app.show_vst3_manager = false;
        Blocker::Archive.install(&mut app);
        assert!(app.main_chrome_pointer_blocked());
        {
            let state = app.archive_convert.as_mut().unwrap();
            state.phase = ArchiveConvertPhase::Scanning;
            state.suppress_confirm = true;
        }
        assert!(!app.archive_convert_dialog_visible());
        assert!(!app.main_chrome_pointer_blocked());
        app.archive_convert.as_mut().unwrap().suppress_confirm = false;
        assert!(app.archive_convert_dialog_visible());
        assert!(app.main_chrome_pointer_blocked());
    }

    fn click(harness: &mut egui_kittest::Harness<'_, ()>, label: &str) {
        let pos = harness.get_by_label(label).rect().center();
        harness.hover_at(pos);
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            harness.step();
        }
        harness.run();
    }

    #[test]
    fn main_chrome_toolbar_and_menu_clicks_follow_common_modal_ownership() {
        let mut env = crate::app::setup_app_for_test();
        let destination = env.tmp.path().join("favorite");
        env.settings.favorites = vec![crate::settings::FavoriteEntry {
            id: uuid::Uuid::new_v4(),
            name: "modal test favorite".into(),
            path: destination.clone(),
            auto_index_structure: false,
            auto_index_metadata: false,
            auto_index_thumbs: false,
            auto_index_similar: false,
        }];
        env.settings.toolbar_favorites_display = crate::settings::ToolbarSectionDisplay::Buttons;
        let open_label = env
            .keymap
            .menu_command_label(crate::keymap::MenuCommandId::FileOpenFolder);
        let app = Rc::new(RefCell::new(env));
        let render = Rc::clone(&app);
        let nav = Rc::new(RefCell::new(Vec::new()));
        let rendered_nav = Rc::clone(&nav);
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(1600.0, 640.0))
            .build_ui(move |root_ui| {
                let mut app = render.borrow_mut();
                app.note_main_chrome_pointer_block_transition(root_ui.ctx());
                app.render_menubar(root_ui);
                if let (Some(path), _) = app.render_toolbar(root_ui) {
                    rendered_nav.borrow_mut().push(path);
                }
            });
        for blocker in BLOCKERS {
            blocker.install(&mut app.borrow_mut());
            app.borrow_mut().show_open_folder_dialog = false;
            harness.run();
            click(&mut harness, "modal test favorite");
            let expected = if blocker.blocked() {
                vec![]
            } else {
                vec![destination.clone()]
            };
            assert_eq!(*nav.borrow(), expected, "toolbar: {blocker:?}");
            nav.borrow_mut().clear();
            click(&mut harness, crate::keymap::TopMenuId::File.label());
            if blocker.blocked() {
                assert!(
                    harness.query_by_label(&open_label).is_none(),
                    "menu opened: {blocker:?}"
                );
            } else {
                click(&mut harness, &open_label);
            }
            assert_eq!(
                app.borrow().show_open_folder_dialog,
                !blocker.blocked(),
                "menu: {blocker:?}"
            );
        }
    }

    #[test]
    fn main_chrome_preopened_menu_cannot_dispatch_after_common_modal_appears() {
        let env = crate::app::setup_app_for_test();
        let open_label = env
            .keymap
            .menu_command_label(crate::keymap::MenuCommandId::FileOpenFolder);
        let app = Rc::new(RefCell::new(env));
        let render = Rc::clone(&app);
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(1200.0, 640.0))
            .build_ui(move |root_ui| {
                let mut app = render.borrow_mut();
                app.note_main_chrome_pointer_block_transition(root_ui.ctx());
                app.render_menubar(root_ui);
            });
        for blocker in [Blocker::Delete, Blocker::Preferences, Blocker::Archive] {
            Blocker::None.install(&mut app.borrow_mut());
            app.borrow_mut().show_open_folder_dialog = false;
            egui::Popup::close_all(&harness.ctx);
            harness.run();
            click(&mut harness, crate::keymap::TopMenuId::File.label());
            assert!(harness.query_by_label(&open_label).is_some());
            // An asynchronous owner can become modal while the background menu
            // is already open, without a menu command dismissing that popup.
            blocker.install(&mut app.borrow_mut());
            harness.run();
            if harness.query_by_label(&open_label).is_some() {
                click(&mut harness, &open_label);
            }
            assert!(
                !app.borrow().show_open_folder_dialog,
                "preopened menu dispatched behind {blocker:?}"
            );
        }
    }

    #[test]
    fn main_chrome_transition_preserves_new_dialog_popups_and_menu_only_operations() {
        let mut env = crate::app::setup_app_for_test();
        Blocker::GridMenu.install(&mut env);
        let app = Rc::new(RefCell::new(env));
        let render = Rc::clone(&app);
        let actions = Rc::new(RefCell::new(Vec::new()));
        let clicked = Rc::clone(&actions);
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(800.0, 480.0))
            .build_ui(move |root_ui| {
                let mut app = render.borrow_mut();
                app.note_main_chrome_pointer_block_transition(root_ui.ctx());
                root_ui.scope(|ui| {
                    app.gate_main_chrome(ui);
                    ui.menu_button("background menu", |ui| {
                        if ui.button("background command").clicked() {
                            clicked.borrow_mut().push("background");
                            ui.close();
                        }
                    });
                });
                if app.show_delete_confirm {
                    egui::Window::new("modal owner")
                        .fixed_pos(egui::pos2(300.0, 80.0))
                        .show(root_ui.ctx(), |ui| {
                            ui.menu_button("dialog menu", |ui| {
                                if ui.button("dialog command").clicked() {
                                    clicked.borrow_mut().push("dialog");
                                    ui.close();
                                }
                            });
                        });
                }
            });
        harness.run();
        click(&mut harness, "background menu");
        assert!(harness.query_by_label("background command").is_some());
        click(&mut harness, "background command");
        assert_eq!(*actions.borrow(), ["background"]);
        click(&mut harness, "background menu");
        Blocker::Delete.install(&mut app.borrow_mut());
        harness.run();
        assert!(harness.query_by_label("background command").is_none());
        click(&mut harness, "dialog menu");
        harness.run_steps(3);
        assert!(harness.query_by_label("dialog command").is_some());
        click(&mut harness, "dialog command");
        assert_eq!(*actions.borrow(), ["background", "dialog"]);
        Blocker::None.install(&mut app.borrow_mut());
        harness.run();
        click(&mut harness, "background menu");
        Blocker::Preferences.install(&mut app.borrow_mut());
        harness.run();
        assert!(harness.query_by_label("background command").is_none());
    }

    #[test]
    fn main_chrome_transition_clears_independent_submenu_state() {
        use egui::containers::menu::MenuConfig;
        let app = Rc::new(RefCell::new(crate::app::setup_app_for_test()));
        let render = Rc::clone(&app);
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(800.0, 480.0))
            .build_ui(move |root_ui| {
                let mut app = render.borrow_mut();
                app.note_main_chrome_pointer_block_transition(root_ui.ctx());
                // Inline menu roots remain drawn when their chrome is disabled;
                // their submenu is owned by MenuState, not Popup's memory slot.
                root_ui.scope_builder(
                    egui::UiBuilder::new().ui_stack_info(
                        egui::UiStackInfo::new(egui::UiKind::Menu)
                            .with_tag_value(MenuConfig::MENU_CONFIG_TAG, MenuConfig::default()),
                    ),
                    |ui| {
                        app.gate_main_chrome(ui);
                        ui.menu_button("background submenu", |ui| {
                            let _ = ui.button("submenu command");
                        });
                    },
                );
            });
        harness.run();
        click(&mut harness, "background submenu");
        assert!(harness.query_by_label("submenu command").is_some());
        Blocker::Delete.install(&mut app.borrow_mut());
        harness.run();
        assert!(harness.query_by_label("submenu command").is_none());
    }

    #[test]
    fn main_chrome_folder_tree_clicks_follow_common_modal_ownership() {
        let mut env = crate::app::setup_app_for_test();
        if matches!(env.indexer_init, crate::app::IndexerInit::NotStarted) {
            env.indexer_init = crate::app::IndexerInit::Unavailable;
        }
        assert!(!env.indexer_init.overlay_active());
        let destination = env.tmp.path().join("modal-tree");
        std::fs::create_dir_all(&destination).unwrap();
        env.current_folder = None;
        env.settings.folder_tree_pane_visible = true;
        // This fixture owns click/resize routing, not completion of filesystem I/O.
        env.folder_pane = crate::folder_pane::FolderPaneState::test_tree(&destination, vec![]);
        let label = crate::folder_pane::folder_label(&destination);
        let app = Rc::new(RefCell::new(env));
        let render = Rc::clone(&app);
        let nav = Rc::new(RefCell::new(Vec::new()));
        let rendered_nav = Rc::clone(&nav);
        let pane_rect = Rc::new(Cell::new(None));
        let rendered_rect = Rc::clone(&pane_rect);
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(800.0, 480.0))
            .build_ui(move |root_ui| {
                let mut app = render.borrow_mut();
                app.input_update_begin(root_ui.ctx());
                app.note_main_chrome_pointer_block_transition(root_ui.ctx());
                let (path, rect) = app.render_folder_pane(root_ui);
                app.finish_folder_pane_presentation(root_ui.ctx(), true);
                rendered_rect.set(rect);
                if let Some(path) = path {
                    rendered_nav.borrow_mut().push(path);
                }
            });
        for blocker in BLOCKERS {
            blocker.install(&mut app.borrow_mut());
            harness.run();
            assert_eq!(
                harness
                    .query_by_role(egui::accesskit::Role::Splitter)
                    .is_some(),
                !blocker.blocked(),
                "resize handle: {blocker:?}"
            );
            click(&mut harness, &label);
            let expected = if blocker.blocked() {
                vec![]
            } else {
                vec![crate::folder_pane::FolderPaneActivation::Open(
                    destination.clone(),
                )]
            };
            assert_eq!(*nav.borrow(), expected, "tree: {blocker:?}");
            nav.borrow_mut().clear();
            let before = app.borrow().settings.folder_tree_pane_width_ratio;
            let rect = pane_rect.get().unwrap();
            let start = egui::pos2(rect.right(), rect.center().y);
            let end = start + egui::vec2(45.0, 0.0);
            harness.hover_at(start);
            harness.event(egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            });
            harness.step();
            harness.hover_at(end);
            harness.step();
            harness.event(egui::Event::PointerButton {
                pos: end,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            });
            harness.step();
            harness.run();
            let delta = (app.borrow().settings.folder_tree_pane_width_ratio - before).abs();
            if blocker.blocked() {
                assert!(delta < 0.0001, "background resize moved behind {blocker:?}");
            } else {
                assert!(
                    delta > 0.01,
                    "ordinary pane resize stopped working: {blocker:?}"
                );
            }
        }
    }

    #[test]
    fn main_chrome_global_search_close_clicks_follow_common_modal_ownership() {
        let app = Rc::new(RefCell::new(crate::app::setup_app_for_test()));
        let render = Rc::clone(&app);
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(1600.0, 640.0))
            .build_ui(move |root_ui| {
                let mut app = render.borrow_mut();
                app.note_main_chrome_pointer_block_transition(root_ui.ctx());
                app.render_global_search_bar(root_ui);
            });
        for everything in [false, true] {
            for blocker in BLOCKERS {
                {
                    let mut app = app.borrow_mut();
                    Blocker::None.install(&mut app);
                    if everything {
                        app.open_everything_search();
                    } else {
                        app.open_global_search();
                    }
                    assert!(app.global_search.active);
                    blocker.install(&mut app);
                }
                harness.run();
                click(&mut harness, "\u{d7}");
                assert_eq!(
                    app.borrow().global_search.active,
                    blocker.blocked(),
                    "search everything={everything}: {blocker:?}"
                );
            }
        }
    }

    #[test]
    fn main_chrome_input_gate_preserves_opacity_and_child_disabled_style() {
        let mut app = crate::app::setup_app_for_test();
        app.show_delete_confirm = true;
        let mut harness = crate::test_ui::builder().build_ui(move |ui| {
            let opacity = ui.opacity();
            let disabled_alpha = ui.visuals().disabled_alpha;
            app.note_main_chrome_pointer_block_transition(ui.ctx());
            app.gate_main_chrome(ui);
            assert!(!ui.is_enabled());
            assert_eq!(ui.opacity(), opacity);
            assert_eq!(ui.visuals().disabled_alpha, disabled_alpha);
            ui.add_enabled_ui(false, |ui| {
                assert!(!ui.is_enabled());
                assert_eq!(ui.opacity(), opacity * disabled_alpha);
            });
        });
        harness.run();
    }

    #[derive(Clone, Copy, Debug)]
    enum R6Menu {
        Grid,
        Bookmark,
        ReadingHistory,
    }

    fn r6_menu_fixture(
        kind: R6Menu,
    ) -> (
        crate::app::AppTestEnvForTest,
        egui::Context,
        u64,
        std::path::PathBuf,
    ) {
        let mut app = crate::app::setup_app_for_test();
        if matches!(app.indexer_init, crate::app::IndexerInit::NotStarted) {
            app.indexer_init = crate::app::IndexerInit::Unavailable;
        }
        assert!(!app.indexer_init.overlay_active());
        assert!(
            app.main_hwnd.is_none(),
            "the real grid-menu route must fall back before any native call"
        );
        let root = app.tmp.path().join("r6-menu-tree");
        let origin = root.join("a");
        let target = root.join("b");
        let child = target.join("child");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::create_dir_all(&child).unwrap();
        let media = origin.join("menu-source.mp4");
        std::fs::write(&media, b"menu fixture").unwrap();
        app.current_folder = Some(origin.clone());
        app.install_new_items(
            vec![crate::grid_item::GridItem::Video(media.clone())],
            vec![None],
        );
        app.visible_indices = vec![0];
        app.selected = Some(0);
        match kind {
            R6Menu::Grid => {}
            R6Menu::Bookmark => {
                app.items_are_bookmark_view = true;
                app.current_folder = Some(crate::app::bookmark_view_synthetic_path());
                app.top_level_grid_view.replace_surface(
                    crate::app::top_level_grid_view::TopLevelGridSurface::Bookmarks,
                );
                app.bookmark_browser_rows = vec![crate::bookmark_browser::BookmarkBrowserRow {
                    source: crate::bookmark_browser::BookmarkRowSource::Media {
                        id: 1,
                        path: media.clone(),
                        pts_secs: 1.0,
                        title: None,
                        is_audio: false,
                    },
                    item: crate::grid_item::GridItem::Video(media),
                    relative_page_provenance: None,
                    image_meta: None,
                    marker_thumbnail: None,
                    created_at_ms: 0,
                    missing: false,
                }];
            }
            R6Menu::ReadingHistory => {
                app.items_are_reading_history_view = true;
                app.current_folder = Some(crate::app::reading_history_synthetic_path());
                app.top_level_grid_view.replace_surface(
                    crate::app::top_level_grid_view::TopLevelGridSurface::ReadingHistory(
                        crate::app::top_level_grid_view::ReadingHistoryRootRestore::unavailable(),
                    ),
                );
            }
        }
        app.settings.folder_tree_pane_visible = true;
        app.folder_pane =
            crate::folder_pane::FolderPaneState::test_tree(&root, vec![origin, target.clone()]);
        app.folder_pane
            .test_seed_listing(&target, vec![child], false);
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        crate::ime_focus::install_ime_input_policy(&ctx);
        ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
        let mut frame = 0;
        for _ in 0..3 {
            assert!(r6_menu_frame(&mut app, &ctx, &mut frame, vec![]).is_none());
        }
        (app, ctx, frame, target)
    }

    fn r6_primary(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn r6_menu_layer(kind: R6Menu) -> egui::LayerId {
        match kind {
            R6Menu::Bookmark => egui::LayerId::new(
                egui::Order::Middle,
                egui::Id::unique("bookmark_grid_context_menu"),
            ),
            _ => egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("grid_ctx_menu")),
        }
    }

    fn r6_tree_pos(ctx: &egui::Context, path: &std::path::Path, arrow: bool) -> egui::Pos2 {
        let rect: egui::Rect = ctx
            .data(|data| data.get_temp(egui::Id::unique(("folder_pane_test_label_rect", path))))
            .unwrap();
        if arrow {
            egui::pos2(rect.left() - 14.0, rect.center().y)
        } else {
            rect.center()
        }
    }

    fn r6_menu_frame(
        app: &mut App,
        ctx: &egui::Context,
        frame: &mut u64,
        events: Vec<egui::Event>,
    ) -> Option<crate::folder_pane::FolderPaneActivation> {
        *frame += 1;
        let mut nav = None;
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
                app.input_update_begin(ctx);
                app.update_ime_state(ctx);
                let (keyboard_nav, _, _) = app.input_root_keyboard(ctx);
                assert!(keyboard_nav.is_none());
                app.note_main_chrome_pointer_block_transition(ctx);
                if let Some(owner) = app.context_menu_idx {
                    assert!(
                        app.grid_context_menu_owner_is_current(owner),
                        "this is a live menu owner, not a stale flag"
                    );
                }
                // The actual menu's outside-click retirement precedes toolbar/pane,
                // but follows root recognition, exactly as in App::update_frame.
                assert!(
                    app.show_context_menu(ctx).is_none(),
                    "the test never invokes a menu command"
                );
                let (toolbar_nav, address_nav) = app.render_toolbar(ui);
                assert!(toolbar_nav.is_none() && address_nav.is_none());
                nav = app.render_main_bars(ui, &mut None).0;
                app.finish_folder_pane_presentation(ctx, true);
            },
        )
        .drop_without_applying_deltas();
        nav
    }

    fn r6_open_menu(
        app: &mut App,
        ctx: &egui::Context,
        frame: &mut u64,
        kind: R6Menu,
        pos: egui::Pos2,
    ) {
        app.context_menu_pos = pos;
        let owner = app.capture_grid_context_menu_owner(0);
        assert!(app.grid_context_menu_owner_is_current(owner));
        app.context_menu_idx = Some(owner);
        for _ in 0..2 {
            assert!(r6_menu_frame(app, ctx, frame, vec![]).is_none());
        }
        assert!(app.context_menu_idx.is_some());
        let area = egui::AreaState::load(ctx, r6_menu_layer(kind).id)
            .expect("the production menu Window must be painted");
        assert!(area.interactable && area.rect().is_positive());
    }

    fn r6_outside_menu_click(kind: R6Menu, arrow: bool) {
        let (mut app, ctx, mut frame, target) = r6_menu_fixture(kind);
        let pos = r6_tree_pos(&ctx, &target, arrow);
        r6_open_menu(&mut app, &ctx, &mut frame, kind, egui::pos2(800.0, 100.0));
        let area = egui::AreaState::load(&ctx, r6_menu_layer(kind).id).unwrap();
        assert!(!area.rect().contains(pos));
        assert!(
            app.folder_pane
                .label_display
                .as_ref()
                .unwrap()
                .rect
                .contains(pos)
        );
        assert!(
            r6_menu_frame(
                &mut app,
                &ctx,
                &mut frame,
                vec![egui::Event::PointerMoved(pos), r6_primary(pos, true)]
            )
            .is_none()
        );
        assert!(
            app.context_menu_idx.is_some(),
            "the menu is still open at the press checkpoint"
        );
        let nav = r6_menu_frame(&mut app, &ctx, &mut frame, vec![r6_primary(pos, false)]);
        assert!(
            app.context_menu_idx.is_none(),
            "the real menu retired on this outside release"
        );
        if arrow {
            assert!(nav.is_none());
            assert!(
                app.folder_pane
                    .visible_rows()
                    .iter()
                    .any(|row| row.path == target && row.expanded),
                "{kind:?}: outside arrow click must expand"
            );
        } else {
            assert_eq!(
                nav,
                Some(crate::folder_pane::FolderPaneActivation::Open(
                    target.clone()
                )),
                "{kind:?}: the same outside label click must navigate"
            );
            assert!(
                !app.folder_pane
                    .visible_rows()
                    .iter()
                    .find(|row| row.path == target)
                    .unwrap()
                    .expanded
            );
        }
    }

    #[test]
    fn main_chrome_r6_grid_menu_outside_label_follows_input_order() {
        r6_outside_menu_click(R6Menu::Grid, false);
    }
    #[test]
    fn main_chrome_r6_grid_menu_outside_arrow_follows_input_order() {
        r6_outside_menu_click(R6Menu::Grid, true);
    }
    #[test]
    fn main_chrome_r6_bookmark_menu_outside_label_follows_input_order() {
        r6_outside_menu_click(R6Menu::Bookmark, false);
    }
    #[test]
    fn main_chrome_r6_bookmark_menu_outside_arrow_follows_input_order() {
        r6_outside_menu_click(R6Menu::Bookmark, true);
    }
    #[test]
    fn main_chrome_r6_reading_history_menu_outside_label_follows_input_order() {
        r6_outside_menu_click(R6Menu::ReadingHistory, false);
    }
    #[test]
    fn main_chrome_r6_reading_history_menu_outside_arrow_follows_input_order() {
        r6_outside_menu_click(R6Menu::ReadingHistory, true);
    }

    #[test]
    fn main_chrome_r6_menu_outside_second_press_continues_name_gesture() {
        for kind in [R6Menu::Grid, R6Menu::Bookmark, R6Menu::ReadingHistory] {
            let (mut app, ctx, mut frame, target) = r6_menu_fixture(kind);
            let pos = r6_tree_pos(&ctx, &target, false);
            assert!(
                r6_menu_frame(
                    &mut app,
                    &ctx,
                    &mut frame,
                    vec![egui::Event::PointerMoved(pos), r6_primary(pos, true)]
                )
                .is_none()
            );
            assert_eq!(
                r6_menu_frame(&mut app, &ctx, &mut frame, vec![r6_primary(pos, false)]),
                Some(crate::folder_pane::FolderPaneActivation::Open(
                    target.clone()
                ))
            );
            let first_release = frame;
            r6_open_menu(&mut app, &ctx, &mut frame, kind, egui::pos2(800.0, 100.0));
            assert!(
                r6_menu_frame(
                    &mut app,
                    &ctx,
                    &mut frame,
                    vec![egui::Event::PointerMoved(pos), r6_primary(pos, true)]
                )
                .is_none()
            );
            assert!(
                (frame - first_release) as f64 / 60.0
                    < ctx.options(|options| options.input_options.max_double_click_delay)
            );
            assert!(
                app.context_menu_idx.is_some(),
                "the menu still exists when the outside continuation is applied"
            );
            assert!(
                app.folder_pane
                    .visible_rows()
                    .iter()
                    .any(|row| row.path == target && row.expanded),
                "{kind:?}: the outside second press toggles the first row"
            );
            assert!(
                r6_menu_frame(&mut app, &ctx, &mut frame, vec![r6_primary(pos, false)]).is_none()
            );
            assert!(app.context_menu_idx.is_none());
            assert!(
                app.folder_pane
                    .visible_rows()
                    .iter()
                    .any(|row| row.path == target && row.expanded)
            );
        }
    }

    #[test]
    fn main_chrome_r6_menu_overlap_rejects_new_click_and_name_continuation() {
        for kind in [R6Menu::Grid, R6Menu::Bookmark, R6Menu::ReadingHistory] {
            for continuation in [false, true] {
                for arrow in [false, true] {
                    if continuation && arrow {
                        continue;
                    }
                    let (mut app, ctx, mut frame, target) = r6_menu_fixture(kind);
                    let pos = r6_tree_pos(&ctx, &target, arrow);
                    if continuation {
                        assert!(
                            r6_menu_frame(
                                &mut app,
                                &ctx,
                                &mut frame,
                                vec![egui::Event::PointerMoved(pos), r6_primary(pos, true)]
                            )
                            .is_none()
                        );
                        assert_eq!(
                            r6_menu_frame(&mut app, &ctx, &mut frame, vec![r6_primary(pos, false)]),
                            Some(crate::folder_pane::FolderPaneActivation::Open(
                                target.clone()
                            ))
                        );
                    }
                    // Aim at the Window frame, then prove the actual capture
                    // is non-command before delivering a release. Geometry or
                    // sharing the menu layer alone is not a safety guarantee.
                    r6_open_menu(&mut app, &ctx, &mut frame, kind, pos - egui::vec2(2.0, 2.0));
                    let layer = r6_menu_layer(kind);
                    assert_eq!(ctx.layer_id_at(pos), Some(layer));
                    assert!(
                        r6_menu_frame(
                            &mut app,
                            &ctx,
                            &mut frame,
                            vec![egui::Event::PointerMoved(pos), r6_primary(pos, true)]
                        )
                        .is_none()
                    );
                    let witnesses = ctx.pointer_button_targets();
                    assert_eq!(witnesses.len(), 1);
                    assert!(
                        witnesses[0].3.is_none_or(|widget| ctx
                            .read_response(widget)
                            .is_some_and(|response| response.layer_id == layer)),
                        "capture belongs to the real menu, never the tree"
                    );
                    if let Some(widget) = witnesses[0].3 {
                        let response = ctx
                            .read_response(widget)
                            .expect("captured widget must still have a response");
                        // Area::begin registers its background/move widget as
                        // layer_id.id.with("move"). That widget only moves or
                        // raises the Area; it is not a menu-command button.
                        // A non-CLICK widget is also incapable of dispatching a
                        // button command. Refuse the release for every other id.
                        let safe_background = widget == layer.id.with("move");
                        assert!(
                            safe_background || !response.sense.senses_click(),
                            "refuse release on a possible command: widget={widget:?} layer={:?} sense={:?}",
                            response.layer_id,
                            response.sense
                        );
                    }
                    assert!(
                        !app.folder_pane
                            .visible_rows()
                            .iter()
                            .find(|row| row.path == target)
                            .unwrap()
                            .expanded
                    );
                    assert!(!app.folder_pane.label_gesture_requires_tree_focus());
                    assert!(
                        r6_menu_frame(&mut app, &ctx, &mut frame, vec![r6_primary(pos, false)])
                            .is_none()
                    );
                }
            }
        }
    }

    #[test]
    fn main_chrome_r6_menu_owner_started_after_input_does_not_block_tree_results() {
        for result in ["single", "arrow", "toggle"] {
            let (mut app, ctx, mut frame, target) = r6_menu_fixture(R6Menu::Grid);
            let pos = r6_tree_pos(&ctx, &target, result == "arrow");
            assert!(
                r6_menu_frame(
                    &mut app,
                    &ctx,
                    &mut frame,
                    vec![egui::Event::PointerMoved(pos), r6_primary(pos, true)]
                )
                .is_none()
            );
            if result == "toggle" {
                assert_eq!(
                    r6_menu_frame(&mut app, &ctx, &mut frame, vec![r6_primary(pos, false)]),
                    Some(crate::folder_pane::FolderPaneActivation::Open(
                        target.clone()
                    ))
                );
            }
            frame += 1;
            let mut nav = None;
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1600.0, 700.0),
                    )),
                    time: Some(frame as f64 / 60.0),
                    events: vec![r6_primary(pos, result == "toggle")],
                    ..Default::default()
                },
                |ui| {
                    app.input_update_begin(&ctx);
                    app.update_ime_state(&ctx);
                    let _ = app.input_root_keyboard(&ctx);
                    assert!(app.show_context_menu(&ctx).is_none());
                    let _ = app.render_toolbar(ui);
                    // A later producer can reserve a menu after the menu-drawing
                    // checkpoint. Its existence must not reject accepted chrome input.
                    app.context_menu_pos = egui::pos2(800.0, 100.0);
                    let owner = app.capture_grid_context_menu_owner(0);
                    assert!(app.grid_context_menu_owner_is_current(owner));
                    app.context_menu_idx = Some(owner);
                    nav = app.render_main_bars(ui, &mut None).0;
                    app.finish_folder_pane_presentation(&ctx, true);
                },
            )
            .drop_without_applying_deltas();
            if result == "single" {
                assert_eq!(
                    nav,
                    Some(crate::folder_pane::FolderPaneActivation::Open(
                        target.clone()
                    ))
                );
            } else {
                assert!(nav.is_none());
                assert!(
                    app.folder_pane
                        .visible_rows()
                        .iter()
                        .any(|row| row.path == target && row.expanded),
                    "menu existence must not block {result} application"
                );
            }
        }
    }
}
