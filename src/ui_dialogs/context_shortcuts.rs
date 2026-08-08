//! 現在コンテキストで使えるショートカット一覧の初期ダイアログ。
//!
//! keymap 化済みの操作は `CommandDisplayRow` から、固定扱いのナビゲーションは
//! コンテキスト別の補助行として表示する。

use crate::app::App;
use crate::grid_item::GridItem;
use crate::keymap::{
    CommandDisplayRow, CommandScope, FS_IMAGE_ACTIVE_SCOPES, FS_VIDEO_ACTIVE_SCOPES,
    GRID_ACTIVE_SCOPES, KeyAction,
};
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
        keys: "← / → / ↑ / ↓",
        description: "選択位置を移動する",
    },
    FixedShortcutRow {
        keys: "Shift+矢印",
        description: "移動元から移動先までをチェックする",
    },
];

const FS_IMAGE_FIXED_SHORTCUT_ROWS: &[FixedShortcutRow] = &[
    FixedShortcutRow {
        keys: "?",
        description: "このショートカット一覧を表示する",
    },
    FixedShortcutRow {
        keys: "Esc",
        description: "フルスクリーンを閉じて一覧へ戻る",
    },
    FixedShortcutRow {
        keys: "← / → / ↑ / ↓ / マウスホイール",
        description: "前または次の項目へ移動する",
    },
    FixedShortcutRow {
        keys: "Ctrl+Alt+Shift+D",
        description: "画像パイプラインのデバッグ出力を保存する",
    },
    FixedShortcutRow {
        keys: "Ctrl+ホイール",
        description: "ズーム倍率を変更する",
    },
];

const FS_IMAGE_TOUCH_SHORTCUT_ROWS: &[FixedShortcutRow] = &[
    FixedShortcutRow {
        keys: "中央をタップ",
        description: "上部バー、下部シークバー、左右のパネルハンドルを表示 / 非表示にする",
    },
    FixedShortcutRow {
        keys: "左右をタップ",
        description: "前または次のページへ移動する",
    },
    FixedShortcutRow {
        keys: "2 本指でピンチ / 移動",
        description: "画像をズーム / パンする",
    },
];

const FS_VIDEO_FIXED_SHORTCUT_ROWS: &[FixedShortcutRow] = &[
    FixedShortcutRow {
        keys: "?",
        description: "このショートカット一覧を表示する",
    },
    FixedShortcutRow {
        keys: "Esc",
        description: "一覧へ戻る。タイルモード中は先にタイルモードを閉じる",
    },
    FixedShortcutRow {
        keys: "← / →",
        description: "5秒戻る / 進む。タイルモード中はタイルカーソルを移動する",
    },
    FixedShortcutRow {
        keys: "マウスホイール",
        description: "前または次の項目へ移動する",
    },
    FixedShortcutRow {
        keys: "Ctrl+ホイール",
        description: "タイルモード中はタイル列数を変更する",
    },
];

const ERASE_HELP_SCOPES: &[CommandScope] = &[CommandScope::Erase];
const CONCEAL_HELP_SCOPES: &[CommandScope] = &[CommandScope::Conceal];
const CROP_HELP_SCOPES: &[CommandScope] = &[CommandScope::Crop];
const LOCAL_ADJUST_HELP_SCOPES: &[CommandScope] = &[CommandScope::LocalAdjust];
const TEXT_HELP_SCOPES: &[CommandScope] = &[CommandScope::Text];

const ERASE_FIXED_SHORTCUT_ROWS: &[FixedShortcutRow] = &[
    FixedShortcutRow {
        keys: "?",
        description: "このショートカット一覧を表示する",
    },
    FixedShortcutRow {
        keys: "Esc",
        description: "選択や多角形入力を解除する。解除対象がなければ補完を実行して終了する",
    },
    FixedShortcutRow {
        keys: "矢印 / Ctrl+矢印",
        description: "マスクまたは選択オブジェクトを 1px / 10px 移動する",
    },
    FixedShortcutRow {
        keys: "[ / ] / Ctrl+[ / Ctrl+]",
        description: "マスクまたは選択オブジェクトを 0.1度 / 1度 回転する",
    },
    FixedShortcutRow {
        keys: "マウスホイール / Ctrl+ホイール",
        description: "画像上ではズームする。パネル上ではスクロールまたはズームする",
    },
];

