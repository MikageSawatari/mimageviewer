//! Admission and draft-only handoff from one main-grid item to an existing search input.
use super::App;
use crate::grid_item::GridItem;
use crate::keymap::KeyAction;
use crate::search_request::{SearchRequest, item_search_name};
use crate::ui_pinned_search::PinnedSearchKind as Kind;

impl App {
    /// The existing shortcut conjunctions, shared without broadening their admission.
    /// `main_viewer_blocked` is the main UI checkpoint's existing viewer/modal snapshot.
    pub(crate) fn search_shortcut_availability(
        &self,
        kind: Kind,
        main_viewer_blocked: bool,
    ) -> Result<(), &'static str> {
        self.search_entry_availability(kind, main_viewer_blocked, true)
    }

    /// Evaluate state only. Keyboard ownership stays in the shortcut dispatcher;
    /// a menu's own input ownership is not a reason to disable its commands.
    fn search_entry_availability(
        &self,
        kind: Kind,
        main_viewer_blocked: bool,
        include_context_menu: bool,
    ) -> Result<(), &'static str> {
        if kind == Kind::Everything {
            return self
                .everything_search_entry_availability_with_modal(
                    self.search_common_modal_open(include_context_menu),
                )
                .reason_text()
                .map_or(Ok(()), Err);
        }
        let allowed = !self.address_has_focus
            && !main_viewer_blocked
            && !self.search_dialog_open(include_context_menu)
            && !self.favsearch.has_focus
            && !self.tag_view.has_focus
            && match kind {
                Kind::Local => !self.global_search.has_focus && !self.global_search.active,
                Kind::Metadata => !self.search_has_focus,
                Kind::Container => !self.search_has_focus && !self.global_search.has_focus,
                Kind::Everything => unreachable!(),
            };
        if allowed {
            Ok(())
        } else if kind == Kind::Local && self.global_search.active {
            Err("アイテム検索・Everything の検索を終了してから絞り込んでください")
        } else {
            Err("ほかの操作が完了してから検索してください")
        }
    }

    pub(crate) fn search_common_modal_open(&self, include_context_menu: bool) -> bool {
        self.modal_dialog_block_reason_including_menus(include_context_menu)
            .is_some()
    }

    pub(crate) fn search_dialog_open(&self, include_context_menu: bool) -> bool {
        self.search_common_modal_open(include_context_menu) || self.show_vst3_manager
    }

    /// One item-entry predicate for menus, keys and menu dispatch. Empty names
    /// remain eligible so the menu can explain why the three targets are disabled.
    pub(crate) fn grid_item_search_name(
        &self,
        ctx: &egui::Context,
        item: &GridItem,
    ) -> Option<String> {
        if ctx.viewport_id() != egui::ViewportId::ROOT
            || !self.main_folder_history_available()
            || self.fullscreen_idx.is_some()
            || !self.grid_item_input_allowed()
            || self.items_are_bookmark_view
            || self.items_are_reading_history_view
            || !self.checked.is_empty()
        {
            return None;
        }
        item_search_name(item)
    }

    pub(crate) fn search_with_text_availability(
        &self,
        ctx: &egui::Context,
        kind: Kind,
    ) -> Result<(), &'static str> {
        if ctx.viewport_id() != egui::ViewportId::ROOT {
            return Err("メインウィンドウの一覧から検索してください");
        }
        self.search_prefill_availability(kind, true)
    }

    pub(crate) fn search_with_text_menu_availability(
        &self,
        kind: Kind,
    ) -> Result<(), &'static str> {
        self.search_prefill_availability(kind, false)
    }

    /// Prefill has the ordinary shortcut's state admission plus the item-search
    /// restriction. Keep the shortcut itself free to focus/toggle its open session.
    fn search_prefill_availability(
        &self,
        kind: Kind,
        include_context_menu: bool,
    ) -> Result<(), &'static str> {
        if kind == Kind::Metadata && self.global_search.active {
            return Err("検索を閉じてから使えます");
        }
        self.search_entry_availability(
            kind,
            self.viewer_session_blocks_main_window() || self.viewer_mutation_modal_active(),
            include_context_menu,
        )?;
        self.pinned_search_availability_including_context_menu(kind, include_context_menu)
    }

    pub(crate) fn open_search_with_text(&mut self, ctx: &egui::Context, request: SearchRequest) {
        if let Err(reason) = self.search_with_text_availability(ctx, request.target) {
            self.show_feedback_result(reason.into(), crate::feedback::FeedbackKind::Info);
            return;
        }
        let kind = request.target;
        if kind == Kind::Metadata {
            // Open the existing retained-draft idle state. No edit transaction,
            // producer, or projection is needed before the user submits it.
            *self.search_draft_mut(kind) = request.query;
            self.ensure_search_session(kind);
        } else {
            self.ensure_search_session(kind);
            *self.search_draft_mut(kind) = request.query;
        }
        let input_id = if kind.pinned(&self.settings) {
            self.request_pinned_search_focus(ctx, kind);
            kind.input_id()
        } else {
            match kind {
                Kind::Local => self.search_focus_request = true,
                Kind::Metadata | Kind::Everything => self.global_search.focus_request = true,
                Kind::Container => self.favsearch.focus_request = true,
            }
            kind.temporary_id()
        };
        if matches!(kind, Kind::Local | Kind::Metadata) {
            crate::search_request::request_select_all(ctx, input_id, self.search_draft(kind));
        }
    }

    /// Called after handle_keyboard's existing ownership/IME/pane gates.
    pub(crate) fn handle_grid_search_name_keys(&mut self, ctx: &egui::Context) -> bool {
        let Some(query) = self
            .selected
            .and_then(|index| self.items.get(index))
            .and_then(|item| self.grid_item_search_name(ctx, item))
        else {
            return false;
        };
        for (action, target) in [
            (KeyAction::GridSearchNameEverything, Kind::Everything),
            (KeyAction::GridSearchNameMetadata, Kind::Metadata),
            (KeyAction::GridSearchNameLocal, Kind::Local),
        ] {
            if self.keymap.consume_action_no_repeat(ctx, action) {
                if !query.is_empty() {
                    self.open_search_with_text(ctx, SearchRequest { target, query });
                }
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
#[path = "tests/search_request.rs"]
mod tests;
