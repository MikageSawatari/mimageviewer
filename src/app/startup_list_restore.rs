//! Main's last explicitly adopted restorable list. Load execution and window placement do not
//! own this record; all semantic adoptions and cursor updates pass through this reducer.

use super::*;
use crate::settings::{ListCursorHint, StartupListRestore, StartupListTarget};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StartupListIntent {
    /// Hydration has not yet received the caller's semantic adoption tail.
    InternalInstall,
    /// Hydrate rows with the original target (notably ZIP prefix), while the existing
    /// source/surface adoption tail still owns acceptance of this same request.
    InternalHydration(Box<StartupListIntent>),
    ExplicitList,
    PageContinuation,
    ClassifyFolder,
    PreservePresentation,
    RestoreList {
        target: StartupListTarget,
        cursor: Option<ListCursorHint>,
    },
}

impl StartupListIntent {
    pub(crate) fn captures_list_departure(&self) -> bool {
        matches!(
            self,
            Self::ExplicitList | Self::RestoreList { .. } | Self::ClassifyFolder
        )
    }

    pub(crate) fn materialization_intent(&self) -> &Self {
        match self {
            Self::InternalHydration(intent) => intent.materialization_intent(),
            intent => intent,
        }
    }

    pub(crate) fn folder_navigation(mode: &FolderNavMode) -> Self {
        match mode {
            FolderNavMode::Grid | FolderNavMode::SiblingGrid => Self::ExplicitList,
            FolderNavMode::Favsearch {
                fullscreen: false, ..
            }
            | FolderNavMode::SmartFolder {
                fullscreen: false, ..
            } => Self::ExplicitList,
            _ => Self::PageContinuation,
        }
    }
    pub(crate) fn container_open(auto_open: bool) -> Self {
        if auto_open {
            Self::ClassifyFolder
        } else {
            Self::ExplicitList
        }
    }

    pub(crate) fn for_book(self) -> Self {
        match self {
            Self::ClassifyFolder => Self::PageContinuation,
            intent => intent,
        }
    }

    pub(crate) fn for_scanned_folder(self, auto_open: bool) -> Self {
        match self {
            Self::ClassifyFolder if auto_open => Self::PageContinuation,
            Self::ClassifyFolder => Self::ExplicitList,
            intent => intent,
        }
    }
}

enum MainListRestoreEvent {
    Adopt {
        target: StartupListTarget,
        cursor: Option<ListCursorHint>,
    },
    CaptureCursor {
        target: StartupListTarget,
        cursor: Option<ListCursorHint>,
    },
}

impl App {
    pub(crate) fn open_previous_startup_list(&mut self) {
        let StartupListRestore::V1 { target, cursor } = self
            .settings
            .startup_list_restore
            .clone()
            .unwrap_or_default();
        if target == StartupListTarget::DriveList {
            self.enter_drive_list(None);
            return;
        }
        let path = match &target {
            StartupListTarget::PhysicalList { logical_path, .. } => Some(logical_path.as_path()),
            _ => None,
        };
        let Some(folder) = crate::known_folders::startup_folder(
            crate::settings::StartupFolderMode::Previous,
            path,
            None,
        ) else {
            return;
        };
        let (target, cursor) =
            if path.is_some_and(|path| crate::folder_tree::path_eq(path, &folder)) {
                (target, cursor)
            } else {
                (
                    StartupListTarget::PhysicalList {
                        logical_path: folder.clone(),
                        zip_prefix: None,
                    },
                    None,
                )
            };
        let _ = self.load_folder_or_convert_archive_with_auto_fullscreen_owned(
            folder,
            false,
            OpenRequestOwner::Navigation,
            StartupListIntent::RestoreList { target, cursor },
        );
    }

