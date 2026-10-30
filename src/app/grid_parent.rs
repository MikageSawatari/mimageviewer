//! Resolve the meaning of a grid parent input before performing any side effects.
//!
//! An owned root stops here explicitly; absence of a navigation target must never
//! make a synthetic root fall through to its backing directory's physical parent.

use super::{App, StartupListIntent};
use crate::ui_main::AddressBarNav;

#[derive(Debug)]
pub(crate) enum GridParentAction {
    NoOp,
    SnapshotList,
    GlobalSearchBack,
    FavoriteSearchBack,
    TagBack,
    RatingBack,
    LocalFilterBlocked,
    ZipBack,
    Navigation {
        nav: AddressBarNav,
        child_selection: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::phase_c_support::setup_app;
    use crate::ring_shortcut::{
        MouseButtonSlot, MouseGestureBinding, MouseGestureDirection, RightDragContext,
        RingActionId, RingShortcutContext,
    };
    use std::path::{Path, PathBuf};

    const INPUTS: [&str; 8] = [
        "key",
        "custom-key",
        "stack-key",
        "pad",
        "mouse",
        "ring",
        "gesture",
        "background",
    ];

    fn key_input(app: &mut App, key: egui::Key, stack_refresh: bool) -> Option<AddressBarNav> {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        });
        let nav = if stack_refresh {
            // handle_keyboard publishes this pass's KeyboardOwner before entering
            // its stack allowlist; calling the inner adapter must do the same.
            assert!(!app.shortcuts_blocked_by_text_input(&ctx));
            app.handle_stack_refresh_keyboard(&ctx, true, 0, 0)
        } else {
            app.handle_keyboard(&ctx)
        };
        ctx.end_pass().drop_without_applying_deltas();
        nav
    }

    fn parent_input(app: &mut App, input: &str) -> Option<AddressBarNav> {
        let ctx = egui::Context::default();
        match input {
            "key" => key_input(app, egui::Key::Backspace, false),
            "custom-key" => {
                app.keymap =
                    crate::keymap::Keymap::from_ini_str("[Grid]\nGridParentFolder = F13\n");
                assert!(key_input(app, egui::Key::Backspace, false).is_none());
                key_input(app, egui::Key::F13, false)
            }
            "stack-key" => key_input(app, egui::Key::Backspace, true),
            "pad" => {
                app.settings.gamepad_enabled = true;
                app.gamepad
                    .push_for_test(crate::gamepad::PadEvent::ButtonPressed(
                        crate::gamepad::PadButton::East,
                    ));
                app.gamepad
                    .push_for_test(crate::gamepad::PadEvent::ButtonReleased(
                        crate::gamepad::PadButton::East,
                    ));
                let batch = app.sample_gamepad_input_with_logic_for_test(&ctx);
                app.dispatch_gamepad_batch(&ctx, batch).nav
            }
            "mouse" => {
                app.settings
                    .ring_shortcuts
                    .mouse_button_profile_mut(RingShortcutContext::Grid)
                    .back = RingActionId::GridParentFolder;
                app.apply_mouse_button(
                    &ctx,
                    MouseButtonSlot::Back,
                    super::super::ActionSurface::MainWindow,
                    "section1473-router-test",
                )
            }
            "ring" => app.apply_ring_action(
                &ctx,
                RingShortcutContext::Grid,
                RingActionId::GridParentFolder,
                "section1473-router-test",
            ),
            "gesture" => {
                app.settings.ring_shortcuts.mouse_gestures_grid.bindings =
                    vec![MouseGestureBinding::new(
                        vec![MouseGestureDirection::Up],
                        RingActionId::GridParentFolder,
                    )];
                app.trigger_mouse_gesture_action(
                    &ctx,
                    RightDragContext::Grid,
                    &[MouseGestureDirection::Up],
                )
            }
            // The real pointer-pair adapter has its own ui_main headless tests in both modes.
            "background" => app.handle_grid_parent_folder_action(),
            _ => unreachable!(),
        }
    }

    fn configure(app: &mut App, mode: crate::settings::GridViewMode) {
        app.active_quick_folder_slot = None;
        app.settings.grid_view_mode = mode;
        app.settings.auto_fullscreen_zip_pdf = false;
        app.settings.auto_fullscreen_image_folders = false;
    }

    fn preview_state(app: &App) -> String {
        format!(
            "{:?}/{:?}/{:?}/{:?}/{:?}/{:?}/{:?}/{:?}",
            app.current_folder,
            app.selected,
            app.select_after_load,
            app.items,
            app.tag_view.nav_stack,
            app.folder_nav_back_stack,
            app.folder_nav_forward_stack,
            app.zip_nav.as_ref().map(|nav| nav.current().to_vec()),
        )
    }

    #[test]
    fn section1473_router_preview_is_readonly_and_physical_selection_matches_execution() {
        #[cfg(windows)]
        let _focus = crate::test_native_focus::scoped(1, 1, true);
        for mode in [
            crate::settings::GridViewMode::Thumbnail,
            crate::settings::GridViewMode::Details,
        ] {
            for input in INPUTS {
                let mut app = setup_app();
                configure(&mut app, mode);
                let origin = app.tmp.path().join("library");
                let child = origin.join("child");
                app.current_folder = Some(child);
                app.select_after_load = Some("unchanged-preview".into());
                let before = preview_state(&app);
                let action = app.grid_parent_action();
                assert!(action.available());
                assert!(
                    matches!(action.into_nav_target(), Some(AddressBarNav::Direct(ref path, _)) if path == &origin)
                );
                assert_eq!(before, preview_state(&app), "{mode:?}/{input}");
                let nav = parent_input(&mut app, input);
                assert!(
                    matches!(nav, Some(AddressBarNav::Direct(ref path, _)) if path == &origin),
                    "{mode:?}/{input}: {nav:?}"
                );
                assert_eq!(app.select_after_load.as_deref(), Some("child"));
            }
        }
    }

    #[test]
    fn section1473_router_tag_root_stops_and_history_root_exits_for_all_adapters() {
        #[cfg(windows)]
        let _focus = crate::test_native_focus::scoped(1, 1, true);
        for mode in [
            crate::settings::GridViewMode::Thumbnail,
            crate::settings::GridViewMode::Details,
        ] {
            for input in INPUTS {
                for tag in [true, false] {
                    let mut app = setup_app();
                    configure(&mut app, mode);
                    app.current_folder = Some(crate::data_dir::get().join(if tag {
                        "__tag_view__"
                    } else {
                        "__reading_history__"
                    }));
                    if tag {
                        app.tag_view.active = true;
                        app.items_are_tag_view = true;
                    } else {
                        app.items_are_reading_history_view = true;
                        app.top_level_grid_view.replace_surface(super::super::top_level_grid_view::TopLevelGridSurface::ReadingHistory(
                            super::super::top_level_grid_view::ReadingHistoryRootRestore::unavailable(),
                        ));
                    }
                    let before = preview_state(&app);
                    let action = app.grid_parent_action();
                    assert_eq!(action.available(), !tag);
                    if tag {
                        assert!(matches!(action, GridParentAction::NoOp));
                    } else {
                        assert!(matches!(
                            action.into_nav_target(),
                            Some(AddressBarNav::ReadingHistoryRootExit)
                        ));
                    }
                    assert_eq!(before, preview_state(&app));
                    let nav = parent_input(&mut app, input);
                    assert!(
                        if tag {
                            nav.is_none()
                        } else {
                            matches!(nav, Some(AddressBarNav::ReadingHistoryRootExit))
                        },
                        "{mode:?}/{input}/tag={tag}: {nav:?}"
                    );
                    assert_eq!(
                        before,
                        preview_state(&app),
                        "root input must only produce intent"
                    );
                }
            }
        }
    }

    fn tag_child(app: &mut App, path: &Path, chain: Vec<PathBuf>) {
        app.current_folder = Some(path.to_path_buf());
        app.tag_view.active = true;
        app.items_are_tag_view = false;
        app.tag_view.query = "section1473-router-tag".into();
        app.tag_view.nav_stack = chain;
    }

    fn finish_tag_root(app: &mut App, case: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.tag_view_pending.is_some() {
            app.poll_tag_view();
            assert!(
                std::time::Instant::now() < deadline,
                "Tag root did not adopt"
            );
            std::thread::yield_now();
        }
        assert!(app.tag_view.active);
        assert!(
            app.items_are_tag_view,
            "{case}: query={:?} executed={:?} reject={:?} chain={:?} folder={:?} surface={:?}",
            app.tag_view.query,
            app.tag_view.last_executed,
            app.tag_view.reject_message,
            app.tag_view.nav_stack,
            app.current_folder,
            app.top_level_grid_view.surface()
        );
        assert!(app.tag_view.nav_stack.is_empty());
        assert!(app.zip_nav.is_none());
        assert!(matches!(app.grid_parent_action(), GridParentAction::NoOp));
    }

    #[test]
    fn section1473_router_tag_children_use_owner_and_zip_moves_only_one_level() {
        #[cfg(windows)]
        let _focus = crate::test_native_focus::scoped(1, 1, true);
        for mode in [
            crate::settings::GridViewMode::Thumbnail,
            crate::settings::GridViewMode::Details,
        ] {
            for input in INPUTS {
                let mut app = setup_app();
                configure(&mut app, mode);
                let root = app.tmp.path().join("tag-folder");
                let child = root.join("child");
                std::fs::create_dir_all(&child).unwrap();
                app.open_tag_view_for_tag("section1473-router-tag");
                finish_tag_root(&mut app, &format!("{mode:?}/{input}/initial"));
                tag_child(&mut app, &child, vec![root.clone(), child.clone()]);
                let before = preview_state(&app);
                assert!(matches!(
                    app.grid_parent_action(),
                    GridParentAction::TagBack
                ));
                assert_eq!(before, preview_state(&app));
                assert!(parent_input(&mut app, input).is_none());
                app.finish_page_identity_preparation_for_test();
                assert_eq!(app.current_folder.as_ref(), Some(&root), "{mode:?}/{input}");
                assert_eq!(app.tag_view.nav_stack, vec![root]);
                assert!(app.tag_view.active);
                assert!(parent_input(&mut app, input).is_none());
                finish_tag_root(&mut app, &format!("{mode:?}/{input}/folder"));

                let zip = app.tmp.path().join("tag-book.zip");
                let entries = ["root.png", "chapter/page.png"]
                    .into_iter()
                    .map(|name| crate::zip_loader::ZipImageEntry {
                        entry_name: name.into(),
                        uncompressed_size: 0,
                        mtime: 0,
                        container_ancestry: Vec::new(),
                    })
                    .collect();
                let tree =
                    std::sync::Arc::new(crate::zip_tree::ZipTree::build(zip.clone(), entries));
                let mut zip_nav = crate::zip_tree::ZipNavState::new(tree);
                zip_nav.enter("chapter/");
                app.zip_nav = Some(zip_nav);
                tag_child(&mut app, &zip, vec![zip.clone()]);
                assert!(parent_input(&mut app, input).is_none());
                assert!(app.zip_nav.as_ref().unwrap().at_root(), "{mode:?}/{input}");
                assert_eq!(
                    app.tag_view.nav_stack,
                    vec![zip.clone()],
                    "ZIP Back must not also pop Tag owner"
                );
                assert_eq!(app.current_folder.as_ref(), Some(&zip));
                assert!(app.tag_view.active);
                assert!(parent_input(&mut app, input).is_none());
                finish_tag_root(&mut app, &format!("{mode:?}/{input}/zip"));
            }
        }
    }
}

