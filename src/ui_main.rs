//! メイン画面の UI コンポーネント描画。
//!
//! `App::update()` から呼ばれるメニューバー・ツールバー・フォルダバー・
//! グリッド・進捗オーバーレイ・選択情報オーバーレイの描画メソッドを集約。

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use eframe::egui;

use crate::app::{App, FacetField, LazyColumnState, QuickFolderSlotId, QuickFolderSwitchTarget};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::keymap::{KeyAction, Keymap, MenuCommandId, TopMenuId, resolve_menu_layout};
use crate::settings::{
    DetailsColumnId, DetailsColumnWidth, DetailsSortKey, FacetDatePreset, FacetEditFlag,
    FacetItemKind, FacetSizePreset, FacetTagMode, GridViewMode,
};
// open_external_player はグリッドからは使わなくなった (動画はフルスクリーン化 →
// インライン再生)。フォルダ系は別途同モジュールから直接呼んでいる箇所がある。

use crate::ui_helpers::{
    HoverTipExt, PROGRESS_BG_COLOR, PROGRESS_LABEL_COLOR, PROGRESS_NORMAL_COLOR,
    PROGRESS_UPGRADE_COLOR,
};

const BOOK_REORDER_DEFAULT_TILE_PX: f32 = 78.0;
const BOOK_REORDER_MIN_TILE_PX: f32 = 64.0;
const BOOK_REORDER_MAX_TILE_PX: f32 = 132.0;
const BOOK_REORDER_THUMB_DECODE_PX: u32 = 360;
const BOOK_REORDER_THUMB_MAX_IN_FLIGHT: usize = 4;
const BOOK_REORDER_THUMB_MAX_RECV_PER_FRAME: usize = 32;
const BOOK_REORDER_THUMB_MAX_UPLOADS_PER_FRAME: usize = 1;
const BOOK_REORDER_DEFAULT_WINDOW_W: f32 = 920.0;
const BOOK_REORDER_DEFAULT_WINDOW_H: f32 = 680.0;
const BOOK_REORDER_MIN_WINDOW_W: f32 = 560.0;
const BOOK_REORDER_MIN_WINDOW_H: f32 = 360.0;
const BOOK_REORDER_SCROLLBAR_RESERVE_PX: f32 = 28.0;
const BOOK_REORDER_AUTO_SCROLL_EDGE_PX: f32 = 64.0;
const BOOK_REORDER_AUTO_SCROLL_MAX_STEP_PX: f32 = 34.0;
const COLOR_FILTER_PRESETS: [[u8; 3]; 12] = [
    [86, 86, 86],
    [255, 255, 255],
    [178, 178, 178],
    [185, 154, 118],
    [240, 142, 184],
    [255, 79, 79],
    [255, 181, 106],
    [255, 218, 91],
    [101, 202, 160],
    [100, 199, 201],
    [81, 142, 229],
    [124, 98, 232],
];

#[derive(Clone, Copy)]
enum BookReorderScrollKey {
    PageUp,
    PageDown,
    Home,
    End,
}

// ── ★フィルタのツールバー挙動 (Ctrl/Shift/右クリック) ─────────────────
//
// 通常クリック: そのバケットをトグル
// Ctrl+クリック: solo (そのバケットだけ ON)。同 solo 状態で再クリック → 全 ON (DAW 流)
// Shift+クリック: threshold (そのバケット以上 ON)。同状態で再クリック → 全 ON
// 右クリック: コンテキストメニューから同 3 操作 (こちらは toggle せず常に「set」)

/// フォルダバーの 📌 ボタンが受けたクリック種別 (closure 内で `self` への
/// ミュータブル呼び出しを避けるため、closure を抜けてから dispatch する)。
#[derive(Clone, Copy)]
enum PinButtonClick {
    None,
    /// 左クリック: 選択 item が現在の pin と一致なら解除、不一致なら set。
    Toggle,
    /// 右クリック: 解除。
    Remove,
}

#[derive(Debug)]
pub(crate) enum AddressBarNav {
    Direct(PathBuf),
    DriveList(Option<PathBuf>),
    ReadingHistory,
    BooksRoot,
    HistoryBack,
    HistoryForward,
}

#[derive(Clone, Copy)]
enum FavoriteButtonClick {
    None,
    Add,
    Edit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TagViewMenuChoice {
    pub name: String,
    pub tag_key: String,
    pub count: usize,
}

fn resolve_folder_bar_nav_path(path: &Path) -> Option<PathBuf> {
    let path = normalize_folder_bar_input_path(path);
    let convertible_archive = path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(crate::archive_converter::ArchiveFormat::from_extension)
            .is_some();
    if convertible_archive {
        Some(path)
    } else {
        crate::folder_tree::resolve_openable_path(&path)
    }
}

fn normalize_folder_bar_input_path(path: &Path) -> PathBuf {
    let raw = path.as_os_str().to_string_lossy();
    let trimmed = raw.trim();
    if trimmed.len() == 2 {
        let bytes = trimmed.as_bytes();
        if bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            return PathBuf::from(format!("{}\\", trimmed));
        }
    }
    path.to_path_buf()
}

/// 📌 ボタン描画用の状態スナップショット。`render_address_bar` 入口で 1 度算出する。
pub(crate) struct FolderPinButtonState {
    /// ボタンを enable にして良いか (false なら disabled + tooltip 表示)
    pub enabled: bool,
    /// hover 時の tooltip 文字列
    pub tooltip: String,
    /// 現在選択中の item が既に container の pin source と一致しているか
    /// (true: 強調アイコンで「クリックで解除」を示唆)
    pub matches_current_pin: bool,
}

#[derive(Clone, Copy)]
enum RatingFilterOp {
    Toggle,
    Solo,
    /// ★N + 未評価: `rating_filter[0]` と `rating_filter[idx]` だけを ON にする。
    ///
    /// ★5 を Ctrl+クリックするとフォルダまで消えてナビゲーションできなくなる問題への対処。
    /// 「★N の画像をフォルダツリーで探す」ワークフロー向け。ただし `rating_filter` は
    /// コンテナ (Folder/ZIP/PDF) と画像系 (Image/ZipImage/PdfPage) の両方に同じバケットを
    /// 適用するため、副次的に **未評価の通常画像** も表示される (UI 上は意図した挙動として
    /// ラベルを「★N と未評価」としている)。「フォルダだけを残す」ためには
    /// `[bool; 6]` では表現できず kind-aware な別モードが必要で、v0.8.2 以降の検討事項。
    /// idx=0 では意味をなさないので `apply_rating_filter_op` は idx>=1 前提
    /// (idx=0 なら Solo と同値)。
    SoloWithUnrated,
    Threshold,
    AllOn,
}

/// グリッドのセル寸法 (`cell_w`, `cell_h`) を計算する。
///
/// `avail_w <= 0` (chrome が幅を食い切った) は `None` を返してグリッド描画を skip。
/// `MIN_CELL_PX` 下限を強制しないと、`viewport_h / cell_h` が数百〜数千行に暴発して
/// 1 フレームで数千セル描画して UI フリーズする (極端に窓を狭めた時の実害バグ)。
const MIN_CELL_PX: f32 = 32.0;
fn compute_cell_size(avail_w: f32, cols: usize, height_ratio: f32) -> Option<(f32, f32)> {
    if avail_w <= 0.0 {
        return None;
    }
    let cols = cols.max(1);
    let cell_w = (avail_w / cols as f32).floor().max(MIN_CELL_PX);
    let cell_h = (cell_w * height_ratio).round().max(MIN_CELL_PX);
    Some((cell_w, cell_h))
}

fn snapped_scroll_extent(natural_h: f32, viewport_h: f32, row_h: f32) -> (f32, f32) {
    let natural_h = natural_h.max(0.0);
    let viewport_h = viewport_h.max(0.0);
    if natural_h <= viewport_h {
        return (natural_h, 0.0);
    }

    let row_h = row_h.max(1.0);
    let raw_max = natural_h - viewport_h;
    let max_offset = (raw_max / row_h).ceil() * row_h;
    (max_offset + viewport_h, max_offset)
}

fn is_rating_solo(rf: &[bool; 6], idx: usize) -> bool {
    (0..6).all(|i| rf[i] == (i == idx))
}

fn is_rating_threshold(rf: &[bool; 6], idx: usize) -> bool {
    (0..idx).all(|i| !rf[i]) && (idx..6).all(|i| rf[i])
}

/// 現在フィルタが「★N + 未評価」状態か (idx>=1 前提)。
fn is_rating_solo_with_unrated(rf: &[bool; 6], idx: usize) -> bool {
    if idx == 0 {
        return false;
    }
    rf[0] && rf[idx] && (1..6).all(|i| i == idx || !rf[i])
}

fn apply_rating_filter_op(rf: &mut [bool; 6], op: RatingFilterOp, idx: usize) {
    match op {
        RatingFilterOp::Toggle => rf[idx] = !rf[idx],
        RatingFilterOp::Solo => {
            for i in 0..6 {
                rf[i] = i == idx;
            }
        }
        RatingFilterOp::SoloWithUnrated => {
            for i in 0..6 {
                rf[i] = i == 0 || i == idx;
            }
        }
        RatingFilterOp::Threshold => {
            for i in 0..6 {
                rf[i] = i >= idx;
            }
        }
        RatingFilterOp::AllOn => {
            *rf = crate::settings::default_rating_filter();
        }
    }
}

fn rating_button_label(idx: usize) -> String {
    if idx == 0 {
        "なし".to_string()
    } else {
        "★".repeat(idx)
    }
}

fn rating_view_menu_label(stars: u8, counts: Option<[usize; 6]>) -> String {
    let label = "★".repeat(stars as usize);
    match counts.and_then(|c| c.get(stars as usize).copied()) {
        Some(count) => format!("{label} ({count})"),
        None => label,
    }
}

fn rating_solo_menu_label(idx: usize) -> String {
    if idx == 0 {
        "未評価のみ表示 (Ctrl+クリック)".to_string()
    } else {
        format!("★{} のみ表示 (Ctrl+クリック)", idx)
    }
}

fn rating_threshold_menu_label(idx: usize) -> String {
    if idx == 0 {
        "すべて表示 (Shift+クリック)".to_string()
    } else {
        format!("★{} 以上を表示 (Shift+クリック)", idx)
    }
}

/// 「★N と未評価」(= ★N + なし) メニュー用ラベル。idx>=1 のみ有効。
/// 「フォルダだけ」ではなく `rating_filter[0]` バケットに入るもの全部 (未評価画像 /
/// 未評価 ZIP 内画像 / 未評価 PDF ページ + フォルダ / ZIP / PDF) が対象なので、
/// 文言は「未評価」に寄せて誤解を避ける。
fn rating_solo_with_unrated_menu_label(idx: usize) -> String {
    format!("★{} と未評価 (Ctrl+Shift+クリック)", idx)
}

fn rating_tooltip(keymap: &Keymap, idx: usize) -> String {
    let shortcut = keymap.first_rating_chord_label(false, idx as u8);
    if idx == 0 {
        let shortcut = shortcut
            .map(|label| format!(" [{label} で解除]"))
            .unwrap_or_default();
        format!(
            "未評価を表示{shortcut}\n通常クリック: 切り替え\nCtrl+クリック: これのみ\nShift+クリック: すべて表示"
        )
    } else {
        let shortcut = shortcut
            .map(|label| format!(" [{label} で付与]"))
            .unwrap_or_default();
        format!(
            "★{idx} を表示{shortcut}\n通常クリック: 切り替え\nCtrl+クリック: これのみ\nShift+クリック: ★{idx} 以上\nCtrl+Shift+クリック: ★{idx} と未評価"
        )
    }
}

fn folder_rating_tooltip(keymap: &Keymap) -> String {
    match keymap.rating_chord_summary_label(true) {
        Some(label) => format!("このフォルダ / ZIP / PDF のレーティング [{label}]"),
        None => "このフォルダ / ZIP / PDF のレーティング".to_string(),
    }
}

fn counts_as_thumbnail_item(item: &GridItem) -> bool {
    !matches!(item, GridItem::ZipSeparator { .. })
}

fn thumbnail_count_label(items: &[GridItem], visible_indices: &[usize]) -> String {
    let total = items
        .iter()
        .filter(|item| counts_as_thumbnail_item(item))
        .count();
    let visible = visible_indices
        .iter()
        .filter_map(|&idx| items.get(idx))
        .filter(|item| counts_as_thumbnail_item(item))
        .count();
    let width = total.max(1).to_string().len();
    format!("({:>width$}/{})", visible, total, width = width)
}

fn filtered_count_label(items: &[GridItem], visible_indices: &[usize]) -> String {
    let total = items
        .iter()
        .filter(|item| counts_as_thumbnail_item(item))
        .count();
    let visible = visible_indices
        .iter()
        .filter_map(|&idx| items.get(idx))
        .filter(|item| counts_as_thumbnail_item(item))
        .count();
    format!("{visible} / {total} 件")
}

fn facet_menu_label(base: &str, active: usize) -> String {
    if active == 0 {
        base.to_string()
    } else {
        format!("{base} ({active})")
    }
}

fn sticky_facet_menu_config() -> egui::containers::menu::MenuConfig {
    egui::containers::menu::MenuConfig::new()
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
}

fn prepare_facet_menu_popup(ui: &mut egui::Ui) {
    ui.set_min_width(180.0);
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
}

fn prepare_ai_facet_menu_popup(ui: &mut egui::Ui) {
    ui.set_min_width(260.0);
    ui.set_max_width(520.0);
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
}

fn prepare_place_facet_menu_popup(ui: &mut egui::Ui) {
    ui.set_width(PLACE_FACET_MENU_WIDTH);
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
}

const AI_FACET_MENU_WIDTH: f32 = 520.0;
const AI_FACET_MENU_VISIBLE_ROWS: usize = 18;
const PLACE_FACET_MENU_WIDTH: f32 = 520.0;
const TAG_FACET_MENU_WIDTH: f32 = 360.0;
const FACET_CHOICE_MENU_VISIBLE_ROWS: usize = 24;

fn ai_facet_choice_body_height(ui: &egui::Ui, choice_count: usize) -> f32 {
    let spacing = ui.spacing();
    let row_h = spacing.interact_size.y.max(22.0);
    let rows = choice_count.min(AI_FACET_MENU_VISIBLE_ROWS).max(1) as f32;
    let gaps = (rows - 1.0).max(0.0) * spacing.item_spacing.y;
    row_h * rows + gaps + spacing.item_spacing.y * 2.0 + 4.0
}

fn show_ai_facet_choices<R>(
    ui: &mut egui::Ui,
    choice_count: usize,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    if choice_count <= AI_FACET_MENU_VISIBLE_ROWS {
        return ui
            .scope(|ui| {
                ui.set_width(AI_FACET_MENU_WIDTH);
                add_contents(ui)
            })
            .inner;
    }

    let body_height = ai_facet_choice_body_height(ui, choice_count);
    ui.allocate_ui_with_layout(
        egui::vec2(AI_FACET_MENU_WIDTH, body_height),
        egui::Layout::top_down(egui::Align::LEFT),
        |ui| {
            ui.set_min_height(body_height);
            ui.set_width(AI_FACET_MENU_WIDTH);
            egui::ScrollArea::vertical()
                .max_height(body_height)
                .auto_shrink([false, false])
                .show(ui, add_contents)
                .inner
        },
    )
    .inner
}

fn facet_choice_body_height(ui: &egui::Ui, choice_count: usize) -> f32 {
    let spacing = ui.spacing();
    let row_h = spacing.interact_size.y.max(22.0);
    let rows = choice_count.min(FACET_CHOICE_MENU_VISIBLE_ROWS).max(1) as f32;
    let gaps = (rows - 1.0).max(0.0) * spacing.item_spacing.y;
    row_h * rows + gaps + spacing.item_spacing.y * 2.0 + 4.0
}

fn show_scrollable_facet_choices<R>(
    ui: &mut egui::Ui,
    width: f32,
    choice_count: usize,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    if choice_count <= FACET_CHOICE_MENU_VISIBLE_ROWS {
        return ui
            .scope(|ui| {
                ui.set_width(width);
                add_contents(ui)
            })
            .inner;
    }

    let body_height = facet_choice_body_height(ui, choice_count);
    ui.allocate_ui_with_layout(
        egui::vec2(width, body_height),
        egui::Layout::top_down(egui::Align::LEFT),
        |ui| {
            ui.set_min_height(body_height);
            ui.set_width(width);
            egui::ScrollArea::vertical()
                .max_height(body_height)
                .auto_shrink([false, false])
                .show(ui, add_contents)
                .inner
        },
    )
    .inner
}

fn show_virtualized_facet_choice_rows(
    ui: &mut egui::Ui,
    width: f32,
    choice_count: usize,
    mut add_row: impl FnMut(&mut egui::Ui, usize),
) {
    if choice_count <= FACET_CHOICE_MENU_VISIBLE_ROWS {
        ui.scope(|ui| {
            ui.set_width(width);
            for idx in 0..choice_count {
                add_row(ui, idx);
            }
        });
        return;
    }

    let spacing = ui.spacing();
    let row_h = spacing.interact_size.y.max(22.0) + spacing.item_spacing.y;
    let body_height = facet_choice_body_height(ui, choice_count);
    ui.allocate_ui_with_layout(
        egui::vec2(width, body_height),
        egui::Layout::top_down(egui::Align::LEFT),
        |ui| {
            ui.set_min_height(body_height);
            ui.set_width(width);
            egui::ScrollArea::vertical()
                .max_height(body_height)
                .auto_shrink([false, false])
                .show_rows(ui, row_h, choice_count, |ui, row_range| {
                    ui.set_width(width);
                    for idx in row_range {
                        add_row(ui, idx);
                    }
                });
        },
    );
}

fn draw_facet_checkbox_choice(
    ui: &mut egui::Ui,
    selected: &mut bool,
    text: String,
    hover_text: &str,
) -> bool {
    let row_h = ui.spacing().interact_size.y.max(22.0);
    let row_width = ui.available_width().max(1.0);
    let mut changed = false;
    let response = ui
        .allocate_ui_with_layout(
            egui::vec2(row_width, row_h),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let checkbox_response = ui.add(egui::Checkbox::without_text(selected));
                if checkbox_response.changed() {
                    changed = true;
                }
                let label_width = ui.available_width().max(1.0);
                let (label_rect, label_response) =
                    ui.allocate_exact_size(egui::vec2(label_width, row_h), egui::Sense::click());
                if ui.is_rect_visible(label_rect) {
                    let text_color = ui.style().interact(&label_response).text_color();
                    let font_id = egui::TextStyle::Body.resolve(ui.style());
                    // 1 行に収め、はみ出す分は省略記号 (…) で truncate する。hard clip ではなく
                    // galley の overflow_character を使うことで Label::truncate() 相当の見た目を保つ。
                    let mut job = egui::text::LayoutJob::single_section(
                        text,
                        egui::text::TextFormat::simple(font_id, text_color),
                    );
                    job.wrap = egui::text::TextWrapping {
                        max_width: label_rect.width(),
                        max_rows: 1,
                        break_anywhere: true,
                        overflow_character: Some('…'),
                    };
                    let galley = ui.painter().layout_job(job);
                    let text_pos = egui::pos2(
                        label_rect.left(),
                        label_rect.center().y - galley.size().y * 0.5,
                    );
                    ui.painter().galley(text_pos, galley, text_color);
                }
                if label_response.clicked() {
                    *selected = !*selected;
                    changed = true;
                }
                checkbox_response.union(label_response)
            },
        )
        .inner;
    response.on_hover_text(hover_text);
    changed
}

fn consume_wheel_input(ctx: &egui::Context) {
    ctx.input_mut(|i| {
        i.raw_scroll_delta = egui::Vec2::ZERO;
        i.smooth_scroll_delta = egui::Vec2::ZERO;
        i.events
            .retain(|e| !matches!(e, egui::Event::MouseWheel { .. }));
    });
}

fn suppress_menu_button_wheel_passthrough(ctx: &egui::Context, response: &egui::Response) {
    // egui popup/menu の ScrollArea は wheel を使っても raw input が残ることがある。
    // 背面のサムネイル一覧も同じ frame で描画されるため、menu_button が開いている間は
    // ここで wheel を消費して背面スクロールへの通り抜けを防ぐ。
    if egui::Popup::is_id_open(ctx, egui::Popup::default_response_id(response)) {
        consume_wheel_input(ctx);
    }
}

fn show_sticky_context_menu(response: &egui::Response, add_contents: impl FnOnce(&mut egui::Ui)) {
    // Keep the secondary-click settings menu on a separate popup id from a possible
    // primary-click menu_button on the same response. egui::Popup::context_menu
    // explicitly closes its popup id on primary clicks.
    let _ = egui::Popup::context_menu(response)
        .id(response.id.with("sticky_context_menu"))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(add_contents);
}

fn draw_sticky_settings_menu_header(ui: &mut egui::Ui, title: &str) {
    ui.set_min_width(220.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("×").on_hover_text("閉じる").clicked() {
                ui.close();
            }
        });
    });
}

fn draw_tag_view_menu_section(
    ui: &mut egui::Ui,
    title: &str,
    choices: &[TagViewMenuChoice],
    clicked_tag: &mut Option<String>,
) {
    ui.label(egui::RichText::new(title).strong());
    ui.add_space(2.0);
    if choices.is_empty() {
        ui.label(egui::RichText::new("なし").weak());
        return;
    }

    for choice in choices {
        let label = format!("#{} ({})", choice.name, choice.count);
        if ui.button(&label).clicked() {
            *clicked_tag = Some(crate::tags_db::format_display_tag(&choice.name));
            ui.close();
        }
    }
}

fn facet_chip(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text.into()).small().strong());
}

/// ★フィルタのボタン 1 個を描画し、状態が変わったら true を返す。
/// `enabled = false` の間はクリックを無視し、見た目も disabled スタイルで描画する。
///
/// `add_enabled_ui` で外側からまとめてしまうと、`horizontal_wrapped` 内では scope が
/// 残り幅だけの狭い子 UI を作るのでレイアウトが崩れる。そのため enabled は呼び出し側
/// から各ボタンに直接渡す。`context_menu` は egui 上、disabled でも開いてしまうので
/// `if enabled` で明示ガードする (`resp.clicked()` 側は `add_enabled` が消費するので
/// 二重ガードは belt-and-suspenders)。
/// ★ボタン右クリックメニューでの「付与」要求 (フィルタとは別操作)。
/// ★は左右クリック + 修飾キーが全てフィルタに使われているため、評価の付与は
/// 右クリックメニュー経由で行う (toolbar-customization-plan.md §1.1)。
enum RatingAssign {
    Selection(u8),
    Container(u8),
}

/// ツールバーの折りたたみ可能セクション先頭の ▶/▼ トグル (折りたたみモードのみ描画)。
/// 戻り値 = (インライン項目を描くべきか, トグルされた新 collapsed 値 or None)。
/// 展開=常にインライン描画 / プルダウン=インライン無し (ComboBox 側で出す)。
fn toolbar_section_fold_toggle(
    ui: &mut egui::Ui,
    mode: crate::settings::ToolbarSectionDisplay,
    collapsed: bool,
) -> (bool, Option<bool>) {
    use crate::settings::ToolbarSectionDisplay as D;
    match mode {
        D::Dropdown => (false, None),
        D::Collapsible => {
            let arrow = if collapsed { "▶" } else { "▼" };
            let toggled = ui
                .button(arrow)
                .on_hover_text("このセクションの折りたたみ")
                .clicked();
            (!collapsed, toggled.then_some(!collapsed))
        }
        D::Buttons | D::Unknown => (true, None),
    }
}

fn draw_rating_filter_button(
    ui: &mut egui::Ui,
    keymap: &Keymap,
    rf: &mut [bool; 6],
    idx: usize,
    enabled: bool,
    has_selection: bool,
) -> (bool, Option<RatingAssign>) {
    let sel = rf[idx];
    let resp = ui
        .add_enabled(
            enabled,
            egui::Button::selectable(sel, rating_button_label(idx)),
        )
        .hover_tip(rating_tooltip(keymap, idx));
    let mut changed = false;
    let mut assign: Option<RatingAssign> = None;
    if enabled && resp.clicked() {
        let mods = ui.input(|i| i.modifiers);
        // Windows 専用ビルドなので mods.command は ctrl と同値 (egui 内で alias)。
        // 既存コード (src/ui_main.rs:992 の Ctrl+クリック選択等) と合わせて ctrl のみを見る。
        // 優先順位: Ctrl+Shift > Ctrl > Shift > 通常。
        let op = if mods.ctrl && mods.shift && idx >= 1 {
            // ★N + 未評価 (= `rating_filter[0]` も ON)。idx=0 では意味を成さないので
            // 除外 (下の Ctrl 単独に落ちる)。
            if is_rating_solo_with_unrated(rf, idx) {
                RatingFilterOp::AllOn
            } else {
                RatingFilterOp::SoloWithUnrated
            }
        } else if mods.ctrl {
            if is_rating_solo(rf, idx) {
                RatingFilterOp::AllOn
            } else {
                RatingFilterOp::Solo
            }
        } else if mods.shift {
            if is_rating_threshold(rf, idx) {
                RatingFilterOp::AllOn
            } else {
                RatingFilterOp::Threshold
            }
        } else {
            RatingFilterOp::Toggle
        };
        apply_rating_filter_op(rf, op, idx);
        changed = true;
    }
    // 右クリックメニューは常に「set」(toggle せず) なので op を直接渡す。
    if enabled {
        resp.context_menu(|ui| {
            if ui.button(rating_solo_menu_label(idx)).clicked() {
                apply_rating_filter_op(rf, RatingFilterOp::Solo, idx);
                changed = true;
                ui.close();
            }
            if ui.button(rating_threshold_menu_label(idx)).clicked() {
                apply_rating_filter_op(rf, RatingFilterOp::Threshold, idx);
                changed = true;
                ui.close();
            }
            if idx >= 1
                && ui
                    .button(rating_solo_with_unrated_menu_label(idx))
                    .clicked()
            {
                apply_rating_filter_op(rf, RatingFilterOp::SoloWithUnrated, idx);
                changed = true;
                ui.close();
            }
            ui.separator();
            if ui.button("すべて表示").clicked() {
                apply_rating_filter_op(rf, RatingFilterOp::AllOn, idx);
                changed = true;
                ui.close();
            }
            // ── 評価の付与 (フィルタではなく) — toolbar-customization-plan.md §1.1 ──
            ui.separator();
            let assign_sel = if idx == 0 {
                "選択したアイテムを未評価に".to_string()
            } else {
                format!("選択したアイテムへ ★{idx} を付与")
            };
            if ui
                .add_enabled(has_selection, egui::Button::new(assign_sel))
                .clicked()
            {
                assign = Some(RatingAssign::Selection(idx as u8));
                ui.close();
            }
            let assign_cont = if idx == 0 {
                "この場所(コンテナ)を未評価に".to_string()
            } else {
                format!("この場所(コンテナ)へ ★{idx} を付与")
            };
            if ui.button(assign_cont).clicked() {
                assign = Some(RatingAssign::Container(idx as u8));
                ui.close();
            }
            // ★1〜5 では「未評価に戻す」も同じメニューに出す
            // (「なし」ボタンを探さずにその場で解除できるように)。
            if idx >= 1 {
                if ui
                    .add_enabled(
                        has_selection,
                        egui::Button::new("選択したアイテムを未評価に"),
                    )
                    .clicked()
                {
                    assign = Some(RatingAssign::Selection(0));
                    ui.close();
                }
                if ui.button("この場所(コンテナ)を未評価に").clicked() {
                    assign = Some(RatingAssign::Container(0));
                    ui.close();
                }
            }
        });
    }
    (changed, assign)
}

// ── native ファイル D&D (ドラッグでコピー送出) ───────────────────────
// 設計: docs/file-drag-drop-design.md §5.4

/// ドラッグ開始セルから「何をドラッグするか」を表す決定。
pub(crate) enum DragDecision {
    /// ドラッグを開始する。`paths` は空でない (index 昇順)。
    Start {
        paths: Vec<PathBuf>,
        /// 混在選択で仮想アイテムを除外したときの、ドラッグ完了後トースト文言。
        /// 除外なしなら `None`。
        post_drag_toast: Option<String>,
    },
    /// ドラッグはしないが即時トーストを出す (全選択が仮想アイテムだった等)。
    ImmediateToast(String),
    /// 何もしない (単体の仮想アイテム / セパレータ / 検索コンテナを掴んだ no-op)。
    None,
}

/// ドラッグ開始セル `idx` から、何をドラッグするかを決める純粋関数。
///
/// - `idx` が複数選択 (`checked`) の一部 → checked 全件 (index 昇順) の実パスを
///   ドラッグ対象にする。仮想アイテム (`ZipImage` / `PdfPage`) が混在していれば除外し、
///   完了後トーストで件数を明示する。実パスが 0 件なら即時トーストのみ。
/// - `idx` が複数選択外 → エクスプローラ流に、掴んだ単体だけをドラッグ
///   (実ファイル / 実フォルダのとき)。仮想アイテム等なら no-op。
///
/// 純粋関数にして `App` 構築なしでユニットテストできるようにしている。
pub(crate) fn decide_drag_payload(
    items: &[GridItem],
    checked: &std::collections::HashSet<usize>,
    idx: usize,
) -> DragDecision {
    if checked.contains(&idx) {
        // checked は HashSet で反復順が不定。index 昇順で安定させる
        // (= items の並び順 = 表示順)。
        let mut indices: Vec<usize> = checked.iter().copied().collect();
        indices.sort_unstable();
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut virtual_excluded = 0usize;
        for &i in &indices {
            let Some(item) = items.get(i) else { continue };
            if let Some(p) = item.drag_source_path() {
                paths.push(p.to_path_buf());
            } else if matches!(item, GridItem::ZipImage { .. } | GridItem::PdfPage { .. }) {
                virtual_excluded += 1;
            }
        }
        if paths.is_empty() {
            return DragDecision::ImmediateToast(
                "ドラッグできる実ファイル / フォルダが選択されていません".to_string(),
            );
        }
        let post_drag_toast = (virtual_excluded > 0).then(|| {
            format!(
                "{} 件のフォルダ内画像は除外しました。実ファイル / フォルダ {} 件をドラッグ対象にしました",
                virtual_excluded,
                paths.len(),
            )
        });
        DragDecision::Start {
            paths,
            post_drag_toast,
        }
    } else {
        match items.get(idx).and_then(GridItem::drag_source_path) {
            Some(p) => DragDecision::Start {
                paths: vec![p.to_path_buf()],
                post_drag_toast: None,
            },
            None => DragDecision::None,
        }
    }
}

fn menubar_hover_switch_target(
    open_popup_index: Option<usize>,
    hovered_index: Option<usize>,
) -> Option<usize> {
    match (open_popup_index, hovered_index) {
        (Some(open), Some(hovered)) if open != hovered => Some(hovered),
        _ => None,
    }
}

fn switch_menubar_popup_on_hover(ctx: &egui::Context, responses: &[egui::Response]) {
    let open_popup_index = responses.iter().position(|response| {
        egui::Popup::is_id_open(ctx, egui::Popup::default_response_id(response))
    });
    let hovered_index = responses.iter().position(egui::Response::hovered);

    if let Some(target_index) = menubar_hover_switch_target(open_popup_index, hovered_index) {
        egui::Popup::open_id(
            ctx,
            egui::Popup::default_response_id(&responses[target_index]),
        );
        ctx.request_repaint();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum DetailsColumn {
    Preview,
    Name,
    Rating,
    Tags,
    Kind,
    Size,
    Modified,
    Created,
    State,
    ImageDimensions,
    VideoDuration,
    VideoDimensions,
    VideoCodec,
}

impl DetailsColumn {
    fn all() -> &'static [Self] {
        &[
            Self::Preview,
            Self::Name,
            Self::Rating,
            Self::Tags,
            Self::Kind,
            Self::Size,
            Self::Modified,
            Self::Created,
            Self::State,
            Self::ImageDimensions,
            Self::VideoDuration,
            Self::VideoDimensions,
            Self::VideoCodec,
        ]
    }

    fn title(self) -> &'static str {
        match self {
            Self::Preview => "",
            Self::Name => "名前",
            Self::Rating => "★",
            Self::Tags => "タグ",
            Self::Kind => "種類",
            Self::Size => "サイズ",
            Self::Modified => "更新日時",
            Self::Created => "作成日時",
            Self::State => "状態",
            Self::ImageDimensions => "解像度",
            Self::VideoDuration => "動画長さ",
            Self::VideoDimensions => "動画解像度",
            Self::VideoCodec => "コーデック",
        }
    }

    fn id(self) -> DetailsColumnId {
        match self {
            Self::Preview => DetailsColumnId::Preview,
            Self::Name => DetailsColumnId::Name,
            Self::Rating => DetailsColumnId::Rating,
            Self::Tags => DetailsColumnId::Tags,
            Self::Kind => DetailsColumnId::Kind,
            Self::Size => DetailsColumnId::Size,
            Self::Modified => DetailsColumnId::Modified,
            Self::Created => DetailsColumnId::Created,
            Self::State => DetailsColumnId::State,
            Self::ImageDimensions => DetailsColumnId::ImageDimensions,
            Self::VideoDuration => DetailsColumnId::VideoDuration,
            Self::VideoDimensions => DetailsColumnId::VideoDimensions,
            Self::VideoCodec => DetailsColumnId::VideoCodec,
        }
    }

    fn from_id(id: DetailsColumnId) -> Self {
        match id {
            DetailsColumnId::Preview => Self::Preview,
            DetailsColumnId::Name => Self::Name,
            DetailsColumnId::Rating => Self::Rating,
            DetailsColumnId::Tags => Self::Tags,
            DetailsColumnId::Kind => Self::Kind,
            DetailsColumnId::Size => Self::Size,
            DetailsColumnId::Modified => Self::Modified,
            DetailsColumnId::Created => Self::Created,
            DetailsColumnId::State => Self::State,
            DetailsColumnId::ImageDimensions => Self::ImageDimensions,
            DetailsColumnId::VideoDuration => Self::VideoDuration,
            DetailsColumnId::VideoDimensions => Self::VideoDimensions,
            DetailsColumnId::VideoCodec => Self::VideoCodec,
        }
    }

    fn sort_key(self) -> Option<DetailsSortKey> {
        match self {
            Self::Preview => None,
            Self::Name => Some(DetailsSortKey::Name),
            Self::Rating => Some(DetailsSortKey::Rating),
            Self::Tags => Some(DetailsSortKey::Tags),
            Self::Kind => Some(DetailsSortKey::Kind),
            Self::Size => Some(DetailsSortKey::Size),
            Self::Modified => Some(DetailsSortKey::Modified),
            Self::Created => Some(DetailsSortKey::Created),
            Self::State => Some(DetailsSortKey::State),
            Self::ImageDimensions => Some(DetailsSortKey::ImageDimensions),
            Self::VideoDuration => Some(DetailsSortKey::VideoDuration),
            Self::VideoDimensions => Some(DetailsSortKey::VideoDimensions),
            Self::VideoCodec => Some(DetailsSortKey::VideoCodec),
        }
    }

    fn is_lazy(self) -> bool {
        matches!(
            self,
            Self::Created
                | Self::ImageDimensions
                | Self::VideoDuration
                | Self::VideoDimensions
                | Self::VideoCodec
        )
    }

    fn visible(self, settings: &crate::settings::Settings) -> bool {
        match self {
            Self::Preview => settings.details_show_preview,
            Self::Name => true,
            Self::Rating => settings.details_show_rating,
            Self::Tags => settings.details_show_tags,
            Self::Kind => settings.details_show_kind,
            Self::Size => settings.details_show_size,
            Self::Modified => settings.details_show_modified,
            Self::Created => settings.details_show_created,
            Self::State => settings.details_show_state,
            Self::ImageDimensions => settings.details_show_image_dimensions,
            Self::VideoDuration => settings.details_show_video_duration,
            Self::VideoDimensions => settings.details_show_video_dimensions,
            Self::VideoCodec => settings.details_show_video_codec,
        }
    }

    fn default_width(self) -> f32 {
        match self {
            Self::Preview => 34.0,
            Self::Name => 140.0,
            Self::Rating => 58.0,
            Self::Tags => 160.0,
            Self::Kind => 96.0,
            Self::Size => 92.0,
            Self::Modified | Self::Created => 138.0,
            Self::State => 92.0,
            Self::ImageDimensions => 108.0,
            Self::VideoDuration => 94.0,
            Self::VideoDimensions => 112.0,
            Self::VideoCodec => 112.0,
        }
    }

    fn min_width(self) -> f32 {
        match self {
            Self::Preview => 28.0,
            _ => 40.0,
        }
    }
}

#[derive(Clone, Copy)]
struct DetailsHeaderDrag {
    column: DetailsColumn,
    start: egui::Pos2,
    latest: egui::Pos2,
}

fn details_ordered_columns(
    settings: &crate::settings::Settings,
    include_hidden: bool,
) -> Vec<DetailsColumn> {
    let mut ordered = Vec::with_capacity(DetailsColumn::all().len());
    let source: Vec<DetailsColumnId> = if settings.details_column_order.is_empty() {
        DetailsColumnId::default_order().to_vec()
    } else {
        settings.details_column_order.clone()
    };
    for id in source {
        let col = DetailsColumn::from_id(id);
        if !ordered.contains(&col) {
            ordered.push(col);
        }
    }
    for &col in DetailsColumn::all() {
        if !ordered.contains(&col) {
            ordered.push(col);
        }
    }
    ordered
        .into_iter()
        .filter(|col| include_hidden || col.visible(settings))
        .collect()
}

fn details_column_width(settings: &crate::settings::Settings, col: DetailsColumn) -> f32 {
    if col == DetailsColumn::Name {
        return col.default_width();
    }
    settings
        .details_column_widths
        .iter()
        .find(|entry| entry.column == col.id())
        .map(|entry| entry.width)
        .unwrap_or_else(|| col.default_width())
        .clamp(col.min_width(), 800.0)
}

fn set_details_column_width(
    settings: &mut crate::settings::Settings,
    col: DetailsColumn,
    width: f32,
) -> bool {
    if col == DetailsColumn::Name {
        return false;
    }
    let width = width.clamp(col.min_width(), 800.0);
    if let Some(entry) = settings
        .details_column_widths
        .iter_mut()
        .find(|entry| entry.column == col.id())
    {
        if (entry.width - width).abs() <= 0.1 {
            return false;
        }
        entry.width = width;
    } else {
        settings.details_column_widths.push(DetailsColumnWidth {
            column: col.id(),
            width,
        });
    }
    true
}

/// 固定幅モード時の名前列幅 (clamp 済み)。
fn details_name_fixed_width(settings: &crate::settings::Settings) -> f32 {
    settings
        .details_name_width
        .clamp(DetailsColumn::Name.min_width(), 800.0)
}

/// 名前列を固定幅へ切り替えて幅を保存する。値が変わったら true。
fn set_details_name_width(settings: &mut crate::settings::Settings, width: f32) -> bool {
    let width = width.clamp(DetailsColumn::Name.min_width(), 800.0);
    if !settings.details_name_width_auto && (settings.details_name_width - width).abs() <= 0.1 {
        return false;
    }
    settings.details_name_width_auto = false;
    settings.details_name_width = width;
    true
}

/// 詳細表示の水平レイアウト。縦スクロールバーの gutter を考慮して、ヘッダと行の列が
/// ぴったり揃い、縦バー出現時に右端列が欠けたり不要な横スクロールバーが出たりしないようにする。
struct DetailsLayout {
    /// 名前列の実効幅。
    name_w: f32,
    /// 行 / ヘッダ背景の幅 (>= 全列合計。pane を埋める)。
    pane_w: f32,
    /// 外側 (水平) スクロールが扱う総コンテンツ幅 (= pane_w + gutter)。
    extent: f32,
}

fn details_fixed_columns_width(settings: &crate::settings::Settings) -> f32 {
    details_ordered_columns(settings, false)
        .into_iter()
        .filter(|col| *col != DetailsColumn::Name)
        .map(|col| details_column_width(settings, col))
        .sum()
}

fn details_layout(
    avail_w: f32,
    gutter: f32,
    settings: &crate::settings::Settings,
) -> DetailsLayout {
    let fixed = details_fixed_columns_width(settings);
    let columns_avail = (avail_w - gutter).max(0.0);
    let name_w = if settings.details_name_width_auto {
        (columns_avail - fixed).max(DetailsColumn::Name.default_width())
    } else {
        details_name_fixed_width(settings)
    };
    let columns_w = name_w + fixed;
    let pane_w = columns_w.max(columns_avail);
    let extent = pane_w + gutter;
    DetailsLayout {
        name_w,
        pane_w,
        extent,
    }
}

fn details_column_rects(
    rect: egui::Rect,
    settings: &crate::settings::Settings,
) -> Vec<(DetailsColumn, egui::Rect)> {
    let columns = details_ordered_columns(settings, false);
    let fixed: f32 = columns
        .iter()
        .copied()
        .filter(|col| *col != DetailsColumn::Name)
        .map(|col| details_column_width(settings, col))
        .sum();
    let name_width = if settings.details_name_width_auto {
        (rect.width() - fixed).max(DetailsColumn::Name.default_width())
    } else {
        details_name_fixed_width(settings)
    };
    let specs = columns
        .into_iter()
        .map(|col| {
            let width = if col == DetailsColumn::Name {
                name_width
            } else {
                details_column_width(settings, col)
            };
            (col, width)
        })
        .collect::<Vec<_>>();
    let mut x = rect.left();
    let mut out = Vec::with_capacity(specs.len());
    for (col, width) in specs.iter().copied() {
        let right = x + width;
        out.push((
            col,
            egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(right, rect.bottom())),
        ));
        x = right;
    }
    out
}

fn details_column_at_x(
    columns: &[(DetailsColumn, egui::Rect)],
    x: f32,
) -> Option<(DetailsColumn, egui::Rect)> {
    let first = columns.first().copied()?;
    let last = columns.last().copied()?;
    if x <= first.1.left() {
        return Some(first);
    }
    if x >= last.1.right() {
        return Some(last);
    }
    columns
        .iter()
        .copied()
        .find(|(_, rect)| x >= rect.left() && x <= rect.right())
}

fn clamp_details_tooltip_axis(value: f32, min: f32, max: f32) -> f32 {
    if max < min {
        min
    } else {
        value.clamp(min, max)
    }
}

fn reorder_details_column(
    settings: &mut crate::settings::Settings,
    dragged: DetailsColumn,
    target: DetailsColumn,
    insert_after_target: bool,
) -> bool {
    if dragged == target {
        return false;
    }
    let mut columns = details_ordered_columns(settings, true);
    let Some(from_pos) = columns.iter().position(|col| *col == dragged) else {
        return false;
    };
    let dragged = columns.remove(from_pos);
    let Some(target_pos) = columns.iter().position(|col| *col == target) else {
        return false;
    };
    let insert_pos = if insert_after_target {
        target_pos + 1
    } else {
        target_pos
    };
    columns.insert(insert_pos.min(columns.len()), dragged);
    let new_order = columns
        .into_iter()
        .map(DetailsColumn::id)
        .collect::<Vec<_>>();
    if settings.details_column_order == new_order {
        return false;
    }
    settings.details_column_order = new_order;
    true
}

fn finish_details_header_drag(
    settings: &mut crate::settings::Settings,
    columns: &[(DetailsColumn, egui::Rect)],
    drag: DetailsHeaderDrag,
    min_delta_x: f32,
) -> bool {
    if (drag.latest.x - drag.start.x).abs() < min_delta_x {
        return false;
    }
    let Some((target, target_rect)) = details_column_at_x(columns, drag.latest.x) else {
        return false;
    };
    let insert_after = drag.latest.x > target_rect.center().x;
    reorder_details_column(settings, drag.column, target, insert_after)
}

// ── ツールバー セクション カスタマイズ helper (v2.0.0 Phase 3) ──────────────

/// セクションの表示用ラベル (右クリックメニューのヘッダ / チェックボックス文言)。
fn toolbar_section_display_label(section: crate::settings::ToolbarSectionId) -> &'static str {
    use crate::settings::ToolbarSectionId as TS;
    match section {
        TS::FolderTree => "ツリー",
        TS::Bookshelf => "本棚",
        TS::Cols => "列",
        TS::Aspect => "比率",
        TS::Sort => "ソート",
        TS::Rating => "レーティング (★)",
        TS::Favorites => "お気に入り",
        TS::Tags => "タグ",
        TS::Unknown => "",
    }
}

/// ツールバーのドラッグ並べ替え中の状態 (ctx の temp data に保存)。
#[derive(Clone, Copy)]
struct ToolbarSectionDrag {
    section: crate::settings::ToolbarSectionId,
    start: egui::Pos2,
    latest: egui::Pos2,
}

/// 折返しツールバー上で、ポインタ位置が「可視セクション列」の何番目に挿入されるかを返す。
///
/// `anchors` は描画順 (= 現在の可視順) の (section, ラベル矩形)。詳細ヘッダーの 1 次元
/// hit-test と違い、ツールバーは複数行に折り返すので行 (y) も考慮する:
/// - ポインタより上の行にあるアンカーは「手前」(必ずカウント)
/// - 同じ行で、アンカー中心がポインタより左 (= ポインタがその右) なら「手前」
///
/// 戻り値は可視順での挿入インデックス (0..=anchors.len())。
fn toolbar_drop_index(
    anchors: &[(crate::settings::ToolbarSectionId, egui::Rect)],
    pointer: egui::Pos2,
) -> usize {
    let mut idx = 0usize;
    for (_, r) in anchors {
        let row_above = r.bottom() <= pointer.y;
        let same_row = pointer.y >= r.top() && pointer.y <= r.bottom();
        if row_above || (same_row && pointer.x >= r.center().x) {
            idx += 1;
        }
    }
    idx
}

/// ドラッグ並べ替えの結果となる新しい **全セクション順** を計算する (純関数、テスト対象)。
///
/// - `current_order`: 現在の全セクション順 (`ordered_with_fallback` の結果。非表示も含む)。
/// - `dragged`: 掴んでいるセクション。
/// - `before`: ドロップ先の直後にくる可視セクション (= この手前に挿入)。`None` = 末尾扱い。
/// - `last_visible`: 可視セクションの最後 (`before` が `None` のとき、この直後に置く)。
///
/// 非表示セクションの相対位置は保ったまま、`dragged` を可視セクション間の正しい位置へ移す。
/// 変化が無ければ `None`。
fn reorder_toolbar_section(
    current_order: &[crate::settings::ToolbarSectionId],
    dragged: crate::settings::ToolbarSectionId,
    before: Option<crate::settings::ToolbarSectionId>,
    last_visible: Option<crate::settings::ToolbarSectionId>,
) -> Option<Vec<crate::settings::ToolbarSectionId>> {
    let mut order: Vec<_> = current_order.to_vec();
    let from = order.iter().position(|&s| s == dragged)?;
    order.remove(from);
    let insert_at = match before {
        // 掴んでいるセクション自身の手前にドロップ = 元の位置 = 移動なし (Codex P2)。
        // ここを `_` に落とすと last_visible 経由で末尾へ誤移動してしまう。
        Some(b) if b == dragged => return None,
        Some(b) => order.iter().position(|&s| s == b).unwrap_or(order.len()),
        None => match last_visible {
            Some(l) if l != dragged => order
                .iter()
                .position(|&s| s == l)
                .map(|p| p + 1)
                .unwrap_or(order.len()),
            _ => order.len(),
        },
    };
    order.insert(insert_at.min(order.len()), dragged);
    if order == current_order {
        None
    } else {
        Some(order)
    }
}

/// ドラッグ並べ替え中、ドロップ先に I 字の挿入マーカーを描く (実機フィードバック 2026-06-20)。
/// `anchors` は前フレームの可視セクション**全体**の矩形 (ラベル + 各種ボタンを含む、描画順)。
/// マーカーは挿入先の **次セクションのラベル手前** (= 次セクション矩形の左端) に出す。次が無い
/// (末尾へドロップ) ときは、**前セクションの要素すべての後** (= 末尾セクション矩形の右端) に出す。
/// これでマーカーが「ソート:」等のラベル直後ではなく、ボタンの後ろ (= 実際の挿入位置) に揃う。
fn draw_toolbar_drop_indicator(
    ui: &egui::Ui,
    anchors: &[(crate::settings::ToolbarSectionId, egui::Rect)],
    pointer: egui::Pos2,
) {
    if anchors.is_empty() {
        return;
    }
    let vis_idx = toolbar_drop_index(anchors, pointer);
    let (x, top, bottom) = if vis_idx < anchors.len() {
        // 次セクションのラベル手前 (= 次セクション全体矩形の左端)。
        let r = anchors[vis_idx].1;
        (r.left() - 2.0, r.top(), r.bottom())
    } else {
        // 末尾へドロップ: 末尾セクションの要素すべての後 (= 末尾セクション矩形の右端)。
        let r = anchors[anchors.len() - 1].1;
        (r.right() + 2.0, r.top(), r.bottom())
    };
    let color = ui.visuals().selection.bg_fill;
    let stroke = egui::Stroke::new(2.0, color);
    // パネルより前面に描く (セパレータやボタンに隠れないように)。
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("toolbar_drop_indicator"),
    ));
    painter.line_segment([egui::pos2(x, top), egui::pos2(x, bottom)], stroke);
    // I 字の上下キャップ。
    let cap = 4.0;
    painter.line_segment([egui::pos2(x - cap, top), egui::pos2(x + cap, top)], stroke);
    painter.line_segment(
        [egui::pos2(x - cap, bottom), egui::pos2(x + cap, bottom)],
        stroke,
    );
}

/// セクションの表示フラグ (show_toolbar_*) を設定する。
/// 未知セクションは no-op。
fn set_toolbar_section_visible(
    settings: &mut crate::settings::Settings,
    section: crate::settings::ToolbarSectionId,
    visible: bool,
) {
    use crate::settings::ToolbarSectionId as TS;
    match section {
        TS::FolderTree => settings.show_toolbar_folder_tree_button = visible,
        TS::Bookshelf => settings.show_toolbar_bookshelf = visible,
        TS::Cols => settings.show_toolbar_cols = visible,
        TS::Aspect => settings.show_toolbar_aspect = visible,
        TS::Sort => settings.show_toolbar_sort = visible,
        TS::Rating => settings.show_toolbar_rating = visible,
        TS::Favorites => settings.show_toolbar_favorites = visible,
        TS::Tags => settings.show_toolbar_tags = visible,
        TS::Unknown => {}
    }
}

#[cfg(test)]
fn adjusted_book_reorder_insert_index(src: usize, insert_index: usize, len: usize) -> usize {
    let insert_index = insert_index.min(len);
    if src < insert_index {
        insert_index.saturating_sub(1)
    } else {
        insert_index
    }
}

fn book_reorder_entry_key(entry: &crate::books::BookPageEntry) -> String {
    crate::search_index_db::normalize_path(&entry.path)
}

fn book_reorder_select_single(state: &mut crate::app::BookReorderState, index: usize) {
    state.selected_keys.clear();
    if let Some(entry) = state.entries.get(index) {
        state.selected_keys.insert(book_reorder_entry_key(entry));
        state.selected = Some(index);
        state.selection_anchor = Some(index);
    }
}

fn book_reorder_toggle_selection(state: &mut crate::app::BookReorderState, index: usize) {
    let Some(entry) = state.entries.get(index) else {
        return;
    };
    let key = book_reorder_entry_key(entry);
    if !state.selected_keys.remove(&key) {
        state.selected_keys.insert(key);
    }
    if state.selected_keys.is_empty() {
        state.selected_keys.insert(book_reorder_entry_key(entry));
    }
    state.selected = Some(index);
    state.selection_anchor = Some(index);
}

fn book_reorder_select_range(state: &mut crate::app::BookReorderState, index: usize) {
    if state.entries.is_empty() {
        return;
    }
    let anchor = state
        .selection_anchor
        .unwrap_or_else(|| state.selected.unwrap_or(index))
        .min(state.entries.len() - 1);
    let start = anchor.min(index);
    let end = anchor.max(index).min(state.entries.len() - 1);
    state.selected_keys.clear();
    for entry in &state.entries[start..=end] {
        state.selected_keys.insert(book_reorder_entry_key(entry));
    }
    state.selected = Some(index.min(state.entries.len() - 1));
}

fn ensure_book_reorder_selection(state: &mut crate::app::BookReorderState) {
    let live_keys = state
        .entries
        .iter()
        .map(book_reorder_entry_key)
        .collect::<HashSet<_>>();
    state.selected_keys.retain(|key| live_keys.contains(key));
    if state.entries.is_empty() {
        state.selected = None;
        state.selection_anchor = None;
        return;
    }
    let selected = state
        .selected
        .unwrap_or(0)
        .min(state.entries.len().saturating_sub(1));
    if state.selected_keys.is_empty() {
        book_reorder_select_single(state, selected);
        return;
    }
    state.selected = Some(selected);
    state.selection_anchor = state
        .selection_anchor
        .map(|anchor| anchor.min(state.entries.len().saturating_sub(1)))
        .or(Some(selected));
}

fn selected_book_reorder_indices(
    entries: &[crate::books::BookPageEntry],
    selected_keys: &HashSet<String>,
) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| {
            selected_keys
                .contains(&book_reorder_entry_key(entry))
                .then_some(idx)
        })
        .collect()
}

fn adjusted_book_reorder_group_insert_index(
    selected_indices: &[usize],
    insert_index: usize,
    len: usize,
) -> usize {
    let insert_index = insert_index.min(len);
    let removed_before = selected_indices
        .iter()
        .filter(|idx| **idx < insert_index)
        .count();
    insert_index.saturating_sub(removed_before)
}

fn move_selected_book_reorder_group(
    state: &mut crate::app::BookReorderState,
    insert_index: usize,
) -> bool {
    let selected_indices = selected_book_reorder_indices(&state.entries, &state.selected_keys);
    if selected_indices.is_empty() {
        return false;
    }
    let before = state
        .entries
        .iter()
        .map(book_reorder_entry_key)
        .collect::<Vec<_>>();
    let focus_key = state
        .selected
        .and_then(|idx| state.entries.get(idx))
        .map(book_reorder_entry_key);
    let dst = adjusted_book_reorder_group_insert_index(
        &selected_indices,
        insert_index,
        state.entries.len(),
    );
    let selected_keys = state.selected_keys.clone();
    let mut moving = Vec::with_capacity(selected_indices.len());
    let mut remaining =
        Vec::with_capacity(state.entries.len().saturating_sub(selected_indices.len()));
    for entry in state.entries.drain(..) {
        if selected_keys.contains(&book_reorder_entry_key(&entry)) {
            moving.push(entry);
        } else {
            remaining.push(entry);
        }
    }
    let dst = dst.min(remaining.len());
    remaining.splice(dst..dst, moving);
    state.entries = remaining;
    state.selected = focus_key
        .as_ref()
        .and_then(|key| {
            state
                .entries
                .iter()
                .position(|entry| &book_reorder_entry_key(entry) == key)
        })
        .or_else(|| {
            selected_book_reorder_indices(&state.entries, &state.selected_keys)
                .first()
                .copied()
        });
    let after = state
        .entries
        .iter()
        .map(book_reorder_entry_key)
        .collect::<Vec<_>>();
    before != after
}

fn move_selected_book_reorder_by(state: &mut crate::app::BookReorderState, delta: i32) -> bool {
    let selected_indices = selected_book_reorder_indices(&state.entries, &state.selected_keys);
    if selected_indices.is_empty() {
        return false;
    }
    let focus_key = state
        .selected
        .and_then(|idx| state.entries.get(idx))
        .map(book_reorder_entry_key);
    let before = state
        .entries
        .iter()
        .map(book_reorder_entry_key)
        .collect::<Vec<_>>();
    if delta < 0 {
        for idx in 1..state.entries.len() {
            let key = book_reorder_entry_key(&state.entries[idx]);
            let prev_key = book_reorder_entry_key(&state.entries[idx - 1]);
            if state.selected_keys.contains(&key) && !state.selected_keys.contains(&prev_key) {
                state.entries.swap(idx, idx - 1);
            }
        }
    } else if delta > 0 {
        for idx in (0..state.entries.len().saturating_sub(1)).rev() {
            let key = book_reorder_entry_key(&state.entries[idx]);
            let next_key = book_reorder_entry_key(&state.entries[idx + 1]);
            if state.selected_keys.contains(&key) && !state.selected_keys.contains(&next_key) {
                state.entries.swap(idx, idx + 1);
            }
        }
    }
    state.selected = focus_key
        .as_ref()
        .and_then(|key| {
            state
                .entries
                .iter()
                .position(|entry| &book_reorder_entry_key(entry) == key)
        })
        .or_else(|| {
            selected_book_reorder_indices(&state.entries, &state.selected_keys)
                .first()
                .copied()
        });
    let after = state
        .entries
        .iter()
        .map(book_reorder_entry_key)
        .collect::<Vec<_>>();
    before != after
}

fn book_reorder_grid_columns(available_width: f32, tile_width: f32, gap: f32) -> usize {
    let content_width = (available_width - BOOK_REORDER_SCROLLBAR_RESERVE_PX).max(tile_width);
    let natural_columns = ((content_width + gap) / (tile_width + gap)).floor();
    natural_columns.max(4.0) as usize
}

fn book_reorder_scroll_height(available_height: f32, rows: usize, row_height: f32) -> f32 {
    let content_height = rows.max(1) as f32 * row_height;
    let available_height = available_height.max(row_height);
    content_height.min(available_height).max(row_height)
}

fn book_reorder_auto_scroll_delta(pointer_y: f32, viewport_top: f32, viewport_bottom: f32) -> f32 {
    let height = (viewport_bottom - viewport_top).max(0.0);
    if height <= 1.0 {
        return 0.0;
    }
    let edge = BOOK_REORDER_AUTO_SCROLL_EDGE_PX.min(height * 0.45).max(1.0);
    if pointer_y < viewport_top + edge {
        -((viewport_top + edge - pointer_y) / edge).clamp(0.0, 1.0)
            * BOOK_REORDER_AUTO_SCROLL_MAX_STEP_PX
    } else if pointer_y > viewport_bottom - edge {
        ((pointer_y - (viewport_bottom - edge)) / edge).clamp(0.0, 1.0)
            * BOOK_REORDER_AUTO_SCROLL_MAX_STEP_PX
    } else {
        0.0
    }
}

fn book_reorder_keyboard_scroll_offset(
    current_offset: f32,
    content_height: f32,
    viewport_height: f32,
    row_height: f32,
    key: BookReorderScrollKey,
) -> f32 {
    let max_offset = (content_height - viewport_height).max(0.0);
    let page_step = (viewport_height - row_height).max(row_height);
    match key {
        BookReorderScrollKey::PageUp => current_offset - page_step,
        BookReorderScrollKey::PageDown => current_offset + page_step,
        BookReorderScrollKey::Home => 0.0,
        BookReorderScrollKey::End => max_offset,
    }
    .clamp(0.0, max_offset)
}

fn book_reorder_drop_target_for_pos(
    rect: egui::Rect,
    item_index: usize,
    len: usize,
    cols: usize,
    gap: f32,
    pointer_pos: Option<egui::Pos2>,
) -> Option<(usize, f32)> {
    let pos = pointer_pos?;
    if !rect.contains(pos) {
        return None;
    }
    let insert_after = pos.x >= rect.center().x;
    let insert_index = if insert_after {
        item_index + 1
    } else {
        item_index
    };
    let indicator_x =
        book_reorder_insert_indicator_x(rect, item_index, insert_index, len, cols, gap);
    Some((insert_index.min(len), indicator_x))
}

fn book_reorder_insert_indicator_x(
    rect: egui::Rect,
    item_index: usize,
    insert_index: usize,
    len: usize,
    cols: usize,
    gap: f32,
) -> f32 {
    let cols = cols.max(1);
    if insert_index <= item_index {
        if item_index > 0 && item_index % cols != 0 {
            rect.left() - gap * 0.5
        } else {
            rect.left()
        }
    } else if insert_index < len && insert_index % cols != 0 {
        rect.right() + gap * 0.5
    } else {
        rect.right()
    }
}

fn book_reorder_end_indicator_x(rect: egui::Rect, len: usize, cols: usize, gap: f32) -> f32 {
    if len > 0 && len % cols.max(1) != 0 {
        rect.left() - gap
    } else {
        rect.left()
    }
}

fn draw_book_reorder_insert_indicator(ui: &egui::Ui, x: f32, rect: egui::Rect) {
    let y0 = rect.top() + 5.0;
    let y1 = rect.bottom() - 5.0;
    if y1 <= y0 {
        return;
    }
    let color = ui.visuals().selection.stroke.color;
    let stroke = egui::Stroke::new(3.0, color);
    let cap = 6.0;
    let painter = ui.painter();
    painter.line_segment([egui::pos2(x, y0), egui::pos2(x, y1)], stroke);
    painter.line_segment([egui::pos2(x - cap, y0), egui::pos2(x + cap, y0)], stroke);
    painter.line_segment([egui::pos2(x - cap, y1), egui::pos2(x + cap, y1)], stroke);
}

fn draw_details_text(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    text: &str,
    align: egui::Align2,
    color: egui::Color32,
    strong: bool,
) {
    if text.is_empty() || rect.width() <= 4.0 {
        return;
    }
    let clip = rect.shrink2(egui::vec2(6.0, 1.0));
    if clip.width() <= 1.0 {
        return;
    }
    let x = if matches!(
        align,
        egui::Align2::RIGHT_TOP | egui::Align2::RIGHT_CENTER | egui::Align2::RIGHT_BOTTOM
    ) {
        clip.right()
    } else {
        clip.left()
    };
    let font = if strong {
        egui::TextStyle::Button.resolve(ui.style())
    } else {
        egui::TextStyle::Body.resolve(ui.style())
    };
    ui.painter().with_clip_rect(clip).text(
        egui::pos2(x, clip.center().y),
        align,
        text,
        font,
        color,
    );
}

fn draw_details_preview_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    color: egui::Color32,
    muted: bool,
) {
    if rect.width() < 12.0 || rect.height() < 12.0 {
        return;
    }
    let alpha = if muted { 90 } else { color.a() };
    let stroke_color =
        egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha);
    let icon = egui::Rect::from_center_size(
        rect.center(),
        egui::vec2(rect.width().min(17.0), rect.height().min(15.0)),
    );
    let stroke = egui::Stroke::new(1.25, stroke_color);
    painter.rect_stroke(icon, 2.0, stroke, egui::StrokeKind::Inside);
    painter.circle_filled(
        egui::pos2(
            icon.left() + icon.width() * 0.28,
            icon.top() + icon.height() * 0.32,
        ),
        1.7,
        stroke_color,
    );
    let mountain = vec![
        egui::pos2(
            icon.left() + icon.width() * 0.16,
            icon.bottom() - icon.height() * 0.22,
        ),
        egui::pos2(
            icon.left() + icon.width() * 0.42,
            icon.top() + icon.height() * 0.55,
        ),
        egui::pos2(
            icon.left() + icon.width() * 0.57,
            icon.bottom() - icon.height() * 0.32,
        ),
        egui::pos2(
            icon.left() + icon.width() * 0.78,
            icon.top() + icon.height() * 0.44,
        ),
        egui::pos2(
            icon.right() - icon.width() * 0.12,
            icon.bottom() - icon.height() * 0.22,
        ),
    ];
    painter.add(egui::Shape::line(mountain, stroke));
}

fn details_kind_label(item: &GridItem) -> String {
    match item {
        GridItem::Folder(path) if crate::path_key::is_drive_or_share_root(path) => {
            "ドライブ".to_string()
        }
        GridItem::Folder(_) => "フォルダ".to_string(),
        GridItem::Image(path) => details_ext_kind(path, "画像"),
        GridItem::Video(path) => details_ext_kind(path, "動画"),
        GridItem::ZipFile(path) => details_ext_kind(path, "ZIP"),
        GridItem::PdfFile(path) => details_ext_kind(path, "PDF"),
        GridItem::ConvertibleArchive { format, .. } => format.label().to_string(),
        GridItem::ZipImage { .. } => "ZIP 内画像".to_string(),
        GridItem::ZipSeparator { .. } => "見出し".to_string(),
        GridItem::PdfPage { .. } => "PDF ページ".to_string(),
        GridItem::ZipDir {
            is_archive,
            dir_prefix,
            ..
        } => {
            if *is_archive {
                // セグメント拡張子から実フォーマット名 (展開キャッシュの rar/7z/lzh 含む)。
                let name = crate::grid_item::zipdir_display_name(dir_prefix);
                let ext = name.rsplit('.').next().unwrap_or("");
                let label = crate::archive_converter::ArchiveFormat::nested_from_extension(ext)
                    .map(|f| f.label())
                    .unwrap_or("ZIP");
                format!("内側 {label}")
            } else {
                "ZIP 内フォルダ".to_string()
            }
        }
        GridItem::SearchContainer { kind, .. } => match kind {
            crate::grid_item::SearchContainerKind::Folder => "検索フォルダ".to_string(),
            crate::grid_item::SearchContainerKind::Zip => "検索ZIP".to_string(),
        },
        GridItem::Stack { count, .. } => format!("スタック ({count})"),
    }
}

fn details_ext_kind(path: &Path, fallback: &str) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty())
        .map(|e| e.to_ascii_uppercase())
        .map(|ext| {
            if ext == fallback {
                fallback.to_string()
            } else {
                format!("{ext} {fallback}")
            }
        })
        .unwrap_or_else(|| fallback.to_string())
}

fn short_path_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .map(str::to_string)
}

fn zip_entry_parent_name(entry_name: &str) -> Option<&str> {
    let trimmed = entry_name.trim_matches('/');
    let (parent, _) = trimmed.rsplit_once('/')?;
    parent.rsplit('/').find(|segment| !segment.is_empty())
}

fn selection_info_parent_location_label(item: &GridItem) -> Option<String> {
    match item {
        GridItem::Folder(path)
        | GridItem::Image(path)
        | GridItem::Video(path)
        | GridItem::ZipFile(path)
        | GridItem::PdfFile(path)
        | GridItem::SearchContainer { path, .. }
        | GridItem::ConvertibleArchive { path, .. } => {
            short_path_name(path.parent()?).map(|name| format!("親フォルダ名 {name}"))
        }
        GridItem::ZipImage {
            zip_path,
            entry_name,
        } => {
            let mut label = short_path_name(zip_path)?;
            if let Some(parent) = zip_entry_parent_name(entry_name) {
                label.push_str(" > ");
                label.push_str(parent);
            }
            Some(format!("親フォルダ名 {label}"))
        }
        GridItem::ZipDir {
            zip_path,
            dir_prefix,
            ..
        } => {
            let mut label = short_path_name(zip_path)?;
            if let Some(parent) = zip_entry_parent_name(dir_prefix) {
                label.push_str(" > ");
                label.push_str(parent);
            }
            Some(format!("親フォルダ名 {label}"))
        }
        GridItem::PdfPage { pdf_path, .. } => {
            short_path_name(pdf_path).map(|name| format!("親フォルダ名 {name}"))
        }
        // ファイル名スタック: 代表画像の親フォルダ名を出す。
        GridItem::Stack { representative, .. } => {
            short_path_name(representative.parent()?).map(|name| format!("親フォルダ名 {name}"))
        }
        GridItem::ZipSeparator { .. } => None,
    }
}

#[cfg(windows)]
fn format_details_mtime(secs: i64, show_seconds: bool) -> String {
    if secs <= 0 {
        return String::new();
    }
    const WINDOWS_TICKS_PER_SEC: i128 = 10_000_000;
    const UNIX_TO_WINDOWS_SECS: i128 = 11_644_473_600;
    let ticks = (secs as i128 + UNIX_TO_WINDOWS_SECS) * WINDOWS_TICKS_PER_SEC;
    if ticks <= 0 || ticks > u64::MAX as i128 {
        return String::new();
    }

    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
    use windows::Win32::System::Time::FileTimeToSystemTime;

    let ticks = ticks as u64;
    let filetime = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut local_filetime = FILETIME::default();
    let mut st = SYSTEMTIME::default();
    if unsafe { FileTimeToLocalFileTime(&filetime, &mut local_filetime) }.is_err()
        || unsafe { FileTimeToSystemTime(&local_filetime, &mut st) }.is_err()
    {
        return String::new();
    }
    if show_seconds {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
        )
    } else {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute
        )
    }
}

#[cfg(not(windows))]
fn format_details_mtime(secs: i64, _show_seconds: bool) -> String {
    if secs <= 0 {
        String::new()
    } else {
        secs.to_string()
    }
}

fn reading_history_last_read_text(
    entry: &crate::reading_history_db::ReadingHistoryEntry,
    show_seconds: bool,
) -> Option<String> {
    let text = format_details_mtime(entry.last_read_at_ms / 1000, show_seconds);
    (!text.is_empty()).then_some(text)
}

fn reading_history_progress_text(
    entry: &crate::reading_history_db::ReadingHistoryEntry,
) -> Option<String> {
    let page = entry.last_page?;
    if page <= 0 {
        return None;
    }
    match entry.page_count {
        Some(count) if count > 0 => Some(format!("{page} / {count}")),
        _ => Some(format!("{page} ページ目")),
    }
}

impl App {
    // ── メニューバー ─────────────────────────────────────────────────

    /// メニューバーを描画し、ナビゲーション先とソート変更の有無を返す。
    pub(crate) fn render_menubar(&mut self, ctx: &egui::Context) -> (Option<PathBuf>, bool) {
        let mut fav_nav: Option<PathBuf> = None;
        let mut settings_changed = false;
        let mut sort_changed = false;
        let book_sort_locked =
            self.current_folder_is_book_folder() || self.items_are_reading_history_view;
        let rating_counts = self.rating_counts();
        let selected_video_path =
            self.selected
                .and_then(|idx| self.items.get(idx))
                .and_then(|item| match item {
                    GridItem::Video(path) => Some(path.clone()),
                    _ => None,
                });
        let resolved_menu_layout = resolve_menu_layout(&self.settings.menu_layout);
        let open_folder_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::FileOpenFolder);
        let reading_history_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::FileReadingHistory);
        let local_search_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::FileLocalSearch);
        let open_capture_folder_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::FileOpenCaptureFolder);
        let open_recycle_bin_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::FileOpenRecycleBin);
        let quit_menu_label = self.keymap.menu_command_label(MenuCommandId::FileQuit);
        let favorite_add_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::FavoritesAddCurrentFolder);
        let favorite_edit_menu_label = self.keymap.menu_command_label(MenuCommandId::FavoritesEdit);
        let fav_search_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::FavoritesFavSearch);
        let metadata_search_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::FavoritesMetadataSearch);
        let book_add_selection_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::BooksAddSelectionToActiveBook);
        let book_add_clipboard_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::BooksAddClipboardImage);
        let book_open_root_menu_label =
            self.keymap.menu_command_label(MenuCommandId::BooksOpenRoot);
        let book_open_active_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::BooksOpenActiveBook);
        let book_reorder_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::BooksReorderCurrentBook);
        let book_manage_menu_label = self.keymap.menu_command_label(MenuCommandId::BooksManage);
        let video_register_upscale_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::VideoRegisterUpscale);
        let video_delete_upscale_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::VideoDeleteUpscale);
        let video_show_upscale_tasks_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::VideoShowUpscaleTasks);
        let tag_manage_pinned_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::TagsManagePinned);
        let tag_view_menu_label = self.keymap.menu_command_label(MenuCommandId::TagsTagView);
        let settings_thumbnail_cache_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::SettingsThumbnailCache);
        let settings_archive_cache_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::SettingsArchiveCache);
        let settings_thumbnail_quality_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::SettingsThumbnailQuality);
        let settings_stats_menu_label =
            self.keymap.menu_command_label(MenuCommandId::SettingsStats);
        let settings_reset_rotation_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::SettingsResetRotation);
        let settings_restore_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::SettingsRestoreSettings);
        let settings_operation_customize_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::SettingsOperationCustomize);
        let settings_preferences_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::SettingsPreferences);
        let help_open_manual_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::HelpOpenManual);
        let help_open_logs_menu_label = self.keymap.menu_command_label(MenuCommandId::HelpOpenLogs);
        let help_show_whats_new_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::HelpShowWhatsNew);
        let help_about_menu_label = self.keymap.menu_command_label(MenuCommandId::HelpAbout);

        egui::TopBottomPanel::top("menubar").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                let mut top_menu_responses = Vec::with_capacity(7);

                for resolved_top_menu in &resolved_menu_layout.menus {
                    match resolved_top_menu.id {
                        TopMenuId::File => {
                            let file_menu_commands = &resolved_top_menu.commands;
                            let response = ui.menu_button(TopMenuId::File.label(), |ui| {
                                let mut rating_menu_drawn = false;
                                for &command in file_menu_commands {
                                    match command {
                                        MenuCommandId::FileOpenFolder => {
                                            if ui.button(&open_folder_menu_label).clicked() {
                                                // 既に現在フォルダが設定されていれば初期値として補完
                                                self.open_folder_input = self
                                                    .current_folder
                                                    .as_ref()
                                                    .map(|p| p.to_string_lossy().to_string())
                                                    .unwrap_or_default();
                                                self.show_open_folder_dialog = true;
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::FileReadingHistory => {
                                            if ui.button(&reading_history_menu_label).clicked() {
                                                self.enter_reading_history();
                                                ui.close();
                                            }
                                            ui.menu_button("レーティング一覧", |ui| {
                                                for stars in 1..=5 {
                                                    if ui
                                                        .button(rating_view_menu_label(
                                                            stars,
                                                            rating_counts,
                                                        ))
                                                        .clicked()
                                                    {
                                                        self.enter_rating_view(stars);
                                                        ui.close();
                                                    }
                                                }
                                            });
                                            rating_menu_drawn = true;
                                        }
                                        MenuCommandId::FileLocalSearch => {
                                            if ui.button(&local_search_menu_label).clicked() {
                                                // 相互排他は open_local_metadata_search 内で (Ctrl+S/Ctrl+G を閉じる)
                                                self.open_local_metadata_search();
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::FileOpenCaptureFolder => {
                                            if ui.button(&open_capture_folder_menu_label).clicked()
                                            {
                                                self.open_capture_output_dir();
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::FileOpenRecycleBin => {
                                            if ui.button(&open_recycle_bin_menu_label).clicked() {
                                                crate::ui_helpers::open_recycle_bin_async();
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::FileQuit => {
                                            ui.separator();
                                            if ui.button(&quit_menu_label).clicked() {
                                                // トレイ常駐設定 ON のときでも [×] ではなく明示終了なので、
                                                // `shutdown_requested` を立てて `maybe_intercept_close` を通す。
                                                self.shutdown_requested
                                                    .store(true, std::sync::atomic::Ordering::SeqCst);
                                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                if !rating_menu_drawn {
                                    ui.menu_button("レーティング一覧", |ui| {
                                        for stars in 1..=5 {
                                            if ui
                                                .button(rating_view_menu_label(stars, rating_counts))
                                                .clicked()
                                            {
                                                self.enter_rating_view(stars);
                                                ui.close();
                                            }
                                        }
                                    });
                                }
                            });
                            top_menu_responses.push(response.response);
                        }
                        TopMenuId::Favorites => {
                            let favorites_menu_commands = &resolved_top_menu.commands;
                            let response = ui.menu_button(TopMenuId::Favorites.label(), |ui| {
                                // このフォルダを追加 (クリック時は名称入力ダイアログを開く)。
                                // お気に入りは索引ルートになるため、ZIP/PDF/変換キャッシュではなく
                                // 実ディレクトリだけを対象にする。
                                let favorite_target = self.current_favorite_target();
                                let can_add = favorite_target.is_some();
                                for &command in favorites_menu_commands {
                                    match command {
                                        MenuCommandId::FavoritesAddCurrentFolder => {
                                            if ui
                                                .add_enabled(
                                                    can_add,
                                                    egui::Button::new(&favorite_add_menu_label),
                                                )
                                                .hover_tip_disabled(
                                                    "お気に入りに追加できるのは実フォルダのみです",
                                                )
                                                .clicked()
                                            {
                                                if let Some(folder) = favorite_target.clone() {
                                                    // 既定の名前はフォルダ名から補完
                                                    let default_name = folder
                                                        .file_name()
                                                        .and_then(|n| n.to_str())
                                                        .unwrap_or("")
                                                        .to_string();
                                                    self.fav_add_name_input = default_name;
                                                    self.fav_add_target = Some(folder);
                                                    self.show_fav_add_dialog = true;
                                                }
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::FavoritesEdit => {
                                            if ui.button(&favorite_edit_menu_label).clicked() {
                                                self.show_favorites_editor = true;
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::FavoritesFavSearch => {
                                            if ui.button(&fav_search_menu_label).clicked() {
                                                self.open_favsearch();
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::FavoritesMetadataSearch => {
                                            if ui.button(&metadata_search_menu_label).clicked() {
                                                // 相互排他は toggle_global_search 内で
                                                self.toggle_global_search();
                                                ui.close();
                                            }
                                        }
                                        _ => {}
                                    }
                                }

                                // 区切り線
                                ui.separator();

                                // 登録済みお気に入り一覧
                                if self.settings.favorites.is_empty() {
                                    ui.label(egui::RichText::new("（未登録）").weak());
                                } else {
                                    let favorites = self.settings.favorites.clone();
                                    for fav in &favorites {
                                        if ui.button(&fav.name).clicked() {
                                            fav_nav = Some(fav.path.clone());
                                            ui.close();
                                        }
                                    }
                                }
                            });
                            top_menu_responses.push(response.response);
                        }
                        TopMenuId::Books => {
                            let books_menu_commands = &resolved_top_menu.commands;
                            let response = ui.menu_button(TopMenuId::Books.label(), |ui| {
                                let active_name = self.active_book_name();
                                ui.label(format!("追加先の本: {active_name}"));
                                let has_selection = self.selected.is_some() || !self.checked.is_empty();
                                for &command in books_menu_commands {
                                    match command {
                                        MenuCommandId::BooksAddSelectionToActiveBook => {
                                            let add_resp = ui
                                                .add_enabled(
                                                    has_selection,
                                                    egui::Button::new(&book_add_selection_menu_label),
                                                )
                                                .hover_tip(if has_selection {
                                                    "選択中またはチェック済みの画像・ページを追加先の本へ追加"
                                                } else {
                                                    "追加する画像・ページを選択してください"
                                                });
                                            if add_resp.clicked() {
                                                self.add_grid_selection_to_active_book(ctx);
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::BooksAddClipboardImage => {
                                            if ui.button(&book_add_clipboard_menu_label).clicked() {
                                                self.add_clipboard_image_to_active_book(ctx);
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::BooksOpenRoot => {
                                            if ui.button(&book_open_root_menu_label).clicked() {
                                                self.open_books_root();
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::BooksOpenActiveBook => {
                                            if ui.button(&book_open_active_menu_label).clicked() {
                                                fav_nav = Some(self.active_book_folder_path());
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::BooksReorderCurrentBook => {
                                            let can_reorder_book = self.current_folder_is_book_folder();
                                            let reorder_resp = ui
                                                .add_enabled(
                                                    can_reorder_book,
                                                    egui::Button::new(&book_reorder_menu_label),
                                                )
                                                .hover_tip(if can_reorder_book {
                                                    "現在開いている本のページ順を変更"
                                                } else {
                                                    "本を開くと使用できます"
                                                });
                                            if reorder_resp.clicked() {
                                                self.open_book_reorder_from_current();
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::BooksManage => {
                                            ui.separator();
                                            if ui.button(&book_manage_menu_label).clicked() {
                                                self.show_book_manager = true;
                                                self.book_manager_rename_name = active_name.clone();
                                                self.book_list_cache = None;
                                                ui.close();
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                ui.separator();
                                if self.book_list_cache.is_none() && self.book_op_pending.is_none() {
                                    self.request_book_list_refresh();
                                }
                                let rows = self.book_list_cache.clone();
                                match rows {
                                    Some(rows) if rows.is_empty() => {
                                        ui.label(egui::RichText::new("（本はまだありません）").weak());
                                    }
                                    Some(rows) => {
                                        ui.menu_button("追加先の本を選ぶ", |ui| {
                                            for row in rows {
                                                let selected = row.name == active_name;
                                                let label = if selected {
                                                    format!("✓ {} ({}p)", row.name, row.page_count)
                                                } else {
                                                    format!("{} ({}p)", row.name, row.page_count)
                                                };
                                                if ui.button(label).clicked() {
                                                    self.settings.active_book_name = row.name.clone();
                                                    self.settings.save();
                                                    self.book_manager_rename_name = row.name;
                                                    ui.close();
                                                }
                                            }
                                        });
                                    }
                                    None => {
                                        ui.label(egui::RichText::new("本棚を読み込み中…").weak());
                                    }
                                }
                            });
                            top_menu_responses.push(response.response);
                        }
                        TopMenuId::Video => {
                            let video_menu_commands = &resolved_top_menu.commands;
                            let response = ui.menu_button(TopMenuId::Video.label(), |ui| {
                                let can_apply_to_selected = selected_video_path.is_some();
                                for &command in video_menu_commands {
                                    match command {
                                        MenuCommandId::VideoRegisterUpscale => {
                                            if ui
                                                .add_enabled(
                                                    can_apply_to_selected,
                                                    egui::Button::new(&video_register_upscale_menu_label),
                                                )
                                                .clicked()
                                            {
                                                if let Some(path) = selected_video_path.clone() {
                                                    self.request_video_upscale(path);
                                                }
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::VideoDeleteUpscale => {
                                            if ui
                                                .add_enabled(
                                                    can_apply_to_selected,
                                                    egui::Button::new(&video_delete_upscale_menu_label),
                                                )
                                                .clicked()
                                            {
                                                if let Some(path) = selected_video_path.clone() {
                                                    self.request_video_upscale_artifact_delete(path);
                                                }
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::VideoShowUpscaleTasks => {
                                            ui.separator();
                                            if ui.button(&video_show_upscale_tasks_menu_label).clicked()
                                            {
                                                self.show_video_upscale_tasks = true;
                                                ui.close();
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            });
                            top_menu_responses.push(response.response);
                        }
                        TopMenuId::Tags => {
                            let tags_menu_commands = &resolved_top_menu.commands;
                            let response =
                                ui.menu_button(TopMenuId::Tags.label(), |ui| {
                                    for &command in tags_menu_commands {
                                        match command {
                                            MenuCommandId::TagsManagePinned => {
                                                if ui.button(&tag_manage_pinned_menu_label).clicked()
                                                {
                                                    self.open_tag_editor();
                                                    ui.close();
                                                }
                                            }
                                            MenuCommandId::TagsTagView => {
                                                if ui.button(&tag_view_menu_label).clicked() {
                                                    self.open_tag_view();
                                                    ui.close();
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    ui.separator();
                                    let selection_count = self.tag_target_path_count();
                                    let has_target = selection_count > 0;
                                    if ui
                                        .add_enabled(
                                            has_target,
                                            egui::Button::new(format!(
                                                "タグを付ける/外す… ({selection_count})"
                                            )),
                                        )
                                        .clicked()
                                    {
                                        self.open_tag_apply_dialog();
                                        ui.close();
                                    }
                                    if ui
                                        .add_enabled(
                                            has_target,
                                            egui::Button::new(format!(
                                                "選択中の項目からタグをクリア ({selection_count})"
                                            )),
                                        )
                                        .hover_tip("mIV 内のタグだけを削除します。")
                                        .clicked()
                                    {
                                        self.request_tag_clear_for_selection();
                                        ui.close();
                                    }
                                    let legacy_xmp_count = self.legacy_xmp_target_path_count();
                                    let has_legacy_xmp_target = legacy_xmp_count > 0;
                                    ui.separator();
                                    if ui
                                .add_enabled(
                                    has_legacy_xmp_target,
                                    egui::Button::new(format!("旧XMPタグを取り込む ({legacy_xmp_count})")),
                                )
                                .hover_tip(
                                    "ファイル内に残っている旧mIVの #タグをアプリ内タグへ取り込みます。",
                                )
                                .clicked()
                            {
                                self.request_legacy_xmp_import_for_selection(
                                    crate::tag_legacy_xmp_worker::LegacyXmpImportMode::ImportOnly,
                                );
                                ui.close();
                            }
                                    if ui
                                        .add_enabled(
                                            has_legacy_xmp_target,
                                            egui::Button::new(format!(
                                                "旧XMPタグを取り込んでファイルから削除 ({legacy_xmp_count})"
                                            )),
                                        )
                                        .hover_tip("取り込み後、ファイル内の旧mIV #タグだけを削除します。")
                                        .clicked()
                                    {
                                        self.request_legacy_xmp_import_for_selection(
                                    crate::tag_legacy_xmp_worker::LegacyXmpImportMode::ImportAndRemove,
                                );
                                        ui.close();
                                    }
                                    ui.separator();
                                    let tags_snapshot: Vec<_> = self
                                        .settings
                                        .tags
                                        .iter()
                                        .filter(|tag| tag.show_shortcut)
                                        .cloned()
                                        .collect();
                                    if tags_snapshot.is_empty() {
                                        ui.label(egui::RichText::new("（ピン留めタグなし）").weak());
                                    } else {
                                        ui.menu_button("タグビューで探す", |ui| {
                                            for tag in &tags_snapshot {
                                                if ui.button(format!("#{}", tag.name)).clicked() {
                                                    self.open_tag_view_for_tag(&tag.name);
                                                    ui.close();
                                                }
                                            }
                                        });
                                        ui.separator();
                                        for tag in &tags_snapshot {
                                            let label = format!("#{}", tag.name);
                                            let btn = egui::Button::new(label);
                                            let resp = ui.add_enabled(has_target, btn);
                                            let clicked = resp.clicked();
                                            resp.context_menu(|ui| {
                                                if ui.button("このタグで探す").clicked() {
                                                    self.open_tag_view_for_tag(&tag.name);
                                                    ui.close();
                                                }
                                            });
                                            if clicked {
                                                self.request_tag_toggle_for_selection(&tag.name);
                                                ui.close();
                                            }
                                        }
                                    }
                                });
                            top_menu_responses.push(response.response);
                        }
                        TopMenuId::Settings => {
                            let settings_menu_commands = &resolved_top_menu.commands;
                            let response = ui.menu_button(TopMenuId::Settings.label(), |ui| {
                                ui.menu_button("サムネイル列数", |ui| {
                                    for cols in
                                        crate::settings::MIN_GRID_COLS..=crate::settings::MAX_GRID_COLS
                                    {
                                        let checked = self.settings.grid_cols == cols;
                                        let prefix = if checked { "✓ " } else { "  " };
                                        if ui.button(format!("{prefix}{cols} 列")).clicked() {
                                            self.settings.grid_cols = cols;
                                            settings_changed = true;
                                            ui.close();
                                        }
                                    }
                                });
                                ui.menu_button("サムネイル比率", |ui| {
                                    // 「自動」項目を先頭に表示。auto 中はチェック、再選択で再評価。
                                    let auto_checked = self.settings.thumb_aspect_auto;
                                    let auto_label = if let Some(current) = self.auto_aspect.current {
                                        format!("自動 ({})", current.label())
                                    } else {
                                        "自動".to_string()
                                    };
                                    let auto_prefix = if auto_checked { "✓ " } else { "  " };
                                    if ui.button(format!("{auto_prefix}{auto_label}")).clicked() {
                                        // 「自動」を選択。現在 Manual だったら自動に切替、すでに Auto なら再評価
                                        // のためリセット (samples は活かして即決し直す)。
                                        let was_off = !self.settings.thumb_aspect_auto;
                                        let prev_effective = self.effective_thumb_aspect();
                                        self.settings.thumb_aspect_auto = true;
                                        self.auto_aspect.reset_decision_only();
                                        if was_off {
                                            self.rebuild_auto_aspect_samples_from_loaded();
                                        }
                                        self.maybe_apply_auto_aspect(true);
                                        // Hold で current=None のまま終わったとき effective は Square。
                                        // 描画上のセル比率が変わるのでスクロール位置を補正する
                                        // (Switch されたら maybe_apply 内で fixup 済 → 二重呼出しは
                                        // 冪等で no-op、Codex P3 2026-05)。
                                        let new_effective = self.effective_thumb_aspect();
                                        if prev_effective != new_effective {
                                            self.fixup_scroll_for_aspect_change(new_effective);
                                        }
                                        settings_changed = true;
                                        ui.close();
                                    }
                                    ui.separator();
                                    for &aspect in crate::settings::ThumbAspect::all() {
                                        // 手動値表示: auto モード時はチェックしない (auto がチェックされる)
                                        let checked = !self.settings.thumb_aspect_auto
                                            && self.settings.thumb_aspect == aspect;
                                        let prefix = if checked { "✓ " } else { "  " };
                                        if ui.button(format!("{prefix}{}", aspect.label())).clicked() {
                                            // 個別比率クリック → Manual に切替。scroll 補正も適用。
                                            if self.settings.thumb_aspect_auto
                                                || self.settings.thumb_aspect != aspect
                                            {
                                                self.fixup_scroll_for_aspect_change(aspect);
                                            }
                                            self.settings.thumb_aspect_auto = false;
                                            self.settings.thumb_aspect = aspect;
                                            settings_changed = true;
                                            ui.close();
                                        }
                                    }
                                });
                                if book_sort_locked {
                                    ui.add_enabled(false, egui::Button::new("ソート順: 番号順固定"))
                                        .on_hover_text("本棚内または読書履歴では表示順が固定されます。");
                                } else {
                                    ui.menu_button("ソート順", |ui| {
                                        for &order in crate::settings::SortOrder::all() {
                                            let checked = if self.items_are_rating_view {
                                                self.rating_view_sort
                                                    == crate::rating_view::RatingViewSort::Normal(order)
                                            } else {
                                                self.settings.sort_order == order
                                            };
                                            let prefix = if checked { "✓ " } else { "  " };
                                            let resp = ui
                                                .button(format!("{prefix}{}", order.label()))
                                                .on_hover_text(order.description());
                                            if resp.clicked() {
                                                self.settings.sort_order = order;
                                                if self.items_are_rating_view {
                                                    self.set_rating_view_sort(
                                                        crate::rating_view::RatingViewSort::Normal(order),
                                                    );
                                                } else {
                                                    sort_changed = true;
                                                }
                                                ui.close();
                                            }
                                        }
                                        if self.items_are_rating_view {
                                            ui.separator();
                                            for sort in [
                                                crate::rating_view::RatingViewSort::RatedAtDesc,
                                                crate::rating_view::RatingViewSort::RatedAtAsc,
                                            ] {
                                                let checked = self.rating_view_sort == sort;
                                                let prefix = if checked { "✓ " } else { "  " };
                                                if ui.button(format!("{prefix}{}", sort.label())).clicked()
                                                {
                                                    self.set_rating_view_sort(sort);
                                                    ui.close();
                                                }
                                            }
                                        }
                                    });
                                }
                                ui.separator();
                                let mut toolbar_menu_drawn = false;
                                for &command in settings_menu_commands {
                                    match command {
                                        MenuCommandId::SettingsThumbnailCache => {
                                            if ui
                                                .button(&settings_thumbnail_cache_menu_label)
                                                .clicked()
                                            {
                                                let cache_dir = crate::catalog::default_cache_dir();
                                                // cache_stats は数千フォルダで秒級になるのでワーカーに回す。
                                                // ダイアログは「取得中...」表示で開き、poll 完了時に stats が埋まる。
                                                self.cache_manager_stats = None;
                                                self.cache_manager_tile_bytes = None;
                                                self.cache_manager_auto_aspect_entries = None;
                                                self.cache_manager_result = None;
                                                if self.cache_maint_pending.is_none() {
                                                    self.cache_maint_pending = Some(
                                                        crate::cache_maintenance::spawn(
                                                            crate::cache_maintenance::CacheMaintTask::Stats,
                                                            cache_dir,
                                                            self.video_tile_cache.clone(),
                                                        ),
                                                    );
                                                }
                                                self.show_cache_manager = true;
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::SettingsArchiveCache => {
                                            if ui.button(&settings_archive_cache_menu_label).clicked()
                                            {
                                                self.open_archive_cache_manager();
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::SettingsThumbnailQuality => {
                                            if ui
                                                .button(&settings_thumbnail_quality_menu_label)
                                                .clicked()
                                            {
                                                self.open_thumb_quality_dialog(ctx);
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::SettingsStats => {
                                            if ui.button(&settings_stats_menu_label).clicked() {
                                                self.show_stats_dialog = true;
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::SettingsResetRotation => {
                                            ui.separator();
                                            if ui.button(&settings_reset_rotation_menu_label).clicked()
                                            {
                                                self.show_rotation_reset_confirm = true;
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::SettingsRestoreSettings => {
                                            ui.separator();
                                            if ui.button(&settings_restore_menu_label).clicked() {
                                                // 2026-05-17: settings.db のバックアップから復元する UI。
                                                // 起動時の自動 boot recovery で救えなかった場合、ユーザーが
                                                // 過去 10 世代を選んで巻き戻せるようにする (= 完全リセットも可)。
                                                self.open_settings_restore_dialog();
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::SettingsOperationCustomize => {
                                            ui.separator();
                                            if ui
                                                .button(&settings_operation_customize_menu_label)
                                                .clicked()
                                            {
                                                self.show_operation_customize = true;
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::SettingsPreferences => {
                                            if !toolbar_menu_drawn {
                                                ui.separator();
                                                // ツールバーのカスタマイズは原則ツールバーの右クリックで行うが、全セクションを
                                                // 隠すとツールバー自体が消えて右クリックの入口が無くなる (Codex P2)。
                                                // この常設メニューを最後の砦にして、いつでも再表示・既定化できるようにする。
                                                // 「既定に戻す」は影響が大きいのでここ (設定メニュー) にだけ出す (show_reset=true)。
                                                ui.menu_button("ツールバー", |ui| {
                                                    self.draw_toolbar_visibility_menu(ui, true);
                                                });
                                                toolbar_menu_drawn = true;
                                            }
                                            if ui.button(&settings_preferences_menu_label).clicked()
                                            {
                                                self.show_preferences = true;
                                                ui.close();
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                if !toolbar_menu_drawn {
                                    ui.separator();
                                    // ツールバーのカスタマイズは原則ツールバーの右クリックで行うが、全セクションを
                                    // 隠すとツールバー自体が消えて右クリックの入口が無くなる (Codex P2)。
                                    // この常設メニューを最後の砦にして、いつでも再表示・既定化できるようにする。
                                    // 「既定に戻す」は影響が大きいのでここ (設定メニュー) にだけ出す (show_reset=true)。
                                    ui.menu_button("ツールバー", |ui| {
                                        self.draw_toolbar_visibility_menu(ui, true);
                                    });
                                }
                                // VST3 関連の設定は環境設定→VST3 プラグインページに集約。
                                // 専用メニューは重複なので持たない (= ユーザー要望 2026-04)。
                                // 動画再生中はホバーバー / ツールバーの VST ボタンから
                                // プレイバックパネルを開く運用。
                            });
                            top_menu_responses.push(response.response);
                        }
                        TopMenuId::Help => {
                            let help_menu_commands = &resolved_top_menu.commands;
                            let response = ui.menu_button(TopMenuId::Help.label(), |ui| {
                                let mut update_check_drawn = false;
                                for &command in help_menu_commands {
                                    match command {
                                        MenuCommandId::HelpOpenManual => {
                                            if ui.button(&help_open_manual_menu_label).clicked() {
                                                let url =
                                                    crate::ui_helpers::manual_url("index.html", None);
                                                crate::ui_helpers::open_url(&url);
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::HelpOpenLogs => {
                                            ui.separator();
                                            if ui.button(&help_open_logs_menu_label).clicked() {
                                                let dir = crate::data_dir::logs_dir();
                                                let _ = std::fs::create_dir_all(&dir);
                                                crate::ui_helpers::open_external_player(&dir);
                                                ui.close();
                                            }
                                            ui.separator();
                                            let checking = self.update_check_pending.is_some();
                                            if ui
                                                .add_enabled(
                                                    !checking,
                                                    egui::Button::new(if checking {
                                                        "更新を確認中…"
                                                    } else {
                                                        "更新を確認…"
                                                    }),
                                                )
                                                .clicked()
                                            {
                                                self.kick_update_check(true);
                                                ui.close();
                                            }
                                            update_check_drawn = true;
                                        }
                                        MenuCommandId::HelpShowWhatsNew => {
                                            if ui
                                                .button(&help_show_whats_new_menu_label)
                                                .on_hover_text(
                                                    "このバージョンの主な変更点をもう一度表示します",
                                                )
                                                .clicked()
                                            {
                                                // 現行版のエントリ。無ければ現行版以下の最新エントリに
                                                // フォールバック (次版エントリを先に埋め込んでも未来の
                                                // 変更点を表示しない)。
                                                let mut entries =
                                                    crate::version_highlights::for_version(
                                                        env!("CARGO_PKG_VERSION"),
                                                        crate::version_highlights::table(),
                                                    );
                                                if entries.is_empty() {
                                                    if let Some(latest) =
                                                        crate::version_highlights::latest_not_newer_than(
                                                            env!("CARGO_PKG_VERSION"),
                                                            crate::version_highlights::table(),
                                                        )
                                                    {
                                                        entries = vec![latest];
                                                    }
                                                }
                                                // 空 (= テーブル自体が空) のときは「見えないダイアログ開状態」で
                                                // ショートカットを塞がないよう、開かない (Codex P3)。
                                                if !entries.is_empty() {
                                                    self.whats_new_entries = entries;
                                                    self.show_whats_new = true;
                                                }
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::HelpAbout => {
                                            if ui.button(&help_about_menu_label).clicked() {
                                                self.show_about_dialog = true;
                                                ui.close();
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                if !update_check_drawn {
                                    ui.separator();
                                    let checking = self.update_check_pending.is_some();
                                    if ui
                                        .add_enabled(
                                            !checking,
                                            egui::Button::new(if checking {
                                                "更新を確認中…"
                                            } else {
                                                "更新を確認…"
                                            }),
                                        )
                                        .clicked()
                                    {
                                        self.kick_update_check(true);
                                        ui.close();
                                    }
                                }
                            });
                            top_menu_responses.push(response.response);
                        }
                    }
                }
                if self.should_show_update_badge() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let tag = self
                            .update_info
                            .as_ref()
                            .map(|i| i.latest_tag.clone())
                            .unwrap_or_default();
                        let resp = ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(format!("🔔 新バージョン {tag}"))
                                        .color(egui::Color32::from_rgb(100, 170, 100))
                                        .size(12.0),
                                )
                                .fill(egui::Color32::from_rgb(40, 70, 40)),
                            )
                            .hover_tip(
                                "新しいバージョンがリリースされています。\n\
                                     クリックで詳細を表示します。",
                            );
                        if resp.clicked() {
                            self.show_update_dialog = true;
                        }
                    });
                }

                switch_menubar_popup_on_hover(ui.ctx(), &top_menu_responses);
            });
        });

        if settings_changed {
            self.settings.save();
        }
        if sort_changed {
            self.settings.save();
            // ネスト ZIP は階層維持で再ソート、Ctrl+G は検索結果再ソート、通常は再ロード。
            self.apply_sort_change_reload();
        }

        (fav_nav, sort_changed)
    }

    pub(crate) fn open_book_reorder_from_current(&mut self) {
        let Some(folder) = self.current_folder.clone() else {
            self.show_feedback_toast("本フォルダを開いてから並べ替えてください".to_string());
            return;
        };
        if !crate::books::is_direct_book_folder(&self.book_root_path(), &folder) {
            self.show_feedback_toast("本フォルダを開いてから並べ替えてください".to_string());
            return;
        }
        let entries: Vec<crate::books::BookPageEntry> = self
            .items
            .iter()
            .filter_map(|item| match item {
                GridItem::Image(path)
                    if path
                        .parent()
                        .is_some_and(|parent| crate::folder_tree::path_eq(parent, &folder)) =>
                {
                    let display_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("page")
                        .to_string();
                    Some(crate::books::BookPageEntry {
                        path: path.clone(),
                        display_name,
                    })
                }
                _ => None,
            })
            .collect();
        if entries.is_empty() {
            self.show_feedback_toast("並べ替えできるページがありません".to_string());
            return;
        }
        let mut selected_keys = HashSet::new();
        if let Some(first) = entries.first() {
            selected_keys.insert(book_reorder_entry_key(first));
        }
        self.book_reorder = Some(crate::app::BookReorderState {
            folder,
            entries,
            selected: Some(0),
            selected_keys,
            selection_anchor: Some(0),
            dragging: None,
            drag_auto_scroll_enabled: false,
            scroll_offset_y: 0.0,
            thumb_textures: HashMap::new(),
            thumb_failed: HashSet::new(),
            thumb_pending_keys: HashSet::new(),
            thumb_upload_backlog: VecDeque::new(),
            thumb_tx: None,
            thumb_rx: None,
            dirty: false,
            drag_insert_index: None,
            thumb_tile_px: BOOK_REORDER_DEFAULT_TILE_PX,
            flush_pending: None,
            transfer_target_book: String::new(),
            transfer_pending: None,
            error: None,
        });
        self.book_list_cache = None;
        self.request_book_list_refresh();
    }

    pub(crate) fn draw_book_manager(&mut self, ctx: &egui::Context) {
        if !self.show_book_manager {
            return;
        }
        if self.book_list_cache.is_none() && self.book_op_pending.is_none() {
            self.request_book_list_refresh();
        }
        let mut open = true;
        let mut open_request: Option<PathBuf> = None;
        let mut set_active_request: Option<String> = None;
        let mut rename_request: Option<(String, String)> = None;
        let mut pin_toggle_request: Option<String> = None;
        let mut delete_request: Option<String> = None;
        let mut refresh_request = false;
        let default_pos = ctx.content_rect().center() - egui::vec2(250.0, 220.0);
        egui::Window::new("製本の管理")
            .collapsible(false)
            .resizable(true)
            .default_pos(default_pos)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(660.0);
                ui.label(format!("本棚: {}", self.book_root_path().display()));
                ui.horizontal(|ui| {
                    ui.label("現在の追加先の本");
                    ui.strong(self.active_book_name());
                    if ui.button("開く").clicked() {
                        open_request = Some(self.active_book_folder_path());
                    }
                    if ui.button("本棚フォルダ").clicked() {
                        open_request = Some(self.book_root_path());
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("新しい本");
                    ui.text_edit_singleline(&mut self.book_manager_new_name);
                    let name = crate::books::normalize_book_name(&self.book_manager_new_name);
                    let can_create = !name.trim().is_empty() && self.book_op_pending.is_none();
                    if ui
                        .add_enabled(can_create, egui::Button::new("作成"))
                        .clicked()
                    {
                        self.start_book_create(ctx, name);
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("本一覧");
                    if ui
                        .add_enabled(self.book_op_pending.is_none(), egui::Button::new("更新"))
                        .clicked()
                    {
                        refresh_request = true;
                    }
                    if self.book_op_pending.is_some() {
                        ui.label(egui::RichText::new("処理中…").weak());
                    }
                });
                match self.book_list_cache.clone() {
                    Some(rows) if rows.is_empty() => {
                        ui.label(egui::RichText::new("本はまだありません。").weak());
                    }
                    Some(rows) => {
                        let active_name = self.active_book_name();
                        let row_names: std::collections::BTreeSet<String> =
                            rows.iter().map(|row| row.name.clone()).collect();
                        self.book_manager_rename_inputs
                            .retain(|name, _| row_names.contains(name));
                        egui::ScrollArea::vertical()
                            .max_height(360.0)
                            .show(ui, |ui| {
                                for row in rows {
                                    let active = row.name == active_name;
                                    ui.horizontal(|ui| {
                                        if active {
                                            ui.strong("●");
                                        } else {
                                            ui.label(" ");
                                        }
                                        ui.label(format!("{}p", row.page_count));
                                        let input = self
                                            .book_manager_rename_inputs
                                            .entry(row.name.clone())
                                            .or_insert_with(|| row.name.clone());
                                        ui.add(
                                            egui::TextEdit::singleline(input).desired_width(220.0),
                                        );
                                        let new_name = crate::books::normalize_book_name(input);
                                        let can_rename = !new_name.is_empty()
                                            && new_name != row.name
                                            && self.book_op_pending.is_none();
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui.button("開く").clicked() {
                                                    open_request = Some(row.path.clone());
                                                }
                                                if ui
                                                    .add_enabled(
                                                        self.book_op_pending.is_none(),
                                                        egui::Button::new("削除"),
                                                    )
                                                    .clicked()
                                                {
                                                    self.book_manager_delete_confirm =
                                                        Some(row.name.clone());
                                                }
                                                if ui
                                                    .add_enabled(
                                                        !active && self.book_op_pending.is_none(),
                                                        egui::Button::new("追加先の本にする"),
                                                    )
                                                    .clicked()
                                                {
                                                    set_active_request = Some(row.name.clone());
                                                }
                                                if ui
                                                    .add_enabled(
                                                        can_rename,
                                                        egui::Button::new("名前変更"),
                                                    )
                                                    .clicked()
                                                {
                                                    rename_request =
                                                        Some((row.name.clone(), new_name.clone()));
                                                }
                                                let is_pinned = self
                                                    .settings
                                                    .pinned_books
                                                    .iter()
                                                    .any(|b| b == &row.name);
                                                if ui
                                                    .selectable_label(is_pinned, "固定")
                                                    .on_hover_text(
                                                        "ツールバーの本棚にこの本のボタンを固定表示する",
                                                    )
                                                    .clicked()
                                                {
                                                    pin_toggle_request = Some(row.name.clone());
                                                }
                                            },
                                        );
                                    });
                                }
                            });
                    }
                    None => {
                        ui.label(egui::RichText::new("読み込み中…").weak());
                    }
                }
            });
        if let Some(name) = self.book_manager_delete_confirm.clone() {
            let mut close_confirm = false;
            let mut confirmed = false;
            let response = egui::Modal::new(egui::Id::new("book_manager_delete_confirm_modal"))
                .show(ctx, |ui| {
                    ui.set_min_width(420.0);
                    ui.heading("本を削除");
                    ui.add_space(8.0);
                    ui.label(format!("「{name}」を削除します。"));
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("この本に入っているページ画像も削除されます。").weak(),
                    );
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(self.book_op_pending.is_none(), egui::Button::new("削除"))
                            .clicked()
                        {
                            confirmed = true;
                            close_confirm = true;
                        }
                        if ui.button("キャンセル").clicked() {
                            close_confirm = true;
                        }
                    });
                });
            if confirmed {
                delete_request = Some(name);
            }
            if close_confirm || response.should_close() {
                self.book_manager_delete_confirm = None;
            }
        }
        if refresh_request {
            self.book_list_cache = None;
            self.book_manager_rename_inputs.clear();
            self.request_book_list_refresh();
        }
        if let Some(path) = open_request {
            if crate::folder_tree::path_eq(&path, &self.book_root_path()) {
                self.open_books_root();
            } else {
                self.load_folder(path);
            }
        }
        if let Some(name) = set_active_request {
            self.settings.active_book_name = name.clone();
            self.settings.save();
            self.book_manager_rename_name = name;
        }
        if let Some(name) = pin_toggle_request {
            if let Some(pos) = self.settings.pinned_books.iter().position(|b| b == &name) {
                self.settings.pinned_books.remove(pos);
            } else {
                self.settings.pinned_books.push(name);
            }
            self.settings.save();
        }
        if let Some((old_name, new_name)) = rename_request {
            self.start_book_rename(ctx, old_name, new_name);
        }
        if let Some(name) = delete_request {
            self.start_book_delete(ctx, name);
        }
        if !open {
            self.show_book_manager = false;
            self.book_manager_delete_confirm = None;
            self.book_manager_rename_inputs.clear();
        }
    }

    pub(crate) fn draw_book_reorder(&mut self, ctx: &egui::Context) {
        enum CompletedBookReorderOp {
            Flush(Result<crate::books::BookOpResult, String>),
            Transfer(Result<crate::books::BookOpResult, String>),
        }

        let mut completed: Option<CompletedBookReorderOp> = None;
        if let Some(state) = self.book_reorder.as_mut() {
            if let Some(pending) = state.flush_pending.as_ref() {
                match pending.rx.try_recv() {
                    Ok(result) => completed = Some(CompletedBookReorderOp::Flush(result)),
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        completed = Some(CompletedBookReorderOp::Flush(Err(
                            "並べ替え保存が中断されました".to_string(),
                        )));
                    }
                }
            }
            if completed.is_none()
                && let Some(pending) = state.transfer_pending.as_ref()
            {
                match pending.rx.try_recv() {
                    Ok(result) => completed = Some(CompletedBookReorderOp::Transfer(result)),
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        completed = Some(CompletedBookReorderOp::Transfer(Err(
                            "ページ転送が中断されました".to_string(),
                        )));
                    }
                }
            }
        } else {
            return;
        }
        if let Some(result) = completed {
            match result {
                CompletedBookReorderOp::Flush(result) => {
                    if let Some(state) = self.book_reorder.as_mut() {
                        state.flush_pending = None;
                    }
                    match result {
                        Ok(crate::books::BookOpResult::Reordered {
                            folder,
                            count,
                            edit_moves,
                        }) => {
                            self.apply_book_page_edit_moves(&edit_moves);
                            self.book_reorder = None;
                            if self.current_folder.as_ref().is_some_and(|current| {
                                crate::folder_tree::path_eq(current, &folder)
                            }) {
                                self.pending_reload = true;
                            }
                            self.show_feedback_toast(format!(
                                "ページ順を保存しました: {count} ページ"
                            ));
                            return;
                        }
                        Ok(_) => {}
                        Err(err) => {
                            if let Some(state) = self.book_reorder.as_ref() {
                                crate::logger::log(format!(
                                    "book reorder flush failed for {}: {err}",
                                    state.folder.display()
                                ));
                            }
                            if let Some(state) = self.book_reorder.as_mut() {
                                state.error = Some(err.clone());
                            }
                            self.show_feedback_toast(format!(
                                "ページ順の保存に失敗しました: {err}"
                            ));
                            ctx.request_repaint();
                            return;
                        }
                    }
                }
                CompletedBookReorderOp::Transfer(result) => {
                    if let Some(state) = self.book_reorder.as_mut() {
                        state.transfer_pending = None;
                    }
                    match result {
                        Ok(crate::books::BookOpResult::Transfer(summary)) => {
                            self.apply_book_page_edit_moves(&summary.edit_moves);
                            self.apply_book_page_edit_copies(&summary.edit_copies);
                            self.book_list_cache = None;
                            if self.current_folder.as_ref().is_some_and(|current| {
                                crate::folder_tree::path_eq(current, &summary.source_folder)
                                    || crate::folder_tree::path_eq(current, &summary.target_folder)
                            }) {
                                self.pending_reload = true;
                            }
                            let verb = match summary.kind {
                                crate::books::BookTransferKind::Copy => "コピー",
                                crate::books::BookTransferKind::Move => "移動",
                            };
                            let toast = format!(
                                "「{}」へ {} ページ{}しました",
                                summary.target_book_name, summary.pages, verb
                            );
                            let mut close_empty_reorder = false;
                            if let Some(state) = self.book_reorder.as_mut() {
                                state.entries = summary.source_entries;
                                state.dirty = false;
                                state.dragging = None;
                                state.drag_auto_scroll_enabled = false;
                                state.drag_insert_index = None;
                                state.thumb_textures.clear();
                                state.thumb_failed.clear();
                                state.thumb_pending_keys.clear();
                                state.thumb_upload_backlog.clear();
                                state.thumb_tx = None;
                                state.thumb_rx = None;
                                state.selected_keys.clear();
                                if state.entries.is_empty() {
                                    close_empty_reorder = true;
                                } else {
                                    let index = state
                                        .selected
                                        .unwrap_or(0)
                                        .min(state.entries.len().saturating_sub(1));
                                    book_reorder_select_single(state, index);
                                }
                            }
                            if close_empty_reorder {
                                self.book_reorder = None;
                            }
                            self.show_feedback_toast(toast);
                            ctx.request_repaint();
                            return;
                        }
                        Ok(_) => {}
                        Err(err) => {
                            let mut reload_current = false;
                            if let Some(state) = self.book_reorder.as_ref() {
                                reload_current =
                                    self.current_folder.as_ref().is_some_and(|current| {
                                        crate::folder_tree::path_eq(current, &state.folder)
                                    });
                                crate::logger::log(format!(
                                    "book page transfer failed for {}: {err}",
                                    state.folder.display()
                                ));
                            }
                            self.book_reorder = None;
                            if reload_current {
                                self.pending_reload = true;
                            }
                            self.show_feedback_toast(format!("ページ転送に失敗しました: {err}"));
                            ctx.request_repaint();
                            return;
                        }
                    }
                }
            }
        }

        let mut thumb_results = Vec::new();
        let mut thumb_disconnected = false;
        if let Some(state) = self.book_reorder.as_ref()
            && let Some(rx) = state.thumb_rx.as_ref()
        {
            for _ in 0..BOOK_REORDER_THUMB_MAX_RECV_PER_FRAME {
                match rx.try_recv() {
                    Ok(result) => thumb_results.push(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        thumb_disconnected = true;
                        break;
                    }
                }
            }
        }
        if !thumb_results.is_empty()
            && let Some(state) = self.book_reorder.as_mut()
        {
            state.thumb_upload_backlog.extend(thumb_results);
        }
        if let Some(state) = self.book_reorder.as_mut() {
            let mut uploads = 0usize;
            while let Some(result) = state.thumb_upload_backlog.pop_front() {
                if let Some(image) = result.image {
                    if uploads >= BOOK_REORDER_THUMB_MAX_UPLOADS_PER_FRAME {
                        state
                            .thumb_upload_backlog
                            .push_front(crate::app::BookReorderThumbResult {
                                key: result.key,
                                image: Some(image),
                            });
                        break;
                    }
                    let texture = ctx.load_texture(
                        format!("book-reorder-thumb-{}", result.key),
                        image,
                        egui::TextureOptions::LINEAR,
                    );
                    state.thumb_pending_keys.remove(&result.key);
                    state.thumb_textures.insert(result.key, texture);
                    uploads += 1;
                } else {
                    state.thumb_pending_keys.remove(&result.key);
                    state.thumb_failed.insert(result.key);
                }
            }
            if !state.thumb_pending_keys.is_empty() || !state.thumb_upload_backlog.is_empty() {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }
        if thumb_disconnected && let Some(state) = self.book_reorder.as_mut() {
            let backlog_keys = state
                .thumb_upload_backlog
                .iter()
                .map(|result| result.key.clone())
                .collect::<HashSet<_>>();
            state
                .thumb_pending_keys
                .retain(|key| backlog_keys.contains(key));
            state.thumb_tx = None;
            state.thumb_rx = None;
            ctx.request_repaint();
        }

        if self.book_list_cache.is_none() && self.book_op_pending.is_none() {
            self.request_book_list_refresh();
        }
        let book_rows = self.book_list_cache.clone().unwrap_or_default();
        let mut close = false;
        let mut save_request: Option<(PathBuf, Vec<PathBuf>)> = None;
        let mut transfer_request: Option<(
            PathBuf,
            Vec<PathBuf>,
            Vec<PathBuf>,
            String,
            crate::books::BookTransferKind,
        )> = None;
        let mut transfer_combo_open = false;
        let mut missing_thumb_requests: Vec<(String, PathBuf)> = Vec::new();
        let mut thumb_by_path: HashMap<String, egui::TextureHandle> = self
            .book_reorder
            .as_ref()
            .map(|state| state.thumb_textures.clone())
            .unwrap_or_default();
        for (idx, item) in self.items.iter().enumerate() {
            let GridItem::Image(path) = item else {
                continue;
            };
            let texture = self.thumb_adjust_tex.get(&idx).cloned().or_else(|| {
                match self.thumbnails.get(idx) {
                    Some(ThumbnailState::Loaded { tex, .. }) => Some(tex.clone()),
                    _ => None,
                }
            });
            if let Some(texture) = texture {
                thumb_by_path.insert(crate::search_index_db::normalize_path(path), texture);
            }
        }
        let title = self
            .book_reorder
            .as_ref()
            .and_then(|state| {
                state
                    .folder
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|name| format!("ページ並べ替え: {name}"))
            })
            .unwrap_or_else(|| "ページ並べ替え".to_string());
        let mut window_open = true;
        egui::Window::new(title)
            .id(egui::Id::new("book_reorder_window"))
            .open(&mut window_open)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(
                BOOK_REORDER_DEFAULT_WINDOW_W,
                BOOK_REORDER_DEFAULT_WINDOW_H,
            ))
            .min_size(egui::vec2(
                BOOK_REORDER_MIN_WINDOW_W,
                BOOK_REORDER_MIN_WINDOW_H,
            ))
            .show(ctx, |ui| {
                let Some(state) = self.book_reorder.as_mut() else {
                    return;
                };
                let busy = state.flush_pending.is_some() || state.transfer_pending.is_some();
                ensure_book_reorder_selection(state);
                if let Some(err) = &state.error {
                    ui.colored_label(egui::Color32::from_rgb(210, 80, 80), err);
                }
                ui.horizontal(|ui| {
                    let selected_indices =
                        selected_book_reorder_indices(&state.entries, &state.selected_keys);
                    let selected_count = selected_indices.len();
                    let selected_index_set =
                        selected_indices.iter().copied().collect::<HashSet<_>>();
                    let can_left = !busy
                        && selected_indices
                            .iter()
                            .any(|idx| *idx > 0 && !selected_index_set.contains(&(*idx - 1)));
                    let can_right = !busy
                        && selected_indices.iter().any(|idx| {
                            *idx + 1 < state.entries.len()
                                && !selected_index_set.contains(&(*idx + 1))
                        });
                    if ui
                        .add_enabled(can_left, egui::Button::new("←"))
                        .on_hover_text("左へ移動")
                        .clicked()
                    {
                        if move_selected_book_reorder_by(state, -1) {
                            state.dirty = true;
                        }
                    }
                    if ui
                        .add_enabled(can_right, egui::Button::new("→"))
                        .on_hover_text("右へ移動")
                        .clicked()
                    {
                        if move_selected_book_reorder_by(state, 1) {
                            state.dirty = true;
                        }
                    }
                    let selected_label = if selected_count > 0 {
                        format!("選択 {selected_count} ページ（ここをドラッグして移動）")
                    } else {
                        "選択 0 ページ".to_string()
                    };
                    let selected_response = ui
                        .add_enabled(
                            !busy && selected_count > 0,
                            egui::Label::new(selected_label).sense(egui::Sense::click_and_drag()),
                        )
                        .on_hover_cursor(egui::CursorIcon::Grab)
                        .on_hover_text("選択ページ全体をドラッグして移動");
                    if !busy
                        && selected_response.drag_started()
                        && let Some(src) = selected_indices.first().copied()
                    {
                        state.selected = Some(src);
                        state.dragging = Some(src);
                        state.drag_auto_scroll_enabled = false;
                        state.drag_insert_index = Some(src);
                    }
                    ui.separator();
                    let slider = egui::Slider::new(
                        &mut state.thumb_tile_px,
                        BOOK_REORDER_MIN_TILE_PX..=BOOK_REORDER_MAX_TILE_PX,
                    )
                    .text("サムネ");
                    ui.add_enabled(!busy, slider);
                    ui.separator();
                    let source_folder = state.folder.clone();
                    let target_rows = book_rows
                        .iter()
                        .filter(|row| !crate::folder_tree::path_eq(&row.path, &source_folder))
                        .cloned()
                        .collect::<Vec<_>>();
                    if !target_rows.is_empty()
                        && !target_rows
                            .iter()
                            .any(|row| row.name == state.transfer_target_book)
                    {
                        state.transfer_target_book = target_rows[0].name.clone();
                    } else if target_rows.is_empty() {
                        state.transfer_target_book.clear();
                    }
                    ui.label("このページを");
                    let target_text = if target_rows.is_empty() {
                        "移動先なし".to_string()
                    } else {
                        state.transfer_target_book.clone()
                    };
                    let combo = egui::ComboBox::from_id_salt("book_reorder_transfer_target")
                        .width(180.0)
                        .height(360.0)
                        .selected_text(target_text)
                        .show_ui(ui, |ui| {
                            for row in &target_rows {
                                ui.selectable_value(
                                    &mut state.transfer_target_book,
                                    row.name.clone(),
                                    format!("{} ({}p)", row.name, row.page_count),
                                );
                            }
                        });
                    if egui::ComboBox::is_open(ctx, combo.response.id) {
                        transfer_combo_open = true;
                        consume_wheel_input(ctx);
                    }
                    ui.label("へ");
                    let target_valid = target_rows
                        .iter()
                        .any(|row| row.name == state.transfer_target_book);
                    let can_transfer = !busy && selected_count > 0 && target_valid;
                    if ui
                        .add_enabled(can_transfer, egui::Button::new("コピー"))
                        .clicked()
                    {
                        let current_order = state
                            .entries
                            .iter()
                            .map(|entry| entry.path.clone())
                            .collect::<Vec<_>>();
                        let selected_paths = selected_indices
                            .iter()
                            .filter_map(|idx| {
                                state.entries.get(*idx).map(|entry| entry.path.clone())
                            })
                            .collect::<Vec<_>>();
                        transfer_request = Some((
                            state.folder.clone(),
                            current_order,
                            selected_paths,
                            state.transfer_target_book.clone(),
                            crate::books::BookTransferKind::Copy,
                        ));
                    }
                    if ui
                        .add_enabled(can_transfer, egui::Button::new("移動"))
                        .clicked()
                    {
                        let current_order = state
                            .entries
                            .iter()
                            .map(|entry| entry.path.clone())
                            .collect::<Vec<_>>();
                        let selected_paths = selected_indices
                            .iter()
                            .filter_map(|idx| {
                                state.entries.get(*idx).map(|entry| entry.path.clone())
                            })
                            .collect::<Vec<_>>();
                        transfer_request = Some((
                            state.folder.clone(),
                            current_order,
                            selected_paths,
                            state.transfer_target_book.clone(),
                            crate::books::BookTransferKind::Move,
                        ));
                    }
                    ui.separator();
                    if ui.add_enabled(!busy, egui::Button::new("閉じる")).clicked() {
                        if state.dirty {
                            let paths = state.entries.iter().map(|e| e.path.clone()).collect();
                            save_request = Some((state.folder.clone(), paths));
                        } else {
                            close = true;
                        }
                    }
                    if busy {
                        let label = if state.transfer_pending.is_some() {
                            "転送中…"
                        } else {
                            "保存中…"
                        };
                        ui.label(egui::RichText::new(label).weak());
                    }
                });
                ui.separator();
                state.thumb_tile_px = state
                    .thumb_tile_px
                    .clamp(BOOK_REORDER_MIN_TILE_PX, BOOK_REORDER_MAX_TILE_PX);
                let tile = egui::vec2(state.thumb_tile_px, state.thumb_tile_px + 20.0);
                let gap = 8.0;
                let scroll_width = ui.available_width().max(tile.x + gap);
                let cols = book_reorder_grid_columns(scroll_width, tile.x, gap);
                let rows = state.entries.len().div_ceil(cols);
                let row_height = tile.y + gap;
                let scroll_height =
                    book_reorder_scroll_height(ui.available_height(), rows, row_height);
                if !transfer_combo_open && state.dragging.is_none() {
                    let scroll_key = ctx.input_mut(|i| {
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::PageUp) {
                            Some(BookReorderScrollKey::PageUp)
                        } else if i.consume_key(egui::Modifiers::NONE, egui::Key::PageDown) {
                            Some(BookReorderScrollKey::PageDown)
                        } else if i.consume_key(egui::Modifiers::NONE, egui::Key::Home) {
                            Some(BookReorderScrollKey::Home)
                        } else if i.consume_key(egui::Modifiers::NONE, egui::Key::End) {
                            Some(BookReorderScrollKey::End)
                        } else {
                            None
                        }
                    });
                    if let Some(key) = scroll_key {
                        let content_height = rows.max(1) as f32 * row_height;
                        state.scroll_offset_y = book_reorder_keyboard_scroll_offset(
                            state.scroll_offset_y,
                            content_height,
                            scroll_height,
                            row_height,
                            key,
                        );
                    }
                }
                let pointer_released = ui.input(|i| i.pointer.any_released());
                let pointer_pos =
                    ui.input(|i| i.pointer.hover_pos().or_else(|| i.pointer.interact_pos()));
                let mut move_request: Option<(usize, usize)> = None;
                state.drag_insert_index = None;
                ui.allocate_ui_with_layout(
                    egui::vec2(scroll_width, scroll_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.set_min_width(scroll_width);
                        ui.set_min_height(scroll_height);
                        let scroll_output = egui::ScrollArea::vertical()
                            .id_salt("book_reorder_thumb_scroll")
                            .vertical_scroll_offset(state.scroll_offset_y)
                            .max_height(scroll_height)
                            .auto_shrink([false, false])
                            .show_rows(ui, row_height, rows.max(1), |ui, row_range| {
                                egui::Grid::new("book_reorder_thumb_grid")
                                    .num_columns(cols)
                                    .spacing(egui::vec2(gap, gap))
                                    .show(ui, |ui| {
                                        for row in row_range {
                                            for col in 0..cols {
                                                let i = row * cols + col;
                                                let Some(entry) = state.entries.get(i) else {
                                                    let (rect, _) = ui.allocate_exact_size(
                                                        tile,
                                                        egui::Sense::hover(),
                                                    );
                                                    if !busy
                                                        && let Some(src) = state.dragging
                                                        && pointer_pos
                                                            .is_some_and(|pos| rect.contains(pos))
                                                    {
                                                        let insert_index = state.entries.len();
                                                        let indicator_x =
                                                            book_reorder_end_indicator_x(
                                                                rect,
                                                                state.entries.len(),
                                                                cols,
                                                                gap,
                                                            );
                                                        state.drag_insert_index =
                                                            Some(insert_index);
                                                        draw_book_reorder_insert_indicator(
                                                            ui,
                                                            indicator_x,
                                                            rect,
                                                        );
                                                        ui.output_mut(|out| {
                                                            out.cursor_icon =
                                                                egui::CursorIcon::Text;
                                                        });
                                                        if pointer_released {
                                                            move_request =
                                                                Some((src, insert_index));
                                                        }
                                                    }
                                                    continue;
                                                };
                                                let (rect, response) = ui.allocate_exact_size(
                                                    tile,
                                                    egui::Sense::click_and_drag(),
                                                );
                                                let key = crate::search_index_db::normalize_path(
                                                    &entry.path,
                                                );
                                                let selected = state.selected_keys.contains(&key);
                                                let dragging = state.dragging.is_some() && selected;
                                                let fill = if dragging {
                                                    ui.visuals().selection.bg_fill
                                                } else if selected {
                                                    ui.visuals().widgets.active.bg_fill
                                                } else {
                                                    ui.visuals().extreme_bg_color
                                                };
                                                ui.painter().rect_filled(rect, 4.0, fill);
                                                ui.painter().rect_stroke(
                                                    rect,
                                                    4.0,
                                                    egui::Stroke::new(
                                                        1.0,
                                                        if selected {
                                                            ui.visuals().selection.stroke.color
                                                        } else {
                                                            ui.visuals()
                                                                .widgets
                                                                .noninteractive
                                                                .bg_stroke
                                                                .color
                                                        },
                                                    ),
                                                    egui::StrokeKind::Inside,
                                                );
                                                let texture = thumb_by_path.get(&key);
                                                if texture.is_none()
                                                    && !state.thumb_failed.contains(&key)
                                                    && !state.thumb_pending_keys.contains(&key)
                                                {
                                                    missing_thumb_requests
                                                        .push((key.clone(), entry.path.clone()));
                                                }
                                                let image_rect =
                                                    rect.shrink2(egui::vec2(6.0, 18.0));
                                                if let Some(tex) = texture {
                                                    let tex_size = tex.size_vec2();
                                                    let scale = (image_rect.width() / tex_size.x)
                                                        .min(image_rect.height() / tex_size.y)
                                                        .min(1.0);
                                                    let size = egui::vec2(
                                                        tex_size.x * scale,
                                                        tex_size.y * scale,
                                                    );
                                                    let img_rect = egui::Rect::from_center_size(
                                                        image_rect.center(),
                                                        size,
                                                    );
                                                    ui.painter().image(
                                                        tex.id(),
                                                        img_rect,
                                                        egui::Rect::from_min_max(
                                                            egui::pos2(0.0, 0.0),
                                                            egui::pos2(1.0, 1.0),
                                                        ),
                                                        egui::Color32::WHITE,
                                                    );
                                                } else {
                                                    let placeholder =
                                                        if state.thumb_failed.contains(&key) {
                                                            "表示不可"
                                                        } else {
                                                            "読込中"
                                                        };
                                                    ui.painter().text(
                                                        image_rect.center(),
                                                        egui::Align2::CENTER_CENTER,
                                                        placeholder,
                                                        egui::FontId::proportional(11.0),
                                                        ui.visuals().weak_text_color(),
                                                    );
                                                }
                                                ui.painter().text(
                                                    rect.left_bottom() + egui::vec2(6.0, -5.0),
                                                    egui::Align2::LEFT_BOTTOM,
                                                    format!("{:04}", i + 1),
                                                    egui::FontId::monospace(11.0),
                                                    ui.visuals().text_color(),
                                                );
                                                let response = response.on_hover_ui(|ui| {
                                                    if let Some(tex) = texture {
                                                        let tex_size = tex.size_vec2();
                                                        let max_side = 360.0;
                                                        let scale = (max_side / tex_size.x)
                                                            .min(max_side / tex_size.y)
                                                            .min(2.5);
                                                        let size = egui::vec2(
                                                            tex_size.x * scale,
                                                            tex_size.y * scale,
                                                        );
                                                        let (preview_rect, _) = ui
                                                            .allocate_exact_size(
                                                                size,
                                                                egui::Sense::hover(),
                                                            );
                                                        ui.painter().image(
                                                            tex.id(),
                                                            preview_rect,
                                                            egui::Rect::from_min_max(
                                                                egui::pos2(0.0, 0.0),
                                                                egui::pos2(1.0, 1.0),
                                                            ),
                                                            egui::Color32::WHITE,
                                                        );
                                                    } else {
                                                        ui.label("サムネイル読み込み中");
                                                    }
                                                });
                                                if !busy && response.clicked() {
                                                    let (ctrl, shift) = ui.input(|input| {
                                                        (
                                                            input.modifiers.ctrl
                                                                || input.modifiers.command,
                                                            input.modifiers.shift,
                                                        )
                                                    });
                                                    if shift {
                                                        book_reorder_select_range(state, i);
                                                    } else if ctrl {
                                                        book_reorder_toggle_selection(state, i);
                                                    } else {
                                                        book_reorder_select_single(state, i);
                                                    }
                                                }
                                                if !busy && response.drag_started() {
                                                    if !state.selected_keys.contains(&key) {
                                                        book_reorder_select_single(state, i);
                                                    } else {
                                                        state.selected = Some(i);
                                                    }
                                                    state.dragging = Some(i);
                                                    state.drag_auto_scroll_enabled = true;
                                                    state.drag_insert_index = Some(i);
                                                }
                                                if !busy
                                                    && let Some(src) = state.dragging
                                                    && let Some((insert_index, indicator_x)) =
                                                        book_reorder_drop_target_for_pos(
                                                            rect,
                                                            i,
                                                            state.entries.len(),
                                                            cols,
                                                            gap,
                                                            pointer_pos,
                                                        )
                                                {
                                                    state.drag_insert_index = Some(insert_index);
                                                    draw_book_reorder_insert_indicator(
                                                        ui,
                                                        indicator_x,
                                                        rect,
                                                    );
                                                    ui.output_mut(|out| {
                                                        out.cursor_icon = egui::CursorIcon::Text;
                                                    });
                                                    if pointer_released {
                                                        move_request = Some((src, insert_index));
                                                    }
                                                }
                                            }
                                            ui.end_row();
                                        }
                                    });
                            });
                        state.scroll_offset_y = scroll_output.state.offset.y;
                        if !busy
                            && !pointer_released
                            && state.dragging.is_some()
                            && state.drag_auto_scroll_enabled
                            && let Some(pos) = pointer_pos
                        {
                            let delta = book_reorder_auto_scroll_delta(
                                pos.y,
                                scroll_output.inner_rect.top(),
                                scroll_output.inner_rect.bottom(),
                            );
                            if delta.abs() > f32::EPSILON {
                                let max_offset = (scroll_output.content_size.y
                                    - scroll_output.inner_rect.height())
                                .max(0.0);
                                state.scroll_offset_y =
                                    (state.scroll_offset_y + delta).clamp(0.0, max_offset);
                                ctx.request_repaint_after(std::time::Duration::from_millis(16));
                            }
                        }
                    },
                );
                if let Some((src, insert_index)) = move_request {
                    let len = state.entries.len();
                    if src < len {
                        if move_selected_book_reorder_group(state, insert_index) {
                            state.dirty = true;
                        }
                    }
                    state.dragging = None;
                    state.drag_auto_scroll_enabled = false;
                    state.drag_insert_index = None;
                } else if pointer_released {
                    state.dragging = None;
                    state.drag_auto_scroll_enabled = false;
                    state.drag_insert_index = None;
                }
            });
        if !window_open
            && save_request.is_none()
            && !close
            && let Some(state) = self.book_reorder.as_ref()
        {
            let busy = state.flush_pending.is_some() || state.transfer_pending.is_some();
            if !busy {
                if state.dirty {
                    let paths = state.entries.iter().map(|e| e.path.clone()).collect();
                    save_request = Some((state.folder.clone(), paths));
                } else {
                    close = true;
                }
            }
        }
        if !missing_thumb_requests.is_empty()
            && let Some(state) = self.book_reorder.as_mut()
        {
            if state.thumb_rx.is_none() || state.thumb_tx.is_none() {
                let (tx, rx) = std::sync::mpsc::channel();
                state.thumb_tx = Some(tx);
                state.thumb_rx = Some(rx);
            }
            let available =
                BOOK_REORDER_THUMB_MAX_IN_FLIGHT.saturating_sub(state.thumb_pending_keys.len());
            let mut scheduled = 0usize;
            let tx = state.thumb_tx.clone();
            if let Some(tx) = tx {
                for (key, path) in missing_thumb_requests {
                    if scheduled >= available {
                        break;
                    }
                    if state.thumb_textures.contains_key(&key)
                        || state.thumb_failed.contains(&key)
                        || !state.thumb_pending_keys.insert(key.clone())
                    {
                        continue;
                    }
                    let tx = tx.clone();
                    let key_for_worker = key.clone();
                    let spawn_result = std::thread::Builder::new()
                        .name("book-reorder-thumb".into())
                        .spawn(move || {
                            let image =
                                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    crate::thumb_loader::decode_image_for_thumb(
                                        &path,
                                        BOOK_REORDER_THUMB_DECODE_PX,
                                    )
                                })) {
                                    Ok(image) => image,
                                    Err(_) => {
                                        crate::logger::log(format!(
                                            "book reorder thumbnail decode panicked: {}",
                                            path.display()
                                        ));
                                        None
                                    }
                                };
                            let _ = tx.send(crate::app::BookReorderThumbResult {
                                key: key_for_worker,
                                image,
                            });
                        });
                    match spawn_result {
                        Ok(_) => {
                            scheduled += 1;
                        }
                        Err(err) => {
                            state.thumb_pending_keys.remove(&key);
                            state.error =
                                Some(format!("サムネイル読み込みを開始できません: {err}"));
                        }
                    }
                }
            }
            if scheduled > 0 {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }
        if let Some((folder, current_order, selected_paths, target_book, kind)) = transfer_request {
            if let Some(pending) = self.start_book_transfer(
                ctx,
                folder,
                current_order,
                selected_paths,
                target_book,
                kind,
            ) {
                if let Some(state) = self.book_reorder.as_mut() {
                    state.transfer_pending = Some(pending);
                    state.error = None;
                }
            }
        }
        if let Some((folder, paths)) = save_request {
            if let Some(pending) = self.start_book_reorder_flush(ctx, folder, paths) {
                if let Some(state) = self.book_reorder.as_mut() {
                    state.flush_pending = Some(pending);
                    state.error = None;
                }
            }
        }
        if close {
            self.book_reorder = None;
        }
    }

    // ── 進捗バー ─────────────────────────────────────────────────────

    /// 進捗バーオーバーレイ（左下フローティング）を描画する。
    pub(crate) fn render_progress_overlay(&self, ctx: &egui::Context) {
        let ((cur_normal, peak_normal), (cur_upgrade, peak_upgrade)) = self.progress_snapshot();
        if peak_normal == 0 && peak_upgrade == 0 {
            return;
        }

        egui::Area::new("progress_overlay".into())
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(8.0, -8.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(PROGRESS_BG_COLOR)
                    .show(ui, |ui| {
                        if peak_normal > 0 {
                            let done = peak_normal.saturating_sub(cur_normal);
                            let progress = done as f32 / peak_normal as f32;
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("先読み    ")
                                        .monospace()
                                        .color(PROGRESS_LABEL_COLOR),
                                );
                                ui.add(
                                    egui::ProgressBar::new(progress)
                                        .desired_width(220.0)
                                        .fill(PROGRESS_NORMAL_COLOR)
                                        .text(
                                            egui::RichText::new(format!(
                                                "{} / {}",
                                                done, peak_normal
                                            ))
                                            .color(egui::Color32::BLACK),
                                        ),
                                );
                            });
                        }
                        if peak_upgrade > 0 {
                            let done = peak_upgrade.saturating_sub(cur_upgrade);
                            let progress = done as f32 / peak_upgrade as f32;
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("高画質化  ")
                                        .monospace()
                                        .color(PROGRESS_LABEL_COLOR),
                                );
                                ui.add(
                                    egui::ProgressBar::new(progress)
                                        .desired_width(220.0)
                                        .fill(PROGRESS_UPGRADE_COLOR)
                                        .text(
                                            egui::RichText::new(format!(
                                                "{} / {}",
                                                done, peak_upgrade
                                            ))
                                            .color(egui::Color32::BLACK),
                                        ),
                                );
                            });
                        }
                    });
            });
        // 進行中は毎フレーム再描画してバーをスムーズに更新
        ctx.request_repaint();
    }

    /// コンテナ (PDF / ZIP) のページ列挙待ちを左下に表示する。
    ///
    /// PDF を Enter で開いた直後の 100ms〜1.3 秒の間 (PDFium が PDF を開いて構造解析する
    /// 時間)、グリッドは「親フォルダのまま動かない」状態になる。これだけだと
    /// 「Enter は効いたのか?」とユーザーが不安になるため、`先読み N/M` バーと同じ
    /// 左下位置に「読み込み中…」バッジを出して進行中を示す。
    ///
    /// `items.is_empty()` (= 初回フォルダロード) のときは [`render_grid`] 内で中央の
    /// `"読み込み中…"` ラベルが既に出るので、ここでは items が残っている遷移ケース
    /// (親フォルダ → PDF / ZIP) のみカバーする。
    pub(crate) fn render_container_enumerate_overlay(&self, ctx: &egui::Context) {
        let pdf_pending = self.pdf_enumerate_pending.is_some();
        let zip_pending = self.zip_enumerate_pending.is_some();
        if !pdf_pending && !zip_pending {
            return;
        }
        // items 空のときは中央ラベルに任せる (二重表示を防ぐ)
        if self.items.is_empty() {
            return;
        }

        let label = if pdf_pending {
            "PDF を読み込み中…"
        } else {
            "ZIP を読み込み中…"
        };

        // 進捗バーが既に出ている場合は、その上に積み上がるよう Y オフセットを調整する。
        // 進捗バー本体は ~40-80px の高さ、ここではざっくり 60px 上に置く。
        let ((_, peak_normal), (_, peak_upgrade)) = self.progress_snapshot();
        let y_offset = if peak_normal > 0 || peak_upgrade > 0 {
            -72.0
        } else {
            -8.0
        };

        egui::Area::new("container_enumerate_overlay".into())
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(8.0, y_offset))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(PROGRESS_BG_COLOR)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(label)
                                .monospace()
                                .color(PROGRESS_LABEL_COLOR),
                        );
                    });
            });

        // pending 中は毎フレーム再描画 (受信ポーリングのため request_repaint は既に
        // 別経路でも走っているが、念のためここでも要求)
        ctx.request_repaint();
    }

    // ── ツールバー ───────────────────────────────────────────────────

    /// ツールバーを描画し、お気に入りナビゲーション先を返す。
    /// ソート変更があった場合はフォルダの再ロードも行う。
    pub(crate) fn render_toolbar(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        // Vec を先にクローンして borrow checker の制約を回避
        let tb_cols = self.settings.toolbar_cols_items.clone();
        let tb_aspects = self.settings.toolbar_aspect_items.clone();
        let tb_sorts = self.settings.toolbar_sort_items.clone();
        let toolbar_details_visible = self.settings.toolbar_cols_details_visible;
        let details_mode = self.settings.grid_view_mode == GridViewMode::Details;
        // v2.0.0: 各セクションの表示可否は明示フラグ (show_toolbar_*) だけで決める。
        // 旧来は「項目リストが空 = 非表示」だったが、それだと項目を全部外したとき
        // セクションのラベル (= 右クリックで項目を再追加する入口) ごと消えてしまう
        // (Codex P3)。フラグで存在を管理し、項目が無いセクションはラベル + ヒントだけ
        // 描く。空き領域 / 「設定→ツールバー」/ ラベル右クリックから ON/OFF・項目編集できる。
        // フラグは既定 true なので、初期状態の見た目は従来と同じ。
        let show_cols = self.settings.show_toolbar_cols;
        // 比率は詳細一覧モードでは無意味なので隠す (従来どおり)。
        let show_aspect = self.settings.show_toolbar_aspect && !details_mode;
        let show_sort = self.settings.show_toolbar_sort;
        let show_favs = self.settings.show_toolbar_favorites;
        let show_rating = self.settings.show_toolbar_rating;
        let show_tags = self.settings.show_toolbar_tags;
        let show_folder_tree_button = self.settings.show_toolbar_folder_tree_button;
        let show_bookshelf = self.settings.show_toolbar_bookshelf;
        let book_sort_locked =
            self.current_folder_is_book_folder() || self.items_are_reading_history_view;
        if show_bookshelf && self.book_list_cache.is_none() && self.book_op_pending.is_none() {
            self.request_book_list_refresh();
        }
        let active_book_name = self.active_book_name();
        let toolbar_book_rows = self.book_list_cache.clone();
        let toolbar_pinned_books = self.settings.pinned_books.clone();
        let has_book_add_target = self.selected.is_some() || !self.checked.is_empty();
        let has_rating_selection = has_book_add_target;
        let toolbar_section_order = crate::settings::ToolbarSectionId::ordered_with_fallback(
            &self.settings.toolbar_section_order,
        );
        // 前フレームのツールバー content rect (空き領域 右クリック用背景 interact の矩形)。
        let toolbar_bg_rect = self.toolbar_content_rect;
        // ドラッグ並べ替えの許可状態 (既定 OFF)。OFF のときはカーソルも変えない。
        let drag_enabled = self.settings.toolbar_section_drag_enabled;
        let any_toolbar_section = show_folder_tree_button
            || show_bookshelf
            || show_cols
            || show_aspect
            || show_sort
            || show_favs
            || show_rating
            || show_tags;

        if !any_toolbar_section {
            return None;
        }

        let mut toolbar_fav_nav: Option<PathBuf> = None;
        let mut toolbar_sort_changed = false;
        let mut toolbar_rating_changed = false;
        let mut toolbar_rating_assign_selection: Option<u8> = None;
        let mut toolbar_rating_assign_container: Option<u8> = None;
        let mut toolbar_book_add = false;
        let mut toolbar_book_open_active = false;
        let mut toolbar_book_target_name: Option<String> = None;
        let mut toolbar_book_pin_open: Option<String> = None;
        let mut toolbar_book_pin_add: Option<String> = None;
        let mut toolbar_tag_click: Option<String> = None;
        let mut toolbar_tag_search: Option<String> = None;
        let mut toolbar_tag_container: Option<String> = None;
        let mut toolbar_tag_apply = false;
        let mut toolbar_tag_view_open = false;
        let mut toolbar_combo_popup_open = false;
        // ドラッグ並べ替え: 前フレームの可視セクション矩形を奪って drop 計算に使い、
        // 今フレーム描いた矩形を集めて次フレーム用に格納する。
        let last_section_anchors = std::mem::take(&mut self.toolbar_section_anchors);
        let mut current_section_anchors: Vec<(crate::settings::ToolbarSectionId, egui::Rect)> =
            Vec::new();

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(2.0);
            // ツールバー内の widget は label / selectable_label / ComboBox / radio が
            // 混在しており、widget 高さの違いで縦位置がガタつく (ComboBox は label より
            // 大きい)。`horizontal_wrapped` (= 既存の折返しレイアウト) は維持しつつ、
            // 内部の widget 高さを統一する。
            //
            // ⚠ 過去の失敗:
            //   - `with_layout(LeftToRight(Center).with_main_wrap(true))`: TopBottomPanel
            //     と組み合わせると panel 高さが膨張する
            //   - `spacing_mut` を `horizontal_wrapped` の **中**で設定: 初期行高は
            //     呼出時点の値で計算されるため反映されず、Yu Gothic の galley が
            //     interact_size.y を超えると widget が膨らむ → ガタつき (Codex 助言)
            //
            // 正しい配置: `ui.scope` で spacing を上書き → その内側で `horizontal_wrapped`
            // を呼ぶ。scope はスタイル変更を toolbar 外に漏らさない安全策。
            // ツールバー専用スタイル適用 helper。
            // 外側の `ui.scope` だけでなく、各 ComboBox の `show_ui` closure 冒頭でも
            // 呼ぶ必要がある。理由: egui の ComboBox popup は親 ui の TextStyle を
            // 継承せず別の context で描画されるため、popup 内の項目だけ通常 Yu Gothic
            // (= 上寄り) で描画されてしまう (Codex P3 2026-05)。
            fn apply_toolbar_style(ui: &mut egui::Ui) {
                let toolbar_family = egui::FontFamily::Name(std::sync::Arc::<str>::from(
                    crate::ui_fonts::TOOLBAR_TEXT_FAMILY_NAME,
                ));
                let body_size = ui.style().text_styles[&egui::TextStyle::Body].size;
                let button_size = ui.style().text_styles[&egui::TextStyle::Button].size;
                ui.style_mut().text_styles.insert(
                    egui::TextStyle::Body,
                    egui::FontId::new(body_size, toolbar_family.clone()),
                );
                ui.style_mut().text_styles.insert(
                    egui::TextStyle::Button,
                    egui::FontId::new(button_size, toolbar_family),
                );
                ui.spacing_mut().interact_size.y = 22.0;
                ui.spacing_mut().button_padding.y = 1.0;
            }

            const TOOLBAR_COLS_COMBO_HEIGHT: f32 = 320.0;
            const TOOLBAR_ASPECT_COMBO_HEIGHT: f32 = 280.0;
            const TOOLBAR_SORT_COMBO_HEIGHT: f32 = 240.0;

            // 空き領域 右クリック用の背景 interact。セクション (= ui.scope の中身) より
            // **前** に登録することで z-order が背面になり、ボタンの直接クリックは奪わず、
            // 何も無い余白の右クリックだけ背景が拾う (egui hit_test の仕様、kittest で検証済)。
            // 矩形は前フレームの content rect を使う (今フレームの高さは描画後でないと
            // 分からないため)。初回 (None) は背景メニュー無効。
            //
            // ⚠ 高さは「ツールバー先頭 1 行ぶん」に丸める (Codex P2)。前フレームが複数行で
            // 今フレームが 1 行に縮んだ場合、前フレームの背の高い rect をそのまま使うと、
            // 下のアドレスバー / グリッドの空き部分まで背景 interact が伸び、そこの右クリックを
            // 誤って奪うことがある。先頭行の高さに収めれば必ず現ツールバー内に収まり、
            // 先頭行の末尾余白 (最も普通の空き領域) は拾える。複数行時の 2 行目以降の余白は
            // 「設定→ツールバー」メニューで代替できる。
            let bg_resp = toolbar_bg_rect.map(|r| {
                let row_h = (ui.spacing().interact_size.y + 6.0).min(r.height());
                let capped =
                    egui::Rect::from_min_size(r.min, egui::vec2(r.width(), row_h.max(1.0)));
                ui.interact(capped, ui.id().with("toolbar_bg_menu"), egui::Sense::click())
            });

            let scope_resp = ui.scope(|ui| {
                // ツールバー本体に toolbar スタイルを適用 (Yu Gothic の glyph 上寄り問題を
                // FontTweak.y_offset で補正)。詳細: src/ui_fonts.rs の TOOLBAR_TEXT_FAMILY_NAME。
                apply_toolbar_style(ui);
                // `horizontal_wrapped` では既定の wrap mode が Button 内テキストにも伝播する。
                // 右端の残り幅が小さいと「日付↑」などがボタン内部で縦に折れ、ツールバー
                // 全体が不自然に膨らむ。ツールバー本体だけはボタン内を折らず、ボタン単位で
                // 次行へ流す。
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

                // ツールバー用ラベル: ComboBox / selectable_label と同じ高さで描画
                // して縦位置を揃える。
                // ⚠ 幅 0.0 を渡すと「親レイアウト上の占有幅 0」と解釈されて次の
                // widget と詰まる/重なる/wrap 判定が狂う (Codex 助言 2026-05)。
                // 固定幅を明示すること。日本語ラベルは数が固定なので呼び出し側で
                // 目視チューンした値を渡す。
                // v2.0.0 Phase 3: セクションラベルは (a) 右クリックで「セクション設定」メニュー、
                // (b) ドラッグでセクション並べ替え (許可時のみ)、の対象にするため、ラベル領域に
                // sense を重ねた response を返す。id はラベル文字列で salt して衝突を避ける。
                // `drag_enabled=false` のときは click のみにして、ドラッグ判定もカーソル変更も
                // 起きないようにする (実機フィードバック: カーソルが頻繁に変わるのが煩わしい)。
                fn toolbar_label(
                    ui: &mut egui::Ui,
                    text: &str,
                    width: f32,
                    drag_enabled: bool,
                ) -> egui::Response {
                    let h = ui.spacing().interact_size.y;
                    let rect = ui
                        .allocate_ui_with_layout(
                            egui::vec2(width, h),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| ui.label(text),
                        )
                        .response
                        .rect;
                    let sense = if drag_enabled {
                        egui::Sense::click_and_drag()
                    } else {
                        egui::Sense::click()
                    };
                    ui.interact(rect, ui.id().with((text, "toolbar_section_label")), sense)
                }

                ui.horizontal_wrapped(|ui| {
                use crate::settings::ToolbarSectionId as TS;
                let mut first_section = true;
                // ピン留めタグ (タグセクションで使用)。ループ前に 1 度だけ算出する。
                let toolbar_tags: Vec<_> = self
                    .settings
                    .tags
                    .iter()
                    .filter(|tag| tag.show_shortcut)
                    .map(|t| t.name.clone())
                    .collect();
                // 「行頭に表示」フラグ集合 (ループ内 self 借用衝突回避のため複製)。
                let toolbar_new_row = self.settings.toolbar_section_new_row.clone();
                // セクションラベルの hover ヒント。ドラッグ許可状態で文言を変える。
                let lead_hint: &str = if drag_enabled {
                    "ドラッグ: 並べ替え / 右クリック: 設定"
                } else {
                    "右クリック: 設定"
                };
                for &section in &toolbar_section_order {
                    // セクションごとの表示可否 (= 旧 `if show_*` と同一条件)。
                    let visible = match section {
                        TS::FolderTree => show_folder_tree_button,
                        TS::Bookshelf => show_bookshelf,
                        TS::Cols => show_cols,
                        TS::Aspect => show_aspect,
                        TS::Sort => show_sort,
                        TS::Rating => show_rating,
                        TS::Favorites => show_favs,
                        TS::Tags => show_tags,
                        TS::Unknown => false,
                    };
                    if !visible {
                        continue;
                    }
                    if !first_section {
                        // 「行頭に表示」セクションは、自動折返しを待たずここで改行する
                        // (egui の end_row は wrapping layout で次の行へ移る)。それ以外は
                        // 従来どおりセパレータで区切る。
                        if toolbar_new_row.contains(&section) {
                            ui.end_row();
                        } else {
                            ui.separator();
                        }
                    }
                    match section {
                // ツールバー VST ボタンは v0.9.0 開発中に削除 (= ユーザー要望 2026-04
                // 「ツールバーの VST ボタンも不要になったので削除」)。
                // VST3 プラグインのプレイバックパネルは動画再生中にホバーバー側の
                // VST ボタンから開く (フルスクリーンビューポート内で完結)。
                // 通常表示中はパネルを開く手段は無く、設定変更は環境設定→
                // VST3 プラグイン から行う運用。
                TS::FolderTree => {
                    let active = self.settings.folder_tree_pane_visible;
                    // selectable_label は click sense のみなので、ドラッグ並べ替えも効くよう
                    // Button::selectable + click_and_drag にする (見た目は selectable と同じ)。
                    // ドラッグ不許可時は click のみ (カーソルが変わらない)。
                    let tree_sense = if drag_enabled {
                        egui::Sense::click_and_drag()
                    } else {
                        egui::Sense::click()
                    };
                    let resp = ui
                        .add(egui::Button::selectable(active, "ツリー").sense(tree_sense))
                        .on_hover_text(format!("左側に実フォルダツリーを表示\n{lead_hint}"));
                    if resp.clicked() {
                        self.set_folder_tree_pane_visible(!active);
                    }
                    self.finish_toolbar_section_lead(
                        ui,
                        resp,
                        TS::FolderTree,
                        &mut current_section_anchors,
                        &last_section_anchors,
                    );
                }
                TS::Bookshelf => {
                    let lead = toolbar_label(ui, "本棚:", 46.0, drag_enabled).hover_tip(lead_hint);
                    self.finish_toolbar_section_lead(
                        ui,
                        lead,
                        TS::Bookshelf,
                        &mut current_section_anchors,
                        &last_section_anchors,
                    );
                    let combo = egui::ComboBox::from_id_salt("toolbar_book_target_combo")
                        .width(160.0)
                        .height(320.0)
                        .selected_text(active_book_name.clone())
                        .show_ui(ui, |ui| {
                            apply_toolbar_style(ui);
                            match toolbar_book_rows.as_ref() {
                                Some(rows) if rows.is_empty() => {
                                    ui.label(egui::RichText::new("本はまだありません").weak());
                                }
                                Some(rows) => {
                                    for row in rows {
                                        let selected = row.name == active_book_name;
                                        let label = format!("{} ({}p)", row.name, row.page_count);
                                        if ui.selectable_label(selected, label).clicked() {
                                            toolbar_book_target_name = Some(row.name.clone());
                                            ui.close();
                                        }
                                    }
                                }
                                None => {
                                    ui.label(egui::RichText::new("本棚を読み込み中…").weak());
                                }
                            }
                        });
                    toolbar_combo_popup_open |= egui::ComboBox::is_open(ctx, combo.response.id);
                    let add_resp = ui
                        .add_enabled(has_book_add_target, egui::Button::new("追加"))
                        .hover_tip(if has_book_add_target {
                            "選択中またはチェック済みの画像・ページを追加先の本へ追加"
                        } else {
                            "追加する画像・ページを選択してください"
                        });
                    if add_resp.clicked() {
                        toolbar_book_add = true;
                    }
                    if ui
                        .button("開く")
                        .hover_tip(format!("追加先の本「{}」を開く", active_book_name))
                        .clicked()
                    {
                        toolbar_book_open_active = true;
                    }
                    // ピン留め本は他セクションと揃えて常にボタンで出す (コンボは全本用に常時表示)。
                    // 折りたたみモードのときだけ ▶/▼ で畳める。
                    let book_mode = self.settings.toolbar_bookshelf_display;
                    let show_pins =
                        if book_mode == crate::settings::ToolbarSectionDisplay::Collapsible {
                            let collapsed = self.settings.toolbar_bookshelf_collapsed;
                            let arrow = if collapsed { "▶" } else { "▼" };
                            if ui
                                .button(arrow)
                                .on_hover_text("ピン留め本の折りたたみ")
                                .clicked()
                            {
                                self.settings.toolbar_bookshelf_collapsed = !collapsed;
                                self.settings.save();
                            }
                            !collapsed
                        } else {
                            true
                        };
                    if show_pins {
                        // 左=開く / 右=選択したアイテムを追加。実在する本だけ (削除済みピンは出さない)。
                        for pin in &toolbar_pinned_books {
                            let exists = toolbar_book_rows
                                .as_ref()
                                .is_some_and(|rows| rows.iter().any(|r| &r.name == pin));
                            if !exists {
                                continue;
                            }
                            // ハイライトはしない (追加先=active 本を強調すると「このアイテムを
                            // その本に入れた」と誤解されるため。ユーザー判断 2026-06-20)。
                            let resp = ui.button(pin).hover_tip(
                                "左: この本を開く / 右: 選択したアイテムをこの本へ追加",
                            );
                            if resp.clicked() {
                                toolbar_book_pin_open = Some(pin.clone());
                            }
                            if resp.secondary_clicked() {
                                toolbar_book_pin_add = Some(pin.clone());
                            }
                        }
                    }
                }
                TS::Cols => {
                    let lead = toolbar_label(ui, "列:", 28.0, drag_enabled).hover_tip(lead_hint);
                    self.finish_toolbar_section_lead(
                        ui,
                        lead,
                        TS::Cols,
                        &mut current_section_anchors,
                        &last_section_anchors,
                    );
                    match self.settings.toolbar_cols_display {
                        crate::settings::ToolbarSectionDisplay::Buttons
                        | crate::settings::ToolbarSectionDisplay::Collapsible
                        | crate::settings::ToolbarSectionDisplay::Unknown => {
                            for &cols in &tb_cols {
                                let selected = !details_mode && self.settings.grid_cols == cols;
                                if ui.selectable_label(selected, format!(" {cols} ")).clicked() {
                                    self.set_grid_view_mode(GridViewMode::Thumbnail);
                                    self.settings.grid_cols = cols;
                                    self.settings.save();
                                }
                            }
                            if toolbar_details_visible {
                                if ui
                                    .selectable_label(details_mode, " 詳細 ")
                                    .on_hover_text(self.keymap.first_chord_action_label(
                                        "サムネイルなしの詳細一覧に切り替えます",
                                        KeyAction::GridToggleDetailsView,
                                    ))
                                    .clicked()
                                {
                                    self.set_grid_view_mode(GridViewMode::Details);
                                }
                            }
                        }
                        crate::settings::ToolbarSectionDisplay::Dropdown => {
                            let current_text = if details_mode && toolbar_details_visible {
                                "詳細".to_string()
                            } else {
                                format!("{} 列", self.settings.grid_cols)
                            };
                            let combo = egui::ComboBox::from_id_salt("toolbar_cols_combo")
                                .width(64.0)
                                .height(TOOLBAR_COLS_COMBO_HEIGHT)
                                .selected_text(current_text)
                                .show_ui(ui, |ui| {
                                    apply_toolbar_style(ui);
                                    for &cols in &tb_cols {
                                        if ui
                                            .selectable_label(
                                                !details_mode && self.settings.grid_cols == cols,
                                                format!("{cols} 列"),
                                            )
                                            .clicked()
                                        {
                                            self.set_grid_view_mode(GridViewMode::Thumbnail);
                                            self.settings.grid_cols = cols;
                                            self.settings.save();
                                        }
                                    }
                                    if toolbar_details_visible {
                                        if !tb_cols.is_empty() {
                                            ui.separator();
                                        }
                                        if ui.selectable_label(details_mode, "詳細").clicked() {
                                            self.set_grid_view_mode(GridViewMode::Details);
                                        }
                                    }
                                });
                            toolbar_combo_popup_open |=
                                egui::ComboBox::is_open(ctx, combo.response.id);
                        }
                    }
                    // 項目が無い (列も詳細も外した) ときは、ラベルだけだと意図が伝わらないので
                    // 右クリックへ誘導するヒントを出す (Codex P3: 項目を全部外しても入口を残す)。
                    if tb_cols.is_empty() && !toolbar_details_visible {
                        ui.label(egui::RichText::new("(右クリックで列を選択)").weak());
                    }
                }
                TS::Aspect => {
                    let lead = toolbar_label(ui, "比率:", 42.0, drag_enabled).hover_tip(lead_hint);
                    self.finish_toolbar_section_lead(
                        ui,
                        lead,
                        TS::Aspect,
                        &mut current_section_anchors,
                        &last_section_anchors,
                    );
                    let auto_visible = self.settings.toolbar_aspect_auto_visible;
                    let auto_selected = self.settings.thumb_aspect_auto;
                    let auto_label = if let Some(current) = self.auto_aspect.current {
                        format!("自動 ({})", current.label())
                    } else {
                        "自動".to_string()
                    };

                    // 「自動」クリック時の共通処理 (展開 / プルダウン両方から呼ぶ)
                    fn activate_auto(app: &mut App) {
                        let was_off = !app.settings.thumb_aspect_auto;
                        let prev_effective = app.effective_thumb_aspect();
                        app.settings.thumb_aspect_auto = true;
                        app.auto_aspect.reset_decision_only();
                        if was_off {
                            app.rebuild_auto_aspect_samples_from_loaded();
                        }
                        app.maybe_apply_auto_aspect(true);
                        let new_effective = app.effective_thumb_aspect();
                        if prev_effective != new_effective {
                            app.fixup_scroll_for_aspect_change(new_effective);
                        }
                        app.settings.save();
                    }
                    // 個別比率クリック時の共通処理
                    fn activate_aspect(app: &mut App, aspect: crate::settings::ThumbAspect) {
                        if app.settings.thumb_aspect_auto || app.settings.thumb_aspect != aspect {
                            app.fixup_scroll_for_aspect_change(aspect);
                        }
                        app.settings.thumb_aspect_auto = false;
                        app.settings.thumb_aspect = aspect;
                        app.settings.save();
                    }

                    match self.settings.toolbar_aspect_display {
                        crate::settings::ToolbarSectionDisplay::Buttons
                        | crate::settings::ToolbarSectionDisplay::Collapsible
                        | crate::settings::ToolbarSectionDisplay::Unknown => {
                            if auto_visible
                                && ui.selectable_label(auto_selected, auto_label).clicked()
                            {
                                activate_auto(self);
                            }
                            for &aspect in &tb_aspects {
                                let selected = !self.settings.thumb_aspect_auto
                                    && self.settings.thumb_aspect == aspect;
                                if ui.selectable_label(selected, aspect.label()).clicked() {
                                    activate_aspect(self, aspect);
                                }
                            }
                        }
                        crate::settings::ToolbarSectionDisplay::Dropdown => {
                            // 現在の選択ラベル: Auto なら auto_label、それ以外は手動値
                            let current_text = if self.settings.thumb_aspect_auto {
                                auto_label.clone()
                            } else {
                                self.settings.thumb_aspect.label().to_string()
                            };
                            let combo = egui::ComboBox::from_id_salt("toolbar_aspect_combo")
                                .width(120.0)
                                .height(TOOLBAR_ASPECT_COMBO_HEIGHT)
                                .selected_text(current_text)
                                .show_ui(ui, |ui| {
                                    apply_toolbar_style(ui);
                                    if auto_visible {
                                        let resp = ui.selectable_label(auto_selected, &auto_label);
                                        if resp.clicked() {
                                            activate_auto(self);
                                        }
                                        // separator は auto と他項目両方ある場合のみ
                                        if !tb_aspects.is_empty() {
                                            ui.separator();
                                        }
                                    }
                                    for &aspect in &tb_aspects {
                                        let selected = !self.settings.thumb_aspect_auto
                                            && self.settings.thumb_aspect == aspect;
                                        if ui.selectable_label(selected, aspect.label()).clicked() {
                                            activate_aspect(self, aspect);
                                        }
                                    }
                                });
                            toolbar_combo_popup_open |=
                                egui::ComboBox::is_open(ctx, combo.response.id);
                        }
                    }
                    // 「自動」も手動比率も全部外したときの右クリック誘導 (Codex P3)。
                    if !auto_visible && tb_aspects.is_empty() {
                        ui.label(egui::RichText::new("(右クリックで比率を選択)").weak());
                    }
                }
                TS::Sort => {
                    let details_sort_disabled = self.details_header_sort_active();
                    let sort_disabled = details_sort_disabled || book_sort_locked;
                    let sort_label = toolbar_label(ui, "ソート:", 54.0, drag_enabled);
                    let sort_label = if book_sort_locked {
                        sort_label.hover_tip("本棚内または読書履歴では表示順が固定されます。")
                    } else if details_sort_disabled {
                        sort_label.hover_tip(
                            "詳細一覧の列ヘッダで並べ替え中です。\nヘッダをもう一度クリックして「ソートなし」に戻すと有効になります。",
                        )
                    } else {
                        sort_label.hover_tip(lead_hint)
                    };
                    self.finish_toolbar_section_lead(
                        ui,
                        sort_label,
                        TS::Sort,
                        &mut current_section_anchors,
                        &last_section_anchors,
                    );
                    match self.settings.toolbar_sort_display {
                        crate::settings::ToolbarSectionDisplay::Buttons
                        | crate::settings::ToolbarSectionDisplay::Collapsible
                        | crate::settings::ToolbarSectionDisplay::Unknown => {
                            // `add_enabled_ui` でボタン群全体を包むと、右端の残り幅だけを持つ
                            // 子 UI 内で折り返されて縦に積まれる。各ボタンを親の
                            // `horizontal_wrapped` に直接載せ、幅不足時はツールバー全体の
                            // 次行へ自然に流す。
                            for &order in &tb_sorts {
                                let selected = if book_sort_locked {
                                    order == crate::settings::SortOrder::Numeric
                                } else if self.items_are_rating_view {
                                    self.rating_view_sort
                                        == crate::rating_view::RatingViewSort::Normal(order)
                                } else {
                                    self.settings.sort_order == order
                                };
                                let resp = ui
                                    .add_enabled(
                                        !sort_disabled,
                                        egui::Button::selectable(selected, order.short_label()),
                                    )
                                    .on_hover_text(order.description());
                                if resp.clicked() && !selected {
                                    self.settings.sort_order = order;
                                    self.settings.save();
                                    if self.items_are_rating_view {
                                        self.set_rating_view_sort(
                                            crate::rating_view::RatingViewSort::Normal(order),
                                        );
                                    } else {
                                        toolbar_sort_changed = true;
                                    }
                                }
                            }
                            if self.items_are_rating_view {
                                for sort in [
                                    crate::rating_view::RatingViewSort::RatedAtDesc,
                                    crate::rating_view::RatingViewSort::RatedAtAsc,
                                ] {
                                    let selected = self.rating_view_sort == sort;
                                    let resp = ui.add_enabled(
                                        !sort_disabled,
                                        egui::Button::selectable(selected, sort.short_label()),
                                    );
                                    if resp.clicked() && !selected {
                                        self.set_rating_view_sort(sort);
                                    }
                                }
                            }
                        }
                        crate::settings::ToolbarSectionDisplay::Dropdown => {
                            ui.add_enabled_ui(!sort_disabled, |ui| {
                                let current_text = if book_sort_locked {
                                    "番号固定".to_string()
                                } else if self.items_are_rating_view {
                                    self.rating_view_sort.short_label().to_string()
                                } else {
                                    self.settings.sort_order.short_label().to_string()
                                };
                                let combo = egui::ComboBox::from_id_salt("toolbar_sort_combo")
                                    .width(100.0)
                                    .height(TOOLBAR_SORT_COMBO_HEIGHT)
                                    .selected_text(current_text)
                                    .show_ui(ui, |ui| {
                                        apply_toolbar_style(ui);
                                        for &order in &tb_sorts {
                                            let selected = if self.items_are_rating_view {
                                                self.rating_view_sort
                                                    == crate::rating_view::RatingViewSort::Normal(order)
                                            } else {
                                                self.settings.sort_order == order
                                            };
                                            let resp = ui
                                                .selectable_label(selected, order.short_label())
                                                .on_hover_text(order.description());
                                            if resp.clicked() && !selected
                                            {
                                                self.settings.sort_order = order;
                                                self.settings.save();
                                                if self.items_are_rating_view {
                                                    self.set_rating_view_sort(
                                                        crate::rating_view::RatingViewSort::Normal(order),
                                                    );
                                                } else {
                                                    toolbar_sort_changed = true;
                                                }
                                            }
                                        }
                                        if self.items_are_rating_view {
                                            ui.separator();
                                            for sort in [
                                                crate::rating_view::RatingViewSort::RatedAtDesc,
                                                crate::rating_view::RatingViewSort::RatedAtAsc,
                                            ] {
                                                let selected = self.rating_view_sort == sort;
                                                if ui
                                                    .selectable_label(selected, sort.short_label())
                                                    .clicked()
                                                    && !selected
                                                {
                                                    self.set_rating_view_sort(sort);
                                                }
                                            }
                                        }
                                    });
                                toolbar_combo_popup_open |=
                                    egui::ComboBox::is_open(ctx, combo.response.id);
                            });
                        }
                    }
                    // ソート候補を全部外したときの右クリック誘導 (Codex P3)。
                    if tb_sorts.is_empty() {
                        ui.label(egui::RichText::new("(右クリックでソートを選択)").weak());
                    }
                }
                TS::Rating => {
                    // Ctrl+G の集約ビュー (= 検索結果のフォルダ一覧) では★フィルタを
                    // 反映できない (ヒット件数と filter の二重集計が必要で実装コスト大)。
                    // ドリルイン後は file list + サブフォルダ件数の両方に反映するので
                    // enable に戻す。
                    let aggregated_search = self.global_search.active
                        && self.global_search.drill.is_none()
                        && self.global_search.aggregate;
                    let rating_view_fixed = self.items_are_rating_view;
                    // hover ヒントは disable 中の widget では拾われにくいので
                    // (egui の sense)、有効な「★:」ラベル側に乗せる。
                    let star_label = toolbar_label(ui, "★:", 24.0, drag_enabled);
                    let star_label = if rating_view_fixed {
                        star_label.hover_tip(
                            "レーティング一覧では★フィルタは対象★で固定されます。",
                        )
                    } else if aggregated_search {
                        star_label.hover_tip(
                            "検索結果のコンテナ一覧では★フィルタは適用できません。\nコンテナを開くと有効になります。",
                        )
                    } else {
                        star_label.hover_tip(lead_hint)
                    };
                    self.finish_toolbar_section_lead(
                        ui,
                        star_label,
                        TS::Rating,
                        &mut current_section_anchors,
                        &last_section_anchors,
                    );
                    // ★ボタン群を `add_enabled_ui` でまとめると、その scope が「残り幅」
                    // だけの狭い子 UI を作るので `horizontal_wrapped` の wrap が子 UI 内で
                    // 起きてしまい、★★ 以降が右端の縦帯に積まれて崩れる。enabled は各
                    // ボタン側に渡し、親の wrap に直接乗せて次の row に流させる。
                    for idx in 0..6 {
                        let (changed, assign) = draw_rating_filter_button(
                            ui,
                            &self.keymap,
                            &mut self.settings.rating_filter,
                            idx,
                            !aggregated_search && !rating_view_fixed,
                            has_rating_selection,
                        );
                        if changed {
                            toolbar_rating_changed = true;
                        }
                        match assign {
                            Some(RatingAssign::Selection(n)) => {
                                toolbar_rating_assign_selection = Some(n)
                            }
                            Some(RatingAssign::Container(n)) => {
                                toolbar_rating_assign_container = Some(n)
                            }
                            None => {}
                        }
                    }
                    // ★フィルタ一時解除中: コンテナ自身の★で開いた結果として filter が
                    // 全 ON に書き換わっている状態を示すバッジ。クリックで即復元。
                    if self.rating_filter_suppressed_at.is_some() {
                        let resp = ui
                            .small_button(
                                egui::RichText::new("★一時解除中")
                                    .color(egui::Color32::from_rgb(200, 140, 40)),
                            )
                            .hover_tip("コンテナ自身の★で開いたため一時解除中です。\n親へ戻るか、このバッジをクリックで復元。");
                        if resp.clicked() && self.restore_rating_filter_suppression() {
                            toolbar_rating_changed = true;
                        }
                    }

                    // ★固定 (Snapshot Lock) ボタン (= 設計: docs/star-lock-snapshot-design.md §4.1)。
                    // ★ボタン群の右に区切りなしで配置。snapshot 中は active 強調、
                    // inactive で検索 pending 等のときは disabled。
                    let snap_active = self.is_snapshot_active();
                    let snap_count = self.snapshot_count();
                    let disabled_reason = self.snapshot_button_disabled_reason();
                    let enabled = snap_active || disabled_reason.is_none();
                    let label_text = if let Some(n) = snap_count {
                        format!("★固定 ({n})")
                    } else {
                        "★固定".to_string()
                    };
                    let rich = if snap_active {
                        // active 時は背景強調色 (= 青系)
                        egui::RichText::new(label_text)
                            .color(egui::Color32::from_rgb(255, 255, 255))
                            .background_color(egui::Color32::from_rgb(58, 110, 165))
                            .strong()
                    } else {
                        egui::RichText::new(label_text)
                    };
                    let tooltip = if snap_active {
                        let n = snap_count.unwrap_or(0);
                        format!("★固定を解除 ({n}件)")
                    } else if let Some(r) = disabled_reason {
                        r.to_string()
                    } else {
                        "現在の絞り込み結果をスナップショットに固定\n(★/Ctrl+F/S/G の結果範囲内のみで巡回)".to_string()
                    };
                    let resp = ui
                        .add_enabled(enabled, egui::Button::new(rich).small())
                        .hover_tip(tooltip);
                    if resp.clicked() {
                        let label = self.infer_snapshot_source_label();
                        self.toggle_snapshot(label);
                    }

                }
                TS::Favorites => {
                    let lead = toolbar_label(ui, "お気に入り:", 76.0, drag_enabled).hover_tip(lead_hint);
                    self.finish_toolbar_section_lead(
                        ui,
                        lead,
                        TS::Favorites,
                        &mut current_section_anchors,
                        &last_section_anchors,
                    );
                    let fav_mode = self.settings.toolbar_favorites_display;
                    let (show_inline, new_collapsed) = toolbar_section_fold_toggle(
                        ui,
                        fav_mode,
                        self.settings.toolbar_favorites_collapsed,
                    );
                    if let Some(c) = new_collapsed {
                        self.settings.toolbar_favorites_collapsed = c;
                        self.settings.save();
                    }
                    let current = self.current_folder.clone();
                    if self.settings.favorites.is_empty() {
                        if show_inline || fav_mode == crate::settings::ToolbarSectionDisplay::Dropdown
                        {
                            ui.label(egui::RichText::new("(未登録)").weak());
                        }
                    } else if fav_mode == crate::settings::ToolbarSectionDisplay::Dropdown {
                        // プルダウン: アクションは「移動」1 つなので、選ぶだけで発動。
                        let sel_text = current
                            .as_ref()
                            .and_then(|c| {
                                self.settings
                                    .favorites
                                    .iter()
                                    .find(|f| &f.path == c)
                                    .map(|f| f.name.clone())
                            })
                            .unwrap_or_else(|| "選択".to_string());
                        // ComboBox を固定サイズ領域に包んで `horizontal_wrapped` の折返しを効かせる
                        // (右端でそのまま置くと見切れるため。toolbar_label と同じ手法)。
                        let combo = ui
                            .allocate_ui_with_layout(
                                egui::vec2(168.0, ui.spacing().interact_size.y),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    egui::ComboBox::from_id_salt("toolbar_fav_combo")
                                        .width(160.0)
                                        .selected_text(sel_text)
                                        .show_ui(ui, |ui| {
                                            apply_toolbar_style(ui);
                                            for fav in &self.settings.favorites {
                                                let selected = current
                                                    .as_ref()
                                                    .map(|c| c == &fav.path)
                                                    .unwrap_or(false);
                                                if ui
                                                    .selectable_label(selected, &fav.name)
                                                    .on_hover_text(fav.path.to_string_lossy())
                                                    .clicked()
                                                {
                                                    toolbar_fav_nav = Some(fav.path.clone());
                                                    ui.close();
                                                }
                                            }
                                        })
                                },
                            )
                            .inner;
                        toolbar_combo_popup_open |= egui::ComboBox::is_open(ctx, combo.response.id);
                    } else if show_inline {
                        for fav in &self.settings.favorites {
                            let selected =
                                current.as_ref().map(|c| c == &fav.path).unwrap_or(false);
                            if ui
                                .selectable_label(selected, &fav.name)
                                .hover_tip(fav.path.to_string_lossy())
                                .clicked()
                            {
                                toolbar_fav_nav = Some(fav.path.clone());
                            }
                        }
                    }
                }
                // タグセクション (docs/tag-feature.md §4.3)
                TS::Tags => {
                    let lead = toolbar_label(ui, "タグ:", 42.0, drag_enabled).hover_tip(lead_hint);
                    self.finish_toolbar_section_lead(
                        ui,
                        lead,
                        TS::Tags,
                        &mut current_section_anchors,
                        &last_section_anchors,
                    );
                    let has_target = self.tag_target_path_count() > 0;
                    // 「設定」「検索」はセクション全体の操作 (タグエディタを開く / タグビュー全体を開く)
                    // で、下のプルダウンの「付与」「タグビュー」(= 選択した 1 タグに対する操作) とは
                    // スコープが異なる。表示形式 (展開/折りたたみ/プルダウン) に依らず常に出して、
                    // どの表示でもタグエディタとタグビューへ 1 クリックで到達できるようにする。
                    if ui
                        .add_enabled(has_target, egui::Button::new("設定"))
                        .hover_tip("選択中の項目へタグを付ける/外す")
                        .clicked()
                    {
                        toolbar_tag_apply = true;
                    }
                    if ui
                        .button("検索")
                        .hover_tip(
                            self.keymap
                                .first_chord_action_label("タグビューを開く", KeyAction::GridTagView),
                        )
                        .clicked()
                    {
                        toolbar_tag_view_open = true;
                    }
                    let tag_mode = self.settings.toolbar_tags_display;
                    let (show_inline, new_collapsed) =
                        toolbar_section_fold_toggle(ui, tag_mode, self.settings.toolbar_tags_collapsed);
                    if let Some(c) = new_collapsed {
                        self.settings.toolbar_tags_collapsed = c;
                        self.settings.save();
                    }
                    if tag_mode == crate::settings::ToolbarSectionDisplay::Dropdown {
                        // プルダウン: コンボでタグを 1 つ選び、[付与][タグビュー] で操作 (右クリックは使わない)。
                        if toolbar_tags.is_empty() {
                            ui.label(egui::RichText::new("(ピン留めタグなし)").weak());
                        } else {
                            // 一時選択を有効なピン留めタグへ正規化 (無ければ先頭)。
                            let pick = self
                                .toolbar_tag_dropdown_pick
                                .as_ref()
                                .filter(|p| toolbar_tags.iter().any(|t| t == *p))
                                .cloned()
                                .unwrap_or_else(|| toolbar_tags[0].clone());
                            let mut new_pick: Option<String> = None;
                            let combo = ui
                                .allocate_ui_with_layout(
                                    egui::vec2(148.0, ui.spacing().interact_size.y),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        egui::ComboBox::from_id_salt("toolbar_tag_combo")
                                            .width(140.0)
                                            .selected_text(format!("#{pick}"))
                                            .show_ui(ui, |ui| {
                                                apply_toolbar_style(ui);
                                                for name in &toolbar_tags {
                                                    if ui
                                                        .selectable_label(
                                                            name == &pick,
                                                            format!("#{name}"),
                                                        )
                                                        .clicked()
                                                    {
                                                        new_pick = Some(name.clone());
                                                        ui.close();
                                                    }
                                                }
                                            })
                                    },
                                )
                                .inner;
                            toolbar_combo_popup_open |= egui::ComboBox::is_open(ctx, combo.response.id);
                            if let Some(p) = new_pick {
                                self.toolbar_tag_dropdown_pick = Some(p);
                            }
                            if ui
                                .add_enabled(has_target, egui::Button::new("付与"))
                                .hover_tip("選択したアイテムへこのタグを付与/解除")
                                .clicked()
                            {
                                toolbar_tag_click = Some(pick.clone());
                            }
                            if ui
                                .button("タグビュー")
                                .hover_tip("このタグのタグビューを開く")
                                .clicked()
                            {
                                toolbar_tag_search = Some(pick.clone());
                            }
                        }
                    } else if show_inline {
                        for name in &toolbar_tags {
                            let label = format!("#{name}");
                            // 統一ジェスチャ (toolbar-customization-plan §1.1):
                            //   左クリック = タグビューを開く / 右クリック = 選択へ付与 / Shift+右 = コンテナへ付与
                            // 左クリックは対象が無くても使えるのでボタンは常に有効にする。
                            let resp = ui.button(label).hover_tip(if has_target {
                                "左: タグビュー / 右: 選択したアイテムへ付与 / Shift+右: この場所(コンテナ)へ付与"
                            } else {
                                "左: タグビュー / Shift+右: この場所(コンテナ)へ付与 (右で選択へ付与するには項目を選ぶ)"
                            });
                            if resp.clicked() {
                                toolbar_tag_search = Some(name.clone());
                            }
                            if resp.secondary_clicked() {
                                if ui.input(|i| i.modifiers.shift) {
                                    toolbar_tag_container = Some(name.clone());
                                } else if has_target {
                                    toolbar_tag_click = Some(name.clone());
                                }
                            }
                        }
                    }
                }
                TS::Unknown => {}
                    }
                    // このセクションのアンカー矩形を「ラベル + 各種ボタン」の全体に広げる
                    // (finish_toolbar_section_lead はラベル矩形しか記録しないため)。これで
                    // ドラッグ挿入マーカーを「次セクションのラベル手前 / 末尾なら前セクションの
                    // 要素すべての後」に正しく出せる (実機フィードバック: マーカーが「ソート:」等の
                    // 文字直後に出てボタンの後ろとずれていた)。同一行のときだけ右端を伸ばす
                    // (セクション内部で折り返した場合はラベル矩形のままにして負幅を避ける)。
                    if let Some(entry) = current_section_anchors.last_mut() {
                        let lead = entry.1;
                        let cur = ui.cursor();
                        if (cur.min.y - lead.top()).abs() < lead.height() {
                            let right = cur.min.x.max(lead.right());
                            entry.1 = egui::Rect::from_min_max(
                                egui::pos2(lead.left(), lead.top()),
                                egui::pos2(right, lead.bottom()),
                            );
                        }
                    }
                    first_section = false;
                }
                });
            });
            // 次フレームの背景 interact 用に content rect を記録する。
            self.toolbar_content_rect = Some(scope_resp.response.rect);
            // 空き領域 右クリック → セクション表示 ON/OFF メニュー。「既定に戻す」は影響が
            // 大きく取り消せないので、ここ (右クリック) には出さない (show_reset=false)。
            if let Some(bg) = bg_resp {
                show_sticky_context_menu(&bg, |ui| {
                    self.draw_toolbar_visibility_menu(ui, false);
                });
            }
            ui.add_space(2.0);
        });

        // 次フレームのドラッグ並べ替え drop 計算用に、今フレームの可視セクション矩形を格納。
        self.toolbar_section_anchors = current_section_anchors;

        if toolbar_combo_popup_open {
            consume_wheel_input(ctx);
        }

        if let Some(name) = toolbar_book_target_name {
            self.settings.active_book_name = name.clone();
            self.settings.save();
            self.book_manager_rename_name = name;
        }
        if toolbar_book_add {
            self.add_grid_selection_to_active_book(ctx);
        }
        if toolbar_book_open_active {
            toolbar_fav_nav = Some(self.active_book_folder_path());
        }
        // ピン留め本: 左クリック=開く / 右クリック=選択を追加。
        if let Some(name) = toolbar_book_pin_open {
            toolbar_fav_nav = Some(crate::books::book_folder(&self.book_root_path(), &name));
        }
        if let Some(name) = toolbar_book_pin_add {
            self.add_grid_selection_to_named_book(ctx, name);
        }

        // ツールバーのソート変更は borrow の関係で遅延実行。
        // ネスト ZIP は階層維持で再ソート、Ctrl+G は検索結果再ソート (§4.3.3。実フォルダを
        // 再ロードすると Ctrl+G ビューから抜けるため)、通常フォルダは再ロード。
        if toolbar_sort_changed {
            self.apply_sort_change_reload();
        }

        // レーティングフィルタ変更: 設定を保存して visible_indices を再計算。
        // selected が filter から外れた場合の処理は `rebuild_visible_indices` が
        // 直近の visible idx にリダイレクト (旧コードは None にクリアしていた)。
        if toolbar_rating_changed {
            // ユーザーによる明示的な filter 操作 → suppression anchor を破棄する
            // (ユーザー意思を尊重して、BS しても以前の filter は復元しない)。
            self.drop_rating_filter_suppression_on_user_edit();
            self.settings.save();
            // Ctrl+G 合成ビュー (drilled / aggregated) ではバッジ件数が
            // build_drilled_items 側で rating_filter を使って再計算されるので
            // items 自体を作り直す。実体ビュー (Ctrl+G から開いた PDF/ZIP/Folder)
            // では合成 items に置き換えてしまわないよう visible_indices だけ
            // 再計算する (Codex P2)。
            if self.global_search.active && self.items_are_global_search_view {
                self.rebuild_items_from_global_search();
            } else {
                self.rebuild_visible_indices();
            }
        }
        // ★付与 (右クリックメニュー、§1.1)。apply_rating_to_selection は対象解決・undo・
        // 再描画を自前で行う。コンテナ付与は変更時のみトースト。
        if let Some(n) = toolbar_rating_assign_selection {
            self.apply_rating_to_selection(n);
        }
        if let Some(n) = toolbar_rating_assign_container {
            if self.set_current_folder_rating(n) {
                self.show_container_rating_toast(n);
            } else {
                // 合成ビュー等で実コンテナが無い場合。常に何らかのフィードバックを返す
                // (グリッドからのコンテナ付与で無反応に見えないように)。
                self.show_feedback_toast("この画面ではこの場所に評価を付けられません".to_string());
            }
        }

        // ツールバーのタグ項目クリック
        if let Some(name) = toolbar_tag_click {
            self.request_tag_toggle_for_selection(&name);
        }
        if let Some(name) = toolbar_tag_search {
            self.open_tag_view_for_tag(&name);
        }
        if let Some(name) = toolbar_tag_container {
            self.request_tag_toggle_for_current_container(&name);
        }
        if toolbar_tag_apply {
            self.open_tag_apply_dialog();
        }
        if toolbar_tag_view_open {
            self.open_tag_view();
        }

        // (旧) VST3 プラグイン管理ボタンの click handler はツールバーボタン削除に伴い撤去。

        toolbar_fav_nav
    }

    /// ツールバーの空き領域 右クリックメニュー (v2.0.0 Phase 3, §1.2 / §5)。
    /// 各セクションの表示 ON/OFF と「ツールバーを既定に戻す」を提供する。
    /// 環境設定のツールバーページに代わるカスタマイズ入口。
    /// 空き領域 右クリック / 「設定→ツールバー」共通の表示チェックリスト。
    /// `show_reset` = 「ツールバーを既定に戻す」を出すか。影響が大きい操作なので、右クリック
    /// メニュー (空き領域) では出さず、「設定→ツールバー」でのみ出す + 実行前に確認を挟む。
    fn draw_toolbar_visibility_menu(&mut self, ui: &mut egui::Ui, show_reset: bool) {
        draw_sticky_settings_menu_header(ui, "表示するセクション");
        ui.separator();
        let s = &mut self.settings;
        let mut changed = false;
        // 既定のセクション並び順 (FolderTree→Bookshelf→Cols→Aspect→Sort→Rating→Favorites→Tags) に
        // 揃えてチェックボックスを並べる。
        changed |= ui
            .checkbox(&mut s.show_toolbar_folder_tree_button, "ツリー")
            .changed();
        changed |= ui.checkbox(&mut s.show_toolbar_bookshelf, "本棚").changed();
        changed |= ui
            .checkbox(&mut s.show_toolbar_cols, "列")
            .on_hover_text("項目が無いと表示されません (セクションのラベルを右クリックで列を選択)")
            .changed();
        changed |= ui
            .checkbox(&mut s.show_toolbar_aspect, "比率")
            .on_hover_text(
                "項目が無いと表示されません (セクションのラベルを右クリックで比率を選択)",
            )
            .changed();
        changed |= ui
            .checkbox(&mut s.show_toolbar_sort, "ソート")
            .on_hover_text(
                "項目が無いと表示されません (セクションのラベルを右クリックでソートを選択)",
            )
            .changed();
        changed |= ui
            .checkbox(&mut s.show_toolbar_rating, "レーティング (★)")
            .changed();
        changed |= ui
            .checkbox(&mut s.show_toolbar_favorites, "お気に入り")
            .changed();
        changed |= ui.checkbox(&mut s.show_toolbar_tags, "タグ").changed();
        ui.separator();
        changed |= ui
            .checkbox(
                &mut s.show_toolbar_facet_filter,
                "スマートフィルタ (絞り込みバー)",
            )
            .on_hover_text("非表示にしても、適用中の絞り込み条件そのものは保持されます。")
            .changed();
        // フォルダバー (アドレス行) も「並べ替えできないだけのセクション」として扱う。
        // 他セクション同様、ここ (メニュー) では表示 ON/OFF だけ。出すボタンの細かい設定は
        // ツールバー上の操作 = アドレスバー左端「フォルダ:」の右クリックに統一する
        // (実機フィードバック: フォルダバーだけメニューから設定できるのは不揃い)。
        changed |= ui
            .checkbox(&mut s.show_toolbar_folder, "フォルダバー (アドレス行)")
            .on_hover_text("詳細設定はフォルダバー左端の「フォルダ:」を右クリック")
            .changed();

        ui.separator();
        // ドラッグ並べ替えの許可 (既定 OFF。OFF のときはカーソルも変わらない)。
        if ui
            .checkbox(
                &mut self.settings.toolbar_section_drag_enabled,
                "ドラッグで並べ替えを許可",
            )
            .on_hover_text(
                "ON にすると、セクションのラベルをドラッグして並べ替えできます。\n\
                 OFF のときはドラッグ無効で、マウスカーソルも変わりません。",
            )
            .changed()
        {
            changed = true;
            ui.close();
        }

        // 「ツールバーを既定に戻す」は影響が大きく取り消せないので、右クリック (空き領域) には
        // 出さず、「設定→ツールバー」(show_reset=true) でのみ出す。実行は確認ダイアログ経由。
        if show_reset {
            ui.separator();
            if ui
                .button("ツールバーを既定に戻す…")
                .on_hover_text(
                    "セクションの表示・並び順・表示形式・出す項目をすべて初期状態に戻します",
                )
                .clicked()
            {
                self.show_toolbar_reset_confirm = true;
                ui.close();
            }
        }

        if changed {
            self.settings.save();
            ui.ctx().request_repaint();
        }
    }

    /// 「ツールバーを既定に戻す」確認ダイアログ (v2.0.0)。`設定→ツールバー` からのみ起動。
    pub(crate) fn show_toolbar_reset_confirm_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_toolbar_reset_confirm {
            return;
        }
        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
        egui::Window::new("ツールバーを既定に戻す")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.label(
                    "ツールバーのセクションの表示・並び順・表示形式・出す項目を\nすべて初期状態に戻します。",
                );
                ui.label("カスタマイズした内容は失われます。");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("既定に戻す").clicked() {
                        self.reset_toolbar_customization();
                        self.show_toolbar_reset_confirm = false;
                        ui.ctx().request_repaint();
                    }
                    if ui.button("キャンセル").clicked() || escape_pressed {
                        self.show_toolbar_reset_confirm = false;
                    }
                });
            });
        if !open {
            self.show_toolbar_reset_confirm = false;
        }
    }

    /// ツールバーのカスタマイズ (表示・並び順・行頭・表示形式・出す項目) を既定に戻す。
    /// 空き領域 / セクション右クリックメニューの「既定に戻す」から呼ぶ。
    fn reset_toolbar_customization(&mut self) {
        use crate::settings::ToolbarSectionDisplay;
        let s = &mut self.settings;
        // 表示フラグ
        s.show_toolbar_folder_tree_button = true;
        s.show_toolbar_bookshelf = true;
        s.show_toolbar_cols = true;
        s.show_toolbar_aspect = true;
        s.show_toolbar_sort = true;
        s.show_toolbar_rating = true;
        s.show_toolbar_favorites = true;
        s.show_toolbar_tags = true;
        s.show_toolbar_facet_filter = true;
        // フォルダバー (アドレス行) もセクション扱いなので一緒に既定へ戻す (Codex P3)。
        s.show_toolbar_folder = true;
        s.show_address_bar_history_nav = true;
        s.show_address_bar_quick_folders = true;
        s.show_toolbar_parent_button = true;
        s.show_toolbar_prev_folder = true;
        s.show_toolbar_next_folder = true;
        s.show_address_bar_favorite_button = true;
        s.show_address_bar_history_menu = true;
        s.show_address_bar_folder_pin = true;
        s.show_address_bar_stack_toggle = true;
        s.show_location_drive_list = true;
        s.show_location_reading_history = true;
        s.show_location_rating = true;
        s.show_location_bookshelf = true;
        s.show_location_desktop = true;
        s.show_location_pictures = true;
        s.show_location_downloads = true;
        s.show_location_drive_roots = true;
        // 並び順 / 行頭 / ドラッグ許可
        s.toolbar_section_order = Vec::new();
        s.toolbar_section_new_row = Vec::new();
        s.toolbar_section_drag_enabled = false;
        // 表示形式 / 折りたたみ。「既定に戻す」= 新規インストールの既定に揃えるので、
        // 列 / 比率 / ソートは新規既定と同じプルダウンにする (Settings::default と一致)。
        s.toolbar_cols_display = ToolbarSectionDisplay::Dropdown;
        s.toolbar_aspect_display = ToolbarSectionDisplay::Dropdown;
        s.toolbar_sort_display = ToolbarSectionDisplay::Dropdown;
        s.toolbar_favorites_display = ToolbarSectionDisplay::default();
        s.toolbar_tags_display = ToolbarSectionDisplay::default();
        s.toolbar_bookshelf_display = ToolbarSectionDisplay::default();
        s.toolbar_favorites_collapsed = false;
        s.toolbar_tags_collapsed = false;
        s.toolbar_bookshelf_collapsed = false;
        // 出す項目
        s.toolbar_cols_items = crate::settings::default_toolbar_cols_items();
        s.toolbar_cols_details_visible = true;
        s.toolbar_aspect_items = crate::settings::default_toolbar_aspect_items();
        s.toolbar_aspect_auto_visible = crate::settings::default_toolbar_aspect_auto_visible();
        s.toolbar_sort_items = crate::settings::default_toolbar_sort_items();
        s.toolbar_facet_filter_items = crate::settings::default_toolbar_facet_filter_items();
        s.save();
    }

    /// セクションのラベル右クリックで開く「このセクションの設定」メニュー
    /// (v2.0.0 Phase 3, §1.2)。表示形式・出す項目・管理ダイアログ・非表示・既定化を提供。
    fn draw_section_settings_menu(
        &mut self,
        ui: &mut egui::Ui,
        section: crate::settings::ToolbarSectionId,
    ) {
        use crate::settings::ToolbarSectionDisplay as TD;
        use crate::settings::ToolbarSectionId as TS;

        draw_sticky_settings_menu_header(ui, toolbar_section_display_label(section));
        ui.separator();

        let mut changed = false;
        // 表示形式 (展開/折りたたみ/プルダウン) ラジオの共通描画。
        fn display_radio(ui: &mut egui::Ui, value: &mut TD, options: &[TD], changed: &mut bool) {
            ui.horizontal(|ui| {
                ui.label("表示:");
                for &opt in options {
                    *changed |= ui.radio_value(value, opt, opt.label()).changed();
                }
            });
        }

        let has_section_specific_settings =
            !matches!(section, TS::FolderTree | TS::Rating | TS::Unknown);
        match section {
            TS::FolderTree | TS::Rating => {
                // 表示形式・出す項目を持たない。非表示 / 既定化のみ (下の共通部)。
            }
            TS::Bookshelf => {
                // 本棚はコンボ(全本)が常時あり、ピンは常にボタンで出すので 展開/折りたたみ の 2 択。
                display_radio(
                    ui,
                    &mut self.settings.toolbar_bookshelf_display,
                    TD::all_collapsible_only(),
                    &mut changed,
                );
                ui.separator();
                if ui.button("本の管理…").clicked() {
                    self.show_book_manager = true;
                    ui.close();
                }
            }
            TS::Cols => {
                display_radio(
                    ui,
                    &mut self.settings.toolbar_cols_display,
                    TD::all(),
                    &mut changed,
                );
                ui.separator();
                ui.label("出す列:");
                ui.horizontal_wrapped(|ui| {
                    for cols in 1..=10usize {
                        let mut checked = self.settings.toolbar_cols_items.contains(&cols);
                        if ui.checkbox(&mut checked, format!("{cols}")).changed() {
                            if checked {
                                self.settings.toolbar_cols_items.push(cols);
                                self.settings.toolbar_cols_items.sort();
                            } else {
                                self.settings.toolbar_cols_items.retain(|&c| c != cols);
                            }
                            changed = true;
                        }
                    }
                    changed |= ui
                        .checkbox(&mut self.settings.toolbar_cols_details_visible, "詳細")
                        .changed();
                });
            }
            TS::Aspect => {
                display_radio(
                    ui,
                    &mut self.settings.toolbar_aspect_display,
                    TD::all(),
                    &mut changed,
                );
                ui.separator();
                ui.label("出す比率:");
                ui.horizontal_wrapped(|ui| {
                    changed |= ui
                        .checkbox(&mut self.settings.toolbar_aspect_auto_visible, "自動")
                        .changed();
                    for &aspect in crate::settings::ThumbAspect::all() {
                        let mut checked = self.settings.toolbar_aspect_items.contains(&aspect);
                        if ui.checkbox(&mut checked, aspect.label()).changed() {
                            if checked {
                                self.settings.toolbar_aspect_items.push(aspect);
                                let order: Vec<_> = crate::settings::ThumbAspect::all().to_vec();
                                self.settings.toolbar_aspect_items.sort_by_key(|a| {
                                    order.iter().position(|o| o == a).unwrap_or(usize::MAX)
                                });
                            } else {
                                self.settings.toolbar_aspect_items.retain(|&a| a != aspect);
                            }
                            changed = true;
                        }
                    }
                });
            }
            TS::Sort => {
                display_radio(
                    ui,
                    &mut self.settings.toolbar_sort_display,
                    TD::all(),
                    &mut changed,
                );
                ui.separator();
                ui.label("出すソート:");
                ui.horizontal_wrapped(|ui| {
                    for &order in crate::settings::SortOrder::all() {
                        let mut checked = self.settings.toolbar_sort_items.contains(&order);
                        if ui.checkbox(&mut checked, order.short_label()).changed() {
                            if checked {
                                self.settings.toolbar_sort_items.push(order);
                                let canonical: Vec<_> = crate::settings::SortOrder::all().to_vec();
                                self.settings.toolbar_sort_items.sort_by_key(|so| {
                                    canonical.iter().position(|o| o == so).unwrap_or(usize::MAX)
                                });
                            } else {
                                self.settings.toolbar_sort_items.retain(|&so| so != order);
                            }
                            changed = true;
                        }
                    }
                });
            }
            TS::Favorites => {
                display_radio(
                    ui,
                    &mut self.settings.toolbar_favorites_display,
                    TD::all_with_collapsible(),
                    &mut changed,
                );
                ui.separator();
                if ui.button("お気に入りを編集…").clicked() {
                    self.show_favorites_editor = true;
                    ui.close();
                }
            }
            TS::Tags => {
                display_radio(
                    ui,
                    &mut self.settings.toolbar_tags_display,
                    TD::all_with_collapsible(),
                    &mut changed,
                );
                ui.separator();
                if ui.button("タグを管理…").clicked() {
                    self.show_tag_editor = true;
                    ui.close();
                }
            }
            TS::Unknown => {}
        }

        if has_section_specific_settings {
            ui.separator();
        }
        // 行頭に表示 (= このセクションの前で必ず改行)。
        let mut new_row = self.settings.toolbar_section_new_row.contains(&section);
        if ui
            .checkbox(&mut new_row, "行頭に表示")
            .on_hover_text("このセクションの前で必ず改行します (自動折返しに優先)")
            .changed()
        {
            if new_row {
                if !self.settings.toolbar_section_new_row.contains(&section) {
                    self.settings.toolbar_section_new_row.push(section);
                }
            } else {
                self.settings
                    .toolbar_section_new_row
                    .retain(|&s| s != section);
            }
            changed = true;
            ui.close();
        }
        // ドラッグ並べ替えの許可 (全セクション共通のグローバル設定。既定 OFF)。
        if ui
            .checkbox(
                &mut self.settings.toolbar_section_drag_enabled,
                "ドラッグで並べ替えを許可",
            )
            .on_hover_text(
                "ON にすると、セクションのラベルをドラッグして並べ替えできます。\n\
                 OFF のときはドラッグ無効で、マウスカーソルも変わりません。",
            )
            .changed()
        {
            changed = true;
            ui.close();
        }

        ui.separator();
        if ui
            .button("このセクションを隠す")
            .on_hover_text("再表示はツールバーの空き領域を右クリック")
            .clicked()
        {
            set_toolbar_section_visible(&mut self.settings, section, false);
            self.settings.save();
            ui.ctx().request_repaint();
            ui.close();
            return;
        }
        // 「ツールバーを既定に戻す」は影響が大きいので、ここ (項目右クリック) には出さない。
        // 「設定→ツールバー」からのみ、確認ダイアログを経て実行する (実機フィードバック)。

        if changed {
            self.settings.save();
            ui.ctx().request_repaint();
        }
    }

    /// フォルダバー (アドレス行) の設定メニュー (v2.0.0 Phase 3, 実機フィードバック 2026-06-20)。
    /// アドレスバー左端の「フォルダ:」ラベル右クリック、および「設定」メニュー → ツールバー →
    /// フォルダバーの設定 から開く。フォルダバーは並べ替えできないだけのセクション扱い。
    fn draw_folder_bar_settings_menu(&mut self, ui: &mut egui::Ui) {
        draw_sticky_settings_menu_header(ui, "フォルダバー");
        ui.separator();
        let mut changed = false;
        changed |= ui
            .checkbox(
                &mut self.settings.show_address_bar_history_nav,
                "履歴の戻る/進む (←/→)",
            )
            .changed();
        changed |= ui
            .checkbox(
                &mut self.settings.show_address_bar_quick_folders,
                "A/B クイックフォルダ",
            )
            .changed();
        changed |= ui
            .checkbox(
                &mut self.settings.show_toolbar_parent_button,
                "親フォルダ (⬆)",
            )
            .changed();
        let mut show_tree_nav =
            self.settings.show_toolbar_prev_folder || self.settings.show_toolbar_next_folder;
        if ui
            .checkbox(&mut show_tree_nav, "ツリー順の前/次フォルダ (▲/▼)")
            .on_hover_text("Ctrl+↑/↓ と同じく、深さ優先のツリー順で前後のフォルダへ移動します。")
            .changed()
        {
            self.settings.show_toolbar_prev_folder = show_tree_nav;
            self.settings.show_toolbar_next_folder = show_tree_nav;
            changed = true;
        }
        changed |= ui
            .checkbox(
                &mut self.settings.show_address_bar_favorite_button,
                "お気に入り追加/設定 (♡/♥)",
            )
            .changed();
        changed |= ui
            .checkbox(
                &mut self.settings.show_address_bar_history_menu,
                "最近開いたフォルダ履歴メニュー",
            )
            .changed();
        changed |= ui
            .checkbox(
                &mut self.settings.show_address_bar_folder_pin,
                "代表サムネ固定 (📌)",
            )
            .changed();
        changed |= ui
            .checkbox(
                &mut self.settings.show_address_bar_stack_toggle,
                "スタック表示トグル",
            )
            .on_hover_text("似たファイルを自動で分類して 1 つに畳んで表示するトグルボタン")
            .changed();

        ui.separator();
        ui.label("場所▼に出す項目:");
        changed |= ui
            .checkbox(&mut self.settings.show_location_drive_list, "ドライブ一覧")
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.show_location_reading_history, "読書履歴")
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.show_location_bookshelf, "本棚フォルダ")
            .changed();
        changed |= ui
            .checkbox(
                &mut self.settings.show_location_rating,
                "レーティングフォルダ",
            )
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.show_location_desktop, "デスクトップ")
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.show_location_pictures, "ピクチャ")
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.show_location_downloads, "ダウンロード")
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.show_location_drive_roots, "各ドライブ")
            .changed();

        ui.separator();
        let mut clear_recent = false;
        let mut clear_quick = false;
        if ui.button("最近開いたフォルダ履歴をクリア").clicked() {
            clear_recent = true;
            ui.close();
        }
        if ui.button("A/B の記憶した場所をクリア").clicked() {
            clear_quick = true;
            ui.close();
        }

        ui.separator();
        if ui
            .button("フォルダバーを隠す")
            .on_hover_text("再表示は「設定」メニュー → ツールバー → フォルダバー")
            .clicked()
        {
            self.settings.show_toolbar_folder = false;
            changed = true;
            ui.close();
        }

        if changed {
            self.settings.save();
            ui.ctx().request_repaint();
        }
        // クリア系は settings 以外 (session 状態) も触るので、専用メソッド + save を呼ぶ。
        if clear_recent {
            self.clear_recent_folders();
            self.settings.save();
        }
        if clear_quick {
            self.clear_quick_folder_slots();
            self.settings.save();
        }
    }

    /// セクションの leading widget (ラベル / ツリーボタン) 共通の後処理 (v2.0.0 Phase 3):
    /// (1) 今フレームの矩形を anchors に記録、(2) ドラッグ並べ替えを処理、
    /// (3) 右クリックでセクション設定メニューを開く。`resp` は消費する。
    fn finish_toolbar_section_lead(
        &mut self,
        ui: &egui::Ui,
        resp: egui::Response,
        section: crate::settings::ToolbarSectionId,
        current_anchors: &mut Vec<(crate::settings::ToolbarSectionId, egui::Rect)>,
        last_anchors: &[(crate::settings::ToolbarSectionId, egui::Rect)],
    ) {
        current_anchors.push((section, resp.rect));
        self.handle_toolbar_section_drag(ui, &resp, section, last_anchors);
        show_sticky_context_menu(&resp, |ui| {
            self.draw_section_settings_menu(ui, section);
        });
    }

    /// セクションラベルのドラッグによる並べ替えを処理する (詳細ヘッダーのドラッグと同型)。
    /// drop 位置は前フレームの全可視セクション矩形 (`last_anchors`) から計算する
    /// (drag_stopped 時点では後続セクションをまだ今フレームで描いていないため)。
    fn handle_toolbar_section_drag(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        section: crate::settings::ToolbarSectionId,
        last_anchors: &[(crate::settings::ToolbarSectionId, egui::Rect)],
    ) {
        // ドラッグ並べ替えが許可されていなければ、カーソル変更も並べ替えもしない
        // (実機フィードバック: 通常操作中にカーソルが頻繁に変わるのが煩わしい)。
        // ラベルの sense も click のみになっているので drag 系イベントは元々飛ばないが、
        // hover カーソルだけはここで明示的に抑止する必要がある。
        if !self.settings.toolbar_section_drag_enabled {
            return;
        }
        let drag_id = egui::Id::new("toolbar_section_drag_state");
        if resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            // ドロップ先に I 字マーカーを表示してどこへ入るか分かるようにする。
            if let Some(pointer) = ui.ctx().input(|i| i.pointer.interact_pos()) {
                draw_toolbar_drop_indicator(ui, last_anchors, pointer);
            }
        } else if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(pos) = ui
                .ctx()
                .input(|i| i.pointer.interact_pos().or_else(|| i.pointer.latest_pos()))
        {
            let start = ui.ctx().input(|i| i.pointer.press_origin().unwrap_or(pos));
            // egui の temp data は remove 時に Default を要求するので Option で包む
            // (詳細ヘッダーの DetailsHeaderDrag と同型)。
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    drag_id,
                    Some(ToolbarSectionDrag {
                        section,
                        start,
                        latest: pos,
                    }),
                )
            });
        }
        if resp.dragged_by(egui::PointerButton::Primary)
            && let Some(pos) = ui
                .ctx()
                .input(|i| i.pointer.interact_pos().or_else(|| i.pointer.latest_pos()))
        {
            ui.ctx().data_mut(|d| {
                if let Some(mut drag) = d.get_temp::<Option<ToolbarSectionDrag>>(drag_id).flatten()
                    && drag.section == section
                {
                    drag.latest = pos;
                    d.insert_temp(drag_id, Some(drag));
                }
            });
        }
        if resp.drag_stopped_by(egui::PointerButton::Primary) {
            let drag = ui
                .ctx()
                .data_mut(|d| d.remove_temp::<Option<ToolbarSectionDrag>>(drag_id))
                .flatten();
            if let Some(mut drag) = drag {
                if let Some(pos) = ui.ctx().input(|i| i.pointer.latest_pos()) {
                    drag.latest = pos;
                }
                // 誤ドラッグ防止: 移動量が小さい場合は並べ替えない (= ただのクリック扱い)。
                if drag.section == section && (drag.latest - drag.start).length() >= 6.0 {
                    let vis_idx = toolbar_drop_index(last_anchors, drag.latest);
                    let before = last_anchors.get(vis_idx).map(|(s, _)| *s);
                    let last_visible = last_anchors.last().map(|(s, _)| *s);
                    let current = crate::settings::ToolbarSectionId::ordered_with_fallback(
                        &self.settings.toolbar_section_order,
                    );
                    if let Some(new_order) =
                        reorder_toolbar_section(&current, drag.section, before, last_visible)
                    {
                        crate::logger::log(format!(
                            "toolbar section reorder: {:?} -> idx {vis_idx}",
                            drag.section
                        ));
                        self.settings.toolbar_section_order = new_order;
                        self.settings.save();
                        ui.ctx().request_repaint();
                    }
                }
            }
        }
    }

    // ── スマートフィルタバー ────────────────────────────────────────

    pub(crate) fn render_facet_filter_bar(&mut self, ctx: &egui::Context) {
        self.color_filter.input_has_focus = false;
        if !self.settings.show_toolbar_facet_filter {
            return;
        }
        if self.items.is_empty() || self.items_are_drive_list || self.items_are_reading_history_view
        {
            return;
        }

        let mut facet_changed = false;
        let mut place_changed = false;
        let mut non_place_facet_changed = false;
        let mut rating_changed = false;
        let mut color_changed = false;
        egui::TopBottomPanel::top("facet_filter_bar").show(ctx, |ui| {
            ui.add_space(1.0);
            ui.horizontal_wrapped(|ui| {
                let filter_label = egui::Label::new(egui::RichText::new("絞り込み:").small())
                    .sense(egui::Sense::click());
                let filter_label_response =
                    ui.add(filter_label).on_hover_text("右クリック: 絞り込みバーの設定");
                show_sticky_context_menu(&filter_label_response, |ui| {
                    self.draw_facet_filter_bar_settings_menu(ui);
                });
                let mut facet_items = crate::settings::ToolbarFacetFilterItem::visible_order(
                    &self.settings.toolbar_facet_filter_items,
                );
                {
                    use crate::settings::ToolbarFacetFilterItem as FI;
                    let should_force_place =
                        self.items_are_rating_view || !self.settings.facet_filter.place_keys.is_empty();
                    if should_force_place && !facet_items.is_empty() && !facet_items.contains(&FI::Place)
                    {
                        if let Some(ext_idx) = facet_items.iter().position(|item| *item == FI::Ext) {
                            facet_items.insert(ext_idx + 1, FI::Place);
                        } else {
                            facet_items.push(FI::Place);
                        }
                    }
                }
                if facet_items.is_empty() {
                    ui.label(egui::RichText::new("(右クリックでボタンを選択)").weak());
                }
                for item in facet_items {
                    use crate::settings::ToolbarFacetFilterItem as FI;
                    match item {
                        FI::Kind => {
                            let changed = self.draw_facet_kind_menu(ui);
                            non_place_facet_changed |= changed;
                            facet_changed |= changed;
                        }
                        FI::Ext => {
                            let changed = self.draw_facet_ext_menu(ui);
                            non_place_facet_changed |= changed;
                            facet_changed |= changed;
                        }
                        FI::Place => {
                            let changed = self.draw_facet_place_menu(ui);
                            place_changed |= changed;
                            facet_changed |= changed;
                        }
                        FI::AiModel => {
                            let changed = self.draw_facet_ai_model_menu(ui);
                            non_place_facet_changed |= changed;
                            facet_changed |= changed;
                        }
                        FI::AiTool => {
                            let changed = self.draw_facet_ai_tool_menu(ui);
                            non_place_facet_changed |= changed;
                            facet_changed |= changed;
                        }
                        FI::Rating => rating_changed |= self.draw_facet_rating_menu(ui),
                        FI::Tags => {
                            let changed = self.draw_facet_tag_menu(ui);
                            non_place_facet_changed |= changed;
                            facet_changed |= changed;
                        }
                        FI::Date => {
                            let changed = self.draw_facet_date_menu(ui);
                            non_place_facet_changed |= changed;
                            facet_changed |= changed;
                        }
                        FI::Size => {
                            let changed = self.draw_facet_size_menu(ui);
                            non_place_facet_changed |= changed;
                            facet_changed |= changed;
                        }
                        FI::Edit => {
                            let changed = self.draw_facet_edit_menu(ui);
                            non_place_facet_changed |= changed;
                            facet_changed |= changed;
                        }
                        FI::Color => {
                            if self.color_filter_available_in_current_view() {
                                color_changed |= self.draw_facet_color_menu(ui);
                            }
                        }
                        FI::Unknown => {}
                    }
                }

                if self.facet_filter_suppressed() {
                    let resp = ui
                        .small_button(
                            egui::RichText::new("親絞り込み退避中")
                                .color(egui::Color32::from_rgb(60, 130, 190)),
                        )
                        .hover_tip(
                            "親階層の絞り込み条件を退避中です。\n内側では新しい絞り込みを設定できます。\n親へ戻るか、このバッジをクリックで復元。",
                        );
                    if resp.clicked() && self.restore_facet_filter_suppression() {
                        facet_changed = true;
                    }
                }

                let rating_filter_visible = self.rating_filter_active() && !self.items_are_rating_view;
                if self.facet_filter_active() || rating_filter_visible || self.color_filter.enabled {
                    ui.separator();
                    self.draw_facet_active_chips(ui);
                    self.draw_color_filter_active_chip(ui);
                    if ui.small_button("全解除").clicked() {
                        if self.facet_filter_active() {
                            self.settings.facet_filter.clear();
                            facet_changed = true;
                        }
                        if rating_filter_visible {
                            self.settings.rating_filter = crate::settings::default_rating_filter();
                            rating_changed = true;
                        }
                        if self.color_filter.enabled {
                            self.color_filter.clear_filter();
                            color_changed = true;
                        }
                    }
                }
                ui.separator();
                ui.label(
                    egui::RichText::new(filtered_count_label(&self.items, &self.visible_indices))
                        .small(),
                );
            });
            ui.add_space(1.0);
        });

        if rating_changed {
            self.drop_rating_filter_suppression_on_user_edit();
        }
        let color_scope_changed = (facet_changed || rating_changed) && self.color_filter.enabled;
        if color_scope_changed {
            self.color_filter.applied_scope_signature = None;
        }
        if facet_changed || rating_changed {
            let preserve_place_counts =
                place_changed && !non_place_facet_changed && !rating_changed;
            let place_counts_cache = preserve_place_counts
                .then(|| self.facet_place_counts_cache.clone())
                .flatten();
            self.settings.save();
            if rating_changed && self.global_search.active && self.items_are_global_search_view {
                self.rebuild_items_from_global_search();
            } else {
                self.rebuild_visible_indices();
            }
            if preserve_place_counts {
                self.facet_place_counts_cache = place_counts_cache;
            }
        }
        if color_changed || color_scope_changed {
            if self.color_filter.enabled {
                self.ensure_color_scan_for_current_scope(ctx);
            } else {
                self.rebuild_visible_indices();
            }
        }
    }

    pub(crate) fn render_details_lazy_status_bar(&mut self, ctx: &egui::Context) {
        if self.settings.grid_view_mode != GridViewMode::Details
            || !(self.settings.details_show_created
                || self.settings.details_show_image_dimensions
                || self.settings.details_show_video_duration
                || self.settings.details_show_video_dimensions
                || self.settings.details_show_video_codec)
            || self.items.is_empty()
        {
            return;
        }

        let show = match self.details_image_dims_state {
            LazyColumnState::Loading { .. }
            | LazyColumnState::NotRequested
            | LazyColumnState::Cancelled => true,
            LazyColumnState::Ready { failed } => failed > 0,
            LazyColumnState::Disabled => false,
        };
        if !show {
            return;
        }

        egui::TopBottomPanel::top("details_lazy_status_bar")
            .exact_height(25.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    match self.details_image_dims_state {
                        LazyColumnState::NotRequested => {
                            ui.label("詳細情報を読み込み準備中");
                        }
                        LazyColumnState::Loading { done, total } => {
                            ui.label(format!(
                                "詳細情報を読み込み中 {}/{}   遅延列 {}/{}",
                                done, total, done, total
                            ));
                            if ui.small_button("停止").clicked() {
                                self.cancel_details_meta_loading();
                            }
                        }
                        LazyColumnState::Cancelled => {
                            ui.label("詳細情報の読み込みを停止中");
                            if ui.small_button("再開").clicked() {
                                self.resume_details_meta_loading();
                                ctx.request_repaint();
                            }
                        }
                        LazyColumnState::Ready { failed } => {
                            if failed > 0 {
                                facet_chip(ui, format!("詳細情報: {failed}件取得失敗"));
                            }
                        }
                        LazyColumnState::Disabled => {}
                    }
                });
            });
    }

    fn draw_facet_filter_bar_settings_menu(&mut self, ui: &mut egui::Ui) {
        use crate::settings::ToolbarFacetFilterItem as FI;

        draw_sticky_settings_menu_header(ui, "絞り込みバー");
        ui.separator();
        ui.label("表示するボタン:");
        let mut changed = false;
        for &item in FI::all() {
            let mut checked = self.settings.toolbar_facet_filter_items.contains(&item);
            if ui.checkbox(&mut checked, item.label()).changed() {
                if checked {
                    self.settings.toolbar_facet_filter_items.push(item);
                    FI::sort_like_default(&mut self.settings.toolbar_facet_filter_items);
                } else {
                    self.settings
                        .toolbar_facet_filter_items
                        .retain(|&candidate| candidate != item);
                }
                self.settings.toolbar_facet_filter_items =
                    FI::visible_order(&self.settings.toolbar_facet_filter_items);
                changed = true;
            }
        }

        ui.separator();
        if ui
            .button("絞り込みバーを隠す")
            .on_hover_text(
                "再表示はツールバーの空き領域を右クリック、または設定メニュー → ツールバー",
            )
            .clicked()
        {
            self.settings.show_toolbar_facet_filter = false;
            changed = true;
            ui.close();
        }

        if changed {
            self.settings.save();
            ui.ctx().request_repaint();
        }
    }

    fn draw_facet_kind_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let label = facet_menu_label("種類", self.settings.facet_filter.kinds.len());
        let (menu_response, _) = egui::containers::menu::MenuButton::new(label)
            .config(sticky_facet_menu_config())
            .ui(ui, |ui| {
                prepare_facet_menu_popup(ui);
                let mut counts = self.facet_kind_counts();
                for kind in &self.settings.facet_filter.kinds {
                    counts.entry(*kind).or_insert(0);
                }
                if self.settings.facet_filter.kinds.is_empty() {
                    ui.label("すべて");
                } else if ui.small_button("種類フィルタを解除").clicked() {
                    self.settings.facet_filter.kinds.clear();
                    changed = true;
                    ui.close();
                }
                ui.separator();
                if counts.is_empty() {
                    ui.label("候補なし");
                }
                for (kind, count) in counts {
                    let mut selected = self.settings.facet_filter.kinds.contains(&kind);
                    let text = format!("{} ({count})", kind.label());
                    if ui.checkbox(&mut selected, text).changed() {
                        if selected {
                            self.settings.facet_filter.kinds.insert(kind);
                        } else {
                            self.settings.facet_filter.kinds.remove(&kind);
                        }
                        changed = true;
                    }
                }
            });
        suppress_menu_button_wheel_passthrough(ui.ctx(), &menu_response);
        changed
    }

    fn draw_facet_ext_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let label = facet_menu_label("拡張子", self.settings.facet_filter.exts.len());
        let (menu_response, _) = egui::containers::menu::MenuButton::new(label)
            .config(sticky_facet_menu_config())
            .ui(ui, |ui| {
                prepare_facet_menu_popup(ui);
                let mut counts = self.facet_ext_counts();
                for ext in &self.settings.facet_filter.exts {
                    counts.entry(ext.clone()).or_insert(0);
                }
                if self.settings.facet_filter.exts.is_empty() {
                    ui.label("すべて");
                } else if ui.small_button("拡張子フィルタを解除").clicked() {
                    self.settings.facet_filter.exts.clear();
                    changed = true;
                    ui.close();
                }
                ui.separator();
                if counts.is_empty() {
                    ui.label("候補なし");
                }
                for (ext, count) in counts {
                    let mut selected = self.settings.facet_filter.exts.contains(&ext);
                    let text = format!(".{} ({count})", ext);
                    if ui.checkbox(&mut selected, text).changed() {
                        if selected {
                            self.settings.facet_filter.exts.insert(ext);
                        } else {
                            self.settings.facet_filter.exts.remove(&ext);
                        }
                        changed = true;
                    }
                }
            });
        suppress_menu_button_wheel_passthrough(ui.ctx(), &menu_response);
        changed
    }

    fn draw_facet_place_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let label = facet_menu_label("場所", self.settings.facet_filter.place_keys.len());
        let (menu_response, _) = egui::containers::menu::MenuButton::new(label)
            .config(sticky_facet_menu_config())
            .ui(ui, |ui| {
                prepare_place_facet_menu_popup(ui);
                draw_sticky_settings_menu_header(ui, "場所");
                ui.separator();
                let mut counts = self.facet_place_counts();
                for key in &self.settings.facet_filter.place_keys {
                    let label = self.facet_place_label_for_key(key);
                    counts.entry(key.clone()).or_insert((label, 0));
                }
                if self.settings.facet_filter.place_keys.is_empty() {
                    ui.label("すべて");
                } else if ui.small_button("場所フィルタを解除").clicked() {
                    self.settings.facet_filter.place_keys.clear();
                    changed = true;
                    ui.close();
                }
                ui.separator();
                if counts.is_empty() {
                    ui.label("候補なし");
                } else {
                    let counts: Vec<_> = counts.into_iter().collect();
                    let choice_count = counts.len();
                    let selected_count = self.settings.facet_filter.place_keys.len();
                    let render_t0 = crate::perf::is_enabled().then(std::time::Instant::now);
                    show_virtualized_facet_choice_rows(
                        ui,
                        PLACE_FACET_MENU_WIDTH,
                        choice_count,
                        |ui, idx| {
                            let (key, (place_label, count)) = &counts[idx];
                            let mut selected = self.settings.facet_filter.place_keys.contains(key);
                            let text = format!("{place_label} ({count})");
                            if draw_facet_checkbox_choice(ui, &mut selected, text, key.as_str()) {
                                if selected {
                                    self.settings.facet_filter.place_keys.insert(key.clone());
                                } else {
                                    self.settings.facet_filter.place_keys.remove(key);
                                }
                                changed = true;
                            }
                        },
                    );
                    if let Some(t0) = render_t0 {
                        crate::perf::event(
                            "ui",
                            "facet_place_menu_render",
                            None,
                            self.input_seq,
                            &[
                                (
                                    "ms",
                                    serde_json::Value::from(t0.elapsed().as_secs_f64() * 1000.0),
                                ),
                                ("choices", serde_json::Value::from(choice_count)),
                                ("selected", serde_json::Value::from(selected_count)),
                            ],
                        );
                    }
                }
            });
        suppress_menu_button_wheel_passthrough(ui.ctx(), &menu_response);
        changed
    }

    fn draw_facet_ai_model_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let label = facet_menu_label("AIモデル", self.settings.facet_filter.ai_models.len());
        let (menu_response, _) = egui::containers::menu::MenuButton::new(label)
            .config(sticky_facet_menu_config())
            .ui(ui, |ui| {
                prepare_ai_facet_menu_popup(ui);
                self.request_ai_model_facet_load();
                ui.ctx().request_repaint();
                if !self.details_lazy_sort_ready() {
                    self.draw_ai_facet_loading_menu(ui);
                    return;
                }
                let mut counts = self.facet_ai_model_counts();
                for model in &self.settings.facet_filter.ai_models {
                    counts.entry(model.clone()).or_insert(0);
                }
                if self.settings.facet_filter.ai_models.is_empty() {
                    ui.label("すべて");
                } else if ui.small_button("AIモデルフィルタを解除").clicked() {
                    self.settings.facet_filter.ai_models.clear();
                    changed = true;
                    ui.close();
                }
                ui.separator();
                if counts.is_empty() {
                    ui.label("候補なし");
                } else {
                    show_ai_facet_choices(ui, counts.len(), |ui| {
                        for (model, count) in counts {
                            let mut selected =
                                self.settings.facet_filter.ai_models.contains(&model);
                            let text = format!("{model} ({count})");
                            if ui.checkbox(&mut selected, text).changed() {
                                if selected {
                                    self.settings.facet_filter.ai_models.insert(model);
                                } else {
                                    self.settings.facet_filter.ai_models.remove(&model);
                                }
                                changed = true;
                            }
                        }
                    });
                }
            });
        suppress_menu_button_wheel_passthrough(ui.ctx(), &menu_response);
        changed
    }

    fn draw_facet_ai_tool_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let label = facet_menu_label("生成ツール", self.settings.facet_filter.ai_tools.len());
        let (menu_response, _) = egui::containers::menu::MenuButton::new(label)
            .config(sticky_facet_menu_config())
            .ui(ui, |ui| {
                prepare_ai_facet_menu_popup(ui);
                self.request_ai_model_facet_load();
                ui.ctx().request_repaint();
                if !self.details_lazy_sort_ready() {
                    self.draw_ai_facet_loading_menu(ui);
                    return;
                }
                let mut counts = self.facet_ai_tool_counts();
                for tool in &self.settings.facet_filter.ai_tools {
                    counts.entry(tool.clone()).or_insert(0);
                }
                if self.settings.facet_filter.ai_tools.is_empty() {
                    ui.label("すべて");
                } else if ui.small_button("生成ツールフィルタを解除").clicked() {
                    self.settings.facet_filter.ai_tools.clear();
                    changed = true;
                    ui.close();
                }
                ui.separator();
                if counts.is_empty() {
                    ui.label("候補なし");
                } else {
                    show_ai_facet_choices(ui, counts.len(), |ui| {
                        for (tool, count) in counts {
                            let mut selected = self.settings.facet_filter.ai_tools.contains(&tool);
                            let text = format!("{tool} ({count})");
                            if ui.checkbox(&mut selected, text).changed() {
                                if selected {
                                    self.settings.facet_filter.ai_tools.insert(tool);
                                } else {
                                    self.settings.facet_filter.ai_tools.remove(&tool);
                                }
                                changed = true;
                            }
                        }
                    });
                }
            });
        suppress_menu_button_wheel_passthrough(ui.ctx(), &menu_response);
        changed
    }

    fn draw_ai_facet_loading_menu(&mut self, ui: &mut egui::Ui) {
        match self.details_image_dims_state {
            LazyColumnState::NotRequested => {
                ui.label("AIメタデータを読み込み準備中");
            }
            LazyColumnState::Loading { done, total } => {
                ui.label(format!("AIメタデータを読み込み中 {done}/{total}"));
                if ui.small_button("停止").clicked() {
                    self.cancel_details_meta_loading();
                }
            }
            LazyColumnState::Cancelled => {
                ui.label("AIメタデータの読み込みを停止中");
                if ui.small_button("再開").clicked() {
                    self.resume_details_meta_loading();
                }
            }
            LazyColumnState::Disabled | LazyColumnState::Ready { .. } => {
                ui.label("AIメタデータを読み込み中");
            }
        }
    }

    fn draw_facet_rating_menu(&mut self, ui: &mut egui::Ui) -> bool {
        if self.items_are_rating_view {
            ui.add_enabled(false, egui::Button::new("★ (固定)"))
                .on_hover_text("レーティング一覧では★フィルタは対象★で固定されます。");
            return false;
        }
        let active = if self.rating_filter_active() { 1 } else { 0 };
        let mut changed = false;
        let (menu_response, _) =
            egui::containers::menu::MenuButton::new(facet_menu_label("★", active))
                .config(sticky_facet_menu_config())
                .ui(ui, |ui| {
                    prepare_facet_menu_popup(ui);
                    if ui.small_button("すべて表示").clicked() {
                        self.settings.rating_filter = crate::settings::default_rating_filter();
                        changed = true;
                        ui.close();
                    }
                    ui.separator();
                    let counts = self.facet_rating_counts();
                    for idx in 0..6 {
                        let mut selected = self.settings.rating_filter[idx];
                        let text = format!("{} ({})", rating_button_label(idx), counts[idx]);
                        if ui
                            .checkbox(&mut selected, text)
                            .on_hover_text(rating_tooltip(&self.keymap, idx))
                            .changed()
                        {
                            self.settings.rating_filter[idx] = selected;
                            changed = true;
                        }
                    }
                });
        suppress_menu_button_wheel_passthrough(ui.ctx(), &menu_response);
        changed
    }

    fn draw_facet_tag_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let active = self.settings.facet_filter.tags.len()
            + usize::from(self.settings.facet_filter.include_untagged);
        let mut changed = false;
        let (menu_response, _) =
            egui::containers::menu::MenuButton::new(facet_menu_label("タグ", active))
                .config(sticky_facet_menu_config())
                .ui(ui, |ui| {
                    prepare_facet_menu_popup(ui);
                    ui.set_width(TAG_FACET_MENU_WIDTH);
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    draw_sticky_settings_menu_header(ui, "タグ");
                    ui.separator();
                    let (mut counts, untagged_count) = self.facet_tag_counts();
                    let display_names: std::collections::BTreeMap<String, String> = self
                        .settings
                        .tags
                        .iter()
                        .map(|tag| (tag.tag_key.clone(), tag.name.clone()))
                        .collect();
                    for tag in &self.settings.facet_filter.tags {
                        counts.entry(tag.clone()).or_insert(0);
                    }
                    let query_key = crate::tags_db::normalize_tag_key(&self.facet_tag_search_query);
                    let mut choices: BTreeMap<String, (String, usize)> = BTreeMap::new();
                    for (tag_key, count) in &counts {
                        let display = display_names
                            .get(tag_key)
                            .cloned()
                            .unwrap_or_else(|| tag_key.clone());
                        choices.insert(tag_key.clone(), (display, *count));
                    }
                    if !query_key.is_empty() {
                        // find_by_prefix は SQLite の GROUP BY + LIKE。menu closure は毎フレーム
                        // 実行されるので、正規化クエリが変わったときだけ引き直す
                        // (tag_apply の cached_tag_apply_suggestions と同じパターン)。
                        let cache_stale = self
                            .facet_tag_suggestion_cache
                            .as_ref()
                            .is_none_or(|(key, _)| *key != query_key);
                        if cache_stale {
                            let summaries = self
                                .tags_db
                                .as_ref()
                                .map(|db| db.find_by_prefix(&self.facet_tag_search_query, 50))
                                .unwrap_or_default();
                            self.facet_tag_suggestion_cache = Some((query_key.clone(), summaries));
                        }
                        if let Some((_, summaries)) = self.facet_tag_suggestion_cache.as_ref() {
                            for summary in summaries {
                                let display = display_names
                                    .get(&summary.tag_key)
                                    .cloned()
                                    .unwrap_or_else(|| summary.tag.clone());
                                let count = counts.get(&summary.tag_key).copied().unwrap_or(0);
                                choices
                                    .entry(summary.tag_key.clone())
                                    .or_insert((display, count));
                            }
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.label("一致:");
                        let mut mode = self.settings.facet_filter.tag_mode;
                        if ui
                            .selectable_value(
                                &mut mode,
                                FacetTagMode::Any,
                                FacetTagMode::Any.label(),
                            )
                            .changed()
                        {
                            self.settings.facet_filter.tag_mode = mode;
                            changed = true;
                        }
                        if ui
                            .selectable_value(
                                &mut mode,
                                FacetTagMode::All,
                                FacetTagMode::All.label(),
                            )
                            .changed()
                        {
                            self.settings.facet_filter.tag_mode = mode;
                            changed = true;
                        }
                    });
                    if self.settings.facet_filter.tags.is_empty()
                        && !self.settings.facet_filter.include_untagged
                    {
                        ui.label("すべて");
                    } else if ui.small_button("タグフィルタを解除").clicked() {
                        self.settings.facet_filter.tags.clear();
                        self.settings.facet_filter.include_untagged = false;
                        changed = true;
                        ui.close();
                    }
                    ui.separator();
                    let mut include_untagged = self.settings.facet_filter.include_untagged;
                    if ui
                        .checkbox(
                            &mut include_untagged,
                            format!("タグなし ({untagged_count})"),
                        )
                        .changed()
                    {
                        self.settings.facet_filter.include_untagged = include_untagged;
                        changed = true;
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("検索:");
                        let mut output =
                            egui::TextEdit::singleline(&mut self.facet_tag_search_query)
                                .hint_text("#タグ")
                                .desired_width(150.0)
                                .min_size(egui::vec2(150.0, 20.0))
                                .show(ui);
                        let _ = crate::ui_helpers::singleline_text_edit_context_menu(
                            ui,
                            &mut output,
                            &mut self.facet_tag_search_query,
                        );
                    });
                    ui.separator();
                    if choices.is_empty() {
                        ui.label("タグ候補なし");
                    } else {
                        let choice_count = choices.len();
                        show_scrollable_facet_choices(
                            ui,
                            TAG_FACET_MENU_WIDTH,
                            choice_count,
                            |ui| {
                                for (tag_key, (display, count)) in choices {
                                    let mut selected =
                                        self.settings.facet_filter.tags.contains(&tag_key);
                                    let text = format!("#{} ({count})", display);
                                    if draw_facet_checkbox_choice(
                                        ui,
                                        &mut selected,
                                        text,
                                        &format!("#{display}"),
                                    ) {
                                        if selected {
                                            self.settings.facet_filter.tags.insert(tag_key);
                                        } else {
                                            self.settings.facet_filter.tags.remove(&tag_key);
                                        }
                                        changed = true;
                                    }
                                }
                            },
                        );
                    }
                });
        suppress_menu_button_wheel_passthrough(ui.ctx(), &menu_response);
        changed
    }

    fn draw_facet_date_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let active = usize::from(self.settings.facet_filter.date_preset.is_some());
        let mut changed = false;
        let menu = ui.menu_button(facet_menu_label("日付", active), |ui| {
            prepare_facet_menu_popup(ui);
            let current = self.settings.facet_filter.date_preset;
            if ui.selectable_label(current.is_none(), "すべて").clicked() {
                self.settings.facet_filter.date_preset = None;
                changed = true;
                ui.close();
            }
            ui.separator();
            for &preset in FacetDatePreset::all() {
                if ui
                    .selectable_label(current == Some(preset), preset.label())
                    .clicked()
                {
                    self.settings.facet_filter.date_preset = Some(preset);
                    changed = true;
                    ui.close();
                }
            }
        });
        suppress_menu_button_wheel_passthrough(ui.ctx(), &menu.response);
        changed
    }

    fn draw_facet_size_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let active = usize::from(self.settings.facet_filter.size_preset.is_some());
        let mut changed = false;
        let menu = ui.menu_button(facet_menu_label("サイズ", active), |ui| {
            prepare_facet_menu_popup(ui);
            let current = self.settings.facet_filter.size_preset;
            if ui.selectable_label(current.is_none(), "すべて").clicked() {
                self.settings.facet_filter.size_preset = None;
                changed = true;
                ui.close();
            }
            ui.separator();
            for &preset in FacetSizePreset::all() {
                if ui
                    .selectable_label(current == Some(preset), preset.label())
                    .clicked()
                {
                    self.settings.facet_filter.size_preset = Some(preset);
                    changed = true;
                    ui.close();
                }
            }
        });
        suppress_menu_button_wheel_passthrough(ui.ctx(), &menu.response);
        changed
    }

    fn draw_facet_edit_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let label = facet_menu_label("状態", self.settings.facet_filter.edits.len());
        let (menu_response, _) = egui::containers::menu::MenuButton::new(label)
            .config(sticky_facet_menu_config())
            .ui(ui, |ui| {
            prepare_facet_menu_popup(ui);
            if self.settings.facet_filter.edits.is_empty() {
                ui.label("すべて");
            } else if ui.small_button("状態フィルタを解除").clicked() {
                self.settings.facet_filter.edits.clear();
                changed = true;
                ui.close();
            }
            ui.separator();
            for &flag in FacetEditFlag::all() {
                let mut selected = self.settings.facet_filter.edits.contains(&flag);
                if ui.checkbox(&mut selected, flag.menu_label()).changed() {
                    if selected {
                        self.settings.facet_filter.edits.insert(flag);
                    } else {
                        self.settings.facet_filter.edits.remove(&flag);
                    }
                    changed = true;
                }
            }
            ui.separator();
            let rollup_filter_selected = self.settings.facet_filter.has_rollup_edit_filter();
            ui.add_enabled_ui(rollup_filter_selected, |ui| {
                let mut include_descendants =
                    self.settings.facet_filter.edit_include_descendants;
                let resp = ui.checkbox(&mut include_descendants, "子フォルダも対象");
                if resp
                    .on_hover_text(
                        "補正・補正レイヤー・消しゴム・隠蔽・注釈・回転の状態フィルタで、子フォルダ配下の保存済み編集も対象にします",
                    )
                    .changed()
                {
                    self.settings.facet_filter.edit_include_descendants = include_descendants;
                    changed = true;
                }
            });
        });
        suppress_menu_button_wheel_passthrough(ui.ctx(), &menu_response);
        changed
    }

    fn draw_facet_color_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let active = usize::from(self.color_filter.enabled);
        self.color_filter.input_has_focus = false;
        let (menu_response, _) =
            egui::containers::menu::MenuButton::new(facet_menu_label("画像色", active))
                .config(sticky_facet_menu_config())
                .ui(ui, |ui| {
                    prepare_facet_menu_popup(ui);
                    ui.set_min_width(292.0);
                    self.draw_image_color_picker_header(ui);
                    ui.label(
                        egui::RichText::new("画像のみが対象です（動画・フォルダ・書庫は除外）")
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                    ui.add_space(6.0);
                    changed |= self.draw_image_color_sv_square(ui);
                    ui.add_space(6.0);
                    changed |= self.draw_image_color_hue_slider(ui);
                    ui.add_space(8.0);
                    changed |= self.draw_image_color_presets(ui);
                    ui.add_space(6.0);
                    changed |= self.draw_image_color_inputs(ui);
                    ui.add_space(6.0);
                    changed |= self.draw_image_color_tolerance(ui);

                    if let Some(pending) = self.color_filter.pending.as_ref() {
                        ui.separator();
                        ui.label(format!("絞り込み準備中 {}/{}", pending.done, pending.total));
                        let progress = if pending.total == 0 {
                            0.0
                        } else {
                            pending.done as f32 / pending.total as f32
                        };
                        ui.add(egui::ProgressBar::new(progress).desired_width(160.0));
                        if ui.small_button("キャンセル").clicked() {
                            self.color_filter.cancel_pending();
                            self.color_filter.confirmation = None;
                            self.color_filter.confirmed_large_scan_scope = None;
                            self.color_filter.enabled = false;
                            self.color_filter.applied_scope_signature = None;
                            changed = true;
                        }
                    } else if let Some(confirmation) = self.color_filter.confirmation.clone() {
                        ui.separator();
                        ui.label(format!(
                            "未スキャンの画像 {} 件を確認します",
                            confirmation.missing
                        ));
                        ui.horizontal(|ui| {
                            if ui.small_button("スキャン開始").clicked() {
                                self.confirm_large_color_scan(ui.ctx());
                                changed = true;
                            }
                            if ui.small_button("キャンセル").clicked() {
                                self.cancel_large_color_scan_confirmation();
                                changed = true;
                            }
                        });
                    } else if self.color_filter.enabled {
                        ui.separator();
                        ui.label(format!(
                            "画像色で絞り込み中 {}",
                            crate::color_search::hex_rgb(self.color_filter.query_rgb)
                        ));
                    }

                    ui.separator();
                    if !self.color_filter.enabled
                        && ui.small_button("この画像色で絞り込み").clicked()
                    {
                        changed |= self.activate_image_color_filter_from_ui();
                    }
                    if self.color_filter.enabled
                        && ui.small_button("画像色フィルタを解除").clicked()
                    {
                        self.color_filter.clear_filter();
                        changed = true;
                    }
                });
        let menu_response = menu_response.on_hover_text(
            "画像として扱える項目だけを、主要色で絞り込みます。動画やフォルダは対象外です。",
        );
        suppress_menu_button_wheel_passthrough(ui.ctx(), &menu_response);
        changed
    }

    fn draw_image_color_picker_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            self.draw_image_color_swatch(ui, self.color_filter.query_rgb, egui::vec2(28.0, 28.0));
            ui.vertical(|ui| {
                ui.label("画像色");
                ui.label(
                    egui::RichText::new(crate::color_search::hex_rgb(self.color_filter.query_rgb))
                        .monospace()
                        .color(ui.visuals().weak_text_color()),
                );
            });
        });
    }

    fn draw_image_color_sv_square(&mut self, ui: &mut egui::Ui) -> bool {
        let desired = egui::vec2(268.0, 156.0);
        let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let hue = self.color_filter.picker_hue_degrees;
        let columns = 40usize;
        let rows = 24usize;
        let cell_w = rect.width() / columns as f32;
        let cell_h = rect.height() / rows as f32;
        for row in 0..rows {
            for col in 0..columns {
                let saturation = col as f32 / (columns - 1) as f32;
                let value = 1.0 - row as f32 / (rows - 1) as f32;
                let rgb = crate::color_search::hsv_to_rgb(hue, saturation, value);
                let cell = egui::Rect::from_min_size(
                    egui::pos2(
                        rect.min.x + col as f32 * cell_w,
                        rect.min.y + row as f32 * cell_h,
                    ),
                    egui::vec2(cell_w + 0.5, cell_h + 0.5),
                );
                painter.rect_filled(cell, 0.0, egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
            }
        }

        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(4),
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
            egui::epaint::StrokeKind::Outside,
        );

        let (_, saturation, value) = crate::color_search::rgb_to_hsv(self.color_filter.query_rgb);
        let marker = egui::pos2(
            rect.left() + saturation * rect.width(),
            rect.top() + (1.0 - value) * rect.height(),
        );
        painter.circle_stroke(marker, 6.0, egui::Stroke::new(2.0, egui::Color32::WHITE));
        painter.circle_stroke(marker, 7.5, egui::Stroke::new(1.0, egui::Color32::BLACK));

        if (response.dragged() || response.clicked())
            && let Some(pos) = response.interact_pointer_pos()
        {
            let saturation = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let value = (1.0 - (pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
            let rgb = crate::color_search::hsv_to_rgb(hue, saturation, value);
            return self.set_image_color_query_from_ui(rgb);
        }
        false
    }

    fn draw_image_color_hue_slider(&mut self, ui: &mut egui::Ui) -> bool {
        let desired = egui::vec2(268.0, 16.0);
        let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let segments = 72usize;
        let segment_w = rect.width() / segments as f32;
        for i in 0..segments {
            let hue = i as f32 / segments as f32 * 360.0;
            let rgb = crate::color_search::hsv_to_rgb(hue, 1.0, 1.0);
            let segment = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + i as f32 * segment_w, rect.min.y),
                egui::vec2(segment_w + 0.5, rect.height()),
            );
            painter.rect_filled(
                segment,
                0.0,
                egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
            );
        }
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(4),
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
            egui::epaint::StrokeKind::Outside,
        );
        let hue = self.color_filter.picker_hue_degrees.rem_euclid(360.0);
        let x = rect.left() + hue / 360.0 * rect.width();
        painter.line_segment(
            [
                egui::pos2(x, rect.top() - 3.0),
                egui::pos2(x, rect.bottom() + 3.0),
            ],
            egui::Stroke::new(2.0, egui::Color32::WHITE),
        );
        painter.line_segment(
            [
                egui::pos2(x + 1.5, rect.top() - 2.0),
                egui::pos2(x + 1.5, rect.bottom() + 2.0),
            ],
            egui::Stroke::new(1.0, egui::Color32::BLACK),
        );

        if (response.dragged() || response.clicked())
            && let Some(pos) = response.interact_pointer_pos()
        {
            let hue = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0) * 360.0;
            self.color_filter.picker_hue_degrees = hue;
            let (_, saturation, value) =
                crate::color_search::rgb_to_hsv(self.color_filter.query_rgb);
            let saturation = if saturation < 0.01 { 1.0 } else { saturation };
            let value = if value < 0.01 { 1.0 } else { value };
            let rgb = crate::color_search::hsv_to_rgb(hue, saturation, value);
            return self.set_image_color_query_from_ui(rgb);
        }
        false
    }

    fn draw_image_color_presets(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            for rgb in COLOR_FILTER_PRESETS {
                let size = egui::vec2(24.0, 24.0);
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
                let fill = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                ui.painter()
                    .rect_filled(rect, egui::CornerRadius::same(5), fill);
                let selected = self.color_filter.query_rgb == rgb;
                let stroke = if selected {
                    egui::Stroke::new(2.0, ui.visuals().selection.stroke.color)
                } else {
                    egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
                };
                ui.painter().rect_stroke(
                    rect,
                    egui::CornerRadius::same(5),
                    stroke,
                    egui::epaint::StrokeKind::Outside,
                );
                if response
                    .on_hover_text(crate::color_search::hex_rgb(rgb))
                    .clicked()
                {
                    changed |= self.set_image_color_query_from_ui(rgb);
                }
            }
        });
        changed
    }

    fn draw_image_color_inputs(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            for (mode, label) in [
                (crate::color_search::ColorInputMode::Hex, "HEX"),
                (crate::color_search::ColorInputMode::Rgb, "RGB"),
                (crate::color_search::ColorInputMode::Hsl, "HSL"),
            ] {
                if ui
                    .selectable_label(self.color_filter.input_mode == mode, label)
                    .clicked()
                {
                    self.color_filter.input_mode = mode;
                }
            }
        });
        match self.color_filter.input_mode {
            crate::color_search::ColorInputMode::Hex => {
                ui.horizontal(|ui| {
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.color_filter.hex_input)
                            .desired_width(118.0)
                            .char_limit(7)
                            .hint_text("RRGGBB"),
                    );
                    self.color_filter.input_has_focus |= response.has_focus();
                    if response.changed()
                        && let Some(rgb) =
                            crate::color_search::parse_hex_rgb(&self.color_filter.hex_input)
                    {
                        changed |= self.set_image_color_query_from_ui(rgb);
                    }
                });
            }
            crate::color_search::ColorInputMode::Rgb => {
                let mut r = self.color_filter.query_rgb[0] as i32;
                let mut g = self.color_filter.query_rgb[1] as i32;
                let mut b = self.color_filter.query_rgb[2] as i32;
                let mut input_changed = false;
                ui.horizontal(|ui| {
                    let r_response =
                        ui.add(egui::DragValue::new(&mut r).range(0..=255).prefix("R "));
                    self.color_filter.input_has_focus |= r_response.has_focus();
                    input_changed |= r_response.changed();
                    let g_response =
                        ui.add(egui::DragValue::new(&mut g).range(0..=255).prefix("G "));
                    self.color_filter.input_has_focus |= g_response.has_focus();
                    input_changed |= g_response.changed();
                    let b_response =
                        ui.add(egui::DragValue::new(&mut b).range(0..=255).prefix("B "));
                    self.color_filter.input_has_focus |= b_response.has_focus();
                    input_changed |= b_response.changed();
                });
                if input_changed {
                    changed |= self.set_image_color_query_from_ui([r as u8, g as u8, b as u8]);
                }
            }
            crate::color_search::ColorInputMode::Hsl => {
                let (h, s, l) = crate::color_search::rgb_to_hsl(self.color_filter.query_rgb);
                let mut h = h.round() as i32;
                let mut s = (s * 100.0).round() as i32;
                let mut l = (l * 100.0).round() as i32;
                let mut input_changed = false;
                ui.horizontal(|ui| {
                    let h_response =
                        ui.add(egui::DragValue::new(&mut h).range(0..=360).prefix("H "));
                    self.color_filter.input_has_focus |= h_response.has_focus();
                    input_changed |= h_response.changed();
                    let s_response =
                        ui.add(egui::DragValue::new(&mut s).range(0..=100).prefix("S "));
                    self.color_filter.input_has_focus |= s_response.has_focus();
                    input_changed |= s_response.changed();
                    let l_response =
                        ui.add(egui::DragValue::new(&mut l).range(0..=100).prefix("L "));
                    self.color_filter.input_has_focus |= l_response.has_focus();
                    input_changed |= l_response.changed();
                });
                if input_changed {
                    let rgb = crate::color_search::hsl_to_rgb(
                        h as f32,
                        s as f32 / 100.0,
                        l as f32 / 100.0,
                    );
                    changed |= self.set_image_color_query_from_ui(rgb);
                }
            }
        }
        changed
    }

    fn draw_image_color_tolerance(&mut self, ui: &mut egui::Ui) -> bool {
        let mut tolerance = self.color_filter.tolerance;
        if ui
            .add(
                egui::Slider::new(
                    &mut tolerance,
                    crate::color_search::MIN_TOLERANCE..=crate::color_search::MAX_TOLERANCE,
                )
                .text("許容範囲"),
            )
            .changed()
        {
            self.color_filter.tolerance = tolerance;
            self.activate_image_color_filter_from_ui();
            return true;
        }
        false
    }

    fn draw_image_color_swatch(&self, ui: &mut egui::Ui, rgb: [u8; 3], size: egui::Vec2) {
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(5),
            egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
        );
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::same(5),
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
            egui::epaint::StrokeKind::Outside,
        );
    }

    fn set_image_color_query_from_ui(&mut self, rgb: [u8; 3]) -> bool {
        let changed = self.color_filter.query_rgb != rgb;
        if changed {
            self.color_filter.set_query_rgb(rgb);
        }
        let activated = self.activate_image_color_filter_from_ui();
        changed || activated
    }

    fn activate_image_color_filter_from_ui(&mut self) -> bool {
        if self.color_filter.enabled {
            return false;
        }
        self.color_filter.enabled = true;
        true
    }

    fn draw_facet_active_chips(&self, ui: &mut egui::Ui) {
        let filter = &self.settings.facet_filter;
        if !filter.kinds.is_empty() {
            facet_chip(ui, format!("種類:{}", filter.kinds.len()));
        }
        if !filter.exts.is_empty() {
            let values = filter
                .exts
                .iter()
                .take(3)
                .map(|ext| format!(".{ext}"))
                .collect::<Vec<_>>()
                .join(",");
            facet_chip(ui, format!("拡張子:{values}"));
        }
        if !filter.place_keys.is_empty() {
            let values = filter
                .place_keys
                .iter()
                .take(2)
                .map(|key| self.facet_place_label_for_key(key))
                .collect::<Vec<_>>()
                .join(",");
            facet_chip(ui, format!("場所:{values}"));
        }
        if !filter.ai_models.is_empty() {
            let values = filter
                .ai_models
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join(",");
            facet_chip(ui, format!("AIモデル:{values}"));
        }
        if !filter.ai_tools.is_empty() {
            let values = filter
                .ai_tools
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join(",");
            facet_chip(ui, format!("生成ツール:{values}"));
        }
        if self.rating_filter_active() && !self.items_are_rating_view {
            let values = (0..6)
                .filter(|&idx| self.settings.rating_filter[idx])
                .map(rating_button_label)
                .collect::<Vec<_>>()
                .join(",");
            facet_chip(ui, format!("★:{values}"));
        }
        if !filter.tags.is_empty() || filter.include_untagged {
            let display_names: std::collections::BTreeMap<&str, &str> = self
                .settings
                .tags
                .iter()
                .map(|tag| (tag.tag_key.as_str(), tag.name.as_str()))
                .collect();
            let mut parts = filter
                .tags
                .iter()
                .take(3)
                .map(|tag_key| {
                    format!(
                        "#{}",
                        display_names
                            .get(tag_key.as_str())
                            .copied()
                            .unwrap_or(tag_key.as_str())
                    )
                })
                .collect::<Vec<_>>();
            if filter.include_untagged {
                parts.push("タグなし".to_string());
            }
            facet_chip(
                ui,
                format!("タグ:{} {}", filter.tag_mode.label(), parts.join(",")),
            );
        }
        if let Some(preset) = filter.date_preset {
            facet_chip(ui, format!("日付:{}", preset.label()));
        }
        if let Some(preset) = filter.size_preset {
            facet_chip(ui, format!("サイズ:{}", preset.label()));
        }
        if !filter.edits.is_empty() {
            let mut values = filter
                .edits
                .iter()
                .take(3)
                .map(|flag| flag.label())
                .collect::<Vec<_>>();
            if filter.has_rollup_edit_filter()
                && filter.edit_include_descendants
                && values.len() < 3
            {
                values.push("子フォルダ");
            }
            let values = values.join(",");
            facet_chip(ui, format!("状態:{values}"));
        }
    }

    fn draw_color_filter_active_chip(&self, ui: &mut egui::Ui) {
        if !self.color_filter.enabled {
            return;
        }
        let text = if let Some(pending) = self.color_filter.pending.as_ref() {
            format!(
                "画像色 {}: {}/{}",
                crate::color_search::hex_rgb(self.color_filter.query_rgb),
                pending.done,
                pending.total
            )
        } else if let Some(confirmation) = self.color_filter.confirmation.as_ref() {
            format!(
                "画像色 {}: 確認待ち {}",
                crate::color_search::hex_rgb(self.color_filter.query_rgb),
                confirmation.missing
            )
        } else {
            format!(
                "画像色 {}",
                crate::color_search::hex_rgb(self.color_filter.query_rgb)
            )
        };
        facet_chip(ui, text);
    }

    fn facet_kind_counts(&mut self) -> BTreeMap<FacetItemKind, usize> {
        let indices = self.facet_candidate_indices(FacetField::Kind);
        let mut counts = BTreeMap::new();
        for idx in indices {
            if let Some(kind) = self.facet_item_kind(idx) {
                *counts.entry(kind).or_insert(0) += 1;
            }
        }
        counts
    }

    fn facet_rating_counts(&mut self) -> [usize; 6] {
        let indices = self.facet_candidate_indices(FacetField::Rating);
        let mut counts = [0usize; 6];
        for idx in indices {
            let accepts_rating = self
                .items
                .get(idx)
                .is_some_and(crate::grid_item::GridItem::accepts_rating);
            if !accepts_rating {
                continue;
            }
            let stars = self.get_rating(idx).min(5) as usize;
            counts[stars] += 1;
        }
        counts
    }

    fn facet_ext_counts(&mut self) -> BTreeMap<String, usize> {
        let indices = self.facet_candidate_indices(FacetField::Ext);
        let mut counts = BTreeMap::new();
        for idx in indices {
            let ext = self.facet_item_ext(idx);
            if !ext.is_empty() {
                *counts.entry(ext).or_insert(0) += 1;
            }
        }
        counts
    }

    fn facet_place_counts(&mut self) -> BTreeMap<String, (String, usize)> {
        if let Some(counts) = self.facet_place_counts_cache.as_ref() {
            return counts.clone();
        }
        let perf_t0 = crate::perf::is_enabled().then(std::time::Instant::now);
        let indices = self.facet_candidate_indices(FacetField::Place);
        let candidate_count = indices.len();
        let mut counts = BTreeMap::new();
        for idx in indices {
            let Some(item) = self.items.get(idx) else {
                continue;
            };
            let Some(path) = self.facet_place_path_for_item(item) else {
                continue;
            };
            let key = crate::adjustment_db::normalize_path(&path);
            let label = self.facet_place_label_for_path(&path);
            let entry = counts.entry(key).or_insert((label, 0));
            entry.1 += 1;
        }
        if let Some(t0) = perf_t0 {
            crate::perf::event(
                "ui",
                "facet_place_counts_build",
                None,
                self.input_seq,
                &[
                    (
                        "ms",
                        serde_json::Value::from(t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("candidates", serde_json::Value::from(candidate_count)),
                    ("places", serde_json::Value::from(counts.len())),
                ],
            );
        }
        self.facet_place_counts_cache = Some(counts.clone());
        counts
    }

    fn facet_ai_model_counts(&mut self) -> BTreeMap<String, usize> {
        if let Some(counts) = self.facet_ai_model_counts_cache.as_ref() {
            return counts.clone();
        }
        let perf_t0 = crate::perf::is_enabled().then(std::time::Instant::now);
        let indices = self.facet_candidate_indices(FacetField::AiModel);
        let candidate_count = indices.len();
        let mut counts = BTreeMap::new();
        for idx in indices {
            for model in self.facet_ai_model_values(idx) {
                *counts.entry(model).or_insert(0) += 1;
            }
        }
        if let Some(t0) = perf_t0 {
            crate::perf::event(
                "ui",
                "facet_ai_model_counts_build",
                None,
                self.input_seq,
                &[
                    (
                        "ms",
                        serde_json::Value::from(t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("candidates", serde_json::Value::from(candidate_count)),
                    ("models", serde_json::Value::from(counts.len())),
                ],
            );
        }
        self.facet_ai_model_counts_cache = Some(counts.clone());
        counts
    }

    fn facet_ai_tool_counts(&mut self) -> BTreeMap<String, usize> {
        if let Some(counts) = self.facet_ai_tool_counts_cache.as_ref() {
            return counts.clone();
        }
        let perf_t0 = crate::perf::is_enabled().then(std::time::Instant::now);
        let indices = self.facet_candidate_indices(FacetField::AiTool);
        let candidate_count = indices.len();
        let mut counts = BTreeMap::new();
        for idx in indices {
            let tool = self.facet_ai_tool_value(idx);
            if !tool.is_empty() {
                *counts.entry(tool).or_insert(0) += 1;
            }
        }
        if let Some(t0) = perf_t0 {
            crate::perf::event(
                "ui",
                "facet_ai_tool_counts_build",
                None,
                self.input_seq,
                &[
                    (
                        "ms",
                        serde_json::Value::from(t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("candidates", serde_json::Value::from(candidate_count)),
                    ("tools", serde_json::Value::from(counts.len())),
                ],
            );
        }
        self.facet_ai_tool_counts_cache = Some(counts.clone());
        counts
    }

    fn facet_tag_counts(&mut self) -> (BTreeMap<String, usize>, usize) {
        // menu closure は開いている間毎フレーム実行される。全可視アイテム × 全タグの
        // NFKC 正規化を毎フレーム回さないよう、タグ変更 (invalidate_tag_apply_suggestions)
        // と表示集合変更 (rebuild_visible_indices) まではキャッシュを返す。
        if let Some((counts, untagged)) = self.facet_tag_counts_cache.as_ref() {
            return (counts.clone(), *untagged);
        }
        let indices = self.facet_candidate_indices(FacetField::Tags);
        let mut counts = BTreeMap::new();
        let mut untagged = 0usize;
        for idx in indices {
            let Some(kind) = self.facet_item_kind(idx) else {
                continue;
            };
            if !matches!(
                kind,
                FacetItemKind::Image
                    | FacetItemKind::Video
                    | FacetItemKind::Zip
                    | FacetItemKind::Pdf
                    | FacetItemKind::Archive
            ) {
                continue;
            }
            let tags = self.cell_tag_list(idx);
            if tags.is_empty() {
                untagged += 1;
            } else {
                for tag in tags {
                    let tag_key = crate::tags_db::normalize_tag_key(tag);
                    if !tag_key.is_empty() {
                        *counts.entry(tag_key).or_insert(0) += 1;
                    }
                }
            }
        }
        self.facet_tag_counts_cache = Some((counts.clone(), untagged));
        (counts, untagged)
    }

    // ── フォルダバー ─────────────────────────────────────────────────

    /// フォルダバーを描画し、Enter や履歴ボタンで確定されたナビゲーションを返す。
    pub(crate) fn render_address_bar(&mut self, ctx: &egui::Context) -> Option<AddressBarNav> {
        if !self.settings.show_toolbar_folder {
            self.address_has_focus = false;
            return None;
        }

        let enter_pressed = self.dialog_enter_pressed(ctx);
        let effective_folder = self.effective_folder();
        let has_current = effective_folder.is_some();
        let parent_nav_target = self.grid_parent_nav_target();
        let back_target = self.folder_history_back_target().cloned();
        let forward_target = self.folder_history_forward_target().cloned();
        let rating_counts = self.rating_counts();
        let quick_folder_targets: [Option<PathBuf>; 2] =
            std::array::from_fn(|idx| self.quick_folder_workspaces[idx].target.clone());
        let active_quick_folder_slot = self.active_quick_folder_slot;
        // Codex P2-1: ★固定 中は履歴/親/ツリーボタンを disabled (= 余計な処理を起動しない)
        let snapshot_active = self.is_snapshot_active();
        let drive_list_active = self.items_are_drive_list;
        let recent_folders: Vec<PathBuf> = self.recent_folder_entries().to_vec();
        let favorite_target = self.current_favorite_target();
        let current_is_favorite = favorite_target.as_ref().is_some_and(|p| {
            self.settings
                .favorites
                .iter()
                .any(|fav| crate::folder_tree::path_eq(&fav.path, p))
        });

        // 検索 (Ctrl+G / Ctrl+S / Ctrl+T) 中はアドレスバーの ←/→/履歴 を無効化し、⬆/▲▼ を
        // 検索仮想階層用に転用する。closure 内で複数のボタン分岐から参照するため事前計算。
        let search_active =
            self.global_search.active || self.favsearch.active || self.tag_view.active;
        let local_search_active =
            self.show_search_bar || self.search_filter.is_some() || self.search_pending.is_some();
        // 検索の仮想階層でドリルイン中か (= ⬆ で 1 段戻れる / ▲▼ で前後ヒットへ
        // 動ける状態。最上位ではこれらを disabled にする条件)。
        let search_drilled_in = (self.global_search.active && self.global_search.drill.is_some())
            || (self.favsearch.active && !self.favsearch.nav_stack.is_empty())
            || (self.tag_view.active && !self.tag_view.nav_stack.is_empty());
        let search_tree_drilled_in = (self.global_search.active
            && self.global_search.drill.is_some())
            || (self.favsearch.active && !self.favsearch.nav_stack.is_empty());
        let local_search_blocks_parent = self.local_search_blocks_parent_nav();

        // 現在表示中フォルダ / ZIP / PDF のコンテナレーティングを取得。
        // 0 のときは非表示、1〜5 のときは★バッジをアドレス欄の右端に表示する。
        let folder_rating = self.current_folder_rating();
        let thumbnail_count = (!self.global_search.active
            && !self.favsearch.active
            && !self.tag_view.active
            && !self.items_are_global_search_view
            && !self.items_are_tag_view
            && self.search_filter.is_none()
            && self.search_pending.is_none())
        .then(|| thumbnail_count_label(&self.items, &self.visible_indices));
        // 📌 (代表サムネ固定) ボタンの表示判定 + 状態をあらかじめ計算する。
        // closure 内で `self` のミュータブル借用が衝突しないように外で確定しておく。
        let pin_button_info = self.compute_folder_pin_button_state();
        // ファイル名スタック (v2.0.0) のトグル状態も外で確定 (closure 内は表示のみ)。
        // フォルダバー右クリックの「スタック表示トグル」設定が OFF なら隠す。ただし現在 ON の
        // ときは隠さない (= 隠したまま OFF にできず詰むのを防ぐ)。
        let stack_on = self.stack_mode_on();
        let stack_available = (self.settings.show_address_bar_stack_toggle || stack_on)
            && self.stack_mode_available();
        let subfolder_expansion_on = self.subfolder_expansion_on();
        let subfolder_expansion_pending_label = self.subfolder_expansion_pending_label();
        let subfolder_expansion_pending_tooltip = self.subfolder_expansion_pending_tooltip();
        let subfolder_expansion_available = subfolder_expansion_on
            || subfolder_expansion_pending_label.is_some()
            || self.subfolder_expansion_available();
        egui::TopBottomPanel::top("address_bar")
            .show(ctx, |ui| -> Option<AddressBarNav> {
                ui.add_space(3.0);
                let mut result = None;
                let mut pin_click = PinButtonClick::None;
                let mut favorite_click = FavoriteButtonClick::None;
                let mut tree_nav: Option<bool> = None;
                // ファイル名スタックのトグルクリックは closure 後に処理する (load_folder で
                // App ミュータブル借用が必要)。
                let mut stack_toggle = false;
                let mut subfolder_expansion_toggle = false;
                // 検索中の ⬆ ボタン (検索仮想階層を 1 段ドリルアップ) を closure 後に適用。
                let mut search_drill_up = false;
                ui.horizontal(|ui| {
                    // 左端のラベルを右クリックすると、フォルダバーの設定メニューを開く
                    // (他のツールバーセクションと操作を揃える。実機フィードバック 2026-06-20)。
                    let folder_label_response = ui
                        .add(egui::Label::new("フォルダ:").sense(egui::Sense::click()))
                        .on_hover_text("右クリック: フォルダバーの設定");
                    show_sticky_context_menu(&folder_label_response, |ui| {
                        self.draw_folder_bar_settings_menu(ui);
                    });
                    let show_history_nav = self.settings.show_address_bar_history_nav;
                    let show_quick_folders = self.settings.show_address_bar_quick_folders;
                    let show_parent = self.settings.show_toolbar_parent_button;
                    let show_tree_nav = self.settings.show_toolbar_prev_folder
                        || self.settings.show_toolbar_next_folder;

                    if show_history_nav {
                        // 検索 (Ctrl+G / Ctrl+S) 中は ←/→ を無効化する。検索は透明な
                        // 一時オーバーレイで、フォルダ履歴の概念が適用されないため。
                        // ★固定 中も同様に無効化 (Codex P2-1)。
                        let back_hover = if drive_list_active {
                            "ドライブ一覧では履歴ナビを使用できません".to_string()
                        } else if snapshot_active {
                            "★固定中は履歴ナビを使用できません".to_string()
                        } else if search_active {
                            "検索中はフォルダ履歴を使用できません".to_string()
                        } else {
                            back_target
                                .as_ref()
                                .map(|p| format!("フォルダ履歴を戻る\n{}", p.to_string_lossy()))
                                .unwrap_or_else(|| "フォルダ履歴を戻る".to_string())
                        };
                        if ui
                            .add_enabled(
                                back_target.is_some()
                                    && !search_active
                                    && !snapshot_active
                                    && !drive_list_active,
                                egui::Button::new("←"),
                            )
                            .hover_tip(back_hover)
                            .clicked()
                        {
                            result = Some(AddressBarNav::HistoryBack);
                        }
                        let forward_hover = if drive_list_active {
                            "ドライブ一覧では履歴ナビを使用できません".to_string()
                        } else if snapshot_active {
                            "★固定中は履歴ナビを使用できません".to_string()
                        } else if search_active {
                            "検索中はフォルダ履歴を使用できません".to_string()
                        } else {
                            forward_target
                                .as_ref()
                                .map(|p| format!("フォルダ履歴を進む\n{}", p.to_string_lossy()))
                                .unwrap_or_else(|| "フォルダ履歴を進む".to_string())
                        };
                        if ui
                            .add_enabled(
                                forward_target.is_some()
                                    && !search_active
                                    && !snapshot_active
                                    && !drive_list_active,
                                egui::Button::new("→"),
                            )
                            .hover_tip(forward_hover)
                            .clicked()
                        {
                            result = Some(AddressBarNav::HistoryForward);
                        }
                    }

                    if show_quick_folders {
                        for slot in QuickFolderSlotId::ALL {
                            let idx = slot.index();
                            let label = slot.label();
                            let target = quick_folder_targets[idx].as_ref();
                            let is_active = active_quick_folder_slot == Some(slot);
                            let same_target = target.as_ref().is_some_and(|target| {
                                effective_folder.as_ref().is_some_and(|current| {
                                    crate::folder_tree::path_eq(current, target)
                                })
                            });
                            let enabled = !search_active && !local_search_active && !snapshot_active;
                            let tooltip = if snapshot_active {
                                format!("{label}: ★固定中は使用できません")
                            } else if search_active {
                                format!("{label}: 検索中は使用できません")
                            } else if local_search_active {
                                format!("{label}: 現在地検索中は使用できません")
                            } else if let Some(path) = target {
                                format!(
                                    "{label}: {}\n左クリック: この作業場所に切り替え",
                                    path.to_string_lossy()
                                )
                            } else {
                                format!("{label}: 未訪問です\n左クリック: ドライブ一覧へ切り替え")
                            };
                            let mut rich = egui::RichText::new(label).monospace();
                            if is_active {
                                rich = rich
                                    .strong()
                                    .color(egui::Color32::from_rgb(230, 170, 70));
                            } else if same_target {
                                rich = rich.color(egui::Color32::from_rgb(120, 170, 210));
                            } else if target.is_none() {
                                rich = rich.weak();
                            }
                            let response = ui
                                .add_enabled(
                                    enabled,
                                    egui::Button::new(rich).min_size(egui::vec2(24.0, 20.0)),
                                )
                                .hover_tip(tooltip);
                            if response.clicked() {
                                match self.activate_quick_folder_slot(slot) {
                                    QuickFolderSwitchTarget::Current => {}
                                    QuickFolderSwitchTarget::DriveList => {
                                        result = Some(AddressBarNav::DriveList(None));
                                    }
                                    QuickFolderSwitchTarget::Folder(raw_target) => {
                                        if let Some(resolved) =
                                            resolve_folder_bar_nav_path(&raw_target)
                                        {
                                            result = Some(AddressBarNav::Direct(resolved));
                                        } else {
                                            self.show_feedback_toast(format!(
                                                "{label} の最後の場所が見つかりません。ドライブ一覧へ切り替えます: {}",
                                                raw_target.to_string_lossy()
                                            ));
                                            result = Some(AddressBarNav::DriveList(None));
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if (show_history_nav || show_quick_folders) && (show_parent || show_tree_nav) {
                        ui.separator();
                    }

                    if show_parent {
                        if search_active {
                            // 検索中: ⬆ は検索仮想階層を 1 段ドリルアップする
                            // (BS と同じ動作)。最上位 (集約ビュー / 結果一覧) では disabled。
                            let up_hover = if search_drilled_in {
                                "検索結果を 1 階層戻る [BS]"
                            } else {
                                "これ以上戻れません"
                            };
                            if ui
                                .add_enabled(search_drilled_in, egui::Button::new("⬆"))
                                .hover_tip(up_hover)
                                .clicked()
                            {
                                search_drill_up = true;
                            }
                        } else {
                            // Codex P2-1: ★固定 中は親への移動 (= scope 外) を disabled
                            let parent_hover = if snapshot_active {
                                "★固定中は親フォルダへ移動できません".to_string()
                            } else if local_search_blocks_parent {
                                "Ctrl+F フィルタ中は親フォルダへ移動できません\nEsc または × で検索を閉じます"
                                    .to_string()
                            } else {
                                match parent_nav_target.as_ref() {
                                    Some(AddressBarNav::Direct(p)) => {
                                        format!("親フォルダへ [BS]\n{}", p.to_string_lossy())
                                    }
                                    Some(AddressBarNav::DriveList(Some(origin))) => {
                                        format!("ドライブ一覧へ [BS]\n{}", origin.to_string_lossy())
                                    }
                                    Some(AddressBarNav::DriveList(None)) => {
                                        "ドライブ一覧へ [BS]".to_string()
                                    }
                                    Some(AddressBarNav::ReadingHistory) => {
                                        "読書履歴へ戻る [BS]".to_string()
                                    }
                                    Some(AddressBarNav::BooksRoot) => {
                                        "本棚フォルダへ".to_string()
                                    }
                                    Some(
                                        AddressBarNav::HistoryBack | AddressBarNav::HistoryForward,
                                    )
                                    | None => "親フォルダへ [BS]".to_string(),
                                }
                            };
                            if ui
                                .add_enabled(
                                    parent_nav_target.is_some()
                                        && !snapshot_active
                                        && !local_search_blocks_parent,
                                    egui::Button::new("⬆"),
                                )
                                .hover_tip(parent_hover)
                                .clicked()
                            {
                                // BS と同じ優先順位: ネスト ZIP ツリー内なら実フォルダ親へ
                                // 抜ける前に 1 階層戻る (tooltip も「[BS]」と同挙動を謳って
                                // いるため。レビュー P3)。ルートでは false → 従来どおり親へ。
                                if !self.zip_nav_back() {
                                    result = self.resolve_grid_parent_nav();
                                }
                            }
                        }
                    }

                    if show_tree_nav {
                        // 検索中は ▲▼ を「前後のヒットフォルダへ移動」に転用する
                        // (キーボード Ctrl+↑↓ と一致)。最上位では disabled。
                        // ★固定 中は scope 外への DFS 移動を起動しない方が良い
                        // (= snapshot 内 nav は別経路、Codex P2-1)。
                        let tree_enabled = if snapshot_active {
                            // snapshot 中も ▲▼ は使える: snapshot 内 container 巡回 (= キー Ctrl+↑↓ と一致)
                            true
                        } else if search_active {
                            search_tree_drilled_in
                        } else if subfolder_expansion_on {
                            false
                        } else {
                            has_current
                        };
                        let prev_hover = if snapshot_active {
                            "★固定リストの前へ [Ctrl+↑]"
                        } else if search_active {
                            "前のヒットフォルダへ [Ctrl+↑]"
                        } else if subfolder_expansion_on {
                            "サブ展開中はフォルダ移動しません"
                        } else {
                            "ツリー順で前のフォルダへ [Ctrl+↑]"
                        };
                        let next_hover = if snapshot_active {
                            "★固定リストの次へ [Ctrl+↓]"
                        } else if search_active {
                            "次のヒットフォルダへ [Ctrl+↓]"
                        } else if subfolder_expansion_on {
                            "サブ展開中はフォルダ移動しません"
                        } else {
                            "ツリー順で次のフォルダへ [Ctrl+↓]"
                        };
                        if ui
                            .add_enabled(tree_enabled, egui::Button::new("▲"))
                            .hover_tip(prev_hover)
                            .clicked()
                        {
                            tree_nav = Some(false);
                        }
                        if ui
                            .add_enabled(tree_enabled, egui::Button::new("▼"))
                            .hover_tip(next_hover)
                            .clicked()
                        {
                            tree_nav = Some(true);
                        }
                    }

                    if show_history_nav || show_parent || show_tree_nav {
                        ui.separator();
                    }

                    let place_nav_enabled = !search_active && !snapshot_active;
                    ui.add_enabled_ui(place_nav_enabled, |ui| {
                        let place_response = ui.menu_button("場所▼", |ui| {
                            ui.set_min_width(220.0);
                            let mut drew_place_item = false;
                            if self.settings.show_location_drive_list
                                && ui.button("ドライブ一覧").clicked()
                            {
                                result = Some(AddressBarNav::DriveList(None));
                                ui.close();
                            }
                            drew_place_item |= self.settings.show_location_drive_list;
                            if self.settings.show_location_reading_history
                                && ui.button("読書履歴").clicked()
                            {
                                result = Some(AddressBarNav::ReadingHistory);
                                ui.close();
                            }
                            drew_place_item |= self.settings.show_location_reading_history;
                            if self.settings.show_location_rating {
                                ui.menu_button("レーティング", |ui| {
                                    for stars in 1..=5 {
                                        if ui
                                            .button(rating_view_menu_label(stars, rating_counts))
                                            .clicked()
                                        {
                                            self.enter_rating_view(stars);
                                            ui.close();
                                        }
                                    }
                                });
                                drew_place_item = true;
                            }
                            if self.settings.show_location_bookshelf
                                && ui
                                    .button("本棚フォルダ")
                                    .hover_tip(self.book_root_path().to_string_lossy().to_string())
                                    .clicked()
                            {
                                result = Some(AddressBarNav::BooksRoot);
                                ui.close();
                            }
                            drew_place_item |= self.settings.show_location_bookshelf;

                            let mut quick_locations = Vec::new();
                            let mut push_quick =
                                |label: &'static str, path: Option<std::path::PathBuf>| {
                                    let Some(path) = path else {
                                        return;
                                    };
                                    if quick_locations.iter().any(
                                        |existing: &crate::known_folders::QuickLocation| {
                                            crate::folder_tree::path_eq(&existing.path, &path)
                                        },
                                    ) {
                                        return;
                                    }
                                    quick_locations
                                        .push(crate::known_folders::QuickLocation { label, path });
                                };
                            if self.settings.show_location_desktop {
                                push_quick("デスクトップ", crate::known_folders::desktop_dir());
                            }
                            if self.settings.show_location_pictures {
                                push_quick("ピクチャ", crate::known_folders::pictures_dir());
                            }
                            if self.settings.show_location_downloads {
                                push_quick("ダウンロード", crate::known_folders::downloads_dir());
                            }
                            drop(push_quick);
                            let drew_quick_locations = !quick_locations.is_empty();
                            if drew_quick_locations && drew_place_item {
                                ui.separator();
                            }
                            for location in quick_locations {
                                let full = location.path.to_string_lossy().to_string();
                                if ui.button(location.label).hover_tip(&full).clicked() {
                                    if let Some(resolved) =
                                        resolve_folder_bar_nav_path(&location.path)
                                    {
                                        result = Some(AddressBarNav::Direct(resolved));
                                    }
                                    ui.close();
                                }
                            }
                            drew_place_item |= drew_quick_locations;

                            if self.settings.show_location_drive_roots {
                                let drives = crate::known_folders::available_drives();
                                if !drives.is_empty() && drew_place_item {
                                    ui.separator();
                                }
                                for drive in drives {
                                    let label = drive.to_string_lossy().to_string();
                                    if ui
                                        .button(egui::RichText::new(&label).monospace())
                                        .hover_tip(&label)
                                        .clicked()
                                    {
                                        if let Some(resolved) =
                                            resolve_folder_bar_nav_path(&drive)
                                        {
                                            result = Some(AddressBarNav::Direct(resolved));
                                        }
                                        ui.close();
                                    }
                                }
                                drew_place_item = true;
                            }

                            if !drew_place_item {
                                ui.label(egui::RichText::new("表示項目なし").weak());
                            }
                        })
                        .response
                        .hover_tip(if snapshot_active {
                            "★固定中は場所ジャンプを使用できません"
                        } else if search_active {
                            "検索中は場所ジャンプを使用できません"
                        } else {
                            "デスクトップ / 主要フォルダ / ドライブへ移動"
                        });
                        show_sticky_context_menu(&place_response, |ui| {
                            self.draw_folder_bar_settings_menu(ui);
                        });
                    });
                    ui.add_space(4.0);
                    ui.separator();

                    // ★バッジは右寄せで先に配置し、残り幅を TextEdit が埋める。
                    // right_to_left レイアウトで ★ → TextEdit の順に追加すると、
                    // TextEdit は available width いっぱいに広がる。
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if folder_rating >= 1 && folder_rating <= 5 {
                            let stars = "★".repeat(folder_rating as usize);
                            ui.label(
                                egui::RichText::new(format!("📁{stars}"))
                                    .color(egui::Color32::from_rgb(130, 170, 220))
                                    .strong(),
                            )
                            .hover_tip(folder_rating_tooltip(&self.keymap));
                            ui.add_space(4.0);
                        }
                        if let Some(count) = thumbnail_count.as_ref() {
                            ui.label(
                                egui::RichText::new(count.as_str())
                                    .size(11.0)
                                    .monospace()
                                    .color(egui::Color32::from_gray(140)),
                            )
                            .hover_tip("表示中のサムネイル数 / 全サムネイル数");
                            ui.add_space(4.0);
                        }
                        // ファイル名スタック表示トグル (v2.0.0)。サムネ枚数 (12/345) の左側に置く
                        // (実機フィードバック 2026-06-20)。通常フォルダ表示のときだけ出す
                        // (検索 / ZIP ツリー / ドライブ一覧では無効)。
                        if stack_available {
                            let resp = ui.selectable_label(stack_on, "スタック").hover_tip(
                                "似たファイルを自動で分類して 1 つに畳んで表示 [トグル]。スタックを開くと \
                                 ↓↑ で全画像送り・Shift+↓↑ で次/前のスタックへ (分類ルールはヘルプ参照)",
                            );
                            if resp.clicked() {
                                stack_toggle = true;
                            }
                            ui.add_space(4.0);
                        }
                        if subfolder_expansion_available {
                            let pending = subfolder_expansion_pending_label.is_some();
                            if pending {
                                let tooltip = subfolder_expansion_pending_tooltip
                                    .clone()
                                    .unwrap_or_else(|| "サブフォルダを走査中".to_string());
                                let cancel_resp = ui
                                    .small_button("中止")
                                    .hover_tip(format!("{tooltip}\nクリック: 走査をキャンセル"));
                                if cancel_resp.clicked() {
                                    subfolder_expansion_toggle = true;
                                }
                                ui.add_space(2.0);
                            }
                            let label = if let Some(progress) =
                                subfolder_expansion_pending_label.as_ref()
                            {
                                progress.as_str()
                            } else {
                                "サブ展開"
                            };
                            let tooltip = if pending {
                                subfolder_expansion_pending_tooltip
                                    .clone()
                                    .unwrap_or_else(|| "サブフォルダを走査中".to_string())
                            } else if subfolder_expansion_on {
                                "サブ展開を解除して元のフォルダへ戻る".to_string()
                            } else {
                                self.subfolder_expansion_action_tooltip()
                            };
                            let resp = ui
                                .selectable_label(subfolder_expansion_on || pending, label)
                                .hover_tip(tooltip);
                            if resp.clicked() {
                                subfolder_expansion_toggle = true;
                            }
                            ui.add_space(4.0);
                        }
                        // 📌 (代表サムネ固定): right_to_left なので 📁★ より左 (= 入力欄寄り) に置く。
                        if let Some(info) = pin_button_info.as_ref() {
                            let label = if info.matches_current_pin {
                                egui::RichText::new("📌")
                                    .color(egui::Color32::from_rgb(230, 180, 90))
                                    .strong()
                            } else {
                                egui::RichText::new("📌")
                            };
                            let btn = egui::Button::new(label).frame(false);
                            let resp = ui.add_enabled(info.enabled, btn);
                            let resp = resp.hover_tip(info.tooltip.as_str());
                            if info.enabled {
                                if resp.clicked() {
                                    pin_click = PinButtonClick::Toggle;
                                } else if resp.secondary_clicked() {
                                    pin_click = PinButtonClick::Remove;
                                }
                            }
                            ui.add_space(4.0);
                        }

                        if self.settings.show_address_bar_history_menu {
                            // 検索 (Ctrl+G / Ctrl+S) 中は履歴メニューを無効化する
                            // (検索は透明な一時オーバーレイで履歴の概念が適用されない)。
                            // Codex 3rd P3 fix: snapshot 中も同様に無効化。
                            ui.add_enabled_ui(!search_active && !snapshot_active, |ui| {
                                ui.menu_button("履歴▼", |ui| {
                                    let menu_width =
                                        (ctx.content_rect().width() * 0.72).clamp(560.0, 1100.0);
                                    ui.set_min_width(menu_width);
                                    let mut shown = 0usize;
                                    for path in &recent_folders {
                                        if effective_folder.as_ref().is_some_and(|cur| {
                                            crate::folder_tree::path_eq(cur, path)
                                        }) {
                                            continue;
                                        }
                                        let full = path.to_string_lossy().to_string();
                                        let button = egui::Button::new(
                                            egui::RichText::new(&full).monospace(),
                                        )
                                        .wrap_mode(egui::TextWrapMode::Extend);
                                        if ui.add(button).hover_tip(&full).clicked() {
                                            if let Some(resolved) =
                                                resolve_folder_bar_nav_path(path)
                                            {
                                                result = Some(AddressBarNav::Direct(resolved));
                                            }
                                            ui.close();
                                        }
                                        shown += 1;
                                        if shown >= 20 {
                                            break;
                                        }
                                    }
                                    if shown == 0 {
                                        ui.label(egui::RichText::new("履歴はありません").weak());
                                    }
                                })
                                .response
                                .hover_tip(if search_active {
                                    "検索中は履歴メニューを使用できません"
                                } else {
                                    "最近開いたフォルダ"
                                });
                            });
                            ui.add_space(4.0);
                        }

                        if self.settings.show_address_bar_favorite_button {
                            let (label, tooltip) = if current_is_favorite {
                                (
                                    egui::RichText::new("♥")
                                        .color(egui::Color32::from_rgb(230, 110, 130))
                                        .strong(),
                                    "このフォルダのお気に入り設定を開く",
                                )
                            } else if favorite_target.is_none() {
                                (
                                    egui::RichText::new("♡").weak(),
                                    "お気に入りに追加できるのは実フォルダのみです",
                                )
                            } else {
                                (egui::RichText::new("♡"), "このフォルダをお気に入りに追加…")
                            };
                            if ui
                                .add_enabled(
                                    favorite_target.is_some(),
                                    egui::Button::new(label).frame(false),
                                )
                                .hover_tip(tooltip)
                                .clicked()
                            {
                                favorite_click = if current_is_favorite {
                                    FavoriteButtonClick::Edit
                                } else {
                                    FavoriteButtonClick::Add
                                };
                            }
                            ui.add_space(4.0);
                        }

                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            // snapshot 中はフォルダパス入力を disabled にする (= §4.4)。
                            // suffix は TextEdit の **左** に colored_label として置く
                            // (= 旧版で右に置くと TextEdit が desired_width(INFINITY) で残り幅を
                            // 全消費するため履歴プルダウン等に重なる事故が起きていた、ユーザー報告)。
                            let snap_suffix = self.snapshot_path_suffix();
                            let is_snap_active = snap_suffix.is_some();
                            if let Some(suffix) = snap_suffix {
                                ui.colored_label(egui::Color32::from_rgb(58, 110, 165), suffix);
                            }
                            let resp = if is_snap_active {
                                ui.add_enabled(
                                    false,
                                    egui::TextEdit::singleline(&mut self.address)
                                        .desired_width(f32::INFINITY),
                                )
                            } else {
                                let mut output = egui::TextEdit::singleline(&mut self.address)
                                    .desired_width(f32::INFINITY)
                                    .show(ui);
                                crate::ui_helpers::singleline_text_edit_context_menu(
                                    ui,
                                    &mut output,
                                    &mut self.address,
                                );
                                output.response
                            };
                            self.address_has_focus = resp.has_focus();
                            if !is_snap_active && resp.lost_focus() && enter_pressed {
                                let address_text = self.address.trim();
                                if address_text.is_empty() {
                                    result = Some(AddressBarNav::DriveList(None));
                                } else if address_text == "本棚" {
                                    result = Some(AddressBarNav::Direct(self.book_root_path()));
                                } else if let Some(book_name) =
                                    address_text.strip_prefix("本棚 > ")
                                {
                                    result = Some(AddressBarNav::Direct(
                                        crate::books::book_folder(
                                            &self.book_root_path(),
                                            book_name.trim(),
                                        ),
                                    ));
                                } else if let Some(resolved) =
                                    resolve_folder_bar_nav_path(&PathBuf::from(&self.address))
                                {
                                    result = Some(AddressBarNav::Direct(resolved));
                                }
                            }
                        });
                    });
                });
                ui.add_space(3.0);
                // pin ボタンクリックは closure 抜けてから処理する (App ミュータブル借用が必要)
                match pin_click {
                    PinButtonClick::Toggle => self.toggle_folder_pin_from_selection(),
                    PinButtonClick::Remove => self.remove_folder_pin_for_current_container(),
                    PinButtonClick::None => {}
                }
                match favorite_click {
                    FavoriteButtonClick::Add => {
                        if let Some(folder) = favorite_target.clone() {
                            let default_name = folder
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .to_string();
                            self.fav_add_name_input = default_name;
                            self.fav_add_target = Some(folder);
                            self.show_fav_add_dialog = true;
                        }
                    }
                    FavoriteButtonClick::Edit => {
                        self.show_favorites_editor = true;
                    }
                    FavoriteButtonClick::None => {}
                }
                if stack_toggle {
                    self.toggle_stack_mode();
                }
                if subfolder_expansion_toggle {
                    self.toggle_subfolder_expansion_view();
                }
                if let Some(forward) = tree_nav {
                    // 検索中は ▲▼ を検索仮想階層の前後ヒット移動に振り分ける
                    // (キーボード Ctrl+↑↓ と同じ分岐)。最上位では ▲▼ が disabled な
                    // ので通常ここには来ないが、防御的に各ナビ関数も空なら no-op。
                    // ★固定 中は snapshot 内 container 巡回 (Codex P2-1)。
                    if self.is_snapshot_active() {
                        let _ = self.snapshot_navigate_grid(forward);
                    } else if self.global_search.active && self.global_search.drill.is_some() {
                        self.global_search_ctrl_nav(forward);
                    } else if self.favsearch.active && !self.favsearch.nav_stack.is_empty() {
                        self.favsearch_ctrl_nav(forward);
                    } else if self.zip_nav_handle_ctrl_updown(forward) {
                        // ネスト ZIP 内: ツリーを DFS で前後のノードへ (#4 改)。
                    } else if let Some(cur) = effective_folder.clone() {
                        self.start_folder_nav(cur, forward, crate::app::FolderNavMode::Grid);
                    }
                }
                // 検索中の ⬆ ボタン: 検索仮想階層を 1 段ドリルアップ (BS と同じ動作)。
                if search_drill_up {
                    if self.global_search.active {
                        self.drill_back_one_level();
                    } else if self.favsearch.active {
                        self.favsearch_back();
                    } else if self.tag_view.active {
                        self.tag_view_back();
                    }
                }
                result
            })
            .inner
    }

    // ── 検索バー ─────────────────────────────────────────────────────

    /// メタデータ検索バーを描画する。
    pub(crate) fn render_search_bar(&mut self, ctx: &egui::Context) {
        if !self.show_search_bar {
            return;
        }

        // §4.1.1: PDF のページ表示中は Ctrl+F を無効化する。バーが開いたまま PDF を
        // 開いた場合はここで閉じる (ショートカット側は app.rs で抑止済み)。
        if self.grid_is_pdf_pages() {
            self.show_search_bar = false;
            self.search_query.clear();
            self.search_filter = None;
            self.search_filter_origin_folder = None;
            self.search_has_focus = false;
            self.search_tag_bridge.clear();
            self.cancel_search_pending();
            self.rebuild_visible_indices();
            return;
        }

        // §4.1.2: ZIP 表示中は検索対象をファイル名フィルタに固定する。
        let zip_view = self.grid_is_zip_entries();

        // Enter は **raw** で読む。`dialog_enter_pressed` は IME 変換直後 300ms の
        // グレース中も false を返すため、日本語で「おはよう[Enter]」と確定兼送信した
        // ケースで検索が走らず、代わりにグリッドの Enter ショートカット (フルスクリーン)
        // が走ってしまう。ここでは `response.lost_focus()` が Tab / クリック外しでも
        // true になる性質を raw Enter との AND で打ち消し、"Enter でフォーカスを失った"
        // ときだけ execute_search を呼ぶ。search_query は IME Commit で既に確定済み。
        let raw_enter_pressed = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let local_search_label = self
            .keymap
            .first_chord_action_label("現在地フィルタ", KeyAction::GlobalLocalSearch);
        egui::TopBottomPanel::top("search_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label("検索:").on_hover_text(format!(
                    "{local_search_label}: 今開いているフォルダ / ZIP の表示中\n\
                     アイテムを名前やメタ情報で絞り込みます (索引不要・再帰なし)。"
                ));
                let mut output = egui::TextEdit::singleline(&mut self.search_query)
                    .hint_text(r#"現在地のアイテムを名前やメタ情報で絞り込み (AND / -除外 / "…")"#)
                    .desired_width(320.0)
                    .min_size(egui::vec2(320.0, 20.0))
                    .show(ui);
                let _menu_changed = crate::ui_helpers::singleline_text_edit_context_menu(
                    ui,
                    &mut output,
                    &mut self.search_query,
                );
                let response = output.response;

                // フォーカスリクエスト
                if self.search_focus_request {
                    self.search_focus_request = false;
                    response.request_focus();
                }

                // フォーカス状態を追跡
                self.search_has_focus = response.has_focus();

                // Enter で検索実行 (IME 変換確定の Enter も同じ扱い)
                if response.lost_focus() && raw_enter_pressed {
                    self.execute_search(ctx);
                    // フォーカスを外してカーソルキーでグリッド操作できるようにする
                    response.surrender_focus();
                    self.search_has_focus = false;
                }

                // × ボタン
                if ui.small_button("×").hover_tip("検索を閉じる").clicked() {
                    self.cancel_pending_folder_nav();
                    self.show_search_bar = false;
                    self.search_query.clear();
                    self.search_filter = None;
                    self.search_filter_origin_folder = None;
                    self.search_has_focus = false;
                    self.search_tag_bridge.clear();
                    self.cancel_search_pending();
                    self.rebuild_visible_indices();
                    self.refresh_color_filter_for_scope_change(ctx);
                }

                // ── 検索対象ドロップダウン (§19.7) ──
                if zip_view {
                    // §4.1.2: ZIP 内はファイル名フィルタ固定。ドロップダウンは無効化
                    // して「ファイル名」を表示する (メタ系を選んでも無反応な
                    // 分かりにくさを防ぐ)。
                    ui.add_enabled_ui(false, |ui| {
                        egui::ComboBox::from_id_salt("ctrl_f_search_target")
                            .selected_text("ファイル名")
                            .width(160.0)
                            .show_ui(ui, |_ui| {});
                    });
                } else {
                    let current =
                        crate::global_search_ui::TargetChoice::from_target(&self.search_target);
                    let mut next = current;
                    egui::ComboBox::from_id_salt("ctrl_f_search_target")
                        .selected_text(current.label())
                        .width(160.0)
                        .show_ui(ui, |ui| {
                            for &choice in crate::global_search_ui::TARGET_CHOICES {
                                ui.selectable_value(&mut next, choice, choice.label());
                            }
                        });
                    if next != current {
                        self.search_target = next.to_target();
                        // クエリが空でなければ即再検索
                        if !self.search_query.trim().is_empty() {
                            self.execute_search(ctx);
                        }
                    }
                }

                if crate::ui_helpers::or_mode_checkbox(ui, &mut self.search_or_mode)
                    && !self.search_query.trim().is_empty()
                {
                    self.execute_search(ctx);
                }

                // Esc で検索解除（ダイアログが開いていない場合のみ。IME 変換中もスキップ）
                if !self.any_dialog_open() && escape_pressed {
                    self.cancel_pending_folder_nav();
                    self.show_search_bar = false;
                    self.search_query.clear();
                    self.search_filter = None;
                    self.search_filter_origin_folder = None;
                    self.search_has_focus = false;
                    self.search_tag_bridge.clear();
                    self.cancel_search_pending();
                    self.rebuild_visible_indices();
                    self.refresh_color_filter_for_scope_change(ctx);
                }

                // 検索中インジケータ or マッチ件数 (separator の後に表示)
                if let Some(pending) = self.search_pending.as_ref() {
                    ui.separator();
                    let progress = pending.progress_snapshot();
                    let text = if progress.total == 0 {
                        "検索中...".to_string()
                    } else {
                        format!("検索中 {}/{} 件", progress.done, progress.total)
                    };
                    let response = ui.label(
                        egui::RichText::new(text)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(180, 180, 80)),
                    );
                    if progress.total > 0 {
                        response.on_hover_text(format!(
                            "ヒット {} 件 / 確認済み {} 件 / 全 {} 件",
                            progress.matched, progress.done, progress.total
                        ));
                    }
                } else if let Some(ref filter) = self.search_filter {
                    ui.separator();
                    // 構造アイテム (Folder/ZIP/PDF) も一貫して絞れるようになったので
                    // (§4.1)、可視マッチ全体を「X/Y 件」で数える。グループ見出しの
                    // separator は件数に含めない。
                    let countable = |it: &crate::grid_item::GridItem| {
                        !matches!(it, crate::grid_item::GridItem::ZipSeparator { .. })
                    };
                    let total = self.items.iter().filter(|it| countable(it)).count();
                    let matched = filter
                        .iter()
                        .filter(|&&i| self.items.get(i).is_some_and(|it| countable(it)))
                        .count();
                    ui.label(
                        egui::RichText::new(format!("{matched}/{total} 件"))
                            .size(11.0)
                            .color(egui::Color32::from_gray(140)),
                    );
                }
            });
            // タグ橋渡しチップ (D17): クエリ語が mIV タグに一致したら、タグビューへの
            // ワンクリック導線を出す (Ctrl+G と同等。タグ自体は Ctrl+F の検索対象外)。
            let mut open_tag_bridge: Option<String> = None;
            if !self.search_tag_bridge.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(44.0);
                    ui.label(
                        egui::RichText::new("タグ候補:")
                            .size(11.0)
                            .color(egui::Color32::from_gray(150)),
                    )
                    .on_hover_text(
                        "mIV タグは Ctrl+F の絞り込み対象には混ぜません。\n\
                         ここからタグビューを開いて tags.db のタグ付き項目を表示できます。",
                    );
                    for suggestion in self.search_tag_bridge.clone() {
                        let label = format!("#{} ({}件)", suggestion.tag, suggestion.count);
                        if ui
                            .small_button(label)
                            .on_hover_text("タグビューで表示")
                            .clicked()
                        {
                            open_tag_bridge = Some(suggestion.tag);
                        }
                    }
                });
            }
            ui.add_space(2.0);
            if let Some(tag) = open_tag_bridge {
                // open_tag_view_with_query が close_other_search_bars で Ctrl+F バーを
                // 閉じる (相互排他) ので、ここでの後始末は不要。
                self.open_tag_view_for_tag(&tag);
            }
        });
    }

    /// お気に入り検索バー (ツールバー直下の 2 行目) を描画する。
    /// `favsearch.active` が true のときだけ表示される。
    pub(crate) fn render_favsearch_bar(&mut self, ctx: &egui::Context) {
        if !self.favsearch.active {
            return;
        }
        // Ctrl+F 側と同じく raw Enter で判定する (IME 変換確定の Enter も送信扱い)。
        // `response.lost_focus()` と AND することで Tab / クリック外しの誤発火は弾ける。
        let raw_enter_pressed = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let escape_pressed = self.dialog_escape_pressed(ctx);

        let mut close_requested = false;
        let mut query_changed = false;

        egui::TopBottomPanel::top("favsearch_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label("コンテナ検索:");
                let mut output = egui::TextEdit::singleline(&mut self.favsearch.query)
                    .hint_text(r#"フォルダ・ZIP・PDF をコンテナ名で探す (AND / -除外 / "…")"#)
                    .desired_width(320.0)
                    .min_size(egui::vec2(320.0, 20.0))
                    .show(ui);
                let menu_changed = crate::ui_helpers::singleline_text_edit_context_menu(
                    ui,
                    &mut output,
                    &mut self.favsearch.query,
                );
                let response = output.response;

                if self.favsearch.focus_request {
                    self.favsearch.focus_request = false;
                    response.request_focus();
                }
                self.favsearch.has_focus = response.has_focus();

                // 入力が変わるたびに即座に検索を再実行 (小規模 DB 前提)
                if response.changed() || menu_changed {
                    query_changed = true;
                }
                // Enter で確定的に再実行 (IME 変換確定の Enter も同じ扱い)
                if response.lost_focus() && raw_enter_pressed {
                    query_changed = true;
                }

                if ui.small_button("×").hover_tip("検索を閉じる").clicked() {
                    close_requested = true;
                }

                // ── お気に入り絞り込みドロップダウン (§19.7) ──
                // `auto_index_structure=true` のお気に入りのみ候補に出す (名前索引の対象と一致)。
                {
                    let current = self.favsearch.favorite_filter;
                    let label_for = |opt: Option<uuid::Uuid>| -> String {
                        match opt {
                            None => "すべてのお気に入り".to_string(),
                            Some(id) => self
                                .settings
                                .favorite_by_id(id)
                                .map(|f| f.name.clone())
                                .unwrap_or_else(|| "(削除済)".to_string()),
                        }
                    };
                    let mut next = current;
                    egui::ComboBox::from_id_salt("favsearch_fav")
                        .selected_text(label_for(current))
                        .width(140.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut next, None, "すべてのお気に入り");
                            for fav in &self.settings.favorites {
                                if !fav.auto_index_structure {
                                    continue;
                                }
                                ui.selectable_value(&mut next, Some(fav.id), &fav.name);
                            }
                        });
                    if next != current {
                        self.favsearch.favorite_filter = next;
                        // ドロップダウン変更は即再実行。クエリが空なら execute_favsearch が早期 return する。
                        query_changed = true;
                        // last_executed と現 query が一致していても再実行するよう last_executed を空に倒す。
                        self.favsearch.last_executed.clear();
                    }
                }

                // ── 種別フィルタ (§4.2) ──
                // コンテナ検索は フォルダ / ZIP / PDF が対象 (動画はコンテナではないので無し)。
                {
                    use crate::search_index_db::IndexKind;
                    let current = self.favsearch.kind_filter;
                    let label_for = |opt: Option<IndexKind>| -> &'static str {
                        match opt {
                            None => "すべての種別",
                            Some(IndexKind::Folder) => "フォルダ",
                            Some(IndexKind::ZipFile) => "ZIP",
                            Some(IndexKind::PdfFile) => "PDF",
                            Some(IndexKind::VideoFile) => "動画",
                        }
                    };
                    let mut next = current;
                    egui::ComboBox::from_id_salt("favsearch_kind")
                        .selected_text(label_for(current))
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            for choice in [
                                None,
                                Some(IndexKind::Folder),
                                Some(IndexKind::ZipFile),
                                Some(IndexKind::PdfFile),
                            ] {
                                ui.selectable_value(&mut next, choice, label_for(choice));
                            }
                        });
                    if next != current {
                        self.favsearch.kind_filter = next;
                        query_changed = true;
                        self.favsearch.last_executed.clear();
                    }
                }

                if crate::ui_helpers::or_mode_checkbox(ui, &mut self.favsearch.or_mode) {
                    query_changed = true;
                    self.favsearch.last_executed.clear();
                }

                if self.favsearch_pending.is_some() {
                    ui.separator();
                    ui.label(
                        egui::RichText::new("検索中...")
                            .size(11.0)
                            .color(egui::Color32::from_rgb(180, 180, 80)),
                    );
                } else if self.favsearch.on_results_grid() {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("{} 件", self.favsearch.results_paths.len()))
                            .size(11.0)
                            .color(egui::Color32::from_gray(140)),
                    );
                }
            });
            ui.add_space(2.0);
        });

        // Esc で閉じる (IME 変換中はスキップ、他のダイアログが開いていないときのみ)。
        // テキストボックスが Esc で focus を失ってから本チェックに達するため、
        // has_focus は要求せず active のみで判定する (Ctrl+F の検索バーと同じ挙動)。
        if !self.any_dialog_open() && escape_pressed {
            close_requested = true;
        }

        if close_requested {
            self.close_favsearch();
            return;
        }
        if query_changed && self.favsearch.query != self.favsearch.last_executed {
            self.execute_favsearch();
        }
    }

    pub(crate) fn tag_view_menu_sections(
        &self,
    ) -> (
        Vec<TagViewMenuChoice>,
        Vec<TagViewMenuChoice>,
        Vec<TagViewMenuChoice>,
    ) {
        const RECENT_LIMIT: usize = 20;
        const POPULAR_LIMIT: usize = 20;

        let summary_by_key: HashMap<&str, &crate::tags_db::TagSummary> = self
            .tag_view
            .summaries
            .iter()
            .map(|summary| (summary.tag_key.as_str(), summary))
            .collect();
        let display_names: HashMap<&str, &str> = self
            .settings
            .tags
            .iter()
            .map(|tag| (tag.tag_key.as_str(), tag.name.as_str()))
            .collect();
        let mut excluded: HashSet<String> = HashSet::new();

        let mut pinned = Vec::new();
        for tag in self.settings.tags.iter().filter(|tag| tag.show_shortcut) {
            if tag.tag_key.is_empty() || tag.name.trim().is_empty() {
                continue;
            }
            let count = summary_by_key
                .get(tag.tag_key.as_str())
                .map_or(0, |summary| summary.count);
            pinned.push(TagViewMenuChoice {
                name: tag.name.clone(),
                tag_key: tag.tag_key.clone(),
                count,
            });
            excluded.insert(tag.tag_key.clone());
        }
        pinned.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        let choice_for_summary = |summary: &crate::tags_db::TagSummary| -> TagViewMenuChoice {
            let name = display_names
                .get(summary.tag_key.as_str())
                .copied()
                .unwrap_or(summary.tag.as_str())
                .to_string();
            TagViewMenuChoice {
                name,
                tag_key: summary.tag_key.clone(),
                count: summary.count,
            }
        };

        let mut recent_source: Vec<_> = self.tag_view.summaries.iter().collect();
        recent_source.sort_by(|a, b| {
            b.last_applied_at
                .cmp(&a.last_applied_at)
                .then_with(|| a.tag.to_lowercase().cmp(&b.tag.to_lowercase()))
        });
        let mut recent = Vec::new();
        for summary in recent_source {
            if !excluded.insert(summary.tag_key.clone()) {
                continue;
            }
            recent.push(choice_for_summary(summary));
            if recent.len() >= RECENT_LIMIT {
                break;
            }
        }

        let mut popular_source: Vec<_> = self.tag_view.summaries.iter().collect();
        popular_source.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.tag.to_lowercase().cmp(&b.tag.to_lowercase()))
        });
        let mut popular = Vec::new();
        for summary in popular_source {
            if !excluded.insert(summary.tag_key.clone()) {
                continue;
            }
            popular.push(choice_for_summary(summary));
            if popular.len() >= POPULAR_LIMIT {
                break;
            }
        }

        (pinned, recent, popular)
    }

    /// タグビュー (Ctrl+T) の検索バーを描画する。
    pub(crate) fn render_tag_view_bar(&mut self, ctx: &egui::Context) {
        if !self.tag_view.active {
            self.tag_view.has_focus = false;
            return;
        }

        let raw_enter_pressed = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let escape_pressed = self.dialog_escape_pressed(ctx);

        let mut close_requested = false;
        let mut query_changed = false;
        let mut filter_changed = false;
        let mut clicked_tag: Option<String> = None;

        egui::TopBottomPanel::top("tag_view_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("タグビュー:");
                let mut output = egui::TextEdit::singleline(&mut self.tag_view.query)
                    .hint_text("#タグ")
                    .desired_width(260.0)
                    .min_size(egui::vec2(260.0, 20.0))
                    .show(ui);
                let menu_changed = crate::ui_helpers::singleline_text_edit_context_menu(
                    ui,
                    &mut output,
                    &mut self.tag_view.query,
                );
                let response = output.response;

                if self.tag_view.focus_request {
                    self.tag_view.focus_request = false;
                    response.request_focus();
                }
                self.tag_view.has_focus = response.has_focus();

                if response.changed() || menu_changed {
                    query_changed = true;
                }
                if response.lost_focus() && raw_enter_pressed {
                    response.request_focus();
                    self.tag_view.focus_request = true;
                    self.tag_view.has_focus = true;
                }

                let (pinned, recent, popular) = self.tag_view_menu_sections();
                ui.menu_button("一覧▼", |ui| {
                    ui.set_min_width(260.0);
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    draw_tag_view_menu_section(ui, "ピン留めしたタグ", &pinned, &mut clicked_tag);
                    ui.separator();
                    draw_tag_view_menu_section(ui, "最近使ったタグ", &recent, &mut clicked_tag);
                    ui.separator();
                    draw_tag_view_menu_section(ui, "数が多いタグ", &popular, &mut clicked_tag);
                });

                if ui
                    .small_button("×")
                    .hover_tip("タグビューを閉じる")
                    .clicked()
                {
                    close_requested = true;
                }

                {
                    let current = self.tag_view.kind_filter;
                    let mut next = current;
                    egui::ComboBox::from_id_salt("tag_view_kind")
                        .selected_text(current.label())
                        .width(140.0)
                        .show_ui(ui, |ui| {
                            for &choice in crate::tag_view::TAG_VIEW_KIND_FILTER_CHOICES {
                                ui.selectable_value(&mut next, choice, choice.label());
                            }
                        });
                    if next != current {
                        self.tag_view.kind_filter = next;
                        self.tag_view.last_executed.clear();
                        filter_changed = true;
                    }
                }

                if self.tag_view_pending.is_some() {
                    ui.separator();
                    ui.label(
                        egui::RichText::new("検索中...")
                            .size(11.0)
                            .color(egui::Color32::from_rgb(180, 180, 80)),
                    );
                } else if let Some(msg) = self.tag_view.reject_message.as_ref() {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(msg)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(190, 80, 80)),
                    );
                } else if !self.tag_view.last_executed.trim().is_empty() {
                    ui.separator();
                    let suffix = if self.tag_view.truncated {
                        " 件以上"
                    } else {
                        " 件"
                    };
                    ui.label(
                        egui::RichText::new(format!("{}{}", self.tag_view.result_count, suffix))
                            .size(11.0)
                            .color(egui::Color32::from_gray(140)),
                    );
                }
            });
            ui.add_space(2.0);
        });

        if let Some(tag) = clicked_tag {
            self.tag_view.query = tag;
            self.tag_view.last_executed.clear();
            query_changed = true;
        }

        if !self.any_dialog_open() && escape_pressed {
            close_requested = true;
        }

        if close_requested {
            self.close_tag_view();
            return;
        }
        if (query_changed || filter_changed)
            && (self.tag_view.query != self.tag_view.last_executed
                || self.tag_view.kind_filter != self.tag_view.last_executed_kind_filter)
        {
            self.execute_tag_view();
        }
    }

    // ── セルインタラクション ─────────────────────────────────────────

    /// グリッドセルのクリック・ダブルクリック・右クリックを処理する。
    /// ダブルクリックでフォルダに入る場合はそのパスを返す。
    fn handle_cell_interaction(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        cell_rect: egui::Rect,
        idx: usize,
    ) -> Option<PathBuf> {
        // click_and_drag: clicked() / double_clicked() / secondary_clicked() は従来通り
        // 発火しつつ、drag_started_by(Primary) で native ファイル D&D を開始できる。
        let response = ui.interact(cell_rect, ui.id().with(idx), egui::Sense::click_and_drag());
        let mut nav = None;
        if !self.any_dialog_open()
            && !self.items_are_drive_list
            && let Some(pos) = ctx.input(|i| {
                i.pointer
                    .secondary_pressed()
                    .then(|| i.pointer.interact_pos().or_else(|| i.pointer.latest_pos()))
                    .flatten()
            })
            && cell_rect.contains(pos)
        {
            match self
                .settings
                .ring_shortcuts
                .right_drag_mode(crate::ring_shortcut::RightDragContext::Grid)
            {
                crate::ring_shortcut::RightDragMode::RingShortcut => self.start_mouse_ring_flick(
                    ctx,
                    crate::ring_shortcut::RingShortcutContext::Grid,
                    pos,
                    Some(idx),
                ),
                crate::ring_shortcut::RightDragMode::MouseGesture => self.start_mouse_gesture(
                    ctx,
                    crate::ring_shortcut::RightDragContext::Grid,
                    pos,
                    Some(idx),
                ),
                crate::ring_shortcut::RightDragMode::Disabled
                | crate::ring_shortcut::RightDragMode::Unknown(_) => {}
            }
        }
        if response.clicked() || response.double_clicked() || response.secondary_clicked() {
            self.folder_pane.set_focus_grid();
        }
        let tag_badge_target = self.grid_tag_badge_target(ui, cell_rect, idx);
        if let Some((tag_name, badge_rect)) = tag_badge_target.as_ref() {
            if response.hovered()
                && ctx
                    .input(|i| i.pointer.hover_pos())
                    .is_some_and(|pos| badge_rect.contains(pos))
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                response
                    .clone()
                    .on_hover_text(format!("{tag_name} をタグビューで探す"));
            }
        }
        if response.clicked()
            && !ctx.input(|i| i.modifiers.ctrl || i.modifiers.shift)
            && let Some((tag_name, badge_rect)) = tag_badge_target
            && response
                .interact_pointer_pos()
                .is_some_and(|pos| badge_rect.contains(pos))
        {
            self.open_tag_view_for_tag(&tag_name);
            return None;
        }
        if response.clicked() {
            let ctrl = ctx.input(|i| i.modifiers.ctrl);
            let shift = ctx.input(|i| i.modifiers.shift);
            if shift {
                // Shift+クリック: 前回選択位置から現在位置までを範囲チェック
                if let Some(prev_sel) = self.selected {
                    let display_order = self.current_grid_order().to_vec();
                    let prev_pos = display_order
                        .iter()
                        .position(|&i| i == prev_sel)
                        .unwrap_or(0);
                    let cur_pos = display_order.iter().position(|&i| i == idx).unwrap_or(0);
                    let (start, end) = if prev_pos <= cur_pos {
                        (prev_pos, cur_pos)
                    } else {
                        (cur_pos, prev_pos)
                    };
                    for vp in start..=end {
                        if let Some(&vidx) = display_order.get(vp) {
                            if self.grid_item_can_be_checked(vidx) {
                                self.checked.insert(vidx);
                            }
                        }
                    }
                }
            } else if ctrl {
                // Ctrl+クリック: チェック ON/OFF トグル + 選択移動。
                // 初回 Ctrl+クリック (checked が空) のときは直前のカーソル位置も checked に
                // 入れる (エクスプローラ流「A 通常クリック → B Ctrl+クリックで A+B が選択」)。
                if self.checked.is_empty() {
                    if let Some(prev_sel) = self.selected {
                        if prev_sel != idx
                            && self.idx_visible(prev_sel)
                            && self.grid_item_can_be_checked(prev_sel)
                        {
                            self.checked.insert(prev_sel);
                        }
                    }
                }
                if self.grid_item_can_be_checked(idx) {
                    if self.checked.contains(&idx) {
                        self.checked.remove(&idx);
                    } else {
                        self.checked.insert(idx);
                    }
                }
            }
            self.selected = Some(idx);
            self.update_last_selected_image();
        }
        if response.double_clicked() && self.guard_reading_history_open(idx) {
            // 読書履歴ビューから本を開く場合は、閉じたときに読書履歴へ戻れるよう予約する。
            self.note_reading_history_open(idx);
            // ファイル名スタックの集約グリッドでメディアセルをダブルクリックしたら、フラット読書
            // フルスクリーンへ (スタック/単独画像/動画を直接開く)。コンテナは false で通常ナビへ。
            if self.stack_try_open_from_grid(idx, true) {
                return nav;
            }
            match self.items.get(idx) {
                Some(GridItem::Folder(p)) => {
                    // Ctrl+G 絞り込みビューでは「ヒットを含む子フォルダ」を Folder として
                    // 並べているので、通常の load_folder ではなく絞り込みをさらに 1 段潜る
                    // 経路に流す (docs §10.3 [3] 絞り込みビュー)。
                    if self.global_search.active && self.global_search.drill.is_some() {
                        self.drill_into_subfolder(p.clone());
                    } else {
                        let p = p.clone();
                        self.maybe_suppress_rating_filter_for_opened_container(idx);
                        self.maybe_suppress_facet_filter_for_opened_container(idx);
                        nav = Some(p);
                    }
                }
                Some(GridItem::ZipFile(p)) | Some(GridItem::PdfFile(p)) => {
                    // Folder 分岐とは global_search drill-in 判定が違うためここは別のまま。
                    let p = p.clone();
                    self.maybe_suppress_rating_filter_for_opened_container(idx);
                    self.maybe_suppress_facet_filter_for_opened_container(idx);
                    // 環境設定 ON なら、ページ一覧を経由せず 1 ページ目を即フルスクリーンで開く。
                    if self.settings.auto_fullscreen_zip_pdf {
                        self.pending_auto_fs_open = true;
                    }
                    nav = Some(p);
                }
                Some(GridItem::Image(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::ZipSeparator { .. })
                | Some(GridItem::PdfPage { .. })
                | Some(GridItem::Video(_)) => {
                    // 動画も画像と同じくフルスクリーン化 → VideoPlayer がインライン再生する。
                    // 外部プレイヤーで開きたい場合はフルスクリーン中の Shift+Enter または
                    // 右クリックメニューから (近日対応予定)。
                    // Phase 7.J: グリッドから明示的に開いたケースなので、
                    // 「一覧から開いたときだけ再生する」設定でも再生開始する。
                    self.bump_input_seq_for_item("grid_double_click", idx);
                    if matches!(self.items.get(idx), Some(GridItem::Video(_))) {
                        // Prevent the second click of the grid double-click from
                        // reaching the newly-opened fullscreen video and toggling
                        // playback back to paused.
                        self.fs_suppress_primary_until_release = true;
                        self.fs_focus_regained_at = Some(std::time::Instant::now());
                    }
                    self.fs_open_intent_from_grid = true;
                    // P10-1 follow-up: grid_action_open は Enter (および双クリック) からも
                    // 呼ばれる。Enter 経路では `handle_fullscreen_root_key_input` が同フレームで
                    // Enter を `consume_key` で拾って即 close する事故を防ぐためのガード。
                    // ダブルクリック経路では Enter event がそもそも無いので no-op。
                    self.fs_suppress_enter_close_until_release = true;
                    self.open_fullscreen(idx);
                }
                Some(GridItem::ConvertibleArchive { path, format }) => {
                    let pf = path.clone();
                    let fmt = *format;
                    let auto_fs = self.settings.auto_fullscreen_zip_pdf;
                    let search_rollback = if self.favsearch.active || self.tag_view.active {
                        Some(self.folder_nav_history_snapshot())
                    } else {
                        None
                    };
                    if self.favsearch.active {
                        self.favsearch.nav_stack.push(pf.clone());
                    }
                    if self.tag_view.active {
                        self.record_tag_view_nav_open(&pf);
                    }
                    self.maybe_suppress_rating_filter_for_opened_container(idx);
                    self.maybe_suppress_facet_filter_for_opened_container(idx);
                    let open_outcome = if self.settings.archive_file_handling_ignores_convertible()
                    {
                        self.show_feedback_toast(
                            "設定により RAR / 7z / LZH アーカイブを無視しています".into(),
                        );
                        crate::app::FolderOpenOutcome::Ignored
                    } else if let Some(cached) = self.try_archive_cache_lookup(&pf) {
                        if self.open_archive_via_cache(pf, cached, auto_fs) {
                            crate::app::FolderOpenOutcome::Loaded
                        } else {
                            crate::app::FolderOpenOutcome::Ignored
                        }
                    } else if self.request_archive_convert(pf, fmt, auto_fs) {
                        crate::app::FolderOpenOutcome::ConversionDialogOpened
                    } else {
                        crate::app::FolderOpenOutcome::Ignored
                    };
                    match (open_outcome, search_rollback) {
                        (crate::app::FolderOpenOutcome::ConversionDialogOpened, Some(snapshot)) => {
                            self.attach_archive_convert_nav_history_rollback(snapshot);
                        }
                        (crate::app::FolderOpenOutcome::Ignored, Some(snapshot)) => {
                            self.restore_folder_nav_history(snapshot);
                        }
                        _ => {}
                    }
                    if self.favsearch.active
                        && matches!(open_outcome, crate::app::FolderOpenOutcome::Loaded)
                    {
                        self.update_favsearch_address();
                    }
                    if self.tag_view.active
                        && matches!(open_outcome, crate::app::FolderOpenOutcome::Loaded)
                    {
                        self.update_tag_view_address();
                    }
                }
                Some(GridItem::SearchContainer { path, kind, .. }) => {
                    // Ctrl+G 結果ビューのコンテナ: ダブルクリックで drill-down view に遷移
                    // (docs §10.3 [3] 絞り込みビュー)
                    let p = path.clone();
                    let is_zip = matches!(kind, crate::grid_item::SearchContainerKind::Zip);
                    // ★コンテナを開いた時の中身空表示対策 (Codex P2)
                    self.maybe_suppress_rating_filter_for_opened_container_path(&p);
                    self.maybe_suppress_facet_filter_for_opened_container_path(&p);
                    self.drill_into_container(p, is_zip);
                }
                // レーティング一覧に復元された ZipDir は zip_nav を持たないので、
                // 外側 ZIP を開いてから該当 prefix へ入る。
                Some(GridItem::ZipDir {
                    zip_path,
                    dir_prefix,
                    ..
                }) if self.items_are_rating_view => {
                    self.open_rating_view_zipdir(zip_path.clone(), dir_prefix.clone());
                }
                // ネスト ZIP ツリーの子コンテナへダブルクリックで降りる (Phase 3)。
                Some(GridItem::ZipDir { dir_prefix, .. }) => {
                    let dp = dir_prefix.clone();
                    // ★付きの本を絞り込み中に開くと中身が空表示になるのを防ぐ
                    // (Codex P2)。enter 前に抑制を仕込む。
                    self.maybe_suppress_rating_filter_for_opened_zip_book(idx);
                    self.maybe_suppress_facet_filter_for_opened_zip_book(idx);
                    self.zip_nav_enter(&dp);
                }
                // ファイル名スタック (v2.0.0): 集約グリッドのセルは上の stack_try_open_from_grid
                // で処理済み (フラットフルスクリーンへ)。非スタックモードでは Stack セルは存在
                // しないので網羅性のため no-op。
                Some(GridItem::Stack { .. }) => {}
                None => {}
            }
        }
        // 右クリック → コンテキストメニュー
        if !self.items_are_drive_list
            && response.secondary_clicked()
            && !self.mouse_ring_context_menu_suppressed(ctx)
        {
            self.selected = Some(idx);
            self.update_last_selected_image();
            self.context_menu_idx = Some(idx);
            self.context_menu_pos = ctx.input(|i| i.pointer.interact_pos().unwrap_or_default());
        }

        // ── native ファイル D&D の開始検出 (docs/file-drag-drop-design.md §5.4) ──
        // primary (左) ボタンのドラッグだけを起点にする。native drag 直後の
        // 1 フレームは抑止 (幽霊ドラッグ防止の保険、§6.1)。
        if !self.items_are_drive_list
            && !self.native_drag_just_finished
            && response.drag_started_by(egui::PointerButton::Primary)
        {
            match decide_drag_payload(&self.items, &self.checked, idx) {
                DragDecision::Start {
                    paths,
                    post_drag_toast,
                } => {
                    self.pending_native_drag = Some(crate::app::PendingNativeDrag {
                        paths,
                        post_drag_toast,
                    });
                }
                DragDecision::ImmediateToast(msg) => self.show_feedback_toast(msg),
                DragDecision::None => {}
            }
        }
        nav
    }

    fn grid_tag_badge_target(
        &self,
        ui: &egui::Ui,
        cell_rect: egui::Rect,
        idx: usize,
    ) -> Option<(String, egui::Rect)> {
        if self.items_are_drive_list {
            return None;
        }
        let tags = self.cell_tag_list(idx);
        let tag_name = crate::app::primary_grid_tag_for_badge(tags)?.to_owned();
        let badges = self.grid_edit_badges(idx);
        let badge_rect = crate::app::grid_tag_badge_hit_rect(
            ui,
            cell_rect,
            badges.page_override,
            badges.local_adjust,
            badges.mask,
            badges.conceal,
            badges.comic,
            self.cell_has_pin_badge(idx),
            tags,
        )?;
        Some((tag_name, badge_rect))
    }

    fn cell_has_pin_badge(&self, idx: usize) -> bool {
        if let (Some(pin_container), Some(src)) = (
            self.pin_container_key(),
            self.folder_pin_selected_source(idx),
        ) {
            let key = crate::path_key::normalize_keep_drive(&pin_container);
            self.folder_pin_map.get(&key) == Some(&src)
        } else {
            false
        }
    }

    /// 現在表示中の場所に対する空白右クリックメニューを開く。
    fn open_current_folder_context_menu(&mut self, ctx: &egui::Context) {
        let pos = ctx.input(|i| i.pointer.interact_pos().unwrap_or_default());
        self.open_current_folder_context_menu_at(ctx, pos);
    }

    fn open_current_folder_context_menu_at(&mut self, ctx: &egui::Context, pos: egui::Pos2) {
        if self.current_folder.is_some() {
            self.context_menu_idx = Some(usize::MAX);
            self.context_menu_pos = pos;
            ctx.request_repaint();
        }
    }

    fn start_grid_background_mouse_ring_flick_if_pressed(
        &mut self,
        ctx: &egui::Context,
        rect: egui::Rect,
    ) {
        if self.any_dialog_open()
            || self.items_are_drive_list
            || self.mouse_ring_flick.is_some()
            || self.mouse_gesture.is_some()
        {
            return;
        }
        if let Some(pos) = ctx.input(|i| {
            i.pointer
                .secondary_pressed()
                .then(|| i.pointer.interact_pos().or_else(|| i.pointer.latest_pos()))
                .flatten()
        }) && rect.contains(pos)
        {
            match self
                .settings
                .ring_shortcuts
                .right_drag_mode(crate::ring_shortcut::RightDragContext::Grid)
            {
                crate::ring_shortcut::RightDragMode::RingShortcut => self.start_mouse_ring_flick(
                    ctx,
                    crate::ring_shortcut::RingShortcutContext::Grid,
                    pos,
                    None,
                ),
                crate::ring_shortcut::RightDragMode::MouseGesture => self.start_mouse_gesture(
                    ctx,
                    crate::ring_shortcut::RightDragContext::Grid,
                    pos,
                    None,
                ),
                crate::ring_shortcut::RightDragMode::Disabled
                | crate::ring_shortcut::RightDragMode::Unknown(_) => {}
            }
        }
    }

    fn update_grid_mouse_ring_flick(&mut self, ctx: &egui::Context) {
        if self.any_dialog_open() {
            self.cancel_mouse_ring_flick();
            self.cancel_mouse_gesture();
            return;
        }
        match self.update_mouse_ring_flick(ctx, crate::ring_shortcut::RingShortcutContext::Grid) {
            crate::ring_shortcut::MouseFlickOutcome::ShortTap => {
                let target_idx = self.mouse_ring_grid_target_idx.take();
                self.open_grid_right_drag_short_tap_menu(ctx, target_idx);
            }
            crate::ring_shortcut::MouseFlickOutcome::Cancelled
            | crate::ring_shortcut::MouseFlickOutcome::Fired
            | crate::ring_shortcut::MouseFlickOutcome::None => {}
        }
        match self.update_mouse_gesture(ctx, crate::ring_shortcut::RightDragContext::Grid) {
            crate::ring_shortcut::MouseFlickOutcome::ShortTap => {
                let target_idx = self.mouse_gesture_grid_target_idx.take();
                self.open_grid_right_drag_short_tap_menu(ctx, target_idx);
            }
            crate::ring_shortcut::MouseFlickOutcome::Cancelled
            | crate::ring_shortcut::MouseFlickOutcome::Fired
            | crate::ring_shortcut::MouseFlickOutcome::None => {}
        }
    }

    fn open_grid_right_drag_short_tap_menu(
        &mut self,
        ctx: &egui::Context,
        target_idx: Option<usize>,
    ) {
        let pos = ctx.input(|i| i.pointer.interact_pos().unwrap_or_default());
        if self.context_menu_idx.is_some() {
            return;
        }
        if let Some(idx) = target_idx
            && idx < self.items.len()
            && !self.items_are_drive_list
        {
            self.selected = Some(idx);
            self.update_last_selected_image();
            self.context_menu_idx = Some(idx);
            self.context_menu_pos = pos;
            ctx.request_repaint();
        } else {
            self.open_current_folder_context_menu_at(ctx, pos);
        }
    }
    // ── 詳細リスト ───────────────────────────────────────────────────

    const DETAILS_HEADER_H: f32 = 26.0;
    const DETAILS_ROW_H: f32 = 28.0;

    fn render_details_list(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        scroll_to: bool,
        spread_pair_cursor_idx: Option<usize>,
    ) -> Option<PathBuf> {
        let avail_w = ui.available_width().max(1.0);
        let avail_h = ui.available_height().max(0.0);

        if self.details_order.len() != self.visible_indices.len() {
            self.rebuild_details_order();
        }
        let display_order = self.current_grid_order().to_vec();
        let row_count = display_order.len();
        let natural_h = row_count as f32 * Self::DETAILS_ROW_H;

        // 縦スクロールバーが出るときだけその幅 (gutter) を右側へ確保する。これを見込まないと、
        // 内側の縦スクロール (solid) が gutter ぶん外形を広げ、ヘッダ (縦バー外) と行 (縦バー内) が
        // ずれて右端列が欠け、不要な横スクロールバーまで出る。viewport はヘッダ + 行間を引いた概算で
        // 判定し、境界帯では「gutter を多めに確保」側へ倒す (= 列が欠けるより無害)。
        // 横方向にあふれる (固定名前列 / 広い列) と外側に水平スクロールバーが出る。それが solid 設定だと
        // 内側縦ビューポートの高さを削るので、その分も概算から引く (既定 floating では allocated=0 なので無影響)。
        let fixed_cols_w = details_fixed_columns_width(&self.settings);
        let name_w_unconstrained = if self.settings.details_name_width_auto {
            (avail_w - fixed_cols_w).max(DetailsColumn::Name.default_width())
        } else {
            details_name_fixed_width(&self.settings)
        };
        let h_overflow = name_w_unconstrained + fixed_cols_w > avail_w + 0.5;
        let hbar = if h_overflow {
            ui.spacing().scroll.allocated_width()
        } else {
            0.0
        };
        // 危険なのは「出ないと予測したのに egui が出す」側 (= 上記バグが再発) だけ。境界では gutter を
        // 多めに確保する方へ倒す (余分に取っても右に空き帯が出るだけで無害) ため概算を少し小さめにする。
        let viewport_h_est =
            (avail_h - Self::DETAILS_HEADER_H - ui.spacing().item_spacing.y - hbar - 2.0).max(0.0);
        let needs_vscroll = natural_h > viewport_h_est;
        let gutter = if needs_vscroll {
            egui::style::ScrollStyle::solid().allocated_width()
        } else {
            0.0
        };
        let layout = details_layout(avail_w, gutter, &self.settings);
        let content_w = layout.pane_w;
        self.last_details_name_width = layout.name_w;

        if (content_w - self.last_cell_size).abs() > 0.5
            || (Self::DETAILS_ROW_H - self.last_cell_h).abs() > 0.5
        {
            self.last_cell_size = content_w;
            self.last_cell_h = Self::DETAILS_ROW_H;
        }

        let mut nav: Option<PathBuf> = None;
        let mut body_inner_rect = egui::Rect::NOTHING;
        let mut egui_offset_y = self.scroll_offset_y;
        let mut hovered_preview: Option<(usize, egui::Rect)> = None;
        egui::ScrollArea::horizontal()
            .id_salt("details_list_horizontal")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // 外側コンテンツ幅 = pane + gutter。pane より広くしておくことで内側縦スクロールの
                // gutter が pane の右外に収まり、ヘッダ・行の列が揃う。
                ui.set_min_width(layout.extent);
                let (header_rect, _) = ui.allocate_exact_size(
                    egui::vec2(content_w, Self::DETAILS_HEADER_H),
                    egui::Sense::hover(),
                );
                self.draw_details_header(ui, header_rect);

                let viewport_h = ui.available_height().max(0.0);
                self.last_viewport_h = viewport_h;
                if scroll_to {
                    self.apply_scroll_to_selected(1, Self::DETAILS_ROW_H);
                }
                let (total_h, max_offset) =
                    snapped_scroll_extent(natural_h, viewport_h, Self::DETAILS_ROW_H);
                self.scroll_offset_y = self.scroll_offset_y.clamp(0.0, max_offset);

                let old_scroll_style = ui.spacing().scroll;
                ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
                let scroll_output = egui::ScrollArea::vertical()
                    .id_salt("details_list_vertical")
                    .auto_shrink([false, false])
                    .vertical_scroll_offset(self.scroll_offset_y)
                    .show_viewport(ui, |ui, viewport| {
                        self.last_viewport_h = viewport.height();

                        let (content_rect, _) = ui.allocate_exact_size(
                            egui::vec2(content_w, total_h),
                            egui::Sense::hover(),
                        );

                        let first_row = (viewport.min.y / Self::DETAILS_ROW_H) as usize;
                        let last_row =
                            ((viewport.max.y / Self::DETAILS_ROW_H) as usize + 2).min(row_count);
                        let prewarm_first = first_row.saturating_sub(8);
                        let prewarm_last = (last_row + 8).min(row_count);
                        self.details_tag_prewarm_indices.clear();
                        self.details_tag_prewarm_indices.extend(
                            display_order
                                .get(prewarm_first..prewarm_last)
                                .unwrap_or(&[])
                                .iter()
                                .copied(),
                        );

                        let vis_first_idx = display_order.get(first_row).copied().unwrap_or(0);
                        self.scroll_hint.store(vis_first_idx, Ordering::Relaxed);
                        let vis_end_idx = display_order
                            .get(last_row.saturating_sub(1).min(row_count.saturating_sub(1)))
                            .copied()
                            .map(|i| i + 1)
                            .unwrap_or(vis_first_idx);
                        self.visible_end_shared
                            .store(vis_end_idx, Ordering::Relaxed);

                        for row in first_row..last_row {
                            let Some(&idx) = display_order.get(row) else {
                                continue;
                            };
                            let row_rect = egui::Rect::from_min_size(
                                content_rect.min
                                    + egui::vec2(0.0, row as f32 * Self::DETAILS_ROW_H),
                                egui::vec2(content_w, Self::DETAILS_ROW_H),
                            );

                            if let Some(n) = self.handle_cell_interaction(ui, ctx, row_rect, idx) {
                                nav = Some(n);
                            }
                            if idx >= self.items.len() {
                                break;
                            }

                            if let Some(preview_rect) = self.draw_details_row(
                                ui,
                                row_rect,
                                idx,
                                row,
                                spread_pair_cursor_idx == Some(idx),
                            ) {
                                hovered_preview = Some((idx, preview_rect));
                            }
                            if self.selected == Some(idx) {
                                self.selected_cell_rect = Some(row_rect);
                            }
                        }
                    });
                ui.spacing_mut().scroll = old_scroll_style;
                body_inner_rect = scroll_output.inner_rect;
                egui_offset_y = scroll_output.state.offset.y;
            });

        self.start_grid_background_mouse_ring_flick_if_pressed(ctx, body_inner_rect);
        self.update_grid_mouse_ring_flick(ctx);

        let bg_right_clicked = ui.rect_contains_pointer(body_inner_rect)
            && ctx.input(|i| i.pointer.secondary_clicked());
        if bg_right_clicked
            && self.context_menu_idx.is_none()
            && !self.mouse_ring_context_menu_suppressed(ctx)
        {
            self.open_current_folder_context_menu(ctx);
        }
        self.clear_mouse_ring_context_menu_suppression_if_idle(ctx);

        if (egui_offset_y - self.scroll_offset_y).abs() > Self::DETAILS_ROW_H * 0.5 {
            self.scroll_offset_y =
                (egui_offset_y / Self::DETAILS_ROW_H).round() * Self::DETAILS_ROW_H;
        }

        let full_rect = ui.max_rect();
        self.draw_mouse_ring_flick_overlay(
            ui,
            full_rect,
            crate::ring_shortcut::RingShortcutContext::Grid,
        );
        self.draw_mouse_gesture_overlay(
            ui,
            full_rect,
            crate::ring_shortcut::RightDragContext::Grid,
        );
        self.draw_gamepad_ring_overlay(ui, full_rect);
        self.draw_gamepad_picker_overlay(ui, full_rect);
        self.draw_gamepad_favorite_picker_overlay(ui, full_rect);
        self.draw_gamepad_location_picker_overlay(ui, full_rect);
        self.draw_feedback_toast(ui, full_rect, ctx);
        self.render_details_thumbnail_tooltip(ctx, hovered_preview);

        nav
    }

    fn render_details_thumbnail_tooltip(
        &mut self,
        ctx: &egui::Context,
        hovered_preview: Option<(usize, egui::Rect)>,
    ) {
        let viewport_id = egui::ViewportId::from_hash_of("details_thumbnail_tooltip");
        if self.items_are_drive_list || hovered_preview.is_none() {
            self.set_details_hover_thumbnail_idx(None);
            if self.details_hover_thumb_viewport_open {
                ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Close);
                self.details_hover_thumb_viewport_open = false;
            }
            return;
        }
        let Some((idx, anchor_rect)) = hovered_preview else {
            return;
        };

        self.set_details_hover_thumbnail_idx(Some(idx));

        let texture = match self.thumbnails.get(idx) {
            Some(ThumbnailState::Loaded { tex, .. }) => Some(tex.clone()),
            _ => None,
        };
        let failed = matches!(self.thumbnails.get(idx), Some(ThumbnailState::Failed));
        let image_size = texture
            .as_ref()
            .map(|tex| {
                let tex_size = tex.size_vec2();
                let max_side = 320.0;
                if tex_size.x <= 0.0 || tex_size.y <= 0.0 {
                    egui::vec2(180.0, 120.0)
                } else {
                    let scale = (max_side / tex_size.x).min(max_side / tex_size.y).min(1.0);
                    egui::vec2(tex_size.x * scale, tex_size.y * scale)
                }
            })
            .unwrap_or_else(|| egui::vec2(180.0, 120.0));
        let padding = 8.0;
        let viewport_size = image_size + egui::vec2(padding * 2.0, padding * 2.0);
        let anchor_screen_rect = self.details_anchor_screen_rect(ctx, anchor_rect);
        let pos = self.details_thumbnail_tooltip_pos(ctx, anchor_screen_rect, viewport_size);
        let loaded = texture.is_some();

        let builder = egui::ViewportBuilder::default()
            .with_title("mimageviewer preview")
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(false)
            .with_taskbar(false)
            .with_always_on_top()
            .with_mouse_passthrough(true)
            .with_position(pos)
            .with_inner_size(viewport_size)
            .with_min_inner_size(viewport_size)
            .with_max_inner_size(viewport_size)
            .with_visible(true);

        ctx.show_viewport_immediate(viewport_id, builder, move |vp_ctx, _class| {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(vp_ctx.style().visuals.extreme_bg_color)
                        .stroke(egui::Stroke::new(
                            1.0,
                            vp_ctx
                                .style()
                                .visuals
                                .widgets
                                .noninteractive
                                .bg_stroke
                                .color,
                        ))
                        .inner_margin(egui::Margin::same(padding as i8)),
                )
                .show(vp_ctx, |ui| {
                    let rect = ui.max_rect().shrink(padding);
                    if let Some(tex) = texture.as_ref() {
                        let img_rect = egui::Rect::from_center_size(rect.center(), image_size);
                        ui.painter().image(
                            tex.id(),
                            img_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    } else {
                        ui.centered_and_justified(|ui| {
                            ui.label(if failed {
                                "表示できません"
                            } else {
                                "読み込み中..."
                            });
                        });
                    }
                });
        });
        self.details_hover_thumb_viewport_open = true;
        if !loaded && !failed {
            ctx.request_repaint();
        }
    }

    fn details_anchor_screen_rect(
        &self,
        ctx: &egui::Context,
        anchor_rect: egui::Rect,
    ) -> egui::Rect {
        let origin = ctx
            .input(|i| i.viewport().inner_rect.map(|rect| rect.min))
            .or_else(|| self.last_outer_rect.map(|rect| rect.min))
            .unwrap_or(egui::Pos2::ZERO);
        egui::Rect::from_min_max(
            origin + anchor_rect.min.to_vec2(),
            origin + anchor_rect.max.to_vec2(),
        )
    }

    fn details_thumbnail_tooltip_pos(
        &self,
        ctx: &egui::Context,
        anchor_rect: egui::Rect,
        size: egui::Vec2,
    ) -> egui::Pos2 {
        let ppp = self.last_pixels_per_point.max(0.1);
        let monitor_rect = crate::monitor::get_monitor_logical_rect_at(
            anchor_rect.center().x * ppp,
            anchor_rect.center().y * ppp,
        )
        .or(self.last_outer_rect)
        .unwrap_or_else(|| egui::Rect::from_min_size(egui::Pos2::ZERO, ctx.content_rect().size()));

        let margin = 6.0;
        let gap = 8.0;
        let min_x = monitor_rect.left() + margin;
        let max_x = monitor_rect.right() - margin - size.x;
        let x = clamp_details_tooltip_axis(anchor_rect.center().x - size.x * 0.5, min_x, max_x);

        let above = anchor_rect.top() - gap - size.y;
        let below = anchor_rect.bottom() + gap;
        let min_y = monitor_rect.top() + margin;
        let max_y = monitor_rect.bottom() - margin - size.y;
        let preferred_y = if above >= min_y { above } else { below };
        let y = clamp_details_tooltip_axis(preferred_y, min_y, max_y);
        egui::pos2(x, y)
    }

    pub(crate) fn reading_history_entry_for_idx(
        &self,
        idx: usize,
    ) -> Option<&crate::reading_history_db::ReadingHistoryEntry> {
        if !self.items_are_reading_history_view {
            return None;
        }
        let key = self
            .items
            .get(idx)
            .and_then(crate::app::reading_history_key_for_item)?;
        self.reading_history_rows.get(&key)
    }

    /// 詳細表示「最終閲覧」列 (= 読書履歴ビューでは更新日時列を転用) の文字列。
    pub(crate) fn reading_history_last_read_for_idx(&self, idx: usize) -> Option<String> {
        let entry = self.reading_history_entry_for_idx(idx)?;
        reading_history_last_read_text(entry, self.settings.details_timestamp_show_seconds)
    }

    /// 詳細表示「既読位置」列 (= 読書履歴ビューでは状態列を転用) の文字列。
    pub(crate) fn reading_history_progress_for_idx(&self, idx: usize) -> Option<String> {
        let entry = self.reading_history_entry_for_idx(idx)?;
        reading_history_progress_text(entry)
    }

    fn reading_history_info_lines(
        &self,
        idx: usize,
        show_last_read: bool,
        show_progress: bool,
    ) -> Option<Vec<String>> {
        let entry = self.reading_history_entry_for_idx(idx)?;
        let mut lines = Vec::new();
        if show_last_read
            && let Some(last_read) =
                reading_history_last_read_text(entry, self.settings.details_timestamp_show_seconds)
        {
            lines.push(format!("最終閲覧 {last_read}"));
        }
        if show_progress && let Some(progress) = reading_history_progress_text(entry) {
            lines.push(format!("既読位置 {progress}"));
        }
        (!lines.is_empty()).then_some(lines)
    }

    pub(crate) fn reading_history_tooltip_lines(&self, idx: usize) -> Option<Vec<String>> {
        self.reading_history_info_lines(idx, true, true)
    }

    pub(crate) fn reading_history_selection_info_lines(&self, idx: usize) -> Option<Vec<String>> {
        self.reading_history_info_lines(
            idx,
            self.settings.thumb_tooltip_show_reading_history_last_read,
            self.settings.thumb_tooltip_show_reading_history_progress,
        )
    }

    fn draw_reading_history_tooltip(&self, ui: &mut egui::Ui, rect: egui::Rect, idx: usize) {
        let Some(entry) = self.reading_history_entry_for_idx(idx) else {
            return;
        };
        // 読書履歴は複数フォルダ / ドライブの本が混在するため、場所 (フルパス) を
        // 先頭に出して同名の本を判別できるようにする。
        let mut lines = vec![format!("場所 {}", entry.path.display())];
        if let Some(extra) = self.reading_history_tooltip_lines(idx) {
            lines.extend(extra);
        }
        let response = ui.interact(
            rect,
            ui.id().with(("reading_history_item_tooltip", idx)),
            egui::Sense::hover(),
        );
        response.on_hover_ui_at_pointer(|ui| {
            for line in lines {
                ui.label(line);
            }
        });
    }

    fn draw_details_header(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let bg = ui.visuals().extreme_bg_color;
        let stroke_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let text_color = ui.visuals().strong_text_color();
        let hover_bg = ui.visuals().widgets.hovered.bg_fill;
        let book_sort_locked =
            self.current_folder_is_book_folder() || self.items_are_reading_history_view;
        ui.painter().rect_filled(rect, 0.0, bg);
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            egui::Stroke::new(1.0, stroke_color),
        );

        let columns = details_column_rects(rect, &self.settings);
        let header_drag_id = ui.id().with("details_header_drag_state");
        for (col, col_rect) in columns.iter().copied() {
            let mut header_hit = col_rect;
            // 右端 6px は列幅リサイズ用に空けておく (名前列も固定幅化のためリサイズ可)。
            if header_hit.width() > 12.0 {
                header_hit.max.x -= 6.0;
            }
            let response = ui.interact(
                header_hit,
                ui.id().with(("details_header", col)),
                egui::Sense::click_and_drag(),
            );
            if response.hovered() {
                ui.painter().rect_filled(col_rect, 0.0, hover_bg);
            }
            let sort_key = col.sort_key();
            let lazy_sort = col.is_lazy();
            let sort_enabled = !book_sort_locked
                && sort_key.is_some()
                && (!lazy_sort || self.details_lazy_sort_ready());
            if response.clicked() && sort_enabled {
                if let Some(sort_key) = sort_key {
                    self.set_details_sort_key(sort_key);
                }
            }
            if response.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
            if response.drag_started_by(egui::PointerButton::Primary)
                && let Some(pos) = ui
                    .ctx()
                    .input(|i| i.pointer.interact_pos().or_else(|| i.pointer.latest_pos()))
            {
                let start = ui.ctx().input(|i| i.pointer.press_origin().unwrap_or(pos));
                ui.ctx().data_mut(|data| {
                    data.insert_temp(
                        header_drag_id,
                        Some(DetailsHeaderDrag {
                            column: col,
                            start,
                            latest: pos,
                        }),
                    )
                });
            }
            if response.dragged_by(egui::PointerButton::Primary)
                && let Some(pos) = ui
                    .ctx()
                    .input(|i| i.pointer.interact_pos().or_else(|| i.pointer.latest_pos()))
            {
                ui.ctx().data_mut(|data| {
                    let mut drag = data
                        .get_temp::<Option<DetailsHeaderDrag>>(header_drag_id)
                        .flatten()
                        .unwrap_or(DetailsHeaderDrag {
                            column: col,
                            start: pos,
                            latest: pos,
                        });
                    if drag.column == col {
                        drag.latest = pos;
                    }
                    data.insert_temp(header_drag_id, Some(drag));
                });
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                let stopped_drag = ui
                    .ctx()
                    .data_mut(|data| data.remove_temp::<Option<DetailsHeaderDrag>>(header_drag_id))
                    .flatten();
                if let Some(mut drag) = stopped_drag {
                    if let Some(pos) = ui.ctx().input(|i| i.pointer.latest_pos()) {
                        drag.latest = pos;
                    }
                    if finish_details_header_drag(&mut self.settings, &columns, drag, 12.0) {
                        crate::logger::log(format!(
                            "details header reorder: {:?} -> x {:.1}",
                            drag.column.id(),
                            drag.latest.x
                        ));
                        self.settings.save();
                        ui.ctx().request_repaint();
                    }
                }
            }
            let sorted = !book_sort_locked
                && sort_key.is_some_and(|sort_key| self.settings.details_sort_key == sort_key);
            let mut base_title = if self.items_are_reading_history_view {
                match col {
                    DetailsColumn::Modified => "最終閲覧".to_string(),
                    DetailsColumn::State => "既読位置".to_string(),
                    _ => col.title().to_string(),
                }
            } else {
                col.title().to_string()
            };
            if matches!(
                col,
                DetailsColumn::Created
                    | DetailsColumn::ImageDimensions
                    | DetailsColumn::VideoDuration
                    | DetailsColumn::VideoDimensions
                    | DetailsColumn::VideoCodec
            ) && matches!(
                self.details_image_dims_state,
                LazyColumnState::Loading { .. } | LazyColumnState::NotRequested
            ) {
                base_title.push_str(" ...");
            }
            let title = if sorted {
                format!(
                    "{} {}",
                    base_title,
                    if self.settings.details_sort_ascending {
                        "↑"
                    } else {
                        "↓"
                    }
                )
            } else {
                base_title
            };
            draw_details_text(
                ui,
                col_rect,
                &title,
                egui::Align2::LEFT_CENTER,
                text_color,
                true,
            );
            if col == DetailsColumn::Preview {
                draw_details_preview_icon(
                    ui.painter(),
                    col_rect.shrink2(egui::vec2(6.0, 4.0)),
                    text_color,
                    false,
                );
            }
            ui.painter().line_segment(
                [col_rect.right_top(), col_rect.right_bottom()],
                egui::Stroke::new(1.0, stroke_color),
            );
            {
                // リサイズのつかみ代は列の内側 (右端から左へ 8px) に置く。境界中央に置くと右半分が
                // 次列ヘッダ (click_and_drag) の当たり判定と重なり、ドラッグがソート/並べ替えに
                // 横取りされる。境界線の視覚インジケータは下で別途描くので操作感は保たれる。
                let resize_rect = egui::Rect::from_min_max(
                    egui::pos2(col_rect.right() - 8.0, col_rect.top()),
                    egui::pos2(col_rect.right(), col_rect.bottom()),
                );
                let resize_response = ui.interact(
                    resize_rect,
                    ui.id().with(("details_header_resize", col)),
                    egui::Sense::drag(),
                );
                if resize_response.hovered() || resize_response.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    ui.painter().line_segment(
                        [col_rect.right_top(), col_rect.right_bottom()],
                        egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
                    );
                }
                if resize_response.dragged() {
                    let changed = if col == DetailsColumn::Name {
                        // 名前列の境界ドラッグ = 自動調整をやめ、その幅を固定幅として保存する。
                        set_details_name_width(
                            &mut self.settings,
                            col_rect.width() + resize_response.drag_delta().x,
                        )
                    } else {
                        let current = details_column_width(&self.settings, col);
                        set_details_column_width(
                            &mut self.settings,
                            col,
                            current + resize_response.drag_delta().x,
                        )
                    };
                    if changed {
                        ui.ctx().request_repaint();
                    }
                }
                if resize_response.drag_stopped() {
                    self.settings.save();
                }
            }
            let response = if sort_enabled {
                response.hover_tip("クリックで 昇順 → 降順 → ソートなし")
            } else if book_sort_locked && sort_key.is_some() {
                response.hover_tip("本棚内または読書履歴では表示順が固定されます")
            } else if sort_key.is_none() {
                response.hover_tip("サムネイルプレビュー")
            } else {
                response.hover_tip("詳細情報の読み込み完了後に並べ替えできます")
            };
            response.context_menu(|ui| {
                self.draw_details_column_context_menu(ui);
            });
        }
    }

    fn draw_details_column_context_menu(&mut self, ui: &mut egui::Ui) {
        ui.label("表示する列");
        ui.separator();
        let mut name_visible = true;
        ui.add_enabled(false, egui::Checkbox::new(&mut name_visible, "名前"));

        // 名前列の幅: 既定は残り幅へ自動調整。OFF にすると現在の幅で固定し、横スクロールで
        // 全列を確認できる。境界ドラッグでも自動的に固定幅へ切り替わる。
        let mut name_auto = self.settings.details_name_width_auto;
        if ui
            .checkbox(&mut name_auto, "名前の幅を自動調整")
            .on_hover_text("OFF にすると現在の名前列幅で固定します (境界ドラッグでも固定されます)")
            .changed()
        {
            if name_auto {
                self.settings.details_name_width_auto = true;
                self.settings.details_name_width = DetailsColumn::Name.default_width();
            } else {
                self.settings.details_name_width_auto = false;
                self.settings.details_name_width = self
                    .last_details_name_width
                    .clamp(DetailsColumn::Name.min_width(), 800.0);
            }
            self.settings.save();
            ui.ctx().request_repaint();
        }

        let old_lazy = (
            self.settings.details_show_created,
            self.settings.details_show_image_dimensions,
            self.settings.details_show_video_duration,
            self.settings.details_show_video_dimensions,
            self.settings.details_show_video_codec,
        );
        let mut changed = false;
        changed |= ui
            .checkbox(&mut self.settings.details_show_preview, "プレビュー")
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.details_show_rating, "★")
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.details_show_tags, "タグ")
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.details_show_kind, "種類")
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.details_show_size, "サイズ")
            .changed();
        // 読書履歴ビューでは「更新日時」列を最終閲覧、「状態」列を既読位置に転用するため、
        // 列表示メニューのラベルもヘッダ・行表示と揃える。
        let (modified_label, state_label) = if self.items_are_reading_history_view {
            ("最終閲覧", "既読位置")
        } else {
            ("更新日時", "状態")
        };
        changed |= ui
            .checkbox(&mut self.settings.details_show_modified, modified_label)
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.details_show_state, state_label)
            .changed();

        ui.separator();
        changed |= ui
            .checkbox(&mut self.settings.details_show_created, "作成日時")
            .on_hover_text("ファイルシステムの作成日時をバックグラウンドで読み込みます")
            .changed();
        changed |= ui
            .checkbox(
                &mut self.settings.details_show_image_dimensions,
                "画像解像度",
            )
            .on_hover_text("必要な値をバックグラウンドで読み込みます")
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.details_show_video_duration, "動画長さ")
            .on_hover_text("FFmpeg で動画情報をバックグラウンド読み込みします")
            .changed();
        changed |= ui
            .checkbox(
                &mut self.settings.details_show_video_dimensions,
                "動画解像度",
            )
            .on_hover_text("FFmpeg で動画情報をバックグラウンド読み込みします")
            .changed();
        changed |= ui
            .checkbox(
                &mut self.settings.details_show_video_codec,
                "動画コーデック",
            )
            .on_hover_text("FFmpeg で動画情報をバックグラウンド読み込みします")
            .changed();

        ui.separator();
        ui.label("サイズ表示");
        for &mode in crate::settings::DetailsSizeDisplayMode::all() {
            changed |= ui
                .radio_value(
                    &mut self.settings.details_size_display_mode,
                    mode,
                    mode.label(),
                )
                .changed();
        }

        ui.separator();
        ui.label("日時");
        changed |= ui
            .checkbox(
                &mut self.settings.details_timestamp_show_seconds,
                "秒まで表示",
            )
            .changed();

        if changed {
            let new_lazy = (
                self.settings.details_show_created,
                self.settings.details_show_image_dimensions,
                self.settings.details_show_video_duration,
                self.settings.details_show_video_dimensions,
                self.settings.details_show_video_codec,
            );
            if old_lazy != new_lazy {
                let has_lazy = self.settings.details_show_created
                    || self.settings.details_show_image_dimensions
                    || self.settings.details_show_video_duration
                    || self.settings.details_show_video_dimensions
                    || self.settings.details_show_video_codec;
                if has_lazy {
                    self.details_image_dims_state = LazyColumnState::NotRequested;
                } else {
                    self.cancel_details_meta_loading();
                }
            }
            self.reset_details_sort_if_hidden();
            self.rebuild_details_order();
            self.settings.save();
            ui.ctx().request_repaint();
        }
    }

    fn draw_details_row(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        idx: usize,
        _row: usize,
        is_spread_pair_cursor: bool,
    ) -> Option<egui::Rect> {
        let Some(item) = self.items.get(idx).cloned() else {
            return None;
        };
        let visuals = ui.visuals();
        let selected = self.selected == Some(idx);
        let checked = self.checked.contains(&idx);
        let bg = if selected {
            visuals.selection.bg_fill
        } else if checked {
            visuals.widgets.active.bg_fill
        } else {
            visuals.panel_fill
        };
        let text_color = if selected {
            visuals.selection.stroke.color
        } else {
            visuals.text_color()
        };

        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, bg);
        if checked && !selected {
            let accent = egui::Rect::from_min_max(
                rect.left_top(),
                egui::pos2(rect.left() + 3.0, rect.bottom()),
            );
            painter.rect_filled(accent, 0.0, visuals.selection.bg_fill);
        }
        painter.line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
        );
        if is_spread_pair_cursor && !selected {
            crate::app::draw_spread_pair_cursor(painter, rect, visuals);
        }

        let name = item.name().into_owned();
        let rating = if self.items_are_drive_list {
            0
        } else {
            self.get_rating(idx)
        };
        let rating_text = if rating == 0 {
            String::new()
        } else {
            "★".repeat(rating as usize)
        };
        let tags_text = self.cell_tag_list(idx).join(" ");
        let kind_text = details_kind_label(&item);
        let (size_text, modified_text) = self
            .image_metas
            .get(idx)
            .and_then(|m| *m)
            .map(|(mtime, size)| {
                let size_text = if size > 0 {
                    crate::ui_helpers::format_details_size(
                        size as u64,
                        self.settings.details_size_display_mode,
                    )
                } else {
                    String::new()
                };
                (
                    size_text,
                    format_details_mtime(mtime, self.settings.details_timestamp_show_seconds),
                )
            })
            .unwrap_or_else(|| (String::new(), String::new()));
        // 読書履歴ビューでは「更新日時」列を最終閲覧、「状態」列を既読位置に転用する。
        let modified_text = if self.items_are_reading_history_view {
            self.reading_history_last_read_for_idx(idx)
                .unwrap_or_default()
        } else {
            modified_text
        };
        let state_text = self.details_state_text(idx);
        let mut hovered_preview_rect = None;

        for (col, col_rect) in details_column_rects(rect, &self.settings) {
            match col {
                DetailsColumn::Preview => {
                    let enabled = !matches!(item, GridItem::ZipSeparator { .. });
                    let response = ui.interact(
                        col_rect,
                        ui.id().with(("details_preview_icon", idx)),
                        egui::Sense::hover(),
                    );
                    if enabled && response.hovered() {
                        hovered_preview_rect = Some(col_rect);
                    }
                    draw_details_preview_icon(
                        ui.painter(),
                        col_rect.shrink2(egui::vec2(6.0, 5.0)),
                        text_color,
                        !enabled,
                    );
                }
                DetailsColumn::Name => draw_details_text(
                    ui,
                    col_rect,
                    &name,
                    egui::Align2::LEFT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::Rating => draw_details_text(
                    ui,
                    col_rect,
                    &rating_text,
                    egui::Align2::LEFT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::Tags => draw_details_text(
                    ui,
                    col_rect,
                    &tags_text,
                    egui::Align2::LEFT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::Kind => draw_details_text(
                    ui,
                    col_rect,
                    &kind_text,
                    egui::Align2::LEFT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::Size => draw_details_text(
                    ui,
                    col_rect,
                    &size_text,
                    egui::Align2::RIGHT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::Modified => draw_details_text(
                    ui,
                    col_rect,
                    &modified_text,
                    egui::Align2::LEFT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::Created => draw_details_text(
                    ui,
                    col_rect,
                    &self.details_created_text(idx),
                    egui::Align2::LEFT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::ImageDimensions => draw_details_text(
                    ui,
                    col_rect,
                    &self.details_image_dims_text(idx),
                    egui::Align2::RIGHT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::VideoDuration => draw_details_text(
                    ui,
                    col_rect,
                    &self.details_video_duration_text(idx),
                    egui::Align2::RIGHT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::VideoDimensions => draw_details_text(
                    ui,
                    col_rect,
                    &self.details_video_dims_text(idx),
                    egui::Align2::RIGHT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::VideoCodec => draw_details_text(
                    ui,
                    col_rect,
                    &self.details_video_codec_text(idx),
                    egui::Align2::LEFT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::State => draw_details_text(
                    ui,
                    col_rect,
                    &state_text,
                    egui::Align2::LEFT_CENTER,
                    text_color,
                    false,
                ),
            }
        }
        self.draw_reading_history_tooltip(ui, rect, idx);
        hovered_preview_rect
    }

    fn details_state_text(&mut self, idx: usize) -> String {
        if self.items_are_reading_history_view {
            return self
                .reading_history_progress_for_idx(idx)
                .unwrap_or_default();
        }
        let mut flags = Vec::new();
        let badges = self.grid_edit_badges(idx);
        if badges.page_override {
            flags.push("補");
        }
        if badges.local_adjust {
            flags.push("レ");
        }
        if badges.mask {
            flags.push("消");
        }
        if badges.conceal {
            flags.push("隠");
        }
        if badges.comic {
            flags.push("文");
        }
        if badges.rotation {
            flags.push("回");
        }
        // 代表サムネピン (ネスト ZIP では本ごとピン Model B: ルート = zip_path /
        // 本の中 = 実効 prefix の book キー + ZipEntry)。
        if let (Some(pin_container), Some(src)) = (
            self.pin_container_key(),
            self.folder_pin_selected_source(idx),
        ) {
            let key = crate::path_key::normalize_keep_drive(&pin_container);
            if self.folder_pin_map.get(&key) == Some(&src) {
                flags.push("ピ");
            }
        }
        flags.join(" ")
    }

    // ── サムネイルグリッド ───────────────────────────────────────────

    /// サムネイルグリッドを描画し、フォルダナビゲーション先を返す。
    pub(crate) fn render_grid(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        let scroll_to = self.scroll_to_selected;
        self.scroll_to_selected = false;
        if self.settings.grid_view_mode != GridViewMode::Details
            && self.details_hover_thumb_viewport_open
        {
            let viewport_id = egui::ViewportId::from_hash_of("details_thumbnail_tooltip");
            ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Close);
            self.details_hover_thumb_viewport_open = false;
            self.set_details_hover_thumbnail_idx(None);
        }

        egui::CentralPanel::default()
            .show(ctx, |ui| -> Option<PathBuf> {
                let global_searching =
                    self.items_are_global_search_view && self.global_search.is_searching();
                if self.items.is_empty() {
                    // ZIP / PDF 非同期列挙中は「読み込み中…」にして待ち状態を明示する。
                    // BS や Ctrl+↑↓ はこの間でも受理され、load_folder 側で pending が
                    // Drop されて worker が cancel する。
                    let loading = self.zip_enumerate_pending.is_some()
                        || self.pdf_enumerate_pending.is_some();
                    let msg = if global_searching {
                        "検索中"
                    } else if loading {
                        "読み込み中…"
                    } else if self.current_folder.is_some() {
                        "表示するファイルがありません"
                    } else {
                        "フォルダを入力して Enter キーを押してください"
                    };
                    ui.centered_and_justified(|ui| ui.label(msg));
                    // 空フォルダでも右クリックでフォルダ操作可能にする
                    self.start_grid_background_mouse_ring_flick_if_pressed(ctx, ui.max_rect());
                    self.update_grid_mouse_ring_flick(ctx);
                    if ui.rect_contains_pointer(ui.max_rect())
                        && ctx.input(|i| i.pointer.secondary_clicked())
                        && !self.mouse_ring_context_menu_suppressed(ctx)
                    {
                        self.open_current_folder_context_menu(ctx);
                    }
                    let full_rect = ui.max_rect();
                    self.draw_mouse_ring_flick_overlay(
                        ui,
                        full_rect,
                        crate::ring_shortcut::RingShortcutContext::Grid,
                    );
                    self.draw_mouse_gesture_overlay(
                        ui,
                        full_rect,
                        crate::ring_shortcut::RightDragContext::Grid,
                    );
                    self.draw_gamepad_ring_overlay(ui, full_rect);
                    self.draw_gamepad_picker_overlay(ui, full_rect);
                    self.draw_gamepad_favorite_picker_overlay(ui, full_rect);
                    self.draw_gamepad_location_picker_overlay(ui, full_rect);
                    self.draw_feedback_toast(ui, full_rect, ctx);
                    self.clear_mouse_ring_context_menu_suppression_if_idle(ctx);
                    return None;
                }

                if self.visible_indices.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label(if global_searching {
                            "検索中"
                        } else {
                            "検索結果なし"
                        });
                    });
                    self.start_grid_background_mouse_ring_flick_if_pressed(ctx, ui.max_rect());
                    self.update_grid_mouse_ring_flick(ctx);
                    if ui.rect_contains_pointer(ui.max_rect())
                        && ctx.input(|i| i.pointer.secondary_clicked())
                        && !self.mouse_ring_context_menu_suppressed(ctx)
                    {
                        self.open_current_folder_context_menu(ctx);
                    }
                    let full_rect = ui.max_rect();
                    self.draw_mouse_ring_flick_overlay(
                        ui,
                        full_rect,
                        crate::ring_shortcut::RingShortcutContext::Grid,
                    );
                    self.draw_mouse_gesture_overlay(
                        ui,
                        full_rect,
                        crate::ring_shortcut::RightDragContext::Grid,
                    );
                    self.draw_gamepad_ring_overlay(ui, full_rect);
                    self.draw_gamepad_picker_overlay(ui, full_rect);
                    self.draw_gamepad_favorite_picker_overlay(ui, full_rect);
                    self.draw_gamepad_location_picker_overlay(ui, full_rect);
                    self.draw_feedback_toast(ui, full_rect, ctx);
                    self.clear_mouse_ring_context_menu_suppression_if_idle(ctx);
                    return None;
                }

                let spread_pair_cursor_idx = self.main_grid_spread_pair_cursor_idx();

                if self.settings.grid_view_mode == GridViewMode::Details {
                    return self.render_details_list(ui, ctx, scroll_to, spread_pair_cursor_idx);
                }

                let cols = self.settings.grid_cols.max(1);
                let avail_w = ui.available_width();
                let height_ratio = self.effective_thumb_aspect().height_ratio();
                let Some((cell_w, cell_h)) = compute_cell_size(avail_w, cols, height_ratio) else {
                    return None;
                };

                // ウィンドウリサイズやアスペクト比変更でセルサイズが変わった場合スナップし直す
                if (cell_w - self.last_cell_size).abs() > 0.5
                    || (cell_h - self.last_cell_h).abs() > 0.5
                {
                    self.scroll_offset_y = (self.scroll_offset_y / cell_h).round() * cell_h;
                    self.last_cell_size = cell_w;
                    self.last_cell_h = cell_h;
                }

                let viewport_h = ui.available_height().max(0.0);
                self.last_viewport_h = viewport_h;

                if scroll_to {
                    self.apply_scroll_to_selected(cols, cell_h);
                }

                let total_rows = self.visible_indices.len().div_ceil(cols);
                let natural_h = total_rows as f32 * cell_h;

                // egui 内部の max offset = total_h - viewport_h が行境界に揃うよう、
                // total_h を拡張する。これにより egui と自前の行スナップが一致し振動を防ぐ。
                // 拡張量は最大 cell_h 未満（端数の補正のみ）。
                let (total_h, max_offset) = snapped_scroll_extent(natural_h, viewport_h, cell_h);
                self.scroll_offset_y = self.scroll_offset_y.clamp(0.0, max_offset);

                let mut nav: Option<PathBuf> = None;

                // egui にスクロールを管理させず、自前の offset を毎フレーム注入する。
                // ただしスクロールバードラッグ時は egui 側のオフセットを読み戻す。
                let scroll_output = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .vertical_scroll_offset(self.scroll_offset_y)
                    .show_viewport(ui, |ui, viewport| {
                        // 実際のビューポート高さも記録する。リサイズ中の scroll extent
                        // 計算自体は上の `ui.available_height()` で同フレーム内に行う。
                        self.last_viewport_h = viewport.height();

                        let (content_rect, _) = ui.allocate_exact_size(
                            egui::vec2(avail_w, total_h),
                            egui::Sense::hover(),
                        );

                        let first_row = (viewport.min.y / cell_h) as usize;
                        let last_row = ((viewport.max.y / cell_h) as usize + 2).min(total_rows);

                        // Phase 2b ワーカーへ現在の可視先頭/終端アイテムを通知
                        let vis_first_idx = self
                            .visible_indices
                            .get(first_row * cols)
                            .copied()
                            .unwrap_or(0);
                        self.scroll_hint.store(vis_first_idx, Ordering::Relaxed);
                        // 可視範囲の終端 (exclusive)。先読みの forward 側距離計算に使う。
                        // last_row は exclusive、可視セルの最後の位置は (last_row*cols - 1) だが
                        // 末尾の行は半分しか埋まっていない場合があるので visible_indices.len() で clamp。
                        let last_pos_inclusive = (last_row * cols)
                            .saturating_sub(1)
                            .min(self.visible_indices.len().saturating_sub(1));
                        let vis_end_idx = self
                            .visible_indices
                            .get(last_pos_inclusive)
                            .copied()
                            .map(|i| i + 1)
                            .unwrap_or(vis_first_idx);
                        self.visible_end_shared
                            .store(vis_end_idx, Ordering::Relaxed);

                        for row in first_row..last_row {
                            for col in 0..cols {
                                let vis_pos = row * cols + col;
                                if vis_pos >= self.visible_indices.len() {
                                    break;
                                }
                                let idx = self.visible_indices[vis_pos];

                                let cell_rect = egui::Rect::from_min_size(
                                    content_rect.min
                                        + egui::vec2(col as f32 * cell_w, row as f32 * cell_h),
                                    egui::vec2(cell_w, cell_h),
                                );

                                if let Some(n) =
                                    self.handle_cell_interaction(ui, ctx, cell_rect, idx)
                                {
                                    nav = Some(n);
                                }
                                // handle_cell_interaction 内で同期的に items が差し替わる
                                // 経路がある (SearchContainer ダブルクリック →
                                // drill_into_container、Ctrl+G 絞り込み中の Folder
                                // ダブルクリック → drill_into_subfolder)。以降の
                                // self.items[idx] / self.thumbnails[idx] は stale idx で
                                // out-of-bounds panic するので、境界を再チェックして
                                // 残りの列/行を抜ける (panic.log の ui_main.rs:1026
                                // "len is 0 but index is 102" を回避)。
                                if idx >= self.items.len() || idx >= self.thumbnails.len() {
                                    break;
                                }

                                let rot = self.get_rotation(idx);
                                let badges = self.grid_edit_badges(idx);
                                let rating = if self.items_are_drive_list {
                                    0
                                } else {
                                    self.get_rating(idx)
                                };
                                // 可視セルは同期適用 (~3ms/枚)。先読み分は背後の
                                // process_thumb_adjust_budget が逐次処理する。
                                // ドラッグ中は両経路ともスキップして生サムネ表示に戻す
                                // (70 枚毎フレーム再生成は ~200ms のフリーズになるため)。
                                if !self.adjustment_dragging {
                                    self.maybe_apply_thumb_adjustment(ctx, idx);
                                }
                                let adjusted_tex = if self.adjustment_dragging {
                                    None
                                } else {
                                    self.thumb_adjust_tex.get(&idx)
                                };
                                let tags = self.cell_tag_list(idx).to_vec();
                                let filter_match = if self.items_are_drive_list {
                                    None
                                } else {
                                    self.folder_rating_match(idx)
                                };
                                let filter_match_count = filter_match.map(|(c, _)| c);
                                // 📌 バッジ (金色) — ユーザーが Pin 操作した対象アイテムの
                                // 目印。「現在表示中のコンテナの pin source = この item」
                                // (= ユーザーがこのアイテムを選択して P / 📌 を押した) のとき
                                // のみ出す。
                                //
                                // **「コンテナ自身が pin 済み」の表示は出さない** (= ユーザーから
                                // 「pin で表示されているサムネ」と「auto-pick で選ばれたサムネ」を
                                // 区別させないことで、「badge = 自分が Pin 操作した対象」を 1 対 1
                                // で対応させる)。
                                // ネスト ZIP では本ごとピン (Model B): book キー + ZipEntry source
                                // (ルート = zip_path / 本の中 = 実効 prefix)。
                                let has_pin = self.cell_has_pin_badge(idx);
                                crate::app::draw_cell(
                                    ui,
                                    cell_rect,
                                    self.selected == Some(idx),
                                    self.checked.contains(&idx),
                                    spread_pair_cursor_idx == Some(idx),
                                    badges.page_override,
                                    badges.local_adjust,
                                    badges.mask,
                                    badges.conceal,
                                    badges.comic,
                                    rating,
                                    &self.items[idx],
                                    &self.thumbnails[idx],
                                    rot,
                                    adjusted_tex,
                                    &tags,
                                    filter_match_count,
                                    has_pin,
                                    self.items_are_drive_list,
                                );
                                // 小さい右下バッジに限らずセル全体をホバー領域にして
                                // ★内訳 tooltip を出す。
                                if let Some((_total, per_star)) = filter_match {
                                    let hover_id = egui::Id::new(("folder_rating_badge", idx));
                                    let resp =
                                        ui.interact(cell_rect, hover_id, egui::Sense::hover());
                                    if resp.hovered() {
                                        resp.on_hover_ui_at_pointer(|ui| {
                                            ui.vertical(|ui| {
                                                ui.label("マッチする子孫ファイル");
                                                for s in (1..=5usize).rev() {
                                                    let c = per_star[s - 1];
                                                    if c > 0 {
                                                        ui.label(format!(
                                                            "{} : {} 件",
                                                            "★".repeat(s),
                                                            c
                                                        ));
                                                    }
                                                }
                                            });
                                        });
                                    }
                                }

                                self.draw_reading_history_tooltip(ui, cell_rect, idx);

                                // 選択中セルの矩形を記録 (オーバーレイ配置用)
                                if self.selected == Some(idx) {
                                    self.selected_cell_rect = Some(cell_rect);
                                }
                            }
                        }
                    });

                // グリッドの空白部分で右クリック → フォルダメニュー。
                // セルの右クリックは handle_cell_interaction 側で先に `context_menu_idx`
                // をセットする。ここではそれが無かった場合だけ、ScrollArea の表示領域
                // 全体を背景として扱う。content Ui の `ui_contains_pointer()` だと、
                // サムネイル総高さが viewport より低いときに最後の行より下の余白を
                // 拾えないため、`scroll_output.inner_rect` を使う。
                self.start_grid_background_mouse_ring_flick_if_pressed(
                    ctx,
                    scroll_output.inner_rect,
                );
                self.update_grid_mouse_ring_flick(ctx);
                let bg_right_clicked = ui.rect_contains_pointer(scroll_output.inner_rect)
                    && ctx.input(|i| i.pointer.secondary_clicked());
                if bg_right_clicked
                    && self.context_menu_idx.is_none()
                    && !self.mouse_ring_context_menu_suppressed(ctx)
                {
                    self.open_current_folder_context_menu(ctx);
                }
                self.clear_mouse_ring_context_menu_suppression_if_idle(ctx);

                // スクロールバードラッグによるオフセット変化を読み戻す。
                // egui が内部で管理するオフセットと自前オフセットを同期させる。
                // ただし行スナップによる端数差分で毎フレーム振動するのを防ぐため、
                // 1 行分 (cell_h) 以上ずれた場合のみ同期する。
                let egui_offset = scroll_output.state.offset.y;
                if (egui_offset - self.scroll_offset_y).abs() > cell_h * 0.5 {
                    self.scroll_offset_y = (egui_offset / cell_h).round() * cell_h;
                }

                // 右上フィードバックトースト (Q / Ctrl+Backspace / F7〜F10 / レーティング等)
                // show_feedback_toast でセットされたテキストをグリッド画面でも描画する。
                // フルスクリーン側は render_fullscreen_viewport が別途呼ぶ。
                let full_rect = ui.max_rect();
                self.draw_mouse_ring_flick_overlay(
                    ui,
                    full_rect,
                    crate::ring_shortcut::RingShortcutContext::Grid,
                );
                self.draw_mouse_gesture_overlay(
                    ui,
                    full_rect,
                    crate::ring_shortcut::RightDragContext::Grid,
                );
                self.draw_gamepad_ring_overlay(ui, full_rect);
                self.draw_gamepad_picker_overlay(ui, full_rect);
                self.draw_gamepad_favorite_picker_overlay(ui, full_rect);
                self.draw_gamepad_location_picker_overlay(ui, full_rect);
                self.draw_feedback_toast(ui, full_rect, ctx);

                nav
            })
            .inner
    }

    // ── 選択情報オーバーレイ ─────────────────────────────────────────

    /// 選択中アイテムの情報をセル直下に表示する。
    pub(crate) fn render_selection_info(&self, ctx: &egui::Context) {
        // メインウィンドウを専有するビューア中は出さない (独自のホバーヘッダーを持つため)。
        if self.viewer_session_blocks_main_window() {
            return;
        }
        if self.settings.grid_view_mode == GridViewMode::Details {
            return;
        }
        if self.items_are_drive_list {
            return;
        }

        let (Some(idx), Some(cell_rect)) = (self.selected, self.selected_cell_rect) else {
            return;
        };

        // ZipSeparator はスキップ
        if matches!(
            self.items.get(idx),
            Some(GridItem::ZipSeparator { .. }) | None
        ) {
            return;
        }
        let Some(item) = self.items.get(idx) else {
            return;
        };

        let mut lines = Vec::new();
        if self.settings.thumb_tooltip_show_filename {
            let name = item.name().into_owned();
            if !name.is_empty() {
                lines.push(name);
            }
        }
        if let Some(history_lines) = self.reading_history_selection_info_lines(idx) {
            lines.push(history_lines.join("   "));
        }

        let mut fields = Vec::new();
        let mut full_location_line = None;
        if self.settings.thumb_tooltip_show_kind {
            fields.push(format!("種類 {}", details_kind_label(item)));
        }
        if self.settings.thumb_tooltip_show_file_size {
            let size = self
                .image_metas
                .get(idx)
                .copied()
                .flatten()
                .map(|(_, size)| size)
                .unwrap_or(0);
            let text = if size > 0 {
                crate::ui_helpers::format_details_size(
                    size as u64,
                    self.settings.details_size_display_mode,
                )
            } else {
                "-".to_string()
            };
            fields.push(format!("サイズ {text}"));
        }
        if self.settings.thumb_tooltip_show_modified {
            let text = self
                .image_metas
                .get(idx)
                .copied()
                .flatten()
                .map(|(mtime, _)| {
                    format_details_mtime(mtime, self.settings.details_timestamp_show_seconds)
                })
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "-".to_string());
            fields.push(format!("更新 {text}"));
        }
        if self.settings.thumb_tooltip_show_image_dimensions
            && matches!(
                item,
                GridItem::Image(_) | GridItem::ZipImage { .. } | GridItem::PdfPage { .. }
            )
        {
            let text = match self.thumbnails.get(idx) {
                Some(ThumbnailState::Loaded {
                    source_dims: Some((w, h)),
                    ..
                }) => format!("{w} × {h}"),
                _ => {
                    let lazy = self.details_image_dims_text(idx);
                    if lazy.is_empty() {
                        "...".to_string()
                    } else {
                        lazy.replace('x', " × ")
                    }
                }
            };
            fields.push(format!("画像 {text}"));
        }
        if self.settings.thumb_tooltip_show_video_duration && matches!(item, GridItem::Video(_)) {
            let duration_text = self.details_video_duration_text(idx);
            let display = if duration_text.is_empty() {
                "..."
            } else {
                duration_text.as_str()
            };
            fields.push(format!("長さ {display}"));
        }
        if self.settings.thumb_tooltip_show_video_dimensions && matches!(item, GridItem::Video(_)) {
            let text = self.details_video_dims_text(idx);
            let text = if text.is_empty() {
                "...".to_string()
            } else {
                text.replace('x', " × ")
            };
            fields.push(format!("動画 {text}"));
        }
        if self.settings.thumb_tooltip_show_video_codec && matches!(item, GridItem::Video(_)) {
            let codec_text = self.details_video_codec_text(idx);
            let display = if codec_text.is_empty() {
                "..."
            } else {
                codec_text.as_str()
            };
            fields.push(format!("コーデック {display}"));
        }
        if self.settings.thumb_tooltip_show_created {
            let created_text = self.details_created_text(idx);
            let display = if created_text.is_empty() {
                "..."
            } else {
                created_text.as_str()
            };
            fields.push(format!("作成 {display}"));
        }
        if self.settings.thumb_tooltip_show_location {
            if let Some(location) = selection_info_parent_location_label(item) {
                fields.push(location);
            }
        }
        if self.settings.thumb_tooltip_show_full_location {
            if let Some(path) = self.facet_place_path_for_item(item) {
                let location = self.facet_place_label_for_path(&path);
                if !location.is_empty() {
                    full_location_line = Some(format!("場所 {location}"));
                }
            }
        }

        if !fields.is_empty() {
            lines.push(fields.join("   "));
        }
        if let Some(location) = full_location_line {
            lines.push(location);
        }
        if lines.is_empty() {
            return;
        }
        let text = lines.join("\n");

        let viewport = ctx.content_rect();
        let popup_w = (cell_rect.width() * 2.5)
            .clamp(180.0, 520.0)
            .min((viewport.width() - 16.0).max(80.0));
        let min_x = viewport.left() + 8.0;
        let max_x = viewport.right() - 8.0 - popup_w;
        let mut x = cell_rect.left();
        if x + popup_w > viewport.right() - 8.0 {
            x = cell_rect.right() - popup_w;
        }
        x = if max_x >= min_x {
            x.clamp(min_x, max_x)
        } else {
            min_x
        };

        let row_count = text.lines().count().clamp(1, 3) as f32;
        let estimated_h = 18.0 * row_count + 18.0;
        let mut y = cell_rect.bottom() + 4.0;
        if y + estimated_h > viewport.bottom() - 8.0 {
            y = (cell_rect.top() - estimated_h - 4.0).max(viewport.top() + 8.0);
        }
        let area_pos = egui::pos2(x, y);

        egui::Area::new("selection_info".into())
            .order(egui::Order::Middle)
            .fixed_pos(area_pos)
            .show(ctx, |ui| {
                let dark = ui.visuals().dark_mode;
                let (fill, text_color, stroke, shadow) = if dark {
                    (
                        egui::Color32::from_rgba_unmultiplied(20, 25, 35, 230),
                        egui::Color32::WHITE,
                        egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 38),
                        ),
                        egui::Shadow {
                            offset: [0, 2],
                            blur: 10,
                            spread: 0,
                            color: egui::Color32::from_rgba_unmultiplied(0, 0, 0, 90),
                        },
                    )
                } else {
                    (
                        egui::Color32::from_rgba_unmultiplied(232, 235, 240, 248),
                        egui::Color32::from_gray(25),
                        egui::Stroke::new(1.0, egui::Color32::from_gray(172)),
                        egui::Shadow {
                            offset: [0, 2],
                            blur: 12,
                            spread: 0,
                            color: egui::Color32::from_rgba_unmultiplied(0, 0, 0, 52),
                        },
                    )
                };
                egui::Frame::popup(ui.style())
                    .fill(fill)
                    .stroke(stroke)
                    .shadow(shadow)
                    .show(ui, |ui| {
                        let inner_width = (popup_w - 12.0).max(40.0);
                        ui.set_min_width(inner_width);
                        ui.set_max_width(inner_width);
                        // ファイル名や仮想コンテナ名は空白なしで長くなりやすい。
                        // 幅を広げた上で最大 3 行に収め、超過分は末尾省略にする。
                        let mut job = egui::text::LayoutJob::single_section(
                            text,
                            egui::TextFormat {
                                font_id: egui::TextStyle::Monospace.resolve(ui.style()),
                                color: text_color,
                                ..Default::default()
                            },
                        );
                        job.wrap.max_width = inner_width;
                        job.wrap.max_rows = 3;
                        job.wrap.break_anywhere = true;
                        job.wrap.overflow_character = Some('…');
                        let galley = ui.painter().layout_job(job);
                        ui.add(egui::Label::new(galley));
                    });
            });
    }
}

#[cfg(test)]
mod menubar_hover_switch_tests {
    use super::*;

    #[test]
    fn switches_to_hovered_sibling_only_while_menu_is_open() {
        assert_eq!(menubar_hover_switch_target(Some(0), Some(1)), Some(1));
        assert_eq!(menubar_hover_switch_target(Some(5), Some(2)), Some(2));
    }

    #[test]
    fn does_not_open_cold_menu_on_hover() {
        assert_eq!(menubar_hover_switch_target(None, Some(1)), None);
    }

    #[test]
    fn keeps_current_menu_when_hovering_open_button_or_nothing() {
        assert_eq!(menubar_hover_switch_target(Some(2), Some(2)), None);
        assert_eq!(menubar_hover_switch_target(Some(2), None), None);
    }
}

#[cfg(test)]
mod rating_filter_op_tests {
    use super::*;

    #[test]
    fn thumbnail_count_label_pads_visible_to_total_digits() {
        let items: Vec<GridItem> = (0..100)
            .map(|i| GridItem::Image(PathBuf::from(format!("img_{i}.jpg"))))
            .collect();
        let visible_indices: Vec<usize> = (0..20).collect();

        assert_eq!(thumbnail_count_label(&items, &visible_indices), "( 20/100)");
    }

    #[test]
    fn filtered_count_label_shows_visible_and_total_counts() {
        let items: Vec<GridItem> = (0..300)
            .map(|i| GridItem::Image(PathBuf::from(format!("img_{i}.jpg"))))
            .collect();
        let visible_indices: Vec<usize> = (0..123).collect();

        assert_eq!(
            filtered_count_label(&items, &visible_indices),
            "123 / 300 件"
        );
    }

    #[test]
    fn filtered_count_label_ignores_zip_separators() {
        let items = vec![
            GridItem::Image(PathBuf::from("a.jpg")),
            GridItem::ZipSeparator {
                dir_display: "dir".into(),
            },
            GridItem::Image(PathBuf::from("b.jpg")),
        ];
        let visible_indices = vec![0, 1];

        assert_eq!(filtered_count_label(&items, &visible_indices), "1 / 2 件");
    }

    #[test]
    fn folder_bar_drive_letter_input_becomes_drive_root() {
        assert_eq!(
            normalize_folder_bar_input_path(Path::new("D:")),
            PathBuf::from(r"D:\")
        );
        assert_eq!(
            normalize_folder_bar_input_path(Path::new(" d: ")),
            PathBuf::from(r"d:\")
        );
    }

    #[test]
    fn folder_bar_non_drive_input_is_preserved() {
        assert_eq!(
            normalize_folder_bar_input_path(Path::new(r"D:\books")),
            PathBuf::from(r"D:\books")
        );
        assert_eq!(
            normalize_folder_bar_input_path(Path::new(r"D:books")),
            PathBuf::from(r"D:books")
        );
    }

    #[test]
    fn thumbnail_count_label_ignores_zip_separators() {
        let items = vec![
            GridItem::Image(PathBuf::from("a.jpg")),
            GridItem::ZipSeparator {
                dir_display: "chapter".to_string(),
            },
            GridItem::Image(PathBuf::from("b.jpg")),
        ];
        let visible_indices = vec![0, 1];

        assert_eq!(thumbnail_count_label(&items, &visible_indices), "(1/2)");
    }

    #[test]
    fn is_solo_detects_single_on_bucket() {
        let mut rf = [false; 6];
        rf[3] = true;
        assert!(is_rating_solo(&rf, 3));
        assert!(!is_rating_solo(&rf, 2));
        // 全 ON は solo ではない
        assert!(!is_rating_solo(&[true; 6], 3));
        // 全 OFF も solo ではない
        assert!(!is_rating_solo(&[false; 6], 3));
    }

    #[test]
    fn is_threshold_detects_idx_and_above() {
        let rf = [false, false, false, true, true, true];
        assert!(is_rating_threshold(&rf, 3));
        assert!(!is_rating_threshold(&rf, 2));
        assert!(!is_rating_threshold(&rf, 4));
        // idx=0 のとき threshold は全 ON と等価
        assert!(is_rating_threshold(&[true; 6], 0));
        assert!(!is_rating_threshold(&[false; 6], 0));
    }

    #[test]
    fn apply_toggle_flips_single_bucket() {
        let mut rf = [true; 6];
        apply_rating_filter_op(&mut rf, RatingFilterOp::Toggle, 2);
        assert_eq!(rf, [true, true, false, true, true, true]);
        apply_rating_filter_op(&mut rf, RatingFilterOp::Toggle, 2);
        assert_eq!(rf, [true; 6]);
    }

    #[test]
    fn apply_solo_sets_only_target_on() {
        let mut rf = [true; 6];
        apply_rating_filter_op(&mut rf, RatingFilterOp::Solo, 3);
        assert_eq!(rf, [false, false, false, true, false, false]);
    }

    #[test]
    fn apply_threshold_sets_idx_and_above() {
        let mut rf = [false; 6];
        apply_rating_filter_op(&mut rf, RatingFilterOp::Threshold, 3);
        assert_eq!(rf, [false, false, false, true, true, true]);
        // idx=0 は全 ON と等価
        apply_rating_filter_op(&mut rf, RatingFilterOp::Threshold, 0);
        assert_eq!(rf, [true; 6]);
    }

    #[test]
    fn apply_all_on_matches_default() {
        let mut rf = [false; 6];
        apply_rating_filter_op(&mut rf, RatingFilterOp::AllOn, 0);
        assert_eq!(rf, crate::settings::default_rating_filter());
    }

    /// Ctrl+Shift+★N の挙動: ★N と「なし」だけ ON、残りはすべて OFF。
    /// 未評価コンテナ (= フォルダ / 未評価 ZIP / 未評価 PDF) と未評価画像の両方が残るので
    /// ★N 画像をフォルダツリーから探す用途向け。
    #[test]
    fn apply_solo_with_unrated_keeps_none_bucket_on() {
        let mut rf = [true; 6];
        apply_rating_filter_op(&mut rf, RatingFilterOp::SoloWithUnrated, 5);
        assert_eq!(rf, [true, false, false, false, false, true]);
    }

    /// `is_rating_solo_with_unrated` は Ctrl+Shift 状態の検出用。
    /// トグル 2 回目の Ctrl+Shift クリックで AllOn に戻るための述語。
    #[test]
    fn is_solo_with_unrated_detects_none_plus_target() {
        let rf = [true, false, false, false, false, true];
        assert!(is_rating_solo_with_unrated(&rf, 5));
        assert!(!is_rating_solo_with_unrated(&rf, 4));
        // idx=0 は定義外 (常に false)
        assert!(!is_rating_solo_with_unrated(&rf, 0));
        // なし が OFF なら false
        let rf_no_none = [false, false, false, false, false, true];
        assert!(!is_rating_solo_with_unrated(&rf_no_none, 5));
        // 2 星バケツ以上 ON も false
        let rf_two_stars = [true, false, false, true, false, true];
        assert!(!is_rating_solo_with_unrated(&rf_two_stars, 5));
    }

    /// Ctrl+Shift+クリック は solo_with_unrated ↔ 全 ON を往復する。
    #[test]
    fn ctrl_shift_click_model_toggles_with_unrated() {
        let mut rf = [true; 6];
        // 初回 Ctrl+Shift+★5 → ★5 + なし だけ
        let op = if is_rating_solo_with_unrated(&rf, 5) {
            RatingFilterOp::AllOn
        } else {
            RatingFilterOp::SoloWithUnrated
        };
        apply_rating_filter_op(&mut rf, op, 5);
        assert!(is_rating_solo_with_unrated(&rf, 5));
        // 同じボタンを Ctrl+Shift+クリック再度 → 全 ON
        let op = if is_rating_solo_with_unrated(&rf, 5) {
            RatingFilterOp::AllOn
        } else {
            RatingFilterOp::SoloWithUnrated
        };
        apply_rating_filter_op(&mut rf, op, 5);
        assert_eq!(rf, crate::settings::default_rating_filter());
    }

    /// click logic のモデル: Ctrl+click は solo ↔ 全 ON を往復する。
    #[test]
    fn ctrl_click_model_solo_and_restore() {
        let mut rf = [true; 6];
        // 既に全 ON で Ctrl+★3 → solo 状態に
        let op = if is_rating_solo(&rf, 3) {
            RatingFilterOp::AllOn
        } else {
            RatingFilterOp::Solo
        };
        apply_rating_filter_op(&mut rf, op, 3);
        assert!(is_rating_solo(&rf, 3));
        // 同じボタンを Ctrl+クリック再度 → 全 ON
        let op = if is_rating_solo(&rf, 3) {
            RatingFilterOp::AllOn
        } else {
            RatingFilterOp::Solo
        };
        apply_rating_filter_op(&mut rf, op, 3);
        assert_eq!(rf, crate::settings::default_rating_filter());
    }

    /// Shift+click は threshold ↔ 全 ON を往復する。
    #[test]
    fn shift_click_model_threshold_and_restore() {
        let mut rf = [true; 6];
        // 全 ON で Shift+★3 → threshold (idx>=3 のみ ON)
        let op = if is_rating_threshold(&rf, 3) {
            RatingFilterOp::AllOn
        } else {
            RatingFilterOp::Threshold
        };
        apply_rating_filter_op(&mut rf, op, 3);
        assert_eq!(rf, [false, false, false, true, true, true]);
        // 同ボタン再度 → 全 ON
        let op = if is_rating_threshold(&rf, 3) {
            RatingFilterOp::AllOn
        } else {
            RatingFilterOp::Threshold
        };
        apply_rating_filter_op(&mut rf, op, 3);
        assert_eq!(rf, crate::settings::default_rating_filter());
    }
}

#[cfg(test)]
mod book_reorder_drag_tests {
    use super::*;

    fn reorder_entry(name: &str) -> crate::books::BookPageEntry {
        crate::books::BookPageEntry {
            path: PathBuf::from(format!("C:/book/{name}.jpg")),
            display_name: format!("{name}.jpg"),
        }
    }

    fn reorder_state(names: &[&str], selected: &[&str]) -> crate::app::BookReorderState {
        let entries = names
            .iter()
            .map(|name| reorder_entry(name))
            .collect::<Vec<_>>();
        let selected_keys = entries
            .iter()
            .filter(|entry| {
                selected.iter().any(|name| {
                    entry
                        .path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .is_some_and(|stem| stem == *name)
                })
            })
            .map(book_reorder_entry_key)
            .collect::<HashSet<_>>();
        crate::app::BookReorderState {
            folder: PathBuf::from("C:/book"),
            entries,
            selected: Some(0),
            selected_keys,
            selection_anchor: Some(0),
            dragging: None,
            drag_auto_scroll_enabled: false,
            scroll_offset_y: 0.0,
            thumb_textures: HashMap::new(),
            thumb_failed: HashSet::new(),
            thumb_pending_keys: HashSet::new(),
            thumb_upload_backlog: VecDeque::new(),
            thumb_tx: None,
            thumb_rx: None,
            dirty: false,
            drag_insert_index: None,
            thumb_tile_px: BOOK_REORDER_DEFAULT_TILE_PX,
            flush_pending: None,
            transfer_target_book: String::new(),
            transfer_pending: None,
            error: None,
        }
    }

    fn reorder_names(state: &crate::app::BookReorderState) -> Vec<String> {
        state
            .entries
            .iter()
            .map(|entry| {
                entry
                    .path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn drop_target_uses_pointer_position_inside_target_rect() {
        let rect = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 140.0));

        assert_eq!(
            book_reorder_drop_target_for_pos(rect, 4, 10, 5, 8.0, Some(egui::pos2(20.0, 80.0))),
            Some((4, rect.left() - 4.0))
        );
        assert_eq!(
            book_reorder_drop_target_for_pos(rect, 4, 10, 5, 8.0, Some(egui::pos2(95.0, 80.0))),
            Some((5, rect.right()))
        );
    }

    #[test]
    fn drop_target_ignores_pointer_outside_target_rect() {
        let rect = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 140.0));

        assert_eq!(
            book_reorder_drop_target_for_pos(rect, 4, 10, 5, 8.0, Some(egui::pos2(9.0, 80.0))),
            None
        );
        assert_eq!(
            book_reorder_drop_target_for_pos(rect, 4, 10, 5, 8.0, None),
            None
        );
    }

    #[test]
    fn drop_indicator_uses_single_midpoint_between_adjacent_pages() {
        let prev = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 140.0));
        let next = egui::Rect::from_min_max(egui::pos2(118.0, 20.0), egui::pos2(218.0, 140.0));
        let from_prev =
            book_reorder_drop_target_for_pos(prev, 1, 8, 4, 8.0, Some(egui::pos2(100.0, 80.0)));
        let from_next =
            book_reorder_drop_target_for_pos(next, 2, 8, 4, 8.0, Some(egui::pos2(130.0, 80.0)));

        assert_eq!(from_prev, Some((2, 114.0)));
        assert_eq!(from_next, Some((2, 114.0)));
    }

    #[test]
    fn drop_indicator_stays_attached_at_row_edges() {
        let row_start = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 140.0));
        let row_end = egui::Rect::from_min_max(egui::pos2(334.0, 20.0), egui::pos2(434.0, 140.0));

        assert_eq!(
            book_reorder_drop_target_for_pos(row_start, 4, 9, 4, 8.0, Some(egui::pos2(20.0, 80.0))),
            Some((4, row_start.left()))
        );
        assert_eq!(
            book_reorder_drop_target_for_pos(row_end, 3, 9, 4, 8.0, Some(egui::pos2(425.0, 80.0))),
            Some((4, row_end.right()))
        );
    }

    #[test]
    fn end_indicator_from_blank_cell_matches_last_page_edge() {
        let blank = egui::Rect::from_min_max(egui::pos2(118.0, 20.0), egui::pos2(218.0, 140.0));

        assert_eq!(book_reorder_end_indicator_x(blank, 1, 4, 8.0), 110.0);
    }

    #[test]
    fn adjusted_insert_index_accounts_for_removed_source() {
        assert_eq!(adjusted_book_reorder_insert_index(2, 5, 8), 4);
        assert_eq!(adjusted_book_reorder_insert_index(5, 2, 8), 2);
        assert_eq!(adjusted_book_reorder_insert_index(2, 99, 8), 7);
    }

    #[test]
    fn adjusted_group_insert_index_accounts_for_removed_selected_pages() {
        assert_eq!(adjusted_book_reorder_group_insert_index(&[1, 2], 5, 6), 3);
        assert_eq!(adjusted_book_reorder_group_insert_index(&[3, 4], 1, 6), 1);
        assert_eq!(adjusted_book_reorder_group_insert_index(&[1, 3], 4, 6), 2);
    }

    #[test]
    fn grid_columns_reserve_space_for_reorder_scrollbar() {
        assert_eq!(book_reorder_grid_columns(900.0, 78.0, 8.0), 10);
        assert_eq!(book_reorder_grid_columns(870.0, 78.0, 8.0), 9);
        assert_eq!(book_reorder_grid_columns(220.0, 78.0, 8.0), 4);
        assert_eq!(book_reorder_grid_columns(900.0, 64.0, 8.0), 12);
        assert_eq!(book_reorder_grid_columns(1540.0, 64.0, 8.0), 21);
    }

    #[test]
    fn scroll_height_tracks_resized_reorder_window() {
        assert_eq!(book_reorder_scroll_height(780.0, 20, 86.0), 780.0);
        assert_eq!(book_reorder_scroll_height(400.0, 20, 86.0), 400.0);
        assert_eq!(book_reorder_scroll_height(780.0, 2, 86.0), 172.0);
        assert_eq!(book_reorder_scroll_height(40.0, 20, 86.0), 86.0);
    }

    #[test]
    fn auto_scroll_delta_activates_near_reorder_edges() {
        assert_eq!(book_reorder_auto_scroll_delta(200.0, 100.0, 500.0), 0.0);
        assert!(book_reorder_auto_scroll_delta(112.0, 100.0, 500.0) < 0.0);
        assert!(book_reorder_auto_scroll_delta(488.0, 100.0, 500.0) > 0.0);
        assert_eq!(
            book_reorder_auto_scroll_delta(100.0, 100.0, 500.0),
            -BOOK_REORDER_AUTO_SCROLL_MAX_STEP_PX
        );
        assert_eq!(
            book_reorder_auto_scroll_delta(500.0, 100.0, 500.0),
            BOOK_REORDER_AUTO_SCROLL_MAX_STEP_PX
        );
    }

    #[test]
    fn keyboard_scroll_moves_reorder_view_without_selection_cursor() {
        let content_h = 2000.0;
        let viewport_h = 500.0;
        let row_h = 86.0;

        assert_eq!(
            book_reorder_keyboard_scroll_offset(
                300.0,
                content_h,
                viewport_h,
                row_h,
                BookReorderScrollKey::PageDown
            ),
            714.0
        );
        assert_eq!(
            book_reorder_keyboard_scroll_offset(
                300.0,
                content_h,
                viewport_h,
                row_h,
                BookReorderScrollKey::PageUp
            ),
            0.0
        );
        assert_eq!(
            book_reorder_keyboard_scroll_offset(
                300.0,
                content_h,
                viewport_h,
                row_h,
                BookReorderScrollKey::Home
            ),
            0.0
        );
        assert_eq!(
            book_reorder_keyboard_scroll_offset(
                300.0,
                content_h,
                viewport_h,
                row_h,
                BookReorderScrollKey::End
            ),
            1500.0
        );
    }

    #[test]
    fn moving_selected_group_preserves_internal_order() {
        let mut state = reorder_state(&["a", "b", "c", "d", "e"], &["b", "c"]);

        assert!(move_selected_book_reorder_group(&mut state, 5));

        assert_eq!(reorder_names(&state), ["a", "d", "e", "b", "c"]);
    }

    #[test]
    fn arrow_move_treats_adjacent_selection_as_one_block() {
        let mut state = reorder_state(&["a", "b", "c", "d"], &["b", "c"]);

        assert!(move_selected_book_reorder_by(&mut state, -1));
        assert_eq!(reorder_names(&state), ["b", "c", "a", "d"]);

        assert!(move_selected_book_reorder_by(&mut state, 1));
        assert_eq!(reorder_names(&state), ["a", "b", "c", "d"]);
    }
}

#[cfg(test)]
mod compute_cell_size_tests {
    use super::*;

    fn minimal_details_settings() -> crate::settings::Settings {
        let mut settings = crate::settings::Settings::default();
        settings.details_show_preview = false;
        settings.details_show_rating = false;
        settings.details_show_tags = false;
        settings.details_show_kind = false;
        settings.details_show_modified = false;
        settings.details_show_created = false;
        settings.details_show_state = false;
        settings.details_show_image_dimensions = false;
        settings.details_show_video_duration = false;
        settings.details_show_video_dimensions = false;
        settings.details_show_video_codec = false;
        settings
    }

    #[test]
    fn details_layout_overflows_when_saved_width_needs_it() {
        let mut settings = minimal_details_settings();
        assert!(set_details_column_width(
            &mut settings,
            DetailsColumn::Size,
            220.0
        ));

        // avail が狭くても、保存済みの広い列のために pane は名前列既定幅 + その列幅まで広がる。
        let layout = details_layout(200.0, 0.0, &settings);
        assert_eq!(layout.pane_w, DetailsColumn::Name.default_width() + 220.0);
        assert_eq!(layout.extent, layout.pane_w, "gutter 0 なら extent == pane");
    }

    #[test]
    fn details_layout_reserves_gutter_and_avoids_horizontal_scroll() {
        // 名前列が残り幅を埋める通常ケース。縦バー gutter を引いた幅に列を収めれば、
        // 総コンテンツ幅 (extent) は avail と一致 → 余計な横スクロールバーは出ない。
        let settings = minimal_details_settings(); // Name + Size(92)
        let gutter = 10.0;
        let layout = details_layout(600.0, gutter, &settings);
        assert!((layout.extent - 600.0).abs() < 0.01, "extent == avail");
        assert!(
            (layout.pane_w - (600.0 - gutter)).abs() < 0.01,
            "pane は gutter を引いた幅"
        );
        assert!(
            (layout.name_w - (600.0 - gutter - 92.0)).abs() < 0.01,
            "名前列が残り幅を埋める"
        );
    }

    #[test]
    fn details_layout_without_gutter_fills_pane() {
        let settings = minimal_details_settings();
        let layout = details_layout(600.0, 0.0, &settings);
        assert!((layout.extent - 600.0).abs() < 0.01);
        assert!((layout.pane_w - 600.0).abs() < 0.01);
    }

    #[test]
    fn details_layout_fixed_name_overflows_into_horizontal_scroll() {
        let mut settings = minimal_details_settings();
        settings.details_name_width_auto = false;
        settings.details_name_width = 500.0;
        let layout = details_layout(300.0, 10.0, &settings);
        assert!((layout.name_w - 500.0).abs() < 0.01, "固定幅を尊重");
        assert!(
            layout.extent > 300.0,
            "列が収まらないので横スクロールが必要 (extent > avail)"
        );
    }

    #[test]
    fn details_layout_fixed_name_smaller_than_pane_leaves_gap() {
        let mut settings = minimal_details_settings();
        settings.details_name_width_auto = false;
        settings.details_name_width = 120.0;
        let layout = details_layout(600.0, 10.0, &settings);
        assert!((layout.name_w - 120.0).abs() < 0.01);
        let columns_w = layout.name_w + details_fixed_columns_width(&settings);
        assert!(
            columns_w < layout.pane_w,
            "固定名前列が pane より狭いと右側に余白が残る"
        );
        assert!((layout.extent - 600.0).abs() < 0.01, "横スクロールは不要");
    }

    #[test]
    fn set_details_name_width_switches_to_fixed_and_is_idempotent() {
        let mut settings = crate::settings::Settings::default();
        assert!(settings.details_name_width_auto);
        assert!(set_details_name_width(&mut settings, 210.0));
        assert!(!settings.details_name_width_auto, "固定モードへ切替");
        assert!((settings.details_name_width - 210.0).abs() < 0.01);
        assert!(
            !set_details_name_width(&mut settings, 210.0),
            "同値なら変更なし"
        );
    }

    #[test]
    fn details_column_rects_fixed_name_uses_saved_width() {
        let mut settings = minimal_details_settings();
        settings.details_name_width_auto = false;
        settings.details_name_width = 150.0;
        let rects = details_column_rects(
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1000.0, 24.0)),
            &settings,
        );
        let name_rect = rects
            .iter()
            .find(|(col, _)| *col == DetailsColumn::Name)
            .map(|(_, r)| *r)
            .expect("name column rect");
        assert!(
            (name_rect.width() - 150.0).abs() < 0.01,
            "固定名前列は rect 幅でなく保存幅を使う"
        );
    }

    #[test]
    fn details_reorder_column_persists_dragged_order() {
        let mut settings = crate::settings::Settings::default();

        assert!(reorder_details_column(
            &mut settings,
            DetailsColumn::Size,
            DetailsColumn::Name,
            false
        ));

        let size_pos = settings
            .details_column_order
            .iter()
            .position(|id| *id == DetailsColumnId::Size)
            .expect("size column is present");
        let name_pos = settings
            .details_column_order
            .iter()
            .position(|id| *id == DetailsColumnId::Name)
            .expect("name column is present");
        assert!(size_pos < name_pos);
    }

    #[test]
    fn details_header_drag_uses_recorded_latest_pos_on_release() {
        let mut settings = crate::settings::Settings::default();
        let columns = details_column_rects(
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1200.0, 24.0)),
            &settings,
        );
        let size_rect = columns
            .iter()
            .find(|(col, _)| *col == DetailsColumn::Size)
            .map(|(_, rect)| *rect)
            .expect("size column rect");
        let name_rect = columns
            .iter()
            .find(|(col, _)| *col == DetailsColumn::Name)
            .map(|(_, rect)| *rect)
            .expect("name column rect");

        assert!(finish_details_header_drag(
            &mut settings,
            &columns,
            DetailsHeaderDrag {
                column: DetailsColumn::Size,
                start: size_rect.center(),
                latest: egui::pos2(name_rect.left() + 4.0, name_rect.center().y),
            },
            12.0,
        ));

        let size_pos = settings
            .details_column_order
            .iter()
            .position(|id| *id == DetailsColumnId::Size)
            .expect("size column is present");
        let name_pos = settings
            .details_column_order
            .iter()
            .position(|id| *id == DetailsColumnId::Name)
            .expect("name column is present");
        assert!(size_pos < name_pos);
    }

    #[test]
    fn returns_none_for_non_positive_width() {
        assert_eq!(compute_cell_size(0.0, 4, 1.0), None);
        assert_eq!(compute_cell_size(-10.0, 4, 1.0), None);
    }

    #[test]
    fn computes_cell_size_for_normal_window() {
        let (w, h) = compute_cell_size(800.0, 4, 1.0).expect("Some");
        assert_eq!(w, 200.0);
        assert_eq!(h, 200.0);
    }

    #[test]
    fn applies_height_ratio_to_cell_h() {
        let (w, h) = compute_cell_size(800.0, 4, 1.5).expect("Some");
        assert_eq!(w, 200.0);
        assert_eq!(h, 300.0);
    }

    /// **回帰テスト** (主目的): 狭幅 window で cell_w が MIN_CELL_PX (32px) 未満になると、
    /// `viewport_h / cell_h` が数千行に暴発して UI が固まるバグの再発検知。
    #[test]
    fn clamps_cell_w_to_min_when_window_too_narrow() {
        let (w, _) = compute_cell_size(100.0, 10, 1.0).expect("Some");
        assert!(w >= MIN_CELL_PX);
        assert_eq!(w, MIN_CELL_PX);
    }

    #[test]
    fn clamps_cell_h_to_min_when_aspect_ratio_extreme() {
        let (_, h) = compute_cell_size(800.0, 4, 0.05).expect("Some");
        assert_eq!(h, MIN_CELL_PX);
    }

    #[test]
    fn cols_zero_falls_back_to_one() {
        let (w, _) = compute_cell_size(800.0, 0, 1.0).expect("Some");
        assert_eq!(w, 800.0);
    }

    #[test]
    fn snapped_scroll_extent_tracks_current_viewport_height() {
        let natural_h = 1000.0;
        let row_h = 100.0;

        let (tall_total_h, tall_max_offset) = snapped_scroll_extent(natural_h, 600.0, row_h);
        let (short_total_h, short_max_offset) = snapped_scroll_extent(natural_h, 450.0, row_h);

        assert_eq!(tall_total_h, 1000.0);
        assert_eq!(tall_max_offset, 400.0);
        assert_eq!(short_total_h, 1050.0);
        assert_eq!(short_max_offset, 600.0);
    }

    #[test]
    fn snapped_scroll_extent_does_not_expand_short_content() {
        let (total_h, max_offset) = snapped_scroll_extent(280.0, 600.0, 100.0);

        assert_eq!(total_h, 280.0);
        assert_eq!(max_offset, 0.0);
    }
}

#[cfg(test)]
mod toolbar_reorder_tests {
    use super::*;
    use crate::settings::ToolbarSectionId as TS;

    fn rect(x: f32, y: f32, w: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, 20.0))
    }

    #[test]
    fn drop_index_single_row_picks_by_x() {
        // 1 行に 3 セクション: [A x=0..40][B x=50..90][C x=100..140] (y=0..20)。
        let anchors = [
            (TS::Cols, rect(0.0, 0.0, 40.0)),
            (TS::Aspect, rect(50.0, 0.0, 40.0)),
            (TS::Sort, rect(100.0, 0.0, 40.0)),
        ];
        // 一番左より前 → 0
        assert_eq!(toolbar_drop_index(&anchors, egui::pos2(-5.0, 10.0)), 0);
        // A の中心より左 (x<20) → 0
        assert_eq!(toolbar_drop_index(&anchors, egui::pos2(10.0, 10.0)), 0);
        // A の中心より右、B の中心より左 → 1
        assert_eq!(toolbar_drop_index(&anchors, egui::pos2(30.0, 10.0)), 1);
        // 末尾より右 → 3
        assert_eq!(toolbar_drop_index(&anchors, egui::pos2(200.0, 10.0)), 3);
    }

    #[test]
    fn drop_index_second_row_counts_first_row_as_before() {
        // 2 行: 行0 = [A,B] (y=0..20), 行1 = [C] (y=30..50)。
        let anchors = [
            (TS::Cols, rect(0.0, 0.0, 40.0)),
            (TS::Aspect, rect(50.0, 0.0, 40.0)),
            (TS::Sort, rect(0.0, 30.0, 40.0)),
        ];
        // 行1 の C 中心より左 → 行0 の 2 つが「上の行」で手前 → 2
        assert_eq!(toolbar_drop_index(&anchors, egui::pos2(5.0, 40.0)), 2);
        // 行1 の C 中心より右 → 3
        assert_eq!(toolbar_drop_index(&anchors, egui::pos2(35.0, 40.0)), 3);
    }

    #[test]
    fn reorder_moves_before_target() {
        let order = TS::default_order().to_vec();
        // Tags を Cols の手前へ。
        let got = reorder_toolbar_section(&order, TS::Tags, Some(TS::Cols), None).unwrap();
        let cols_pos = got.iter().position(|&s| s == TS::Cols).unwrap();
        let tags_pos = got.iter().position(|&s| s == TS::Tags).unwrap();
        assert_eq!(tags_pos + 1, cols_pos, "Tags は Cols の直前に来る");
        assert_eq!(got.len(), order.len(), "全セクションが保たれる");
    }

    #[test]
    fn reorder_to_end_uses_last_visible() {
        let order = TS::default_order().to_vec();
        // FolderTree を末尾 (最後の可視 = Tags) の直後へ。
        let got = reorder_toolbar_section(&order, TS::FolderTree, None, Some(TS::Tags)).unwrap();
        assert_eq!(*got.last().unwrap(), TS::FolderTree);
    }

    #[test]
    fn reorder_noop_returns_none() {
        let order = TS::default_order().to_vec();
        // Bookshelf を Cols (= 元々その直後) の手前へ → 位置不変。
        assert!(reorder_toolbar_section(&order, TS::Bookshelf, Some(TS::Cols), None).is_none());
    }

    #[test]
    fn reorder_before_self_is_noop() {
        // 掴んでいるセクション自身の左半分でドロップ → before==dragged → 移動なし。
        // (回帰: 以前は last_visible 経由で末尾へ誤移動して保存されていた。Codex P2)
        let order = TS::default_order().to_vec();
        assert!(
            reorder_toolbar_section(&order, TS::Bookshelf, Some(TS::Bookshelf), Some(TS::Tags))
                .is_none(),
            "自分自身の手前ドロップは no-op であるべき"
        );
    }

    #[test]
    fn reorder_keeps_hidden_sections_relative_order() {
        // 非表示も含む全順序を current_order に渡す前提。可視= [Cols, Sort] だけで
        // Sort を Cols の手前へ動かしても、間にある (非表示の) Aspect は保たれる。
        let order = vec![TS::Cols, TS::Aspect, TS::Sort, TS::Tags];
        let got = reorder_toolbar_section(&order, TS::Sort, Some(TS::Cols), None).unwrap();
        assert_eq!(got, vec![TS::Sort, TS::Cols, TS::Aspect, TS::Tags]);
    }
}

#[cfg(test)]
mod decide_drag_payload_tests {
    use super::*;
    use std::collections::HashSet;

    fn img(p: &str) -> GridItem {
        GridItem::Image(PathBuf::from(p))
    }
    fn zip_img(zip: &str, entry: &str) -> GridItem {
        GridItem::ZipImage {
            zip_path: PathBuf::from(zip),
            entry_name: entry.to_string(),
        }
    }

    #[test]
    fn single_real_file_not_checked() {
        let items = vec![img(r"C:\a.jpg"), img(r"C:\b.jpg")];
        let checked = HashSet::new();
        match decide_drag_payload(&items, &checked, 1) {
            DragDecision::Start {
                paths,
                post_drag_toast,
            } => {
                assert_eq!(paths, vec![PathBuf::from(r"C:\b.jpg")]);
                assert!(post_drag_toast.is_none());
            }
            _ => panic!("expected Start"),
        }
    }

    #[test]
    fn single_folder_not_checked_is_draggable() {
        let items = vec![GridItem::Folder(PathBuf::from(r"C:\dir"))];
        let checked = HashSet::new();
        match decide_drag_payload(&items, &checked, 0) {
            DragDecision::Start { paths, .. } => {
                assert_eq!(paths, vec![PathBuf::from(r"C:\dir")]);
            }
            _ => panic!("expected Start"),
        }
    }

    #[test]
    fn single_virtual_item_is_noop() {
        let items = vec![zip_img(r"C:\a.zip", "p1.jpg")];
        let checked = HashSet::new();
        assert!(matches!(
            decide_drag_payload(&items, &checked, 0),
            DragDecision::None
        ));
    }

    #[test]
    fn multi_selection_all_real_in_index_order() {
        let items = vec![img(r"C:\a.jpg"), img(r"C:\b.jpg"), img(r"C:\c.jpg")];
        // HashSet の反復順は不定。payload が index 昇順に並ぶことを検証する。
        let checked: HashSet<usize> = [2, 0, 1].into_iter().collect();
        match decide_drag_payload(&items, &checked, 2) {
            DragDecision::Start {
                paths,
                post_drag_toast,
            } => {
                assert_eq!(
                    paths,
                    vec![
                        PathBuf::from(r"C:\a.jpg"),
                        PathBuf::from(r"C:\b.jpg"),
                        PathBuf::from(r"C:\c.jpg"),
                    ]
                );
                assert!(post_drag_toast.is_none());
            }
            _ => panic!("expected Start"),
        }
    }

    #[test]
    fn mixed_selection_excludes_virtual_with_toast() {
        let items = vec![
            img(r"C:\a.jpg"),
            zip_img(r"C:\b.zip", "p1.jpg"),
            img(r"C:\c.jpg"),
        ];
        let checked: HashSet<usize> = [0, 1, 2].into_iter().collect();
        match decide_drag_payload(&items, &checked, 0) {
            DragDecision::Start {
                paths,
                post_drag_toast,
            } => {
                // 仮想アイテム (idx 1) は除外され実ファイルだけ残る。
                assert_eq!(
                    paths,
                    vec![PathBuf::from(r"C:\a.jpg"), PathBuf::from(r"C:\c.jpg")]
                );
                // 除外があったので完了後トーストが付く。
                assert!(post_drag_toast.is_some());
            }
            _ => panic!("expected Start"),
        }
    }

    #[test]
    fn all_virtual_selection_is_immediate_toast() {
        let items = vec![
            zip_img(r"C:\a.zip", "p1.jpg"),
            zip_img(r"C:\a.zip", "p2.jpg"),
        ];
        let checked: HashSet<usize> = [0, 1].into_iter().collect();
        assert!(matches!(
            decide_drag_payload(&items, &checked, 0),
            DragDecision::ImmediateToast(_)
        ));
    }
}
