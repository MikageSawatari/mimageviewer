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

const FS_IMAGE_FIXED_SHORTCUT_ROWS: &[FixedShortcutRow] = &[
    FixedShortcutRow {
        keys: "?",
        description: "このショートカット一覧を表示する",
    },
    FixedShortcutRow {
        keys: "Esc / Enter",
        description: "フルスクリーンを閉じて一覧へ戻る",
    },
    FixedShortcutRow {
        keys: "Backspace",
        description: "一覧へ戻る。ZIP/PDF 内ページではページ一覧へ戻る",
    },
    FixedShortcutRow {
        keys: "← / → / ↑ / ↓ / マウスホイール",
        description: "前または次の項目へ移動する",
    },
    FixedShortcutRow {
        keys: "Home / End",
        description: "先頭または末尾の項目へ移動する",
    },
    FixedShortcutRow {
        keys: "F11",
        description: "ウィンドウ内表示と全画面表示を切り替える",
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

const FS_VIDEO_FIXED_SHORTCUT_ROWS: &[FixedShortcutRow] = &[
    FixedShortcutRow {
        keys: "?",
        description: "このショートカット一覧を表示する",
    },
    FixedShortcutRow {
        keys: "Esc / Backspace",
        description: "一覧へ戻る。タイルモード中の Esc は先にタイルモードを閉じる",
    },
    FixedShortcutRow {
        keys: "← / →",
        description: "5秒戻る / 進む。タイルモード中はタイルカーソルを移動する",
    },
    FixedShortcutRow {
        keys: "Shift+← / Shift+→",
        description: "1秒戻る / 進む。タイルモード中はタイルカーソルを移動する",
    },
    FixedShortcutRow {
        keys: "Ctrl+← / Ctrl+→",
        description: "30秒戻る / 進む。タイルモード中はタイルカーソルを1行移動する",
    },
    FixedShortcutRow {
        keys: "Ctrl+Shift+← / Ctrl+Shift+→",
        description: "1フレーム戻る / 進む",
    },
    FixedShortcutRow {
        keys: "Home / End",
        description: "先頭または末尾の項目へ移動する",
    },
    FixedShortcutRow {
        keys: "F11",
        description: "ウィンドウ内表示と全画面表示を切り替える",
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
        keys: "Enter",
        description: "多角形ツールの頂点列を確定する",
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
        keys: "Enter",
        description: "多角形ツールの頂点列を確定する",
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
        keys: "Enter",
        description: "多角形マスクの頂点列を確定する",
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

#[derive(Clone, Copy)]
enum ShortcutHelpContext {
    Grid,
    FsImage,
    FsVideo,
    Erase,
    Conceal,
    Crop,
    LocalAdjust,
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
        }
    }

    fn includes_row(self, row: &CommandDisplayRow) -> bool {
        match self {
            Self::Grid => true,
            Self::FsImage => {
                row.spec.scope != CommandScope::Global
                    || row.spec.action == KeyAction::ToggleDetachedViewerMode
            }
            Self::FsVideo => video_help_includes_row(row),
            Self::Erase | Self::Conceal | Self::Crop | Self::LocalAdjust => true,
        }
    }
}

impl App {
    pub(crate) fn show_context_shortcuts_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_context_shortcuts_help {
            return;
        }

        let help_context = self.current_shortcut_help_context();

        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let dialog_pos = ctx.content_rect().min + egui::vec2(72.0, 52.0);
        let scroll_max_h = (ctx.content_rect().height() - 180.0).min(620.0).max(160.0);
        let rows = self
            .keymap
            .command_display_rows_for_active_scopes(help_context.active_scopes(), true)
            .into_iter()
            .filter(|row| help_context.includes_row(row))
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
                        draw_unassigned_command_rows(ui, &rows);
                        draw_fixed_rows(ui, help_context.fixed_rows());
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
        if self.export_crop_mode {
            return ShortcutHelpContext::Crop;
        }
        if let Some(fs_idx) = self.fullscreen_idx
            && matches!(self.items.get(fs_idx), Some(GridItem::Video(_)))
        {
            return ShortcutHelpContext::FsVideo;
        }
        if let Some(fs_idx) = self.fullscreen_idx
            && matches!(
                self.items.get(fs_idx),
                Some(
                    GridItem::Image(_)
                        | GridItem::ZipImage { .. }
                        | GridItem::PdfPage { .. }
                        | GridItem::ZipSeparator { .. }
                )
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
        | KeyAction::FsToggleMetadata
        | KeyAction::FsCtrlNavPrev
        | KeyAction::FsCtrlNavNext
        | KeyAction::FsSiblingPrev
        | KeyAction::FsSiblingNext => true,
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

fn draw_unassigned_command_rows(ui: &mut egui::Ui, rows: &[CommandDisplayRow]) {
    if !rows.iter().any(|row| row.shortcut_labels.is_empty()) {
        return;
    }

    ui.add_space(10.0);
    ui.label(egui::RichText::new("キー未設定 / 無効化中").strong());
    ui.label("左の名前は keymap.ini の Action 名です。");
    ui.add_space(2.0);
    egui::Grid::new("context_shortcuts_unassigned")
        .num_columns(2)
        .spacing([18.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for row in rows.iter().filter(|row| row.shortcut_labels.is_empty()) {
                ui.monospace(row.spec.ini_name());
                ui.label(row.spec.description());
                ui.end_row();
            }
        });
}

fn draw_fixed_rows(ui: &mut egui::Ui, rows: &[FixedShortcutRow]) {
    ui.add_space(10.0);
    ui.label(egui::RichText::new("固定キー").strong());
    ui.add_space(2.0);
    egui::Grid::new(("context_shortcuts_fixed", rows.as_ptr() as usize))
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