    fn restorable_current_main_list(&self) -> Option<StartupListTarget> {
        use top_level_grid_view::{
            CollectionGridPosition, SmartFolderPosition, TopLevelGridSurface,
        };
        if !self.main_folder_history_available() {
            return None;
        }
        if self.is_snapshot_active()
            || self.sidecar_restore_active()
            || (self.zip_enumerate_pending.is_some() && self.zip_nav.is_none())
        {
            return None;
        }
        if self.items_are_drive_list {
            return matches!(
                self.top_level_grid_view.surface(),
                TopLevelGridSurface::DriveList
            )
            .then_some(StartupListTarget::DriveList);
        }
        if self.items_are_global_search_view
            || self.items_are_tag_view
            || self.items_are_reading_history_view
            || self.items_are_bookmark_view
            || self.items_are_rating_view
            || self.items_are_subfolder_expansion_view
            || self.items_are_smart_folder_view
        {
            return None;
        }
        let logical_path = self.effective_folder()?;
        if is_synthetic_view_path(&logical_path) {
            return None;
        }
        let position_matches = match self.top_level_grid_view.surface() {
            TopLevelGridSurface::Folder => true,
            TopLevelGridSurface::SmartFolder(state) => match &state.position {
                SmartFolderPosition::Root => false,
                SmartFolderPosition::Container { current, .. }
                | SmartFolderPosition::Scoped { current, .. } => {
                    crate::folder_tree::path_eq(current, &logical_path)
                }
            },
            TopLevelGridSurface::Collection(_) => self
                .top_level_grid_view
                .collection_session()
                .is_some_and(|session| match &session.position {
                    CollectionGridPosition::Root => false,
                    CollectionGridPosition::PhysicalSource { .. } => {
                        matches!(
                            session.load,
                            top_level_grid_view::CollectionGridLoadState::Deleted { .. }
                        ) || self
                            .collection_grid_physical_reload_owner(&logical_path)
                            .is_some_and(|owner| {
                                self.collection_grid_physical_load_owner_is_current(
                                    &owner,
                                    &logical_path,
                                )
                            })
                    }
                }),
            TopLevelGridSurface::Search(top_level_grid_view::TopLevelSearchView::Global) => {
                self.global_search.drill.as_ref().is_some_and(|drill| {
                    crate::folder_tree::path_eq(&drill.current_path, &logical_path)
                })
            }
            TopLevelGridSurface::Search(top_level_grid_view::TopLevelSearchView::Favorite) => self
                .favsearch
                .nav_stack
                .last()
                .is_some_and(|path| crate::folder_tree::path_eq(path, &logical_path)),
            TopLevelGridSurface::Search(top_level_grid_view::TopLevelSearchView::Tag) => self
                .tag_view
                .nav_stack
                .last()
                .is_some_and(|path| crate::folder_tree::path_eq(path, &logical_path)),
            _ => false,
        };
        if !position_matches {
            return None;
        }
        let zip_prefix = if let Some(nav) = self.zip_nav.as_ref() {
            if self
                .current_folder
                .as_ref()
                .is_none_or(|path| !crate::folder_tree::path_eq(path, &nav.tree.zip_path))
            {
                return None;
            }
            Some(zip_prefix_string(nav.current()))
        } else {
            None
        };
        Some(StartupListTarget::PhysicalList {
            logical_path,
            zip_prefix,
        })
    }

    fn current_main_list_cursor(&self, target: &StartupListTarget) -> Option<ListCursorHint> {
        if matches!(target, StartupListTarget::DriveList) {
            return None;
        }
        let selected = self.selected?;
        let name = self.items.get(selected)?.name().to_string();
        (!name.is_empty()).then(|| ListCursorHint {
            name,
            rows_above: Some(
                self.scroll_selected_to_rows_above
                    .unwrap_or_else(|| self.selected_rows_below_view_top(selected)),
            ),
        })
    }

    fn commit_main_list_restore(&mut self, event: MainListRestoreEvent) {
        match event {
            MainListRestoreEvent::Adopt { target, cursor } => {
                self.settings.startup_list_restore =
                    Some(StartupListRestore::V1 { target, cursor });
            }
            MainListRestoreEvent::CaptureCursor { target, cursor } => {
                if let Some(StartupListRestore::V1 {
                    target: saved,
                    cursor: saved_cursor,
                }) = self.settings.startup_list_restore.as_mut()
                    && same_list_target(saved, &target)
                {
                    *saved_cursor = cursor;
                }
            }
        }
    }

