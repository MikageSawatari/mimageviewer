//! 最上位一覧 surface の所有権と復元先。
//!
//! 検索・snapshot・サブ展開・スマートフォルダなどは同じ `items` surface を共有する。
//! 個別の active flag は描画互換の派生情報として残すが、遷移の正本はこの型に集約する。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::subfolder_expansion::SubfolderExpansionRestoreState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TopLevelSearchView {
    Favorite,
    Global,
    Tag,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SmartFolderPosition {
    Root,
    Scoped {
        entry_index: usize,
        entry_root: PathBuf,
        current: PathBuf,
        back_stack: Vec<PathBuf>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SmartFolderViewState {
    pub(crate) definition_id: uuid::Uuid,
    /// prepare 済み root grid に現れた実フォルダの表示順。
    pub(crate) folder_entries: Arc<Vec<PathBuf>>,
    pub(crate) position: SmartFolderPosition,
}

impl SmartFolderViewState {
    pub(crate) fn root(definition_id: uuid::Uuid, folder_entries: Vec<PathBuf>) -> Self {
        Self {
            definition_id,
            folder_entries: Arc::new(folder_entries),
            position: SmartFolderPosition::Root,
        }
    }

    pub(crate) fn scoped_current(&self) -> Option<&Path> {
        match &self.position {
            SmartFolderPosition::Root => None,
            SmartFolderPosition::Scoped { current, .. } => Some(current),
        }
    }

    pub(crate) fn scoped_entry_root(&self) -> Option<&Path> {
        match &self.position {
            SmartFolderPosition::Root => None,
            SmartFolderPosition::Scoped { entry_root, .. } => Some(entry_root),
        }
    }

    #[cfg(test)]
    pub(crate) fn entry_index(&self) -> Option<usize> {
        match self.position {
            SmartFolderPosition::Root => None,
            SmartFolderPosition::Scoped { entry_index, .. } => Some(entry_index),
        }
    }

    /// `path` 自体が root entry、またはその子孫なら、その entry の scoped drill を開始する。
    /// 親子の entry が両方 root にある場合は exact match、次に最も深い祖先を優先する。
    pub(crate) fn enter_containing_path(&mut self, path: &Path) -> bool {
        let entry_index = self
            .folder_entries
            .iter()
            .position(|entry| crate::folder_tree::path_eq(entry, path))
            .or_else(|| {
                self.folder_entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| crate::search_index_db::is_under(path, entry))
                    .max_by_key(|(_, entry)| entry.components().count())
                    .map(|(index, _)| index)
            });
        let Some(entry_index) = entry_index else {
            return false;
        };
        let entry_root = self.folder_entries[entry_index].clone();
        self.position = SmartFolderPosition::Scoped {
            entry_index,
            entry_root: entry_root.clone(),
            current: entry_root,
            back_stack: Vec::new(),
        };
        self.move_to(path)
    }

    pub(crate) fn contains_scoped_path(&self, path: &Path) -> bool {
        self.scoped_entry_root()
            .is_some_and(|root| crate::search_index_db::is_under(path, root))
    }

    /// scoped drill 内の実フォルダ遷移を記録する。
    ///
    /// `path` は entry root 配下に限定し、直接の親移動なら stack を pop、子孫移動なら
    /// 現在地を stack に積む。Ctrl+↑↓ のような非隣接 DFS 遷移では stack を root から
    /// 再構築して、Backspace が常に scope 内の親へ戻るようにする。
    pub(crate) fn move_to(&mut self, path: &Path) -> bool {
        let SmartFolderPosition::Scoped {
            entry_root,
            current,
            back_stack,
            ..
        } = &mut self.position
        else {
            return false;
        };
        if !crate::search_index_db::is_under(path, entry_root) {
            return false;
        }
        if crate::folder_tree::path_eq(path, current) {
            return true;
        }
        if current
            .parent()
            .is_some_and(|parent| crate::folder_tree::path_eq(parent, path))
        {
            *current = path.to_path_buf();
            back_stack.pop();
            return true;
        }
        if path
            .parent()
            .is_some_and(|parent| crate::folder_tree::path_eq(parent, current))
        {
            back_stack.push(current.clone());
            *current = path.to_path_buf();
            return true;
        }
        let mut lineage = Vec::new();
        let mut cursor = path.parent();
        while let Some(parent) = cursor {
            if !crate::search_index_db::is_under(parent, entry_root) {
                return false;
            }
            lineage.push(parent.to_path_buf());
            if crate::folder_tree::path_eq(parent, entry_root) {
                break;
            }
            cursor = parent.parent();
        }
        if !lineage
            .last()
            .is_some_and(|root| crate::folder_tree::path_eq(root, entry_root))
        {
            return false;
        }
        lineage.reverse();
        *back_stack = lineage;
        *current = path.to_path_buf();
        true
    }

    pub(crate) fn parent_target(&self) -> Option<SmartFolderParentTarget> {
        let SmartFolderPosition::Scoped {
            entry_root,
            current,
            back_stack,
            ..
        } = &self.position
        else {
            return None;
        };
        if crate::folder_tree::path_eq(entry_root, current) {
            Some(SmartFolderParentTarget::Root)
        } else {
            back_stack
                .last()
                .cloned()
                .or_else(|| current.parent().map(Path::to_path_buf))
                .map(SmartFolderParentTarget::Folder)
        }
    }

    pub(crate) fn entry_at_offset(&self, forward: bool) -> Option<&Path> {
        let index = match self.position {
            SmartFolderPosition::Root if forward => 0,
            SmartFolderPosition::Root => self.folder_entries.len().checked_sub(1)?,
            SmartFolderPosition::Scoped { entry_index, .. } if forward => {
                entry_index.checked_add(1)?
            }
            SmartFolderPosition::Scoped { entry_index, .. } => entry_index.checked_sub(1)?,
        };
        self.folder_entries.get(index).map(PathBuf::as_path)
    }

    /// root 再準備後の表示順を取り込む。現在 entry が同じ path として残っていれば
    /// scope と現在地を維持し、削除・リネームで見つからなければ安全に root へ戻す。
    pub(crate) fn refresh_folder_entries(&mut self, folder_entries: Vec<PathBuf>) -> bool {
        let retained_index = self.scoped_entry_root().and_then(|entry_root| {
            folder_entries
                .iter()
                .position(|entry| crate::folder_tree::path_eq(entry, entry_root))
        });
        self.folder_entries = Arc::new(folder_entries);
        match (&mut self.position, retained_index) {
            (SmartFolderPosition::Root, _) => true,
            (SmartFolderPosition::Scoped { entry_index, .. }, Some(index)) => {
                *entry_index = index;
                true
            }
            (SmartFolderPosition::Scoped { .. }, None) => {
                self.position = SmartFolderPosition::Root;
                false
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SmartFolderParentTarget {
    Root,
    Folder(PathBuf),
}

#[derive(Clone, Debug)]
pub(crate) enum TopLevelGridRestore {
    Unavailable,
    Folder(PathBuf),
    DriveList,
    ReadingHistory,
    Bookmarks,
    Rating { stars: u8 },
    SubfolderExpansion(SubfolderExpansionRestoreState),
    SmartFolder(SmartFolderViewState),
}

impl TopLevelGridRestore {
    pub(crate) fn from_legacy_parts(
        path: Option<PathBuf>,
        subfolder_restore: Option<SubfolderExpansionRestoreState>,
        rating_view_stars: Option<u8>,
        smart_state: Option<SmartFolderViewState>,
    ) -> Self {
        if let Some(state) = subfolder_restore {
            return Self::SubfolderExpansion(state);
        }
        let Some(path) = path else {
            return Self::Unavailable;
        };
        if crate::folder_tree::path_eq(&path, &super::drive_list_synthetic_path()) {
            Self::DriveList
        } else if crate::folder_tree::path_eq(&path, &super::reading_history_synthetic_path()) {
            Self::ReadingHistory
        } else if crate::folder_tree::path_eq(&path, &super::bookmark_view_synthetic_path()) {
            Self::Bookmarks
        } else if crate::folder_tree::path_eq(&path, &super::rating_view_synthetic_path()) {
            rating_view_stars
                .filter(|stars| (1..=5).contains(stars))
                .map(|stars| Self::Rating { stars })
                .unwrap_or(Self::Unavailable)
        } else if let Some(definition_id) =
            super::smart_folder::smart_folder_id_from_synthetic_path(&path)
        {
            let state = smart_state
                .filter(|state| state.definition_id == definition_id)
                .unwrap_or_else(|| SmartFolderViewState::root(definition_id, Vec::new()));
            Self::SmartFolder(state)
        } else {
            Self::Folder(path)
        }
    }

    pub(crate) fn legacy_path(&self) -> Option<PathBuf> {
        match self {
            Self::Unavailable => None,
            Self::Folder(path) => Some(path.clone()),
            Self::DriveList => Some(super::drive_list_synthetic_path()),
            Self::ReadingHistory => Some(super::reading_history_synthetic_path()),
            Self::Bookmarks => Some(super::bookmark_view_synthetic_path()),
            Self::Rating { .. } => Some(super::rating_view_synthetic_path()),
            Self::SubfolderExpansion(_) => Some(super::subfolder_expansion_synthetic_path()),
            Self::SmartFolder(state) => Some(super::smart_folder::smart_folder_synthetic_path(
                state.definition_id,
            )),
        }
    }

    pub(crate) fn rating_stars(&self) -> Option<u8> {
        match self {
            Self::Rating { stars } => Some(*stars),
            _ => None,
        }
    }

    pub(crate) fn subfolder_restore(&self) -> Option<SubfolderExpansionRestoreState> {
        match self {
            Self::SubfolderExpansion(state) => Some(state.clone()),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TopLevelGridSurface {
    Folder,
    DriveList,
    Search(TopLevelSearchView),
    Snapshot,
    SubfolderExpansion,
    SmartFolder(SmartFolderViewState),
    ReadingHistory,
    Bookmarks,
    Rating { stars: u8 },
}

pub(crate) struct TopLevelGridView {
    surface: TopLevelGridSurface,
    return_to: Option<TopLevelGridRestore>,
    generation: u64,
    /// The completed smart-folder result has exactly the same lifetime as this surface plus
    /// descendants opened from it. `begin` is an explicit top-level transition and always drops
    /// the old session; `replace_surface` preserves it only while the same smart-folder surface
    /// owns the navigation scope.
    smart_folder_session: Option<super::smart_folder::SmartFolderSession>,
}

impl Clone for TopLevelGridView {
    fn clone(&self) -> Self {
        Self {
            surface: self.surface.clone(),
            return_to: self.return_to.clone(),
            generation: self.generation,
            // Context duplication may copy the visible grid identity for an independent viewer,
            // but the main smart-folder result remains owned by the main top-level surface.
            smart_folder_session: None,
        }
    }
}

impl std::fmt::Debug for TopLevelGridView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TopLevelGridView")
            .field("surface", &self.surface)
            .field("return_to", &self.return_to)
            .field("generation", &self.generation)
            .field(
                "has_smart_folder_session",
                &self.smart_folder_session.is_some(),
            )
            .finish()
    }
}

impl Default for TopLevelGridView {
    fn default() -> Self {
        Self {
            surface: TopLevelGridSurface::Folder,
            return_to: None,
            generation: 0,
            smart_folder_session: None,
        }
    }
}

impl TopLevelGridView {
    pub(crate) fn surface(&self) -> &TopLevelGridSurface {
        &self.surface
    }

    pub(crate) fn begin(
        &mut self,
        surface: TopLevelGridSurface,
        return_to: Option<TopLevelGridRestore>,
    ) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.smart_folder_session = None;
        self.surface = surface;
        self.return_to = return_to;
        self.generation
    }

    pub(crate) fn replace_surface(&mut self, surface: TopLevelGridSurface) -> u64 {
        let keeps_smart_folder_session = matches!(
            (self.smart_folder_session.as_ref(), &surface),
            (
                Some(session),
                TopLevelGridSurface::SmartFolder(state),
            ) if session.definition_id() == state.definition_id
        );
        self.generation = self.generation.wrapping_add(1);
        if !keeps_smart_folder_session {
            self.smart_folder_session = None;
        }
        self.surface = surface;
        self.return_to = None;
        self.generation
    }

    pub(crate) fn take_return_to(&mut self) -> Option<TopLevelGridRestore> {
        self.generation = self.generation.wrapping_add(1);
        self.smart_folder_session = None;
        self.surface = TopLevelGridSurface::Folder;
        self.return_to.take()
    }

    pub(crate) fn return_to(&self) -> Option<&TopLevelGridRestore> {
        self.return_to.as_ref()
    }

    pub(crate) fn smart_folder(&self) -> Option<&SmartFolderViewState> {
        match &self.surface {
            TopLevelGridSurface::SmartFolder(state) => Some(state),
            _ => None,
        }
    }

    pub(crate) fn smart_folder_mut(&mut self) -> Option<&mut SmartFolderViewState> {
        match &mut self.surface {
            TopLevelGridSurface::SmartFolder(state) => Some(state),
            _ => None,
        }
    }

    pub(crate) fn smart_folder_session(&self) -> Option<&super::smart_folder::SmartFolderSession> {
        self.smart_folder_session.as_ref()
    }

    pub(crate) fn smart_folder_session_mut(
        &mut self,
    ) -> Option<&mut super::smart_folder::SmartFolderSession> {
        self.smart_folder_session.as_mut()
    }

    pub(crate) fn install_smart_folder_session(
        &mut self,
        session: super::smart_folder::SmartFolderSession,
    ) {
        debug_assert!(matches!(
            &self.surface,
            TopLevelGridSurface::SmartFolder(state)
                if state.definition_id == session.definition_id()
        ));
        self.smart_folder_session = Some(session);
    }

    pub(crate) fn take_smart_folder_session(
        &mut self,
    ) -> Option<super::smart_folder::SmartFolderSession> {
        self.smart_folder_session.take()
    }

    pub(crate) fn discard_smart_folder_session(&mut self, definition_id: uuid::Uuid) {
        if self
            .smart_folder_session
            .as_ref()
            .is_some_and(|session| session.definition_id() == definition_id)
        {
            self.smart_folder_session = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_folder_scope_never_accepts_sibling_escape() {
        let id = uuid::Uuid::new_v4();
        let root = PathBuf::from(r"C:\books\entry");
        let mut state = SmartFolderViewState::root(id, vec![root.clone()]);
        assert!(state.enter_containing_path(&root));
        assert!(state.move_to(&root.join("child")));
        assert!(!state.move_to(Path::new(r"C:\books\other")));
        assert_eq!(state.scoped_current(), Some(root.join("child").as_path()));
    }

    #[test]
    fn smart_folder_parent_returns_root_without_filesystem_escape() {
        let id = uuid::Uuid::new_v4();
        let root = PathBuf::from(r"C:\books\entry");
        let mut state = SmartFolderViewState::root(id, vec![root.clone()]);
        assert!(state.enter_containing_path(&root));
        assert_eq!(state.parent_target(), Some(SmartFolderParentTarget::Root));
        assert!(state.move_to(&root.join("child")));
        assert_eq!(
            state.parent_target(),
            Some(SmartFolderParentTarget::Folder(root))
        );
    }

    #[test]
    fn smart_folder_entry_order_moves_between_root_entries() {
        let id = uuid::Uuid::new_v4();
        let first = PathBuf::from(r"C:\books\first");
        let second = PathBuf::from(r"D:\library\second");
        let mut state = SmartFolderViewState::root(id, vec![first.clone(), second.clone()]);

        assert_eq!(state.entry_at_offset(true), Some(first.as_path()));
        assert_eq!(state.entry_at_offset(false), Some(second.as_path()));
        assert!(state.enter_containing_path(&first));
        assert_eq!(state.entry_at_offset(true), Some(second.as_path()));
        assert_eq!(state.entry_at_offset(false), None);
        assert!(state.enter_containing_path(&second));
        assert_eq!(state.entry_at_offset(true), None);
        assert_eq!(state.entry_at_offset(false), Some(first.as_path()));
    }

    #[test]
    fn smart_folder_refresh_reorders_retained_scope_and_drops_deleted_or_renamed_scope() {
        let id = uuid::Uuid::new_v4();
        let first = PathBuf::from(r"C:\books\first");
        let second = PathBuf::from(r"D:\library\second");
        let renamed = PathBuf::from(r"C:\books\renamed");
        let mut state = SmartFolderViewState::root(id, vec![first.clone(), second.clone()]);
        assert!(state.enter_containing_path(&first));
        assert!(state.refresh_folder_entries(vec![second.clone(), first.clone()]));
        assert_eq!(state.entry_index(), Some(1));
        assert!(state.move_to(&first.join("child")));

        assert!(!state.refresh_folder_entries(vec![renamed, second]));
        assert!(matches!(state.position, SmartFolderPosition::Root));
    }

    #[test]
    fn smart_folder_enters_most_specific_root_for_a_descendant() {
        let id = uuid::Uuid::new_v4();
        let parent = PathBuf::from(r"C:\books");
        let entry = parent.join("series");
        let current = entry.join("volume-1");
        let mut state = SmartFolderViewState::root(id, vec![parent, entry.clone()]);

        assert!(state.enter_containing_path(&current));
        assert_eq!(state.scoped_entry_root(), Some(entry.as_path()));
        assert_eq!(state.scoped_current(), Some(current.as_path()));
    }

    #[test]
    fn direct_top_level_transition_transfers_one_return_owner() {
        let origin = TopLevelGridRestore::Folder(PathBuf::from(r"D:\origin"));
        let mut view = TopLevelGridView::default();
        let first = view.begin(
            TopLevelGridSurface::Search(TopLevelSearchView::Global),
            Some(origin.clone()),
        );
        assert_eq!(first, 1);
        let transferred = view.take_return_to().expect("return owner");
        view.begin(
            TopLevelGridSurface::Search(TopLevelSearchView::Tag),
            Some(transferred),
        );
        assert!(matches!(
            view.return_to(),
            Some(TopLevelGridRestore::Folder(path)) if path == Path::new(r"D:\origin")
        ));
    }

    #[test]
    fn bookmark_synthetic_path_restores_bookmark_surface() {
        let restore = TopLevelGridRestore::from_legacy_parts(
            Some(super::super::bookmark_view_synthetic_path()),
            None,
            None,
            None,
        );
        assert!(matches!(restore, TopLevelGridRestore::Bookmarks));
        assert!(crate::folder_tree::path_eq(
            &restore.legacy_path().expect("bookmark path"),
            &super::super::bookmark_view_synthetic_path()
        ));
    }
}
