use super::App;

impl App {
    /// Dismiss background menus once when a real modal takes input ownership.
    pub(crate) fn note_main_chrome_pointer_block_transition(&mut self, ctx: &egui::Context) {
        let blocked = self.main_chrome_pointer_blocked();
        if blocked && !self.main_chrome_pointer_was_blocked {
            // Popups have independent Uis; disabling their anchor cannot revoke
            // an already open menu. Submenus keep a separate open-item owner.
            egui::Popup::close_all(ctx);
            ctx.data_mut(|data| data.remove_by_type::<egui::containers::menu::MenuState>());
        }
        self.main_chrome_pointer_was_blocked = blocked;
    }

    /// Hold background chrome input without dimming the whole main window.
    pub(crate) fn gate_main_chrome(&self, ui: &mut egui::Ui) {
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
        let destination = env.tmp.path().join("modal-tree");
        std::fs::create_dir_all(&destination).unwrap();
        env.current_folder = None;
        env.settings.folder_tree_pane_visible = true;
        env.folder_pane.select_drive(destination.clone());
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
            // Existing leaf expanders are blank buttons; this test owns input
            // routing and resize behavior, not the separate accessibility defect.
            .with_accessibility_check(false)
            .build_ui(move |root_ui| {
                let mut app = render.borrow_mut();
                app.note_main_chrome_pointer_block_transition(root_ui.ctx());
                let (path, rect) = app.render_folder_pane(root_ui);
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
                vec![destination.clone()]
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
}
