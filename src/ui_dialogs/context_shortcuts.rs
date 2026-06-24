//! 現在コンテキストで使えるショートカット一覧の初期ダイアログ。
//!
//! 初期スライスではグリッド文脈だけを扱い、keymap 化済みの操作は
//! `CommandDisplayRow` から、固定扱いのナビゲーションだけを補助行として表示する。

use crate::app::App;
use crate::keymap::{CommandDisplayRow, CommandScope, GRID_ACTIVE_SCOPES};
use eframe::egui;

struct FixedShortcutRow {
    keys: &'static str,
    description: &'static str,
}

const GRID_FIXED_SHORTCUT_ROWS: &[FixedShortcutRow] = &[
    FixedShortcutRow {
        keys: "?",
        description: "このショートカット一覧を表示する",
    },
    FixedShortcutRow {
        keys: "Enter",
        description: "選択中の項目を開く",
    },
    FixedShortcutRow {
        keys: "Shift+Enter",
        description: "選択中の動画を外部プレイヤーで開く",
    },
    FixedShortcutRow {
        keys: "Backspace / Alt+↑",
        description: "親フォルダまたは検索結果の上位階層へ戻る",
    },
    FixedShortcutRow {
        keys: "← / → / ↑ / ↓",
        description: "選択位置を移動する",
    },
    FixedShortcutRow {
        keys: "Home / End",
        description: "先頭または末尾の項目へ移動する",
    },
    FixedShortcutRow {
        keys: "PageUp / PageDown",
        description: "1ページ分移動する",
    },
    FixedShortcutRow {
        keys: "Shift+矢印",
        description: "移動元から移動先までをチェックする",
    },
    FixedShortcutRow {
        keys: "Ctrl+↑ / Ctrl+↓",
        description: "前または次のフォルダ / 検索結果へ移動する",
    },
    FixedShortcutRow {
        keys: "Alt+← / Alt+→",
        description: "フォルダ履歴を戻る / 進む",
    },
    FixedShortcutRow {
        keys: "Ctrl+PageUp / Ctrl+PageDown",
        description: "前または次の兄弟フォルダへ移動する",
    },
    FixedShortcutRow {
        keys: "F11",
        description: "メインウィンドウを最大化または復元する",
    },
];

impl App {
    pub(crate) fn show_context_shortcuts_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_context_shortcuts_help {
            return;
        }

        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let dialog_pos = ctx.content_rect().min + egui::vec2(72.0, 52.0);
        let scroll_max_h = (ctx.content_rect().height() - 180.0).min(620.0).max(160.0);
        let rows = self
            .keymap
            .command_display_rows_for_active_scopes(GRID_ACTIVE_SCOPES, false);
        let mut close_clicked = false;

        egui::Window::new("ショートカット")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_pos(dialog_pos)
            .min_width(520.0)
            .default_height(520.0)
            .show(ctx, |ui| {
                ui.label("現在のコンテキスト: サムネイル一覧");
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .id_salt("context_shortcuts_scroll")
                    .max_height(scroll_max_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for scope in GRID_ACTIVE_SCOPES {
                            draw_command_scope_rows(ui, *scope, &rows);
                        }
                        draw_fixed_grid_rows(ui);
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                if ui.button("閉じる").clicked() {
                    close_clicked = true;
                }
            });

        if !open || close_clicked || escape_pressed {
            self.show_context_shortcuts_help = false;
        }
    }
}

fn draw_command_scope_rows(ui: &mut egui::Ui, scope: CommandScope, rows: &[CommandDisplayRow]) {
    if !rows.iter().any(|row| row.spec.scope == scope) {
        return;
    }

    ui.add_space(6.0);
    ui.label(egui::RichText::new(scope.description()).strong());
    ui.add_space(2.0);
    egui::Grid::new(("context_shortcuts_scope", scope.ini_name()))
        .num_columns(2)
        .spacing([18.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for row in rows.iter().filter(|row| row.spec.scope == scope) {
                let shortcut = row.shortcut_labels.join(" / ");
                ui.monospace(shortcut);
                ui.label(row.spec.description());
                ui.end_row();
            }
        });
}

fn draw_fixed_grid_rows(ui: &mut egui::Ui) {
    ui.add_space(10.0);
    ui.label(egui::RichText::new("固定キー").strong());
    ui.add_space(2.0);
    egui::Grid::new("context_shortcuts_grid_fixed")
        .num_columns(2)
        .spacing([18.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for row in GRID_FIXED_SHORTCUT_ROWS {
                ui.monospace(row.keys);
                ui.label(row.description);
                ui.end_row();
            }
        });
}
