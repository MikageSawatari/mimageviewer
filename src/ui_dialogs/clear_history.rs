//! Shared history deletion commands and the Grid-only reading-history confirmation.

use crate::app::App;
use eframe::egui;

impl App {
    /// Preferences clears immediately too; keep its draft and result display in sync.
    pub(crate) fn clear_search_input_history(&mut self) -> usize {
        let count = self.settings.search_input_history.len();
        self.settings.search_input_history.clear();
        if let Some(state) = self.pref_state.as_mut() {
            state.settings.search_input_history.clear();
            state.search_input_history_entry_count = 0;
            state.search_input_history_clear_result =
                Some(format!("検索欄の入力履歴を {count} 件消去しました。"));
        }
        count
    }

    pub(crate) fn execute_clear_search_input_history(&mut self) {
        if self.settings.search_input_history.len() == 0 {
            self.show_feedback_toast("検索欄の入力履歴はありません".into());
            return;
        }
        let count = self.clear_search_input_history();
        if !self.settings.save_checked() {
            crate::logger::log("[search_input_history] clear could not be saved");
            self.show_feedback_warning("検索欄の入力履歴の消去を保存できませんでした".into());
            return;
        }
        self.show_feedback_toast(format!("検索欄の入力履歴を {count} 件消去しました"));
    }

    /// Reuse the shipped Preferences DELETE, mutation admission, log and view reload.
    /// None means admission rejected the mutation (and supplied its own feedback).
    pub(crate) fn clear_reading_history(&mut self) -> Option<Result<(usize, usize), String>> {
        if !self.admit_book_append_followup_mutation() {
            return None;
        }
        let result = match self.reading_history_db.as_ref() {
            Some(db) => db
                .clear_all()
                .map(|deleted| {
                    crate::logger::log(format!("[reading_history] cleared {deleted} entries"));
                    (deleted, db.count())
                })
                .map_err(|error| error.to_string()),
            None => Err("閲覧履歴 DB を開けませんでした".into()),
        };
        if let Err(error) = &result {
            crate::logger::log(format!("[reading_history] clear failed: {error}"));
        }
        if self.items_are_reading_history_view {
            self.enter_reading_history();
        }
        Some(result)
    }

    fn reading_history_is_empty(&self) -> bool {
        // A missing DB is an error, not an empty history. Let the shared command report it.
        self.reading_history_db
            .as_ref()
            .is_some_and(|db| db.count() == 0)
    }

    pub(crate) fn request_clear_reading_history(&mut self) {
        if self.reading_history_is_empty() {
            self.show_feedback_toast("閲覧履歴はありません".into());
            return;
        }
        self.show_reading_history_clear_confirm = true;
    }

    pub(crate) fn reading_history_clear_dialog_visible(&self) -> bool {
        self.show_reading_history_clear_confirm
    }

    pub(crate) fn resolve_reading_history_clear_confirmation(&mut self, clear: bool) {
        if !std::mem::take(&mut self.show_reading_history_clear_confirm) || !clear {
            return;
        }
        // Recount at confirmation, rather than retaining an opening-time count.
        if self.reading_history_is_empty() {
            self.show_feedback_toast("閲覧履歴はありません".into());
            return;
        }
        match self.clear_reading_history() {
            Some(Ok((deleted, _))) => {
                self.show_feedback_toast(format!("閲覧履歴を {deleted} 件削除しました"));
            }
            Some(Err(_)) => {
                self.show_feedback_warning("閲覧履歴の削除に失敗しました".into());
            }
            None => {}
        }
    }

    pub(crate) fn show_reading_history_clear_confirm_dialog(&mut self, ctx: &egui::Context) {
        if !self.reading_history_clear_dialog_visible() {
            return;
        }
        // Holding an Enter-bound command must not confirm via its autorepeat.
        let enter_pressed = self.dialog_enter_pressed(ctx)
            && ctx.input(|input| {
                input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::Enter,
                            pressed: true,
                            repeat: false,
                            ..
                        }
                    )
                })
            });
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let mut open = true;
        let mut clear = false;
        let mut cancel = escape_pressed;
        // Normal Window uses the resolved OS theme, including its warning text color.
        egui::Window::new("閲覧履歴のクリア")
            .id(egui::Id::unique("reading_history_clear_confirm"))
            .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("閲覧履歴をすべてクリアしますか？");
                ui.colored_label(ui.visuals().warn_fg_color, "元に戻せません。");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    clear = ui.button("クリア").clicked() || enter_pressed;
                    cancel |= ui.button("キャンセル").clicked();
                });
            });
        if cancel || !open {
            self.resolve_reading_history_clear_confirmation(false);
        } else if clear {
            self.resolve_reading_history_clear_confirmation(true);
        }
    }
}