const CONCEAL_FIXED_SHORTCUT_ROWS: &[FixedShortcutRow] = &[
    FixedShortcutRow {
        keys: "?",
        description: "このショートカット一覧を表示する",
    },
    FixedShortcutRow {
        keys: "Esc",
        description: "選択や多角形入力を解除する。解除対象がなければ隠蔽加工モードを終了する",
    },
    FixedShortcutRow {
        keys: "矢印 / Ctrl+矢印",
        description: "選択オブジェクト、またはオブジェクト全体を 1px / 10px 移動する",
    },
    FixedShortcutRow {
        keys: "マウスホイール / Ctrl+ホイール",
        description: "画像上ではズームする。パネル上ではスクロールまたはズームする",
    },
];

const CROP_FIXED_SHORTCUT_ROWS: &[FixedShortcutRow] = &[
    FixedShortcutRow {
        keys: "?",
        description: "このショートカット一覧を表示する",
    },
    FixedShortcutRow {
        keys: "Esc",
        description: "切り取りモードを終了する",
    },
    FixedShortcutRow {
        keys: "ドラッグ",
        description: "切り取り範囲を作成、移動、またはリサイズする",
    },
    FixedShortcutRow {
        keys: "マウスホイール / Ctrl+ホイール",
        description: "画像上ではズームする。パネル上ではスクロールまたはズームする",
    },
];

const LOCAL_ADJUST_FIXED_SHORTCUT_ROWS: &[FixedShortcutRow] = &[
    FixedShortcutRow {
        keys: "?",
        description: "このショートカット一覧を表示する",
    },
    FixedShortcutRow {
        keys: "Esc",
        description: "編集中の図形操作を解除する。解除対象がなければ補正レイヤーパネルを閉じる",
    },
    FixedShortcutRow {
        keys: "Delete",
        description: "選択中の図形マスクを削除する",
    },
    FixedShortcutRow {
        keys: "Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z",
        description: "多角形入力中は頂点を戻す。それ以外は補正レイヤー操作を取り消し / やり直し",
    },
    FixedShortcutRow {
        keys: "矢印 / Ctrl+矢印",
        description: "選択中の図形マスクを 1px / 10px 移動する",
    },
    FixedShortcutRow {
        keys: "[ / ] / Ctrl+[ / Ctrl+]",
        description: "選択中の図形マスクを 0.1度 / 1度 回転する",
    },
    FixedShortcutRow {
        keys: "Shift+ハンドル / Alt+ハンドル",
        description: "角度や比率をスナップする、または中心固定で変形する",
    },
    FixedShortcutRow {
        keys: "ドラッグ",
        description: "画像上でマスク範囲を作成、選択、または編集する",
    },
    FixedShortcutRow {
        keys: "マウスホイール / Ctrl+ホイール",
        description: "画像上ではズームする。パネル上ではスクロールまたはズームする",
    },
];

const TEXT_FIXED_SHORTCUT_ROWS: &[FixedShortcutRow] = &[
    FixedShortcutRow {
        keys: "?",
        description: "このショートカット一覧を表示する",
    },
    FixedShortcutRow {
        keys: "Esc",
        description: "選択を解除する。未選択ならテキスト注釈モードを終了する",
    },
    FixedShortcutRow {
        keys: "Delete / Backspace",
        description: "選択中の注釈をまとめて削除する。本文の編集中はテキスト入力を優先する",
    },
    FixedShortcutRow {
        keys: "Ctrl / Shift+クリック",
        description: "注釈を複数選択する。一覧の Shift+クリックは範囲選択する",
    },
    FixedShortcutRow {
        keys: "ドラッグ",
        description: "選択注釈をまとめて移動する。空き領域では矩形選択、単一選択ではハンドル編集する",
    },
    FixedShortcutRow {
        keys: "マウスホイール / Ctrl+ホイール",
        description: "画像上ではズームする。パネル上ではスクロールまたはズームする",
    },
    FixedShortcutRow {
        keys: "中ボタン+上下ドラッグ",
        description: "ドラッグ開始位置を中心にズームする",
    },
];

