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
        ctx: &egui::Context,
        kind: Kind,
        main_viewer_blocked: bool,
    ) -> Result<(), &'static str> {
        if kind == Kind::Everything {
            if ctx.viewport_id() != egui::ViewportId::ROOT {
                return Err("メインウィンドウの一覧から検索してください");
            }
            if let Some(reason) = self.everything_search_entry_availability().reason_text() {
                return Err(reason);
            }
            let permit = self.keyboard_owner_for_pass(ctx).shortcut_permit();
            return if permit.is_some_and(|permit| {
                permit.scope().viewport == ctx.viewport_id()
                    && permit.scope().surface == crate::keyboard_input::ShortcutSurface::Main
            }) {
                Ok(())
            } else {
                Err("文字入力などの操作を終えてから検索してください")
            };
        }
        let allowed = !self.address_has_focus
            && !main_viewer_blocked
            && !self.any_dialog_open()
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
        self.search_shortcut_availability(
            ctx,
            kind,
            self.viewer_session_blocks_main_window() || self.viewer_mutation_modal_active(),
        )?;
        self.pinned_search_availability(kind)
    }

    pub(crate) fn open_search_with_text(&mut self, ctx: &egui::Context, request: SearchRequest) {
        if let Err(reason) = self.search_with_text_availability(ctx, request.target) {
            self.show_feedback_result(reason.into(), crate::feedback::FeedbackKind::Info);
            return;
        }
        let kind = request.target;
        self.ensure_search_session(kind);
        *self.search_draft_mut(kind) = request.query;
        if kind == Kind::Metadata {
            // Injection is not Edited/Submit: never arm (or retain) a debounce.
            self.global_search.last_change_at = None;
            if self.global_search.pending.is_none() {
                self.global_search.done = true;
            }
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
        crate::search_request::request_select_all(ctx, input_id, self.search_draft(kind));
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