    pub(crate) fn capture_main_list_restore_cursor(&mut self) {
        // A linked window can paint main's grid while its logical presentation is still a page.
        if self
            .fs_holdover_tex
            .as_ref()
            .and_then(FsHoldover::navigation_sequence)
            .is_some()
            || self.fs_nav_locked_gen.is_some()
            || self.fullscreen_idx.is_some()
            || self.stack_showing_flat
            || self.sidecar_restore_active()
        {
            return;
        }
        if let Some(target) = self.restorable_current_main_list() {
            let cursor = self.current_main_list_cursor(&target);
            self.commit_main_list_restore(MainListRestoreEvent::CaptureCursor { target, cursor });
        }
    }

    /// Called only after an input caller has accepted showing the already adopted page list.
    /// The record intentionally survives quitting before a deferred TerminalClose finishes.
    pub(super) fn accept_current_page_list_request(&mut self) {
        if self.fullscreen_idx.is_none() {
            return;
        }
        let Some(target) = self.restorable_current_main_list() else {
            return;
        };
        // Fullscreen's selected row may still be the original grid click. Derive precisely
        // the anchor used by close, without changing selection before that existing teardown.
        // Filename-stack close materializes aggregate rows later; flat names are not hints.
        let cursor = if self.stack_showing_flat {
            None
        } else {
            self.page_edit_navigation_anchor_idx()
                .or(self.fullscreen_idx)
                .filter(|&idx| self.items.get(idx).is_some())
                .and_then(|idx| {
                    let visible = &self.visible_indices;
                    if visible.binary_search(&idx).is_ok() {
                        Some(idx)
                    } else {
                        let pos = visible.partition_point(|&i| i < idx);
                        pos.checked_sub(1)
                            .map(|p| visible[p])
                            .or_else(|| visible.get(pos).copied())
                    }
                })
                .map(|idx| ListCursorHint {
                    name: self.items[idx].name().to_string(),
                    rows_above: None,
                })
        };
        self.commit_main_list_restore(MainListRestoreEvent::Adopt { target, cursor });
        self.settings.save();
    }

    pub(crate) fn close_fullscreen_to_page_list(&mut self) {
        self.accept_current_page_list_request();
        self.close_fullscreen();
    }

    pub(crate) fn finish_main_list_open(&mut self, intent: StartupListIntent) {
        if matches!(
            intent,
            StartupListIntent::InternalInstall | StartupListIntent::InternalHydration(_)
        ) || !self.main_folder_history_available()
        {
            return;
        }
        if self.defer_startup_list_adoption(&intent) {
            return;
        }
        let Some(target) = self.restorable_current_main_list() else {
            return;
        };
        if !matches!(
            intent,
            StartupListIntent::ExplicitList | StartupListIntent::RestoreList { .. }
        ) {
            self.settings.save();
            return;
        }
        if let StartupListIntent::RestoreList {
            target: requested,
            cursor,
        } = intent
            && restore_cursor_matches(&requested, &target)
            && self.settings.restore_last_cursor
            && let Some(cursor) = cursor
        {
            self.select_after_load = Some(cursor.name);
            self.scroll_selected_to_rows_above = cursor.rows_above;
            self.try_select_after_load();
        }
        let cursor = self.current_main_list_cursor(&target);
        self.commit_main_list_restore(MainListRestoreEvent::Adopt { target, cursor });
        self.settings.save();
    }
}

pub(super) fn zip_prefix_string(prefix: &[String]) -> String {
    if prefix.is_empty() {
        String::new()
    } else {
        format!("{}/", prefix.join("/"))
    }
}

fn same_list_target(left: &StartupListTarget, right: &StartupListTarget) -> bool {
    match (left, right) {
        (
            StartupListTarget::PhysicalList {
                logical_path: a,
                zip_prefix: ap,
            },
            StartupListTarget::PhysicalList {
                logical_path: b,
                zip_prefix: bp,
            },
        ) => crate::folder_tree::path_eq(a, b) && ap == bp,
        _ => left == right,
    }
}

fn restore_cursor_matches(requested: &StartupListTarget, actual: &StartupListTarget) -> bool {
    match (requested, actual) {
        // Prefix-less released book data denotes its initial (possibly collapsed) root.
        (
            StartupListTarget::PhysicalList {
                logical_path: a,
                zip_prefix: None,
            },
            StartupListTarget::PhysicalList {
                logical_path: b, ..
            },
        ) => crate::folder_tree::path_eq(a, b),
        _ => same_list_target(requested, actual),
    }
}
