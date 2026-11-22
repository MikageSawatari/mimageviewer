//! Read the mounted main context in the existing root UI admission phase.
//! No detached predicates, viewport changes, filesystem probes or DB reads.
use super::*;
use mimageviewer_agent_ipc::{
    AgentError, AgentLevel, ErrorCode, ItemKind, ViewFavorite, ViewItem, ViewKind, ViewSearch,
    ViewState,
};

impl App {
    pub(super) fn agent_view_state(
        &self,
        default_books_root: &Path,
    ) -> Result<ViewState, AgentError> {
        if self.settings.agent_level < AgentLevel::ReadOnly
            || !self.settings.agent_show_current_view
        {
            return Err(AgentError::new(
                ErrorCode::Disabled,
                crate::agent_ipc::CURRENT_VIEW_PERMISSION_MESSAGE,
            ));
        }
        if !self.initialized || self.indexer_init.overlay_active() {
            return Err(AgentError::new(ErrorCode::NotReady, "mIV の起動処理中です"));
        }
        if let Some(session) = self.remote_session_handle() {
            // A short, read-only snapshot under the same admission boundary as
            // other local operations. Remote cannot acquire midway through it.
            session
                .with_local_composition_admission(|| {
                    self.capture_agent_view_state(default_books_root)
                })
                .map_err(|_| {
                    AgentError::new(ErrorCode::Busy, "リモート操作中は表示の状態を返しません")
                })
        } else {
            Ok(self.capture_agent_view_state(default_books_root))
        }
    }

    fn capture_agent_view_state(&self, default_books_root: &Path) -> ViewState {
        let folder = self
            .current_folder
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        let excluded = self
            .settings
            .book_root
            .as_deref()
            .unwrap_or(default_books_root)
            .to_path_buf();
        let ownership =
            crate::metadata_ownership::metadata_ownership(&self.settings.favorites, &[excluded]);
        let owner = self
            .current_folder
            .as_deref()
            .and_then(|p| ownership.owner(&crate::metadata_ownership::root_key(p)));
        let favorite = owner
            .and_then(|owner| self.settings.favorites.iter().find(|f| f.id == owner.id))
            .map(|f| ViewFavorite {
                path: f.path.to_string_lossy().into_owned(),
                name: f.name.clone(),
            });
        let favorite_root = self
            .current_folder
            .as_deref()
            .is_some_and(|p| self.settings.is_favorite(p));
        let item = self
            .fullscreen_idx
            .or(self.selected)
            .and_then(|idx| self.items.get(idx))
            .map(view_item);
        ViewState {
            view: self.agent_view_kind(),
            folder,
            fullscreen: self.fullscreen_idx.is_some(),
            item,
            search: ViewSearch {
                item_index: owner.is_some(),
                favorite,
                favorite_root,
            },
        }
    }

    fn agent_view_kind(&self) -> ViewKind {
        if self.items_are_global_search_view {
            ViewKind::SearchResults
        } else if self.items_are_tag_view {
            ViewKind::Tag
        } else if self.items_are_reading_history_view {
            ViewKind::ReadingHistory
        } else if self.items_are_bookmark_view {
            ViewKind::Bookmark
        } else if self.items_are_rating_view {
            ViewKind::Rating
        } else if self.items_are_smart_folder_view {
            ViewKind::SmartFolder
        } else if self.items_are_subfolder_expansion_view {
            ViewKind::SubfolderExpansion
        } else if self.items_are_drive_list {
            ViewKind::DriveList
        } else if self.current_folder.is_some()
            && matches!(
                self.top_level_grid_view.surface(),
                top_level_grid_view::TopLevelGridSurface::Folder
            )
        {
            ViewKind::Folder
        } else {
            ViewKind::Other
        }
    }
}

fn view_item(item: &GridItem) -> ViewItem {
    let (path, subresource, kind) = match item {
        GridItem::Folder(p) => (p, None, ItemKind::Folder),
        GridItem::Image(p) => (p, None, ItemKind::Image),
        GridItem::Video(p) => (p, None, ItemKind::Video),
        GridItem::Audio(p) => (p, None, ItemKind::Audio),
        GridItem::ZipFile(p) | GridItem::ConvertibleArchive { path: p, .. } => {
            (p, None, ItemKind::Archive)
        }
        GridItem::PdfFile(p) => (p, None, ItemKind::Pdf),
        GridItem::ZipImage {
            zip_path,
            entry_name,
        } => (zip_path, Some(entry_name.clone()), ItemKind::Image),
        GridItem::ZipDir {
            zip_path,
            dir_prefix,
            ..
        } => (zip_path, Some(dir_prefix.clone()), ItemKind::Folder),
        GridItem::PdfPage {
            pdf_path, page_num, ..
        } => (pdf_path, Some(format!("page_{page_num}")), ItemKind::Pdf),
        GridItem::SearchContainer { path, kind, .. } => (
            path,
            None,
            match kind {
                crate::grid_item::SearchContainerKind::Folder => ItemKind::Folder,
                crate::grid_item::SearchContainerKind::Zip => ItemKind::Archive,
            },
        ),
        // Match the grid's existing display_path for aggregate/presentation rows;
        // do not pretend the representative or unavailable source is an image.
        GridItem::Stack { representative, .. } => (representative, None, ItemKind::Other),
        GridItem::CollectionPlaceholder { path, .. } => (path, None, ItemKind::Other),
    };
    ViewItem {
        path: path.to_string_lossy().into_owned(),
        subresource,
        name: item.name().into_owned(),
        kind,
    }
}

#[cfg(test)]
#[path = "agent_view_state_tests.rs"]
mod tests;