const TEXT_SUPPLEMENTAL_ACTION_ROWS: &[(KeyAction, &str)] = &[(
    KeyAction::FsOriginalPreviewHold,
    "押している間だけテキスト注釈を外した元画像を表示する",
)];

#[derive(Clone, Copy)]
enum ShortcutHelpContext {
    Grid,
    FsImage,
    FsVideo,
    Erase,
    Conceal,
    Crop,
    LocalAdjust,
    Text,
}

impl ShortcutHelpContext {
    fn title(self) -> &'static str {
        match self {
            Self::Grid => "サムネイル一覧",
            Self::FsImage => "画像フルスクリーン",
            Self::FsVideo => "動画フルスクリーン",
            Self::Erase => "消しゴムモード",
            Self::Conceal => "隠蔽加工モード",
            Self::Crop => "切り取りモード",
            Self::LocalAdjust => "補正レイヤー",
            Self::Text => "テキスト注釈モード",
        }
    }

    fn active_scopes(self) -> &'static [CommandScope] {
        match self {
            Self::Grid => GRID_ACTIVE_SCOPES,
            Self::FsImage => FS_IMAGE_ACTIVE_SCOPES,
            Self::FsVideo => FS_VIDEO_ACTIVE_SCOPES,
            Self::Erase => ERASE_HELP_SCOPES,
            Self::Conceal => CONCEAL_HELP_SCOPES,
            Self::Crop => CROP_HELP_SCOPES,
            Self::LocalAdjust => LOCAL_ADJUST_HELP_SCOPES,
            Self::Text => TEXT_HELP_SCOPES,
        }
    }

    fn fixed_rows(self) -> &'static [FixedShortcutRow] {
        match self {
            Self::Grid => GRID_FIXED_SHORTCUT_ROWS,
            Self::FsImage => FS_IMAGE_FIXED_SHORTCUT_ROWS,
            Self::FsVideo => FS_VIDEO_FIXED_SHORTCUT_ROWS,
            Self::Erase => ERASE_FIXED_SHORTCUT_ROWS,
            Self::Conceal => CONCEAL_FIXED_SHORTCUT_ROWS,
            Self::Crop => CROP_FIXED_SHORTCUT_ROWS,
            Self::LocalAdjust => LOCAL_ADJUST_FIXED_SHORTCUT_ROWS,
            Self::Text => TEXT_FIXED_SHORTCUT_ROWS,
        }
    }

    fn supplemental_action_rows(self) -> &'static [(KeyAction, &'static str)] {
        match self {
            Self::Text => TEXT_SUPPLEMENTAL_ACTION_ROWS,
            Self::Grid
            | Self::FsImage
            | Self::FsVideo
            | Self::Erase
            | Self::Conceal
            | Self::Crop
            | Self::LocalAdjust => &[],
        }
    }

    fn touch_rows(self) -> &'static [FixedShortcutRow] {
        match self {
            Self::FsImage => FS_IMAGE_TOUCH_SHORTCUT_ROWS,
            Self::Grid
            | Self::FsVideo
            | Self::Erase
            | Self::Conceal
            | Self::Crop
            | Self::LocalAdjust
            | Self::Text => &[],
        }
    }

    fn includes_row(self, row: &CommandDisplayRow) -> bool {
        if row.spec.action.is_location_navigation_action() && row.shortcut_labels.is_empty() {
            return false;
        }
        match self {
            Self::Grid => true,
            Self::FsImage => {
                row.spec.scope != CommandScope::Global
                    || row.spec.action == KeyAction::ToggleDetachedViewerMode
                    || row.spec.action.is_location_navigation_action()
            }
            Self::FsVideo => video_help_includes_row(row),
            Self::Erase | Self::Conceal | Self::Crop | Self::LocalAdjust | Self::Text => true,
        }
    }
}

