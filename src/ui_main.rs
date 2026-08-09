//! メイン画面の UI コンポーネント描画。
//!
//! `App::update()` から呼ばれるメニューバー・ツールバー・フォルダバー・
//! グリッド・進捗オーバーレイ・選択情報オーバーレイの描画メソッドを集約。

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use eframe::egui;

use crate::app::{
    App, FacetField, GridClickSelectionAnchor, GridScrollIntent, LazyColumnState,
    QuickFolderSlotId, QuickFolderSwitchTarget,
};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::keymap::{KeyAction, Keymap, MenuCommandId, TopMenuId, resolve_menu_layout};
use crate::settings::{
    DetailsColumnId, DetailsColumnWidth, DetailsRowStyle, DetailsSelectionBarMode,
    DetailsSizeDisplayMode, DetailsSortKey, FacetCalendarDate, FacetDatePreset, FacetEditFlag,
    FacetItemKind, FacetSizePreset, FacetSizeUnit, FacetSizeValue, FacetTagMode,
    GridClickSelectionMode, GridViewMode,
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
// 詳細ヘッダ + 詳細行 + パネル内の最小余白。下部情報バーはこの 2 行に加えて
// solid 横スクロールバー専用レーンを CentralPanel より先に予約する。
const SELECTION_INFO_BAR_CONTENT_HEIGHT: f32 = 58.0;
// 列境界のダブルクリックは UI thread 上で egui の font atlas を使って文字幅を測る。
// 全行の exact max を維持したまま 1 frame の仕事量を一定にするため、分割 job で測る。
const DETAILS_BEST_FIT_ROWS_PER_FRAME: usize = 192;
const DETAILS_BEST_FIT_HORIZONTAL_PADDING: f32 = 14.0;
const DETAILS_BEST_FIT_MAX_WIDTH: f32 = 800.0;
const DETAILS_RATING_BEST_FIT_SEED: &str = "★★★★★";
const DETAILS_STATE_BEST_FIT_SEED: &str = "補 レ 消 隠 文 回 ピ";
// ScrollArea 本体の外にある popup frame と、上下の配置余白を合わせて確保する。
const DETAILS_COLUMN_MENU_SCREEN_MARGIN: f32 = 48.0;
const DETAILS_COLUMN_MENU_COLUMNS_WIDTH: f32 = 240.0;
const DETAILS_COLUMN_MENU_FORMAT_WIDTH: f32 = 200.0;
// egui::Separator::default() と同じ占有幅。高さ 0 の gap として確保し、線は pane 実測後に描く。
const DETAILS_COLUMN_MENU_SEPARATOR_WIDTH: f32 = 6.0;
// content / viewport 端の物理 pixel 丸めで scrollbar が出ないよう 2 device px を足す。
const DETAILS_COLUMN_MENU_HEIGHT_ROUNDING_SLACK_PX: f32 = 2.0;
// 自動サイズ popup の初回だけ使う。実測値は次フレームから menu 固有 Id に保持する。
const DETAILS_COLUMN_MENU_INITIAL_CONTENT_HEIGHT: f32 = 420.0;
const DETAILS_COLUMN_MENU_CONTENT_HEIGHT_ID: u64 = 0xD37A_115C_01_u64;
const DETAILS_LAYOUT_DEBUG_ENV: &str = "MIV_DETAILS_LAYOUT_DEBUG";
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

#[derive(Clone, Copy)]
enum DetailsColumnMenuPane {
    Columns,
    Format,
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
    Bookmarks,
    RatingViewBack,
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

fn checked_selection_overlay_label(checked_count: usize) -> Option<String> {
    (checked_count > 0).then(|| format!("{checked_count} 件"))
}

/// オーバーレイの下地色。文字と枠は `warn_fg_color` を載せるので、それが読める明度にする。
/// 白い一覧の上で埋もれないことが目的なので、`Frame::popup` の既定 fill は使わない。
fn checked_selection_overlay_fill(dark_mode: bool) -> egui::Color32 {
    match dark_mode {
        false => egui::Color32::from_rgb(255, 243, 209),
        true => egui::Color32::from_rgb(66, 50, 20),
    }
}

/// チェック件数をメイン一覧の右上へ表示する。「選択解除」が押されたら true を返す。
fn show_checked_selection_overlay(ctx: &egui::Context, checked_count: usize) -> bool {
    let Some(count_label) = checked_selection_overlay_label(checked_count) else {
        return false;
    };
    let top_offset = ctx.available_rect().top() + 8.0;
    let mut clear_clicked = false;
    egui::Area::new(egui::Id::new("checked_selection_count_overlay"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, top_offset))
        .show(ctx, |ui| {
            let accent = ui.visuals().warn_fg_color;
            egui::Frame::popup(ui.style())
                .fill(checked_selection_overlay_fill(ui.visuals().dark_mode))
                .stroke(egui::Stroke::new(1.5, accent))
                .show(ui, |ui| {
                    // Area の幅は自動なので、中央寄せ系のレイアウトは available_width を
                    // 画面端まで取ってしまう。縦積みだけに留める。
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("チェック").color(accent));
                        ui.label(
                            egui::RichText::new(count_label)
                                .strong()
                                .size(18.0)
                                .color(accent),
                        );
                        clear_clicked = ui
                            .button("選択解除")
                            .on_hover_text("チェックをすべて解除 (Ctrl+D)")
                            .clicked();
                    });
                });
        });
    clear_clicked
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

const GRID_TOUCH_REMAINDER_EPSILON: f32 = 0.01;
/// A release may settle against the last scroll direction only while the
/// viewport is within 15% of the row it is leaving. This absorbs a barely
/// started drag without allowing the half-row reversal seen with nearest-row
/// snapping; keep it centralized for real-device tuning.
const GRID_TOUCH_SETTLE_REVERSAL_TOLERANCE: f32 = 0.15;
/// Snap travel below 20% of a row remains instant because easing a few pixels
/// felt sluggish on the real touch display. Longer travel uses a short 130 ms
/// ease-out so a direction-of-travel snap covering most of a row reads as
/// continuous motion rather than a jump. Keep both values together for
/// real-device tuning.
const GRID_TOUCH_SNAP_GLIDE_MIN_TRAVEL_RATIO: f32 = 0.20;
const GRID_TOUCH_SNAP_GLIDE_DURATION: std::time::Duration = std::time::Duration::from_millis(130);
/// A list pinch must accumulate a 25% scale change before moving one column.
/// Its reciprocal (0.8) is the contraction threshold, so small sample noise
/// cancels around 1.0 instead of making the discrete column count oscillate.
const GRID_PINCH_COLUMN_STEP_RATIO: f32 = 1.25;

pub(crate) fn grid_touch_fraction_is_visible(remainder_y: f32) -> bool {
    remainder_y > GRID_TOUCH_REMAINDER_EPSILON
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct GridTouchScrollPosition {
    anchor_y: f32,
    remainder_y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GridTouchScrollDirection {
    /// The displayed scroll position (anchor plus remainder) increased.
    Increasing,
    /// The displayed scroll position (anchor plus remainder) decreased.
    Decreasing,
}

#[derive(Clone, Copy, Debug, Default)]
struct GridTouchScrollState {
    anchor_y: f32,
    remainder_y: f32,
    row_h: f32,
    items_generation: u64,
    phase: GridTouchScrollPhase,
}

#[derive(Clone, Copy, Debug)]
enum GridTouchScrollPhase {
    Contact {
        direction: Option<GridTouchScrollDirection>,
    },
    Glide {
        animation: GridTouchSnapAnimation,
        started_at: std::time::Instant,
    },
}

impl Default for GridTouchScrollPhase {
    fn default() -> Self {
        Self::Contact { direction: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GridTouchSnapAnimation {
    start: GridTouchScrollPosition,
    target: GridTouchScrollPosition,
    row_h: f32,
}

impl GridTouchSnapAnimation {
    fn travel_y(self) -> f32 {
        ((self.target.anchor_y + self.target.remainder_y)
            - (self.start.anchor_y + self.start.remainder_y))
            .abs()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum GridTouchSnapPlan {
    Immediate(GridTouchScrollPosition),
    Glide(GridTouchSnapAnimation),
}

/// Applies incremental finger motion without ever making the canonical grid
/// offset fractional. `anchor_y` is reconstructed from an integer row on
/// every return; only `remainder_y` may sit between rows.
fn apply_grid_touch_scroll_delta(
    anchor_y: f32,
    remainder_y: f32,
    finger_delta_y: f32,
    row_h: f32,
    max_offset: f32,
) -> GridTouchScrollPosition {
    let row_h = row_h.max(1.0);
    let max_row = (max_offset.max(0.0) / row_h).round().max(0.0);
    let max_anchor = max_row * row_h;
    let anchor_row = (anchor_y / row_h).round().clamp(0.0, max_row);
    let anchor_y = anchor_row * row_h;
    let remainder_y = if remainder_y.is_finite() {
        remainder_y.clamp(0.0, row_h - GRID_TOUCH_REMAINDER_EPSILON)
    } else {
        0.0
    };
    let finger_delta_y = if finger_delta_y.is_finite() {
        finger_delta_y
    } else {
        0.0
    };
    let effective_y = (anchor_y + remainder_y - finger_delta_y).clamp(0.0, max_anchor);
    let mut row = (effective_y / row_h).floor().clamp(0.0, max_row);
    let mut remainder_y = effective_y - row * row_h;

    if remainder_y <= GRID_TOUCH_REMAINDER_EPSILON {
        remainder_y = 0.0;
    } else if row < max_row && row_h - remainder_y <= GRID_TOUCH_REMAINDER_EPSILON {
        row += 1.0;
        remainder_y = 0.0;
    }

    GridTouchScrollPosition {
        anchor_y: row * row_h,
        remainder_y,
    }
}

fn grid_touch_direction_after_move(
    before_y: f32,
    after_y: f32,
    previous: Option<GridTouchScrollDirection>,
) -> Option<GridTouchScrollDirection> {
    if after_y > before_y + GRID_TOUCH_REMAINDER_EPSILON {
        Some(GridTouchScrollDirection::Increasing)
    } else if after_y < before_y - GRID_TOUCH_REMAINDER_EPSILON {
        Some(GridTouchScrollDirection::Decreasing)
    } else {
        previous
    }
}

/// Commits the fractional drawing position along the last direction of
/// travel. A reversal is allowed only inside the centralized row tolerance;
/// unknown direction retains nearest-row behavior.
fn settle_grid_touch_scroll(
    anchor_y: f32,
    remainder_y: f32,
    row_h: f32,
    max_offset: f32,
    direction: Option<GridTouchScrollDirection>,
) -> GridTouchScrollPosition {
    let row_h = row_h.max(1.0);
    let max_row = (max_offset.max(0.0) / row_h).round().max(0.0);
    let mut row = (anchor_y / row_h).round().clamp(0.0, max_row);
    let remainder_y = if remainder_y.is_finite() {
        remainder_y.clamp(0.0, row_h - GRID_TOUCH_REMAINDER_EPSILON)
    } else {
        0.0
    };
    let reversal_tolerance = row_h * GRID_TOUCH_SETTLE_REVERSAL_TOLERANCE;
    let advance_row = match direction {
        Some(GridTouchScrollDirection::Increasing) => remainder_y > reversal_tolerance,
        Some(GridTouchScrollDirection::Decreasing) => remainder_y >= row_h - reversal_tolerance,
        None => remainder_y >= row_h * 0.5,
    };
    if advance_row && row < max_row {
        row += 1.0;
    }
    GridTouchScrollPosition {
        anchor_y: row * row_h,
        remainder_y: 0.0,
    }
}

/// Chooses between an immediate release commit and a short remainder-only
/// glide. The returned animation keeps start.anchor_y unchanged until its
/// terminal frame; only that frame publishes the row-aligned target anchor.
fn plan_grid_touch_snap(
    anchor_y: f32,
    remainder_y: f32,
    row_h: f32,
    max_offset: f32,
    direction: Option<GridTouchScrollDirection>,
) -> GridTouchSnapPlan {
    let row_h = row_h.max(1.0);
    let start = apply_grid_touch_scroll_delta(anchor_y, remainder_y, 0.0, row_h, max_offset);
    let target = settle_grid_touch_scroll(
        start.anchor_y,
        start.remainder_y,
        row_h,
        max_offset,
        direction,
    );
    let animation = GridTouchSnapAnimation {
        start,
        target,
        row_h,
    };
    if animation.travel_y() < row_h * GRID_TOUCH_SNAP_GLIDE_MIN_TRAVEL_RATIO {
        GridTouchSnapPlan::Immediate(target)
    } else {
        GridTouchSnapPlan::Glide(animation)
    }
}

/// Pure cubic ease-out progression for a grid snap glide.
///
/// Before completion the canonical anchor is exactly the starting row and the
/// interpolated motion lives only in remainder_y. At or beyond 130 ms the
/// exact row-aligned target is returned, which is the glide's sole anchor
/// commit.
fn grid_touch_snap_position_at(
    animation: GridTouchSnapAnimation,
    elapsed: std::time::Duration,
) -> GridTouchScrollPosition {
    if elapsed >= GRID_TOUCH_SNAP_GLIDE_DURATION {
        return animation.target;
    }

    let linear = elapsed.as_secs_f32() / GRID_TOUCH_SNAP_GLIDE_DURATION.as_secs_f32();
    let eased = 1.0 - (1.0 - linear.clamp(0.0, 1.0)).powi(3);
    let start_y = animation.start.anchor_y + animation.start.remainder_y;
    let target_y = animation.target.anchor_y + animation.target.remainder_y;
    GridTouchScrollPosition {
        anchor_y: animation.start.anchor_y,
        remainder_y: (start_y + (target_y - start_y) * eased - animation.start.anchor_y).clamp(
            GRID_TOUCH_REMAINDER_EPSILON,
            animation.row_h - GRID_TOUCH_REMAINDER_EPSILON,
        ),
    }
}

fn grid_touch_snap_needs_animation_repaint(phase: GridTouchScrollPhase) -> bool {
    matches!(phase, GridTouchScrollPhase::Glide { .. })
}

/// A fractional viewport reveals the next row at the bottom. Extend both the
/// strict visible end and the retention end by exactly one row while it is
/// present.
pub(crate) fn extend_grid_end_for_touch_fraction(
    base_end: usize,
    cols: usize,
    total: usize,
    remainder_y: f32,
) -> usize {
    if grid_touch_fraction_is_visible(remainder_y) {
        base_end.saturating_add(cols.max(1)).min(total)
    } else {
        base_end.min(total)
    }
}

fn grid_touch_scroll_state_id(ctx: &egui::Context) -> egui::Id {
    egui::Id::new(("miv_grid_touch_scroll_fraction", ctx.viewport_id()))
}

fn grid_touch_scroll_state(
    ctx: &egui::Context,
    anchor_y: f32,
    row_h: f32,
    items_generation: u64,
) -> GridTouchScrollState {
    let id = grid_touch_scroll_state_id(ctx);
    ctx.data(|data| data.get_temp::<GridTouchScrollState>(id))
        .filter(|state| {
            state.items_generation == items_generation
                && (state.anchor_y - anchor_y).abs() <= GRID_TOUCH_REMAINDER_EPSILON
                && (state.row_h - row_h).abs() <= 0.5
        })
        .unwrap_or(GridTouchScrollState {
            anchor_y,
            remainder_y: 0.0,
            row_h,
            items_generation,
            phase: GridTouchScrollPhase::Contact { direction: None },
        })
}

pub(crate) fn grid_touch_scroll_remainder(
    ctx: &egui::Context,
    anchor_y: f32,
    row_h: f32,
    items_generation: u64,
) -> f32 {
    grid_touch_scroll_state(ctx, anchor_y, row_h, items_generation).remainder_y
}

fn set_grid_touch_scroll_state(
    ctx: &egui::Context,
    anchor_y: f32,
    row_h: f32,
    items_generation: u64,
    remainder_y: f32,
    phase: GridTouchScrollPhase,
) {
    let id = grid_touch_scroll_state_id(ctx);
    ctx.data_mut(|data| {
        if grid_touch_fraction_is_visible(remainder_y) {
            data.insert_temp(
                id,
                GridTouchScrollState {
                    anchor_y,
                    remainder_y,
                    row_h,
                    items_generation,
                    phase,
                },
            );
        } else {
            data.remove_temp::<GridTouchScrollState>(id);
        }
    });
}

fn clear_grid_touch_scroll_remainder(ctx: &egui::Context) {
    let id = grid_touch_scroll_state_id(ctx);
    ctx.data_mut(|data| {
        data.remove_temp::<GridTouchScrollState>(id);
    });
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GridPinchColumnState {
    accumulated_scale: f32,
    columns_changed: bool,
}

impl Default for GridPinchColumnState {
    fn default() -> Self {
        Self {
            accumulated_scale: 1.0,
            columns_changed: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GridPinchColumnUpdate {
    new_cols: usize,
    accumulated_scale: f32,
    threshold_crossed: bool,
    columns_changed: bool,
}

fn accumulate_grid_pinch_column_factor(
    state: &mut GridPinchColumnState,
    current_cols: usize,
    factor: f32,
) -> GridPinchColumnUpdate {
    let current_cols = current_cols.clamp(
        crate::settings::MIN_GRID_COLS,
        crate::settings::MAX_GRID_COLS,
    );
    if factor.is_finite() && factor > 0.0 {
        let accumulated = state.accumulated_scale * factor;
        if accumulated.is_finite() && accumulated > 0.0 {
            state.accumulated_scale = accumulated;
        }
    }

    let delta = if state.accumulated_scale >= GRID_PINCH_COLUMN_STEP_RATIO {
        -1
    } else if state.accumulated_scale <= GRID_PINCH_COLUMN_STEP_RATIO.recip() {
        1
    } else {
        0
    };
    if delta == 0 {
        return GridPinchColumnUpdate {
            new_cols: current_cols,
            accumulated_scale: state.accumulated_scale,
            threshold_crossed: false,
            columns_changed: false,
        };
    }

    // A threshold represents exactly one discrete step, including at a clamp
    // boundary. Resetting there gives a reversal the same neutral 1.0 base.
    state.accumulated_scale = 1.0;
    let new_cols = (current_cols as i32 + delta).clamp(
        crate::settings::MIN_GRID_COLS as i32,
        crate::settings::MAX_GRID_COLS as i32,
    ) as usize;
    let columns_changed = new_cols != current_cols;
    state.columns_changed |= columns_changed;
    GridPinchColumnUpdate {
        new_cols,
        accumulated_scale: state.accumulated_scale,
        threshold_crossed: true,
        columns_changed,
    }
}

fn grid_pinch_column_state_id(ctx: &egui::Context) -> egui::Id {
    egui::Id::new(("miv_grid_pinch_columns", ctx.viewport_id()))
}

fn apply_grid_pinch_column_factor(
    ctx: &egui::Context,
    current_cols: usize,
    factor: f32,
) -> GridPinchColumnUpdate {
    let id = grid_pinch_column_state_id(ctx);
    ctx.data_mut(|data| {
        let mut state = data
            .get_temp::<GridPinchColumnState>(id)
            .unwrap_or_default();
        let update = accumulate_grid_pinch_column_factor(&mut state, current_cols, factor);
        data.insert_temp(id, state);
        update
    })
}

/// Removes the per-viewport pinch state at the gesture boundary. Returning
/// the dirty bit only once makes this the sole gate for the deferred save.
fn take_grid_pinch_columns_save(ctx: &egui::Context) -> bool {
    let id = grid_pinch_column_state_id(ctx);
    ctx.data_mut(|data| {
        let should_save = data
            .get_temp::<GridPinchColumnState>(id)
            .is_some_and(|state| state.columns_changed);
        data.remove_temp::<GridPinchColumnState>(id);
        should_save
    })
}

fn finish_grid_pinch_column_gesture(app: &mut App, ctx: &egui::Context, boundary: &str) {
    let should_save = take_grid_pinch_columns_save(ctx);
    if should_save {
        app.settings.save();
    }
    // The disabled path calls this every frame, so a boundary with nothing
    // pending must stay silent or it drowns the log we ask users to capture.
    if should_save && crate::touch_debug::touch_debug_enabled() {
        crate::logger::log(format!(
            "[TOUCH-DEBUG] grid_pinch_columns boundary={boundary} saved={should_save} cols={}",
            app.settings.grid_cols
        ));
    }
}

pub(crate) fn grid_wheel_scroll_offset(current: f32, scroll_delta_y: f32, row_h: f32) -> f32 {
    let row_h = row_h.max(1.0);
    let direction = -scroll_delta_y.signum();
    let offset = (current + direction * row_h).max(0.0);
    (offset / row_h).round() * row_h
}

fn should_sync_grid_scrollbar(
    touch_derived_pointer_activity: bool,
    fractional_drag_y: f32,
    egui_offset: f32,
    anchor_y: f32,
    row_h: f32,
) -> bool {
    !touch_derived_pointer_activity
        && !grid_touch_fraction_is_visible(fractional_drag_y)
        && (egui_offset - anchor_y).abs() > row_h * 0.5
}

fn resolve_grid_scroll_offset(
    current: f32,
    max_offset: f32,
    intent: Option<GridScrollIntent>,
) -> f32 {
    let max_offset = max_offset.max(0.0);
    match intent {
        Some(GridScrollIntent::Top) => 0.0,
        Some(GridScrollIntent::Bottom) => max_offset,
        None => current.clamp(0.0, max_offset),
    }
}

/// Claims only the request that existed when the current layout extent became known.
/// A ring / gesture action may enqueue another request later in the same frame; callers
/// must not clear the field after drawing or that new request would be lost.
fn take_grid_scroll_for_current_layout(
    pending: &mut Option<GridScrollIntent>,
) -> Option<GridScrollIntent> {
    pending.take()
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

fn thumbnail_item_counts(items: &[GridItem], visible_indices: &[usize]) -> (usize, usize) {
    // 全 GridItem が一覧上の 1 項目に対応するため、数百万件でも長さを読むだけでよい。
    (items.len(), visible_indices.len())
}

fn thumbnail_count_label(items: &[GridItem], visible_indices: &[usize]) -> String {
    let (total, visible) = thumbnail_item_counts(items, visible_indices);
    let width = total.max(1).to_string().len();
    format!("({:>width$}/{})", visible, total, width = width)
}

fn filtered_count_label(items: &[GridItem], visible_indices: &[usize]) -> String {
    let (total, visible) = thumbnail_item_counts(items, visible_indices);
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

fn ui_scale_menu_button(ui: &mut egui::Ui, checked: bool, percent: u32) -> egui::Response {
    let prefix = if checked { "✓ " } else { "  " };
    // The scaling submenu is only as wide as its unselected numeric labels. If the check mark
    // makes the selected label slightly wider, the default wrap mode can split `✓ 100%` across
    // two rows. Let this one menu row extend the popup width instead.
    ui.add(egui::Button::new(format!("{prefix}{percent}%")).wrap_mode(egui::TextWrapMode::Extend))
}

fn draw_facet_calendar_date_row(
    ui: &mut egui::Ui,
    label: &str,
    date: &mut Option<FacetCalendarDate>,
) -> bool {
    let before = *date;
    let mut enabled = date.is_some();
    let mut value = date.unwrap_or_else(FacetCalendarDate::today_local);
    ui.horizontal(|ui| {
        ui.checkbox(&mut enabled, label);
        ui.add_enabled_ui(enabled, |ui| {
            ui.add(
                egui::DragValue::new(&mut value.year)
                    .range(1970..=9999)
                    .suffix("年"),
            );
            ui.add(
                egui::DragValue::new(&mut value.month)
                    .range(1..=12)
                    .suffix("月"),
            );
            ui.add(
                egui::DragValue::new(&mut value.day)
                    .range(1..=31)
                    .suffix("日"),
            );
        });
    });
    value.sanitize();
    *date = enabled.then_some(value);
    *date != before
}

/// サイズ範囲の片側 1 行を描く。
///
/// 関係語 (`以上` / `未満`) は値の**後ろ**へ置く。前に置くと「最大 100 KB 未満」のように
/// 日本語として読めない並びになる (実機確認で指摘された)。行全体で
/// 「下限 100 KB 以上」「上限 1 MB 未満」と読める並びにする。
pub(crate) fn draw_facet_size_value_row(
    ui: &mut egui::Ui,
    label: &str,
    relation: &str,
    id: &'static str,
    value: &mut Option<FacetSizeValue>,
    default: FacetSizeValue,
) -> bool {
    let before = *value;
    let mut enabled = value.is_some();
    let mut current = value.unwrap_or(default);
    ui.horizontal(|ui| {
        ui.checkbox(&mut enabled, label);
        ui.add_enabled_ui(enabled, |ui| {
            ui.add(
                egui::DragValue::new(&mut current.value)
                    .range(0..=u32::MAX)
                    .speed(1),
            );
            egui::ComboBox::from_id_salt(("facet_size_unit", id))
                .selected_text(current.unit.label())
                .show_ui(ui, |ui| {
                    for unit in FacetSizeUnit::ALL {
                        ui.selectable_value(&mut current.unit, unit, unit.label());
                    }
                });
            ui.label(relation);
        });
    });
    *value = enabled.then_some(current);
    *value != before
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
    if egui::Popup::is_id_open(ctx, egui::Popup::default_response_id(response))
        || egui::Popup::is_id_open(ctx, sticky_context_menu_id(response))
    {
        consume_wheel_input(ctx);
    }
}

fn sticky_context_menu_id(response: &egui::Response) -> egui::Id {
    response.id.with("sticky_context_menu")
}

fn show_sticky_context_menu(
    response: &egui::Response,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::Id {
    // Keep the secondary-click settings menu on a separate popup id from a possible
    // primary-click menu_button on the same response. egui::Popup::context_menu
    // explicitly closes its popup id on primary clicks.
    let popup_id = sticky_context_menu_id(response);
    let _ = egui::Popup::context_menu(response)
        .id(popup_id)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(add_contents);
    popup_id
}

fn draw_sticky_settings_menu_header(ui: &mut egui::Ui, title: &str, show_close_button: bool) {
    ui.set_min_width(220.0);
    // A bare right-to-left child consumes the popup's entire remaining available width, so the
    // header alone decided the popup width: the toolbar submenu grew to the screen edge (~605px)
    // and egui gave up on placing it beside its parent item, dropping it on top of the entries
    // below instead. Bound the row to the width the menu has already claimed so the close button
    // stays right-aligned without the header widening anything. Menus that want to be wide
    // (place / tag facets) call `set_width` before this and keep their own width.
    let row_size = egui::vec2(ui.min_size().x, ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(
        row_size,
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            if show_close_button && ui.small_button("×").on_hover_text("閉じる").clicked() {
                ui.close();
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(title).strong());
            });
        },
    );
}

#[cfg(test)]
mod sticky_settings_menu_header_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn header_width_does_not_follow_available_screen_width() {
        use egui_kittest::Harness;

        let measured_width = Arc::new(Mutex::new(None));
        let measured_width_in_ui = measured_width.clone();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(800.0, 180.0))
            .build(move |ctx| {
                let area = egui::Area::new(egui::Id::new("sticky_settings_menu_header_test"))
                    .fixed_pos(egui::pos2(0.0, 0.0))
                    .movable(false)
                    .show(ctx, |ui| {
                        draw_sticky_settings_menu_header(ui, "表示するセクション", true);
                    });
                *measured_width_in_ui.lock().unwrap() = Some(area.response.rect.width());
            });
        harness.run();

        let width = measured_width.lock().unwrap().expect("header was rendered");
        assert!(
            width >= 220.0,
            "header was narrower than its minimum: {width}"
        );
        assert!(
            width <= 280.0,
            "header consumed the 800px available width: {width}"
        );
    }

    #[test]
    fn sticky_menu_keeps_checkbox_clicks_open_and_closes_explicitly() {
        use egui_kittest::{Harness, kittest::Queryable};

        let dedicated = Arc::new(Mutex::new(false));
        let dedicated_in_ui = Arc::clone(&dedicated);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(640.0, 360.0))
            .build(move |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        let response = ui.button("列メニュー");
                        ui.add_space(360.0);
                        let _ = ui.button("外側");
                        show_sticky_context_menu(&response, |ui| {
                            draw_sticky_settings_menu_header(ui, "列の設定", true);
                            let mut checked = *dedicated_in_ui.lock().unwrap();
                            if ui
                                .checkbox(&mut checked, "以下を専用の設定にする")
                                .changed()
                            {
                                *dedicated_in_ui.lock().unwrap() = checked;
                            }
                        });
                    });
                });
            });
        harness.run();

        harness.get_by_label("列メニュー").click_secondary();
        harness.run();
        assert!(harness.query_by_label("以下を専用の設定にする").is_some());

        harness.get_by_label("以下を専用の設定にする").click();
        harness.run();
        assert!(*dedicated.lock().unwrap());
        assert!(
            harness.query_by_label("以下を専用の設定にする").is_some(),
            "checkbox click closed a sticky menu"
        );

        harness.get_by_label("×").click();
        harness.run();
        assert!(
            harness.query_by_label("以下を専用の設定にする").is_none(),
            "close button left the sticky menu open"
        );

        harness.get_by_label("列メニュー").click_secondary();
        harness.run();
        assert!(harness.query_by_label("以下を専用の設定にする").is_some());
        harness.get_by_label("外側").click();
        harness.run();
        assert!(
            harness.query_by_label("以下を専用の設定にする").is_none(),
            "outside click left the sticky menu open"
        );
    }
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

fn omitted_entries_chip_label(counts: crate::app::OmittedFolderEntryCounts) -> Option<String> {
    let primary = counts.primary_count();
    (primary > 0).then(|| format!("非表示 {primary} 件"))
}

fn omitted_entries_breakdown_label(counts: crate::app::OmittedFolderEntryCounts) -> String {
    let mut label = format!(
        "同名など {} / 隠し項目 {} / 対象外 {}",
        counts.same_name, counts.hidden, counts.unsupported
    );
    if counts.system > 0 {
        // システムファイルは主数字に入れないので、内訳でも別立てにして誤解を防ぐ。
        label.push_str(&format!(" / システム {}", counts.system));
    }
    label
}

/// 走査時に確定した内訳だけを描画する。ここからファイルシステムを再走査しない。
/// 戻り値は「同名ファイル設定を開く」が押されたかどうか。
fn draw_omitted_entries_chip(
    ui: &mut egui::Ui,
    counts: crate::app::OmittedFolderEntryCounts,
) -> bool {
    let Some(label) = omitted_entries_chip_label(counts) else {
        return false;
    };
    let mut open_settings = false;
    let button_label = egui::RichText::new(label).small().strong();
    let (response, _) = egui::containers::menu::MenuButton::new(button_label).ui(ui, |ui| {
        ui.set_min_width(300.0);
        ui.label(omitted_entries_breakdown_label(counts));
        ui.label(
            egui::RichText::new(
                "「システム」は Thumbs.db など自動で作られるファイルです。件数には含めません。",
            )
            .small()
            .weak(),
        );
        ui.separator();
        if ui.link("同名ファイル設定を開く").clicked() {
            open_settings = true;
            ui.close();
        }
    });
    let response = response.hover_tip("一覧に表示していない項目の内訳を表示");
    suppress_menu_button_wheel_passthrough(ui.ctx(), &response);
    open_settings
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

fn native_grid_drag_start_allowed(
    items_are_drive_list: bool,
    native_drag_just_finished: bool,
    touch_derived_pointer_activity: bool,
) -> bool {
    !items_are_drive_list && !native_drag_just_finished && !touch_derived_pointer_activity
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DetailsColumn {
    Preview,
    Name,
    Rating,
    Tags,
    Kind,
    PageCount,
    Place,
    Size,
    Modified,
    Created,
    State,
    ImageDimensions,
    VideoDuration,
    VideoDimensions,
    VideoCodec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DetailsColumnSet {
    Details,
    SharedBar,
    DedicatedBar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DetailsColumnMenuOrigin {
    DetailsListHeader,
    ThumbnailSelectionBar,
    DetailsSelectionBar,
}

#[derive(Clone, Debug, PartialEq)]
struct DetailsBestFitJobKey {
    items_generation: u64,
    order_revision: u64,
    content_revision: u64,
    total_rows: usize,
    column: DetailsColumn,
    column_set: DetailsColumnSet,
    view_kind: DetailsBestFitViewKind,
    book_sort_locked: bool,
    header_title: String,
    button_font: egui::FontId,
    body_font: egui::FontId,
    pixels_per_point_bits: u32,
    ui_font: crate::settings::UiFontSettings,
}

impl DetailsBestFitJobKey {
    fn has_restartable_content_update(&self, other: &Self) -> bool {
        if self.content_revision == other.content_revision
            && self.header_title == other.header_title
        {
            return false;
        }
        let mut started = self.clone();
        let mut current = other.clone();
        started.content_revision = 0;
        current.content_revision = 0;
        // Lazy columns append/remove `...` as the same worker delivers cell values. Treat that
        // header transition as part of the restartable content update instead of losing the
        // user's best-fit request at worker completion.
        started.header_title.clear();
        current.header_title.clear();
        started == current
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DetailsBestFitViewKind {
    Normal,
    Bookmark,
    ReadingHistory,
}

#[derive(Clone, Debug)]
struct DetailsBestFitJob {
    key: DetailsBestFitJobKey,
    next_row: usize,
    widest: f32,
    started: std::time::Instant,
    measured_rows: usize,
    batches: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DetailsBestFitFrameBudget {
    frame_nr: u64,
    claimed: bool,
}

impl DetailsBestFitFrameBudget {
    fn claim(&mut self, frame_nr: u64) -> bool {
        if self.frame_nr != frame_nr {
            self.frame_nr = frame_nr;
            self.claimed = false;
        }
        if self.claimed {
            return false;
        }
        self.claimed = true;
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DetailsBestFitBatch {
    Stale,
    Complete,
    Measure {
        range: std::ops::Range<usize>,
        completes_job: bool,
    },
}

impl DetailsBestFitJob {
    fn next_batch(&mut self, current_key: &DetailsBestFitJobKey) -> DetailsBestFitBatch {
        if self.key != *current_key {
            return DetailsBestFitBatch::Stale;
        }
        if self.next_row >= self.key.total_rows {
            return DetailsBestFitBatch::Complete;
        }

        let start = self.next_row;
        let end = start
            .saturating_add(DETAILS_BEST_FIT_ROWS_PER_FRAME)
            .min(self.key.total_rows);
        self.next_row = end;
        self.batches += 1;
        DetailsBestFitBatch::Measure {
            range: start..end,
            completes_job: end == self.key.total_rows,
        }
    }
}

impl DetailsColumnSet {
    fn includes(self, column: DetailsColumn) -> bool {
        self == Self::Details || column != DetailsColumn::Preview
    }

    fn column_order<'a>(self, settings: &'a crate::settings::Settings) -> &'a [DetailsColumnId] {
        match self {
            Self::Details | Self::SharedBar => &settings.details_column_order,
            Self::DedicatedBar => &settings.details_selection_bar_column_order,
        }
    }

    fn column_order_mut<'a>(
        self,
        settings: &'a mut crate::settings::Settings,
    ) -> &'a mut Vec<DetailsColumnId> {
        match self {
            Self::Details | Self::SharedBar => &mut settings.details_column_order,
            Self::DedicatedBar => &mut settings.details_selection_bar_column_order,
        }
    }

    fn column_widths<'a>(
        self,
        settings: &'a crate::settings::Settings,
    ) -> &'a [DetailsColumnWidth] {
        match self {
            Self::Details | Self::SharedBar => &settings.details_column_widths,
            Self::DedicatedBar => &settings.details_selection_bar_column_widths,
        }
    }

    fn name_width_auto(self, settings: &crate::settings::Settings) -> bool {
        match self {
            Self::Details | Self::SharedBar => settings.details_name_width_auto,
            Self::DedicatedBar => settings.details_selection_bar_name_width_auto,
        }
    }

    fn name_width(self, settings: &crate::settings::Settings) -> f32 {
        match self {
            Self::Details | Self::SharedBar => settings.details_name_width,
            Self::DedicatedBar => settings.details_selection_bar_name_width,
        }
    }
}

impl DetailsColumn {
    fn all() -> &'static [Self] {
        &[
            Self::Preview,
            Self::Name,
            Self::Rating,
            Self::Tags,
            Self::Kind,
            Self::PageCount,
            Self::Place,
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
            Self::PageCount => "ページ数",
            Self::Place => "場所",
            Self::Size => "サイズ",
            Self::Modified => "更新日時",
            Self::Created => "作成日時",
            Self::State => "状態",
            Self::ImageDimensions => "解像度",
            Self::VideoDuration => "長さ",
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
            Self::PageCount => DetailsColumnId::PageCount,
            Self::Place => DetailsColumnId::Place,
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
            DetailsColumnId::PageCount => Self::PageCount,
            DetailsColumnId::Place => Self::Place,
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
            Self::PageCount => Some(DetailsSortKey::PageCount),
            Self::Place => Some(DetailsSortKey::Place),
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
                | Self::PageCount
                | Self::ImageDimensions
                | Self::VideoDuration
                | Self::VideoDimensions
                | Self::VideoCodec
        )
    }

    fn visible(self, settings: &crate::settings::Settings, column_set: DetailsColumnSet) -> bool {
        let dedicated = column_set == DetailsColumnSet::DedicatedBar;
        match self {
            Self::Preview => {
                if dedicated {
                    settings.details_selection_bar_show_preview
                } else {
                    settings.details_show_preview
                }
            }
            Self::Name => true,
            Self::Rating => dedicated
                .then_some(settings.details_selection_bar_show_rating)
                .unwrap_or(settings.details_show_rating),
            Self::Tags => dedicated
                .then_some(settings.details_selection_bar_show_tags)
                .unwrap_or(settings.details_show_tags),
            Self::Kind => dedicated
                .then_some(settings.details_selection_bar_show_kind)
                .unwrap_or(settings.details_show_kind),
            Self::PageCount => dedicated
                .then_some(settings.details_selection_bar_show_page_count)
                .unwrap_or(settings.details_show_page_count),
            Self::Place => dedicated
                .then_some(settings.details_selection_bar_show_place)
                .unwrap_or(settings.details_show_place),
            Self::Size => dedicated
                .then_some(settings.details_selection_bar_show_size)
                .unwrap_or(settings.details_show_size),
            Self::Modified => dedicated
                .then_some(settings.details_selection_bar_show_modified)
                .unwrap_or(settings.details_show_modified),
            Self::Created => dedicated
                .then_some(settings.details_selection_bar_show_created)
                .unwrap_or(settings.details_show_created),
            Self::State => dedicated
                .then_some(settings.details_selection_bar_show_state)
                .unwrap_or(settings.details_show_state),
            Self::ImageDimensions => dedicated
                .then_some(settings.details_selection_bar_show_image_dimensions)
                .unwrap_or(settings.details_show_image_dimensions),
            Self::VideoDuration => dedicated
                .then_some(settings.details_selection_bar_show_video_duration)
                .unwrap_or(settings.details_show_video_duration),
            Self::VideoDimensions => dedicated
                .then_some(settings.details_selection_bar_show_video_dimensions)
                .unwrap_or(settings.details_show_video_dimensions),
            Self::VideoCodec => dedicated
                .then_some(settings.details_selection_bar_show_video_codec)
                .unwrap_or(settings.details_show_video_codec),
        }
    }

    fn default_width(self) -> f32 {
        // `egui_kittest` で本体の既定フォント (Yu Gothic Medium) を入れ、100% scale の
        // Body text を実測した固定シード幅 + DETAILS_BEST_FIT_HORIZONTAL_PADDING の ceil。
        // Rating: 65 + 14 = 79、State: 113 + 14 = 127。バッジ種別を増やした場合は
        // DETAILS_STATE_BEST_FIT_SEED とこの既定幅を必ず一緒に見直す。
        match self {
            Self::Preview => 34.0,
            Self::Name => 140.0,
            Self::Rating => 79.0,
            Self::Tags => 160.0,
            Self::Kind => 96.0,
            Self::PageCount => 80.0,
            Self::Place => 180.0,
            Self::Size => 92.0,
            Self::Modified | Self::Created => 138.0,
            Self::State => 127.0,
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
    column_set: DetailsColumnSet,
    include_hidden: bool,
) -> Vec<DetailsColumn> {
    details_storage_ordered_columns(settings, column_set)
        .into_iter()
        .filter(|col| {
            column_set.includes(*col) && (include_hidden || col.visible(settings, column_set))
        })
        .collect()
}

fn details_storage_ordered_columns(
    settings: &crate::settings::Settings,
    column_set: DetailsColumnSet,
) -> Vec<DetailsColumn> {
    let mut ordered = Vec::with_capacity(DetailsColumn::all().len());
    let source: Vec<DetailsColumnId> = if column_set.column_order(settings).is_empty() {
        DetailsColumnId::default_order().to_vec()
    } else {
        column_set.column_order(settings).to_vec()
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
}

fn details_column_is_visible(
    settings: &crate::settings::Settings,
    column_set: DetailsColumnSet,
    column: DetailsColumn,
) -> bool {
    column_set.includes(column) && column.visible(settings, column_set)
}

fn details_visible_columns(
    settings: &crate::settings::Settings,
    column_set: DetailsColumnSet,
) -> Vec<DetailsColumn> {
    details_ordered_columns(settings, column_set, false)
}

fn selection_info_bottom_bar_is_hidden(settings: &crate::settings::Settings) -> bool {
    settings.grid_view_mode == GridViewMode::Details
        && settings.details_selection_bar_mode.normalized() == DetailsSelectionBarMode::Hidden
}

fn grid_row_text_tooltips_enabled(grid_view_mode: GridViewMode) -> bool {
    grid_view_mode != GridViewMode::Details
}

fn selection_info_bottom_bar_column_set(settings: &crate::settings::Settings) -> DetailsColumnSet {
    if settings.grid_view_mode == GridViewMode::Details
        && settings.details_selection_bar_mode.normalized() == DetailsSelectionBarMode::Dedicated
    {
        DetailsColumnSet::DedicatedBar
    } else {
        DetailsColumnSet::SharedBar
    }
}

fn details_column_menu_heading(
    mode: DetailsSelectionBarMode,
    origin: DetailsColumnMenuOrigin,
) -> &'static str {
    if mode.normalized() == DetailsSelectionBarMode::SameAsDetails {
        "一覧と下部情報バー共通"
    } else if origin == DetailsColumnMenuOrigin::DetailsSelectionBar {
        "詳細表示の下部情報バー専用"
    } else {
        "一覧・サムネイル表示の下部情報バー"
    }
}

fn details_shared_preview_hover_text(
    column_set: DetailsColumnSet,
    origin: DetailsColumnMenuOrigin,
) -> Option<&'static str> {
    // 下部バーは詳細一覧・サムネイル一覧のどちらでもプレビューを描かない
    // (`DetailsColumnSet::includes`)。バーから開いたメニューでは、チェックの行き先が
    // 一覧側であることを伝える。`DetailsSelectionBar` だけを見ていると
    // サムネイル一覧の `ThumbnailSelectionBar` で説明が出ない (実害あり)。
    let from_bottom_bar = matches!(
        origin,
        DetailsColumnMenuOrigin::DetailsSelectionBar
            | DetailsColumnMenuOrigin::ThumbnailSelectionBar
    );
    (column_set == DetailsColumnSet::SharedBar && from_bottom_bar)
        .then_some("この設定は一覧側に反映されます。下部バーはプレビューを表示しません")
}

fn toggle_details_selection_bar_mode_from_menu(settings: &mut crate::settings::Settings) -> bool {
    let old_mode = settings.details_selection_bar_mode.normalized();
    let new_mode = if old_mode == DetailsSelectionBarMode::Dedicated {
        DetailsSelectionBarMode::SameAsDetails
    } else {
        DetailsSelectionBarMode::Dedicated
    };
    if old_mode == new_mode {
        return false;
    }
    if new_mode == DetailsSelectionBarMode::Dedicated {
        settings.copy_details_columns_to_selection_bar();
    }
    settings.details_selection_bar_mode = new_mode;
    true
}

pub(crate) fn selection_info_bottom_bar_shows_column(
    settings: &crate::settings::Settings,
    column: DetailsColumn,
) -> bool {
    settings.selection_info_display_mode.shows_bottom_bar()
        && !selection_info_bottom_bar_is_hidden(settings)
        && details_column_is_visible(
            settings,
            selection_info_bottom_bar_column_set(settings),
            column,
        )
}

fn selection_info_bar_contains_pos(
    bar_rect: Option<egui::Rect>,
    pointer_pos: Option<egui::Pos2>,
) -> bool {
    bar_rect
        .zip(pointer_pos)
        .is_some_and(|(rect, pos)| rect.contains(pos))
}

fn details_column_width(
    settings: &crate::settings::Settings,
    column_set: DetailsColumnSet,
    col: DetailsColumn,
) -> f32 {
    if col == DetailsColumn::Name {
        return col.default_width();
    }
    column_set
        .column_widths(settings)
        .iter()
        .find(|entry| entry.column == col.id())
        .map(|entry| entry.width)
        .unwrap_or_else(|| col.default_width())
        .clamp(col.min_width(), 800.0)
}

fn set_details_column_width(
    settings: &mut crate::settings::Settings,
    column_set: DetailsColumnSet,
    col: DetailsColumn,
    width: f32,
) -> bool {
    if col == DetailsColumn::Name {
        return false;
    }
    let width = width.clamp(col.min_width(), 800.0);
    let widths = match column_set {
        DetailsColumnSet::Details | DetailsColumnSet::SharedBar => {
            &mut settings.details_column_widths
        }
        DetailsColumnSet::DedicatedBar => &mut settings.details_selection_bar_column_widths,
    };
    if let Some(entry) = widths.iter_mut().find(|entry| entry.column == col.id()) {
        if (entry.width - width).abs() <= 0.1 {
            return false;
        }
        entry.width = width;
    } else {
        widths.push(DetailsColumnWidth {
            column: col.id(),
            width,
        });
    }
    true
}

/// 固定幅モード時の名前列幅 (clamp 済み)。
fn details_name_fixed_width(
    settings: &crate::settings::Settings,
    column_set: DetailsColumnSet,
) -> f32 {
    column_set
        .name_width(settings)
        .clamp(DetailsColumn::Name.min_width(), 800.0)
}

/// 名前列を固定幅へ切り替えて幅を保存する。値が変わったら true。
fn set_details_name_width(
    settings: &mut crate::settings::Settings,
    column_set: DetailsColumnSet,
    width: f32,
) -> bool {
    let width = width.clamp(DetailsColumn::Name.min_width(), 800.0);
    match column_set {
        DetailsColumnSet::Details | DetailsColumnSet::SharedBar => {
            if !settings.details_name_width_auto
                && (settings.details_name_width - width).abs() <= 0.1
            {
                return false;
            }
            settings.details_name_width_auto = false;
            settings.details_name_width = width;
        }
        DetailsColumnSet::DedicatedBar => {
            if !settings.details_selection_bar_name_width_auto
                && (settings.details_selection_bar_name_width - width).abs() <= 0.1
            {
                return false;
            }
            settings.details_selection_bar_name_width_auto = false;
            settings.details_selection_bar_name_width = width;
        }
    }
    true
}

fn details_column_visibility_slot(
    settings: &mut crate::settings::Settings,
    column_set: DetailsColumnSet,
    column: DetailsColumn,
) -> Option<&mut bool> {
    let dedicated = column_set == DetailsColumnSet::DedicatedBar;
    Some(match (dedicated, column) {
        (_, DetailsColumn::Name) => return None,
        (false, DetailsColumn::Preview) => &mut settings.details_show_preview,
        (false, DetailsColumn::Rating) => &mut settings.details_show_rating,
        (false, DetailsColumn::Tags) => &mut settings.details_show_tags,
        (false, DetailsColumn::Kind) => &mut settings.details_show_kind,
        (false, DetailsColumn::PageCount) => &mut settings.details_show_page_count,
        (false, DetailsColumn::Place) => &mut settings.details_show_place,
        (false, DetailsColumn::Size) => &mut settings.details_show_size,
        (false, DetailsColumn::Modified) => &mut settings.details_show_modified,
        (false, DetailsColumn::Created) => &mut settings.details_show_created,
        (false, DetailsColumn::State) => &mut settings.details_show_state,
        (false, DetailsColumn::ImageDimensions) => &mut settings.details_show_image_dimensions,
        (false, DetailsColumn::VideoDuration) => &mut settings.details_show_video_duration,
        (false, DetailsColumn::VideoDimensions) => &mut settings.details_show_video_dimensions,
        (false, DetailsColumn::VideoCodec) => &mut settings.details_show_video_codec,
        (true, DetailsColumn::Preview) => &mut settings.details_selection_bar_show_preview,
        (true, DetailsColumn::Rating) => &mut settings.details_selection_bar_show_rating,
        (true, DetailsColumn::Tags) => &mut settings.details_selection_bar_show_tags,
        (true, DetailsColumn::Kind) => &mut settings.details_selection_bar_show_kind,
        (true, DetailsColumn::PageCount) => &mut settings.details_selection_bar_show_page_count,
        (true, DetailsColumn::Place) => &mut settings.details_selection_bar_show_place,
        (true, DetailsColumn::Size) => &mut settings.details_selection_bar_show_size,
        (true, DetailsColumn::Modified) => &mut settings.details_selection_bar_show_modified,
        (true, DetailsColumn::Created) => &mut settings.details_selection_bar_show_created,
        (true, DetailsColumn::State) => &mut settings.details_selection_bar_show_state,
        (true, DetailsColumn::ImageDimensions) => {
            &mut settings.details_selection_bar_show_image_dimensions
        }
        (true, DetailsColumn::VideoDuration) => {
            &mut settings.details_selection_bar_show_video_duration
        }
        (true, DetailsColumn::VideoDimensions) => {
            &mut settings.details_selection_bar_show_video_dimensions
        }
        (true, DetailsColumn::VideoCodec) => &mut settings.details_selection_bar_show_video_codec,
    })
}

fn set_details_name_width_auto(
    settings: &mut crate::settings::Settings,
    column_set: DetailsColumnSet,
    enabled: bool,
    current_name_width: f32,
) -> bool {
    if column_set.name_width_auto(settings) == enabled {
        return false;
    }
    if enabled {
        match column_set {
            DetailsColumnSet::Details | DetailsColumnSet::SharedBar => {
                settings.details_name_width_auto = true;
                settings.details_name_width = DetailsColumn::Name.default_width();
            }
            DetailsColumnSet::DedicatedBar => {
                settings.details_selection_bar_name_width_auto = true;
                settings.details_selection_bar_name_width = DetailsColumn::Name.default_width();
            }
        }
        true
    } else {
        let stored_width =
            details_stored_name_width_from_effective(settings, column_set, current_name_width);
        set_details_name_width(settings, column_set, stored_width)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct DetailsColumnMenuState {
    name_width_auto: bool,
    show_preview: bool,
    show_rating: bool,
    show_tags: bool,
    show_kind: bool,
    show_page_count: bool,
    show_place: bool,
    show_size: bool,
    show_modified: bool,
    show_created: bool,
    show_state: bool,
    show_image_dimensions: bool,
    show_video_duration: bool,
    show_video_dimensions: bool,
    show_video_codec: bool,
    size_display_mode: DetailsSizeDisplayMode,
    timestamp_show_seconds: bool,
    row_style: DetailsRowStyle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DetailsColumnMenuChanges {
    columns: bool,
    lazy_columns: bool,
    format: bool,
}

impl DetailsColumnMenuChanges {
    fn any(self) -> bool {
        self.columns || self.format
    }
}

impl DetailsColumnMenuState {
    fn from_settings(settings: &crate::settings::Settings, column_set: DetailsColumnSet) -> Self {
        Self {
            name_width_auto: column_set.name_width_auto(settings),
            show_preview: DetailsColumn::Preview.visible(settings, column_set),
            show_rating: DetailsColumn::Rating.visible(settings, column_set),
            show_tags: DetailsColumn::Tags.visible(settings, column_set),
            show_kind: DetailsColumn::Kind.visible(settings, column_set),
            show_page_count: DetailsColumn::PageCount.visible(settings, column_set),
            show_place: DetailsColumn::Place.visible(settings, column_set),
            show_size: DetailsColumn::Size.visible(settings, column_set),
            show_modified: DetailsColumn::Modified.visible(settings, column_set),
            show_created: DetailsColumn::Created.visible(settings, column_set),
            show_state: DetailsColumn::State.visible(settings, column_set),
            show_image_dimensions: DetailsColumn::ImageDimensions.visible(settings, column_set),
            show_video_duration: DetailsColumn::VideoDuration.visible(settings, column_set),
            show_video_dimensions: DetailsColumn::VideoDimensions.visible(settings, column_set),
            show_video_codec: DetailsColumn::VideoCodec.visible(settings, column_set),
            // 書式はセット C を編集するメニューでもセット A と共有する。
            size_display_mode: settings.details_size_display_mode,
            timestamp_show_seconds: settings.details_timestamp_show_seconds,
            row_style: settings.details_row_style,
        }
    }
}

impl DetailsColumnMenuState {
    fn apply(
        &self,
        settings: &mut crate::settings::Settings,
        column_set: DetailsColumnSet,
        current_name_width: f32,
    ) -> DetailsColumnMenuChanges {
        let mut changes = DetailsColumnMenuChanges::default();
        changes.columns |= set_details_name_width_auto(
            settings,
            column_set,
            self.name_width_auto,
            current_name_width,
        );
        apply_details_column_menu_visibility(self, settings, column_set, &mut changes);
        if settings.details_size_display_mode != self.size_display_mode {
            settings.details_size_display_mode = self.size_display_mode;
            changes.format = true;
        }
        if settings.details_timestamp_show_seconds != self.timestamp_show_seconds {
            settings.details_timestamp_show_seconds = self.timestamp_show_seconds;
            changes.format = true;
        }
        if settings.details_row_style != self.row_style {
            settings.details_row_style = self.row_style;
            changes.format = true;
        }
        changes
    }
}

fn apply_details_column_menu_visibility(
    state: &DetailsColumnMenuState,
    settings: &mut crate::settings::Settings,
    column_set: DetailsColumnSet,
    changes: &mut DetailsColumnMenuChanges,
) {
    apply_details_column_menu_visibility_group(
        settings,
        column_set,
        changes,
        &[
            (DetailsColumn::Preview, state.show_preview),
            (DetailsColumn::Rating, state.show_rating),
            (DetailsColumn::Tags, state.show_tags),
            (DetailsColumn::Kind, state.show_kind),
            (DetailsColumn::PageCount, state.show_page_count),
            (DetailsColumn::Place, state.show_place),
            (DetailsColumn::Size, state.show_size),
        ],
    );
    apply_details_column_menu_visibility_group(
        settings,
        column_set,
        changes,
        &[
            (DetailsColumn::Modified, state.show_modified),
            (DetailsColumn::Created, state.show_created),
            (DetailsColumn::State, state.show_state),
            (DetailsColumn::ImageDimensions, state.show_image_dimensions),
            (DetailsColumn::VideoDuration, state.show_video_duration),
            (DetailsColumn::VideoDimensions, state.show_video_dimensions),
            (DetailsColumn::VideoCodec, state.show_video_codec),
        ],
    );
}

fn apply_details_column_menu_visibility_group(
    settings: &mut crate::settings::Settings,
    column_set: DetailsColumnSet,
    changes: &mut DetailsColumnMenuChanges,
    values: &[(DetailsColumn, bool)],
) {
    for &(column, visible) in values {
        let slot = details_column_visibility_slot(settings, column_set, column)
            .expect("name is the only immutable details column");
        if *slot != visible {
            *slot = visible;
            changes.columns = true;
            changes.lazy_columns |= column.is_lazy();
        }
    }
}

fn blend_details_row_color(
    base: egui::Color32,
    target: egui::Color32,
    amount: f32,
) -> egui::Color32 {
    let amount = amount.clamp(0.0, 1.0);
    let blend =
        |from: u8, to: u8| ((from as f32) + ((to as f32) - (from as f32)) * amount).round() as u8;
    egui::Color32::from_rgba_unmultiplied(
        blend(base.r(), target.r()),
        blend(base.g(), target.g()),
        blend(base.b(), target.b()),
        base.a(),
    )
}

fn details_alternating_row_fill(visuals: &egui::Visuals) -> egui::Color32 {
    if visuals.dark_mode {
        blend_details_row_color(visuals.panel_fill, egui::Color32::WHITE, 0.055)
    } else {
        blend_details_row_color(visuals.panel_fill, egui::Color32::BLACK, 0.035)
    }
}

fn details_separator_color(visuals: &egui::Visuals) -> egui::Color32 {
    if visuals.dark_mode {
        blend_details_row_color(visuals.panel_fill, egui::Color32::WHITE, 0.24)
    } else {
        blend_details_row_color(visuals.panel_fill, egui::Color32::BLACK, 0.18)
    }
}

fn details_row_background(
    visuals: &egui::Visuals,
    style: DetailsRowStyle,
    row: usize,
    selected: bool,
    checked: bool,
    hovered: bool,
) -> egui::Color32 {
    if selected {
        visuals.selection.bg_fill
    } else if checked {
        visuals.widgets.active.bg_fill
    } else if hovered {
        visuals.widgets.hovered.bg_fill
    } else if style.show_alternating_background() && row % 2 == 1 {
        details_alternating_row_fill(visuals)
    } else {
        visuals.panel_fill
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DetailsRowVisualState {
    selected: bool,
    checked: bool,
    hovered: bool,
}

fn details_row_visual_state(
    display_only: bool,
    selected: bool,
    checked: bool,
    hovered: bool,
) -> DetailsRowVisualState {
    if display_only {
        DetailsRowVisualState {
            selected: false,
            checked: false,
            hovered: false,
        }
    } else {
        DetailsRowVisualState {
            selected,
            checked,
            hovered,
        }
    }
}

fn normalized_pixels_per_point(pixels_per_point: f32) -> f32 {
    if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    }
}

fn details_separator_stroke_width(pixels_per_point: f32) -> f32 {
    1.0 / normalized_pixels_per_point(pixels_per_point)
}

fn details_separator_y(rect: egui::Rect, pixels_per_point: f32) -> f32 {
    let pixels_per_point = normalized_pixels_per_point(pixels_per_point);
    ((rect.bottom() * pixels_per_point).floor() - 0.5) / pixels_per_point
}

fn details_scroll_style() -> egui::style::ScrollStyle {
    // floating style は前景色の濃いハンドルを内容の上へ重ねる。詳細一覧の縦横バーと
    // 下部選択情報バーを同じ solid style に揃え、列テキストとは別領域に表示する。
    egui::style::ScrollStyle::solid()
}

fn details_layout_right_guard(pixels_per_point: f32) -> f32 {
    // 名前の自動幅では最低 1 physical px を空ける。高 DPI ではヘッダ右端の
    // 1pt separator の半幅の方が大きくなるため、そちらも収まる余白にする。
    (1.0 / normalized_pixels_per_point(pixels_per_point)).max(0.5)
}

fn selection_info_bar_height() -> f32 {
    SELECTION_INFO_BAR_CONTENT_HEIGHT + details_scroll_style().allocated_width()
}

fn details_grid_available_height_after_selection_bar(
    available_before: egui::Rect,
    style: &egui::Style,
) -> f32 {
    let remaining_outer_height = (available_before.height() - selection_info_bar_height()).max(0.0);
    let central_margin_y = egui::Frame::central_panel(style).inner_margin.sum().y;
    (remaining_outer_height - central_margin_y).max(0.0)
}

/// 詳細表示の水平レイアウト。縦スクロールバーの gutter を考慮して、ヘッダと行の列が
/// ぴったり揃い、縦バー出現時に右端列が欠けたり不要な横スクロールバーが出たりしないようにする。
#[derive(Clone, Copy)]
struct DetailsLayout {
    /// 名前列の実効幅。
    name_w: f32,
    /// 表示列の実幅合計。pane の空き背景は含めない。
    columns_w: f32,
    /// 行 / ヘッダ背景の幅 (>= 全列合計。pane を埋める)。
    pane_w: f32,
    /// 外側 (水平) スクロールが扱う総コンテンツ幅 (= pane_w + gutter)。
    extent: f32,
}

#[derive(Clone, Copy, Debug)]
struct DetailsHorizontalScrollPolicy {
    /// アプリが把握する描画範囲が viewport の物理ピクセル右端を越えるか。
    overflow: bool,
    /// offset=0 の ScrollArea が子 Ui に与える丸め後の開始座標。
    content_origin_x: f32,
    /// 背景・列・右端 separator をすべて含む必要幅。
    required_extent: f32,
    /// viewport の内向き物理 px 右端まで、描画開始位置から安全に使える幅。
    viewport_capacity: f32,
    /// overflow 時に右端まで確実に到達できるよう外向きに確保した幅。
    scroll_extent: f32,
    viewport_right_px: i64,
    required_right_px: i64,
}

fn physical_pixel_floor(value: f32) -> i64 {
    let value = value as f64;
    let nearest = value.round();
    // f32 の座標×pppで生じる数 ULP だけを整数へ吸着する。固定の大きい epsilon を
    // 使うと実在する微小 overflow を隠すため、座標値に応じた上限 1/100 px の範囲にする。
    let integer_epsilon = (f32::EPSILON as f64 * value.abs() * 4.0).clamp(1.0e-5, 1.0e-2);
    if (value - nearest).abs() <= integer_epsilon {
        nearest as i64
    } else {
        value.floor() as i64
    }
}

fn physical_pixel_ceil(value: f32) -> i64 {
    let value = value as f64;
    let nearest = value.round();
    let integer_epsilon = (f32::EPSILON as f64 * value.abs() * 4.0).clamp(1.0e-5, 1.0e-2);
    if (value - nearest).abs() <= integer_epsilon {
        nearest as i64
    } else {
        value.ceil() as i64
    }
}

fn details_scroll_content_origin_x(viewport_left: f32, pixels_per_point: f32) -> f32 {
    use egui::emath::GuiRounding as _;

    viewport_left
        .round_to_pixels(normalized_pixels_per_point(pixels_per_point))
        .round_ui()
}

fn details_horizontal_viewport_capacity(viewport: egui::Rect, pixels_per_point: f32) -> f32 {
    let pixels_per_point = normalized_pixels_per_point(pixels_per_point);
    let content_origin_x =
        details_scroll_content_origin_x(viewport.left(), pixels_per_point).max(viewport.left());
    let viewport_right_px = physical_pixel_floor(viewport.right() * pixels_per_point);
    (viewport_right_px as f32 / pixels_per_point - content_origin_x).max(0.0)
}

fn details_horizontal_scroll_policy(
    viewport: egui::Rect,
    layout_extent: f32,
    columns_w: f32,
    pixels_per_point: f32,
) -> DetailsHorizontalScrollPolicy {
    let pixels_per_point = normalized_pixels_per_point(pixels_per_point);
    let content_origin_x = details_scroll_content_origin_x(viewport.left(), pixels_per_point);
    // 背景 (layout_extent) と、最終列の中央に描く 1pt separator の右半分を両方含める。
    let required_extent = layout_extent.max(columns_w + 0.5);
    // 子 Ui の丸めが左へ寄った場合に得られる偶然の余白には依存しない。右へ寄る場合だけ
    // 実際の描画開始位置として反映し、右端切れ判定を安全側へ倒す。
    let required_origin_x = content_origin_x.max(viewport.left());
    let viewport_right_px = physical_pixel_floor(viewport.right() * pixels_per_point);
    let viewport_capacity =
        (viewport_right_px as f32 / pixels_per_point - required_origin_x).max(0.0);
    let required_right_px =
        physical_pixel_ceil((required_origin_x + required_extent) * pixels_per_point);
    let overflow = required_right_px > viewport_right_px;
    let scroll_extent = if overflow {
        // min_width 自体も egui で再丸めされる。必要右端のさらに 1 physical px 外まで
        // 確保し、手動幅が境界をわずかに越えた場合も最終列まで確実にスクロールできるようにする。
        (((required_right_px.saturating_add(1)) as f32 / pixels_per_point) - content_origin_x)
            .max(layout_extent)
            .max(required_extent)
    } else {
        layout_extent
    };

    DetailsHorizontalScrollPolicy {
        overflow,
        content_origin_x,
        required_extent,
        viewport_capacity,
        scroll_extent,
        viewport_right_px,
        required_right_px,
    }
}

fn configured_details_horizontal_scroll_area(
    policy: DetailsHorizontalScrollPolicy,
) -> egui::ScrollArea {
    use egui::containers::scroll_area::{ScrollBarVisibility, ScrollSource};

    let area = egui::ScrollArea::horizontal()
        .auto_shrink([false, false])
        .scroll_bar_visibility(if policy.overflow {
            ScrollBarVisibility::AlwaysVisible
        } else {
            ScrollBarVisibility::AlwaysHidden
        })
        .scroll_source(if policy.overflow {
            ScrollSource::ALL
        } else {
            ScrollSource::NONE
        });
    if policy.overflow {
        area
    } else {
        // 以前はみ出していた列をスクロールした後に自動幅へ戻しても、保持中 offset や
        // その丸め位相が fit 判定へ混ざらないよう、非 overflow 状態では原点を所有する。
        area.horizontal_scroll_offset(0.0)
    }
}

#[derive(Clone, Copy)]
struct DetailsVerticalScrollDebug {
    inner_rect: egui::Rect,
    content_size: egui::Vec2,
    state: egui::scroll_area::State,
}

struct DetailsLayoutDebugSample<'a> {
    surface: &'static str,
    source_rect: egui::Rect,
    avail_w: f32,
    avail_h: f32,
    row_count: usize,
    natural_h: f32,
    viewport_h_est: f32,
    horizontal_policy: DetailsHorizontalScrollPolicy,
    predicted_hbar: f32,
    predicted_vscroll: bool,
    gutter: f32,
    fixed_columns_w: f32,
    name_w: f32,
    pane_w: f32,
    layout_extent: f32,
    requested_extent: f32,
    column_set: DetailsColumnSet,
    outer_inner_rect: egui::Rect,
    outer_content_size: egui::Vec2,
    outer_state: egui::scroll_area::State,
    vertical: Option<DetailsVerticalScrollDebug>,
    settings: &'a crate::settings::Settings,
}

fn details_layout_debug_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var(DETAILS_LAYOUT_DEBUG_ENV).ok().as_deref() == Some("1"))
}

fn format_details_layout_debug_rect(rect: Option<egui::Rect>) -> String {
    rect.map_or_else(
        || "none".to_string(),
        |rect| {
            format!(
                "({:.6},{:.6}) {:.6}x{:.6}",
                rect.min.x,
                rect.min.y,
                rect.width(),
                rect.height()
            )
        },
    )
}

fn log_details_layout_debug(ctx: &egui::Context, sample: DetailsLayoutDebugSample<'_>) {
    if !details_layout_debug_enabled() {
        return;
    }

    let effective_ppp = ctx.pixels_per_point();
    let native_ppp = ctx.native_pixels_per_point();
    let zoom_factor = ctx.zoom_factor();
    let (viewport_outer, viewport_inner, viewport_focused, viewport_maximized) =
        ctx.input(|input| {
            let viewport = input.viewport();
            (
                viewport.outer_rect,
                viewport.inner_rect,
                viewport.focused,
                viewport.maximized,
            )
        });
    let outer_delta_x = sample.outer_content_size.x - sample.outer_inner_rect.width();
    let columns = details_visible_columns(sample.settings, sample.column_set);
    let viewport_id = ctx.viewport_id();
    let rounding_slack = details_layout_right_guard(effective_ppp);
    let columns_avail = (sample.horizontal_policy.viewport_capacity - sample.gutter).max(0.0);
    let columns_w = sample.name_w + sample.fixed_columns_w;
    let columns_debug = columns
        .iter()
        .map(|&column| {
            let width = if column == DetailsColumn::Name {
                sample.name_w
            } else {
                details_column_width(sample.settings, sample.column_set, column)
            };
            format!("{column:?}:{width:.6}")
        })
        .collect::<Vec<_>>()
        .join(",");
    let vertical_debug = sample.vertical.map_or_else(
        || "none".to_string(),
        |vertical| {
            let delta_x = vertical.content_size.x - vertical.inner_rect.width();
            format!(
                "inner=({:.6},{:.6}) {:.6}x{:.6} content={:.6}x{:.6} delta_x={:.6}pt/{:.6}px state={:?}",
                vertical.inner_rect.min.x,
                vertical.inner_rect.min.y,
                vertical.inner_rect.width(),
                vertical.inner_rect.height(),
                vertical.content_size.x,
                vertical.content_size.y,
                delta_x,
                delta_x * effective_ppp,
                vertical.state
            )
        },
    );
    crate::logger::log(format!(
        "[DETAILS_LAYOUT] frame={} viewport={viewport_id:?} surface={} native_ppp={:.6?} zoom={zoom_factor:.6} effective_ppp={effective_ppp:.6} focused={viewport_focused:?} maximized={viewport_maximized:?} viewport_outer={} viewport_inner={} source=({:.6},{:.6}) {:.6}x{:.6} avail={:.6}x{:.6} rows={} natural_h={:.6} predicted={{h_overflow:{} hbar:{:.6} viewport_h:{:.6} vscroll:{} gutter:{:.6}}} layout={{name_auto:{} fixed:{:.6} columns_avail:{:.6} slack:{rounding_slack:.6} name:{:.6} columns:{columns_w:.6} pane:{:.6} extent:{:.6} requested:{:.6}}} physical={{origin:{:.6} capacity:{:.6} required_extent:{:.6} viewport_right_px:{} required_right_px:{} overflow:{} scroll_extent:{:.6}}} outer={{inner:({:.6},{:.6}) {:.6}x{:.6} content:{:.6}x{:.6} delta_x:{outer_delta_x:.6}pt/{:.6}px state:{:?}}} vertical={{{vertical_debug}}} columns=[{columns_debug}]",
        ctx.cumulative_frame_nr(),
        sample.surface,
        native_ppp,
        format_details_layout_debug_rect(viewport_outer),
        format_details_layout_debug_rect(viewport_inner),
        sample.source_rect.min.x,
        sample.source_rect.min.y,
        sample.source_rect.width(),
        sample.source_rect.height(),
        sample.avail_w,
        sample.avail_h,
        sample.row_count,
        sample.natural_h,
        sample.horizontal_policy.overflow,
        sample.predicted_hbar,
        sample.viewport_h_est,
        sample.predicted_vscroll,
        sample.gutter,
        sample.column_set.name_width_auto(sample.settings),
        sample.fixed_columns_w,
        columns_avail,
        sample.name_w,
        sample.pane_w,
        sample.layout_extent,
        sample.requested_extent,
        sample.horizontal_policy.content_origin_x,
        sample.horizontal_policy.viewport_capacity,
        sample.horizontal_policy.required_extent,
        sample.horizontal_policy.viewport_right_px,
        sample.horizontal_policy.required_right_px,
        sample.horizontal_policy.overflow,
        sample.horizontal_policy.scroll_extent,
        sample.outer_inner_rect.min.x,
        sample.outer_inner_rect.min.y,
        sample.outer_inner_rect.width(),
        sample.outer_inner_rect.height(),
        sample.outer_content_size.x,
        sample.outer_content_size.y,
        outer_delta_x * effective_ppp,
        sample.outer_state,
    ));
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DetailsRowData {
    cells: Vec<(DetailsColumn, String)>,
}

impl DetailsRowData {
    fn text(&self, column: DetailsColumn) -> &str {
        self.cells
            .iter()
            .find_map(|(candidate, text)| (*candidate == column).then_some(text.as_str()))
            .unwrap_or_default()
    }
}

fn details_fixed_columns_width(
    settings: &crate::settings::Settings,
    column_set: DetailsColumnSet,
) -> f32 {
    details_ordered_columns(settings, column_set, false)
        .into_iter()
        .filter(|col| *col != DetailsColumn::Name)
        .map(|col| details_column_width(settings, column_set, col))
        .sum()
}

/// この面では描かないが、元の列セットで表示中の列幅。
///
/// 下部バーではプレビュー列を描かない。その幅を空白として予約せず名前列へ移すことで、
/// 名前より右の列を一覧と同じ x 座標に保つ。
fn details_omitted_columns_width(
    settings: &crate::settings::Settings,
    column_set: DetailsColumnSet,
) -> f32 {
    DetailsColumn::all()
        .iter()
        .copied()
        .filter(|column| !column_set.includes(*column) && column.visible(settings, column_set))
        .map(|column| details_column_width(settings, column_set, column))
        .sum()
}

fn details_effective_fixed_name_width(
    settings: &crate::settings::Settings,
    column_set: DetailsColumnSet,
) -> f32 {
    details_name_fixed_width(settings, column_set)
        + details_omitted_columns_width(settings, column_set)
}

fn details_stored_name_width_from_effective(
    settings: &crate::settings::Settings,
    column_set: DetailsColumnSet,
    width: f32,
) -> f32 {
    width - details_omitted_columns_width(settings, column_set)
}

fn details_layout(
    avail_w: f32,
    gutter: f32,
    pixels_per_point: f32,
    settings: &crate::settings::Settings,
    column_set: DetailsColumnSet,
) -> DetailsLayout {
    let fixed = details_fixed_columns_width(settings, column_set);
    let omitted = details_omitted_columns_width(settings, column_set);
    let columns_avail = (avail_w - gutter).max(0.0);
    let name_width_auto = column_set.name_width_auto(settings);
    let minimum_name_width = DetailsColumn::Name.default_width() + omitted;
    let auto_fits = name_width_auto && columns_avail >= fixed + minimum_name_width;
    let rounding_slack = details_layout_right_guard(pixels_per_point);
    let name_w = if auto_fits {
        (columns_avail - fixed - rounding_slack).max(DetailsColumn::Name.min_width())
    } else if name_width_auto {
        minimum_name_width
    } else {
        details_effective_fixed_name_width(settings, column_set)
    };
    let columns_w = name_w + fixed;
    // Leave one physical pixel of slack when auto-fit has enough room. At UI
    // scales below 100%, f32 add/subtract can otherwise round the content one
    // device pixel past the horizontal ScrollArea viewport and show a phantom bar.
    let pane_w = if auto_fits {
        columns_w
    } else if columns_w <= columns_avail - rounding_slack {
        // Fixed-width columns may also fit with spare room. Keep the list pane
        // almost full-width, but preserve the same device-pixel rounding guard.
        columns_avail - rounding_slack
    } else {
        columns_w.max(columns_avail)
    };
    let extent = pane_w + gutter;
    DetailsLayout {
        name_w,
        columns_w,
        pane_w,
        extent,
    }
}

#[derive(Clone, Copy)]
struct DetailsListLayoutResolution {
    layout: DetailsLayout,
    horizontal_policy: DetailsHorizontalScrollPolicy,
    hbar: f32,
    viewport_h_est: f32,
    needs_vscroll: bool,
    gutter: f32,
}

fn resolve_details_list_layout(
    source_rect: egui::Rect,
    avail_h: f32,
    item_spacing_y: f32,
    natural_h: f32,
    pixels_per_point: f32,
    settings: &crate::settings::Settings,
) -> DetailsListLayoutResolution {
    let scroll_style = details_scroll_style();
    let avail_w = source_rect.width().max(1.0);
    let layout_avail_w = details_horizontal_viewport_capacity(source_rect, pixels_per_point)
        .min(avail_w)
        .max(1.0);
    let mut h_overflow = false;
    let mut settled = None;
    for _ in 0..3 {
        let hbar = if h_overflow {
            scroll_style.allocated_width()
        } else {
            0.0
        };
        // 境界帯では gutter を多めに確保する方へ倒し、右端列の欠けを避ける。
        let viewport_h_est =
            (avail_h - App::DETAILS_HEADER_H - item_spacing_y - hbar - 2.0).max(0.0);
        let needs_vscroll = natural_h > viewport_h_est;
        let gutter = if needs_vscroll {
            scroll_style.allocated_width()
        } else {
            0.0
        };
        let layout = details_layout(
            layout_avail_w,
            gutter,
            pixels_per_point,
            settings,
            DetailsColumnSet::Details,
        );
        let horizontal_policy = details_horizontal_scroll_policy(
            source_rect,
            layout.extent,
            layout.columns_w,
            pixels_per_point,
        );
        let stable = horizontal_policy.overflow == h_overflow;
        h_overflow = horizontal_policy.overflow;
        settled = Some(DetailsListLayoutResolution {
            layout,
            horizontal_policy,
            hbar,
            viewport_h_est,
            needs_vscroll,
            gutter,
        });
        if stable {
            break;
        }
    }
    settled.expect("details horizontal policy loop always runs")
}

fn selection_info_bottom_bar_gutter(grid_view_mode: GridViewMode, details_list_gutter: f32) -> f32 {
    if grid_view_mode == GridViewMode::Details {
        details_list_gutter
    } else {
        0.0
    }
}

fn details_content_width_for_column_set(full_pane_width: f32) -> f32 {
    full_pane_width.max(DetailsColumn::Name.min_width())
}

fn details_column_rects_for_columns(
    rect: egui::Rect,
    settings: &crate::settings::Settings,
    column_set: DetailsColumnSet,
) -> Vec<(DetailsColumn, egui::Rect)> {
    let columns = details_visible_columns(settings, column_set);
    let fixed: f32 = columns
        .iter()
        .copied()
        .filter(|col| *col != DetailsColumn::Name)
        .map(|col| details_column_width(settings, column_set, col))
        .sum();
    let name_width = if column_set.name_width_auto(settings) {
        (rect.width() - fixed).max(
            DetailsColumn::Name.default_width()
                + details_omitted_columns_width(settings, column_set),
        )
    } else {
        details_effective_fixed_name_width(settings, column_set)
    };
    let specs = columns
        .into_iter()
        .map(|col| {
            let width = if col == DetailsColumn::Name {
                name_width
            } else {
                details_column_width(settings, column_set, col)
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

#[cfg(test)]
fn details_column_rects(
    rect: egui::Rect,
    settings: &crate::settings::Settings,
) -> Vec<(DetailsColumn, egui::Rect)> {
    details_column_rects_for_columns(rect, settings, DetailsColumnSet::Details)
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
    column_set: DetailsColumnSet,
    dragged: DetailsColumn,
    target: DetailsColumn,
    insert_after_target: bool,
) -> bool {
    if dragged == target {
        return false;
    }
    let mut columns = details_ordered_columns(settings, column_set, true);
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
    // Bottom bars omit Preview, but their persisted order may still contain it. Replace only
    // columns owned by this surface so a bar reorder cannot drop or relocate excluded columns.
    let mut reordered = columns.into_iter();
    let new_order = details_storage_ordered_columns(settings, column_set)
        .into_iter()
        .map(|column| {
            if column_set.includes(column) {
                reordered.next().unwrap_or(column)
            } else {
                column
            }
        })
        .map(DetailsColumn::id)
        .collect::<Vec<_>>();
    let stored_order = column_set.column_order_mut(settings);
    if *stored_order == new_order {
        return false;
    }
    *stored_order = new_order;
    true
}

fn finish_details_header_drag(
    settings: &mut crate::settings::Settings,
    column_set: DetailsColumnSet,
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
    reorder_details_column(settings, column_set, drag.column, target, insert_after)
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
        TS::SmartFolders => "スマートフォルダ",
        TS::Tags => "タグ",
        TS::Unknown => "",
    }
}

fn smart_folder_toolbar_visible(show_setting: bool, definition_count: usize) -> bool {
    show_setting && definition_count > 0
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
        TS::SmartFolders => settings.show_toolbar_smart_folders = visible,
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

fn details_text_is_right_aligned(align: egui::Align2) -> bool {
    matches!(
        align,
        egui::Align2::RIGHT_TOP | egui::Align2::RIGHT_CENTER | egui::Align2::RIGHT_BOTTOM
    )
}

fn details_text_clip_rect(
    clip: egui::Rect,
    align: egui::Align2,
    pixels_per_point: f32,
) -> Option<egui::Rect> {
    if clip.width() <= 1.0 {
        return None;
    }
    if details_text_is_right_aligned(align) {
        // The right-aligned anchor stays at the padded column edge, but text and scissor
        // rounding can otherwise clip its last glyph. Left-aligned text has no reported
        // issue, so only give the right clip edge one physical pixel of rounding room.
        Some(egui::Rect::from_min_max(
            clip.min,
            egui::pos2(
                clip.right() + 1.0 / normalized_pixels_per_point(pixels_per_point),
                clip.bottom(),
            ),
        ))
    } else {
        Some(clip)
    }
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
    let Some(text_clip) = details_text_clip_rect(clip, align, ui.ctx().pixels_per_point()) else {
        return;
    };
    let x = if details_text_is_right_aligned(align) {
        clip.right()
    } else {
        clip.left()
    };
    let font = if strong {
        egui::TextStyle::Button.resolve(ui.style())
    } else {
        egui::TextStyle::Body.resolve(ui.style())
    };
    ui.painter().with_clip_rect(text_clip).text(
        egui::pos2(x, clip.center().y),
        align,
        text,
        font,
        color,
    );
}

#[cfg(test)]
mod details_text_clip_tests {
    use super::*;

    #[test]
    fn right_aligned_clip_adds_one_physical_pixel() {
        let clip = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 40.0));
        for pixels_per_point in [1.0_f32, 1.25, 1.5] {
            let text_clip =
                details_text_clip_rect(clip, egui::Align2::RIGHT_CENTER, pixels_per_point)
                    .expect("wide clip should be drawable");
            assert_eq!(text_clip.min, clip.min);
            assert!((text_clip.right() - clip.right() - 1.0 / pixels_per_point).abs() < 0.0001);
        }
    }

    #[test]
    fn left_aligned_clip_is_unchanged() {
        let clip = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 40.0));
        assert_eq!(
            details_text_clip_rect(clip, egui::Align2::LEFT_CENTER, 1.0),
            Some(clip)
        );
    }

    #[test]
    fn tiny_clip_is_not_drawn() {
        for width in [0.0_f32, 0.5, 1.0] {
            let clip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 20.0));
            assert_eq!(
                details_text_clip_rect(clip, egui::Align2::RIGHT_CENTER, 1.0),
                None
            );
        }
    }
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

fn archive_container_format_label(
    zip_path: &std::path::Path,
    archive_source_override: Option<&std::path::Path>,
    current_folder: Option<&std::path::Path>,
) -> &'static str {
    let display_path = if archive_source_override.is_some()
        && current_folder.is_some_and(|folder| crate::folder_tree::path_eq(folder, zip_path))
    {
        archive_source_override.unwrap_or(zip_path)
    } else {
        zip_path
    };
    display_path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(crate::archive_converter::ArchiveFormat::nested_from_extension)
        .map(crate::archive_converter::ArchiveFormat::label)
        .unwrap_or("ZIP")
}

fn archive_inner_image_kind_label(
    zip_path: &std::path::Path,
    archive_source_override: Option<&std::path::Path>,
    current_folder: Option<&std::path::Path>,
) -> String {
    format!(
        "{} 内画像",
        archive_container_format_label(zip_path, archive_source_override, current_folder)
    )
}

fn details_row_text_color(visuals: &egui::Visuals, selected: bool) -> egui::Color32 {
    if selected {
        visuals.selection.stroke.color
    } else {
        // 通常行は共通テーマの primary text。文字コントラスト設定は os_theme 側で
        // ラベル、ツールバー、メニューとまとめて反映する。
        visuals.text_color()
    }
}

fn selection_info_popup_y(cell_rect: egui::Rect, viewport: egui::Rect, popup_height: f32) -> f32 {
    const VIEWPORT_MARGIN: f32 = 8.0;
    const CELL_GAP: f32 = 6.0;

    let below = cell_rect.bottom() + CELL_GAP;
    let below_max = viewport.bottom() - VIEWPORT_MARGIN - popup_height;
    if below <= below_max {
        return below;
    }

    let above = cell_rect.top() - CELL_GAP - popup_height;
    let viewport_top = viewport.top() + VIEWPORT_MARGIN;
    if above >= viewport_top {
        above
    } else {
        // 極端に低い viewport では完全非重複にできないが、通常サイズでは上側領域の
        // 上限を越えて下へ clamp しない。これが選択行への再侵入を防ぐ境界になる。
        viewport_top.min(above)
    }
}

fn details_kind_label(
    item: &GridItem,
    archive_source_override: Option<&std::path::Path>,
    current_folder: Option<&std::path::Path>,
) -> String {
    match item {
        GridItem::Folder(path) if crate::path_key::is_drive_or_share_root(path) => {
            "ドライブ".to_string()
        }
        GridItem::Folder(_) => "フォルダ".to_string(),
        GridItem::Image(path) => details_ext_kind(path, "画像"),
        GridItem::Video(path) => details_ext_kind(path, "動画"),
        GridItem::Audio(path) => details_ext_kind(path, "音声"),
        GridItem::ZipFile(path) => details_ext_kind(path, "ZIP"),
        GridItem::PdfFile(path) => details_ext_kind(path, "PDF"),
        GridItem::ConvertibleArchive { format, .. } => format.label().to_string(),
        GridItem::ZipImage { zip_path, .. } => {
            archive_inner_image_kind_label(zip_path, archive_source_override, current_folder)
        }
        GridItem::PdfPage { .. } => "PDF ページ".to_string(),
        GridItem::ZipDir {
            zip_path,
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
                format!(
                    "{} 内フォルダ",
                    archive_container_format_label(
                        zip_path,
                        archive_source_override,
                        current_folder,
                    )
                )
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
        | GridItem::Audio(path)
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
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SelectionInfoContent {
    lines: Vec<String>,
}

impl SelectionInfoContent {
    fn tooltip_text(&self) -> String {
        self.lines.join("\n")
    }

    #[cfg(test)]
    fn single_line_text(&self) -> String {
        self.lines.join("   ")
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
    if matches!(
        entry.kind,
        crate::reading_history_db::ReadingHistoryKind::Video
            | crate::reading_history_db::ReadingHistoryKind::Audio
    ) {
        let position_ms = entry.media_position_ms?;
        if position_ms < 0 {
            return None;
        }
        return match entry.media_duration_ms {
            Some(duration_ms) if duration_ms > 0 => Some(format!(
                "{} / {}",
                format_reading_history_media_time(position_ms),
                format_reading_history_media_time(duration_ms),
            )),
            _ => Some(format_reading_history_media_time(position_ms)),
        };
    }

    let page = entry.last_page?;
    if page <= 0 {
        return None;
    }
    match entry.page_count {
        Some(count) if count > 0 => Some(format!("{page} / {count}")),
        _ => Some(format!("{page} ページ目")),
    }
}

fn format_reading_history_media_time(value_ms: i64) -> String {
    let total_secs = value_ms.max(0) / 1000;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn grid_selection_item_is_checkable(items: &[GridItem], allow_folders: bool, idx: usize) -> bool {
    items.get(idx).is_some_and(|item| {
        item.is_checkable() || (allow_folders && matches!(item, GridItem::Folder(_)))
    })
}

fn grid_selection_anchor(
    anchor: Option<GridClickSelectionAnchor>,
    items_generation: u64,
    selected: Option<usize>,
    display_order: &[usize],
    clicked_idx: usize,
) -> usize {
    anchor
        .and_then(|anchor| anchor.index_for_generation(items_generation))
        .filter(|idx| display_order.contains(idx))
        .or_else(|| selected.filter(|idx| display_order.contains(idx)))
        .unwrap_or(clicked_idx)
}

fn add_grid_selection_range(
    checked: &mut HashSet<usize>,
    display_order: &[usize],
    anchor_idx: usize,
    clicked_idx: usize,
    items: &[GridItem],
    allow_folders: bool,
) {
    let Some(anchor_pos) = display_order.iter().position(|&idx| idx == anchor_idx) else {
        return;
    };
    let Some(clicked_pos) = display_order.iter().position(|&idx| idx == clicked_idx) else {
        return;
    };
    let (start, end) = if anchor_pos <= clicked_pos {
        (anchor_pos, clicked_pos)
    } else {
        (clicked_pos, anchor_pos)
    };
    for &idx in &display_order[start..=end] {
        if grid_selection_item_is_checkable(items, allow_folders, idx) {
            checked.insert(idx);
        }
    }
}

/// サムネイル / 詳細表示に共通のクリック選択状態遷移。
///
/// 単一選択は `selected`、複数選択は `checked` で表す既存モデルを維持する。
/// `anchor` は Shift+クリックの起点で、通常 / Ctrl クリック時だけ更新する。
#[allow(clippy::too_many_arguments)]
fn apply_grid_click_selection(
    mode: GridClickSelectionMode,
    selected: &mut Option<usize>,
    anchor: &mut Option<GridClickSelectionAnchor>,
    items_generation: u64,
    checked: &mut HashSet<usize>,
    display_order: &[usize],
    items: &[GridItem],
    allow_folders: bool,
    clicked_idx: usize,
    ctrl: bool,
    shift: bool,
) {
    let mode = mode.normalized();
    if shift {
        let previous_anchor_is_valid = anchor
            .and_then(|anchor| anchor.index_for_generation(items_generation))
            .is_some_and(|idx| display_order.contains(&idx));
        let range_anchor = grid_selection_anchor(
            *anchor,
            items_generation,
            *selected,
            display_order,
            clicked_idx,
        );
        if mode == GridClickSelectionMode::Explorer {
            checked.clear();
        }
        add_grid_selection_range(
            checked,
            display_order,
            range_anchor,
            clicked_idx,
            items,
            allow_folders,
        );
        *selected = Some(clicked_idx);
        if !previous_anchor_is_valid {
            *anchor = Some(GridClickSelectionAnchor::new(
                range_anchor,
                items_generation,
            ));
        }
        return;
    }

    if !ctrl {
        if mode == GridClickSelectionMode::Explorer {
            checked.clear();
        }
        *selected = Some(clicked_idx);
        *anchor = Some(GridClickSelectionAnchor::new(clicked_idx, items_generation));
        return;
    }

    if mode == GridClickSelectionMode::Check {
        if checked.is_empty()
            && let Some(previous) = *selected
            && previous != clicked_idx
            && display_order.contains(&previous)
            && grid_selection_item_is_checkable(items, allow_folders, previous)
        {
            checked.insert(previous);
        }
        if grid_selection_item_is_checkable(items, allow_folders, clicked_idx)
            && !checked.remove(&clicked_idx)
        {
            checked.insert(clicked_idx);
        }
        *selected = Some(clicked_idx);
        *anchor = Some(GridClickSelectionAnchor::new(clicked_idx, items_generation));
        return;
    }

    // エクスプローラー方式の Ctrl+クリック。チェック不可のセルは複数選択へ
    // 混在できないため、そのセルだけの単一選択として扱う。
    if !grid_selection_item_is_checkable(items, allow_folders, clicked_idx) {
        if checked.is_empty() && *selected == Some(clicked_idx) {
            *selected = None;
            *anchor = None;
        } else {
            checked.clear();
            *selected = Some(clicked_idx);
            *anchor = Some(GridClickSelectionAnchor::new(clicked_idx, items_generation));
        }
        return;
    }

    if checked.is_empty() {
        match *selected {
            Some(previous) if previous == clicked_idx => {
                *selected = None;
                *anchor = None;
                return;
            }
            Some(previous)
                if display_order.contains(&previous)
                    && grid_selection_item_is_checkable(items, allow_folders, previous) =>
            {
                checked.insert(previous);
                checked.insert(clicked_idx);
            }
            _ => {
                *selected = Some(clicked_idx);
                *anchor = Some(GridClickSelectionAnchor::new(clicked_idx, items_generation));
                return;
            }
        }
    } else if !checked.remove(&clicked_idx) {
        checked.insert(clicked_idx);
    }

    *anchor = Some(GridClickSelectionAnchor::new(clicked_idx, items_generation));
    match checked.len() {
        0 => {
            *selected = None;
        }
        1 => {
            *selected = checked.iter().copied().next();
            checked.clear();
        }
        _ => {
            *selected = if checked.contains(&clicked_idx) {
                Some(clicked_idx)
            } else {
                display_order
                    .iter()
                    .copied()
                    .find(|idx| checked.contains(idx))
            };
        }
    }
}

fn apply_grid_secondary_selection(
    mode: GridClickSelectionMode,
    selected: &mut Option<usize>,
    anchor: &mut Option<GridClickSelectionAnchor>,
    items_generation: u64,
    checked: &mut HashSet<usize>,
    clicked_idx: usize,
) {
    if mode.normalized() == GridClickSelectionMode::Explorer && !checked.contains(&clicked_idx) {
        checked.clear();
    }
    *selected = Some(clicked_idx);
    *anchor = Some(GridClickSelectionAnchor::new(clicked_idx, items_generation));
}

fn clear_grid_selection_for_background_click(
    mode: GridClickSelectionMode,
    selected: &mut Option<usize>,
    anchor: &mut Option<GridClickSelectionAnchor>,
    checked: &mut HashSet<usize>,
) -> bool {
    if mode.normalized() != GridClickSelectionMode::Explorer {
        return false;
    }
    let changed = selected.is_some() || anchor.is_some() || !checked.is_empty();
    *selected = None;
    *anchor = None;
    checked.clear();
    changed
}

/// Applies the optional grid-selection change at the right-button press
/// boundary. Keeping this separate from the ring/gesture state machines makes
/// the selection policy explicit and leaves checked multi-selection untouched.
fn apply_grid_right_drag_start_selection(
    selected: &mut Option<usize>,
    enabled: bool,
    mode: &crate::ring_shortcut::RightDragMode,
    target_idx: Option<usize>,
) -> bool {
    if !enabled
        || !matches!(
            mode,
            crate::ring_shortcut::RightDragMode::RingShortcut
                | crate::ring_shortcut::RightDragMode::MouseGesture
        )
    {
        return false;
    }
    let Some(idx) = target_idx else {
        return false;
    };
    *selected = Some(idx);
    true
}

#[cfg(test)]
mod grid_right_drag_start_selection_tests {
    use super::*;
    use crate::ring_shortcut::RightDragMode;

    #[test]
    fn active_modes_select_the_start_cell_only_when_opted_in() {
        for mode in [RightDragMode::RingShortcut, RightDragMode::MouseGesture] {
            let mut selected = Some(1);
            assert!(apply_grid_right_drag_start_selection(
                &mut selected,
                true,
                &mode,
                Some(4)
            ));
            assert_eq!(selected, Some(4));

            selected = Some(1);
            assert!(!apply_grid_right_drag_start_selection(
                &mut selected,
                false,
                &mode,
                Some(4)
            ));
            assert_eq!(selected, Some(1));
        }
    }

    #[test]
    fn disabled_mode_and_background_start_preserve_selection() {
        let mut selected = Some(2);
        assert!(!apply_grid_right_drag_start_selection(
            &mut selected,
            true,
            &RightDragMode::Disabled,
            Some(5)
        ));
        assert_eq!(selected, Some(2));

        assert!(!apply_grid_right_drag_start_selection(
            &mut selected,
            true,
            &RightDragMode::MouseGesture,
            None
        ));
        assert_eq!(selected, Some(2));
    }
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug)]
struct DetailsColumnMenuLayout {
    pane_rects: [egui::Rect; 2],
    #[cfg(test)]
    viewport_rect: egui::Rect,
    #[cfg(test)]
    content_size: egui::Vec2,
    #[cfg(test)]
    scroll_offset: egui::Vec2,
}

fn draw_details_column_menu_layout(
    ui: &mut egui::Ui,
    max_height: f32,
    mut draw_pane: impl FnMut(DetailsColumnMenuPane, &mut egui::Ui),
) -> DetailsColumnMenuLayout {
    let content_height_id = ui.make_persistent_id(DETAILS_COLUMN_MENU_CONTENT_HEIGHT_ID);
    let memoized_content_height = ui
        .ctx()
        .data_mut(|data| data.get_temp::<f32>(content_height_id))
        .unwrap_or(DETAILS_COLUMN_MENU_INITIAL_CONTENT_HEIGHT);
    let rounding_slack = DETAILS_COLUMN_MENU_HEIGHT_ROUNDING_SLACK_PX
        / normalized_pixels_per_point(ui.ctx().pixels_per_point());
    let body_height = (memoized_content_height + rounding_slack)
        .min(max_height)
        .max(1.0);
    // ScrollArea は max_height より先に親 Ui の available height で縮むため、画面予算と
    // 実測 content height の小さい方を親領域として明示確保する。
    let output = ui
        .allocate_ui_with_layout(
            egui::vec2(0.0, body_height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                egui::ScrollArea::vertical()
                    .max_height(body_height)
                    .auto_shrink([true, false])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            let columns = ui.allocate_ui_with_layout(
                                egui::vec2(DETAILS_COLUMN_MENU_COLUMNS_WIDTH, 0.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.set_width(DETAILS_COLUMN_MENU_COLUMNS_WIDTH);
                                    draw_pane(DetailsColumnMenuPane::Columns, ui);
                                },
                            );

                            // Separator widget は horizontal Ui の available height を使い切り、
                            // content 計測を body height へ張り付かせる。占有幅だけ先に予約する。
                            let (separator_rect, _) = ui.allocate_exact_size(
                                egui::vec2(DETAILS_COLUMN_MENU_SEPARATOR_WIDTH, 0.0),
                                egui::Sense::hover(),
                            );

                            let format = ui.allocate_ui_with_layout(
                                egui::vec2(DETAILS_COLUMN_MENU_FORMAT_WIDTH, 0.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.set_width(DETAILS_COLUMN_MENU_FORMAT_WIDTH);
                                    draw_pane(DetailsColumnMenuPane::Format, ui);
                                },
                            );

                            let pane_rects = [columns.response.rect, format.response.rect];
                            let separator_top = pane_rects[0].top().min(pane_rects[1].top());
                            let separator_bottom =
                                pane_rects[0].bottom().max(pane_rects[1].bottom());
                            let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
                            ui.painter().vline(
                                separator_rect.center().x,
                                separator_top..=separator_bottom,
                                stroke,
                            );
                            pane_rects
                        })
                        .inner
                    })
            },
        )
        .inner;
    let pane_rects = output.inner;
    // 中央線は layout に参加しないため、ScrollArea の実 content 高をそのまま次 frame へ渡せる。
    let measured_content_height = output.content_size.y;
    ui.ctx().data_mut(|data| {
        data.insert_temp(content_height_id, measured_content_height);
    });
    if (measured_content_height - memoized_content_height).abs() > 0.5 {
        ui.ctx().request_repaint();
    }
    DetailsColumnMenuLayout {
        pane_rects,
        #[cfg(test)]
        viewport_rect: output.inner_rect,
        #[cfg(test)]
        content_size: output.content_size,
        #[cfg(test)]
        scroll_offset: output.state.offset,
    }
}

fn draw_details_column_menu_with_header(
    ui: &mut egui::Ui,
    max_height: f32,
    draw_pane: impl FnMut(DetailsColumnMenuPane, &mut egui::Ui),
) -> DetailsColumnMenuLayout {
    let menu_width = DETAILS_COLUMN_MENU_COLUMNS_WIDTH
        + DETAILS_COLUMN_MENU_SEPARATOR_WIDTH
        + DETAILS_COLUMN_MENU_FORMAT_WIDTH
        + ui.spacing().item_spacing.x * 2.0;
    // ヘッダを先に描くため、ここで本文 2 ペイン分の幅を確定する。広い親 Ui の
    // justified layout にヘッダだけが引っ張られて popup が画面幅まで伸びるのを防ぐ。
    ui.set_width(menu_width);
    let content_start_y = ui.cursor().min.y;
    draw_sticky_settings_menu_header(ui, "列の設定", true);
    let header_height = (ui.cursor().min.y - content_start_y).max(0.0);
    draw_details_column_menu_layout(ui, (max_height - header_height).max(1.0), draw_pane)
}

#[cfg(test)]
mod details_column_context_menu_layout_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn draw_height_test_menu(pane: DetailsColumnMenuPane, ui: &mut egui::Ui) {
        match pane {
            DetailsColumnMenuPane::Columns => {
                let mut dedicated = false;
                ui.checkbox(&mut dedicated, "以下を専用の設定にする");
                ui.separator();
                ui.label(egui::RichText::new("一覧と下部情報バー共通").strong());
                ui.separator();
                let mut checked = true;
                for label in [
                    "名前",
                    "名前の幅を自動調整",
                    "プレビュー",
                    "★",
                    "タグ",
                    "種類",
                    "ページ数",
                    "サイズ",
                    "更新日時",
                    "状態",
                ] {
                    ui.checkbox(&mut checked, label);
                }
                ui.separator();
                for label in ["作成日時", "画像解像度", "長さ", "動画解像度", "コーデック"]
                {
                    ui.checkbox(&mut checked, label);
                }
            }
            DetailsColumnMenuPane::Format => {
                ui.label(egui::RichText::new("書式 (すべての表示で共通)").strong());
                ui.separator();
                ui.label("サイズ表示");
                let mut selected = 0;
                for (value, label) in [(0, "最適"), (1, "バイト"), (2, "KB"), (3, "MB")] {
                    ui.radio_value(&mut selected, value, label);
                }
                ui.separator();
                ui.label("日時");
                let mut show_seconds = false;
                ui.checkbox(&mut show_seconds, "秒まで表示");
                ui.separator();
                ui.label("行表示");
                for (value, label) in [
                    (0, "線のみ"),
                    (1, "交互背景色"),
                    (2, "線+交互"),
                    (3, "なし"),
                ] {
                    ui.radio_value(&mut selected, value, label);
                }
            }
        }
    }

    #[test]
    fn tall_screen_shows_the_full_menu_without_scrolling() {
        use egui_kittest::Harness;

        let measured = Arc::new(Mutex::new(None));
        let measured_in_ui = Arc::clone(&measured);
        let mut harness = Harness::builder()
            .with_pixels_per_point(1.25)
            .with_size(egui::vec2(900.0, 900.0))
            .build(move |ctx| {
                let area = egui::Area::new(egui::Id::new(0xD37A_11_u64))
                    .fixed_pos(egui::pos2(0.0, 0.3))
                    .movable(false)
                    .default_size(egui::vec2(500.0, 120.0))
                    .layout(egui::Layout::top_down_justified(egui::Align::Min))
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style())
                            .show(ui, |ui| {
                                draw_details_column_menu_with_header(
                                    ui,
                                    (ctx.content_rect().height()
                                        - DETAILS_COLUMN_MENU_SCREEN_MARGIN)
                                        .max(1.0),
                                    draw_height_test_menu,
                                )
                            })
                            .inner
                    });
                *measured_in_ui.lock().unwrap() =
                    Some((area.inner, area.response.rect, ctx.content_rect()));
            });
        harness.ctx.options_mut(|options| options.zoom_factor = 1.3);
        harness.run_steps(3);

        let (layout, menu_rect, content_rect) = measured.lock().unwrap().unwrap();
        assert!(
            layout.content_size.y <= layout.viewport_rect.height() + 0.5,
            "tall menu still scrolls: content={:?}, viewport={:?}, panes={:?}",
            layout.content_size,
            layout.viewport_rect,
            layout.pane_rects
        );
        assert!(
            menu_rect.bottom() <= content_rect.bottom() + 1.0,
            "menu including header exceeds the screen: menu={menu_rect:?}, screen={content_rect:?}"
        );
    }

    #[test]
    fn short_screen_bounds_the_menu_and_keeps_it_scrollable() {
        use egui_kittest::{Harness, kittest::Queryable};

        let measured = Arc::new(Mutex::new(None));
        let measured_in_ui = Arc::clone(&measured);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 200.0))
            .build(move |ctx| {
                let area = egui::Area::new(egui::Id::new(0xD37A_12_u64))
                    .fixed_pos(egui::pos2(0.0, 0.0))
                    .movable(false)
                    .default_size(egui::vec2(500.0, 500.0))
                    .layout(egui::Layout::top_down_justified(egui::Align::Min))
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style())
                            .show(ui, |ui| {
                                draw_details_column_menu_with_header(
                                    ui,
                                    (ctx.content_rect().height()
                                        - DETAILS_COLUMN_MENU_SCREEN_MARGIN)
                                        .max(1.0),
                                    draw_height_test_menu,
                                )
                            })
                            .inner
                    });
                *measured_in_ui.lock().unwrap() = Some(area.inner);
            });
        harness.run_steps(3);

        let layout = measured.lock().unwrap().unwrap();
        assert!(layout.viewport_rect.height() <= 200.0 - DETAILS_COLUMN_MENU_SCREEN_MARGIN + 1.0);
        assert!(layout.content_size.y > layout.viewport_rect.height() + 1.0);

        harness.get_by_label("コーデック").scroll_to_me();
        harness.run_steps(2);
        let scrolled_layout = measured.lock().unwrap().unwrap();
        assert!(scrolled_layout.scroll_offset.y > 1.0);
    }

    #[test]
    fn menu_width_is_bounded_and_columns_are_side_by_side() {
        use egui_kittest::Harness;

        let measured = Arc::new(Mutex::new(None));
        let measured_in_ui = Arc::clone(&measured);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1200.0, 700.0))
            .build(move |ctx| {
                let area = egui::Area::new(egui::Id::new("details_column_menu_layout_test"))
                    .fixed_pos(egui::pos2(0.0, 0.0))
                    .movable(false)
                    .default_size(egui::vec2(1200.0, 650.0))
                    .layout(egui::Layout::top_down_justified(egui::Align::Min))
                    .show(ctx, |ui| {
                        draw_details_column_menu_with_header(ui, 650.0, |pane, ui| match pane {
                            DetailsColumnMenuPane::Columns => {
                                ui.label(egui::RichText::new("表示する列").strong());
                                ui.separator();
                                let mut checked = true;
                                for label in [
                                    "名前",
                                    "名前の幅を自動調整",
                                    "プレビュー",
                                    "★",
                                    "タグ",
                                    "種類",
                                    "ページ数",
                                    "サイズ",
                                    "更新日時",
                                    "状態",
                                ] {
                                    ui.checkbox(&mut checked, label);
                                }
                                ui.separator();
                                for label in
                                    ["作成日時", "画像解像度", "長さ", "動画解像度", "コーデック"]
                                {
                                    ui.checkbox(&mut checked, label);
                                }
                            }
                            DetailsColumnMenuPane::Format => {
                                ui.label(egui::RichText::new("書式 (すべての表示で共通)").strong());
                                ui.separator();
                                ui.label("サイズ表示");
                                let mut selected = 0;
                                for (value, label) in
                                    [(0, "最適"), (1, "バイト"), (2, "KB"), (3, "MB")]
                                {
                                    ui.radio_value(&mut selected, value, label);
                                }
                                ui.separator();
                                ui.label("日時");
                                let mut show_seconds = false;
                                ui.checkbox(&mut show_seconds, "秒まで表示");
                                ui.separator();
                                ui.label("行表示");
                                for (value, label) in [
                                    (0, "線のみ"),
                                    (1, "交互背景色"),
                                    (2, "線+交互"),
                                    (3, "なし"),
                                ] {
                                    ui.radio_value(&mut selected, value, label);
                                }
                            }
                        })
                    });
                *measured_in_ui.lock().unwrap() = Some((area.response.rect, area.inner.pane_rects));
            });
        harness.run_steps(3);

        let (menu_rect, [columns_rect, format_rect]) =
            measured.lock().unwrap().expect("menu was rendered");
        assert!(
            menu_rect.width() <= 500.0,
            "menu consumed the 1200px available width: {}",
            menu_rect.width()
        );
        assert!(
            (columns_rect.width() - DETAILS_COLUMN_MENU_COLUMNS_WIDTH).abs() <= 1.0,
            "columns pane width changed: {}",
            columns_rect.width()
        );
        assert!(
            (format_rect.width() - DETAILS_COLUMN_MENU_FORMAT_WIDTH).abs() <= 1.0,
            "format pane width changed: {}",
            format_rect.width()
        );
        assert!(
            columns_rect.max.x < format_rect.min.x,
            "columns overlap: columns={columns_rect:?}, format={format_rect:?}"
        );
        assert!(
            (columns_rect.min.y - format_rect.min.y).abs() <= 1.0,
            "columns are not top-aligned: columns={columns_rect:?}, format={format_rect:?}"
        );
        assert!(
            format_rect.max.x <= menu_rect.max.x + 1.0,
            "format pane was pushed outside the menu: menu={menu_rect:?}, format={format_rect:?}"
        );
    }
}

impl App {
    // ── メニューバー ─────────────────────────────────────────────────

    /// メニューバーを描画し、ナビゲーション先とソート変更の有無を返す。
    pub(crate) fn render_menubar(&mut self, ctx: &egui::Context) -> (Option<PathBuf>, bool) {
        let mut fav_nav: Option<PathBuf> = None;
        let mut smart_folder_open: Option<uuid::Uuid> = None;
        let mut settings_changed = false;
        let mut sort_changed = false;
        let book_sort_locked = self.page_order_locked_for_current_view();
        let rating_counts = self.rating_counts();
        let selected_video_path =
            self.selected
                .and_then(|idx| self.items.get(idx))
                .and_then(|item| match item {
                    GridItem::Video(path) => Some(path.clone()),
                    _ => None,
                });
        // 変換対象 = スペース選択 (checked) があればその集合、無ければカーソル位置の
        // ConvertibleArchive。レーティング / タグと同じ selection_target_indices を使う。
        let convert_target_count = {
            let mut seen = std::collections::HashSet::new();
            self.selection_target_indices(crate::app::ActionSurface::MainWindow)
                .into_iter()
                .filter_map(|idx| match self.items.get(idx) {
                    Some(GridItem::ConvertibleArchive { path, .. }) => Some(path.clone()),
                    _ => None,
                })
                .filter(|p| seen.insert(p.clone()))
                .count()
        };
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
        let reload_menu_label = self.keymap.menu_command_label(MenuCommandId::FileReload);
        let metadata_export_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::FileMetadataExport);
        let metadata_import_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::FileMetadataImport);
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
        let smart_folder_new_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::SmartFoldersNew);
        let smart_folder_add_current_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::SmartFoldersAddCurrent);
        let smart_folder_manage_menu_label = self
            .keymap
            .menu_command_label(MenuCommandId::SmartFoldersManage);
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
        let convert_to_zip_menu_label = self.keymap.menu_command_label(MenuCommandId::ConvertToZip);
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
                let mut top_menu_responses = Vec::with_capacity(9);

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
                                                self.enter_reading_history_from_menu();
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
                                                        self.enter_rating_view_from_menu(stars);
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
                                        MenuCommandId::FileReload => {
                                            if ui.button(&reload_menu_label).clicked() {
                                                self.reload_top_level_grid(ctx);
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::FileMetadataExport => {
                                            let response = ui.add_enabled(
                                                self.metadata_transfer_target().is_some(),
                                                egui::Button::new(&metadata_export_menu_label),
                                            );
                                            if response.clicked() {
                                                self.open_metadata_export_dialog();
                                                ui.close();
                                            }
                                            response.on_disabled_hover_text(
                                                "実フォルダを表示しているときに使用できます",
                                            );
                                        }
                                        MenuCommandId::FileMetadataImport => {
                                            let response = ui.add_enabled(
                                                self.metadata_transfer_target().is_some(),
                                                egui::Button::new(&metadata_import_menu_label),
                                            );
                                            if response.clicked() {
                                                self.open_metadata_import_dialog();
                                                ui.close();
                                            }
                                            response.on_disabled_hover_text(
                                                "実フォルダを表示しているときに使用できます",
                                            );
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
                                                self.request_application_quit(ctx);
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
                                                self.enter_rating_view_from_menu(stars);
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
                                                    self.fav_add_error = None;
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
                        TopMenuId::SmartFolders => {
                            let smart_folder_commands = &resolved_top_menu.commands;
                            let response = ui.menu_button(TopMenuId::SmartFolders.label(), |ui| {
                                for &command in smart_folder_commands {
                                    match command {
                                        MenuCommandId::SmartFoldersNew => {
                                            if ui.button(&smart_folder_new_menu_label).clicked() {
                                                self.begin_new_smart_folder();
                                                ui.close();
                                            }
                                        }
                                        MenuCommandId::SmartFoldersAddCurrent => {
                                            let targets = self
                                                .settings
                                                .smart_folders
                                                .iter()
                                                .map(|definition| {
                                                    (definition.id, definition.name.clone())
                                                })
                                                .collect::<Vec<_>>();
                                            let source_error =
                                                self.smart_folder_current_rule_source().err();
                                            let can_add = !targets.is_empty()
                                                && source_error.is_none();
                                            let disabled_reason = if targets.is_empty() {
                                                "先にスマートフォルダを作成してください"
                                            } else {
                                                source_error.unwrap_or("")
                                            };
                                            let submenu = ui.add_enabled_ui(can_add, |ui| {
                                                ui.menu_button(
                                                    &smart_folder_add_current_menu_label,
                                                    |ui| {
                                                        for (id, name) in &targets {
                                                            if ui.button(name).clicked() {
                                                                self.begin_add_current_smart_folder_rule(*id);
                                                                ui.close();
                                                            }
                                                        }
                                                    },
                                                )
                                            });
                                            submenu
                                                .inner
                                                .response
                                                .hover_tip_disabled(disabled_reason);
                                        }
                                        MenuCommandId::SmartFoldersManage => {
                                            if ui
                                                .add_enabled(
                                                    !self.settings.smart_folders.is_empty(),
                                                    egui::Button::new(&smart_folder_manage_menu_label),
                                                )
                                                .clicked()
                                            {
                                                self.open_smart_folder_manager(None);
                                                ui.close();
                                            }
                                        }
                                        _ => {}
                                    }
                                }

                                ui.separator();
                                if self.settings.smart_folders.is_empty() {
                                    ui.label(egui::RichText::new("（未登録）").weak());
                                } else {
                                    for definition in &self.settings.smart_folders {
                                        let selected = self.items_are_smart_folder_view
                                            && self.current_smart_folder_id
                                                == Some(definition.id);
                                        if ui
                                            .selectable_label(selected, &definition.name)
                                            .clicked()
                                        {
                                            smart_folder_open = Some(definition.id);
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
                        TopMenuId::Convert => {
                            let commands = &resolved_top_menu.commands;
                            let response = ui.menu_button(TopMenuId::Convert.label(), |ui| {
                                for &command in commands {
                                    if command != MenuCommandId::ConvertToZip {
                                        continue;
                                    }
                                    let enabled = convert_target_count > 0
                                        && self.archive_convert.is_none()
                                        && self.batch_convert.is_none();
                                    let response = ui
                                        .add_enabled(
                                            enabled,
                                            egui::Button::new(&convert_to_zip_menu_label),
                                        )
                                        .hover_tip_disabled(
                                            "RAR/CBR/7z/CB7/LZH/LHA ファイルを選択してください\
                                             (スペースキーで複数選択可)",
                                        );
                                    if response.clicked() {
                                        self.start_batch_convert_to_zip();
                                        ui.close();
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
                                    let selection_count = self.tag_target_path_count(crate::app::ActionSurface::MainWindow);
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
                                        self.request_tag_clear_for_selection(crate::app::ActionSurface::MainWindow);
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
                                                self.request_tag_toggle_for_selection(&tag.name, crate::app::ActionSurface::MainWindow);
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
                                    // 無効ウィジェットは通常の hover を sense しないため、
                                    // disabled 専用ツールチップで理由を出す。
                                    ui.add_enabled(false, egui::Button::new("ソート順: 固定"))
                                        .on_disabled_hover_text(
                                            "本として表示中や閲覧履歴では、並び順が固定されます（一覧の並べ替えは使えません）。",
                                        );
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
                                        if self.items_are_subfolder_expansion_view {
                                            ui.separator();
                                            ui.label("サブ展開の並び単位");
                                            for &mode in crate::settings::SubfolderExpansionOrder::all() {
                                                let checked =
                                                    self.settings.subfolder_expansion_order == mode;
                                                let prefix = if checked { "✓ " } else { "  " };
                                                let resp = ui
                                                    .button(format!("{prefix}{}", mode.label()))
                                                    .on_hover_text(mode.description());
                                                if resp.clicked() && !checked {
                                                    self.settings.subfolder_expansion_order = mode;
                                                    sort_changed = true;
                                                    ui.close();
                                                }
                                            }
                                        }
                                    });
                                }
                                ui.menu_button("スケーリング", |ui| {
                                    for scale in crate::settings::ui_scale_factor_steps() {
                                        let checked = (self.settings.ui_scale_factor - scale).abs()
                                            < f32::EPSILON;
                                        let percent = (scale * 100.0).round() as u32;
                                        if ui_scale_menu_button(ui, checked, percent).clicked() {
                                            self.set_ui_scale_factor(ctx, scale);
                                            settings_changed = true;
                                            ui.close();
                                        }
                                    }
                                });
                                ui.separator();
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
                                            if ui.button(&settings_preferences_menu_label).clicked()
                                            {
                                                self.show_preferences = true;
                                                ui.close();
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                ui.separator();
                                // このサブメニューは最後に置く。左右に収まらず親項目の下へ開く場合でも、
                                // 後続の設定項目を覆わないための防御であり、上のサブメニュー群へ移さない。
                                // ツールバーのカスタマイズは原則ツールバーの右クリックで行うが、全セクションを
                                // 隠すとツールバー自体が消えて右クリックの入口が無くなる (Codex P2)。
                                // この常設メニューを最後の砦にして、いつでも再表示・既定化できるようにする。
                                // 「既定に戻す」は影響が大きいのでここ (設定メニュー) にだけ出す (show_reset=true)。
                                ui.menu_button("ツールバー", |ui| {
                                    // 通常のサブメニューは外側クリックで閉じるため、右クリック設定
                                    // popup 専用の明示的な × は出さない。
                                    self.draw_toolbar_visibility_menu(ui, true, false);
                                });
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
        if let Some(id) = smart_folder_open {
            let refresh =
                self.items_are_smart_folder_view && self.current_smart_folder_id == Some(id);
            self.open_smart_folder(id, refresh);
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
                    crate::ime_focus::add_singleline(
                        ui,
                        &mut self.book_manager_new_name,
                        None,
                        |edit| edit,
                    );
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
                            .auto_shrink([false, true])
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
                                        crate::ime_focus::add_singleline(
                                            ui,
                                            input,
                                            None,
                                            |edit| edit.desired_width(220.0),
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
                            bookmark_migration_journal_id,
                        }) => {
                            self.apply_book_page_edit_moves_with_journal(
                                &edit_moves,
                                bookmark_migration_journal_id.as_deref(),
                            );
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
                            self.apply_book_page_edit_moves_with_journal(
                                &summary.edit_moves,
                                summary.bookmark_migration_journal_id.as_deref(),
                            );
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

    // ── 選択件数 ─────────────────────────────────────────────────────

    /// チェック済みがある間だけ、メイン一覧の右上に件数と解除操作を表示する。
    pub(crate) fn render_checked_selection_overlay(&mut self, ctx: &egui::Context) {
        let checked_count = self.checked.len();
        if checked_count == 0 {
            return;
        }
        if self.viewer_session_blocks_main_window() || self.any_dialog_open() {
            return;
        }
        if show_checked_selection_overlay(ctx, checked_count) {
            self.checked.clear();
        }
    }

    // ── 進捗バー ─────────────────────────────────────────────────────

    /// 進捗バーオーバーレイ（左下フローティング）を描画する。
    pub(crate) fn render_progress_overlay(&self, ctx: &egui::Context) {
        let ((cur_normal, peak_normal), (cur_upgrade, peak_upgrade)) = self.progress_snapshot();
        let rename_pending = self.rename_pending.is_some();
        if peak_normal == 0 && peak_upgrade == 0 && !rename_pending {
            return;
        }

        egui::Area::new("progress_overlay".into())
            .order(egui::Order::Foreground)
            .interactable(false)
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
                        if rename_pending {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("変更中...");
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
            .interactable(false)
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
        let show_smart_folders = smart_folder_toolbar_visible(
            self.settings.show_toolbar_smart_folders,
            self.settings.smart_folders.len(),
        );
        let show_rating = self.settings.show_toolbar_rating;
        let show_tags = self.settings.show_toolbar_tags;
        let show_folder_tree_button = self.settings.show_toolbar_folder_tree_button;
        let show_bookshelf = self.settings.show_toolbar_bookshelf;
        let book_sort_locked = self.page_order_locked_for_current_view();
        if show_bookshelf && self.book_list_cache.is_none() && self.book_op_pending.is_none() {
            self.request_book_list_refresh();
        }
        let active_book_name = self.active_book_name();
        let toolbar_book_rows = self.book_list_cache.clone();
        let toolbar_pinned_books = self.settings.pinned_books.clone();
        let toolbar_smart_folder_definitions = self.settings.smart_folders.clone();
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
            || show_smart_folders
            || show_rating
            || show_tags;

        if !any_toolbar_section {
            return None;
        }

        let mut toolbar_fav_nav: Option<PathBuf> = None;
        let mut toolbar_smart_folder_open: Option<uuid::Uuid> = None;
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
                        TS::SmartFolders => show_smart_folders,
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
                        sort_label.hover_tip(
                            "本として表示中や閲覧履歴では、並び順が固定されます（一覧の並べ替えは使えません）。",
                        )
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
                                    false
                                } else if self.items_are_bookmark_view {
                                    self.bookmark_view_sort
                                        == crate::bookmark_browser::BookmarkViewSort::Normal(order)
                                } else if self.items_are_rating_view {
                                    self.rating_view_sort
                                        == crate::rating_view::RatingViewSort::Normal(order)
                                } else {
                                    self.settings.sort_order == order
                                };
                                let resp = ui.add_enabled(
                                    !sort_disabled,
                                    egui::Button::selectable(selected, order.short_label()),
                                );
                                // 固定中はボタンが無効なので、通常 hover ではなく disabled
                                // 専用ツールチップで固定理由を出す。
                                let resp = if book_sort_locked {
                                    resp.hover_tip_disabled(
                                        "本として表示中や閲覧履歴では、並び順が固定されます（一覧の並べ替えは使えません）。",
                                    )
                                } else {
                                    resp.on_hover_text(order.description())
                                };
                                if resp.clicked() && !selected {
                                    self.settings.sort_order = order;
                                    self.settings.save();
                                    if self.items_are_bookmark_view {
                                        self.set_bookmark_view_sort(
                                            crate::bookmark_browser::BookmarkViewSort::Normal(order),
                                        );
                                    } else if self.items_are_rating_view {
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
                            if self.items_are_bookmark_view {
                                for sort in [
                                    crate::bookmark_browser::BookmarkViewSort::CreatedAtDesc,
                                    crate::bookmark_browser::BookmarkViewSort::CreatedAtAsc,
                                ] {
                                    let selected = self.bookmark_view_sort == sort;
                                    let resp = ui.add_enabled(
                                        !sort_disabled,
                                        egui::Button::selectable(selected, sort.short_label()),
                                    );
                                    if resp.clicked() && !selected {
                                        self.set_bookmark_view_sort(sort);
                                    }
                                }
                            }
                        }
                        crate::settings::ToolbarSectionDisplay::Dropdown => {
                            ui.add_enabled_ui(!sort_disabled, |ui| {
                                let current_text = if book_sort_locked {
                                    "固定".to_string()
                                } else if self.items_are_bookmark_view {
                                    self.bookmark_view_sort.short_label().to_string()
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
                                            let selected = if self.items_are_bookmark_view {
                                                self.bookmark_view_sort
                                                    == crate::bookmark_browser::BookmarkViewSort::Normal(order)
                                            } else if self.items_are_rating_view {
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
                                                if self.items_are_bookmark_view {
                                                    self.set_bookmark_view_sort(
                                                        crate::bookmark_browser::BookmarkViewSort::Normal(order),
                                                    );
                                                } else if self.items_are_rating_view {
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
                                        if self.items_are_bookmark_view {
                                            ui.separator();
                                            for sort in [
                                                crate::bookmark_browser::BookmarkViewSort::CreatedAtDesc,
                                                crate::bookmark_browser::BookmarkViewSort::CreatedAtAsc,
                                            ] {
                                                let selected = self.bookmark_view_sort == sort;
                                                if ui
                                                    .selectable_label(selected, sort.short_label())
                                                    .clicked()
                                                    && !selected
                                                {
                                                    self.set_bookmark_view_sort(sort);
                                                }
                                            }
                                        }
                                    });
                                // 固定中はコンボが無効なので、disabled 専用ツールチップで
                                // 「固定」表示のホバー時に固定理由を出す。
                                let combo_id = combo.response.id;
                                if book_sort_locked {
                                    combo.response.hover_tip_disabled(
                                        "本として表示中や閲覧履歴では、並び順が固定されます（一覧の並べ替えは使えません）。",
                                    );
                                }
                                toolbar_combo_popup_open |=
                                    egui::ComboBox::is_open(ctx, combo_id);
                            });
                        }
                    }
                    if self.items_are_subfolder_expansion_view {
                        let current_mode = self.settings.subfolder_expansion_order;
                        let combo = egui::ComboBox::from_id_salt("toolbar_subfolder_order_combo")
                            .width(132.0)
                            .height(TOOLBAR_SORT_COMBO_HEIGHT)
                            .selected_text(current_mode.label())
                            .show_ui(ui, |ui| {
                                apply_toolbar_style(ui);
                                for &mode in crate::settings::SubfolderExpansionOrder::all() {
                                    let selected = current_mode == mode;
                                    if ui
                                        .selectable_label(selected, mode.label())
                                        .on_hover_text(mode.description())
                                        .clicked()
                                        && !selected
                                    {
                                        self.settings.subfolder_expansion_order = mode;
                                        self.settings.save();
                                        toolbar_sort_changed = true;
                                    }
                                }
                            });
                        toolbar_combo_popup_open |=
                            egui::ComboBox::is_open(ctx, combo.response.id);
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
                TS::SmartFolders => {
                    let lead = toolbar_label(ui, "スマート:", 60.0, drag_enabled)
                        .hover_tip(lead_hint);
                    self.finish_toolbar_section_lead(
                        ui,
                        lead,
                        TS::SmartFolders,
                        &mut current_section_anchors,
                        &last_section_anchors,
                    );
                    let mode = self.settings.toolbar_smart_folders_display;
                    let (show_inline, new_collapsed) = toolbar_section_fold_toggle(
                        ui,
                        mode,
                        self.settings.toolbar_smart_folders_collapsed,
                    );
                    if let Some(collapsed) = new_collapsed {
                        self.settings.toolbar_smart_folders_collapsed = collapsed;
                        self.settings.save();
                    }
                    let current_id = self
                        .items_are_smart_folder_view
                        .then_some(self.current_smart_folder_id)
                        .flatten();
                    if mode == crate::settings::ToolbarSectionDisplay::Dropdown {
                        let selected_text = current_id
                            .and_then(|id| {
                                toolbar_smart_folder_definitions
                                    .iter()
                                    .find(|definition| definition.id == id)
                            })
                            .map(|definition| definition.name.clone())
                            .unwrap_or_else(|| "選択".to_string());
                        let combo = ui
                            .allocate_ui_with_layout(
                                egui::vec2(180.0, ui.spacing().interact_size.y),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    egui::ComboBox::from_id_salt("toolbar_smart_folder_combo")
                                        .width(172.0)
                                        .selected_text(selected_text)
                                        .show_ui(ui, |ui| {
                                            apply_toolbar_style(ui);
                                            for definition in &toolbar_smart_folder_definitions {
                                                if ui
                                                    .selectable_label(
                                                        current_id == Some(definition.id),
                                                        &definition.name,
                                                    )
                                                    .clicked()
                                                {
                                                    toolbar_smart_folder_open =
                                                        Some(definition.id);
                                                    ui.close();
                                                }
                                            }
                                        })
                                },
                            )
                            .inner;
                        toolbar_combo_popup_open |=
                            egui::ComboBox::is_open(ctx, combo.response.id);
                    } else if show_inline {
                        for definition in &toolbar_smart_folder_definitions {
                            if ui
                                .selectable_label(
                                    current_id == Some(definition.id),
                                    &definition.name,
                                )
                                .clicked()
                            {
                                toolbar_smart_folder_open = Some(definition.id);
                            }
                        }
                    }
                }
                // タグセクション (docs/archive/search-metadata/tag-feature.md §4.3)
                TS::Tags => {
                    let lead = toolbar_label(ui, "タグ:", 42.0, drag_enabled).hover_tip(lead_hint);
                    self.finish_toolbar_section_lead(
                        ui,
                        lead,
                        TS::Tags,
                        &mut current_section_anchors,
                        &last_section_anchors,
                    );
                    let has_target = self.tag_target_path_count(crate::app::ActionSurface::MainWindow) > 0;
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
                    self.draw_toolbar_visibility_menu(ui, false, true);
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
            match self.set_current_folder_rating(n) {
                Ok(true) => self.show_container_rating_toast(n),
                Ok(false) => {
                    // 合成ビュー等で実コンテナが無い場合。常に何らかのフィードバックを返す
                    // (グリッドからのコンテナ付与で無反応に見えないように)。
                    self.show_feedback_toast(
                        "この画面ではこの場所に評価を付けられません".to_string(),
                    );
                }
                Err(error) => self.report_rating_write_error(&error),
            }
        }

        // ツールバーのタグ項目クリック
        if let Some(name) = toolbar_tag_click {
            self.request_tag_toggle_for_selection(&name, crate::app::ActionSurface::MainWindow);
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
        if let Some(id) = toolbar_smart_folder_open {
            let refresh =
                self.items_are_smart_folder_view && self.current_smart_folder_id == Some(id);
            self.open_smart_folder(id, refresh);
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
    /// `show_close_button` = sticky な右クリック設定 popup の明示的な × を出すか。
    fn draw_toolbar_visibility_menu(
        &mut self,
        ui: &mut egui::Ui,
        show_reset: bool,
        show_close_button: bool,
    ) {
        draw_sticky_settings_menu_header(ui, "表示するセクション", show_close_button);
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
        changed |= ui
            .checkbox(&mut s.show_toolbar_smart_folders, "スマートフォルダ")
            .on_hover_text("登録が1件以上あるときだけツールバーに表示されます")
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
        s.show_toolbar_smart_folders = true;
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
        s.toolbar_smart_folders_display = ToolbarSectionDisplay::default();
        s.toolbar_tags_display = ToolbarSectionDisplay::default();
        s.toolbar_bookshelf_display = ToolbarSectionDisplay::default();
        s.toolbar_favorites_collapsed = false;
        s.toolbar_smart_folders_collapsed = false;
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

        draw_sticky_settings_menu_header(ui, toolbar_section_display_label(section), true);
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
            TS::SmartFolders => {
                display_radio(
                    ui,
                    &mut self.settings.toolbar_smart_folders_display,
                    TD::all_with_collapsible(),
                    &mut changed,
                );
                ui.separator();
                if ui.button("スマートフォルダを管理…").clicked() {
                    self.open_smart_folder_manager(None);
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
        draw_sticky_settings_menu_header(ui, "フォルダバー", true);
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
        changed |= ui
            .checkbox(
                &mut self.settings.show_address_bar_omitted_entries,
                "非表示 N 件",
            )
            .on_hover_text(
                "同名でまとめた分・隠れている項目・開けない形式など、\
                 一覧に出していないファイルの件数",
            )
            .changed();

        ui.separator();
        ui.label("場所▼に出す項目:");
        changed |= ui
            .checkbox(&mut self.settings.show_location_drive_list, "ドライブ一覧")
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.show_location_reading_history, "閲覧履歴")
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
        self.facet_name_has_focus = false;
        self.color_filter.input_has_focus = false;
        if !self.settings.show_toolbar_facet_filter {
            return;
        }
        if self.items.is_empty() || self.items_are_drive_list {
            return;
        }

        let mut facet_changed = false;
        let mut place_changed = false;
        let mut non_place_facet_changed = false;
        let mut rating_changed = false;
        let mut color_changed = false;
        let mut facet_name_changed = false;
        let mut bookmark_filter_changed = false;
        let mut reading_history_filter_changed = false;
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
                if self.items_are_reading_history_view {
                    egui::ComboBox::from_id_salt("reading_history_grid_media_filter")
                        .selected_text(format!(
                            "閲覧履歴種別: {}",
                            self.reading_history_media_filter.label()
                        ))
                        .show_ui(ui, |ui| {
                            for filter in crate::bookmark_browser::MediaFilter::ALL {
                                reading_history_filter_changed |= ui
                                    .selectable_value(
                                        &mut self.reading_history_media_filter,
                                        filter,
                                        filter.label(),
                                    )
                                    .changed();
                            }
                        });
                    if matches!(
                        self.reading_history_media_filter,
                        crate::bookmark_browser::MediaFilter::All
                            | crate::bookmark_browser::MediaFilter::Book
                    ) {
                        egui::ComboBox::from_id_salt("reading_history_grid_book_kind_filter")
                            .selected_text(format!(
                                "本: {}",
                                self.reading_history_book_kind_filter.label()
                            ))
                            .show_ui(ui, |ui| {
                                for filter in crate::bookmark_browser::BookKindFilter::ALL {
                                    reading_history_filter_changed |= ui
                                        .selectable_value(
                                            &mut self.reading_history_book_kind_filter,
                                            filter,
                                            filter.label(),
                                        )
                                        .changed();
                                }
                            });
                    }
                    ui.separator();
                }
                if self.items_are_bookmark_view {
                    egui::ComboBox::from_id_salt("bookmark_grid_media_filter")
                        .selected_text(format!(
                            "ブックマーク種別: {}",
                            self.bookmark_media_filter.label()
                        ))
                        .show_ui(ui, |ui| {
                            for filter in crate::bookmark_browser::MediaFilter::ALL {
                                bookmark_filter_changed |= ui
                                    .selectable_value(
                                        &mut self.bookmark_media_filter,
                                        filter,
                                        filter.label(),
                                    )
                                    .changed();
                            }
                        });
                    if matches!(
                        self.bookmark_media_filter,
                        crate::bookmark_browser::MediaFilter::All
                            | crate::bookmark_browser::MediaFilter::Book
                    ) {
                        egui::ComboBox::from_id_salt("bookmark_grid_book_kind_filter")
                            .selected_text(format!(
                                "本: {}",
                                self.bookmark_book_kind_filter.label()
                            ))
                            .show_ui(ui, |ui| {
                                for filter in crate::bookmark_browser::BookKindFilter::ALL {
                                    bookmark_filter_changed |= ui
                                        .selectable_value(
                                            &mut self.bookmark_book_kind_filter,
                                            filter,
                                            filter.label(),
                                        )
                                        .changed();
                                }
                            });
                    }
                    ui.separator();
                }
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
                    if !facet_items.contains(&FI::NameFilter) {
                        facet_name_changed |= self.clear_facet_name_filter_state();
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
                        FI::NameFilter => {
                            ui.separator();
                            let mut output = crate::ime_focus::show_singleline(
                                ui,
                                &mut self.facet_name_input,
                                None,
                                |edit| {
                                    edit.hint_text("ファイル名").desired_width(
                                        self.settings.facet_name_filter_width.width(),
                                    )
                                },
                            );
                            let response_changed = output.response.changed();
                            let menu_changed =
                                crate::ui_helpers::singleline_text_edit_context_menu(
                                    ui,
                                    &mut output,
                                    &mut self.facet_name_input,
                                );
                            let response = output
                                .response
                                .clone()
                                .on_hover_text("表示中の一覧をファイル名で絞り込みます");
                            self.facet_name_has_focus |= response.has_focus();
                            if response_changed || menu_changed {
                                self.schedule_facet_name_filter_update();
                            }
                            if ui
                                .add_enabled(
                                    !self.facet_name_input.is_empty(),
                                    egui::Button::new("×").small(),
                                )
                                .hover_tip("ファイル名フィルターを解除")
                                .clicked()
                            {
                                facet_name_changed |= self.clear_facet_name_filter_state();
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
                let bookmark_filter_active = self.items_are_bookmark_view
                    && (self.bookmark_media_filter
                        != crate::bookmark_browser::MediaFilter::All
                        || self.bookmark_book_kind_filter
                            != crate::bookmark_browser::BookKindFilter::All);
                let reading_history_filter_active = self.items_are_reading_history_view
                    && (self.reading_history_media_filter
                        != crate::bookmark_browser::MediaFilter::All
                        || self.reading_history_book_kind_filter
                            != crate::bookmark_browser::BookKindFilter::All);
                if self.facet_filter_active()
                    || rating_filter_visible
                    || self.color_filter.enabled
                    || bookmark_filter_active
                    || reading_history_filter_active
                {
                    ui.separator();
                    self.draw_facet_active_chips(ui);
                    self.draw_color_filter_active_chip(ui);
                    if ui.small_button("全解除").clicked() {
                        if self.facet_filter_active() {
                            facet_name_changed |= self.clear_facet_name_filter_state();
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
                        if bookmark_filter_active {
                            self.bookmark_media_filter =
                                crate::bookmark_browser::MediaFilter::All;
                            self.bookmark_book_kind_filter =
                                crate::bookmark_browser::BookKindFilter::All;
                            bookmark_filter_changed = true;
                        }
                        if reading_history_filter_active {
                            self.reading_history_media_filter =
                                crate::bookmark_browser::MediaFilter::All;
                            self.reading_history_book_kind_filter =
                                crate::bookmark_browser::BookKindFilter::All;
                            reading_history_filter_changed = true;
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
        let color_scope_changed =
            (facet_changed || facet_name_changed || rating_changed) && self.color_filter.enabled;
        if color_scope_changed {
            self.color_filter.applied_scope_signature = None;
        }
        if facet_changed || facet_name_changed || rating_changed {
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
        if bookmark_filter_changed || reading_history_filter_changed {
            self.rebuild_visible_indices();
        }
    }

    pub(crate) fn render_details_lazy_status_bar(&mut self, ctx: &egui::Context) {
        if self.settings.grid_view_mode != GridViewMode::Details
            || !(self.settings.details_show_page_count
                || self.settings.details_show_created
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
            | LazyColumnState::Reconciling { .. }
            | LazyColumnState::NotRequested
            | LazyColumnState::Cancelled => true,
            LazyColumnState::Ready { failed } => failed > 0,
            LazyColumnState::Disabled => false,
        };
        if !show {
            return;
        }

        // Keep the list origin fixed while metadata loads. A top panel made the
        // entire list jump by one row when it disappeared at completion.
        egui::TopBottomPanel::bottom("details_lazy_status_bar")
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
                        LazyColumnState::Reconciling { done, total, .. } => {
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

        draw_sticky_settings_menu_header(ui, "絞り込みバー", true);
        ui.separator();
        ui.label("表示する項目:");
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
        ui.label("ファイル名欄の幅:");
        ui.horizontal(|ui| {
            for &width in crate::settings::FacetNameFilterWidth::all() {
                changed |= ui
                    .selectable_value(
                        &mut self.settings.facet_name_filter_width,
                        width,
                        width.label(),
                    )
                    .changed();
            }
        });

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
                draw_sticky_settings_menu_header(ui, "場所", true);
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
            LazyColumnState::Reconciling { done, total, .. } => {
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
                    draw_sticky_settings_menu_header(ui, "タグ", true);
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
                        let mut output = crate::ime_focus::show_singleline(
                            ui,
                            &mut self.facet_tag_search_query,
                            None,
                            |edit| {
                                edit.hint_text("#タグ")
                                    .desired_width(150.0)
                                    .min_size(egui::vec2(150.0, 20.0))
                            },
                        );
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
        let (menu_response, _) =
            egui::containers::menu::MenuButton::new(facet_menu_label("日付", active))
                .config(sticky_facet_menu_config())
                .ui(ui, |ui| {
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
                    ui.separator();

                    let mut custom_days = match self.settings.facet_filter.date_preset {
                        Some(FacetDatePreset::CustomDays(days)) => days,
                        _ => 30,
                    };
                    ui.horizontal(|ui| {
                        let selected = matches!(
                            self.settings.facet_filter.date_preset,
                            Some(FacetDatePreset::CustomDays(_))
                        );
                        if ui.selectable_label(selected, "日数を指定").clicked() {
                            self.settings.facet_filter.date_preset =
                                Some(FacetDatePreset::CustomDays(custom_days));
                            changed = true;
                        }
                        if ui
                            .add(
                                egui::DragValue::new(&mut custom_days)
                                    .range(1..=36_500)
                                    .suffix(" 日以内"),
                            )
                            .changed()
                        {
                            self.settings.facet_filter.date_preset =
                                Some(FacetDatePreset::CustomDays(custom_days));
                            changed = true;
                        }
                    });

                    let (mut start, mut end) = match self.settings.facet_filter.date_preset {
                        Some(FacetDatePreset::Range { start, end }) => (start, end),
                        _ => (None, None),
                    };
                    let range_selected = matches!(
                        self.settings.facet_filter.date_preset,
                        Some(FacetDatePreset::Range { .. })
                    );
                    if ui.selectable_label(range_selected, "期間を指定").clicked() {
                        let today = FacetCalendarDate::today_local();
                        start = Some(today);
                        end = Some(today);
                        self.settings.facet_filter.date_preset =
                            Some(FacetDatePreset::Range { start, end });
                        changed = true;
                    }
                    let mut range_changed = false;
                    range_changed |= draw_facet_calendar_date_row(ui, "開始", &mut start);
                    range_changed |= draw_facet_calendar_date_row(ui, "終了", &mut end);
                    if range_changed {
                        if start.zip(end).is_some_and(|(start, end)| start > end) {
                            std::mem::swap(&mut start, &mut end);
                        }
                        self.settings.facet_filter.date_preset =
                            Some(FacetDatePreset::Range { start, end });
                        changed = true;
                    }
                });
        suppress_menu_button_wheel_passthrough(ui.ctx(), &menu_response);
        changed
    }

    fn draw_facet_size_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let active = usize::from(self.settings.facet_filter.size_preset.is_some());
        let mut changed = false;
        let (menu_response, _) =
            egui::containers::menu::MenuButton::new(facet_menu_label("サイズ", active))
                .config(sticky_facet_menu_config())
                .ui(ui, |ui| {
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
                    ui.separator();
                    let range_selected = matches!(
                        self.settings.facet_filter.size_preset,
                        Some(FacetSizePreset::Range { .. })
                    );
                    if ui.selectable_label(range_selected, "範囲指定").clicked() {
                        if !range_selected {
                            // 両端ともチェックを外した状態から始める。片方に既定値を入れて
                            // おくと、範囲指定を選んだだけで意図しない絞り込みが掛かる。
                            self.settings.facet_filter.size_preset = Some(FacetSizePreset::Range {
                                min: None,
                                max: None,
                            });
                        }
                        changed = true;
                    }
                    if let Some(FacetSizePreset::Range { mut min, mut max }) =
                        self.settings.facet_filter.size_preset
                    {
                        let mut range_changed = false;
                        range_changed |= draw_facet_size_value_row(
                            ui,
                            "下限",
                            "以上",
                            "main_min",
                            &mut min,
                            FacetSizeValue::new(100, FacetSizeUnit::KB),
                        );
                        range_changed |= draw_facet_size_value_row(
                            ui,
                            "上限",
                            "未満",
                            "main_max",
                            &mut max,
                            FacetSizeValue::new(1, FacetSizeUnit::MB),
                        );
                        if range_changed {
                            self.settings.facet_filter.size_preset =
                                Some(FacetSizePreset::Range { min, max }.sanitized());
                            changed = true;
                        }
                    }
                });
        suppress_menu_button_wheel_passthrough(ui.ctx(), &menu_response);
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
            if self.settings.facet_filter.uses_bookmark_state()
                && self.bookmark_presence_pending.is_some()
            {
                ui.label("ブックマーク状態を読み込み中…");
                ui.separator();
            }
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
            let Some(path) = self.facet_place_path_for_idx(idx) else {
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
                    | FacetItemKind::Audio
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
        let subfolder_expansion_pending = self.subfolder_expansion_busy();
        let subfolder_expansion_pending_tooltip = self.subfolder_expansion_pending_tooltip();
        let subfolder_expansion_available = subfolder_expansion_on
            || subfolder_expansion_pending
            || self.subfolder_expansion_available();
        // 一覧から落ちるのは絞り込み条件ではなくフォルダそのものの既定動作なので、
        // 利用者が手で条件を足す絞り込みバーではなくフォルダバーが定位置。
        let omitted_counts = self
            .settings
            .show_address_bar_omitted_entries
            .then(|| self.current_normal_folder_omitted_counts())
            .flatten();
        let mut open_duplicate_settings = false;
        let result = egui::TopBottomPanel::top("address_bar")
            .show(ctx, |ui| -> Option<AddressBarNav> {
                ui.add_space(3.0);
                let mut result = None;
                let mut pin_click = PinButtonClick::None;
                let mut favorite_click = FavoriteButtonClick::None;
                let mut tree_nav: Option<bool> = None;
                // ファイル名スタックのトグルクリックは closure 後に処理する (load_folder で
                // App ミュータブル借用が必要)。
                let mut stack_toggle = false;
                let mut subfolder_expansion_button_clicked = false;
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
                        let back_hover = if snapshot_active {
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
                                back_target.is_some() && !search_active && !snapshot_active,
                                egui::Button::new("←"),
                            )
                            .hover_tip(back_hover)
                            .clicked()
                        {
                            result = Some(AddressBarNav::HistoryBack);
                        }
                        let forward_hover = if snapshot_active {
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
                                forward_target.is_some() && !search_active && !snapshot_active,
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
                            let parent_hover = if drive_list_active {
                                "ドライブ一覧には親フォルダがありません".to_string()
                            } else if snapshot_active {
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
                                        "閲覧履歴へ戻る [BS]".to_string()
                                    }
                                    Some(AddressBarNav::Bookmarks) => {
                                        "ブックマークへ戻る [BS]".to_string()
                                    }
                                    Some(AddressBarNav::RatingViewBack) => {
                                        if self.rating_view_nav_stack.is_empty() {
                                            "レーティング一覧を閉じる [BS]".to_string()
                                        } else {
                                            "レーティング一覧へ戻る [BS]".to_string()
                                        }
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
                            if self.settings.show_location_drive_list
                                && ui.button("ドライブ一覧").clicked()
                            {
                                result = Some(AddressBarNav::DriveList(None));
                                ui.close();
                            }
                            if self.settings.show_location_reading_history
                                && ui.button("閲覧履歴").clicked()
                            {
                                result = Some(AddressBarNav::ReadingHistory);
                                ui.close();
                            }
                            if ui.button("ブックマーク").clicked() {
                                self.open_bookmark_browser();
                                ui.close();
                            }
                            if self.settings.show_location_rating {
                                ui.menu_button("レーティング", |ui| {
                                    for stars in 1..=5 {
                                        if ui
                                            .button(rating_view_menu_label(stars, rating_counts))
                                            .clicked()
                                        {
                                            self.enter_rating_view_from_menu(stars);
                                            ui.close();
                                        }
                                    }
                                });
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
                            if drew_quick_locations {
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
                            if self.settings.show_location_drive_roots {
                                let drives = crate::known_folders::available_drives();
                                if !drives.is_empty() {
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
                        if let Some(counts) = omitted_counts {
                            open_duplicate_settings |= draw_omitted_entries_chip(ui, counts);
                            if omitted_entries_chip_label(counts).is_some() {
                                ui.add_space(4.0);
                            }
                        }
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
                                    .color(ui.visuals().weak_text_color()),
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
                            let depth_choice =
                                crate::app::SubfolderExpansionDepthChoice::from_setting(
                                    self.settings.subfolder_expansion_max_depth,
                                );
                            let mut tooltip = if subfolder_expansion_pending {
                                subfolder_expansion_pending_tooltip
                                    .clone()
                                    .unwrap_or_else(|| "サブフォルダを走査中".to_string())
                            } else if subfolder_expansion_on {
                                "サブ展開を解除して元のフォルダへ戻る".to_string()
                            } else {
                                self.subfolder_expansion_action_tooltip()
                            };
                            if subfolder_expansion_pending || subfolder_expansion_on {
                                tooltip.push_str(&format!(
                                    "\n走査階層: {}",
                                    depth_choice.label()
                                ));
                            }

                            let resp = ui
                                .add_enabled(
                                    !subfolder_expansion_pending,
                                    egui::Button::selectable(
                                        subfolder_expansion_on || subfolder_expansion_pending,
                                        "サブ展開",
                                    ),
                                )
                                .hover_tip(tooltip);
                            if resp.clicked() {
                                subfolder_expansion_button_clicked = true;
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
                                let mut output = crate::ime_focus::show_singleline(
                                    ui,
                                    &mut self.address,
                                    None,
                                    |edit| edit.desired_width(f32::INFINITY),
                                );
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
                            self.fav_add_error = None;
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
                if subfolder_expansion_button_clicked {
                    self.activate_subfolder_expansion_button();
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
            .inner;
        if open_duplicate_settings {
            self.open_preferences_page(
                crate::ui_dialogs::preferences::PreferencesPage::DuplicateFiles,
            );
        }
        result
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

        // IME 変換確定の Enter は送信に使わず、確定後の通常 Enter だけを検索実行に使う。
        let enter_pressed = self.dialog_enter_pressed(ctx);
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
                let mut output = crate::ime_focus::show_singleline(
                    ui,
                    &mut self.search_query,
                    Some(&mut self.search_focus_request),
                    |edit| {
                        edit.hint_text(
                            r#"現在地のアイテムを名前やメタ情報で絞り込み (AND / -除外 / "…")"#,
                        )
                        .desired_width(320.0)
                        .min_size(egui::vec2(320.0, 20.0))
                    },
                );
                let _menu_changed = crate::ui_helpers::singleline_text_edit_context_menu(
                    ui,
                    &mut output,
                    &mut self.search_query,
                );
                let response = output.response;

                // フォーカス状態を追跡
                self.search_has_focus = response.has_focus();

                // Enter で検索実行 (IME 変換確定の Enter は除外)
                if response.lost_focus() && enter_pressed {
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
                            .color(ui.visuals().warn_fg_color),
                    );
                    if progress.total > 0 {
                        response.on_hover_text(format!(
                            "ヒット {} 件 / 確認済み {} 件 / 全 {} 件",
                            progress.matched, progress.done, progress.total
                        ));
                    }
                } else if let Some(ref filter) = self.search_filter {
                    ui.separator();
                    // 構造アイテム (Folder/ZIP/PDF) も含む可視マッチ全体を数える。
                    // Vec の長さだけを使い、数百万件を毎フレーム走査しない。
                    let (total, matched) = (self.items.len(), filter.len());
                    ui.label(
                        egui::RichText::new(format!("{matched}/{total} 件"))
                            .size(11.0)
                            .color(ui.visuals().weak_text_color()),
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
                            .color(ui.visuals().weak_text_color()),
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
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);

        let mut close_requested = false;
        let mut query_changed = false;

        egui::TopBottomPanel::top("favsearch_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label("コンテナ検索:");
                let mut output = crate::ime_focus::show_singleline(
                    ui,
                    &mut self.favsearch.query,
                    Some(&mut self.favsearch.focus_request),
                    |edit| {
                        edit.hint_text(
                            r#"フォルダ・ZIP・PDF をコンテナ名で探す (AND / -除外 / "…")"#,
                        )
                        .desired_width(320.0)
                        .min_size(egui::vec2(320.0, 20.0))
                    },
                );
                let menu_changed = crate::ui_helpers::singleline_text_edit_context_menu(
                    ui,
                    &mut output,
                    &mut self.favsearch.query,
                );
                let response = output.response;

                self.favsearch.has_focus = response.has_focus();

                // 入力が変わるたびに即座に検索を再実行 (小規模 DB 前提)
                if response.changed() || menu_changed {
                    query_changed = true;
                }
                // Enter で確定的に再実行 (IME 変換確定の Enter は除外)
                if response.lost_focus() && enter_pressed {
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
                            .color(ui.visuals().warn_fg_color),
                    );
                } else if self.favsearch.on_results_grid() {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("{} 件", self.favsearch.results_paths.len()))
                            .size(11.0)
                            .color(ui.visuals().weak_text_color()),
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

        let escape_pressed = self.dialog_escape_pressed(ctx);

        let mut close_requested = false;
        let mut query_changed = false;
        let mut filter_changed = false;
        let mut clicked_tag: Option<String> = None;

        egui::TopBottomPanel::top("tag_view_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("タグビュー:");
                let mut output = crate::ime_focus::show_singleline(
                    ui,
                    &mut self.tag_view.query,
                    Some(&mut self.tag_view.focus_request),
                    |edit| {
                        edit.hint_text("#タグ")
                            .desired_width(260.0)
                            .min_size(egui::vec2(260.0, 20.0))
                    },
                );
                let menu_changed = crate::ui_helpers::singleline_text_edit_context_menu(
                    ui,
                    &mut output,
                    &mut self.tag_view.query,
                );
                let response = output.response;

                self.tag_view.has_focus = response.has_focus();

                if response.changed() || menu_changed {
                    query_changed = true;
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
                            .color(ui.visuals().warn_fg_color),
                    );
                } else if let Some(msg) = self.tag_view.reject_message.as_ref() {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(msg)
                            .size(11.0)
                            .color(ui.visuals().error_fg_color),
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
                            .color(ui.visuals().weak_text_color()),
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
        overlay_layout: &crate::thumb_overlay_layout::ThumbnailOverlayLayout,
        touch_derived_pointer_activity: bool,
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
            let mode = self
                .settings
                .ring_shortcuts
                .right_drag_mode(crate::ring_shortcut::RightDragContext::Grid);
            if apply_grid_right_drag_start_selection(
                &mut self.selected,
                self.settings
                    .ring_shortcuts
                    .select_grid_item_on_right_drag_start,
                &mode,
                Some(idx),
            ) {
                self.grid_click_selection_anchor =
                    Some(GridClickSelectionAnchor::new(idx, self.items_generation));
                self.update_last_selected_image();
            }
            match mode {
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
        let tag_badge_target = self.grid_tag_badge_target(idx, overlay_layout);
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
            let display_order = if ctrl || shift {
                self.current_grid_order().to_vec()
            } else {
                Vec::new()
            };
            let allow_folders = self.subfolder_expansion_available();
            apply_grid_click_selection(
                self.settings.grid_click_selection_mode,
                &mut self.selected,
                &mut self.grid_click_selection_anchor,
                self.items_generation,
                &mut self.checked,
                &display_order,
                &self.items,
                allow_folders,
                idx,
                ctrl,
                shift,
            );
            self.update_last_selected_image();
        }
        if response.double_clicked() && self.items_are_bookmark_view {
            if let Some(row) = self.bookmark_browser_rows.get(idx).cloned() {
                self.open_bookmark_browser_row(ctx, &row);
            }
            return None;
        }
        if response.double_clicked() && self.guard_reading_history_open(idx) {
            // 閲覧履歴ビューから本を開く場合は、閉じたときに閲覧履歴へ戻れるよう予約する。
            self.note_reading_history_open(idx);
            // ファイル名スタックの集約グリッドでメディアセルをダブルクリックしたら、フラット読書
            // フルスクリーンへ (スタック/単独画像/動画を直接開く)。コンテナは false で通常ナビへ。
            if self.stack_try_open_from_grid(ctx, idx, true) {
                return nav;
            }
            #[cfg(windows)]
            if self.open_grid_container_in_detached_book_context(ctx, idx) {
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
                        let auto_fs = self.should_auto_fullscreen_grid_container(idx);
                        #[cfg(windows)]
                        if auto_fs && !self.park_active_detached_context_for_new_grid_open(ctx, idx)
                        {
                            return nav;
                        }
                        self.maybe_suppress_rating_filter_for_opened_container(idx);
                        self.maybe_suppress_facet_filter_for_opened_container(idx);
                        self.record_rating_view_nav_open(&p);
                        self.begin_smart_folder_drill(&p);
                        if auto_fs {
                            self.pending_auto_fs_open = true;
                        }
                        nav = Some(p);
                    }
                }
                Some(GridItem::ZipFile(p)) | Some(GridItem::PdfFile(p)) => {
                    // Folder 分岐とは global_search drill-in 判定が違うためここは別のまま。
                    let p = p.clone();
                    let auto_fs = self.should_auto_fullscreen_grid_container(idx);
                    #[cfg(windows)]
                    if auto_fs && !self.park_active_detached_context_for_new_grid_open(ctx, idx) {
                        return nav;
                    }
                    self.maybe_suppress_rating_filter_for_opened_container(idx);
                    self.maybe_suppress_facet_filter_for_opened_container(idx);
                    self.record_rating_view_nav_open(&p);
                    self.begin_smart_folder_drill(&p);
                    // 環境設定 ON なら、ページ一覧を経由せず 1 ページ目を即フルスクリーンで開く。
                    if auto_fs {
                        self.pending_auto_fs_open = true;
                    }
                    nav = Some(p);
                }
                Some(GridItem::Image(_))
                | Some(GridItem::Audio(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::PdfPage { .. })
                | Some(GridItem::Video(_)) => {
                    // 動画も画像と同じくフルスクリーン化 → VideoPlayer がインライン再生する。
                    // 外部プレイヤーで開きたい場合はフルスクリーン中の Shift+Enter または
                    // 右クリックメニューから (近日対応予定)。
                    // Phase 7.J: グリッドから明示的に開いたケースなので、
                    // 「一覧から開いたときだけ再生する」設定でも再生開始する。
                    // 同じ active/ParkedLive メディアなら park より先に前面化して open を消費する。
                    // (review-v2.3.0 追補4: double-click same-media)
                    #[cfg(windows)]
                    if !self.prepare_detached_context_for_grid_open(ctx, idx) {
                        return nav;
                    }
                    self.bump_input_seq_for_item("grid_double_click", idx);
                    if matches!(self.items.get(idx), Some(GridItem::Video(_))) {
                        // Prevent the second click of the grid double-click from
                        // reaching the newly-opened fullscreen video and toggling
                        // playback back to paused.
                        self.fs_primary_suppression.arm_pointer_stream();
                        self.fs_focus_regained_at = Some(std::time::Instant::now());
                    }
                    self.fs_open_intent_from_grid = true;
                    // P10-1 follow-up: grid_action_open は Enter (および双クリック) からも
                    // 呼ばれる。Enter 経路では `handle_fullscreen_root_key_input` が同フレームで
                    // Enter を `consume_key` で拾って即 close する事故を防ぐためのガード。
                    // ダブルクリック経路では Enter event がそもそも無いので no-op。
                    self.fs_suppress_enter_close_until_release = true;
                    self.open_fullscreen(idx, crate::app::HistoryTrigger::UserChosen);
                }
                Some(GridItem::ConvertibleArchive { path, .. }) => {
                    let pf = path.clone();
                    self.begin_smart_folder_drill(&pf);
                    let auto_fs = self.settings.effective_auto_fullscreen_zip_pdf();
                    let search_rollback = if self.favsearch.active
                        || self.tag_view.active
                        || self.rating_view_nav_context_active()
                    {
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
                    self.record_rating_view_nav_open(&pf);
                    self.maybe_suppress_rating_filter_for_opened_container(idx);
                    self.maybe_suppress_facet_filter_for_opened_container(idx);
                    let open_outcome =
                        self.load_folder_or_convert_archive_with_auto_fullscreen(pf, auto_fs);
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
            && !self.selection_info_bar_contains_pointer(ctx)
            && !self.mouse_ring_context_menu_suppressed(ctx)
        {
            apply_grid_secondary_selection(
                self.settings.grid_click_selection_mode,
                &mut self.selected,
                &mut self.grid_click_selection_anchor,
                self.items_generation,
                &mut self.checked,
                idx,
            );
            self.update_last_selected_image();
            self.context_menu_idx = Some(idx);
            self.context_menu_pos = ctx.input(|i| i.pointer.interact_pos().unwrap_or_default());
        }

        // ── native ファイル D&D の開始検出 (docs/file-drag-drop-design.md §5.4) ──
        // primary (左) ボタンのドラッグだけを起点にする。native drag 直後の
        // 1 フレームは抑止 (幽霊ドラッグ防止の保険、§6.1)。
        if native_grid_drag_start_allowed(
            self.items_are_drive_list,
            self.native_drag_just_finished,
            touch_derived_pointer_activity,
        ) && response.drag_started_by(egui::PointerButton::Primary)
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
        idx: usize,
        overlay_layout: &crate::thumb_overlay_layout::ThumbnailOverlayLayout,
    ) -> Option<(String, egui::Rect)> {
        if self.items_are_drive_list {
            return None;
        }
        let tags = self.cell_tag_list(idx);
        let tag_name = crate::app::primary_grid_tag_for_badge(tags)?.to_owned();
        let badge_rect = crate::app::grid_tag_badge_hit_rect(overlay_layout)?;
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

    fn handle_grid_background_primary_click(
        &mut self,
        ctx: &egui::Context,
        rect: egui::Rect,
        hit_cell: bool,
    ) {
        if self.settings.grid_click_selection_mode.normalized() != GridClickSelectionMode::Explorer
            || hit_cell
            || self.any_dialog_open()
            || self.selection_info_bar_contains_pointer(ctx)
            || !ctx.input(|i| {
                i.pointer.primary_clicked()
                    && i.pointer
                        .interact_pos()
                        .or_else(|| i.pointer.latest_pos())
                        .is_some_and(|pos| rect.contains(pos))
            })
        {
            return;
        }
        self.folder_pane.set_focus_grid();
        clear_grid_selection_for_background_click(
            self.settings.grid_click_selection_mode,
            &mut self.selected,
            &mut self.grid_click_selection_anchor,
            &mut self.checked,
        );
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
            || self.selection_info_bar_contains_pointer(ctx)
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
        if self.selection_info_bar_contains_pointer(ctx) {
            return;
        }
        let pos = ctx.input(|i| i.pointer.interact_pos().unwrap_or_default());
        if self.context_menu_idx.is_some() {
            return;
        }
        if let Some(idx) = target_idx
            && idx < self.items.len()
            && !self.items_are_drive_list
        {
            apply_grid_secondary_selection(
                self.settings.grid_click_selection_mode,
                &mut self.selected,
                &mut self.grid_click_selection_anchor,
                self.items_generation,
                &mut self.checked,
                idx,
            );
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
        let horizontal_source_rect = ui.available_rect_before_wrap();
        let avail_w = horizontal_source_rect.width().max(1.0);
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
        // 横方向の要否は egui の丸め後 content_size に任せず、アプリが所有する全列幅を
        // 物理ピクセル右端へ変換して決める。横バーが出ると縦 viewport が縮み、縦バー gutter が
        // 新たに必要になる場合があるため、false -> true の単調な状態を最大 3 回で収束させる。
        let pixels_per_point = ui.ctx().pixels_per_point();
        let DetailsListLayoutResolution {
            layout,
            horizontal_policy,
            hbar,
            viewport_h_est,
            needs_vscroll,
            gutter,
        } = resolve_details_list_layout(
            horizontal_source_rect,
            avail_h,
            ui.spacing().item_spacing.y,
            natural_h,
            pixels_per_point,
            &self.settings,
        );
        let details_scroll_style = details_scroll_style();
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
        let mut vertical_scroll_debug = None;
        let primary_click_pos = ctx.input(|i| {
            i.pointer
                .primary_clicked()
                .then(|| i.pointer.interact_pos().or_else(|| i.pointer.latest_pos()))
                .flatten()
        });
        let mut primary_click_hit_cell = false;
        let previous_scroll_style = ui.spacing().scroll;
        ui.spacing_mut().scroll = details_scroll_style;
        let horizontal_output = configured_details_horizontal_scroll_area(horizontal_policy)
            .id_salt("details_list_horizontal")
            .show(ui, |ui| {
                // 外側コンテンツ幅 = pane + gutter。pane より広くしておくことで内側縦スクロールの
                // gutter が pane の右外に収まり、ヘッダ・行の列が揃う。
                ui.set_min_width(horizontal_policy.scroll_extent);
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
                // このフレームで解決する要求だけを描画前に所有する。下でリング / ジェスチャが
                // 新しく積んだ要求は次フレームまで残し、同フレーム末尾で消さない。
                let pending_scroll =
                    take_grid_scroll_for_current_layout(&mut self.pending_grid_scroll);
                self.scroll_offset_y =
                    resolve_grid_scroll_offset(self.scroll_offset_y, max_offset, pending_scroll);

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

                            primary_click_hit_cell |=
                                primary_click_pos.is_some_and(|pos| row_rect.contains(pos));
                            let no_thumbnail_overlays =
                                crate::thumb_overlay_layout::ThumbnailOverlayLayout::default();
                            if let Some(n) = self.handle_cell_interaction(
                                ui,
                                ctx,
                                row_rect,
                                idx,
                                &no_thumbnail_overlays,
                                false,
                            ) {
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
                                DetailsColumnSet::Details,
                                false,
                                grid_row_text_tooltips_enabled(self.settings.grid_view_mode),
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
                vertical_scroll_debug = Some(DetailsVerticalScrollDebug {
                    inner_rect: scroll_output.inner_rect,
                    content_size: scroll_output.content_size,
                    state: scroll_output.state,
                });
            });
        ui.spacing_mut().scroll = previous_scroll_style;
        log_details_layout_debug(
            ctx,
            DetailsLayoutDebugSample {
                surface: "details_list",
                source_rect: horizontal_source_rect,
                avail_w,
                avail_h,
                row_count,
                natural_h,
                viewport_h_est,
                horizontal_policy,
                predicted_hbar: hbar,
                predicted_vscroll: needs_vscroll,
                gutter,
                fixed_columns_w: layout.columns_w - layout.name_w,
                name_w: layout.name_w,
                pane_w: layout.pane_w,
                layout_extent: layout.extent,
                requested_extent: horizontal_policy.scroll_extent,
                column_set: DetailsColumnSet::Details,
                outer_inner_rect: horizontal_output.inner_rect,
                outer_content_size: horizontal_output.content_size,
                outer_state: horizontal_output.state,
                vertical: vertical_scroll_debug,
                settings: &self.settings,
            },
        );

        self.start_grid_background_mouse_ring_flick_if_pressed(ctx, body_inner_rect);
        self.update_grid_mouse_ring_flick(ctx);
        self.handle_grid_background_primary_click(ctx, body_inner_rect, primary_click_hit_cell);

        let bg_right_clicked = ui.rect_contains_pointer(body_inner_rect)
            && ctx.input(|i| i.pointer.secondary_clicked());
        if bg_right_clicked
            && self.context_menu_idx.is_none()
            && !self.selection_info_bar_contains_pointer(ctx)
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
        self.draw_gamepad_ring_overlay(ui, full_rect, crate::app::ActionSurface::MainWindow);
        self.draw_gamepad_picker_overlay(ui, full_rect, crate::app::ActionSurface::MainWindow);
        self.draw_gamepad_favorite_picker_overlay(
            ui,
            full_rect,
            crate::app::ActionSurface::MainWindow,
        );
        self.draw_gamepad_location_picker_overlay(
            ui,
            full_rect,
            crate::app::ActionSurface::MainWindow,
        );
        self.draw_feedback_toast(ui, full_rect, ctx, crate::app::ActionSurface::MainWindow);
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

    /// 詳細表示「最終閲覧」列 (= 閲覧履歴ビューでは更新日時列を転用) の文字列。
    pub(crate) fn reading_history_last_read_for_idx(&self, idx: usize) -> Option<String> {
        let entry = self.reading_history_entry_for_idx(idx)?;
        reading_history_last_read_text(entry, self.settings.details_timestamp_show_seconds)
    }

    /// 詳細表示「閲覧位置」列 (= 閲覧履歴ビューでは状態列を転用) の文字列。
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
            lines.push(format!("閲覧位置 {progress}"));
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
        // 閲覧履歴は複数フォルダ / ドライブの項目が混在するため、場所 (フルパス) を
        // 先頭に出して同名の項目を判別できるようにする。
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

    fn draw_bookmark_view_overlay(
        &self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        idx: usize,
        overlay_layout: &crate::thumb_overlay_layout::ThumbnailOverlayLayout,
    ) {
        let Some(row) = self.bookmark_view_row(idx) else {
            return;
        };
        if let Some(placement) = overlay_layout.top_left.bookmark_time.as_ref() {
            crate::ui_helpers::draw_overlay_bookmark_time_badge(
                ui.painter(),
                placement,
                row.missing,
            );
        }
        if let Some(title) = row.title() {
            let font = crate::ui_fonts::user_text_font(13.0);
            let max_text_width = (rect.width() - 28.0).max(0.0);
            if max_text_width >= 8.0 {
                let initial = crate::ui_helpers::truncate_name(title, 36);
                let mut galley = ui.painter().layout_no_wrap(
                    initial.clone(),
                    font.clone(),
                    egui::Color32::WHITE,
                );
                if galley.size().x > max_text_width {
                    let chars: Vec<char> = initial.chars().collect();
                    for take in (1..chars.len()).rev() {
                        let candidate = chars[..take].iter().collect::<String>() + "…";
                        let candidate_galley = ui.painter().layout_no_wrap(
                            candidate,
                            font.clone(),
                            egui::Color32::WHITE,
                        );
                        if candidate_galley.size().x <= max_text_width {
                            galley = candidate_galley;
                            break;
                        }
                    }
                }
                if galley.size().x <= max_text_width {
                    let centered_rect = egui::Rect::from_center_size(
                        rect.center(),
                        galley.size() + egui::vec2(14.0, 8.0),
                    );
                    let lane_bottom = overlay_layout.top_left.lane_bottom().unwrap_or(rect.min.y);
                    let min_title_y = lane_bottom + 4.0;
                    let plate_rect = centered_rect.translate(egui::vec2(
                        0.0,
                        (min_title_y - centered_rect.min.y).max(0.0),
                    ));
                    if plate_rect.max.y <= rect.max.y - 4.0 {
                        ui.painter().rect_filled(
                            plate_rect,
                            4.0,
                            egui::Color32::from_black_alpha(190),
                        );
                        ui.painter().galley(
                            plate_rect.center() - galley.size() * 0.5,
                            galley,
                            egui::Color32::WHITE,
                        );
                    }
                }
            }
        }
        let source = row.source_path().display().to_string();
        let position = row.position_label();
        let registered = format_details_mtime(
            row.created_at_ms.div_euclid(1000),
            self.settings.details_timestamp_show_seconds,
        );
        let response = ui.interact(
            rect,
            ui.id().with(("bookmark_view_tooltip", idx)),
            egui::Sense::hover(),
        );
        response.on_hover_ui_at_pointer(|ui| {
            if let Some(title) = row.title() {
                ui.label(format!("名前 {title}"));
            }
            ui.label(format!("場所 {source}"));
            ui.label(format!("位置 {position}"));
            ui.label(format!("登録日時 {registered}"));
        });
    }

    /// 現在のビューでその列に出す名前。ブックマーク / 閲覧履歴では同じ列が別の意味を
    /// 持つので、一覧ヘッダ (`details_header_title`) と列の表示切替メニュー
    /// (`draw_details_column_context_menu`) の**両方**がここを通る。片方だけ更新すると
    /// 「一覧には位置と出ているのにメニューには状態しか無い」状態になり、ユーザーが
    /// 目的の列を見つけられなくなる (実害あり)。
    fn details_column_view_title(&self, col: DetailsColumn) -> &'static str {
        if self.items_are_bookmark_view {
            match col {
                DetailsColumn::Modified => "登録日時",
                DetailsColumn::State => "位置",
                _ => col.title(),
            }
        } else if self.items_are_reading_history_view {
            match col {
                DetailsColumn::Modified => "最終閲覧",
                DetailsColumn::State => "閲覧位置",
                _ => col.title(),
            }
        } else {
            col.title()
        }
    }

    fn details_header_title(
        &self,
        col: DetailsColumn,
        book_sort_locked: bool,
        show_sort_indicator: bool,
    ) -> String {
        let mut base_title = self.details_column_view_title(col).to_string();
        if col.is_lazy()
            && matches!(
                self.details_image_dims_state,
                LazyColumnState::Loading { .. }
                    | LazyColumnState::Reconciling { .. }
                    | LazyColumnState::NotRequested
            )
        {
            base_title.push_str(" ...");
        }
        let sorted = show_sort_indicator
            && !book_sort_locked
            && col
                .sort_key()
                .is_some_and(|sort_key| self.settings.details_sort_key == sort_key);
        if sorted {
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
        }
    }

    fn interact_details_header_drag(
        &mut self,
        ui: &mut egui::Ui,
        column_set: DetailsColumnSet,
        column: DetailsColumn,
        column_rect: egui::Rect,
        columns: &[(DetailsColumn, egui::Rect)],
    ) -> egui::Response {
        let mut header_hit = column_rect;
        // Keep the rightmost 8 px exclusive to the existing resize interaction.
        if header_hit.width() > 16.0 {
            header_hit.max.x -= 8.0;
        }
        let response = ui.interact(
            header_hit,
            ui.id().with(("details_header", column_set, column)),
            egui::Sense::click_and_drag(),
        );
        let drag_state_id = ui.id().with(("details_header_drag_state", column_set));
        if response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(pos) = ui.ctx().input(|input| {
                input
                    .pointer
                    .interact_pos()
                    .or_else(|| input.pointer.latest_pos())
            })
        {
            let start = ui
                .ctx()
                .input(|input| input.pointer.press_origin().unwrap_or(pos));
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    drag_state_id,
                    Some(DetailsHeaderDrag {
                        column,
                        start,
                        latest: pos,
                    }),
                )
            });
        }
        if response.dragged_by(egui::PointerButton::Primary)
            && let Some(pos) = ui.ctx().input(|input| {
                input
                    .pointer
                    .interact_pos()
                    .or_else(|| input.pointer.latest_pos())
            })
        {
            ui.ctx().data_mut(|data| {
                let mut drag = data
                    .get_temp::<Option<DetailsHeaderDrag>>(drag_state_id)
                    .flatten()
                    .unwrap_or(DetailsHeaderDrag {
                        column,
                        start: pos,
                        latest: pos,
                    });
                if drag.column == column {
                    drag.latest = pos;
                }
                data.insert_temp(drag_state_id, Some(drag));
            });
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) {
            let stopped_drag = ui
                .ctx()
                .data_mut(|data| data.remove_temp::<Option<DetailsHeaderDrag>>(drag_state_id))
                .flatten();
            if let Some(mut drag) = stopped_drag {
                if let Some(pos) = ui.ctx().input(|input| input.pointer.latest_pos()) {
                    drag.latest = pos;
                }
                if finish_details_header_drag(&mut self.settings, column_set, columns, drag, 12.0) {
                    crate::logger::log(format!(
                        "details header reorder ({column_set:?}): {:?} -> x {:.1}",
                        drag.column.id(),
                        drag.latest.x
                    ));
                    self.settings.save();
                    ui.ctx().request_repaint();
                }
            }
        }
        response
    }

    fn draw_details_header_static(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        column_set: DetailsColumnSet,
    ) {
        let bg = ui.visuals().extreme_bg_color;
        let stroke_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let text_color = ui.visuals().strong_text_color();
        let book_sort_locked = self.page_order_locked_for_current_view();
        self.advance_details_best_fit_job(ui, book_sort_locked);
        ui.painter().rect_filled(rect, 0.0, bg);
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            egui::Stroke::new(1.0, stroke_color),
        );
        let columns = details_column_rects_for_columns(rect, &self.settings, column_set);
        let current_name_width = columns
            .iter()
            .find_map(|(column, rect)| (*column == DetailsColumn::Name).then_some(rect.width()))
            .unwrap_or_else(|| DetailsColumn::Name.default_width());
        for (col, col_rect) in columns.iter().copied() {
            let response =
                self.interact_details_header_drag(ui, column_set, col, col_rect, &columns);
            draw_details_text(
                ui,
                col_rect,
                &self.details_header_title(col, book_sort_locked, false),
                egui::Align2::LEFT_CENTER,
                text_color,
                true,
            );
            ui.painter().line_segment(
                [col_rect.right_top(), col_rect.right_bottom()],
                egui::Stroke::new(1.0, stroke_color),
            );
            self.draw_details_column_resize_handle(ui, col, col_rect, column_set, book_sort_locked);
            let origin = if self.settings.grid_view_mode == GridViewMode::Details {
                DetailsColumnMenuOrigin::DetailsSelectionBar
            } else {
                DetailsColumnMenuOrigin::ThumbnailSelectionBar
            };
            self.show_details_column_context_menu(
                ui,
                &response,
                column_set,
                origin,
                current_name_width,
            );
        }
    }

    fn details_best_fit_job_id() -> egui::Id {
        egui::Id::new("details_best_fit_job")
    }

    fn details_best_fit_frame_budget_id() -> egui::Id {
        egui::Id::new("details_best_fit_frame_budget")
    }

    fn claim_details_best_fit_frame_budget(ctx: &egui::Context) -> bool {
        let frame_nr = ctx.cumulative_frame_nr();
        ctx.data_mut(|data| {
            let id = Self::details_best_fit_frame_budget_id();
            let mut budget = data
                .get_temp::<DetailsBestFitFrameBudget>(id)
                .unwrap_or_default();
            let claimed = budget.claim(frame_nr);
            data.insert_temp(id, budget);
            claimed
        })
    }

    fn cancel_details_best_fit_job(ctx: &egui::Context) {
        ctx.data_mut(|data| {
            data.remove_temp::<Option<DetailsBestFitJob>>(Self::details_best_fit_job_id());
        });
    }

    fn details_best_fit_measure(ui: &egui::Ui, text: String, font: egui::FontId) -> f32 {
        ui.painter()
            .layout_no_wrap(text, font, egui::Color32::WHITE)
            .size()
            .x
    }

    fn details_best_fit_job_key(
        &self,
        ui: &egui::Ui,
        column: DetailsColumn,
        column_set: DetailsColumnSet,
        book_sort_locked: bool,
    ) -> DetailsBestFitJobKey {
        let view_kind = if self.items_are_bookmark_view {
            DetailsBestFitViewKind::Bookmark
        } else if self.items_are_reading_history_view {
            DetailsBestFitViewKind::ReadingHistory
        } else {
            DetailsBestFitViewKind::Normal
        };
        let revisions = self.details_cell_content_revisions;
        let content_revision = match column {
            DetailsColumn::Tags => revisions.tags,
            DetailsColumn::PageCount => revisions.page_count,
            DetailsColumn::Created => revisions.created_at,
            DetailsColumn::ImageDimensions => revisions.image_dims,
            DetailsColumn::VideoDuration
            | DetailsColumn::VideoDimensions
            | DetailsColumn::VideoCodec => revisions.video_meta,
            _ => 0,
        };
        DetailsBestFitJobKey {
            items_generation: self.items_generation,
            order_revision: self.details_order_revision,
            content_revision,
            total_rows: self.current_grid_order().len(),
            column,
            column_set,
            view_kind,
            book_sort_locked,
            header_title: self.details_header_title(column, book_sort_locked, true),
            button_font: egui::TextStyle::Button.resolve(ui.style()),
            body_font: egui::TextStyle::Body.resolve(ui.style()),
            pixels_per_point_bits: ui.ctx().pixels_per_point().to_bits(),
            ui_font: self.settings.ui_font.clone(),
        }
    }

    fn details_best_fit_seed_width(
        &self,
        ui: &egui::Ui,
        key: &DetailsBestFitJobKey,
    ) -> (f32, bool) {
        let mut widest =
            Self::details_best_fit_measure(ui, key.header_title.clone(), key.button_font.clone());
        let needs_dynamic_rows = match key.column {
            DetailsColumn::Preview => {
                widest = widest.max(key.column.default_width() - 12.0);
                false
            }
            // These columns have a small bounded vocabulary. Avoid calling the regular row
            // formatter for every item because Rating may perform a cache-miss DB lookup.
            DetailsColumn::Rating => {
                widest = widest.max(Self::details_best_fit_measure(
                    ui,
                    DETAILS_RATING_BEST_FIT_SEED.to_owned(),
                    key.body_font.clone(),
                ));
                false
            }
            DetailsColumn::State => {
                for sample in [DETAILS_STATE_BEST_FIT_SEED, "9999 / 9999", "未読"] {
                    widest = widest.max(Self::details_best_fit_measure(
                        ui,
                        sample.to_owned(),
                        key.body_font.clone(),
                    ));
                }
                !matches!(key.view_kind, DetailsBestFitViewKind::Normal)
            }
            _ => true,
        };
        (widest, needs_dynamic_rows)
    }

    fn apply_details_best_fit_width(
        &mut self,
        ui: &egui::Ui,
        column_set: DetailsColumnSet,
        column: DetailsColumn,
        widest: f32,
    ) {
        let width = (widest + DETAILS_BEST_FIT_HORIZONTAL_PADDING)
            .ceil()
            .clamp(column.min_width(), DETAILS_BEST_FIT_MAX_WIDTH);
        let changed = if column == DetailsColumn::Name {
            set_details_name_width(&mut self.settings, column_set, width)
        } else {
            set_details_column_width(&mut self.settings, column_set, column, width)
        };
        if changed {
            crate::logger::log(format!(
                "details column best-fit: {:?} {:?} -> {:.1}",
                column_set,
                column.id(),
                width
            ));
            self.settings.save();
            ui.ctx().request_repaint();
        }
    }

    fn finish_details_best_fit_job(
        &mut self,
        ui: &egui::Ui,
        job: DetailsBestFitJob,
        saturated: bool,
    ) {
        if crate::perf::is_enabled() {
            crate::perf::event(
                "details",
                "column_best_fit",
                None,
                job.key.items_generation,
                &[
                    ("rows", serde_json::Value::from(job.key.total_rows)),
                    ("measured_rows", serde_json::Value::from(job.measured_rows)),
                    ("batches", serde_json::Value::from(job.batches)),
                    ("saturated", serde_json::Value::from(saturated)),
                    (
                        "ms",
                        serde_json::Value::from(job.started.elapsed().as_secs_f64() * 1000.0),
                    ),
                ],
            );
        }
        self.apply_details_best_fit_width(ui, job.key.column_set, job.key.column, job.widest);
    }

    fn advance_details_best_fit_job(&mut self, ui: &egui::Ui, book_sort_locked: bool) {
        let job_id = Self::details_best_fit_job_id();
        let Some(mut job) = ui.ctx().data_mut(|data| {
            data.remove_temp::<Option<DetailsBestFitJob>>(job_id)
                .flatten()
        }) else {
            return;
        };

        if !details_visible_columns(&self.settings, job.key.column_set).contains(&job.key.column) {
            return;
        }
        let current_key =
            self.details_best_fit_job_key(ui, job.key.column, job.key.column_set, book_sort_locked);
        if job.key == current_key
            && job.next_row < job.key.total_rows
            && !Self::claim_details_best_fit_frame_budget(ui.ctx())
        {
            ui.ctx()
                .data_mut(|data| data.insert_temp(job_id, Some(job)));
            ui.ctx().request_repaint();
            return;
        }
        let batch = job.next_batch(&current_key);
        match batch {
            DetailsBestFitBatch::Stale => {
                let restart_for_content_change =
                    job.key.has_restartable_content_update(&current_key);
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "details",
                        "column_best_fit_cancel",
                        None,
                        self.items_generation,
                        &[
                            (
                                "started_generation",
                                serde_json::Value::from(job.key.items_generation),
                            ),
                            (
                                "started_order_revision",
                                serde_json::Value::from(job.key.order_revision),
                            ),
                            (
                                "current_order_revision",
                                serde_json::Value::from(self.details_order_revision),
                            ),
                            (
                                "started_content_revision",
                                serde_json::Value::from(job.key.content_revision),
                            ),
                            (
                                "current_content_revision",
                                serde_json::Value::from(current_key.content_revision),
                            ),
                        ],
                    );
                }
                if restart_for_content_change {
                    let (widest, needs_dynamic_rows) =
                        self.details_best_fit_seed_width(ui, &current_key);
                    if needs_dynamic_rows {
                        let restarted = DetailsBestFitJob {
                            key: current_key,
                            next_row: 0,
                            widest,
                            started: std::time::Instant::now(),
                            measured_rows: 0,
                            batches: 0,
                        };
                        ui.ctx()
                            .data_mut(|data| data.insert_temp(job_id, Some(restarted)));
                        ui.ctx().request_repaint();
                    }
                }
            }
            DetailsBestFitBatch::Complete => {
                self.finish_details_best_fit_job(ui, job, false);
            }
            DetailsBestFitBatch::Measure {
                range,
                completes_job,
            } => {
                let batch_started = std::time::Instant::now();
                // current_grid_order の全体 clone は避け、今 frame に測る最大 192 idx だけを
                // 借用解除用にコピーする。order revision が変われば次 frame で job を破棄する。
                let indices = self.current_grid_order()[range].to_vec();
                for idx in indices.iter().copied() {
                    let Some(item) = self.items.get(idx).cloned() else {
                        continue;
                    };
                    job.measured_rows += 1;
                    let meta = self.image_metas.get(idx).copied().flatten();
                    let text = self.details_cell_text(idx, &item, meta, job.key.column);
                    if !text.is_empty() {
                        job.widest = job.widest.max(Self::details_best_fit_measure(
                            ui,
                            text,
                            job.key.body_font.clone(),
                        ));
                    }
                }
                let saturated =
                    job.widest + DETAILS_BEST_FIT_HORIZONTAL_PADDING >= DETAILS_BEST_FIT_MAX_WIDTH;
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "details",
                        "column_best_fit_batch",
                        None,
                        job.key.items_generation,
                        &[
                            ("batch_rows", serde_json::Value::from(indices.len())),
                            ("measured_rows", serde_json::Value::from(job.measured_rows)),
                            ("rows", serde_json::Value::from(job.key.total_rows)),
                            (
                                "ms",
                                serde_json::Value::from(
                                    batch_started.elapsed().as_secs_f64() * 1000.0,
                                ),
                            ),
                        ],
                    );
                }
                if completes_job || saturated {
                    self.finish_details_best_fit_job(ui, job, saturated);
                } else {
                    ui.ctx()
                        .data_mut(|data| data.insert_temp(job_id, Some(job)));
                    ui.ctx().request_repaint();
                }
            }
        }
    }

    fn start_details_best_fit_job(
        &mut self,
        ui: &egui::Ui,
        column: DetailsColumn,
        column_set: DetailsColumnSet,
        book_sort_locked: bool,
    ) {
        let key = self.details_best_fit_job_key(ui, column, column_set, book_sort_locked);
        let (widest, needs_dynamic_rows) = self.details_best_fit_seed_width(ui, &key);
        if !needs_dynamic_rows {
            Self::cancel_details_best_fit_job(ui.ctx());
            self.apply_details_best_fit_width(ui, column_set, column, widest);
            return;
        }

        let job = DetailsBestFitJob {
            key,
            next_row: 0,
            widest,
            started: std::time::Instant::now(),
            measured_rows: 0,
            batches: 0,
        };
        ui.ctx()
            .data_mut(|data| data.insert_temp(Self::details_best_fit_job_id(), Some(job)));
        // クリック frame で最初の batch を測る。小規模一覧はこの 1 回で exact 幅になる。
        self.advance_details_best_fit_job(ui, book_sort_locked);
    }

    fn draw_details_column_resize_handle(
        &mut self,
        ui: &mut egui::Ui,
        column: DetailsColumn,
        column_rect: egui::Rect,
        column_set: DetailsColumnSet,
        book_sort_locked: bool,
    ) {
        // つかみ代は列の内側へ置き、隣のヘッダ操作と重ねない。詳細一覧ではソートと
        // 並べ替え、下部情報バーでは右クリックメニューから境界操作を分離する。
        let resize_rect = egui::Rect::from_min_max(
            egui::pos2(column_rect.right() - 8.0, column_rect.top()),
            egui::pos2(column_rect.right(), column_rect.bottom()),
        );
        let resize_response = ui
            .interact(
                resize_rect,
                ui.id().with(("details_header_resize", column_set, column)),
                egui::Sense::click_and_drag(),
            )
            .on_hover_text("ドラッグで幅変更 / ダブルクリックで内容に合わせる");
        if resize_response.hovered() || resize_response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            ui.painter().line_segment(
                [column_rect.right_top(), column_rect.right_bottom()],
                egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
            );
        }
        if resize_response.double_clicked() {
            self.start_details_best_fit_job(ui, column, column_set, book_sort_locked);
        } else if resize_response.dragged() {
            // 手動幅変更はユーザーの最新意思。進行中の測定結果で後から上書きしない。
            Self::cancel_details_best_fit_job(ui.ctx());
            let changed = if column == DetailsColumn::Name {
                let effective_width = column_rect.width() + resize_response.drag_delta().x;
                let stored_width = details_stored_name_width_from_effective(
                    &self.settings,
                    column_set,
                    effective_width,
                );
                set_details_name_width(&mut self.settings, column_set, stored_width)
            } else {
                let current = details_column_width(&self.settings, column_set, column);
                set_details_column_width(
                    &mut self.settings,
                    column_set,
                    column,
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

    fn draw_details_header(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let bg = ui.visuals().extreme_bg_color;
        let stroke_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let text_color = ui.visuals().strong_text_color();
        let hover_bg = ui.visuals().widgets.hovered.bg_fill;
        let book_sort_locked = self.page_order_locked_for_current_view();
        self.advance_details_best_fit_job(ui, book_sort_locked);
        ui.painter().rect_filled(rect, 0.0, bg);
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            egui::Stroke::new(1.0, stroke_color),
        );

        let columns =
            details_column_rects_for_columns(rect, &self.settings, DetailsColumnSet::Details);
        for (col, col_rect) in columns.iter().copied() {
            let response = self.interact_details_header_drag(
                ui,
                DetailsColumnSet::Details,
                col,
                col_rect,
                &columns,
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
            let title = self.details_header_title(col, book_sort_locked, true);
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
            self.draw_details_column_resize_handle(
                ui,
                col,
                col_rect,
                DetailsColumnSet::Details,
                book_sort_locked,
            );
            let response = if sort_enabled {
                response.hover_tip("クリックで 昇順 → 降順 → ソートなし")
            } else if book_sort_locked && sort_key.is_some() {
                response.hover_tip(
                    "本として表示中や閲覧履歴では、並び順が固定されます（一覧の並べ替えは使えません）。",
                )
            } else if sort_key.is_none() {
                response.hover_tip("サムネイルプレビュー")
            } else {
                response.hover_tip("詳細情報の読み込み完了後に並べ替えできます")
            };
            self.show_details_column_context_menu(
                ui,
                &response,
                DetailsColumnSet::Details,
                DetailsColumnMenuOrigin::DetailsListHeader,
                self.last_details_name_width,
            );
        }
    }

    fn details_column_context_menu_tracker_id() -> egui::Id {
        egui::Id::new("details_column_context_menu_tracker")
    }

    pub(crate) fn details_column_context_menu_is_open(ctx: &egui::Context) -> bool {
        let popup_id = ctx
            .data(|data| data.get_temp::<egui::Id>(Self::details_column_context_menu_tracker_id()));
        popup_id.is_some_and(|popup_id| egui::Popup::is_id_open(ctx, popup_id))
    }

    fn show_details_column_context_menu(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        column_set: DetailsColumnSet,
        origin: DetailsColumnMenuOrigin,
        current_name_width: f32,
    ) {
        let popup_id = show_sticky_context_menu(response, |ui| {
            self.draw_details_column_context_menu(ui, column_set, origin, current_name_width);
        });
        if egui::Popup::is_id_open(ui.ctx(), popup_id) {
            ui.ctx().data_mut(|data| {
                data.insert_temp(Self::details_column_context_menu_tracker_id(), popup_id);
            });
        }
        // 詳細一覧ヘッダはグローバルな grid wheel 処理より後で描かれるため、
        // app.rs 側はこの popup の wheel だけここまで保持する。ScrollArea が処理した後、
        // raw input を消費して背面の詳細行へ通さない。
        suppress_menu_button_wheel_passthrough(ui.ctx(), response);
    }

    fn draw_details_column_context_menu(
        &mut self,
        ui: &mut egui::Ui,
        column_set: DetailsColumnSet,
        origin: DetailsColumnMenuOrigin,
        current_name_width: f32,
    ) {
        let columns_heading =
            details_column_menu_heading(self.settings.details_selection_bar_mode, origin);
        let show_mode_toggle = origin == DetailsColumnMenuOrigin::DetailsSelectionBar;
        let mut dedicated = self.settings.details_selection_bar_mode.normalized()
            == DetailsSelectionBarMode::Dedicated;
        let modified_label = self.details_column_view_title(DetailsColumn::Modified);
        let state_label = self.details_column_view_title(DetailsColumn::State);
        let mut menu_state = DetailsColumnMenuState::from_settings(&self.settings, column_set);
        let mut toggle_requested = false;
        let max_height =
            (ui.ctx().content_rect().height() - DETAILS_COLUMN_MENU_SCREEN_MARGIN).max(1.0);
        draw_details_column_menu_with_header(ui, max_height, |pane, ui| match pane {
            DetailsColumnMenuPane::Columns => {
                if show_mode_toggle {
                    toggle_requested |= ui
                        .checkbox(&mut dedicated, "以下を専用の設定にする")
                        .changed();
                    ui.separator();
                }
                ui.label(egui::RichText::new(columns_heading).strong());
                ui.separator();
                let mut name_visible = true;
                ui.add_enabled(false, egui::Checkbox::new(&mut name_visible, "名前"));

                // 名前列の幅: 既定は残り幅へ自動調整。OFF にすると現在の幅で固定し、横スクロールで
                // 全列を確認できる。境界ドラッグでも自動的に固定幅へ切り替わる。
                ui.checkbox(&mut menu_state.name_width_auto, "名前の幅を自動調整")
                    .on_hover_text(
                        "OFF にすると現在の名前列幅で固定します (境界ドラッグでも固定されます)",
                    );

                // 下部バーはプレビューを描かない (`DetailsColumnSet::includes`)。専用設定では
                // この列を ON にしても行き先が無いので、押せない状態にして理由を出す。
                // 一覧と同じ設定のときは一覧側のプレビューを操作できるので有効のまま。
                if column_set == DetailsColumnSet::DedicatedBar {
                    ui.add_enabled(
                        false,
                        egui::Checkbox::new(&mut menu_state.show_preview, "プレビュー"),
                    )
                    .on_disabled_hover_text("下部バーはプレビューを表示しません");
                } else {
                    let response = ui.checkbox(&mut menu_state.show_preview, "プレビュー");
                    if let Some(text) = details_shared_preview_hover_text(column_set, origin) {
                        response.on_hover_text(text);
                    }
                }
                ui.checkbox(&mut menu_state.show_rating, "★");
                ui.checkbox(&mut menu_state.show_tags, "タグ");
                ui.checkbox(&mut menu_state.show_kind, "種類");
                ui.checkbox(&mut menu_state.show_page_count, "ページ数")
                    .on_hover_text(
                        "ZIP / PDF / 画像のみフォルダのページ数をバックグラウンドで読み込みます",
                    );
                ui.checkbox(&mut menu_state.show_place, "場所");
                ui.checkbox(&mut menu_state.show_size, "サイズ");
                ui.checkbox(&mut menu_state.show_modified, modified_label);
                ui.checkbox(&mut menu_state.show_state, state_label);

                ui.separator();
                ui.checkbox(&mut menu_state.show_created, "作成日時")
                    .on_hover_text("ファイルシステムの作成日時をバックグラウンドで読み込みます");
                ui.checkbox(&mut menu_state.show_image_dimensions, "画像解像度")
                    .on_hover_text("必要な値をバックグラウンドで読み込みます");
                ui.checkbox(&mut menu_state.show_video_duration, "長さ")
                    .on_hover_text("動画・音声の長さをバックグラウンドで読み込みます");
                ui.checkbox(&mut menu_state.show_video_dimensions, "動画解像度")
                    .on_hover_text("動画の解像度をバックグラウンドで読み込みます");
                ui.checkbox(&mut menu_state.show_video_codec, "コーデック")
                    .on_hover_text("動画・音声のコーデックをバックグラウンドで読み込みます");
            }
            DetailsColumnMenuPane::Format => {
                ui.label(egui::RichText::new("書式 (すべての表示で共通)").strong());
                ui.separator();
                ui.label("サイズ表示");
                for &mode in DetailsSizeDisplayMode::all() {
                    ui.radio_value(&mut menu_state.size_display_mode, mode, mode.label());
                }

                ui.separator();
                ui.label("日時");
                ui.checkbox(&mut menu_state.timestamp_show_seconds, "秒まで表示");

                ui.separator();
                ui.label("行表示");
                for &style in DetailsRowStyle::all() {
                    ui.radio_value(&mut menu_state.row_style, style, style.label());
                }
            }
        });

        let changes = menu_state.apply(&mut self.settings, column_set, current_name_width);
        // details_selection_bar_mode は環境設定ダイアログ所有のため、ダイアログを開いたまま
        // ここで切り替えた後に OK を押すと prepare_preferences_settings_for_commit が保持する
        // snapshot 側の値へ戻る。ほかの環境設定所有フィールドと同じ意図した優先順位である。
        let mode_changed =
            toggle_requested && toggle_details_selection_bar_mode_from_menu(&mut self.settings);
        if changes.any() || mode_changed {
            Self::cancel_details_best_fit_job(ui.ctx());
            if changes.lazy_columns || mode_changed {
                self.invalidate_details_meta_requirements();
            }
            if changes.columns && column_set != DetailsColumnSet::DedicatedBar {
                self.reset_details_sort_if_hidden();
                self.rebuild_details_order();
            }
            self.settings.save();
            ui.ctx().request_repaint();
        }
    }

    fn details_cell_text(
        &mut self,
        idx: usize,
        item: &GridItem,
        meta: Option<(i64, i64)>,
        column: DetailsColumn,
    ) -> String {
        match column {
            DetailsColumn::Preview => String::new(),
            DetailsColumn::Name => self
                .bookmark_view_row(idx)
                .map(crate::bookmark_browser::BookmarkBrowserRow::details_name)
                .unwrap_or_else(|| item.name().into_owned()),
            DetailsColumn::Rating => {
                let rating = if self.items_are_drive_list {
                    0
                } else {
                    self.get_rating(idx)
                };
                "★".repeat(rating as usize)
            }
            DetailsColumn::Tags => self.cell_tag_list(idx).join(" "),
            DetailsColumn::Kind => details_kind_label(
                item,
                self.archive_source_override.as_deref(),
                self.current_folder.as_deref(),
            ),
            DetailsColumn::PageCount => self.details_page_count_text(idx),
            DetailsColumn::Place => self
                .facet_place_path_for_idx(idx)
                .map(|path| self.facet_place_label_for_path(&path))
                .unwrap_or_default(),
            DetailsColumn::Size => meta
                .and_then(|(_, size)| (size > 0).then_some(size))
                .map(|size| {
                    crate::ui_helpers::format_details_size(
                        size as u64,
                        self.settings.details_size_display_mode,
                    )
                })
                .unwrap_or_default(),
            DetailsColumn::Modified => {
                if self.items_are_bookmark_view {
                    self.bookmark_view_row(idx)
                        .map(|row| {
                            format_details_mtime(
                                row.created_at_ms.div_euclid(1000),
                                self.settings.details_timestamp_show_seconds,
                            )
                        })
                        .unwrap_or_default()
                } else if self.items_are_reading_history_view {
                    self.reading_history_last_read_for_idx(idx)
                        .unwrap_or_default()
                } else {
                    meta.map(|(mtime, _)| {
                        format_details_mtime(mtime, self.settings.details_timestamp_show_seconds)
                    })
                    .unwrap_or_default()
                }
            }
            DetailsColumn::Created => self.details_created_text(idx),
            DetailsColumn::State => self.details_state_text(idx),
            DetailsColumn::ImageDimensions => self.details_image_dims_text(idx),
            DetailsColumn::VideoDuration => self.details_video_duration_text(idx),
            DetailsColumn::VideoDimensions => self.details_video_dims_text(idx),
            DetailsColumn::VideoCodec => self.details_video_codec_text(idx),
        }
    }

    /// 詳細一覧と下部情報バーが共有する、指定列の表示文字列を構築する。
    /// 既存キャッシュだけを参照し、列ごとの書式を単一箇所に保つ。
    fn details_row_data(
        &mut self,
        idx: usize,
        columns: &[DetailsColumn],
    ) -> Option<DetailsRowData> {
        let item = self.items.get(idx)?.clone();
        let meta = self.image_metas.get(idx).copied().flatten();
        let mut cells = Vec::with_capacity(columns.len());

        for &column in columns {
            if column == DetailsColumn::Preview {
                continue;
            }
            let text = self.details_cell_text(idx, &item, meta, column);
            cells.push((column, text));
        }
        Some(DetailsRowData { cells })
    }

    fn draw_details_row(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        idx: usize,
        row: usize,
        is_spread_pair_cursor: bool,
        column_set: DetailsColumnSet,
        display_only: bool,
        show_tooltip: bool,
    ) -> Option<egui::Rect> {
        if self.items.get(idx).is_none() {
            return None;
        }
        let column_rects = details_column_rects_for_columns(rect, &self.settings, column_set);
        let columns = column_rects
            .iter()
            .map(|(column, _)| *column)
            .collect::<Vec<_>>();
        let row_data = self.details_row_data(idx, &columns)?;
        let visuals = ui.visuals();
        let visual_state = details_row_visual_state(
            display_only,
            self.selected == Some(idx),
            self.checked.contains(&idx),
            ui.rect_contains_pointer(rect),
        );
        let selected = visual_state.selected;
        let checked = visual_state.checked;
        let row_style = self.settings.details_row_style;
        let bg = details_row_background(
            visuals,
            row_style,
            row,
            selected,
            checked,
            visual_state.hovered,
        );
        let text_color = details_row_text_color(visuals, selected);

        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, bg);
        if checked && !selected {
            let accent = egui::Rect::from_min_max(
                rect.left_top(),
                egui::pos2(rect.left() + 3.0, rect.bottom()),
            );
            painter.rect_filled(accent, 0.0, visuals.selection.bg_fill);
        }
        if row_style.show_separator() {
            let pixels_per_point = ui.ctx().pixels_per_point();
            let y = details_separator_y(rect, pixels_per_point);
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                egui::Stroke::new(
                    details_separator_stroke_width(pixels_per_point),
                    details_separator_color(visuals),
                ),
            );
        }
        if is_spread_pair_cursor && !selected {
            crate::app::draw_spread_pair_cursor(painter, rect, visuals);
        }

        let name = row_data.text(DetailsColumn::Name);
        let rating_text = row_data.text(DetailsColumn::Rating);
        let tags_text = row_data.text(DetailsColumn::Tags);
        let kind_text = row_data.text(DetailsColumn::Kind);
        let size_text = row_data.text(DetailsColumn::Size);
        let modified_text = row_data.text(DetailsColumn::Modified);
        let state_text = row_data.text(DetailsColumn::State);
        let mut hovered_preview_rect = None;

        for (col, col_rect) in column_rects {
            match col {
                DetailsColumn::Preview => {
                    let response = ui.interact(
                        col_rect,
                        ui.id().with(("details_preview_icon", idx)),
                        egui::Sense::hover(),
                    );
                    if response.hovered() {
                        hovered_preview_rect = Some(col_rect);
                    }
                    draw_details_preview_icon(
                        ui.painter(),
                        col_rect.shrink2(egui::vec2(6.0, 5.0)),
                        text_color,
                        false,
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
                DetailsColumn::PageCount => draw_details_text(
                    ui,
                    col_rect,
                    row_data.text(DetailsColumn::PageCount),
                    egui::Align2::RIGHT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::Place => draw_details_text(
                    ui,
                    col_rect,
                    row_data.text(DetailsColumn::Place),
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
                    row_data.text(DetailsColumn::Created),
                    egui::Align2::LEFT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::ImageDimensions => draw_details_text(
                    ui,
                    col_rect,
                    row_data.text(DetailsColumn::ImageDimensions),
                    egui::Align2::RIGHT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::VideoDuration => draw_details_text(
                    ui,
                    col_rect,
                    row_data.text(DetailsColumn::VideoDuration),
                    egui::Align2::RIGHT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::VideoDimensions => draw_details_text(
                    ui,
                    col_rect,
                    row_data.text(DetailsColumn::VideoDimensions),
                    egui::Align2::RIGHT_CENTER,
                    text_color,
                    false,
                ),
                DetailsColumn::VideoCodec => draw_details_text(
                    ui,
                    col_rect,
                    row_data.text(DetailsColumn::VideoCodec),
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
        if show_tooltip {
            self.draw_reading_history_tooltip(ui, rect, idx);
        }
        hovered_preview_rect
    }

    fn details_state_text(&mut self, idx: usize) -> String {
        if self.items_are_bookmark_view {
            return self
                .bookmark_view_row(idx)
                .map(crate::bookmark_browser::BookmarkBrowserRow::position_label)
                .unwrap_or_default();
        }
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
        if self.settings.grid_view_mode != GridViewMode::Details || self.items.is_empty() {
            Self::cancel_details_best_fit_job(ctx);
        }

        egui::CentralPanel::default()
            .show(ctx, |ui| -> Option<PathBuf> {
                // Sole MainGrid driver. Embedded still fullscreen returns
                // before render_grid, preventing a double feed on main ctx.
                let touch_scroll_enabled =
                    self.settings.grid_view_mode != GridViewMode::Details && !self.items.is_empty();
                let touch_frame = crate::touch_correlation::drive_egui_touch_input(
                    ctx,
                    crate::touch_correlation::TouchSurface::MainGrid,
                    crate::touch_input::TapZoneGeometry {
                        surface: ui.max_rect(),
                        excluded: Vec::new(),
                        behavior: crate::touch_input::TouchSurfaceBehavior::Grid,
                    },
                    self.frame_counter,
                    touch_scroll_enabled,
                );
                if !touch_scroll_enabled {
                    clear_grid_touch_scroll_remainder(ctx);
                    // The recognizer keeps observing contacts while command
                    // execution is disabled, so caller-owned pinch persistence
                    // must be closed explicitly instead of relying on a reset.
                    finish_grid_pinch_column_gesture(self, ctx, "disabled");
                }
                let touch_derived_pointer_activity =
                    touch_frame.has_touch_derived_pointer_activity();
                let global_searching =
                    self.items_are_global_search_view && self.global_search.is_searching();
                if self.items.is_empty() {
                    self.pending_grid_scroll = None;
                    // ZIP / PDF 非同期列挙中は「読み込み中…」にして待ち状態を明示する。
                    // BS や Ctrl+↑↓ はこの間でも受理され、load_folder 側で pending が
                    // Drop されて worker が cancel する。
                    let loading = self.zip_enumerate_pending.is_some()
                        || self.pdf_enumerate_pending.is_some();
                    let msg = if self.items_are_bookmark_view
                        && self.bookmark_browser_pending.is_some()
                    {
                        "ブックマークを読み込み中…"
                    } else if self.items_are_bookmark_view {
                        "ブックマークはありません"
                    } else if global_searching {
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
                    self.handle_grid_background_primary_click(ctx, ui.max_rect(), false);
                    if !self.items_are_bookmark_view
                        && ui.rect_contains_pointer(ui.max_rect())
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
                    self.draw_gamepad_ring_overlay(
                        ui,
                        full_rect,
                        crate::app::ActionSurface::MainWindow,
                    );
                    self.draw_gamepad_picker_overlay(
                        ui,
                        full_rect,
                        crate::app::ActionSurface::MainWindow,
                    );
                    self.draw_gamepad_favorite_picker_overlay(
                        ui,
                        full_rect,
                        crate::app::ActionSurface::MainWindow,
                    );
                    self.draw_gamepad_location_picker_overlay(
                        ui,
                        full_rect,
                        crate::app::ActionSurface::MainWindow,
                    );
                    self.draw_feedback_toast(
                        ui,
                        full_rect,
                        ctx,
                        crate::app::ActionSurface::MainWindow,
                    );
                    self.clear_mouse_ring_context_menu_suppression_if_idle(ctx);
                    return None;
                }

                if self.visible_indices.is_empty() {
                    self.pending_grid_scroll = None;
                    ui.centered_and_justified(|ui| {
                        ui.label(if self.items_are_bookmark_view {
                            "条件に一致するブックマークはありません"
                        } else if global_searching {
                            "検索中"
                        } else {
                            "検索結果なし"
                        });
                    });
                    self.start_grid_background_mouse_ring_flick_if_pressed(ctx, ui.max_rect());
                    self.update_grid_mouse_ring_flick(ctx);
                    self.handle_grid_background_primary_click(ctx, ui.max_rect(), false);
                    if !self.items_are_bookmark_view
                        && ui.rect_contains_pointer(ui.max_rect())
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
                    self.draw_gamepad_ring_overlay(
                        ui,
                        full_rect,
                        crate::app::ActionSurface::MainWindow,
                    );
                    self.draw_gamepad_picker_overlay(
                        ui,
                        full_rect,
                        crate::app::ActionSurface::MainWindow,
                    );
                    self.draw_gamepad_favorite_picker_overlay(
                        ui,
                        full_rect,
                        crate::app::ActionSurface::MainWindow,
                    );
                    self.draw_gamepad_location_picker_overlay(
                        ui,
                        full_rect,
                        crate::app::ActionSurface::MainWindow,
                    );
                    self.draw_feedback_toast(
                        ui,
                        full_rect,
                        ctx,
                        crate::app::ActionSurface::MainWindow,
                    );
                    self.clear_mouse_ring_context_menu_suppression_if_idle(ctx);
                    return None;
                }

                let spread_pair_cursor_idx = self.main_grid_spread_pair_cursor_idx();

                if self.settings.grid_view_mode == GridViewMode::Details {
                    let nav = self.render_details_list(ui, ctx, scroll_to, spread_pair_cursor_idx);
                    return nav;
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
                    clear_grid_touch_scroll_remainder(ctx);
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
                // セル寸法と最大 offset が確定した時点でだけ要求を所有する。これより後に
                // リング / ジェスチャが積んだ要求は次フレームで処理する。
                let pending_scroll =
                    take_grid_scroll_for_current_layout(&mut self.pending_grid_scroll);
                self.scroll_offset_y =
                    resolve_grid_scroll_offset(self.scroll_offset_y, max_offset, pending_scroll);

                let scroll_state = grid_touch_scroll_state(
                    ctx,
                    self.scroll_offset_y,
                    cell_h,
                    self.items_generation,
                );
                let mut fractional_drag_y = scroll_state.remainder_y;
                let mut touch_scroll_phase = scroll_state.phase;
                let mut touch_input_changed = false;
                let mut glide_advanced = false;
                let snap_now = std::time::Instant::now();
                if let GridTouchScrollPhase::Glide {
                    animation,
                    started_at,
                } = touch_scroll_phase
                {
                    let elapsed = snap_now.saturating_duration_since(started_at);
                    let position = grid_touch_snap_position_at(animation, elapsed);
                    self.scroll_offset_y = position.anchor_y;
                    fractional_drag_y = position.remainder_y;
                    glide_advanced = true;
                    if elapsed >= GRID_TOUCH_SNAP_GLIDE_DURATION {
                        touch_scroll_phase =
                            GridTouchScrollPhase::Contact { direction: None };
                        if crate::touch_debug::touch_debug_enabled() {
                            crate::logger::log(format!(
                                "[TOUCH-DEBUG] grid_scroll_glide_finish anchor={:.2} remainder={:.2}",
                                position.anchor_y, position.remainder_y
                            ));
                        }
                    }
                }
                for command in touch_frame.commands().iter().copied() {
                    match command {
                        crate::touch_input::TouchCommand::ScrollGrid { delta_y } => {
                            let previous_direction = match touch_scroll_phase {
                                GridTouchScrollPhase::Contact { direction } => direction,
                                GridTouchScrollPhase::Glide { .. } => None,
                            };
                            if matches!(touch_scroll_phase, GridTouchScrollPhase::Glide { .. })
                                && crate::touch_debug::touch_debug_enabled()
                            {
                                crate::logger::log(format!(
                                    "[TOUCH-DEBUG] grid_scroll_glide_interrupt cause=scroll anchor={:.2} remainder={fractional_drag_y:.2}",
                                    self.scroll_offset_y
                                ));
                            }
                            let before_y = self.scroll_offset_y + fractional_drag_y;
                            let position = apply_grid_touch_scroll_delta(
                                self.scroll_offset_y,
                                fractional_drag_y,
                                delta_y,
                                cell_h,
                                max_offset,
                            );
                            let after_y = position.anchor_y + position.remainder_y;
                            let direction = grid_touch_direction_after_move(
                                before_y,
                                after_y,
                                previous_direction,
                            );
                            self.scroll_offset_y = position.anchor_y;
                            fractional_drag_y = position.remainder_y;
                            touch_scroll_phase =
                                GridTouchScrollPhase::Contact { direction };
                            touch_input_changed = true;
                        }
                        crate::touch_input::TouchCommand::ScrollGridEnd => {
                            let before_y = self.scroll_offset_y + fractional_drag_y;
                            let direction = match touch_scroll_phase {
                                GridTouchScrollPhase::Contact { direction } => direction,
                                GridTouchScrollPhase::Glide { .. } => None,
                            };
                            let plan = plan_grid_touch_snap(
                                self.scroll_offset_y,
                                fractional_drag_y,
                                cell_h,
                                max_offset,
                                direction,
                            );
                            match plan {
                                GridTouchSnapPlan::Immediate(position) => {
                                    if crate::touch_debug::touch_debug_enabled() {
                                        crate::logger::log(format!(
                                            "[TOUCH-DEBUG] grid_scroll_settle mode=instant direction={direction:?} before={before_y:.2} after={:.2} travel={:.2} threshold={:.2}",
                                            position.anchor_y,
                                            (position.anchor_y - before_y).abs(),
                                            cell_h * GRID_TOUCH_SNAP_GLIDE_MIN_TRAVEL_RATIO
                                        ));
                                    }
                                    self.scroll_offset_y = position.anchor_y;
                                    fractional_drag_y = position.remainder_y;
                                    touch_scroll_phase =
                                        GridTouchScrollPhase::Contact { direction: None };
                                }
                                GridTouchSnapPlan::Glide(animation) => {
                                    if crate::touch_debug::touch_debug_enabled() {
                                        crate::logger::log(format!(
                                            "[TOUCH-DEBUG] grid_scroll_settle mode=glide direction={direction:?} before={before_y:.2} target={:.2} travel={:.2} threshold={:.2} duration_ms={}",
                                            animation.target.anchor_y,
                                            animation.travel_y(),
                                            cell_h * GRID_TOUCH_SNAP_GLIDE_MIN_TRAVEL_RATIO,
                                            GRID_TOUCH_SNAP_GLIDE_DURATION.as_millis()
                                        ));
                                    }
                                    self.scroll_offset_y = animation.start.anchor_y;
                                    fractional_drag_y = animation.start.remainder_y;
                                    touch_scroll_phase = GridTouchScrollPhase::Glide {
                                        animation,
                                        started_at: snap_now,
                                    };
                                }
                            }
                            touch_input_changed = true;
                        }
                        crate::touch_input::TouchCommand::Zoom { factor, .. } => {
                            let old_cols = self.settings.grid_cols;
                            let update =
                                apply_grid_pinch_column_factor(ctx, old_cols, factor);
                            if update.columns_changed {
                                let changed =
                                    self.change_grid_cols_by(update.new_cols as i32 - old_cols as i32);
                                debug_assert!(changed);
                                ctx.request_repaint();
                            }
                            if crate::touch_debug::touch_debug_enabled()
                                && update.threshold_crossed
                            {
                                crate::logger::log(format!(
                                    "[TOUCH-DEBUG] grid_pinch_columns factor={factor:.4} accumulated={:.4} cols={old_cols}->{} changed={}",
                                    update.accumulated_scale,
                                    update.new_cols,
                                    update.columns_changed
                                ));
                            }
                        }
                        crate::touch_input::TouchCommand::PinchEnd => {
                            finish_grid_pinch_column_gesture(self, ctx, "pinch_end");
                        }
                        // The grid gives pinch exclusive ownership: centroid
                        // motion must not scroll while scale changes columns.
                        crate::touch_input::TouchCommand::Pan { .. } => {}
                        crate::touch_input::TouchCommand::ToggleChrome
                        | crate::touch_input::TouchCommand::PageSide { .. } => {}
                    }
                }
                if let GridTouchScrollPhase::Glide {
                    animation,
                    started_at,
                } = touch_scroll_phase
                    && touch_frame.is_active()
                {
                    let position = grid_touch_snap_position_at(
                        animation,
                        snap_now.saturating_duration_since(started_at),
                    );
                    self.scroll_offset_y = position.anchor_y;
                    fractional_drag_y = position.remainder_y;
                    touch_scroll_phase = GridTouchScrollPhase::Contact { direction: None };
                    touch_input_changed = true;
                    if crate::touch_debug::touch_debug_enabled() {
                        crate::logger::log(format!(
                            "[TOUCH-DEBUG] grid_scroll_glide_interrupt cause=touch owner={:?} anchor={:.2} remainder={:.2}",
                            touch_frame.owner(),
                            position.anchor_y,
                            position.remainder_y
                        ));
                    }
                }
                if (touch_frame.touch_cancelled()
                    || touch_frame.owner() == crate::touch_input::TouchOwner::Cancelled)
                    && fractional_drag_y > GRID_TOUCH_REMAINDER_EPSILON
                {
                    let direction = match touch_scroll_phase {
                        GridTouchScrollPhase::Contact { direction } => direction,
                        GridTouchScrollPhase::Glide { .. } => None,
                    };
                    let position = settle_grid_touch_scroll(
                        self.scroll_offset_y,
                        fractional_drag_y,
                        cell_h,
                        max_offset,
                        direction,
                    );
                    self.scroll_offset_y = position.anchor_y;
                    fractional_drag_y = position.remainder_y;
                    touch_scroll_phase = GridTouchScrollPhase::Contact { direction: None };
                    touch_input_changed = true;
                }
                if touch_frame.touch_cancelled()
                    || touch_frame.owner() == crate::touch_input::TouchOwner::Cancelled
                {
                    finish_grid_pinch_column_gesture(self, ctx, "cancel");
                }
                // A touch that interrupted a glide may remain a tap rather
                // than becoming GridScroll. Once that contact leaves, settle
                // the inherited remainder instead of stranding a fractional
                // viewport indefinitely.
                if let GridTouchScrollPhase::Contact { direction } = touch_scroll_phase
                    && !touch_frame.is_active()
                    && fractional_drag_y > GRID_TOUCH_REMAINDER_EPSILON
                {
                    match plan_grid_touch_snap(
                        self.scroll_offset_y,
                        fractional_drag_y,
                        cell_h,
                        max_offset,
                        direction,
                    ) {
                        GridTouchSnapPlan::Immediate(position) => {
                            self.scroll_offset_y = position.anchor_y;
                            fractional_drag_y = position.remainder_y;
                            touch_scroll_phase =
                                GridTouchScrollPhase::Contact { direction: None };
                        }
                        GridTouchSnapPlan::Glide(animation) => {
                            self.scroll_offset_y = animation.start.anchor_y;
                            fractional_drag_y = animation.start.remainder_y;
                            touch_scroll_phase = GridTouchScrollPhase::Glide {
                                animation,
                                started_at: snap_now,
                            };
                        }
                    }
                    touch_input_changed = true;
                }
                set_grid_touch_scroll_state(
                    ctx,
                    self.scroll_offset_y,
                    cell_h,
                    self.items_generation,
                    fractional_drag_y,
                    touch_scroll_phase,
                );
                let touch_scroll_active = touch_scroll_enabled
                    && touch_frame.is_active()
                    && touch_frame.owner() == crate::touch_input::TouchOwner::GridScroll;
                let snap_glide_active =
                    grid_touch_snap_needs_animation_repaint(touch_scroll_phase);
                if touch_input_changed
                    || touch_scroll_active
                    || snap_glide_active
                    || glide_advanced
                {
                    self.note_grid_touch_scroll_activity();
                }
                // The glide is the only animation repaint producer. Its
                // terminal frame changes the phase before this branch, so an
                // absent Glide state cannot keep requesting animation frames.
                if snap_glide_active {
                    ctx.request_repaint();
                } else if touch_input_changed {
                    ctx.request_repaint();
                }
                let display_scroll_offset_y = self.scroll_offset_y + fractional_drag_y;

                let mut nav: Option<PathBuf> = None;
                let primary_click_pos = ctx.input(|i| {
                    i.pointer
                        .primary_clicked()
                        .then(|| i.pointer.interact_pos().or_else(|| i.pointer.latest_pos()))
                        .flatten()
                });
                let mut primary_click_hit_cell = false;

                // egui にスクロールを管理させず、自前の offset を毎フレーム注入する。
                // ただしスクロールバードラッグ時は egui 側のオフセットを読み戻す。
                let scroll_output = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .vertical_scroll_offset(display_scroll_offset_y)
                    .show_viewport(ui, |ui, viewport| {
                        // 実際のビューポート高さも記録する。リサイズ中の scroll extent
                        // 計算自体は上の `ui.available_height()` で同フレーム内に行う。
                        self.last_viewport_h = viewport.height();

                        let (content_rect, _) = ui.allocate_exact_size(
                            egui::vec2(avail_w, total_h),
                            egui::Sense::hover(),
                        );

                        let first_row = (viewport.min.y / cell_h) as usize;
                        let fractional_extra_row =
                            usize::from(fractional_drag_y > GRID_TOUCH_REMAINDER_EPSILON);
                        let last_row =
                            ((viewport.max.y / cell_h) as usize + 2 + fractional_extra_row)
                                .min(total_rows);

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

                                let badges = self.grid_edit_badges(idx);
                                let rating = if self.items_are_drive_list {
                                    0
                                } else {
                                    self.get_rating(idx)
                                };
                                let tags = self.cell_tag_list(idx).to_vec();
                                let has_pin = self.cell_has_pin_badge(idx);
                                let bookmark_time = self.bookmark_view_row(idx).map(|row| {
                                    if row.missing {
                                        format!("! {}", row.badge_label())
                                    } else {
                                        row.badge_label()
                                    }
                                });
                                let overlay_layout = crate::app::layout_cell_overlays(
                                    ui.painter(),
                                    cell_rect,
                                    crate::thumb_overlay_layout::EditBadgeFlags {
                                        page_override: badges.page_override,
                                        local_adjust: badges.local_adjust,
                                        mask: badges.mask,
                                        conceal: badges.conceal,
                                        comic: badges.comic,
                                        pin: has_pin,
                                    },
                                    rating,
                                    &self.items[idx],
                                    &self.thumbnails[idx],
                                    &tags,
                                    bookmark_time.as_deref(),
                                    self.items_are_drive_list,
                                );

                                primary_click_hit_cell |=
                                    primary_click_pos.is_some_and(|pos| cell_rect.contains(pos));
                                if let Some(n) = self.handle_cell_interaction(
                                    ui,
                                    ctx,
                                    cell_rect,
                                    idx,
                                    &overlay_layout,
                                    touch_derived_pointer_activity,
                                ) {
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
                                crate::app::draw_cell(
                                    ui,
                                    cell_rect,
                                    self.selected == Some(idx),
                                    self.checked.contains(&idx),
                                    spread_pair_cursor_idx == Some(idx),
                                    &overlay_layout,
                                    &self.items[idx],
                                    &self.thumbnails[idx],
                                    rot,
                                    adjusted_tex,
                                    filter_match_count,
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
                                self.draw_bookmark_view_overlay(
                                    ui,
                                    cell_rect,
                                    idx,
                                    &overlay_layout,
                                );

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
                self.handle_grid_background_primary_click(
                    ctx,
                    scroll_output.inner_rect,
                    primary_click_hit_cell,
                );
                let bg_right_clicked = ui.rect_contains_pointer(scroll_output.inner_rect)
                    && ctx.input(|i| i.pointer.secondary_clicked());
                if bg_right_clicked
                    && self.context_menu_idx.is_none()
                    && !self.selection_info_bar_contains_pointer(ctx)
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
                if should_sync_grid_scrollbar(
                    touch_derived_pointer_activity,
                    fractional_drag_y,
                    egui_offset,
                    self.scroll_offset_y,
                    cell_h,
                ) {
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
                self.draw_gamepad_ring_overlay(
                    ui,
                    full_rect,
                    crate::app::ActionSurface::MainWindow,
                );
                self.draw_gamepad_picker_overlay(
                    ui,
                    full_rect,
                    crate::app::ActionSurface::MainWindow,
                );
                self.draw_gamepad_favorite_picker_overlay(
                    ui,
                    full_rect,
                    crate::app::ActionSurface::MainWindow,
                );
                self.draw_gamepad_location_picker_overlay(
                    ui,
                    full_rect,
                    crate::app::ActionSurface::MainWindow,
                );
                self.draw_feedback_toast(ui, full_rect, ctx, crate::app::ActionSurface::MainWindow);

                nav
            })
            .inner
    }

    // ── 選択情報オーバーレイ ─────────────────────────────────────────

    /// ツールチップと下部情報バーが共有する、選択情報の整形結果を構築する。
    /// 参照するのは一覧・サムネイル・遅延メタデータの既存キャッシュだけで、I/O は行わない。
    fn selection_info_content(&self) -> Option<SelectionInfoContent> {
        let mut checked_indices = self.checked.iter().copied().collect::<Vec<_>>();
        checked_indices.sort_unstable();
        if checked_indices.len() > 1 {
            let mut lines = vec![format!("{} 個選択", checked_indices.len())];
            if self.settings.thumb_tooltip_show_file_size {
                let total_size = checked_indices.iter().try_fold(0u64, |total, &idx| {
                    let (_, size) = self.image_metas.get(idx).copied().flatten()?;
                    total.checked_add(u64::try_from(size).ok()?)
                });
                if let Some(total_size) = total_size {
                    lines.push(format!(
                        "合計サイズ {}",
                        crate::ui_helpers::format_details_size(
                            total_size,
                            self.settings.details_size_display_mode,
                        )
                    ));
                }
            }
            return Some(SelectionInfoContent { lines });
        }

        let idx = self.selected?;
        let item = self.items.get(idx)?;
        let bookmark_row = self.bookmark_view_row(idx);
        let mut lines = Vec::new();
        if self.settings.thumb_tooltip_show_filename {
            let name = bookmark_row
                .map(crate::bookmark_browser::BookmarkBrowserRow::display_name)
                .unwrap_or_else(|| item.name().into_owned());
            if !name.is_empty() {
                lines.push(name);
            }
        }
        if let Some(history_lines) = self.reading_history_selection_info_lines(idx) {
            lines.push(history_lines.join("   "));
        }
        if let Some(row) = bookmark_row {
            lines.push(format!(
                "位置 {}   登録日時 {}",
                row.position_label(),
                format_details_mtime(
                    row.created_at_ms.div_euclid(1000),
                    self.settings.details_timestamp_show_seconds,
                )
            ));
        }

        let mut fields = Vec::new();
        let mut full_location_line = None;
        if self.settings.thumb_tooltip_show_kind {
            fields.push(format!(
                "種類 {}",
                details_kind_label(
                    item,
                    self.archive_source_override.as_deref(),
                    self.current_folder.as_deref(),
                )
            ));
        }
        if self.settings.thumb_tooltip_show_page_count && self.details_item_supports_page_count(idx)
        {
            let page_count = self.details_page_count_text(idx);
            let display = if page_count.is_empty() {
                "..."
            } else {
                page_count.as_str()
            };
            fields.push(format!("ページ数 {display}"));
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
        if self.settings.thumb_tooltip_show_video_duration
            && matches!(item, GridItem::Video(_) | GridItem::Audio(_))
        {
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
        if self.settings.thumb_tooltip_show_video_codec
            && matches!(item, GridItem::Video(_) | GridItem::Audio(_))
        {
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
            if let Some(row) = bookmark_row {
                full_location_line = Some(format!("場所 {}", row.source_path().display()));
            } else if let Some(path) = self.facet_place_path_for_item(item) {
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
        (!lines.is_empty()).then_some(SelectionInfoContent { lines })
    }

    /// 選択情報の固定 1 行バーを、グリッド用 CentralPanel より先に確保する。
    /// TopBottomPanel が `ctx.available_rect()` を縮めるため、render_grid が読む
    /// `ui.available_height()` と仮想スクロールの viewport 高さには予約分が反映される。
    pub(crate) fn render_selection_info_bar(&mut self, ctx: &egui::Context) {
        self.selection_info_bar_rect = None;
        if self.viewer_session_blocks_main_window()
            || !self.settings.selection_info_display_mode.shows_bottom_bar()
            || selection_info_bottom_bar_is_hidden(&self.settings)
            || self.items_are_drive_list
        {
            return;
        }

        let column_set = selection_info_bottom_bar_column_set(&self.settings);
        let selected_idx = self.selected.filter(|&idx| self.items.get(idx).is_some());
        let available_before = ctx.available_rect();
        let future_details_avail_h = details_grid_available_height_after_selection_bar(
            available_before,
            ctx.style().as_ref(),
        );
        let details_natural_h = self.visible_indices.len() as f32 * Self::DETAILS_ROW_H;
        let scroll_style = details_scroll_style();
        let panel = egui::TopBottomPanel::bottom("selection_info_bottom_bar")
            .exact_height(selection_info_bar_height())
            .show_separator_line(true)
            .show(ctx, |ui| {
                let source_rect = ui.available_rect_before_wrap();
                let avail_w = source_rect.width().max(1.0);
                let pixels_per_point = ui.ctx().pixels_per_point();
                let layout_avail_w =
                    details_horizontal_viewport_capacity(source_rect, pixels_per_point)
                        .min(avail_w)
                        .max(1.0);
                let details_list_gutter = if self.settings.grid_view_mode == GridViewMode::Details {
                    resolve_details_list_layout(
                        source_rect,
                        future_details_avail_h,
                        ui.spacing().item_spacing.y,
                        details_natural_h,
                        pixels_per_point,
                        &self.settings,
                    )
                    .gutter
                } else {
                    0.0
                };
                let gutter = selection_info_bottom_bar_gutter(
                    self.settings.grid_view_mode,
                    details_list_gutter,
                );
                let full_layout = details_layout(
                    layout_avail_w,
                    gutter,
                    pixels_per_point,
                    &self.settings,
                    column_set,
                );
                let content_w = details_content_width_for_column_set(full_layout.pane_w);
                let avail_h = ui.available_height().max(0.0);
                let fixed_columns_w: f32 = details_visible_columns(&self.settings, column_set)
                    .into_iter()
                    .filter(|column| *column != DetailsColumn::Name)
                    .map(|column| details_column_width(&self.settings, column_set, column))
                    .sum();
                let horizontal_policy = details_horizontal_scroll_policy(
                    source_rect,
                    full_layout.extent,
                    full_layout.columns_w,
                    pixels_per_point,
                );
                let previous_scroll_style = ui.spacing().scroll;
                ui.spacing_mut().scroll = scroll_style;
                let output = configured_details_horizontal_scroll_area(horizontal_policy)
                    .id_salt("selection_info_bottom_bar_horizontal")
                    .show(ui, |ui| {
                        let old_spacing_y = ui.spacing().item_spacing.y;
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.set_min_width(horizontal_policy.scroll_extent);
                        let (header_rect, _) = ui.allocate_exact_size(
                            egui::vec2(content_w, Self::DETAILS_HEADER_H),
                            egui::Sense::hover(),
                        );
                        self.draw_details_header_static(ui, header_rect, column_set);
                        let (row_rect, _) = ui.allocate_exact_size(
                            egui::vec2(content_w, Self::DETAILS_ROW_H),
                            egui::Sense::hover(),
                        );
                        if let Some(idx) = selected_idx {
                            let _ = self.draw_details_row(
                                ui, row_rect, idx, 0, false, column_set, true, false,
                            );
                        }
                        ui.spacing_mut().item_spacing.y = old_spacing_y;
                    });
                ui.spacing_mut().scroll = previous_scroll_style;
                log_details_layout_debug(
                    ctx,
                    DetailsLayoutDebugSample {
                        surface: "selection_info_bar",
                        source_rect,
                        avail_w,
                        avail_h,
                        row_count: usize::from(selected_idx.is_some()),
                        natural_h: Self::DETAILS_HEADER_H + Self::DETAILS_ROW_H,
                        viewport_h_est: avail_h,
                        horizontal_policy,
                        predicted_hbar: if horizontal_policy.overflow {
                            scroll_style.allocated_width()
                        } else {
                            0.0
                        },
                        predicted_vscroll: false,
                        gutter,
                        fixed_columns_w,
                        name_w: full_layout.name_w,
                        pane_w: content_w,
                        layout_extent: full_layout.extent,
                        requested_extent: horizontal_policy.scroll_extent,
                        column_set,
                        outer_inner_rect: output.inner_rect,
                        outer_content_size: output.content_size,
                        outer_state: output.state,
                        vertical: None,
                        settings: &self.settings,
                    },
                );
            });
        self.selection_info_bar_rect = Some(panel.response.rect);
        let available_after = ctx.available_rect();
        debug_assert!(
            available_before.height() <= 0.0
                || available_before.height() - available_after.height() + 0.5
                    >= panel.response.rect.height().min(available_before.height()),
            "selection-info bar must reserve height before the grid viewport"
        );
    }

    fn selection_info_bar_contains_pointer(&self, ctx: &egui::Context) -> bool {
        let pos = ctx.input(|input| {
            input
                .pointer
                .interact_pos()
                .or_else(|| input.pointer.latest_pos())
        });
        selection_info_bar_contains_pos(self.selection_info_bar_rect, pos)
    }

    /// 選択中アイテムの情報を選択セル / 行の直下に表示する。
    pub(crate) fn render_selection_info(&self, ctx: &egui::Context) {
        // メインウィンドウを専有するビューア中は出さない (独自のホバーヘッダーを持つため)。
        if self.viewer_session_blocks_main_window()
            || !self.settings.selection_info_display_mode.shows_tooltip()
            || !grid_row_text_tooltips_enabled(self.settings.grid_view_mode)
            || self.items_are_drive_list
        {
            return;
        }

        let Some(cell_rect) = self.selected_cell_rect else {
            return;
        };
        let Some(content) = self.selection_info_content() else {
            return;
        };
        let text = content.tooltip_text();

        let style = ctx.style();
        let dark = style.visuals.dark_mode;
        let (fill, text_color, stroke, shadow) = if dark {
            (
                egui::Color32::from_rgba_unmultiplied(20, 25, 35, 230),
                style.visuals.text_color(),
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
                style.visuals.text_color(),
                egui::Stroke::new(1.0, egui::Color32::from_gray(172)),
                egui::Shadow {
                    offset: [0, 2],
                    blur: 12,
                    spread: 0,
                    color: egui::Color32::from_rgba_unmultiplied(0, 0, 0, 52),
                },
            )
        };

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

        let popup_frame = egui::Frame::popup(&style)
            .fill(fill)
            .stroke(stroke)
            .shadow(shadow);
        let inner_width = (popup_w - popup_frame.total_margin().sum().x).max(40.0);
        // 配置前に実際の折り返し条件で galley を作る。改行数からの高さ推定では、
        // 空白のない長いファイル名が 3 行へ折り返されたときに選択行へ重なっていた。
        let mut job = egui::text::LayoutJob::single_section(
            text,
            egui::TextFormat {
                font_id: egui::TextStyle::Monospace.resolve(&style),
                color: text_color,
                ..Default::default()
            },
        );
        job.wrap.max_width = inner_width;
        job.wrap.max_rows = 3;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let galley = ctx.fonts_mut(|fonts| fonts.layout_job(job));
        let measured_h = galley.size().y + popup_frame.total_margin().sum().y;
        let y = selection_info_popup_y(cell_rect, viewport, measured_h);
        let area_pos = egui::pos2(x, y);

        egui::Area::new("selection_info".into())
            .order(egui::Order::Middle)
            .interactable(false)
            .fixed_pos(area_pos)
            .show(ctx, |ui| {
                popup_frame.show(ui, |ui| {
                    ui.set_min_width(inner_width);
                    ui.set_max_width(inner_width);
                    ui.add(egui::Label::new(galley));
                });
            });
    }
}

#[cfg(test)]
mod facet_filter_bar_tests {
    use super::*;
    use crate::app::setup_app_for_test;

    #[test]
    fn omitted_chip_counts_every_user_file_and_leaves_system_files_out() {
        let counts = crate::app::OmittedFolderEntryCounts {
            same_name: 3,
            hidden: 2,
            unsupported: 5,
            system: 4,
        };
        assert_eq!(
            omitted_entries_chip_label(counts).as_deref(),
            Some("非表示 10 件")
        );
        assert_eq!(
            omitted_entries_breakdown_label(counts),
            "同名など 3 / 隠し項目 2 / 対象外 5 / システム 4"
        );

        let unsupported_only = crate::app::OmittedFolderEntryCounts {
            same_name: 0,
            hidden: 0,
            unsupported: 5,
            system: 0,
        };
        assert_eq!(
            omitted_entries_chip_label(unsupported_only).as_deref(),
            Some("非表示 5 件"),
            "対象外拡張子だけでもチップを出す"
        );
        assert_eq!(
            omitted_entries_breakdown_label(unsupported_only),
            "同名など 0 / 隠し項目 0 / 対象外 5",
            "システム 0 件のときは内訳から省く"
        );

        let system_only = crate::app::OmittedFolderEntryCounts {
            same_name: 0,
            hidden: 0,
            unsupported: 0,
            system: 9,
        };
        assert_eq!(omitted_entries_chip_label(system_only), None);
        assert_eq!(
            omitted_entries_chip_label(Default::default()),
            None,
            "主数字が 0 件ならチップを出さない"
        );
    }

    #[test]
    fn omitted_chip_opens_breakdown_and_same_name_settings_link() {
        use egui_kittest::{Harness, kittest::Queryable};

        let counts = crate::app::OmittedFolderEntryCounts {
            same_name: 3,
            hidden: 2,
            unsupported: 5,
            system: 0,
        };
        let mut harness = Harness::builder()
            .with_size(egui::vec2(640.0, 240.0))
            .build(move |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = draw_omitted_entries_chip(ui, counts);
                });
            });
        harness.run();
        harness.get_by_label("非表示 10 件").click();
        harness.run();

        assert!(
            harness
                .query_by_label("同名など 3 / 隠し項目 2 / 対象外 5")
                .is_some()
        );
        assert!(harness.query_by_label("同名ファイル設定を開く").is_some());
    }

    fn collect_shape_text(shape: &egui::epaint::Shape, text: &mut String) {
        match shape {
            egui::epaint::Shape::Text(text_shape) => {
                text.push_str(&text_shape.galley.job.text);
                text.push('\n');
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_shape_text(shape, text);
                }
            }
            _ => {}
        }
    }

    fn render_filter_bar_text(reading_history: bool) -> String {
        let mut app = setup_app_for_test();
        app.settings.show_toolbar_facet_filter = true;
        app.settings.toolbar_facet_filter_items = vec![
            crate::settings::ToolbarFacetFilterItem::Kind,
            crate::settings::ToolbarFacetFilterItem::Ext,
            crate::settings::ToolbarFacetFilterItem::Place,
            crate::settings::ToolbarFacetFilterItem::NameFilter,
        ];
        app.items = vec![GridItem::Video(std::path::PathBuf::from(
            "c:/media/movie.mp4",
        ))];
        app.visible_indices = vec![0];
        app.items_are_reading_history_view = reading_history;
        app.items_are_bookmark_view = !reading_history;

        let ctx = egui::Context::default();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 240.0),
                )),
                ..Default::default()
            },
            |ctx| app.render_facet_filter_bar(ctx),
        );
        let mut text = String::new();
        for clipped in &output.shapes {
            collect_shape_text(&clipped.shape, &mut text);
        }
        text
    }

    #[test]
    fn reading_history_and_bookmark_bars_both_render_standard_facets() {
        let history = render_filter_bar_text(true);
        assert!(history.contains("閲覧履歴種別: すべて"), "{history}");
        assert!(history.contains("本: すべて"), "{history}");
        for label in ["種類", "拡張子", "場所"] {
            assert!(history.contains(label), "missing {label}: {history}");
        }
        assert!(history.contains("ファイル名"), "{history}");

        let bookmark = render_filter_bar_text(false);
        assert!(bookmark.contains("ブックマーク種別: すべて"), "{bookmark}");
        assert!(bookmark.contains("本: すべて"), "{bookmark}");
        for label in ["種類", "拡張子", "場所"] {
            assert!(bookmark.contains(label), "missing {label}: {bookmark}");
        }
        assert!(bookmark.contains("ファイル名"), "{bookmark}");
    }

    #[test]
    fn hidden_name_filter_is_not_drawn_and_clears_invisible_condition() {
        let mut app = setup_app_for_test();
        app.settings.show_toolbar_facet_filter = true;
        app.settings.toolbar_facet_filter_items =
            vec![crate::settings::ToolbarFacetFilterItem::Kind];
        app.settings.facet_filter.name_query = "needle".to_owned();
        app.facet_name_input = "needle".to_owned();
        app.items = vec![GridItem::Image(std::path::PathBuf::from(
            "c:/media/needle.jpg",
        ))];
        app.visible_indices = vec![0];

        let ctx = egui::Context::default();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 200.0),
                )),
                ..Default::default()
            },
            |ctx| app.render_facet_filter_bar(ctx),
        );
        let mut text = String::new();
        for clipped in &output.shapes {
            collect_shape_text(&clipped.shape, &mut text);
        }

        assert!(!text.contains("ファイル名"), "{text}");
        assert!(app.facet_name_input.is_empty());
        assert!(app.settings.facet_filter.name_query.is_empty());
    }

    #[test]
    fn active_name_filter_uses_input_only_without_duplicate_chip() {
        let mut app = setup_app_for_test();
        app.settings.show_toolbar_facet_filter = true;
        app.settings.toolbar_facet_filter_items =
            vec![crate::settings::ToolbarFacetFilterItem::NameFilter];
        app.settings.facet_filter.name_query = "needle".to_owned();
        app.facet_name_input = "needle".to_owned();
        app.items = vec![GridItem::Image(std::path::PathBuf::from(
            "c:/media/needle.jpg",
        ))];
        app.visible_indices = vec![0];

        let ctx = egui::Context::default();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 200.0),
                )),
                ..Default::default()
            },
            |ctx| app.render_facet_filter_bar(ctx),
        );
        let mut text = String::new();
        for clipped in &output.shapes {
            collect_shape_text(&clipped.shape, &mut text);
        }

        assert!(text.contains("needle"), "{text}");
        assert!(!text.contains("ファイル名:needle"), "{text}");
    }
}

#[cfg(test)]
mod details_column_view_title_tests {
    use super::*;
    use crate::app::setup_app_for_test;

    /// 一覧ヘッダと列の表示切替メニューが同じ列名を出すことを、ビューごとに固定する。
    /// ブックマーク一覧では「状態」列が「位置」を表示するのに、メニュー側だけ「状態」の
    /// ままで目的の列を見つけられない不具合があった。
    #[test]
    fn bookmark_and_history_views_rename_modified_and_state() {
        let mut app = setup_app_for_test();

        assert_eq!(
            app.details_column_view_title(DetailsColumn::Modified),
            "更新日時"
        );
        assert_eq!(app.details_column_view_title(DetailsColumn::State), "状態");

        app.items_are_bookmark_view = true;
        assert_eq!(
            app.details_column_view_title(DetailsColumn::Modified),
            "登録日時"
        );
        assert_eq!(app.details_column_view_title(DetailsColumn::State), "位置");
        // 名前など意味が変わらない列はビューに依存しない。
        assert_eq!(
            app.details_column_view_title(DetailsColumn::Name),
            DetailsColumn::Name.title()
        );

        app.items_are_bookmark_view = false;
        app.items_are_reading_history_view = true;
        assert_eq!(
            app.details_column_view_title(DetailsColumn::Modified),
            "最終閲覧"
        );
        assert_eq!(
            app.details_column_view_title(DetailsColumn::State),
            "閲覧位置"
        );
    }

    /// ヘッダ側 (`details_header_title`) が同じ 1 か所を通ることの回帰テスト。
    /// 片方だけ直す実装に戻ったらここで落ちる。
    #[test]
    fn header_title_uses_the_shared_view_title() {
        let mut app = setup_app_for_test();
        app.items_are_bookmark_view = true;

        for col in [
            DetailsColumn::Modified,
            DetailsColumn::State,
            DetailsColumn::Name,
        ] {
            assert_eq!(
                app.details_header_title(col, false, false),
                app.details_column_view_title(col),
            );
        }
    }
}

#[cfg(test)]
mod selection_info_tests {
    use super::*;
    use crate::app::{AppTestEnvForTest, DetailsLazyMeta, setup_app_for_test};

    fn app_with_item(item: GridItem, meta: Option<(i64, i64)>) -> AppTestEnvForTest {
        let mut app = setup_app_for_test();
        app.items = vec![item];
        app.image_metas = vec![meta];
        app.thumbnails = vec![ThumbnailState::Pending];
        app.selected = Some(0);
        app.settings.thumb_tooltip_show_kind = true;
        app
    }

    fn loaded_thumbnail(ctx: &egui::Context, dims: (u32, u32)) -> ThumbnailState {
        let tex = ctx.load_texture(
            "selection_info_test",
            egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
            egui::TextureOptions::LINEAR,
        );
        ThumbnailState::Loaded {
            tex,
            from_cache: false,
            from_edit_preview: false,
            rendered_at_px: 128,
            source_dims: Some(dims),
        }
    }

    #[test]
    fn row_text_tooltips_are_disabled_only_in_details_view() {
        assert!(grid_row_text_tooltips_enabled(GridViewMode::Thumbnail));
        assert!(!grid_row_text_tooltips_enabled(GridViewMode::Details));
    }

    #[test]
    fn details_place_column_uses_subfolder_relative_label() {
        let root = PathBuf::from(r"C:\library");
        let mut app = app_with_item(
            GridItem::Image(root.join("ID203").join("thumb").join("page.jpg")),
            None,
        );
        app.items_are_subfolder_expansion_view = true;
        app.subfolder_expansion_root = Some(root);

        let data = app.details_row_data(0, &[DetailsColumn::Place]).unwrap();
        assert_eq!(
            data.text(DetailsColumn::Place),
            Path::new("ID203").join("thumb").display().to_string()
        );
    }

    #[test]
    fn shared_builder_formats_image_fields() {
        let ctx = egui::Context::default();
        let mut app = app_with_item(
            GridItem::Image(PathBuf::from(r"C:\pics\photo.jpg")),
            Some((1_700_000_000, 2048)),
        );
        app.settings.thumb_tooltip_show_file_size = true;
        app.thumbnails[0] = loaded_thumbnail(&ctx, (640, 480));

        let text = app.selection_info_content().unwrap().single_line_text();
        assert!(text.contains("photo.jpg"));
        assert!(text.contains("種類 JPG 画像"));
        assert!(text.contains("サイズ 2.0 KB"));
        assert!(text.contains("画像 640 × 480"));
    }

    #[test]
    fn details_row_data_uses_shared_details_formatting_for_cursor_index() {
        let mut app = app_with_item(
            GridItem::Image(PathBuf::from(r"C:\pics\photo.jpg")),
            Some((1_700_000_000, 2048)),
        );
        let columns = [
            DetailsColumn::Name,
            DetailsColumn::Kind,
            DetailsColumn::Size,
            DetailsColumn::Modified,
        ];
        let data = app.details_row_data(0, &columns).unwrap();

        assert_eq!(data.text(DetailsColumn::Name), "photo.jpg");
        assert_eq!(data.text(DetailsColumn::Kind), "JPG 画像");
        assert_eq!(data.text(DetailsColumn::Size), "2.0 KB");
        assert_eq!(
            data.text(DetailsColumn::Modified),
            format_details_mtime(1_700_000_000, false)
        );
    }

    #[test]
    fn shared_builder_formats_video_fields_from_cached_lazy_meta() {
        let mut app = app_with_item(
            GridItem::Video(PathBuf::from(r"C:\clips\movie.mp4")),
            Some((1_700_000_000, 4096)),
        );
        app.settings.thumb_tooltip_show_video_dimensions = true;
        app.settings.thumb_tooltip_show_video_codec = true;
        let key = app.metadata_cache_key(0).unwrap();
        app.details_lazy_meta.insert(
            key,
            DetailsLazyMeta {
                source_mtime: 1_700_000_000,
                source_size: 4096,
                video_duration_secs: Some(125.0),
                video_dims: Some((1920, 1080)),
                video_codec: Some("h264".to_string()),
                ..Default::default()
            },
        );

        let text = app.selection_info_content().unwrap().single_line_text();
        assert!(text.contains("movie.mp4"));
        assert!(text.contains("種類 MP4 動画"));
        assert!(text.contains("長さ 2:05"));
        assert!(text.contains("動画 1920 × 1080"));
        assert!(text.contains("コーデック h264"));
    }

    #[test]
    fn shared_builder_formats_zip_image_fields() {
        let ctx = egui::Context::default();
        let mut app = app_with_item(
            GridItem::ZipImage {
                zip_path: PathBuf::from(r"C:\books\book.zip"),
                entry_name: "chapter/page01.png".to_string(),
            },
            Some((1_700_000_000, 1024)),
        );
        app.thumbnails[0] = loaded_thumbnail(&ctx, (800, 1200));

        let text = app.selection_info_content().unwrap().single_line_text();
        assert!(text.contains("page01.png"));
        assert!(text.contains("種類 ZIP 内画像"));
        assert!(text.contains("画像 800 × 1200"));
    }

    #[test]
    fn converted_archive_page_uses_original_container_kind() {
        let cached_zip = PathBuf::from(r"C:\cache\converted.zip");
        let mut app = app_with_item(
            GridItem::ZipImage {
                zip_path: cached_zip.clone(),
                entry_name: "page01.jpg".to_string(),
            },
            None,
        );
        app.current_folder = Some(cached_zip);
        app.archive_source_override = Some(PathBuf::from(r"C:\books\source.rar"));

        let text = app.selection_info_content().unwrap().single_line_text();
        assert!(text.contains("種類 RAR 内画像"));
        let data = app.details_row_data(0, &[DetailsColumn::Kind]).unwrap();
        assert_eq!(data.text(DetailsColumn::Kind), "RAR 内画像");
    }

    #[test]
    fn archive_page_kind_covers_all_supported_original_formats() {
        let cache = PathBuf::from(r"C:\cache\converted.zip");
        for (source, expected) in [
            (r"C:\books\source.rar", "RAR 内画像"),
            (r"C:\books\source.7z", "7z 内画像"),
            (r"C:\books\source.lzh", "LZH 内画像"),
        ] {
            assert_eq!(
                archive_inner_image_kind_label(
                    &cache,
                    Some(std::path::Path::new(source)),
                    Some(cache.as_path()),
                ),
                expected
            );
        }
    }

    #[test]
    fn converted_archive_folder_uses_original_container_kind() {
        let cache = PathBuf::from(r"C:\cache\converted.zip");
        for (source, expected) in [
            (r"C:\books\source.rar", "RAR 内フォルダ"),
            (r"C:\books\source.7z", "7z 内フォルダ"),
            (r"C:\books\source.lzh", "LZH 内フォルダ"),
        ] {
            let item = GridItem::ZipDir {
                zip_path: cache.clone(),
                dir_prefix: "chapter/".to_string(),
                is_archive: false,
                representative: None,
            };
            assert_eq!(
                details_kind_label(
                    &item,
                    Some(std::path::Path::new(source)),
                    Some(cache.as_path()),
                ),
                expected
            );
        }
    }

    #[test]
    fn selection_info_popup_above_keeps_measured_height_clear_of_selected_row() {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let selected = egui::Rect::from_min_size(egui::pos2(100.0, 520.0), egui::vec2(160.0, 54.0));
        let popup_height = 72.0;

        let y = selection_info_popup_y(selected, viewport, popup_height);

        assert!(y + popup_height <= selected.top() - 6.0);
        assert!(y >= viewport.top() + 8.0);
    }

    #[test]
    fn shared_builder_formats_pdf_page_fields() {
        let ctx = egui::Context::default();
        let mut app = app_with_item(
            GridItem::PdfPage {
                pdf_path: PathBuf::from(r"C:\books\book.pdf"),
                page_num: 2,
                content_type: None,
            },
            Some((1_700_000_000, 3072)),
        );
        app.thumbnails[0] = loaded_thumbnail(&ctx, (1200, 1600));

        let text = app.selection_info_content().unwrap().single_line_text();
        assert!(text.contains("Page 3"));
        assert!(text.contains("種類 PDF ページ"));
        assert!(text.contains("画像 1200 × 1600"));
    }

    #[test]
    fn shared_builder_formats_zip_and_pdf_container_fields() {
        let mut zip = app_with_item(
            GridItem::ZipFile(PathBuf::from(r"C:\books\book.zip")),
            Some((1_700_000_000, 4096)),
        );
        zip.settings.thumb_tooltip_show_file_size = true;
        zip.details_lazy_meta.insert(
            crate::adjustment_db::normalize_path(Path::new(r"C:\books\book.zip")),
            DetailsLazyMeta {
                source_mtime: 1_700_000_000,
                source_size: 4096,
                page_count: Some(42),
                page_count_checked: true,
                ..Default::default()
            },
        );
        let zip_text = zip.selection_info_content().unwrap().single_line_text();
        assert!(zip_text.contains("book.zip"));
        assert!(zip_text.contains("種類 ZIP"));
        assert!(zip_text.contains("ページ数 42"));
        assert!(zip_text.contains("サイズ 4.0 KB"));
        assert_eq!(
            zip.details_row_data(0, &[DetailsColumn::PageCount])
                .unwrap()
                .text(DetailsColumn::PageCount),
            "42"
        );
        drop(zip);

        let mut pdf = app_with_item(
            GridItem::PdfFile(PathBuf::from(r"C:\books\book.pdf")),
            Some((1_700_000_000, 8192)),
        );
        pdf.settings.thumb_tooltip_show_file_size = true;
        pdf.details_lazy_meta.insert(
            crate::adjustment_db::normalize_path(Path::new(r"C:\books\book.pdf")),
            DetailsLazyMeta {
                source_mtime: 1_700_000_000,
                source_size: 8192,
                page_count: Some(88),
                page_count_checked: true,
                ..Default::default()
            },
        );
        let pdf_text = pdf.selection_info_content().unwrap().single_line_text();
        assert!(pdf_text.contains("book.pdf"));
        assert!(pdf_text.contains("種類 PDF"));
        assert!(pdf_text.contains("ページ数 88"));
        assert!(pdf_text.contains("サイズ 8.0 KB"));
    }

    #[test]
    fn shared_builder_formats_multi_selection_and_cached_total_size() {
        let mut app = setup_app_for_test();
        app.items = vec![
            GridItem::Image(PathBuf::from(r"C:\pics\a.jpg")),
            GridItem::Image(PathBuf::from(r"C:\pics\b.jpg")),
            GridItem::Image(PathBuf::from(r"C:\pics\c.jpg")),
        ];
        app.image_metas = vec![Some((1, 1024)), Some((2, 2048)), Some((3, 3072))];
        app.selected = Some(1);
        app.checked = [0usize, 1, 2].into_iter().collect();
        app.settings.thumb_tooltip_show_file_size = true;

        let content = app.selection_info_content().unwrap();
        assert_eq!(content.lines, vec!["3 個選択", "合計サイズ 6.0 KB"]);
    }

    #[test]
    fn multi_selection_omits_total_until_every_size_is_cached() {
        let mut app = setup_app_for_test();
        app.items = vec![
            GridItem::Image(PathBuf::from(r"C:\pics\a.jpg")),
            GridItem::Image(PathBuf::from(r"C:\pics\b.jpg")),
        ];
        app.image_metas = vec![Some((1, 1024)), None];
        app.selected = Some(0);
        app.checked = [0usize, 1].into_iter().collect();
        app.settings.thumb_tooltip_show_file_size = true;

        assert_eq!(
            app.selection_info_content().unwrap().lines,
            vec!["2 個選択"]
        );
    }

    #[test]
    fn bottom_bar_reserves_height_from_grid_central_panel() {
        let mut app = setup_app_for_test();
        app.settings.selection_info_display_mode =
            crate::settings::SelectionInfoDisplayMode::BottomBar;
        let ctx = egui::Context::default();
        let mut raw_input = egui::RawInput::default();
        raw_input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(640.0, 480.0),
        ));
        let mut measured = None;

        let _ = ctx.run(raw_input, |ctx| {
            let before = ctx.available_rect().height();
            app.render_selection_info_bar(ctx);
            let after = ctx.available_rect().height();
            let central = egui::CentralPanel::default()
                .show(ctx, |ui| ui.available_height())
                .inner;
            measured = Some((before, after, central));
        });

        let (before, after, central) = measured.unwrap();
        assert!(before - after >= selection_info_bar_height() - 0.5);
        assert!(central <= after + 0.5);
    }

    #[test]
    fn bottom_bar_predicts_the_details_grid_available_height() {
        for screen_height in [180.0_f32, 480.0, 777.25] {
            let mut app = setup_app_for_test();
            app.settings.grid_view_mode = GridViewMode::Details;
            app.settings.selection_info_display_mode =
                crate::settings::SelectionInfoDisplayMode::BottomBar;
            let ctx = egui::Context::default();
            let mut raw_input = egui::RawInput::default();
            raw_input.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, screen_height),
            ));
            let mut measured = None;

            let _ = ctx.run(raw_input, |ctx| {
                let predicted = details_grid_available_height_after_selection_bar(
                    ctx.available_rect(),
                    ctx.style().as_ref(),
                );
                app.render_selection_info_bar(ctx);
                let actual = egui::CentralPanel::default()
                    .show(ctx, |ui| ui.available_height())
                    .inner;
                measured = Some((predicted, actual));
            });

            let (predicted, actual) = measured.unwrap();
            assert!(
                (predicted - actual).abs() < 0.01,
                "screen_height={screen_height}, predicted={predicted}, actual={actual}"
            );
        }
    }

    #[test]
    fn details_scrollbars_reserve_space_instead_of_covering_rows() {
        let scroll = details_scroll_style();
        assert!(!scroll.floating);
        assert!(scroll.allocated_width() > 0.0);
        assert_eq!(
            selection_info_bar_height(),
            SELECTION_INFO_BAR_CONTENT_HEIGHT + scroll.allocated_width()
        );
    }

    #[test]
    fn details_lazy_status_reserves_bottom_without_moving_list_origin() {
        let mut app = setup_app_for_test();
        app.settings.grid_view_mode = GridViewMode::Details;
        app.settings.details_show_page_count = true;
        app.settings.details_show_created = false;
        app.settings.details_show_image_dimensions = false;
        app.settings.details_show_video_duration = false;
        app.settings.details_show_video_dimensions = false;
        app.settings.details_show_video_codec = false;
        app.items = vec![GridItem::ZipFile(PathBuf::from(r"C:\Books\book.zip"))];
        app.details_image_dims_state = LazyColumnState::Loading { done: 1, total: 2 };
        let ctx = egui::Context::default();
        let mut raw_input = egui::RawInput::default();
        raw_input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(640.0, 480.0),
        ));
        let mut measured = None;

        let _ = ctx.run(raw_input, |ctx| {
            let before = ctx.available_rect();
            app.render_details_lazy_status_bar(ctx);
            let after = ctx.available_rect();
            measured = Some((before, after));
        });

        let (before, after) = measured.unwrap();
        assert!((after.top() - before.top()).abs() < 0.01);
        assert!(before.bottom() - after.bottom() >= 24.5);
    }

    #[test]
    fn details_lazy_status_reserves_bottom_while_reconciling_jobs() {
        let mut app = setup_app_for_test();
        app.settings.grid_view_mode = GridViewMode::Details;
        app.settings.details_show_page_count = true;
        app.items = vec![GridItem::ZipFile(PathBuf::from(r"C:\Books\book.zip"))];
        app.details_image_dims_state = LazyColumnState::Reconciling {
            done: 2,
            total: 2,
            failed: 0,
        };
        let ctx = egui::Context::default();
        let mut raw_input = egui::RawInput::default();
        raw_input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(640.0, 480.0),
        ));
        let mut measured = None;

        let _ = ctx.run(raw_input, |ctx| {
            let before = ctx.available_rect();
            app.render_details_lazy_status_bar(ctx);
            let after = ctx.available_rect();
            measured = Some((before, after));
        });

        let (before, after) = measured.unwrap();
        assert!((after.top() - before.top()).abs() < 0.01);
        assert!(before.bottom() - after.bottom() >= 24.5);
    }

    #[test]
    fn details_lazy_status_keeps_failed_ready_session_visible() {
        let mut app = setup_app_for_test();
        app.settings.grid_view_mode = GridViewMode::Details;
        app.settings.details_show_page_count = true;
        app.items = vec![GridItem::ZipFile(PathBuf::from(r"C:\Books\book.zip"))];
        app.details_image_dims_state = LazyColumnState::Ready { failed: 2 };
        let ctx = egui::Context::default();
        let mut raw_input = egui::RawInput::default();
        raw_input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(640.0, 480.0),
        ));
        let mut bottom_delta = 0.0;

        let _ = ctx.run(raw_input, |ctx| {
            let before = ctx.available_rect();
            app.render_details_lazy_status_bar(ctx);
            bottom_delta = before.bottom() - ctx.available_rect().bottom();
        });

        assert!(bottom_delta >= 24.5);
    }

    #[test]
    fn bottom_bar_context_menu_suppression_only_matches_its_own_rect() {
        let rect = egui::Rect::from_min_max(egui::pos2(0.0, 420.0), egui::pos2(640.0, 480.0));
        assert!(selection_info_bar_contains_pos(
            Some(rect),
            Some(egui::pos2(320.0, 450.0))
        ));
        assert!(!selection_info_bar_contains_pos(
            Some(rect),
            Some(egui::pos2(320.0, 200.0))
        ));
        assert!(!selection_info_bar_contains_pos(
            None,
            Some(egui::pos2(320.0, 450.0))
        ));
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
    use crate::app::{GridEditBadges, setup_app_for_test};

    #[test]
    fn selected_ui_scale_menu_button_stays_on_one_row_in_a_narrow_popup() {
        let ctx = egui::Context::default();
        let mut measured = None;
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(120.0, 100.0),
            )),
            ..Default::default()
        };

        let _ = ctx.run(raw_input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                // Narrower than `✓ 100%`: the button must grow horizontally, not vertically.
                ui.set_max_width(36.0);
                let expected_row_height = ui.spacing().interact_size.y;
                let response = ui_scale_menu_button(ui, true, 100);
                measured = Some((
                    response.rect.width(),
                    response.rect.height(),
                    expected_row_height,
                ));
            });
        });

        let (width, height, expected_row_height) = measured.expect("button measurement");
        assert!(width > 36.0, "the popup row should extend horizontally");
        assert!(
            height <= expected_row_height + 0.01,
            "the checked label wrapped: height={height}, row={expected_row_height}"
        );
    }

    fn minimal_details_settings() -> crate::settings::Settings {
        let mut settings = crate::settings::Settings::default();
        settings.details_show_preview = false;
        settings.details_show_rating = false;
        settings.details_show_tags = false;
        settings.details_show_kind = false;
        settings.details_show_page_count = false;
        settings.details_show_modified = false;
        settings.details_show_created = false;
        settings.details_show_state = false;
        settings.details_show_image_dimensions = false;
        settings.details_show_video_duration = false;
        settings.details_show_video_dimensions = false;
        settings.details_show_video_codec = false;
        settings
    }

    fn best_fit_test_key(total_rows: usize) -> DetailsBestFitJobKey {
        DetailsBestFitJobKey {
            items_generation: 7,
            order_revision: 11,
            content_revision: 13,
            total_rows,
            column: DetailsColumn::Name,
            column_set: DetailsColumnSet::Details,
            view_kind: DetailsBestFitViewKind::Normal,
            book_sort_locked: false,
            header_title: "名前".to_owned(),
            button_font: egui::FontId::proportional(14.0),
            body_font: egui::FontId::proportional(14.0),
            pixels_per_point_bits: 1.0_f32.to_bits(),
            ui_font: crate::settings::UiFontSettings::default(),
        }
    }

    fn best_fit_test_job(total_rows: usize) -> DetailsBestFitJob {
        DetailsBestFitJob {
            key: best_fit_test_key(total_rows),
            next_row: 0,
            widest: 0.0,
            started: std::time::Instant::now(),
            measured_rows: 0,
            batches: 0,
        }
    }

    #[test]
    fn details_fixed_best_fit_seeds_track_bounded_markers() {
        let GridEditBadges {
            page_override,
            local_adjust,
            mask,
            conceal,
            comic,
            rotation,
        } = GridEditBadges::default();
        let edit_badge_field_count =
            [page_override, local_adjust, mask, conceal, comic, rotation].len();
        let pin_badge_count = 1;

        assert_eq!(
            DETAILS_STATE_BEST_FIT_SEED.split_whitespace().count(),
            edit_badge_field_count + pin_badge_count,
            "バッジ種別を増減したら状態列のシード文字列と既定幅を一緒に見直す"
        );
        assert_eq!(DETAILS_RATING_BEST_FIT_SEED.chars().count(), 5);
    }

    #[test]
    fn details_column_menu_heading_covers_every_mode_and_origin() {
        let origins = [
            DetailsColumnMenuOrigin::DetailsListHeader,
            DetailsColumnMenuOrigin::ThumbnailSelectionBar,
            DetailsColumnMenuOrigin::DetailsSelectionBar,
        ];
        for mode in [
            DetailsSelectionBarMode::SameAsDetails,
            DetailsSelectionBarMode::Unknown,
        ] {
            for origin in origins {
                assert_eq!(
                    details_column_menu_heading(mode, origin),
                    "一覧と下部情報バー共通"
                );
            }
        }
        for mode in [
            DetailsSelectionBarMode::Dedicated,
            DetailsSelectionBarMode::Hidden,
        ] {
            assert_eq!(
                details_column_menu_heading(mode, DetailsColumnMenuOrigin::DetailsListHeader),
                "一覧・サムネイル表示の下部情報バー"
            );
            assert_eq!(
                details_column_menu_heading(mode, DetailsColumnMenuOrigin::ThumbnailSelectionBar,),
                "一覧・サムネイル表示の下部情報バー"
            );
            assert_eq!(
                details_column_menu_heading(mode, DetailsColumnMenuOrigin::DetailsSelectionBar),
                "詳細表示の下部情報バー専用"
            );
        }
    }

    #[test]
    fn shared_preview_hover_explains_only_details_bottom_bar_behavior() {
        assert_eq!(
            details_shared_preview_hover_text(
                DetailsColumnSet::SharedBar,
                DetailsColumnMenuOrigin::DetailsSelectionBar,
            ),
            Some("この設定は一覧側に反映されます。下部バーはプレビューを表示しません")
        );
        assert_eq!(
            details_shared_preview_hover_text(
                DetailsColumnSet::Details,
                DetailsColumnMenuOrigin::DetailsListHeader,
            ),
            None
        );
        // サムネイル一覧の下部バーも同じ説明を出す。ここが漏れていて、実機で
        // 「サムネイル一覧側だけ説明が出ない」状態になっていた。
        assert_eq!(
            details_shared_preview_hover_text(
                DetailsColumnSet::SharedBar,
                DetailsColumnMenuOrigin::ThumbnailSelectionBar,
            ),
            Some("この設定は一覧側に反映されます。下部バーはプレビューを表示しません")
        );
        // 専用設定はチェック自体が押せないので、無効時の説明が別に出る。
        assert_eq!(
            details_shared_preview_hover_text(
                DetailsColumnSet::DedicatedBar,
                DetailsColumnMenuOrigin::DetailsSelectionBar,
            ),
            None
        );
    }

    #[test]
    fn details_column_menu_targets_left_set_and_keeps_format_shared() {
        let mut settings = crate::settings::Settings::default();
        settings.details_show_tags = false;
        settings.details_show_size = true;
        settings.details_selection_bar_show_tags = false;
        settings.details_selection_bar_show_size = false;
        settings.details_selection_bar_name_width_auto = false;
        settings.details_size_display_mode = DetailsSizeDisplayMode::FixedKb;
        settings.details_timestamp_show_seconds = true;
        settings.details_row_style = DetailsRowStyle::Stripe;

        let mut state =
            DetailsColumnMenuState::from_settings(&settings, DetailsColumnSet::DedicatedBar);
        assert!(!state.show_tags);
        assert!(!state.show_size);
        assert!(!state.name_width_auto);
        assert_eq!(state.size_display_mode, DetailsSizeDisplayMode::FixedKb);
        assert!(state.timestamp_show_seconds);
        assert_eq!(state.row_style, DetailsRowStyle::Stripe);

        state.show_tags = true;
        state.show_created = true;
        state.name_width_auto = true;
        state.size_display_mode = DetailsSizeDisplayMode::FixedMb;
        state.timestamp_show_seconds = false;
        state.row_style = DetailsRowStyle::Plain;
        let changes = state.apply(&mut settings, DetailsColumnSet::DedicatedBar, 321.0);

        assert!(changes.columns);
        assert!(changes.lazy_columns);
        assert!(changes.format);
        assert!(settings.details_selection_bar_show_tags);
        assert!(settings.details_selection_bar_show_created);
        assert!(settings.details_selection_bar_name_width_auto);
        assert!(!settings.details_show_tags, "セット A の左列は変更しない");
        assert!(
            !settings.details_show_created,
            "セット A の左列は変更しない"
        );
        assert_eq!(
            settings.details_size_display_mode,
            DetailsSizeDisplayMode::FixedMb,
            "右列の書式は対象がセット C でもセット A へ保存する"
        );
        assert!(!settings.details_timestamp_show_seconds);
        assert_eq!(settings.details_row_style, DetailsRowStyle::Plain);

        settings.details_selection_bar_show_tags = false;
        let mut shared =
            DetailsColumnMenuState::from_settings(&settings, DetailsColumnSet::SharedBar);
        assert!(!shared.show_tags);
        shared.show_tags = true;
        let shared_changes = shared.apply(&mut settings, DetailsColumnSet::SharedBar, 180.0);
        assert!(shared_changes.columns);
        assert!(settings.details_show_tags);
        assert!(
            !settings.details_selection_bar_show_tags,
            "セット A の編集でセット C の左列は変更しない"
        );
    }

    #[test]
    fn details_selection_bar_menu_toggle_copies_when_entering_dedicated() {
        let mut settings = crate::settings::Settings::default();
        settings.details_selection_bar_mode = DetailsSelectionBarMode::SameAsDetails;
        settings.details_show_tags = false;
        settings.details_column_widths = vec![DetailsColumnWidth {
            column: DetailsColumnId::Size,
            width: 234.0,
        }];
        settings.details_selection_bar_show_tags = true;
        settings.details_selection_bar_column_widths.clear();

        assert!(toggle_details_selection_bar_mode_from_menu(&mut settings));
        assert_eq!(
            settings.details_selection_bar_mode,
            DetailsSelectionBarMode::Dedicated
        );
        assert!(!settings.details_selection_bar_show_tags);
        assert_eq!(
            settings.details_selection_bar_column_widths,
            settings.details_column_widths
        );

        assert!(toggle_details_selection_bar_mode_from_menu(&mut settings));
        assert_eq!(
            settings.details_selection_bar_mode,
            DetailsSelectionBarMode::SameAsDetails
        );
        settings.details_column_widths[0].width = 345.0;
        settings.details_selection_bar_column_widths.clear();
        assert!(toggle_details_selection_bar_mode_from_menu(&mut settings));
        assert_eq!(
            settings.details_selection_bar_column_widths, settings.details_column_widths,
            "メニューから専用設定へ入り直すたびセット A を再コピーする"
        );
    }

    #[test]
    fn details_separator_stays_one_physical_pixel() {
        for pixels_per_point in [1.0, 1.25, 1.5, 2.0] {
            let width = details_separator_stroke_width(pixels_per_point);
            assert!(
                (width * pixels_per_point - 1.0).abs() < 0.001,
                "stroke should be one physical pixel at {pixels_per_point}x"
            );
        }
    }

    #[test]
    fn normal_details_text_uses_shared_primary_theme_color() {
        let visuals = egui::Visuals::light();
        assert_eq!(
            details_row_text_color(&visuals, false),
            visuals.text_color()
        );
        assert_eq!(
            details_row_text_color(&visuals, true),
            visuals.selection.stroke.color
        );
    }

    #[test]
    fn details_separator_y_snaps_to_physical_pixel_center() {
        let rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(200.0, 28.0));
        for pixels_per_point in [1.0, 1.25, 1.5, 2.0] {
            let y = details_separator_y(rect, pixels_per_point);
            let physical_y = y * pixels_per_point;
            assert!(
                (physical_y.fract() - 0.5).abs() < 0.001,
                "line center should land on pixel center at {pixels_per_point}x"
            );
        }
    }

    #[test]
    fn details_separator_y_stays_inside_current_row() {
        let rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.3), egui::pos2(200.0, 28.3));
        for pixels_per_point in [1.0, 1.25, 1.5, 2.0] {
            let y = details_separator_y(rect, pixels_per_point);
            assert!(
                y < rect.bottom(),
                "separator should not be painted into the next row at {pixels_per_point}x"
            );
            assert!(
                y >= rect.top(),
                "separator should stay within the current row at {pixels_per_point}x"
            );
        }
    }

    #[test]
    fn details_row_background_prioritizes_interaction_over_stripe() {
        let visuals = egui::Visuals::light();
        let style = DetailsRowStyle::SeparatorAndStripe;
        let striped = details_row_background(&visuals, style, 1, false, false, false);
        assert_ne!(striped, visuals.panel_fill);
        assert_eq!(
            details_row_background(&visuals, style, 1, true, true, true),
            visuals.selection.bg_fill
        );
        assert_eq!(
            details_row_background(&visuals, style, 1, false, true, true),
            visuals.widgets.active.bg_fill
        );
        assert_eq!(
            details_row_background(&visuals, style, 1, false, false, true),
            visuals.widgets.hovered.bg_fill
        );
        assert_eq!(
            details_row_background(&visuals, DetailsRowStyle::Separator, 1, false, false, false),
            visuals.panel_fill
        );
    }

    #[test]
    fn display_only_details_row_ignores_all_interaction_visuals() {
        assert_eq!(
            details_row_visual_state(true, true, true, true),
            DetailsRowVisualState {
                selected: false,
                checked: false,
                hovered: false,
            }
        );
        assert_eq!(
            details_row_visual_state(false, true, true, true),
            DetailsRowVisualState {
                selected: true,
                checked: true,
                hovered: true,
            }
        );
    }

    #[test]
    fn details_layout_overflows_when_saved_width_needs_it() {
        let mut settings = minimal_details_settings();
        assert!(set_details_column_width(
            &mut settings,
            DetailsColumnSet::Details,
            DetailsColumn::Size,
            220.0
        ));

        // avail が狭くても、保存済みの広い列のために pane は名前列既定幅 + その列幅まで広がる。
        let layout = details_layout(200.0, 0.0, 1.0, &settings, DetailsColumnSet::Details);
        assert_eq!(layout.pane_w, DetailsColumn::Name.default_width() + 220.0);
        assert_eq!(layout.extent, layout.pane_w, "gutter 0 なら extent == pane");
    }

    #[test]
    fn details_layout_reserves_gutter_and_avoids_horizontal_scroll() {
        // 名前列が残り幅を埋める通常ケース。縦バー gutter と丸め余白を引いて収める。
        let settings = minimal_details_settings(); // Name + Size(92)
        let gutter = 10.0;
        let layout = details_layout(600.0, gutter, 1.0, &settings, DetailsColumnSet::Details);
        assert!((layout.extent - 599.0).abs() < 0.01);
        assert!(
            (layout.pane_w - (600.0 - gutter - 1.0)).abs() < 0.01,
            "pane は gutter と 1 physical px の余白を引いた幅"
        );
        assert!(
            (layout.name_w - (600.0 - gutter - 1.0 - 92.0)).abs() < 0.01,
            "名前列が残り幅を埋める"
        );
    }

    #[test]
    fn details_layout_without_gutter_leaves_one_physical_pixel() {
        let settings = minimal_details_settings();
        let layout = details_layout(600.0, 0.0, 1.0, &settings, DetailsColumnSet::Details);
        assert!((layout.extent - 599.0).abs() < 0.01);
        assert!((layout.pane_w - 599.0).abs() < 0.01);
    }

    #[test]
    fn details_layout_rounding_slack_covers_pixel_and_header_stroke() {
        let settings = minimal_details_settings();
        for pixels_per_point in [0.8_f32, 0.9, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0] {
            let layout = details_layout(
                600.0,
                10.0,
                pixels_per_point,
                &settings,
                DetailsColumnSet::Details,
            );
            let device_pixel_gap = (600.0 - layout.extent) * pixels_per_point;
            let expected_gap = 1.0_f32.max(0.5 * pixels_per_point);
            assert!(
                (device_pixel_gap - expected_gap).abs() < 0.01,
                "pixels_per_point={pixels_per_point}, gap={device_pixel_gap}, expected={expected_gap}"
            );
            assert!(layout.extent < 600.0);
        }
    }

    #[test]
    fn details_physical_scroll_policy_fits_logged_80_and_90_percent_cases() {
        let settings = minimal_details_settings();
        for (pixels_per_point, avail_w) in [(0.8_f32, 1880.25_f32), (0.9, 1669.5625)] {
            let viewport =
                egui::Rect::from_min_size(egui::pos2(8.0, 100.0), egui::vec2(avail_w, 600.0));
            let layout_avail_w =
                details_horizontal_viewport_capacity(viewport, pixels_per_point).min(avail_w);
            let layout = details_layout(
                layout_avail_w,
                10.0,
                pixels_per_point,
                &settings,
                DetailsColumnSet::Details,
            );
            let policy = details_horizontal_scroll_policy(
                viewport,
                layout.extent,
                layout.columns_w,
                pixels_per_point,
            );
            assert!(
                !policy.overflow,
                "pixels_per_point={pixels_per_point}, policy={policy:?}"
            );
            assert!(policy.required_right_px <= policy.viewport_right_px);
        }
    }

    #[test]
    fn details_physical_scroll_policy_is_conservative_for_fractional_overflow() {
        for native_ppp in [1.0_f32, 1.25, 1.5, 1.75, 2.0] {
            for ui_scale_step in 5..=20 {
                let pixels_per_point = native_ppp * ui_scale_step as f32 / 10.0;
                for left in [0.0_f32, 0.125, 0.5, 8.0, 8.3] {
                    for avail_w in [319.3_f32, 600.25, 1669.5625, 1880.25] {
                        let viewport = egui::Rect::from_min_size(
                            egui::pos2(left, 0.0),
                            egui::vec2(avail_w, 100.0),
                        );
                        let content_origin =
                            details_scroll_content_origin_x(left, pixels_per_point).max(left);
                        let viewport_right_px =
                            physical_pixel_floor(viewport.right() * pixels_per_point);
                        let max_required_extent =
                            viewport_right_px as f32 / pixels_per_point - content_origin;

                        let fit = details_horizontal_scroll_policy(
                            viewport,
                            0.0,
                            max_required_extent - 0.5,
                            pixels_per_point,
                        );
                        assert!(
                            !fit.overflow,
                            "fit must stay visible: ppp={pixels_per_point} viewport={viewport:?} policy={fit:?}"
                        );

                        let overflow = details_horizontal_scroll_policy(
                            viewport,
                            0.0,
                            max_required_extent - 0.5 + 0.1 / pixels_per_point,
                            pixels_per_point,
                        );
                        assert!(
                            overflow.overflow,
                            "fractional physical overflow must scroll: ppp={pixels_per_point} viewport={viewport:?} policy={overflow:?}"
                        );
                        assert!(overflow.scroll_extent >= overflow.required_extent);
                        assert!(
                            physical_pixel_floor(
                                (overflow.content_origin_x + overflow.scroll_extent)
                                    * pixels_per_point
                            ) > overflow.required_right_px,
                            "scroll extent must reach past the required right edge: {overflow:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn details_manual_name_width_is_preserved_and_scrolls_when_needed() {
        let avail_w = 600.25_f32;
        let gutter = 10.0_f32;
        for pixels_per_point in [0.8_f32, 0.9, 1.0, 1.25, 1.5, 2.0] {
            let viewport =
                egui::Rect::from_min_size(egui::pos2(8.0, 0.0), egui::vec2(avail_w, 400.0));
            let mut settings = minimal_details_settings();
            settings.details_name_width_auto = false;
            let fixed = details_fixed_columns_width(&settings, DetailsColumnSet::Details);
            let layout_avail_w =
                details_horizontal_viewport_capacity(viewport, pixels_per_point).min(avail_w);
            let fitting_name =
                layout_avail_w - gutter - details_layout_right_guard(pixels_per_point) - fixed;
            settings.details_name_width = fitting_name;

            let fitting_layout = details_layout(
                layout_avail_w,
                gutter,
                pixels_per_point,
                &settings,
                DetailsColumnSet::Details,
            );
            let fitting_policy = details_horizontal_scroll_policy(
                viewport,
                fitting_layout.extent,
                fitting_layout.columns_w,
                pixels_per_point,
            );
            assert!((fitting_layout.name_w - fitting_name).abs() < 0.001);
            assert!(
                !fitting_policy.overflow,
                "manual fit ppp={pixels_per_point}: {fitting_policy:?}"
            );

            settings.details_name_width = fitting_name + 2.0 / pixels_per_point;
            let overflow_layout = details_layout(
                layout_avail_w,
                gutter,
                pixels_per_point,
                &settings,
                DetailsColumnSet::Details,
            );
            let overflow_policy = details_horizontal_scroll_policy(
                viewport,
                overflow_layout.extent,
                overflow_layout.columns_w,
                pixels_per_point,
            );
            assert!((overflow_layout.name_w - settings.details_name_width).abs() < 0.001);
            assert!(
                overflow_policy.overflow,
                "manual overflow ppp={pixels_per_point}: {overflow_policy:?}"
            );
        }
    }

    #[test]
    fn details_auto_width_never_overflows_the_inward_physical_viewport() {
        let settings = minimal_details_settings();
        for native_ppp in [1.0_f32, 1.25, 1.5, 1.75, 2.0] {
            for ui_scale_step in 5..=20 {
                let pixels_per_point = native_ppp * ui_scale_step as f32 / 10.0;
                for left in [0.0_f32, 0.125, 0.5, 8.0, 8.3] {
                    for avail_w in [319.3_f32, 600.25, 1669.5625, 1880.25] {
                        for gutter in [0.0_f32, 10.0] {
                            let viewport = egui::Rect::from_min_size(
                                egui::pos2(left, 0.0),
                                egui::vec2(avail_w, 100.0),
                            );
                            let layout_avail_w =
                                details_horizontal_viewport_capacity(viewport, pixels_per_point)
                                    .min(avail_w);
                            let layout = details_layout(
                                layout_avail_w,
                                gutter,
                                pixels_per_point,
                                &settings,
                                DetailsColumnSet::Details,
                            );
                            let policy = details_horizontal_scroll_policy(
                                viewport,
                                layout.extent,
                                layout.columns_w,
                                pixels_per_point,
                            );
                            assert!(
                                !policy.overflow,
                                "ppp={pixels_per_point} viewport={viewport:?} gutter={gutter} layout={{columns:{}, pane:{}, extent:{}}} policy={policy:?}",
                                layout.columns_w, layout.pane_w, layout.extent
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn configured_scroll_area_hides_egui_fractional_baseline_overflow() {
        let settings = minimal_details_settings();
        for (pixels_per_point, avail_w) in [(0.8_f32, 1880.25_f32), (0.9, 1669.5625)] {
            let ctx = egui::Context::default();
            // set_zoom_factor は次 pass 冒頭で直前 viewport rect も拡縮する。ここでは
            // ScrollArea の丸めだけを再現したいので、test context の option を先に確定する。
            ctx.options_mut(|options| options.zoom_factor = pixels_per_point);
            let mut raw_input = egui::RawInput::default();
            let viewport_rect =
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(avail_w + 16.0, 240.0));
            raw_input.screen_rect = Some(viewport_rect);
            let root_viewport = raw_input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .expect("root viewport");
            root_viewport.native_pixels_per_point = Some(1.0);
            root_viewport.inner_rect = Some(viewport_rect);
            root_viewport.outer_rect = Some(viewport_rect);
            let mut measured = None;

            let _ = ctx.run(raw_input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let viewport = ui.available_rect_before_wrap();
                    let effective_ppp = ui.ctx().pixels_per_point();
                    let layout_avail_w =
                        details_horizontal_viewport_capacity(viewport, effective_ppp)
                            .min(viewport.width());
                    let layout = details_layout(
                        layout_avail_w,
                        10.0,
                        effective_ppp,
                        &settings,
                        DetailsColumnSet::Details,
                    );
                    let policy = details_horizontal_scroll_policy(
                        viewport,
                        layout.extent,
                        layout.columns_w,
                        effective_ppp,
                    );
                    let output = configured_details_horizontal_scroll_area(policy)
                        .id_salt(("fractional_baseline", pixels_per_point.to_bits()))
                        .show(ui, |ui| {
                            // egui の入れ子 ScrollArea が作った「実描画ではない 1pt 超過」を
                            // 明示的に再現する。policy は列が収まると判定済みなので、この
                            // synthetic content_size がバー表示や offset を発生させてはならない。
                            ui.set_min_width(viewport.width() + 1.0);
                            ui.allocate_exact_size(
                                egui::vec2(layout.pane_w, 80.0),
                                egui::Sense::hover(),
                            );
                        });
                    measured = Some((
                        policy,
                        output.state.offset,
                        output.content_size,
                        output.inner_rect,
                        viewport,
                    ));
                });
            });

            let (policy, offset, content_size, inner_rect, viewport) =
                measured.expect("scroll area rendered");
            assert!(!policy.overflow, "{policy:?}");
            assert_eq!(offset.x, 0.0);
            assert!((inner_rect.height() - viewport.height()).abs() < 0.01);
            assert!(
                content_size.x > inner_rect.width(),
                "test must reproduce egui's synthetic fractional overflow: ppp={pixels_per_point}, content={content_size:?}, inner={inner_rect:?}"
            );
        }
    }

    #[test]
    fn apply_details_best_fit_width_writes_only_the_target_set() {
        let mut app = setup_app_for_test();
        app.settings.details_name_width_auto = true;
        app.settings.details_selection_bar_name_width_auto = true;
        app.settings.details_column_widths.clear();
        app.settings.details_selection_bar_column_widths.clear();
        let ctx = egui::Context::default();

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.apply_details_best_fit_width(
                    ui,
                    DetailsColumnSet::DedicatedBar,
                    DetailsColumn::Name,
                    260.0,
                );
                app.apply_details_best_fit_width(
                    ui,
                    DetailsColumnSet::SharedBar,
                    DetailsColumn::Tags,
                    180.0,
                );
            });
        });

        assert!(app.settings.details_name_width_auto);
        assert!(!app.settings.details_selection_bar_name_width_auto);
        assert!(app.settings.details_selection_bar_name_width > 260.0);
        assert!(app.settings.details_selection_bar_column_widths.is_empty());
        assert!(
            app.settings
                .details_column_widths
                .iter()
                .any(|entry| { entry.column == DetailsColumnId::Tags && entry.width > 180.0 })
        );
    }

    #[test]
    fn selection_bar_only_draw_loop_advances_best_fit_job() {
        let mut app = setup_app_for_test();
        app.settings.grid_view_mode = GridViewMode::Thumbnail;
        let row_count = DETAILS_BEST_FIT_ROWS_PER_FRAME * 3 + 1;
        app.items = (0..row_count)
            .map(|idx| GridItem::Image(PathBuf::from(format!(r"C:\Pictures\row-{idx}.png"))))
            .collect();
        app.image_metas = vec![None; row_count];
        app.visible_indices = (0..row_count).collect();
        let ctx = egui::Context::default();

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.start_details_best_fit_job(
                    ui,
                    DetailsColumn::Name,
                    DetailsColumnSet::SharedBar,
                    false,
                );
            });
        });
        let started_next_row = ctx
            .data(|data| {
                data.get_temp::<Option<DetailsBestFitJob>>(App::details_best_fit_job_id())
                    .flatten()
            })
            .expect("multi-frame job remains after the first batch")
            .next_row;

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(800.0, App::DETAILS_HEADER_H),
                    egui::Sense::hover(),
                );
                app.draw_details_header_static(ui, rect, DetailsColumnSet::SharedBar);
            });
        });
        let advanced_next_row = ctx
            .data(|data| {
                data.get_temp::<Option<DetailsBestFitJob>>(App::details_best_fit_job_id())
                    .flatten()
            })
            .expect("bar-only frame keeps the unfinished job")
            .next_row;

        assert_eq!(started_next_row, DETAILS_BEST_FIT_ROWS_PER_FRAME);
        assert_eq!(advanced_next_row, DETAILS_BEST_FIT_ROWS_PER_FRAME * 2);
    }

    #[test]
    fn details_best_fit_name_width_measures_current_rows() {
        let mut app = setup_app_for_test();
        app.settings.grid_view_mode = GridViewMode::Details;
        app.items = vec![GridItem::Image(PathBuf::from(format!(
            r"C:\Pictures\{}.png",
            "very-wide-file-name-".repeat(8)
        )))];
        app.image_metas = vec![None];
        app.visible_indices = vec![0];
        app.details_order = vec![0];
        let ctx = egui::Context::default();
        let mut raw_input = egui::RawInput::default();
        raw_input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, 200.0),
        ));
        let _ = ctx.run(raw_input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.start_details_best_fit_job(
                    ui,
                    DetailsColumn::Name,
                    DetailsColumnSet::Details,
                    false,
                );
            });
        });

        let width = app.settings.details_name_width;
        assert!(!app.settings.details_name_width_auto);
        assert!(width > DetailsColumn::Name.default_width());
        assert!(width <= DETAILS_BEST_FIT_MAX_WIDTH);
    }

    #[test]
    fn details_best_fit_exact_width_reaches_longest_row_after_first_batch() {
        let mut app = setup_app_for_test();
        app.settings.grid_view_mode = GridViewMode::Details;
        let row_count = DETAILS_BEST_FIT_ROWS_PER_FRAME + 1;
        let longest_name = format!("{}tail.png", "outside-first-batch-".repeat(4));
        app.items = (0..row_count)
            .map(|idx| {
                let name = if idx + 1 == row_count {
                    longest_name.clone()
                } else {
                    format!("short-{idx}.png")
                };
                GridItem::Image(PathBuf::from(format!(r"C:\Pictures\{name}")))
            })
            .collect();
        app.image_metas = vec![None; row_count];
        app.visible_indices = (0..row_count).collect();
        app.details_order = app.visible_indices.clone();
        app.details_order_revision = 1;
        let ctx = egui::Context::default();

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.start_details_best_fit_job(
                    ui,
                    DetailsColumn::Name,
                    DetailsColumnSet::Details,
                    false,
                );
            });
        });
        assert!(
            app.settings.details_name_width_auto,
            "最初の 192 行だけでは未標本の最長行を見ず、確定幅を適用しない"
        );

        let mut expected = 0.0;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                expected = (App::details_best_fit_measure(
                    ui,
                    longest_name.clone(),
                    egui::TextStyle::Body.resolve(ui.style()),
                ) + DETAILS_BEST_FIT_HORIZONTAL_PADDING)
                    .ceil()
                    .clamp(DetailsColumn::Name.min_width(), DETAILS_BEST_FIT_MAX_WIDTH);
                app.advance_details_best_fit_job(ui, false);
            });
        });

        assert!(!app.settings.details_name_width_auto);
        assert!(
            (app.settings.details_name_width - expected).abs() < 0.01,
            "全行完了後は可視範囲外の最長値へ exact に収束する"
        );
    }

    #[test]
    fn details_best_fit_uses_current_filtered_and_sorted_order_exactly() {
        let mut app = setup_app_for_test();
        app.settings.grid_view_mode = GridViewMode::Details;
        let item_count = 221;
        let filtered_longest = format!("{}tail.png", "filtered-long-".repeat(4));
        app.items = (0..item_count)
            .map(|idx| {
                let name = match idx {
                    0 => format!("{}hidden.png", "filtered-out-".repeat(100)),
                    1 => filtered_longest.clone(),
                    _ => format!("short-{idx}.png"),
                };
                GridItem::Image(PathBuf::from(format!(r"C:\Pictures\{name}")))
            })
            .collect();
        app.image_metas = vec![None; item_count];
        app.visible_indices = (1..item_count).collect();
        // ソート後の順序を模し、可視集合内の最長行を 2 batch 目の末尾へ置く。
        app.details_order = app.visible_indices.iter().rev().copied().collect();
        app.details_order_revision = 9;
        let ctx = egui::Context::default();

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.start_details_best_fit_job(
                    ui,
                    DetailsColumn::Name,
                    DetailsColumnSet::Details,
                    false,
                );
            });
        });
        let mut expected = 0.0;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                expected = (App::details_best_fit_measure(
                    ui,
                    filtered_longest.clone(),
                    egui::TextStyle::Body.resolve(ui.style()),
                ) + DETAILS_BEST_FIT_HORIZONTAL_PADDING)
                    .ceil()
                    .clamp(DetailsColumn::Name.min_width(), DETAILS_BEST_FIT_MAX_WIDTH);
                app.advance_details_best_fit_job(ui, false);
            });
        });

        assert!((app.settings.details_name_width - expected).abs() < 0.01);
        assert!(
            app.settings.details_name_width < DETAILS_BEST_FIT_MAX_WIDTH,
            "filter で除外した最長値は測定対象へ戻さない"
        );
    }

    #[test]
    fn details_best_fit_discards_job_when_items_generation_changes_mid_scan() {
        let mut app = setup_app_for_test();
        app.settings.grid_view_mode = GridViewMode::Details;
        let row_count = DETAILS_BEST_FIT_ROWS_PER_FRAME + 10;
        app.items = (0..row_count)
            .map(|idx| GridItem::Image(PathBuf::from(format!(r"C:\Pictures\short-{idx}.png"))))
            .collect();
        app.image_metas = vec![None; row_count];
        app.visible_indices = (0..row_count).collect();
        app.details_order = app.visible_indices.clone();
        app.details_order_revision = 3;
        let ctx = egui::Context::default();

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.start_details_best_fit_job(
                    ui,
                    DetailsColumn::Name,
                    DetailsColumnSet::Details,
                    false,
                );
            });
        });
        app.items_generation = app.items_generation.wrapping_add(1);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.advance_details_best_fit_job(ui, false);
            });
        });

        assert!(app.settings.details_name_width_auto);
        assert!(
            ctx.data(|data| {
                data.get_temp::<Option<DetailsBestFitJob>>(App::details_best_fit_job_id())
                    .flatten()
                    .is_none()
            }),
            "古い generation の job は再投入しない"
        );
    }

    #[test]
    fn details_best_fit_keeps_fixed_samples_and_dynamic_column_scans() {
        let mut app = setup_app_for_test();
        app.settings.grid_view_mode = GridViewMode::Details;
        let ctx = egui::Context::default();
        let mut checked = false;

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rating_key = app.details_best_fit_job_key(
                    ui,
                    DetailsColumn::Rating,
                    DetailsColumnSet::Details,
                    false,
                );
                let (rating_width, rating_dynamic) =
                    app.details_best_fit_seed_width(ui, &rating_key);
                let five_stars = App::details_best_fit_measure(
                    ui,
                    "★★★★★".to_owned(),
                    rating_key.body_font.clone(),
                );
                assert!(!rating_dynamic);
                assert!(rating_width >= five_stars);

                let tags_key = app.details_best_fit_job_key(
                    ui,
                    DetailsColumn::Tags,
                    DetailsColumnSet::Details,
                    false,
                );
                let (_, tags_dynamic) = app.details_best_fit_seed_width(ui, &tags_key);
                assert!(tags_dynamic, "タグは現在一覧の動的値を全行測る");

                app.items_are_bookmark_view = true;
                let bookmark_state_key = app.details_best_fit_job_key(
                    ui,
                    DetailsColumn::State,
                    DetailsColumnSet::Details,
                    false,
                );
                let (_, bookmark_state_dynamic) =
                    app.details_best_fit_seed_width(ui, &bookmark_state_key);
                assert!(
                    bookmark_state_dynamic,
                    "ブックマークの位置は固定語彙でないため全行測る"
                );

                app.items_are_bookmark_view = false;
                app.items_are_reading_history_view = true;
                let history_state_key = app.details_best_fit_job_key(
                    ui,
                    DetailsColumn::State,
                    DetailsColumnSet::Details,
                    false,
                );
                let (_, history_state_dynamic) =
                    app.details_best_fit_seed_width(ui, &history_state_key);
                assert!(
                    history_state_dynamic,
                    "閲覧履歴の閲覧位置は固定語彙でないため全行測る"
                );
                checked = true;
            });
        });
        assert!(checked);
    }

    #[test]
    fn details_best_fit_restarts_tags_scan_when_measured_content_changes() {
        let mut app = setup_app_for_test();
        app.settings.grid_view_mode = GridViewMode::Details;
        app.settings.details_show_tags = true;
        let row_count = DETAILS_BEST_FIT_ROWS_PER_FRAME + 1;
        let first_path = PathBuf::from(r"C:\Pictures\tag-0.png");
        app.items = (0..row_count)
            .map(|idx| GridItem::Image(PathBuf::from(format!(r"C:\Pictures\tag-{idx}.png"))))
            .collect();
        app.image_metas = vec![None; row_count];
        app.visible_indices = (0..row_count).collect();
        app.details_order = app.visible_indices.clone();
        app.details_order_revision = 1;
        let ctx = egui::Context::default();

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.start_details_best_fit_job(
                    ui,
                    DetailsColumn::Tags,
                    DetailsColumnSet::Details,
                    false,
                );
            });
        });

        let long_tag = "first-batch-row-received-a-long-tag-after-measurement".to_string();
        let key = crate::tags_db::item_key_for_path(&first_path);
        app.set_tags_cache_entry(key, vec![long_tag.clone()]);

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.advance_details_best_fit_job(ui, false);
            });
        });
        let restarted = ctx
            .data(|data| {
                data.get_temp::<Option<DetailsBestFitJob>>(App::details_best_fit_job_id())
                    .flatten()
            })
            .expect("内容世代変更後は先頭から再走査する");
        assert_eq!(restarted.next_row, 0);
        assert_eq!(
            restarted.key.content_revision,
            app.details_cell_content_revisions.tags
        );

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.advance_details_best_fit_job(ui, false);
            });
        });
        let mut expected = 0.0;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                expected = (App::details_best_fit_measure(
                    ui,
                    long_tag.clone(),
                    egui::TextStyle::Body.resolve(ui.style()),
                ) + DETAILS_BEST_FIT_HORIZONTAL_PADDING)
                    .ceil()
                    .clamp(DetailsColumn::Tags.min_width(), DETAILS_BEST_FIT_MAX_WIDTH);
                app.advance_details_best_fit_job(ui, false);
            });
        });

        assert!(
            (details_column_width(
                &app.settings,
                DetailsColumnSet::Details,
                DetailsColumn::Tags,
            ) - expected)
                .abs()
                < 0.01,
            "再走査後は1 batch目へ遅れて届いたタグも収める"
        );
    }

    #[test]
    fn details_best_fit_restarts_lazy_scan_when_video_codec_arrives() {
        let mut app = setup_app_for_test();
        app.settings.grid_view_mode = GridViewMode::Details;
        app.settings.details_show_video_codec = true;
        let row_count = DETAILS_BEST_FIT_ROWS_PER_FRAME + 1;
        app.items = (0..row_count)
            .map(|idx| GridItem::Video(PathBuf::from(format!(r"C:\Videos\clip-{idx}.mkv"))))
            .collect();
        app.image_metas = vec![Some((1_700_000_000, 4096)); row_count];
        app.visible_indices = (0..row_count).collect();
        app.details_order = app.visible_indices.clone();
        app.details_order_revision = 1;
        let ctx = egui::Context::default();

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.start_details_best_fit_job(
                    ui,
                    DetailsColumn::VideoCodec,
                    DetailsColumnSet::Details,
                    false,
                );
            });
        });

        let codec = "AV1 / unusually-long-worker-result-codec-profile".to_string();
        let key = app.metadata_cache_key(0).unwrap();
        app.details_lazy_meta.insert(
            key,
            crate::app::DetailsLazyMeta {
                source_mtime: 1_700_000_000,
                source_size: 4096,
                video_codec: Some(codec.clone()),
                ..Default::default()
            },
        );
        app.details_cell_content_revisions.video_meta = app
            .details_cell_content_revisions
            .video_meta
            .wrapping_add(1);

        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.advance_details_best_fit_job(ui, false);
                });
            });
        }
        let mut expected = 0.0;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                expected = (App::details_best_fit_measure(
                    ui,
                    codec.clone(),
                    egui::TextStyle::Body.resolve(ui.style()),
                ) + DETAILS_BEST_FIT_HORIZONTAL_PADDING)
                    .ceil()
                    .clamp(
                        DetailsColumn::VideoCodec.min_width(),
                        DETAILS_BEST_FIT_MAX_WIDTH,
                    );
                app.advance_details_best_fit_job(ui, false);
            });
        });

        assert!(
            (details_column_width(
                &app.settings,
                DetailsColumnSet::Details,
                DetailsColumn::VideoCodec,
            ) - expected)
                .abs()
                < 0.01,
            "遅延メタ到着前後の幅を混ぜず、安定世代を全行再走査する"
        );
    }

    #[test]
    fn details_best_fit_ignores_unrelated_column_content_revision() {
        let mut app = setup_app_for_test();
        app.settings.grid_view_mode = GridViewMode::Details;
        app.settings.details_show_tags = true;
        let row_count = DETAILS_BEST_FIT_ROWS_PER_FRAME + 1;
        app.items = (0..row_count)
            .map(|idx| GridItem::Image(PathBuf::from(format!(r"C:\Pictures\tag-{idx}.png"))))
            .collect();
        app.image_metas = vec![None; row_count];
        app.visible_indices = (0..row_count).collect();
        app.details_order = app.visible_indices.clone();
        app.details_order_revision = 1;
        let ctx = egui::Context::default();

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.start_details_best_fit_job(
                    ui,
                    DetailsColumn::Tags,
                    DetailsColumnSet::Details,
                    false,
                );
            });
        });
        app.details_cell_content_revisions.video_meta = app
            .details_cell_content_revisions
            .video_meta
            .wrapping_add(1);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.advance_details_best_fit_job(ui, false);
            });
        });

        assert!(ctx.data(|data| {
            data.get_temp::<Option<DetailsBestFitJob>>(App::details_best_fit_job_id())
                .flatten()
                .is_none()
        }));
    }

    #[test]
    fn details_best_fit_bookmark_state_measures_long_missing_page_position() {
        let mut app = setup_app_for_test();
        app.settings.grid_view_mode = GridViewMode::Details;
        app.settings.details_show_state = true;
        app.items_are_bookmark_view = true;
        let path = PathBuf::from(r"C:\Books\many-pages");
        let row = crate::bookmark_browser::BookmarkBrowserRow {
            source: crate::bookmark_browser::BookmarkRowSource::Book(
                crate::book_bookmarks::BookBookmark {
                    id: 1,
                    container_key: crate::adjustment_db::normalize_path(&path),
                    container_path: path.clone(),
                    container_kind: crate::book_bookmarks::BookContainerKind::ImageFolder,
                    page_identity: crate::book_bookmarks::PageIdentity::RelativePath(
                        "12345.png".to_string(),
                    ),
                    page_index_hint: 12_344,
                    created_at_ms: 1_700_000_000_000,
                    title: None,
                },
            ),
            item: GridItem::Folder(path),
            relative_page_provenance: None,
            image_meta: None,
            marker_thumbnail: None,
            created_at_ms: 1_700_000_000_000,
            missing: true,
        };
        let state = row.position_label();
        app.items = vec![row.item.clone()];
        app.bookmark_browser_rows = vec![row];
        app.image_metas = vec![None];
        app.visible_indices = vec![0];
        app.details_order = vec![0];
        let ctx = egui::Context::default();
        let mut measured = 0.0;

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                measured = App::details_best_fit_measure(
                    ui,
                    state.clone(),
                    egui::TextStyle::Body.resolve(ui.style()),
                );
                app.start_details_best_fit_job(
                    ui,
                    DetailsColumn::State,
                    DetailsColumnSet::Details,
                    false,
                );
            });
        });

        assert!(state.contains("12345 ページ"));
        assert!(state.ends_with("見つかりません"));
        assert!(
            details_column_width(
                &app.settings,
                DetailsColumnSet::Details,
                DetailsColumn::State,
            ) >= (measured + DETAILS_BEST_FIT_HORIZONTAL_PADDING).ceil()
        );
    }

    #[test]
    fn details_best_fit_reading_history_state_measures_large_progress() {
        let mut app = setup_app_for_test();
        app.settings.grid_view_mode = GridViewMode::Details;
        app.settings.details_show_state = true;
        app.items_are_reading_history_view = true;
        let path = PathBuf::from(r"C:\Books\long.pdf");
        let item = GridItem::PdfFile(path.clone());
        let entry = crate::reading_history_db::ReadingHistoryEntry::new(
            path,
            crate::reading_history_db::ReadingHistoryKind::Pdf,
            None,
            "long".to_string(),
            Some(12_345),
            Some(123_456),
        );
        app.reading_history_rows.insert(entry.key.clone(), entry);
        app.items = vec![item];
        app.image_metas = vec![None];
        app.visible_indices = vec![0];
        app.details_order = vec![0];
        let ctx = egui::Context::default();
        let state = "12345 / 123456".to_string();
        let mut measured = 0.0;

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                measured = App::details_best_fit_measure(
                    ui,
                    state.clone(),
                    egui::TextStyle::Body.resolve(ui.style()),
                );
                app.start_details_best_fit_job(
                    ui,
                    DetailsColumn::State,
                    DetailsColumnSet::Details,
                    false,
                );
            });
        });

        assert_eq!(app.details_state_text(0), state);
        assert!(
            details_column_width(
                &app.settings,
                DetailsColumnSet::Details,
                DetailsColumn::State,
            ) >= (measured + DETAILS_BEST_FIT_HORIZONTAL_PADDING).ceil()
        );
    }

    #[test]
    fn bottom_bar_columns_follow_details_order_and_omit_preview() {
        let mut settings = minimal_details_settings();
        settings.details_show_preview = true;
        settings.details_show_kind = true;
        settings.details_column_order = vec![
            DetailsColumnId::Kind,
            DetailsColumnId::Preview,
            DetailsColumnId::Size,
            DetailsColumnId::Name,
        ];

        assert_eq!(
            details_visible_columns(&settings, DetailsColumnSet::SharedBar),
            vec![
                DetailsColumn::Kind,
                DetailsColumn::Size,
                DetailsColumn::Name
            ]
        );
    }

    fn bottom_bar_layout_settings(
        show_preview: bool,
        name_width_auto: bool,
    ) -> crate::settings::Settings {
        let mut settings = minimal_details_settings();
        settings.details_show_preview = show_preview;
        settings.details_show_size = true;
        settings.details_column_order = vec![
            DetailsColumnId::Preview,
            DetailsColumnId::Name,
            DetailsColumnId::Size,
        ];
        settings.details_name_width_auto = name_width_auto;
        // Fixed mode intentionally overflows the 600 px fixture only when preview is ON.
        // This distinguishes a preview-sized trailing gap from ordinary spare pane width.
        settings.details_name_width = 500.0;
        settings.copy_details_columns_to_selection_bar();
        settings
    }

    fn bottom_bar_surface_modes() -> [(GridViewMode, DetailsSelectionBarMode); 3] {
        [
            (GridViewMode::Details, DetailsSelectionBarMode::Dedicated),
            (
                GridViewMode::Details,
                DetailsSelectionBarMode::SameAsDetails,
            ),
            (
                GridViewMode::Thumbnail,
                DetailsSelectionBarMode::SameAsDetails,
            ),
        ]
    }

    fn bottom_bar_alignment_settings() -> crate::settings::Settings {
        let mut settings = minimal_details_settings();
        settings.grid_view_mode = GridViewMode::Details;
        settings.details_selection_bar_mode = DetailsSelectionBarMode::SameAsDetails;
        settings.details_show_preview = true;
        settings.details_show_kind = true;
        settings.details_show_size = true;
        settings.details_show_modified = true;
        settings.details_column_order = vec![
            DetailsColumnId::Preview,
            DetailsColumnId::Name,
            DetailsColumnId::Kind,
            DetailsColumnId::Size,
            DetailsColumnId::Modified,
        ];
        settings
    }

    fn details_list_fixture_resolution(
        settings: &crate::settings::Settings,
        row_count: usize,
    ) -> DetailsListLayoutResolution {
        let source_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 300.0));
        resolve_details_list_layout(
            source_rect,
            300.0,
            egui::Style::default().spacing.item_spacing.y,
            row_count as f32 * App::DETAILS_ROW_H,
            1.0,
            settings,
        )
    }

    fn layout_column_rects(
        layout: DetailsLayout,
        settings: &crate::settings::Settings,
        column_set: DetailsColumnSet,
    ) -> Vec<(DetailsColumn, egui::Rect)> {
        details_column_rects_for_columns(
            egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(layout.pane_w, App::DETAILS_ROW_H),
            ),
            settings,
            column_set,
        )
    }

    fn column_rect(rects: &[(DetailsColumn, egui::Rect)], column: DetailsColumn) -> egui::Rect {
        rects
            .iter()
            .find_map(|(candidate, rect)| (*candidate == column).then_some(*rect))
            .unwrap()
    }

    fn assert_shared_bar_columns_align(
        settings: &crate::settings::Settings,
        resolved: DetailsListLayoutResolution,
    ) {
        let bar_gutter = selection_info_bottom_bar_gutter(GridViewMode::Details, resolved.gutter);
        let bar_layout = details_layout(
            600.0,
            bar_gutter,
            1.0,
            settings,
            DetailsColumnSet::SharedBar,
        );
        let list_rects = layout_column_rects(resolved.layout, settings, DetailsColumnSet::Details);
        let bar_rects = layout_column_rects(bar_layout, settings, DetailsColumnSet::SharedBar);
        for (column, bar_rect) in bar_rects {
            if column == DetailsColumn::Name {
                continue;
            }
            let list_rect = column_rect(&list_rects, column);
            assert_eq!(bar_rect, list_rect, "{column:?}");
        }
    }

    #[test]
    fn details_bottom_bar_columns_align_with_scrolling_list() {
        let settings = bottom_bar_alignment_settings();
        let resolved = details_list_fixture_resolution(&settings, 20);

        assert!(resolved.needs_vscroll);
        assert_eq!(resolved.gutter, details_scroll_style().allocated_width());
        assert_shared_bar_columns_align(&settings, resolved);
    }

    #[test]
    fn details_bottom_bar_columns_align_with_short_list_without_gutter() {
        let settings = bottom_bar_alignment_settings();
        let resolved = details_list_fixture_resolution(&settings, 2);

        assert!(!resolved.needs_vscroll);
        assert_eq!(resolved.gutter, 0.0);
        assert_shared_bar_columns_align(&settings, resolved);
    }

    #[test]
    fn details_bottom_bar_tracks_list_gutter_when_rows_grow() {
        let settings = bottom_bar_alignment_settings();
        let short = details_list_fixture_resolution(&settings, 2);
        let long = details_list_fixture_resolution(&settings, 20);
        let short_bar = details_layout(
            600.0,
            selection_info_bottom_bar_gutter(GridViewMode::Details, short.gutter),
            1.0,
            &settings,
            DetailsColumnSet::SharedBar,
        );
        let long_bar = details_layout(
            600.0,
            selection_info_bottom_bar_gutter(GridViewMode::Details, long.gutter),
            1.0,
            &settings,
            DetailsColumnSet::SharedBar,
        );
        let short_list_x = column_rect(
            &layout_column_rects(short.layout, &settings, DetailsColumnSet::Details),
            DetailsColumn::Size,
        )
        .left();
        let long_list_x = column_rect(
            &layout_column_rects(long.layout, &settings, DetailsColumnSet::Details),
            DetailsColumn::Size,
        )
        .left();
        let short_bar_x = column_rect(
            &layout_column_rects(short_bar, &settings, DetailsColumnSet::SharedBar),
            DetailsColumn::Size,
        )
        .left();
        let long_bar_x = column_rect(
            &layout_column_rects(long_bar, &settings, DetailsColumnSet::SharedBar),
            DetailsColumn::Size,
        )
        .left();

        assert_eq!(short_list_x, short_bar_x);
        assert_eq!(long_list_x, long_bar_x);
        assert_eq!(long_list_x - short_list_x, long_bar_x - short_bar_x);
        assert_eq!(
            long_list_x - short_list_x,
            -details_scroll_style().allocated_width()
        );
    }

    #[test]
    fn thumbnail_bottom_bar_never_reserves_details_gutter() {
        let settings = bottom_bar_alignment_settings();
        let short = details_list_fixture_resolution(&settings, 2);
        let long = details_list_fixture_resolution(&settings, 20);
        let short_gutter = selection_info_bottom_bar_gutter(GridViewMode::Thumbnail, short.gutter);
        let long_gutter = selection_info_bottom_bar_gutter(GridViewMode::Thumbnail, long.gutter);

        assert_eq!(short_gutter, 0.0);
        assert_eq!(long_gutter, 0.0);
        let short_layout = details_layout(
            600.0,
            short_gutter,
            1.0,
            &settings,
            DetailsColumnSet::SharedBar,
        );
        let long_layout = details_layout(
            600.0,
            long_gutter,
            1.0,
            &settings,
            DetailsColumnSet::SharedBar,
        );
        assert_eq!(
            layout_column_rects(short_layout, &settings, DetailsColumnSet::SharedBar),
            layout_column_rects(long_layout, &settings, DetailsColumnSet::SharedBar)
        );
    }

    #[test]
    fn dedicated_details_bottom_bar_right_edge_matches_list_gutter() {
        let mut settings = bottom_bar_alignment_settings();
        settings.copy_details_columns_to_selection_bar();
        settings.details_selection_bar_mode = DetailsSelectionBarMode::Dedicated;
        settings.details_selection_bar_show_kind = false;
        settings.details_selection_bar_show_modified = false;
        settings.details_selection_bar_column_order =
            vec![DetailsColumnId::Name, DetailsColumnId::Size];
        let resolved = details_list_fixture_resolution(&settings, 20);
        let bar_layout = details_layout(
            600.0,
            selection_info_bottom_bar_gutter(GridViewMode::Details, resolved.gutter),
            1.0,
            &settings,
            DetailsColumnSet::DedicatedBar,
        );

        assert_eq!(resolved.gutter, details_scroll_style().allocated_width());
        assert_eq!(bar_layout.pane_w, resolved.layout.pane_w);
        assert_eq!(bar_layout.extent, resolved.layout.extent);
    }

    #[test]
    fn bottom_bar_text_columns_keep_details_widths() {
        for name_width_auto in [true, false] {
            for show_preview in [false, true] {
                let mut settings = minimal_details_settings();
                settings.grid_view_mode = GridViewMode::Details;
                settings.details_selection_bar_mode = DetailsSelectionBarMode::SameAsDetails;
                settings.details_name_width_auto = name_width_auto;
                settings.details_show_preview = show_preview;
                let full_layout =
                    details_layout(600.0, 0.0, 1.0, &settings, DetailsColumnSet::SharedBar);
                let text_width = details_content_width_for_column_set(full_layout.pane_w);
                let full_rect = egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(full_layout.pane_w, 24.0),
                );
                let text_rect =
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(text_width, 24.0));
                let full = details_column_rects(full_rect, &settings);
                let text = details_column_rects_for_columns(
                    text_rect,
                    &settings,
                    DetailsColumnSet::SharedBar,
                );

                for (column, rect) in text {
                    let full_rect = full
                        .iter()
                        .find(|(candidate, _)| *candidate == column)
                        .map(|(_, rect)| *rect)
                        .unwrap();
                    let expected_width = if column == DetailsColumn::Name && show_preview {
                        full_rect.width()
                            + details_column_width(
                                &settings,
                                DetailsColumnSet::Details,
                                DetailsColumn::Preview,
                            )
                    } else {
                        full_rect.width()
                    };
                    assert!(
                        (rect.width() - expected_width).abs() < 0.01,
                        "{column:?}, auto={name_width_auto}, preview={show_preview}"
                    );
                    if column != DetailsColumn::Name {
                        assert_eq!(rect.left(), full_rect.left());
                    }
                }
            }
        }
    }

    #[test]
    fn bottom_bar_preview_does_not_leave_trailing_space_across_surfaces() {
        for name_width_auto in [true, false] {
            for (grid_view_mode, selection_bar_mode) in bottom_bar_surface_modes() {
                let mut settings = bottom_bar_layout_settings(true, name_width_auto);
                settings.grid_view_mode = grid_view_mode;
                settings.details_selection_bar_mode = selection_bar_mode;
                let list_layout =
                    details_layout(600.0, 0.0, 1.0, &settings, DetailsColumnSet::Details);
                let list_rects = details_column_rects_for_columns(
                    egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(list_layout.pane_w, 24.0),
                    ),
                    &settings,
                    DetailsColumnSet::Details,
                );
                let column_set = selection_info_bottom_bar_column_set(&settings);
                let layout = details_layout(600.0, 0.0, 1.0, &settings, column_set);
                let content_width = details_content_width_for_column_set(layout.pane_w);
                assert_eq!(content_width, layout.pane_w);
                let rects = details_column_rects_for_columns(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(content_width, 24.0)),
                    &settings,
                    column_set,
                );
                let list_size = list_rects
                    .iter()
                    .find(|(column, _)| *column == DetailsColumn::Size)
                    .unwrap()
                    .1;
                let bar_size = rects
                    .iter()
                    .find(|(column, _)| *column == DetailsColumn::Size)
                    .unwrap()
                    .1;
                assert_eq!(bar_size, list_size);
                assert!(
                    (rects.last().unwrap().1.right() - content_width).abs() < 0.01,
                    "{grid_view_mode:?}, {selection_bar_mode:?}, auto={name_width_auto}"
                );
            }
        }
    }

    #[test]
    fn bottom_bar_preview_off_keeps_existing_layout_across_surfaces() {
        for name_width_auto in [true, false] {
            for (grid_view_mode, selection_bar_mode) in bottom_bar_surface_modes() {
                let mut settings = bottom_bar_layout_settings(false, name_width_auto);
                settings.grid_view_mode = grid_view_mode;
                settings.details_selection_bar_mode = selection_bar_mode;
                let list_layout =
                    details_layout(600.0, 0.0, 1.0, &settings, DetailsColumnSet::Details);
                let list = details_column_rects_for_columns(
                    egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(list_layout.pane_w, 24.0),
                    ),
                    &settings,
                    DetailsColumnSet::Details,
                );
                let column_set = selection_info_bottom_bar_column_set(&settings);
                let bar_layout = details_layout(600.0, 0.0, 1.0, &settings, column_set);
                let content_width = details_content_width_for_column_set(bar_layout.pane_w);
                let bar = details_column_rects_for_columns(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(content_width, 24.0)),
                    &settings,
                    column_set,
                );
                assert_eq!(
                    bar, list,
                    "{grid_view_mode:?}, {selection_bar_mode:?}, auto={name_width_auto}"
                );
            }
        }
    }

    #[test]
    fn details_selection_bar_mode_selects_shared_or_dedicated_visible_columns() {
        let mut settings = minimal_details_settings();
        settings.grid_view_mode = GridViewMode::Details;
        settings.details_show_size = true;
        settings.details_column_order = vec![DetailsColumnId::Size, DetailsColumnId::Name];

        settings.details_selection_bar_show_preview = false;
        settings.details_selection_bar_show_rating = false;
        settings.details_selection_bar_show_tags = false;
        settings.details_selection_bar_show_kind = true;
        settings.details_selection_bar_show_page_count = false;
        settings.details_selection_bar_show_size = false;
        settings.details_selection_bar_show_modified = false;
        settings.details_selection_bar_show_created = false;
        settings.details_selection_bar_show_state = false;
        settings.details_selection_bar_show_image_dimensions = false;
        settings.details_selection_bar_show_video_duration = false;
        settings.details_selection_bar_show_video_dimensions = false;
        settings.details_selection_bar_show_video_codec = false;
        settings.details_selection_bar_column_order =
            vec![DetailsColumnId::Kind, DetailsColumnId::Name];

        settings.details_selection_bar_mode = DetailsSelectionBarMode::SameAsDetails;
        let shared_set = selection_info_bottom_bar_column_set(&settings);
        assert_eq!(shared_set, DetailsColumnSet::SharedBar);
        assert_eq!(
            details_visible_columns(&settings, shared_set),
            vec![DetailsColumn::Size, DetailsColumn::Name]
        );

        settings.details_selection_bar_mode = DetailsSelectionBarMode::Dedicated;
        let dedicated_set = selection_info_bottom_bar_column_set(&settings);
        assert_eq!(dedicated_set, DetailsColumnSet::DedicatedBar);
        assert_eq!(
            details_visible_columns(&settings, dedicated_set),
            vec![DetailsColumn::Kind, DetailsColumn::Name]
        );

        settings.grid_view_mode = GridViewMode::Thumbnail;
        assert_eq!(
            selection_info_bottom_bar_column_set(&settings),
            DetailsColumnSet::SharedBar,
            "thumbnail bars always keep set A"
        );
    }

    #[test]
    fn details_selection_bar_rects_use_mode_specific_order_and_widths() {
        let mut settings = minimal_details_settings();
        settings.grid_view_mode = GridViewMode::Details;
        settings.details_show_size = true;
        settings.details_column_order = vec![DetailsColumnId::Size, DetailsColumnId::Name];
        settings.details_column_widths = vec![DetailsColumnWidth {
            column: DetailsColumnId::Size,
            width: 111.0,
        }];
        settings.details_name_width_auto = false;
        settings.details_name_width = 150.0;

        settings.details_selection_bar_show_preview = false;
        settings.details_selection_bar_show_rating = false;
        settings.details_selection_bar_show_tags = false;
        settings.details_selection_bar_show_kind = true;
        settings.details_selection_bar_show_page_count = false;
        settings.details_selection_bar_show_size = false;
        settings.details_selection_bar_show_modified = false;
        settings.details_selection_bar_show_created = false;
        settings.details_selection_bar_show_state = false;
        settings.details_selection_bar_show_image_dimensions = false;
        settings.details_selection_bar_show_video_duration = false;
        settings.details_selection_bar_show_video_dimensions = false;
        settings.details_selection_bar_show_video_codec = false;
        settings.details_selection_bar_column_order =
            vec![DetailsColumnId::Kind, DetailsColumnId::Name];
        settings.details_selection_bar_column_widths = vec![DetailsColumnWidth {
            column: DetailsColumnId::Kind,
            width: 222.0,
        }];
        settings.details_selection_bar_name_width_auto = false;
        settings.details_selection_bar_name_width = 333.0;

        settings.details_selection_bar_mode = DetailsSelectionBarMode::SameAsDetails;
        let shared = details_column_rects_for_columns(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(261.0, 24.0)),
            &settings,
            selection_info_bottom_bar_column_set(&settings),
        );
        assert_eq!(
            shared.iter().map(|(column, _)| *column).collect::<Vec<_>>(),
            vec![DetailsColumn::Size, DetailsColumn::Name]
        );
        assert!((shared[0].1.width() - 111.0).abs() < 0.01);
        assert!((shared[1].1.width() - 150.0).abs() < 0.01);

        settings.details_selection_bar_mode = DetailsSelectionBarMode::Dedicated;
        let dedicated = details_column_rects_for_columns(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(555.0, 24.0)),
            &settings,
            selection_info_bottom_bar_column_set(&settings),
        );
        assert_eq!(
            dedicated
                .iter()
                .map(|(column, _)| *column)
                .collect::<Vec<_>>(),
            vec![DetailsColumn::Kind, DetailsColumn::Name]
        );
        assert!((dedicated[0].1.width() - 222.0).abs() < 0.01);
        assert!((dedicated[1].1.width() - 333.0).abs() < 0.01);
    }

    #[test]
    fn hidden_details_selection_bar_suppresses_lazy_column_requests() {
        let mut settings = minimal_details_settings();
        settings.selection_info_display_mode = crate::settings::SelectionInfoDisplayMode::BottomBar;
        settings.grid_view_mode = GridViewMode::Details;
        settings.details_show_size = true;
        settings.details_selection_bar_show_size = true;
        settings.details_selection_bar_mode = DetailsSelectionBarMode::Hidden;

        assert!(!selection_info_bottom_bar_shows_column(
            &settings,
            DetailsColumn::Size
        ));

        settings.grid_view_mode = GridViewMode::Thumbnail;
        assert!(selection_info_bottom_bar_shows_column(
            &settings,
            DetailsColumn::Size
        ));
        assert!(!selection_info_bottom_bar_shows_column(
            &settings,
            DetailsColumn::Preview
        ));
    }

    #[test]
    fn details_layout_fixed_name_overflows_into_horizontal_scroll() {
        let mut settings = minimal_details_settings();
        settings.details_name_width_auto = false;
        settings.details_name_width = 500.0;
        let layout = details_layout(300.0, 10.0, 1.0, &settings, DetailsColumnSet::Details);
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
        let layout = details_layout(600.0, 10.0, 1.0, &settings, DetailsColumnSet::Details);
        assert!((layout.name_w - 120.0).abs() < 0.01);
        let columns_w =
            layout.name_w + details_fixed_columns_width(&settings, DetailsColumnSet::Details);
        assert!(
            columns_w < layout.pane_w,
            "固定名前列が pane より狭いと右側に余白が残る"
        );
        assert!((layout.extent - 599.0).abs() < 0.01, "横スクロールは不要");
    }

    #[test]
    fn details_best_fit_small_list_completes_in_one_bounded_batch() {
        let key = best_fit_test_key(100);
        let mut job = best_fit_test_job(100);
        assert_eq!(
            job.next_batch(&key),
            DetailsBestFitBatch::Measure {
                range: 0..100,
                completes_job: true,
            }
        );
    }

    #[test]
    fn details_best_fit_stable_content_generation_visits_every_row_once_with_a_per_frame_cap() {
        let key = best_fit_test_key(1_003);
        let mut job = best_fit_test_job(1_003);
        let mut visited = Vec::new();
        loop {
            match job.next_batch(&key) {
                DetailsBestFitBatch::Measure {
                    range,
                    completes_job,
                } => {
                    assert!(range.len() <= DETAILS_BEST_FIT_ROWS_PER_FRAME);
                    visited.extend(range);
                    if completes_job {
                        break;
                    }
                }
                other => panic!("unexpected transition: {other:?}"),
            }
        }
        assert_eq!(visited, (0..1_003).collect::<Vec<_>>());
    }

    #[test]
    fn details_best_fit_all_jobs_share_one_batch_budget_per_frame() {
        let mut budget = DetailsBestFitFrameBudget::default();
        assert!(budget.claim(10));
        assert!(
            !budget.claim(10),
            "同じ frame で別の列 job を開始しても 2 batch 目は測らない"
        );
        assert!(budget.claim(11), "次 frame で上限を再び使える");
    }

    #[test]
    fn details_best_fit_job_key_distinguishes_target_column_set() {
        let details = best_fit_test_key(250);
        let mut dedicated = details.clone();
        dedicated.column_set = DetailsColumnSet::DedicatedBar;

        assert_ne!(details, dedicated);
        let mut job = DetailsBestFitJob {
            key: details,
            ..best_fit_test_job(250)
        };
        assert_eq!(job.next_batch(&dedicated), DetailsBestFitBatch::Stale);
    }

    #[test]
    fn details_best_fit_rejects_generation_sort_filter_font_and_dpi_changes() {
        let mut job = best_fit_test_job(250);
        let base = job.key.clone();
        let mut changed = base.clone();
        changed.items_generation += 1;
        assert_eq!(job.next_batch(&changed), DetailsBestFitBatch::Stale);

        changed = base.clone();
        changed.order_revision += 1;
        assert_eq!(job.next_batch(&changed), DetailsBestFitBatch::Stale);

        changed = base.clone();
        changed.content_revision += 1;
        assert_eq!(job.next_batch(&changed), DetailsBestFitBatch::Stale);
        assert!(base.has_restartable_content_update(&changed));

        changed.header_title.push_str(" ...");
        assert!(
            base.has_restartable_content_update(&changed),
            "lazy worker completion may change content revision and loading suffix together"
        );

        changed = base.clone();
        changed.view_kind = DetailsBestFitViewKind::Bookmark;
        assert_eq!(job.next_batch(&changed), DetailsBestFitBatch::Stale);
        assert!(!base.has_restartable_content_update(&changed));

        changed = base.clone();
        changed.column = DetailsColumn::Tags;
        assert_eq!(job.next_batch(&changed), DetailsBestFitBatch::Stale);

        changed = base.clone();
        changed.body_font = egui::FontId::proportional(15.0);
        assert_eq!(job.next_batch(&changed), DetailsBestFitBatch::Stale);

        changed = base;
        changed.pixels_per_point_bits = 1.25_f32.to_bits();
        assert_eq!(job.next_batch(&changed), DetailsBestFitBatch::Stale);
    }

    #[test]
    fn set_details_name_width_switches_to_fixed_and_is_idempotent() {
        let mut settings = crate::settings::Settings::default();
        assert!(settings.details_name_width_auto);
        assert!(set_details_name_width(
            &mut settings,
            DetailsColumnSet::Details,
            210.0
        ));
        assert!(!settings.details_name_width_auto, "固定モードへ切替");
        assert!((settings.details_name_width - 210.0).abs() < 0.01);
        assert!(
            !set_details_name_width(&mut settings, DetailsColumnSet::Details, 210.0),
            "同値なら変更なし"
        );
    }

    #[test]
    fn bottom_bar_effective_name_width_maps_back_to_stored_width() {
        let mut settings = minimal_details_settings();
        settings.details_show_preview = true;
        settings.details_name_width_auto = false;
        settings.details_name_width = 210.0;
        let effective = details_effective_fixed_name_width(&settings, DetailsColumnSet::SharedBar);
        assert_eq!(effective, 244.0);
        assert_eq!(
            details_stored_name_width_from_effective(
                &settings,
                DetailsColumnSet::SharedBar,
                effective,
            ),
            210.0
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
            DetailsColumnSet::Details,
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
            DetailsColumnSet::Details,
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
    fn dedicated_bar_reorder_changes_only_dedicated_order() {
        let mut settings = crate::settings::Settings::default();
        settings.details_column_order = DetailsColumnId::default_order().to_vec();
        settings.details_selection_bar_column_order = DetailsColumnId::default_order().to_vec();
        let details_before = settings.details_column_order.clone();
        let preview_before = settings
            .details_selection_bar_column_order
            .iter()
            .position(|id| *id == DetailsColumnId::Preview);

        assert!(reorder_details_column(
            &mut settings,
            DetailsColumnSet::DedicatedBar,
            DetailsColumn::Size,
            DetailsColumn::Name,
            false,
        ));

        assert_eq!(settings.details_column_order, details_before);
        assert_eq!(
            settings
                .details_selection_bar_column_order
                .iter()
                .position(|id| *id == DetailsColumnId::Preview),
            preview_before,
            "the bar-only reorder must preserve its excluded Preview entry"
        );
        let size_pos = settings
            .details_selection_bar_column_order
            .iter()
            .position(|id| *id == DetailsColumnId::Size)
            .unwrap();
        let name_pos = settings
            .details_selection_bar_column_order
            .iter()
            .position(|id| *id == DetailsColumnId::Name)
            .unwrap();
        assert!(size_pos < name_pos);
    }

    #[test]
    fn shared_bar_reorder_changes_shared_details_order() {
        let mut settings = crate::settings::Settings::default();
        settings.details_column_order = DetailsColumnId::default_order().to_vec();
        settings.details_selection_bar_column_order = DetailsColumnId::default_order().to_vec();
        let dedicated_before = settings.details_selection_bar_column_order.clone();
        let preview_before = settings
            .details_column_order
            .iter()
            .position(|id| *id == DetailsColumnId::Preview);

        assert!(reorder_details_column(
            &mut settings,
            DetailsColumnSet::SharedBar,
            DetailsColumn::Size,
            DetailsColumn::Name,
            false,
        ));

        assert_eq!(
            settings.details_selection_bar_column_order,
            dedicated_before
        );
        assert_eq!(
            settings
                .details_column_order
                .iter()
                .position(|id| *id == DetailsColumnId::Preview),
            preview_before,
            "the shared bar must not relocate the Preview column it does not draw"
        );
        let size_pos = settings
            .details_column_order
            .iter()
            .position(|id| *id == DetailsColumnId::Size)
            .unwrap();
        let name_pos = settings
            .details_column_order
            .iter()
            .position(|id| *id == DetailsColumnId::Name)
            .unwrap();
        assert!(size_pos < name_pos);
    }

    #[test]
    fn details_reorder_same_column_is_a_noop_for_every_column_set() {
        for column_set in [
            DetailsColumnSet::Details,
            DetailsColumnSet::SharedBar,
            DetailsColumnSet::DedicatedBar,
        ] {
            let mut settings = crate::settings::Settings::default();
            settings.details_column_order = DetailsColumnId::default_order().to_vec();
            settings.details_selection_bar_column_order = DetailsColumnId::default_order().to_vec();
            let before = serde_json::to_value(&settings).unwrap();

            assert!(!reorder_details_column(
                &mut settings,
                column_set,
                DetailsColumn::Size,
                DetailsColumn::Size,
                true,
            ));
            assert_eq!(serde_json::to_value(&settings).unwrap(), before);
        }
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

    fn assert_touch_anchor_is_row_snapped(position: GridTouchScrollPosition, row_h: f32) {
        let row = position.anchor_y / row_h;
        assert!((row - row.round()).abs() < 0.0001);
        assert!(position.remainder_y >= 0.0);
        assert!(position.remainder_y < row_h);
    }

    #[test]
    fn grid_touch_scroll_keeps_anchor_snapped_across_fraction_and_rows() {
        let row_h = 100.0;
        let max_offset = 1000.0;
        let cases = [
            (
                apply_grid_touch_scroll_delta(100.0, 0.0, -25.0, row_h, max_offset),
                GridTouchScrollPosition {
                    anchor_y: 100.0,
                    remainder_y: 25.0,
                },
            ),
            (
                apply_grid_touch_scroll_delta(100.0, 80.0, -30.0, row_h, max_offset),
                GridTouchScrollPosition {
                    anchor_y: 200.0,
                    remainder_y: 10.0,
                },
            ),
            (
                apply_grid_touch_scroll_delta(0.0, 0.0, -250.0, row_h, max_offset),
                GridTouchScrollPosition {
                    anchor_y: 200.0,
                    remainder_y: 50.0,
                },
            ),
            (
                apply_grid_touch_scroll_delta(200.0, 20.0, 70.0, row_h, max_offset),
                GridTouchScrollPosition {
                    anchor_y: 100.0,
                    remainder_y: 50.0,
                },
            ),
        ];

        for (actual, expected) in cases {
            assert_eq!(actual, expected);
            assert_touch_anchor_is_row_snapped(actual, row_h);
        }
    }

    #[test]
    fn grid_touch_scroll_clamps_without_overshoot_at_both_ends() {
        let top = apply_grid_touch_scroll_delta(0.0, 10.0, 50.0, 100.0, 300.0);
        let bottom = apply_grid_touch_scroll_delta(200.0, 80.0, -50.0, 100.0, 300.0);

        assert_eq!(top, GridTouchScrollPosition::default());
        assert_eq!(
            bottom,
            GridTouchScrollPosition {
                anchor_y: 300.0,
                remainder_y: 0.0,
            }
        );
        assert_touch_anchor_is_row_snapped(top, 100.0);
        assert_touch_anchor_is_row_snapped(bottom, 100.0);
    }

    #[test]
    fn grid_touch_scroll_release_settles_along_increasing_travel() {
        let row_h = 100.0;
        let direction = Some(GridTouchScrollDirection::Increasing);
        let cases = [(10.0, 200.0), (20.0, 300.0), (40.0, 300.0), (90.0, 300.0)];

        for (remainder_y, expected_anchor) in cases {
            let position = settle_grid_touch_scroll(200.0, remainder_y, row_h, 500.0, direction);
            assert_eq!(position.anchor_y, expected_anchor);
            assert_eq!(position.remainder_y, 0.0);
            assert_touch_anchor_is_row_snapped(position, row_h);
        }
    }

    #[test]
    fn grid_touch_scroll_release_settles_along_decreasing_travel() {
        let row_h = 100.0;
        let direction = Some(GridTouchScrollDirection::Decreasing);
        let cases = [
            (90.0, 300.0),
            (85.0, 300.0),
            (80.0, 200.0),
            (50.0, 200.0),
            (20.0, 200.0),
        ];

        for (remainder_y, expected_anchor) in cases {
            let position = settle_grid_touch_scroll(200.0, remainder_y, row_h, 500.0, direction);
            assert_eq!(position.anchor_y, expected_anchor);
            assert_eq!(position.remainder_y, 0.0);
            assert_touch_anchor_is_row_snapped(position, row_h);
        }
    }

    #[test]
    fn grid_touch_scroll_release_without_direction_uses_nearest_row() {
        let before_half = settle_grid_touch_scroll(200.0, 49.9, 100.0, 500.0, None);
        let at_half = settle_grid_touch_scroll(200.0, 50.0, 100.0, 500.0, None);

        assert_eq!(before_half.anchor_y, 200.0);
        assert_eq!(at_half.anchor_y, 300.0);
    }

    #[test]
    fn grid_touch_scroll_release_never_reverses_beyond_tolerance() {
        let row_h = 100.0;
        let tolerance = row_h * GRID_TOUCH_SETTLE_REVERSAL_TOLERANCE;

        for remainder_y in (0..100).map(|value| value as f32) {
            let release_y = 200.0 + remainder_y;
            let increasing = settle_grid_touch_scroll(
                200.0,
                remainder_y,
                row_h,
                500.0,
                Some(GridTouchScrollDirection::Increasing),
            );
            let increasing_reversal = (release_y - increasing.anchor_y).max(0.0);
            assert!(increasing_reversal <= tolerance);

            let decreasing = settle_grid_touch_scroll(
                200.0,
                remainder_y,
                row_h,
                500.0,
                Some(GridTouchScrollDirection::Decreasing),
            );
            let decreasing_reversal = (decreasing.anchor_y - release_y).max(0.0);
            assert!(decreasing_reversal <= tolerance);

            assert_touch_anchor_is_row_snapped(increasing, row_h);
            assert_touch_anchor_is_row_snapped(decreasing, row_h);
        }
    }

    #[test]
    fn grid_touch_scroll_release_clamps_at_both_ends() {
        let top = settle_grid_touch_scroll(
            0.0,
            10.0,
            100.0,
            500.0,
            Some(GridTouchScrollDirection::Decreasing),
        );
        let bottom = settle_grid_touch_scroll(
            500.0,
            90.0,
            100.0,
            500.0,
            Some(GridTouchScrollDirection::Increasing),
        );

        assert_eq!(top.anchor_y, 0.0);
        assert_eq!(bottom.anchor_y, 500.0);
        assert_touch_anchor_is_row_snapped(top, 100.0);
        assert_touch_anchor_is_row_snapped(bottom, 100.0);
    }

    #[test]
    fn grid_touch_snap_below_travel_threshold_stays_instant() {
        let instant = plan_grid_touch_snap(
            200.0,
            81.0,
            100.0,
            500.0,
            Some(GridTouchScrollDirection::Increasing),
        );
        assert_eq!(
            instant,
            GridTouchSnapPlan::Immediate(GridTouchScrollPosition {
                anchor_y: 300.0,
                remainder_y: 0.0,
            })
        );

        let boundary = plan_grid_touch_snap(
            200.0,
            80.0,
            100.0,
            500.0,
            Some(GridTouchScrollDirection::Increasing),
        );
        assert!(matches!(boundary, GridTouchSnapPlan::Glide(_)));
    }

    #[test]
    fn grid_touch_snap_progress_is_exact_and_ease_out() {
        let GridTouchSnapPlan::Glide(animation) = plan_grid_touch_snap(
            200.0,
            20.0,
            100.0,
            500.0,
            Some(GridTouchScrollDirection::Increasing),
        ) else {
            panic!();
        };

        let start = grid_touch_snap_position_at(animation, std::time::Duration::ZERO);
        let midpoint = grid_touch_snap_position_at(animation, std::time::Duration::from_millis(65));
        let finish = grid_touch_snap_position_at(animation, GRID_TOUCH_SNAP_GLIDE_DURATION);
        let after = grid_touch_snap_position_at(
            animation,
            GRID_TOUCH_SNAP_GLIDE_DURATION + std::time::Duration::from_secs(1),
        );

        assert_eq!(start, animation.start);
        let start_y = start.anchor_y + start.remainder_y;
        let target_y = animation.target.anchor_y + animation.target.remainder_y;
        let midpoint_y = midpoint.anchor_y + midpoint.remainder_y;
        assert!(midpoint_y > start_y + (target_y - start_y) * 0.5);
        assert!(midpoint_y < target_y);
        assert_eq!(finish, animation.target);
        assert_eq!(after, animation.target);
        assert_touch_anchor_is_row_snapped(start, animation.row_h);
        assert_touch_anchor_is_row_snapped(midpoint, animation.row_h);
        assert_touch_anchor_is_row_snapped(finish, animation.row_h);
    }

    #[test]
    fn grid_touch_snap_repaint_requires_glide_state() {
        assert!(!grid_touch_snap_needs_animation_repaint(
            GridTouchScrollPhase::Contact { direction: None }
        ));

        let GridTouchSnapPlan::Glide(animation) = plan_grid_touch_snap(
            200.0,
            20.0,
            100.0,
            500.0,
            Some(GridTouchScrollDirection::Increasing),
        ) else {
            panic!();
        };
        assert!(grid_touch_snap_needs_animation_repaint(
            GridTouchScrollPhase::Glide {
                animation,
                started_at: std::time::Instant::now(),
            }
        ));
    }

    #[test]
    fn grid_touch_snap_interruption_keeps_anchor_and_inherits_remainder() {
        let GridTouchSnapPlan::Glide(animation) = plan_grid_touch_snap(
            200.0,
            20.0,
            100.0,
            500.0,
            Some(GridTouchScrollDirection::Increasing),
        ) else {
            panic!();
        };

        let interrupted =
            grid_touch_snap_position_at(animation, std::time::Duration::from_millis(50));
        assert_eq!(interrupted.anchor_y, animation.start.anchor_y);
        assert!(interrupted.remainder_y > animation.start.remainder_y);
        assert!(interrupted.remainder_y < animation.row_h);
        assert_touch_anchor_is_row_snapped(interrupted, animation.row_h);

        let continued = apply_grid_touch_scroll_delta(
            interrupted.anchor_y,
            interrupted.remainder_y,
            -5.0,
            animation.row_h,
            500.0,
        );
        let inherited_y = interrupted.anchor_y + interrupted.remainder_y;
        let continued_y = continued.anchor_y + continued.remainder_y;
        assert!((continued_y - (inherited_y + 5.0)).abs() < 0.001);
        assert_touch_anchor_is_row_snapped(continued, animation.row_h);
    }

    #[test]
    fn grid_touch_snap_completion_and_end_clamps_keep_row_anchor() {
        let GridTouchSnapPlan::Glide(animation) = plan_grid_touch_snap(
            400.0,
            80.0,
            100.0,
            500.0,
            Some(GridTouchScrollDirection::Increasing),
        ) else {
            panic!();
        };
        let finish = grid_touch_snap_position_at(animation, GRID_TOUCH_SNAP_GLIDE_DURATION);
        assert_eq!(
            finish,
            GridTouchScrollPosition {
                anchor_y: 500.0,
                remainder_y: 0.0,
            }
        );
        assert_touch_anchor_is_row_snapped(finish, 100.0);

        let top = plan_grid_touch_snap(
            0.0,
            10.0,
            100.0,
            500.0,
            Some(GridTouchScrollDirection::Decreasing),
        );
        let bottom = plan_grid_touch_snap(
            500.0,
            90.0,
            100.0,
            500.0,
            Some(GridTouchScrollDirection::Increasing),
        );
        assert_eq!(
            top,
            GridTouchSnapPlan::Immediate(GridTouchScrollPosition::default())
        );
        assert_eq!(
            bottom,
            GridTouchSnapPlan::Immediate(GridTouchScrollPosition {
                anchor_y: 500.0,
                remainder_y: 0.0,
            })
        );
    }

    #[test]
    fn grid_pinch_accumulates_ratio_before_one_column_step() {
        let mut state = GridPinchColumnState::default();

        let first = accumulate_grid_pinch_column_factor(&mut state, 5, 1.1);
        let second = accumulate_grid_pinch_column_factor(&mut state, 5, 1.1);
        assert_eq!(first.new_cols, 5);
        assert_eq!(second.new_cols, 5);
        assert!(!first.threshold_crossed);
        assert!(!second.threshold_crossed);

        let crossed = accumulate_grid_pinch_column_factor(&mut state, 5, 1.04);
        assert_eq!(crossed.new_cols, 4);
        assert!(crossed.threshold_crossed);
        assert!(crossed.columns_changed);
        assert_eq!(state.accumulated_scale, 1.0);
    }

    #[test]
    fn grid_pinch_expand_decreases_and_contract_increases_columns() {
        let mut expanding = GridPinchColumnState::default();
        let expand =
            accumulate_grid_pinch_column_factor(&mut expanding, 5, GRID_PINCH_COLUMN_STEP_RATIO);
        assert_eq!(expand.new_cols, 4);

        let mut contracting = GridPinchColumnState::default();
        let contract = accumulate_grid_pinch_column_factor(
            &mut contracting,
            5,
            GRID_PINCH_COLUMN_STEP_RATIO.recip(),
        );
        assert_eq!(contract.new_cols, 6);
    }

    #[test]
    fn grid_pinch_opposite_samples_cancel_before_hysteresis_threshold() {
        let mut state = GridPinchColumnState::default();
        let outward = accumulate_grid_pinch_column_factor(&mut state, 5, 1.2);
        let inward = accumulate_grid_pinch_column_factor(&mut state, 5, 0.9);

        assert_eq!(outward.new_cols, 5);
        assert_eq!(inward.new_cols, 5);
        assert!(!inward.threshold_crossed);
        assert!((state.accumulated_scale - 1.08).abs() < 0.0001);
    }

    #[test]
    fn grid_pinch_columns_clamp_and_reset_at_limits() {
        let mut minimum = GridPinchColumnState::default();
        let min_update = accumulate_grid_pinch_column_factor(
            &mut minimum,
            crate::settings::MIN_GRID_COLS,
            GRID_PINCH_COLUMN_STEP_RATIO,
        );
        assert_eq!(min_update.new_cols, crate::settings::MIN_GRID_COLS);
        assert!(min_update.threshold_crossed);
        assert!(!min_update.columns_changed);
        assert_eq!(minimum.accumulated_scale, 1.0);

        let mut maximum = GridPinchColumnState::default();
        let max_update = accumulate_grid_pinch_column_factor(
            &mut maximum,
            crate::settings::MAX_GRID_COLS,
            GRID_PINCH_COLUMN_STEP_RATIO.recip(),
        );
        assert_eq!(max_update.new_cols, crate::settings::MAX_GRID_COLS);
        assert!(max_update.threshold_crossed);
        assert!(!max_update.columns_changed);
        assert_eq!(maximum.accumulated_scale, 1.0);
    }

    #[test]
    fn grid_pinch_save_request_is_taken_only_once_at_gesture_end() {
        let ctx = egui::Context::default();
        let update = apply_grid_pinch_column_factor(&ctx, 5, GRID_PINCH_COLUMN_STEP_RATIO);
        assert!(update.columns_changed);

        assert!(take_grid_pinch_columns_save(&ctx));
        assert!(!take_grid_pinch_columns_save(&ctx));
    }

    #[test]
    fn fractional_grid_view_extends_visible_and_keep_ends_by_one_row() {
        assert_eq!(extend_grid_end_for_touch_fraction(20, 4, 100, 0.0), 20);
        assert_eq!(extend_grid_end_for_touch_fraction(20, 4, 100, 0.5), 24);
        assert_eq!(extend_grid_end_for_touch_fraction(98, 4, 100, 0.5), 100);
    }

    #[test]
    fn native_grid_drag_guard_changes_only_touch_derived_streams() {
        assert!(native_grid_drag_start_allowed(false, false, false));
        assert!(!native_grid_drag_start_allowed(false, false, true));
        assert!(!native_grid_drag_start_allowed(true, false, false));
        assert!(!native_grid_drag_start_allowed(false, true, false));
    }

    #[test]
    fn mouse_wheel_still_moves_exactly_one_snapped_row() {
        assert_eq!(grid_wheel_scroll_offset(300.0, -120.0, 100.0), 400.0);
        assert_eq!(grid_wheel_scroll_offset(300.0, 120.0, 100.0), 200.0);
        assert_eq!(grid_wheel_scroll_offset(0.0, 120.0, 100.0), 0.0);
        for offset in [
            grid_wheel_scroll_offset(300.0, -1.0, 100.0),
            grid_wheel_scroll_offset(300.0, 1.0, 100.0),
        ] {
            assert_eq!((offset / 100.0).fract(), 0.0);
        }
    }

    #[test]
    fn scrollbar_sync_is_unchanged_for_mouse_and_paused_for_touch_fraction() {
        assert!(should_sync_grid_scrollbar(false, 0.0, 200.0, 100.0, 100.0));
        assert!(!should_sync_grid_scrollbar(false, 0.0, 140.0, 100.0, 100.0));
        assert!(!should_sync_grid_scrollbar(true, 0.0, 200.0, 100.0, 100.0));
        assert!(!should_sync_grid_scrollbar(
            false, 25.0, 200.0, 100.0, 100.0
        ));
    }

    #[test]
    fn grid_touch_activity_updates_prefetch_settle_and_idle_clocks() {
        let mut app = setup_app_for_test();
        app.scroll_offset_y = 300.0;
        let before = std::time::Instant::now();

        app.note_grid_touch_scroll_activity();

        assert!(app.last_prefetch_scroll_at.is_some_and(|at| at >= before));
        assert!(app.last_scroll_event_at.is_some_and(|at| at >= before));
        assert!(app.last_scroll_change_time >= before);
        assert!(app.last_input_at.is_some_and(|at| at >= before));
        assert_eq!(app.last_scroll_offset_y_tracked, 300.0);
    }

    #[test]
    fn pending_grid_scroll_resolves_against_current_layout_extent() {
        assert_eq!(
            resolve_grid_scroll_offset(240.0, 800.0, Some(GridScrollIntent::Top)),
            0.0
        );
        assert_eq!(
            resolve_grid_scroll_offset(240.0, 800.0, Some(GridScrollIntent::Bottom)),
            800.0
        );
        assert_eq!(resolve_grid_scroll_offset(900.0, 800.0, None), 800.0);
        assert_eq!(
            resolve_grid_scroll_offset(240.0, 0.0, Some(GridScrollIntent::Bottom)),
            0.0
        );
    }

    #[test]
    fn grid_scroll_claim_does_not_clear_request_enqueued_later_in_same_frame() {
        let mut pending = Some(GridScrollIntent::Top);

        let claimed = take_grid_scroll_for_current_layout(&mut pending);
        pending = Some(GridScrollIntent::Bottom);

        assert_eq!(claimed, Some(GridScrollIntent::Top));
        assert_eq!(pending, Some(GridScrollIntent::Bottom));
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
    fn smart_folder_toolbar_requires_visibility_setting_and_a_definition() {
        assert!(!smart_folder_toolbar_visible(true, 0));
        assert!(!smart_folder_toolbar_visible(false, 1));
        assert!(smart_folder_toolbar_visible(true, 1));
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
mod checked_selection_overlay_tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[test]
    fn checked_selection_overlay_is_absent_when_no_items_are_checked() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(480.0, 300.0))
            .build(|ctx| {
                let _ = show_checked_selection_overlay(ctx, 0);
            });

        harness.run();
        assert!(harness.query_by_label("チェック").is_none());
        assert!(harness.query_by_label("選択解除").is_none());
    }

    #[test]
    fn checked_selection_overlay_shows_the_exact_checked_count() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(480.0, 300.0))
            .build(|ctx| {
                let _ = show_checked_selection_overlay(ctx, 1);
            });

        harness.run();
        assert!(harness.query_by_label("チェック").is_some());
        assert!(harness.query_by_label("1 件").is_some());
    }

    #[test]
    fn checked_selection_overlay_click_requests_deselect() {
        let clicked = Arc::new(AtomicBool::new(false));
        let clicked_in_ui = Arc::clone(&clicked);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(480.0, 300.0))
            .build(move |ctx| {
                if show_checked_selection_overlay(ctx, 2) {
                    clicked_in_ui.store(true, Ordering::Relaxed);
                }
            });

        harness.get_by_label("選択解除").click();
        harness.run();
        assert!(clicked.load(Ordering::Relaxed));
    }

    /// 件数表示そのものは押しても解除しない。解除操作はボタンだけが持つ。
    #[test]
    fn checked_selection_overlay_count_text_does_not_deselect() {
        let clicked = Arc::new(AtomicBool::new(false));
        let clicked_in_ui = Arc::clone(&clicked);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(480.0, 300.0))
            .build(move |ctx| {
                if show_checked_selection_overlay(ctx, 2) {
                    clicked_in_ui.store(true, Ordering::Relaxed);
                }
            });

        harness.get_by_label("2 件").click();
        harness.run();
        assert!(!clicked.load(Ordering::Relaxed));
    }
}

#[cfg(test)]
mod grid_click_selection_tests {
    use super::*;

    const ITEMS_GENERATION: u64 = 7;

    fn items(count: usize) -> Vec<GridItem> {
        (0..count)
            .map(|idx| GridItem::Image(PathBuf::from(format!(r"C:\{idx}.png"))))
            .collect()
    }

    fn test_anchor(index: usize) -> Option<GridClickSelectionAnchor> {
        Some(GridClickSelectionAnchor::new(index, ITEMS_GENERATION))
    }

    fn anchor_index(anchor: Option<GridClickSelectionAnchor>) -> Option<usize> {
        anchor.and_then(|anchor| anchor.index_for_generation(ITEMS_GENERATION))
    }

    #[test]
    fn check_mode_keeps_existing_checks_and_adds_shift_range() {
        let items = items(6);
        let order: Vec<_> = (0..items.len()).collect();
        let mut selected = Some(1);
        let mut anchor = test_anchor(1);
        let mut checked = HashSet::from([0]);

        apply_grid_click_selection(
            GridClickSelectionMode::Check,
            &mut selected,
            &mut anchor,
            ITEMS_GENERATION,
            &mut checked,
            &[],
            &items,
            false,
            2,
            false,
            false,
        );
        assert_eq!(selected, Some(2));
        assert_eq!(anchor_index(anchor), Some(2));
        assert_eq!(checked, HashSet::from([0]));

        apply_grid_click_selection(
            GridClickSelectionMode::Check,
            &mut selected,
            &mut anchor,
            ITEMS_GENERATION,
            &mut checked,
            &order,
            &items,
            false,
            4,
            false,
            true,
        );
        assert_eq!(selected, Some(4));
        assert_eq!(anchor_index(anchor), Some(2));
        assert_eq!(checked, HashSet::from([0, 2, 3, 4]));
    }

    #[test]
    fn explorer_normal_and_background_click_replace_then_clear_selection() {
        let items = items(4);
        let mut selected = Some(1);
        let mut anchor = test_anchor(1);
        let mut checked = HashSet::from([0, 1]);

        apply_grid_click_selection(
            GridClickSelectionMode::Explorer,
            &mut selected,
            &mut anchor,
            ITEMS_GENERATION,
            &mut checked,
            &[],
            &items,
            false,
            3,
            false,
            false,
        );
        assert_eq!(selected, Some(3));
        assert_eq!(anchor_index(anchor), Some(3));
        assert!(checked.is_empty());

        assert!(clear_grid_selection_for_background_click(
            GridClickSelectionMode::Explorer,
            &mut selected,
            &mut anchor,
            &mut checked,
        ));
        assert_eq!(selected, None);
        assert_eq!(anchor, None);
        assert!(checked.is_empty());
    }

    #[test]
    fn check_mode_background_click_preserves_selection() {
        let mut selected = Some(2);
        let mut anchor = test_anchor(2);
        let mut checked = HashSet::from([1, 2]);
        assert!(!clear_grid_selection_for_background_click(
            GridClickSelectionMode::Check,
            &mut selected,
            &mut anchor,
            &mut checked,
        ));
        assert_eq!(selected, Some(2));
        assert_eq!(anchor_index(anchor), Some(2));
        assert_eq!(checked, HashSet::from([1, 2]));
    }

    #[test]
    fn explorer_shift_replaces_checks_and_keeps_original_anchor() {
        let items = items(6);
        let order: Vec<_> = (0..items.len()).collect();
        let mut selected = Some(1);
        let mut anchor = test_anchor(1);
        let mut checked = HashSet::from([5]);

        apply_grid_click_selection(
            GridClickSelectionMode::Explorer,
            &mut selected,
            &mut anchor,
            ITEMS_GENERATION,
            &mut checked,
            &order,
            &items,
            false,
            3,
            false,
            true,
        );
        assert_eq!(checked, HashSet::from([1, 2, 3]));
        assert_eq!(selected, Some(3));
        assert_eq!(anchor_index(anchor), Some(1));

        apply_grid_click_selection(
            GridClickSelectionMode::Explorer,
            &mut selected,
            &mut anchor,
            ITEMS_GENERATION,
            &mut checked,
            &order,
            &items,
            false,
            4,
            false,
            true,
        );
        assert_eq!(checked, HashSet::from([1, 2, 3, 4]));
        assert_eq!(anchor_index(anchor), Some(1));
    }

    #[test]
    fn explorer_ctrl_click_toggles_and_collapses_back_to_single_selection() {
        let items = items(5);
        let order: Vec<_> = (0..items.len()).collect();
        let mut selected = Some(1);
        let mut anchor = test_anchor(1);
        let mut checked = HashSet::new();

        apply_grid_click_selection(
            GridClickSelectionMode::Explorer,
            &mut selected,
            &mut anchor,
            ITEMS_GENERATION,
            &mut checked,
            &order,
            &items,
            false,
            3,
            true,
            false,
        );
        assert_eq!(selected, Some(3));
        assert_eq!(checked, HashSet::from([1, 3]));

        apply_grid_click_selection(
            GridClickSelectionMode::Explorer,
            &mut selected,
            &mut anchor,
            ITEMS_GENERATION,
            &mut checked,
            &order,
            &items,
            false,
            3,
            true,
            false,
        );
        assert_eq!(selected, Some(1));
        assert!(checked.is_empty());

        apply_grid_click_selection(
            GridClickSelectionMode::Explorer,
            &mut selected,
            &mut anchor,
            ITEMS_GENERATION,
            &mut checked,
            &order,
            &items,
            false,
            1,
            true,
            false,
        );
        assert_eq!(selected, None);
        assert_eq!(anchor, None);
        assert!(checked.is_empty());
    }

    #[test]
    fn explorer_right_click_preserves_only_a_multi_selection_member() {
        let mut selected = Some(1);
        let mut anchor = test_anchor(1);
        let mut checked = HashSet::from([1, 2]);

        apply_grid_secondary_selection(
            GridClickSelectionMode::Explorer,
            &mut selected,
            &mut anchor,
            ITEMS_GENERATION,
            &mut checked,
            2,
        );
        assert_eq!(checked, HashSet::from([1, 2]));
        assert_eq!(selected, Some(2));

        apply_grid_secondary_selection(
            GridClickSelectionMode::Explorer,
            &mut selected,
            &mut anchor,
            ITEMS_GENERATION,
            &mut checked,
            4,
        );
        assert!(checked.is_empty());
        assert_eq!(selected, Some(4));
        assert_eq!(anchor_index(anchor), Some(4));
    }

    #[test]
    fn previous_list_generation_anchor_falls_back_to_current_selection() {
        let items = items(5);
        let order: Vec<_> = (0..items.len()).collect();
        let mut selected = Some(2);
        // index=0 は現一覧にも存在するが、前世代の別アイテムなのでアンカーに使わない。
        let mut anchor = Some(GridClickSelectionAnchor::new(0, ITEMS_GENERATION - 1));
        let mut checked = HashSet::new();

        apply_grid_click_selection(
            GridClickSelectionMode::Explorer,
            &mut selected,
            &mut anchor,
            ITEMS_GENERATION,
            &mut checked,
            &order,
            &items,
            false,
            4,
            false,
            true,
        );
        assert_eq!(checked, HashSet::from([2, 3, 4]));
        assert_eq!(anchor_index(anchor), Some(2));
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