impl GridParentAction {
    pub(crate) fn available(&self) -> bool {
        !matches!(self, Self::NoOp | Self::LocalFilterBlocked)
    }

    pub(crate) fn into_nav_target(self) -> Option<AddressBarNav> {
        match self {
            Self::Navigation { nav, .. } => Some(nav),
            Self::RatingBack => Some(AddressBarNav::RatingViewBack),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum GridParentPresentation {
    /// The existing key and grid-background path owns these viewer close operations.
    KeyOrBackground,
    Other,
}

impl App {
    pub(crate) fn grid_parent_action(&self) -> GridParentAction {
        if self.is_snapshot_active()
            // A history root uses its typed exit below, ending the fixed scope.
            // Keep the other parent owners and their input restrictions in order.
            && self.reading_history_root_exit_nav().is_none()
            && let Some(snapshot) = self.snapshot.as_ref()
            && !self.current_folder.as_ref().is_some_and(|current| {
                crate::snapshot::snapshot_key_from_path(current)
                    == crate::snapshot::snapshot_key_from_path(&snapshot.origin)
            })
        {
            return GridParentAction::SnapshotList;
        }
        if self.global_search.active {
            return if self.global_search.drill.is_some() || self.everything_has_physical_child() {
                GridParentAction::GlobalSearchBack
            } else {
                GridParentAction::NoOp
            };
        }
        if self.favsearch.active {
            return if self.favsearch.nav_stack.is_empty() {
                GridParentAction::NoOp
            } else {
                GridParentAction::FavoriteSearchBack
            };
        }
        if self.tag_view.active {
            return if self.tag_view.nav_stack.is_empty() {
                GridParentAction::NoOp
            } else {
                GridParentAction::TagBack
            };
        }
        if self.rating_view_parent_nav_available() {
            return GridParentAction::RatingBack;
        }
        if self.local_search_blocks_parent_nav() {
            return GridParentAction::LocalFilterBlocked;
        }
        if self.zip_nav.as_ref().is_some_and(|nav| !nav.at_root()) {
            return GridParentAction::ZipBack;
        }
        self.grid_parent_location_action()
    }

    fn grid_parent_location_action(&self) -> GridParentAction {
        if let Some((nav, child_selection)) = self.collection_grid_parent_navigation() {
            return GridParentAction::Navigation {
                nav,
                child_selection,
            };
        }
        if self
            .top_level_grid_view
            .collection_session()
            .is_some_and(|session| {
                matches!(
                    session.position,
                    super::top_level_grid_view::CollectionGridPosition::Root
                ) && !matches!(
                    session.load,
                    super::top_level_grid_view::CollectionGridLoadState::Deleted { .. }
                )
            })
        {
            return GridParentAction::NoOp;
        }
        if let Some(nav) = self.bookmark_view_back_nav() {
            return GridParentAction::Navigation {
                nav,
                child_selection: None,
            };
        }
        if let Some(nav) = self.reading_history_back_nav() {
            return GridParentAction::Navigation {
                nav,
                child_selection: None,
            };
        }
        if self.items_are_bookmark_view || self.bookmark_browser_pending.is_some() {
            return self
                .folder_history_back_target()
                .map_or(GridParentAction::NoOp, |_| GridParentAction::Navigation {
                    nav: AddressBarNav::HistoryBack,
                    child_selection: None,
                });
        }
        if let Some(nav) = self.subfolder_expansion_back_nav() {
            return GridParentAction::Navigation {
                nav,
                child_selection: None,
            };
        }
        if let Some(nav) = self.smart_folder_parent_nav_target() {
            return GridParentAction::Navigation {
                nav,
                child_selection: None,
            };
        }
        if self.items_are_smart_folder_view {
            return GridParentAction::NoOp;
        }
        if let Some(nav) = self.reading_history_root_exit_nav() {
            return GridParentAction::Navigation {
                nav,
                child_selection: None,
            };
        }
        self.physical_grid_parent_action()
    }

    /// Only the physical child operation. Everything has already checked its own
    /// scope when invoking this, so it must not re-enter the search owner above.
    fn physical_grid_parent_action(&self) -> GridParentAction {
        let Some(current) = self.effective_folder() else {
            return GridParentAction::NoOp;
        };
        let (nav, child_selection) = if let Some(parent) = current.parent() {
            (
                AddressBarNav::Direct(parent.to_path_buf(), StartupListIntent::ExplicitList),
                current
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned),
            )
        } else {
            (AddressBarNav::DriveList(Some(current)), None)
        };
        GridParentAction::Navigation {
            nav,
            child_selection,
        }
    }

    pub(crate) fn grid_parent_nav_target(&self) -> Option<AddressBarNav> {
        self.grid_parent_action().into_nav_target()
    }

    pub(crate) fn resolve_grid_parent_nav(&mut self) -> Option<AddressBarNav> {
        self.handle_grid_parent_action()
    }

    pub(crate) fn resolve_physical_grid_parent_nav(&mut self) -> Option<AddressBarNav> {
        let action = self.physical_grid_parent_action();
        self.execute_grid_parent_action(action, GridParentPresentation::Other)
    }

    /// Keyboard and background activation retain their existing presentation cleanup.
    pub(crate) fn handle_grid_parent_folder_action(&mut self) -> Option<AddressBarNav> {
        let action = self.grid_parent_action();
        self.execute_grid_parent_action(action, GridParentPresentation::KeyOrBackground)
    }

    /// Mouse/ring/gesture/Pad and the parent button share the same semantic owner.
    pub(crate) fn handle_grid_parent_action(&mut self) -> Option<AddressBarNav> {
        let action = self.grid_parent_action();
        self.execute_grid_parent_action(action, GridParentPresentation::Other)
    }

    pub(crate) fn handle_stack_refresh_parent_action(&mut self) -> Option<AddressBarNav> {
        self.handle_grid_parent_action()
    }

    fn execute_grid_parent_action(
        &mut self,
        action: GridParentAction,
        presentation: GridParentPresentation,
    ) -> Option<AddressBarNav> {
        match action {
            GridParentAction::NoOp => None,
            GridParentAction::SnapshotList => {
                self.snapshot_return_to_list_view();
                None
            }
            GridParentAction::GlobalSearchBack => {
                self.drill_back_one_level();
                None
            }
            GridParentAction::FavoriteSearchBack => {
                self.favsearch_back();
                None
            }
            GridParentAction::TagBack => {
                self.tag_view_back();
                None
            }
            GridParentAction::RatingBack => {
                self.rating_view_parent_nav();
                None
            }
            GridParentAction::LocalFilterBlocked => {
                self.cancel_pending_folder_nav();
                None
            }
            GridParentAction::ZipBack => {
                #[cfg(windows)]
                if matches!(presentation, GridParentPresentation::KeyOrBackground)
                    && self.should_preserve_active_detached_image_window_for_main_context_change()
                {
                    self.preserve_active_detached_image_window_for_main_context_change();
                    self.retire_direct_media_reader_for_source_adoption();
                    self.close_fullscreen_content();
                }
                self.zip_nav_back();
                None
            }
            GridParentAction::Navigation {
                nav,
                child_selection,
            } => {
                #[cfg(windows)]
                if matches!(presentation, GridParentPresentation::KeyOrBackground) {
                    self.close_detached_viewer_for_virtual_page_list_parent_nav();
                }
                if child_selection.is_some() {
                    self.select_after_load = child_selection;
                }
                Some(nav)
            }
        }
    }
}