impl App {
    pub(crate) fn show_context_shortcuts_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_context_shortcuts_help {
            return;
        }
        if !self.ime_input_active(ctx) && self.consume_context_shortcuts_help_key(ctx) {
            self.show_context_shortcuts_help = false;
            return;
        }

        let help_context = self.current_shortcut_help_context();

        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let dialog_pos = ctx.content_rect().min + egui::vec2(72.0, 52.0);
        let scroll_max_h = (ctx.content_rect().height() - 180.0).min(620.0).max(160.0);
        let rows = self
            .keymap
            .command_display_rows_for_active_scopes(help_context.active_scopes(), false)
            .into_iter()
            .filter(|row| help_context.includes_row(row))
            .filter(|row| {
                self.explicit_grid_container_open_disabled_reason()
                    .is_none()
                    || !matches!(
                        row.spec.action,
                        KeyAction::GridOpenSelectedAsPage | KeyAction::GridOpenSelectedAsList
                    )
            })
            .collect::<Vec<_>>();
        let mut close_clicked = false;

        egui::Window::new("ショートカット")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_pos(dialog_pos)
            .min_width(520.0)
            .default_height(520.0)
            .show(ctx, |ui| {
                ui.label(format!("現在のコンテキスト: {}", help_context.title()));
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .id_salt("context_shortcuts_scroll")
                    .max_height(scroll_max_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for scope in help_context.active_scopes() {
                            draw_command_scope_rows(ui, *scope, &rows);
                        }
                        draw_supplemental_action_rows(
                            ui,
                            &self.keymap,
                            help_context.supplemental_action_rows(),
                        );
                        draw_touch_rows(ui, help_context.touch_rows());
                        draw_fixed_rows(ui, &self.keymap, help_context.fixed_rows());
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

    fn current_shortcut_help_context(&self) -> ShortcutHelpContext {
        if self.erase_mode {
            return ShortcutHelpContext::Erase;
        }
        if self.conceal_mode {
            return ShortcutHelpContext::Conceal;
        }
        if self.local_adjust_mode {
            return ShortcutHelpContext::LocalAdjust;
        }
        if self.text_mode {
            return ShortcutHelpContext::Text;
        }
        if self.export_crop_mode {
            return ShortcutHelpContext::Crop;
        }
        if let Some(fs_idx) = self.fullscreen_idx
            && matches!(
                self.items.get(fs_idx),
                Some(GridItem::Video(_)) | Some(GridItem::Audio(_))
            )
        {
            // 音声 (映像なし動画) は動画スコープの Video* アクションを共有する。
            return ShortcutHelpContext::FsVideo;
        }
        if let Some(fs_idx) = self.fullscreen_idx
            && matches!(
                self.items.get(fs_idx),
                Some(GridItem::Image(_) | GridItem::ZipImage { .. } | GridItem::PdfPage { .. })
            )
            && !self.is_overlay_edit_mode_active()
        {
            return ShortcutHelpContext::FsImage;
        }
        ShortcutHelpContext::Grid
    }
}

fn video_help_includes_row(row: &CommandDisplayRow) -> bool {
    match row.spec.action {
        KeyAction::ToggleDetachedViewerMode
        | KeyAction::FsToggleWindowMode
        | KeyAction::FsJumpFirst
        | KeyAction::FsJumpLast
        | KeyAction::FsCtrlNavPrev
        | KeyAction::FsCtrlNavNext
        | KeyAction::FsSiblingPrev
        | KeyAction::FsSiblingNext => true,
        action if action.is_location_navigation_action() => !row.shortcut_labels.is_empty(),
        KeyAction::VideoCompareToggle
        | KeyAction::VideoCompareCycle
        | KeyAction::VideoCompareWipe
        | KeyAction::VideoCompareDiff => false,
        _ => matches!(row.spec.scope, CommandScope::Rating | CommandScope::FsVideo),
    }
}

fn draw_command_scope_rows(ui: &mut egui::Ui, scope: CommandScope, rows: &[CommandDisplayRow]) {
    if !rows
        .iter()
        .any(|row| row.spec.scope == scope && !row.shortcut_labels.is_empty())
    {
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
            for row in rows
                .iter()
                .filter(|row| row.spec.scope == scope && !row.shortcut_labels.is_empty())
            {
                let shortcut = row.shortcut_labels.join(" / ");
                ui.monospace(shortcut);
                ui.label(row.spec.description());
                ui.end_row();
            }
        });
}

fn draw_fixed_rows(ui: &mut egui::Ui, keymap: &crate::keymap::Keymap, rows: &[FixedShortcutRow]) {
    ui.add_space(10.0);
    ui.label(egui::RichText::new("固定キー").strong());
    ui.add_space(2.0);
    egui::Grid::new(("context_shortcuts_fixed", rows.as_ptr() as usize))
        .num_columns(2)
        .spacing([18.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for row in rows {
                let keys = if row.keys == "?" {
                    keymap.context_shortcuts_help_label()
                } else {
                    row.keys.to_string()
                };
                ui.monospace(keys);
                ui.label(row.description);
                ui.end_row();
            }
        });
}

fn draw_touch_rows(ui: &mut egui::Ui, rows: &[FixedShortcutRow]) {
    if rows.is_empty() {
        return;
    }
    ui.add_space(10.0);
    ui.label(egui::RichText::new("タッチ操作").strong());
    ui.add_space(2.0);
    egui::Grid::new(("context_shortcuts_touch", rows.as_ptr() as usize))
        .num_columns(2)
        .spacing([18.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for row in rows {
                ui.monospace(row.keys);
                ui.label(row.description);
                ui.end_row();
            }
        });
}

fn supplemental_action_shortcut_label(keymap: &crate::keymap::Keymap, action: KeyAction) -> String {
    let labels = keymap.chord_labels(action);
    if labels.is_empty() {
        "未設定".to_string()
    } else {
        labels.join(" / ")
    }
}

fn draw_supplemental_action_rows(
    ui: &mut egui::Ui,
    keymap: &crate::keymap::Keymap,
    rows: &[(KeyAction, &str)],
) {
    if rows.is_empty() {
        return;
    }
    ui.add_space(10.0);
    ui.label(egui::RichText::new("共通操作").strong());
    ui.add_space(2.0);
    egui::Grid::new(("context_shortcuts_supplemental", rows.as_ptr() as usize))
        .num_columns(2)
        .spacing([18.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for (action, description) in rows {
                ui.monospace(supplemental_action_shortcut_label(keymap, *action));
                ui.label(*description);
                ui.end_row();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{ShortcutHelpContext, supplemental_action_shortcut_label};
    use crate::keymap::{Chord, KeyAction, Keymap, KeymapSettings, ModKind};

    #[test]
    fn original_preview_help_label_follows_assignment_and_disabled_state() {
        let action = KeyAction::FsOriginalPreviewHold;
        assert_eq!(
            supplemental_action_shortcut_label(&Keymap::empty(), action),
            "右Ctrl"
        );

        let mut settings = KeymapSettings::default();
        settings.set_override_chords(action, vec![Chord::modifier(ModKind::RightShift)]);
        assert_eq!(
            supplemental_action_shortcut_label(&Keymap::from_settings(&settings), action),
            "右Shift"
        );

        settings.disable_action(action);
        assert_eq!(
            supplemental_action_shortcut_label(&Keymap::from_settings(&settings), action),
            "未設定"
        );
    }

    #[test]
    fn touch_help_rows_are_limited_to_still_image_fullscreen() {
        let rows = ShortcutHelpContext::FsImage.touch_rows();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().any(|row| row.keys == "中央をタップ"));
        assert!(ShortcutHelpContext::Grid.touch_rows().is_empty());
        assert!(ShortcutHelpContext::FsVideo.touch_rows().is_empty());
    }
}
