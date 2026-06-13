//! メイン画面の UI コンポーネント描画。
//!
//! `App::update()` から呼ばれるメニューバー・ツールバー・フォルダバー・
//! グリッド・進捗オーバーレイ・選択情報オーバーレイの描画メソッドを集約。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use eframe::egui;

use crate::app::{App, FacetField, LazyColumnState};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::settings::{
    DetailsSortKey, FacetDatePreset, FacetEditFlag, FacetItemKind, FacetSizePreset, FacetTagMode,
    GridViewMode,
};
// open_external_player はグリッドからは使わなくなった (動画はフルスクリーン化 →
// インライン再生)。フォルダ系は別途同モジュールから直接呼んでいる箇所がある。

use crate::ui_helpers::{
    HoverTipExt, PROGRESS_BG_COLOR, PROGRESS_LABEL_COLOR, PROGRESS_NORMAL_COLOR,
    PROGRESS_UPGRADE_COLOR,
};

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

fn rating_tooltip(idx: usize) -> String {
    if idx == 0 {
        "未評価を表示 [F6 で解除]\n通常クリック: 切り替え\nCtrl+クリック: これのみ\nShift+クリック: すべて表示"
            .to_string()
    } else {
        format!(
            "★{idx} を表示 [F{idx} で付与]\n通常クリック: 切り替え\nCtrl+クリック: これのみ\nShift+クリック: ★{idx} 以上\nCtrl+Shift+クリック: ★{idx} と未評価"
        )
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

fn facet_menu_label(base: &str, active: usize) -> String {
    if active == 0 {
        base.to_string()
    } else {
        format!("{base} ({active})")
    }
}

fn prepare_facet_menu_popup(ui: &mut egui::Ui) {
    ui.set_min_width(180.0);
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
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
fn draw_rating_filter_button(
    ui: &mut egui::Ui,
    rf: &mut [bool; 6],
    idx: usize,
    enabled: bool,
) -> bool {
    let sel = rf[idx];
    let resp = ui
        .add_enabled(
            enabled,
            egui::Button::selectable(sel, rating_button_label(idx)),
        )
        .hover_tip(rating_tooltip(idx));
    let mut changed = false;
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
        });
    }
    changed
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

    fn sort_key(self) -> DetailsSortKey {
        match self {
            Self::Name => DetailsSortKey::Name,
            Self::Rating => DetailsSortKey::Rating,
            Self::Tags => DetailsSortKey::Tags,
            Self::Kind => DetailsSortKey::Kind,
            Self::Size => DetailsSortKey::Size,
            Self::Modified => DetailsSortKey::Modified,
            Self::Created => DetailsSortKey::Created,
            Self::State => DetailsSortKey::State,
            Self::ImageDimensions => DetailsSortKey::ImageDimensions,
            Self::VideoDuration => DetailsSortKey::VideoDuration,
            Self::VideoDimensions => DetailsSortKey::VideoDimensions,
            Self::VideoCodec => DetailsSortKey::VideoCodec,
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
}

fn details_column_rects(
    rect: egui::Rect,
    settings: &crate::settings::Settings,
) -> Vec<(DetailsColumn, egui::Rect)> {
    let mut non_name = DetailsColumn::all()
        .iter()
        .copied()
        .filter(|col| *col != DetailsColumn::Name && col.visible(settings))
        .map(|col| (col, col.default_width()))
        .collect::<Vec<_>>();
    let name_min = DetailsColumn::Name.default_width();
    let fixed: f32 = non_name.iter().map(|(_, width)| *width).sum();
    if fixed > 0.0 && rect.width() < name_min + fixed {
        let scale = ((rect.width() - name_min).max(0.0) / fixed).clamp(0.55, 1.0);
        for (_, width) in &mut non_name {
            *width *= scale;
        }
    }
    let fixed: f32 = non_name.iter().map(|(_, width)| *width).sum();
    let name = (rect.width() - fixed).max(96.0);
    let mut specs = Vec::with_capacity(non_name.len() + 1);
    specs.push((DetailsColumn::Name, name));
    specs.extend(non_name);

    let mut x = rect.left();
    let mut out = Vec::with_capacity(specs.len());
    for (pos, (col, width)) in specs.iter().copied().enumerate() {
        let right = if pos + 1 == specs.len() {
            rect.right()
        } else {
            (x + width).min(rect.right())
        };
        out.push((
            col,
            egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(right, rect.bottom())),
        ));
        x = right;
    }
    out
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

fn details_kind_label(item: &GridItem) -> String {
    match item {
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
    }
}

fn details_ext_kind(path: &Path, fallback: &str) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty())
        .map(|e| format!("{} {}", e.to_ascii_uppercase(), fallback))
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(windows)]
fn format_details_mtime(secs: i64) -> String {
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
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute
    )
}

#[cfg(not(windows))]
fn format_details_mtime(secs: i64) -> String {
    if secs <= 0 {
        String::new()
    } else {
        secs.to_string()
    }
}

impl App {
    // ── メニューバー ─────────────────────────────────────────────────

    /// メニューバーを描画し、ナビゲーション先とソート変更の有無を返す。
    pub(crate) fn render_menubar(&mut self, ctx: &egui::Context) -> (Option<PathBuf>, bool) {
        let mut fav_nav: Option<PathBuf> = None;
        let mut settings_changed = false;
        let mut sort_changed = false;
        let selected_video_path =
            self.selected
                .and_then(|idx| self.items.get(idx))
                .and_then(|item| match item {
                    GridItem::Video(path) => Some(path.clone()),
                    _ => None,
                });

        egui::TopBottomPanel::top("menubar").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                let mut top_menu_responses = Vec::with_capacity(6);

                let response = ui.menu_button("ファイル", |ui| {
                    if ui.button("フォルダを開く…").clicked() {
                        // 既に現在フォルダが設定されていれば初期値として補完
                        self.open_folder_input = self
                            .current_folder
                            .as_ref()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();
                        self.show_open_folder_dialog = true;
                        ui.close();
                    }
                    if ui.button("現在地フィルタ (Ctrl+F)").clicked() {
                        // 相互排他は open_local_metadata_search 内で (Ctrl+S/Ctrl+G を閉じる)
                        self.open_local_metadata_search();
                        ui.close();
                    }
                    if ui.button("キャプチャ保存フォルダを開く").clicked() {
                        self.open_capture_output_dir();
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("終了").clicked() {
                        // トレイ常駐設定 ON のときでも [×] ではなく明示終了なので、
                        // `shutdown_requested` を立てて `maybe_intercept_close` を通す。
                        self.shutdown_requested
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                top_menu_responses.push(response.response);

                let response = ui.menu_button("お気に入り", |ui| {
                    // このフォルダを追加 (クリック時は名称入力ダイアログを開く)。
                    // お気に入りは索引ルートになるため、ZIP/PDF/変換キャッシュではなく
                    // 実ディレクトリだけを対象にする。
                    let favorite_target = self.current_favorite_target();
                    let can_add = favorite_target.is_some();
                    if ui
                        .add_enabled(can_add, egui::Button::new("このフォルダを追加…"))
                        .hover_tip_disabled("お気に入りに追加できるのは実フォルダのみです")
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

                    // 編集
                    if ui.button("編集").clicked() {
                        self.show_favorites_editor = true;
                        ui.close();
                    }

                    // コンテナ検索 (Ctrl+S)
                    if ui.button("コンテナ検索 (Ctrl+S)").clicked() {
                        self.open_favsearch();
                        ui.close();
                    }

                    // アイテム検索 (Ctrl+G)
                    if ui.button("アイテム検索 (Ctrl+G)").clicked() {
                        // 相互排他は toggle_global_search 内で
                        self.toggle_global_search();
                        ui.close();
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

                let response = ui.menu_button("動画", |ui| {
                    let can_apply_to_selected = selected_video_path.is_some();
                    if ui
                        .add_enabled(
                            can_apply_to_selected,
                            egui::Button::new("この動画をアップスケール登録…"),
                        )
                        .clicked()
                    {
                        if let Some(path) = selected_video_path.clone() {
                            self.request_video_upscale(path);
                        }
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            can_apply_to_selected,
                            egui::Button::new("この動画のアップスケールを削除"),
                        )
                        .clicked()
                    {
                        if let Some(path) = selected_video_path.clone() {
                            self.request_video_upscale_artifact_delete(path);
                        }
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("アップスケールタスク表示").clicked() {
                        self.show_video_upscale_tasks = true;
                        ui.close();
                    }
                });
                top_menu_responses.push(response.response);

                // タグメニュー (docs/tag-feature.md §4.2)
                let response = ui.menu_button("タグ", |ui| {
                    if ui.button("ピン留めタグの管理…").clicked() {
                        self.open_tag_editor();
                        ui.close();
                    }
                    if ui.button("タグビュー (Ctrl+T)").clicked() {
                        self.open_tag_view();
                        ui.close();
                    }
                    ui.separator();
                    let selection_count = self.tag_target_path_count();
                    let has_target = selection_count > 0;
                    if ui
                        .add_enabled(
                            has_target,
                            egui::Button::new(format!("タグを付ける/外す… ({selection_count})")),
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

                let response = ui.menu_button("設定", |ui| {
                    ui.menu_button("サムネイル列数", |ui| {
                        for cols in crate::settings::MIN_GRID_COLS..=crate::settings::MAX_GRID_COLS
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
                    ui.menu_button("ソート順", |ui| {
                        for &order in crate::settings::SortOrder::all() {
                            let checked = self.settings.sort_order == order;
                            let prefix = if checked { "✓ " } else { "  " };
                            if ui.button(format!("{prefix}{}", order.label())).clicked() {
                                self.settings.sort_order = order;
                                sort_changed = true;
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("サムネイルキャッシュ管理").clicked() {
                        let cache_dir = crate::catalog::default_cache_dir();
                        // cache_stats は数千フォルダで秒級になるのでワーカーに回す。
                        // ダイアログは「取得中...」表示で開き、poll 完了時に stats が埋まる。
                        self.cache_manager_stats = None;
                        self.cache_manager_tile_bytes = None;
                        self.cache_manager_auto_aspect_entries = None;
                        self.cache_manager_result = None;
                        if self.cache_maint_pending.is_none() {
                            self.cache_maint_pending = Some(crate::cache_maintenance::spawn(
                                crate::cache_maintenance::CacheMaintTask::Stats,
                                cache_dir,
                                self.video_tile_cache.clone(),
                            ));
                        }
                        self.show_cache_manager = true;
                        ui.close();
                    }
                    if ui.button("変換済みアーカイブキャッシュ管理").clicked() {
                        self.open_archive_cache_manager();
                        ui.close();
                    }
                    if ui.button("サムネイル画質…").clicked() {
                        self.open_thumb_quality_dialog(ctx);
                        ui.close();
                    }
                    if ui.button("統計…").clicked() {
                        self.show_stats_dialog = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("回転情報をリセット…").clicked() {
                        self.show_rotation_reset_confirm = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("設定の復元…").clicked() {
                        // 2026-05-17: settings.db のバックアップから復元する UI。
                        // 起動時の自動 boot recovery で救えなかった場合、ユーザーが
                        // 過去 10 世代を選んで巻き戻せるようにする (= 完全リセットも可)。
                        self.open_settings_restore_dialog();
                        ui.close();
                    }
                    if ui.button("環境設定…").clicked() {
                        self.show_preferences = true;
                        ui.close();
                    }
                    // VST3 関連の設定は環境設定→VST3 プラグインページに集約。
                    // 専用メニューは重複なので持たない (= ユーザー要望 2026-04)。
                    // 動画再生中はホバーバー / ツールバーの VST ボタンから
                    // プレイバックパネルを開く運用。
                });
                top_menu_responses.push(response.response);

                let response = ui.menu_button("ヘルプ", |ui| {
                    if ui.button("ヘルプサイトを開く").clicked() {
                        let url = crate::ui_helpers::manual_url("index.html", None);
                        crate::ui_helpers::open_url(&url);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("ログフォルダを開く").clicked() {
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
                    if ui.button("バージョン情報").clicked() {
                        self.show_about_dialog = true;
                        ui.close();
                    }
                });
                top_menu_responses.push(response.response);

                // メニュー項目の右側に新バージョン通知バッジを表示する。
                // 押すと更新ダイアログを開き、リリースページへの誘導 / skip 操作を行える。
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
        let show_cols = !tb_cols.is_empty() || toolbar_details_visible;
        // 比率セクション: 手動 7 種を全部外しても「自動」だけ ON なら表示する
        // (Codex P3 2026-05)。`toolbar_aspect_auto_visible` は別フラグなので
        // tb_aspects 空 + auto_visible で section が消える事故を防ぐ。
        let show_aspect =
            !details_mode && (self.settings.toolbar_aspect_auto_visible || !tb_aspects.is_empty());
        let show_sort = !tb_sorts.is_empty();
        let show_favs = self.settings.show_toolbar_favorites;
        let show_rating = self.settings.show_toolbar_rating;
        let show_tags = self.settings.show_toolbar_tags;
        let show_folder_tree_button = self.settings.show_toolbar_folder_tree_button;
        let any_toolbar_section = show_folder_tree_button
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
        let mut toolbar_tag_click: Option<String> = None;
        let mut toolbar_tag_search: Option<String> = None;
        let mut toolbar_tag_apply = false;
        let mut toolbar_tag_view_open = false;
        let mut toolbar_combo_popup_open = false;

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

            ui.scope(|ui| {
                // ツールバー本体に toolbar スタイルを適用 (Yu Gothic の glyph 上寄り問題を
                // FontTweak.y_offset で補正)。詳細: src/ui_fonts.rs の TOOLBAR_TEXT_FAMILY_NAME。
                apply_toolbar_style(ui);

                // ツールバー用ラベル: ComboBox / selectable_label と同じ高さで描画
                // して縦位置を揃える。
                // ⚠ 幅 0.0 を渡すと「親レイアウト上の占有幅 0」と解釈されて次の
                // widget と詰まる/重なる/wrap 判定が狂う (Codex 助言 2026-05)。
                // 固定幅を明示すること。日本語ラベルは数が固定なので呼び出し側で
                // 目視チューンした値を渡す。
                fn toolbar_label(ui: &mut egui::Ui, text: &str, width: f32) -> egui::Response {
                    let h = ui.spacing().interact_size.y;
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, h),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| ui.label(text),
                    )
                    .inner
                }

                ui.horizontal_wrapped(|ui| {
                let mut first_section = true;
                // ツールバー VST ボタンは v0.9.0 開発中に削除 (= ユーザー要望 2026-04
                // 「ツールバーの VST ボタンも不要になったので削除」)。
                // VST3 プラグインのプレイバックパネルは動画再生中にホバーバー側の
                // VST ボタンから開く (フルスクリーンビューポート内で完結)。
                // 通常表示中はパネルを開く手段は無く、設定変更は環境設定→
                // VST3 プラグイン から行う運用。
                if show_folder_tree_button {
                    let active = self.settings.folder_tree_pane_visible;
                    let resp = ui
                        .selectable_label(active, "ツリー")
                        .on_hover_text("左側に実フォルダツリーを表示");
                    if resp.clicked() {
                        self.set_folder_tree_pane_visible(!active);
                    }
                    first_section = false;
                }
                if show_cols {
                    if !first_section {
                        ui.separator();
                    }
                    toolbar_label(ui, "列:", 28.0);
                    match self.settings.toolbar_cols_display {
                        crate::settings::ToolbarSectionDisplay::Buttons => {
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
                                    .on_hover_text(
                                        "サムネイルなしの詳細一覧に切り替えます (Alt+-)",
                                    )
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
                    first_section = false;
                }
                if show_aspect {
                    if !first_section {
                        ui.separator();
                    }
                    toolbar_label(ui, "比率:", 42.0);
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
                        crate::settings::ToolbarSectionDisplay::Buttons => {
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
                    first_section = false;
                }
                if show_sort {
                    if !first_section {
                        ui.separator();
                    }
                    let sort_disabled = self.details_header_sort_active();
                    let sort_label = toolbar_label(ui, "ソート:", 54.0);
                    if sort_disabled {
                        sort_label.hover_tip(
                            "詳細一覧の列ヘッダで並べ替え中です。\nヘッダをもう一度クリックして「ソートなし」に戻すと有効になります。",
                        );
                    }
                    ui.add_enabled_ui(!sort_disabled, |ui| {
                        match self.settings.toolbar_sort_display {
                            crate::settings::ToolbarSectionDisplay::Buttons => {
                                for &order in &tb_sorts {
                                    let selected = self.settings.sort_order == order;
                                    if ui
                                        .selectable_label(selected, order.short_label())
                                        .clicked()
                                        && !selected
                                    {
                                        self.settings.sort_order = order;
                                        self.settings.save();
                                        toolbar_sort_changed = true;
                                    }
                                }
                            }
                            crate::settings::ToolbarSectionDisplay::Dropdown => {
                                let current_text =
                                    self.settings.sort_order.short_label().to_string();
                                let combo = egui::ComboBox::from_id_salt("toolbar_sort_combo")
                                    .width(100.0)
                                    .height(TOOLBAR_SORT_COMBO_HEIGHT)
                                    .selected_text(current_text)
                                    .show_ui(ui, |ui| {
                                        apply_toolbar_style(ui);
                                        for &order in &tb_sorts {
                                            let selected = self.settings.sort_order == order;
                                            if ui
                                                .selectable_label(selected, order.short_label())
                                                .clicked()
                                                && !selected
                                            {
                                                self.settings.sort_order = order;
                                                self.settings.save();
                                                toolbar_sort_changed = true;
                                            }
                                        }
                                    });
                                toolbar_combo_popup_open |=
                                    egui::ComboBox::is_open(ctx, combo.response.id);
                            }
                        }
                    });
                    first_section = false;
                }
                if show_rating {
                    if !first_section {
                        ui.separator();
                    }
                    // Ctrl+G の集約ビュー (= 検索結果のフォルダ一覧) では★フィルタを
                    // 反映できない (ヒット件数と filter の二重集計が必要で実装コスト大)。
                    // ドリルイン後は file list + サブフォルダ件数の両方に反映するので
                    // enable に戻す。
                    let aggregated_search = self.global_search.active
                        && self.global_search.drill.is_none()
                        && self.global_search.aggregate;
                    // hover ヒントは disable 中の widget では拾われにくいので
                    // (egui の sense)、有効な「★:」ラベル側に乗せる。
                    let star_label = toolbar_label(ui, "★:", 24.0);
                    if aggregated_search {
                        star_label.hover_tip(
                            "検索結果のコンテナ一覧では★フィルタは適用できません。\nコンテナを開くと有効になります。",
                        );
                    }
                    // ★ボタン群を `add_enabled_ui` でまとめると、その scope が「残り幅」
                    // だけの狭い子 UI を作るので `horizontal_wrapped` の wrap が子 UI 内で
                    // 起きてしまい、★★ 以降が右端の縦帯に積まれて崩れる。enabled は各
                    // ボタン側に渡し、親の wrap に直接乗せて次の row に流させる。
                    for idx in 0..6 {
                        if draw_rating_filter_button(
                            ui,
                            &mut self.settings.rating_filter,
                            idx,
                            !aggregated_search,
                        ) {
                            toolbar_rating_changed = true;
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

                    first_section = false;
                }
                if show_favs {
                    if !first_section {
                        ui.separator();
                    }
                    toolbar_label(ui, "お気に入り:", 76.0);
                    if self.settings.favorites.is_empty() {
                        ui.label(egui::RichText::new("(未登録)").weak());
                    } else {
                        // 現在のフォルダと一致するお気に入りをハイライト
                        let current = self.current_folder.clone();
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
                    first_section = false;
                }

                // タグセクション (docs/tag-feature.md §4.3)
                let toolbar_tags: Vec<_> = self
                    .settings
                    .tags
                    .iter()
                    .filter(|tag| tag.show_shortcut)
                    .map(|t| t.name.clone())
                    .collect();
                if show_tags {
                    if !first_section {
                        ui.separator();
                    }
                    toolbar_label(ui, "タグ:", 42.0);
                    let has_target = self.tag_target_path_count() > 0;
                    if ui
                        .add_enabled(has_target, egui::Button::new("設定"))
                        .hover_tip("選択中の項目へタグを付ける/外す")
                        .clicked()
                    {
                        toolbar_tag_apply = true;
                    }
                    if ui
                        .button("検索")
                        .hover_tip("タグビューを開く (Ctrl+T)")
                        .clicked()
                    {
                        toolbar_tag_view_open = true;
                    }
                    for name in toolbar_tags {
                        let label = format!("#{name}");
                        let resp = ui.add_enabled(has_target, egui::Button::new(label));
                        let clicked = resp.clicked();
                        resp.context_menu(|ui| {
                            if ui.button("このタグで探す").clicked() {
                                toolbar_tag_search = Some(name.clone());
                                ui.close();
                            }
                        });
                        if clicked {
                            toolbar_tag_click = Some(name);
                        }
                    }
                }
                });
            });
            ui.add_space(2.0);
        });

        if toolbar_combo_popup_open {
            ctx.input_mut(|i| {
                i.raw_scroll_delta = egui::Vec2::ZERO;
                i.smooth_scroll_delta = egui::Vec2::ZERO;
                i.events
                    .retain(|e| !matches!(e, egui::Event::MouseWheel { .. }));
            });
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

        // ツールバーのタグ項目クリック
        if let Some(name) = toolbar_tag_click {
            self.request_tag_toggle_for_selection(&name);
        }
        if let Some(name) = toolbar_tag_search {
            self.open_tag_view_for_tag(&name);
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

    // ── スマートフィルタバー ────────────────────────────────────────

    pub(crate) fn render_facet_filter_bar(&mut self, ctx: &egui::Context) {
        if !self.settings.show_toolbar_facet_filter {
            return;
        }
        if self.items.is_empty() || self.items_are_drive_list {
            return;
        }

        let mut facet_changed = false;
        let mut rating_changed = false;
        egui::TopBottomPanel::top("facet_filter_bar").show(ctx, |ui| {
            ui.add_space(1.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("絞り込み:").small());
                facet_changed |= self.draw_facet_kind_menu(ui);
                facet_changed |= self.draw_facet_ext_menu(ui);
                rating_changed |= self.draw_facet_rating_menu(ui);
                facet_changed |= self.draw_facet_tag_menu(ui);
                facet_changed |= self.draw_facet_date_menu(ui);
                facet_changed |= self.draw_facet_size_menu(ui);
                facet_changed |= self.draw_facet_edit_menu(ui);

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

                if self.facet_filter_active() || self.rating_filter_active() {
                    ui.separator();
                    self.draw_facet_active_chips(ui);
                    if ui.small_button("全解除").clicked() {
                        if self.facet_filter_active() {
                            self.settings.facet_filter.clear();
                            facet_changed = true;
                        }
                        if self.rating_filter_active() {
                            self.settings.rating_filter = crate::settings::default_rating_filter();
                            rating_changed = true;
                        }
                    }
                }
                ui.separator();
                ui.label(egui::RichText::new(format!("{} 件", self.visible_indices.len())).small());
            });
            ui.add_space(1.0);
        });

        if rating_changed {
            self.drop_rating_filter_suppression_on_user_edit();
        }
        if facet_changed || rating_changed {
            self.settings.save();
            if rating_changed && self.global_search.active && self.items_are_global_search_view {
                self.rebuild_items_from_global_search();
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

    fn draw_facet_kind_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let label = facet_menu_label("種類", self.settings.facet_filter.kinds.len());
        ui.menu_button(label, |ui| {
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
        changed
    }

    fn draw_facet_ext_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let label = facet_menu_label("拡張子", self.settings.facet_filter.exts.len());
        ui.menu_button(label, |ui| {
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
        changed
    }

    fn draw_facet_rating_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let active = if self.rating_filter_active() { 1 } else { 0 };
        let mut changed = false;
        ui.menu_button(facet_menu_label("★", active), |ui| {
            prepare_facet_menu_popup(ui);
            if ui.small_button("すべて表示").clicked() {
                self.settings.rating_filter = crate::settings::default_rating_filter();
                changed = true;
                ui.close();
            }
            ui.separator();
            for idx in 0..6 {
                let mut selected = self.settings.rating_filter[idx];
                if ui
                    .checkbox(&mut selected, rating_button_label(idx))
                    .on_hover_text(rating_tooltip(idx))
                    .changed()
                {
                    self.settings.rating_filter[idx] = selected;
                    changed = true;
                }
            }
        });
        changed
    }

    fn draw_facet_tag_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let active = self.settings.facet_filter.tags.len()
            + usize::from(self.settings.facet_filter.include_untagged);
        let mut changed = false;
        ui.menu_button(facet_menu_label("タグ", active), |ui| {
            prepare_facet_menu_popup(ui);
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
                    .selectable_value(&mut mode, FacetTagMode::Any, FacetTagMode::Any.label())
                    .changed()
                {
                    self.settings.facet_filter.tag_mode = mode;
                    changed = true;
                }
                if ui
                    .selectable_value(&mut mode, FacetTagMode::All, FacetTagMode::All.label())
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
                let mut output = egui::TextEdit::singleline(&mut self.facet_tag_search_query)
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
            }
            for (tag_key, (display, count)) in choices {
                let mut selected = self.settings.facet_filter.tags.contains(&tag_key);
                let text = format!("#{} ({count})", display);
                if ui.checkbox(&mut selected, text).changed() {
                    if selected {
                        self.settings.facet_filter.tags.insert(tag_key);
                    } else {
                        self.settings.facet_filter.tags.remove(&tag_key);
                    }
                    changed = true;
                }
            }
        });
        changed
    }

    fn draw_facet_date_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let active = usize::from(self.settings.facet_filter.date_preset.is_some());
        let mut changed = false;
        ui.menu_button(facet_menu_label("日付", active), |ui| {
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
        changed
    }

    fn draw_facet_size_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let active = usize::from(self.settings.facet_filter.size_preset.is_some());
        let mut changed = false;
        ui.menu_button(facet_menu_label("サイズ", active), |ui| {
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
        changed
    }

    fn draw_facet_edit_menu(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let label = facet_menu_label("状態", self.settings.facet_filter.edits.len());
        ui.menu_button(label, |ui| {
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
        });
        changed
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
        if self.rating_filter_active() {
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
            let values = filter
                .edits
                .iter()
                .take(3)
                .map(|flag| flag.label())
                .collect::<Vec<_>>()
                .join(",");
            facet_chip(ui, format!("状態:{values}"));
        }
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
        egui::TopBottomPanel::top("address_bar")
            .show(ctx, |ui| -> Option<AddressBarNav> {
                ui.add_space(3.0);
                let mut result = None;
                let mut pin_click = PinButtonClick::None;
                let mut favorite_click = FavoriteButtonClick::None;
                let mut tree_nav: Option<bool> = None;
                // 検索中の ⬆ ボタン (検索仮想階層を 1 段ドリルアップ) を closure 後に適用。
                let mut search_drill_up = false;
                ui.horizontal(|ui| {
                    ui.label("フォルダ:");
                    let show_history_nav = self.settings.show_address_bar_history_nav;
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

                    if show_history_nav && (show_parent || show_tree_nav) {
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
                        } else {
                            has_current
                        };
                        let prev_hover = if snapshot_active {
                            "★固定リストの前へ [Ctrl+↑]"
                        } else if search_active {
                            "前のヒットフォルダへ [Ctrl+↑]"
                        } else {
                            "ツリー順で前のフォルダへ [Ctrl+↑]"
                        };
                        let next_hover = if snapshot_active {
                            "★固定リストの次へ [Ctrl+↓]"
                        } else if search_active {
                            "次のヒットフォルダへ [Ctrl+↓]"
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
                        ui.menu_button("場所▼", |ui| {
                            ui.set_min_width(220.0);
                            if ui.button("ドライブ一覧").clicked() {
                                result = Some(AddressBarNav::DriveList(None));
                                ui.close();
                            }
                            ui.separator();
                            let quick_locations = crate::known_folders::quick_locations();
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
                                    if let Some(resolved) = resolve_folder_bar_nav_path(&drive) {
                                        result = Some(AddressBarNav::Direct(resolved));
                                    }
                                    ui.close();
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
                            .hover_tip("このフォルダ / ZIP / PDF のレーティング [Shift+F1〜F6]");
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
                                if self.address.trim().is_empty() {
                                    result = Some(AddressBarNav::DriveList(None));
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
        egui::TopBottomPanel::top("search_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label("検索:").on_hover_text(
                    "現在地フィルタ (Ctrl+F): 今開いているフォルダ / ZIP の表示中\n\
                     アイテムを名前やメタ情報で絞り込みます (索引不要・再帰なし)。",
                );
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
                    self.execute_search();
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
                            self.execute_search();
                        }
                    }
                }

                if crate::ui_helpers::or_mode_checkbox(ui, &mut self.search_or_mode)
                    && !self.search_query.trim().is_empty()
                {
                    self.execute_search();
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
                            if self.items.get(vidx).is_some_and(|it| it.is_checkable()) {
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
                            && self.items.get(prev_sel).is_some_and(|it| it.is_checkable())
                        {
                            self.checked.insert(prev_sel);
                        }
                    }
                }
                if self.items.get(idx).is_some_and(|it| it.is_checkable()) {
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
        if response.double_clicked() {
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
                    let open_outcome = if let Some(cached) = self.try_archive_cache_lookup(&pf) {
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
                // ネスト ZIP ツリーの子コンテナへダブルクリックで降りる (Phase 3)。
                Some(GridItem::ZipDir { dir_prefix, .. }) => {
                    let dp = dir_prefix.clone();
                    // ★付きの本を絞り込み中に開くと中身が空表示になるのを防ぐ
                    // (Codex P2)。enter 前に抑制を仕込む。
                    self.maybe_suppress_rating_filter_for_opened_zip_book(idx);
                    self.maybe_suppress_facet_filter_for_opened_zip_book(idx);
                    self.zip_nav_enter(&dp);
                }
                None => {}
            }
        }
        // 右クリック → コンテキストメニュー
        if !self.items_are_drive_list && response.secondary_clicked() {
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
        let badge_rect = crate::app::grid_tag_badge_hit_rect(
            ui,
            cell_rect,
            self.adjustment_page_params.contains_key(&idx),
            self.local_adjust_pages.contains(&idx),
            self.mask_pages.contains(&idx),
            self.conceal_pages.contains(&idx),
            self.comic_pages.contains(&idx),
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
        if self.current_folder.is_some() {
            self.context_menu_idx = Some(usize::MAX);
            self.context_menu_pos = ctx.input(|i| i.pointer.interact_pos().unwrap_or_default());
            ctx.request_repaint();
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

        if (avail_w - self.last_cell_size).abs() > 0.5
            || (Self::DETAILS_ROW_H - self.last_cell_h).abs() > 0.5
        {
            self.last_cell_size = avail_w;
            self.last_cell_h = Self::DETAILS_ROW_H;
        }

        if scroll_to {
            self.apply_scroll_to_selected(1, Self::DETAILS_ROW_H);
        }

        let (header_rect, _) = ui.allocate_exact_size(
            egui::vec2(avail_w, Self::DETAILS_HEADER_H),
            egui::Sense::hover(),
        );
        self.draw_details_header(ui, header_rect);

        if self.details_order.len() != self.visible_indices.len() {
            self.rebuild_details_order();
        }
        let display_order = self.current_grid_order().to_vec();
        let row_count = display_order.len();
        let natural_h = row_count as f32 * Self::DETAILS_ROW_H;
        let total_h = if natural_h <= self.last_viewport_h {
            natural_h
        } else {
            let raw_max = natural_h - self.last_viewport_h;
            let snapped_max = (raw_max / Self::DETAILS_ROW_H).ceil() * Self::DETAILS_ROW_H;
            snapped_max + self.last_viewport_h
        };
        let max_offset = if total_h <= self.last_viewport_h {
            0.0
        } else {
            total_h - self.last_viewport_h
        };
        self.scroll_offset_y = self.scroll_offset_y.clamp(0.0, max_offset);

        let mut nav: Option<PathBuf> = None;
        let scroll_output = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .vertical_scroll_offset(self.scroll_offset_y)
            .show_viewport(ui, |ui, viewport| {
                self.last_viewport_h = viewport.height();

                let (content_rect, _) =
                    ui.allocate_exact_size(egui::vec2(avail_w, total_h), egui::Sense::hover());

                let first_row = (viewport.min.y / Self::DETAILS_ROW_H) as usize;
                let last_row = ((viewport.max.y / Self::DETAILS_ROW_H) as usize + 2).min(row_count);
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
                        content_rect.min + egui::vec2(0.0, row as f32 * Self::DETAILS_ROW_H),
                        egui::vec2(avail_w, Self::DETAILS_ROW_H),
                    );

                    if let Some(n) = self.handle_cell_interaction(ui, ctx, row_rect, idx) {
                        nav = Some(n);
                    }
                    if idx >= self.items.len() {
                        break;
                    }

                    self.draw_details_row(
                        ui,
                        row_rect,
                        idx,
                        row,
                        spread_pair_cursor_idx == Some(idx),
                    );
                    if self.selected == Some(idx) {
                        self.selected_cell_rect = Some(row_rect);
                    }
                }
            });

        let bg_right_clicked = ui.rect_contains_pointer(scroll_output.inner_rect)
            && ctx.input(|i| i.pointer.secondary_clicked());
        if bg_right_clicked && self.context_menu_idx.is_none() {
            self.open_current_folder_context_menu(ctx);
        }

        let egui_offset = scroll_output.state.offset.y;
        if (egui_offset - self.scroll_offset_y).abs() > Self::DETAILS_ROW_H * 0.5 {
            self.scroll_offset_y =
                (egui_offset / Self::DETAILS_ROW_H).round() * Self::DETAILS_ROW_H;
        }

        let full_rect = ui.max_rect();
        self.draw_feedback_toast(ui, full_rect, ctx);

        nav
    }

    fn draw_details_header(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let bg = ui.visuals().extreme_bg_color;
        let stroke_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let text_color = ui.visuals().strong_text_color();
        let hover_bg = ui.visuals().widgets.hovered.bg_fill;
        ui.painter().rect_filled(rect, 0.0, bg);
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            egui::Stroke::new(1.0, stroke_color),
        );

        for (col, col_rect) in details_column_rects(rect, &self.settings) {
            let response = ui.interact(
                col_rect,
                ui.id().with(("details_header", col)),
                egui::Sense::click(),
            );
            if response.hovered() {
                ui.painter().rect_filled(col_rect, 0.0, hover_bg);
            }
            let sort_key = col.sort_key();
            let lazy_sort = col.is_lazy();
            let sort_enabled = !lazy_sort || self.details_lazy_sort_ready();
            if response.clicked() && sort_enabled {
                self.set_details_sort_key(col.sort_key());
            }
            let sorted = self.settings.details_sort_key == sort_key;
            let mut base_title = col.title().to_string();
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
            ui.painter().line_segment(
                [col_rect.right_top(), col_rect.right_bottom()],
                egui::Stroke::new(1.0, stroke_color),
            );
            let response = if sort_enabled {
                response.hover_tip("クリックで 昇順 → 降順 → ソートなし")
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

        let old_lazy = (
            self.settings.details_show_created,
            self.settings.details_show_image_dimensions,
            self.settings.details_show_video_duration,
            self.settings.details_show_video_dimensions,
            self.settings.details_show_video_codec,
        );
        let mut changed = false;
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
        changed |= ui
            .checkbox(&mut self.settings.details_show_modified, "更新日時")
            .changed();
        changed |= ui
            .checkbox(&mut self.settings.details_show_state, "状態")
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
        row: usize,
        is_spread_pair_cursor: bool,
    ) {
        let Some(item) = self.items.get(idx).cloned() else {
            return;
        };
        let visuals = ui.visuals();
        let selected = self.selected == Some(idx);
        let checked = self.checked.contains(&idx);
        let bg = if selected {
            visuals.selection.bg_fill
        } else if checked {
            visuals.widgets.active.bg_fill
        } else if row % 2 == 0 {
            visuals.panel_fill
        } else {
            visuals.faint_bg_color
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
                    crate::ui_helpers::format_bytes(size as u64)
                } else {
                    String::new()
                };
                (size_text, format_details_mtime(mtime))
            })
            .unwrap_or_else(|| (String::new(), String::new()));
        let state_text = self.details_state_text(idx);

        for (col, col_rect) in details_column_rects(rect, &self.settings) {
            match col {
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
    }

    fn details_state_text(&mut self, idx: usize) -> String {
        let mut flags = Vec::new();
        if self.adjustment_page_params.contains_key(&idx) {
            flags.push("補");
        }
        if self.local_adjust_pages.contains(&idx) {
            flags.push("レ");
        }
        if self.mask_pages.contains(&idx) {
            flags.push("消");
        }
        if self.conceal_pages.contains(&idx) {
            flags.push("隠");
        }
        if self.comic_pages.contains(&idx) {
            flags.push("文");
        }
        if !self.get_rotation(idx).is_none() {
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
                    if ui.rect_contains_pointer(ui.max_rect())
                        && ctx.input(|i| i.pointer.secondary_clicked())
                    {
                        self.open_current_folder_context_menu(ctx);
                    }
                    let full_rect = ui.max_rect();
                    self.draw_feedback_toast(ui, full_rect, ctx);
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
                    if ui.rect_contains_pointer(ui.max_rect())
                        && ctx.input(|i| i.pointer.secondary_clicked())
                    {
                        self.open_current_folder_context_menu(ctx);
                    }
                    let full_rect = ui.max_rect();
                    self.draw_feedback_toast(ui, full_rect, ctx);
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

                if scroll_to {
                    self.apply_scroll_to_selected(cols, cell_h);
                }

                let total_rows = self.visible_indices.len().div_ceil(cols);
                let natural_h = total_rows as f32 * cell_h;

                // egui 内部の max offset = total_h - viewport_h が行境界に揃うよう、
                // total_h を拡張する。これにより egui と自前の行スナップが一致し振動を防ぐ。
                // 拡張量は最大 cell_h 未満（端数の補正のみ）。
                let total_h = if natural_h <= self.last_viewport_h {
                    natural_h
                } else {
                    let raw_max = natural_h - self.last_viewport_h;
                    let snapped_max = (raw_max / cell_h).ceil() * cell_h;
                    snapped_max + self.last_viewport_h
                };

                let max_offset = if total_h <= self.last_viewport_h {
                    0.0
                } else {
                    total_h - self.last_viewport_h
                };
                self.scroll_offset_y = self.scroll_offset_y.clamp(0.0, max_offset);

                let mut nav: Option<PathBuf> = None;

                // egui にスクロールを管理させず、自前の offset を毎フレーム注入する。
                // ただしスクロールバードラッグ時は egui 側のオフセットを読み戻す。
                let scroll_output = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .vertical_scroll_offset(self.scroll_offset_y)
                    .show_viewport(ui, |ui, viewport| {
                        // ビューポート高さを記録（次フレームのスクロール計算に使う）
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
                                let has_page_override =
                                    self.adjustment_page_params.contains_key(&idx);
                                let has_local_adjust = self.local_adjust_pages.contains(&idx);
                                let has_mask = self.mask_pages.contains(&idx);
                                let has_conceal = self.conceal_pages.contains(&idx);
                                let has_comic = self.comic_pages.contains(&idx);
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
                                    has_page_override,
                                    has_local_adjust,
                                    has_mask,
                                    has_conceal,
                                    has_comic,
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
                let bg_right_clicked = ui.rect_contains_pointer(scroll_output.inner_rect)
                    && ctx.input(|i| i.pointer.secondary_clicked());
                if bg_right_clicked && self.context_menu_idx.is_none() {
                    self.open_current_folder_context_menu(ctx);
                }

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

        // フルパスを表示する (ファイル名だけだとセル幅の表示と大差なく、置き場所が
        // 分かりにくいため)。長いパスは下の折り返し表示で複数行に展開する。
        // 変換キャッシュ閲覧中はユーザー視点の元アーカイブパスに置き換える
        // (archive_cache の実体パスを漏らさない)。
        let path = self
            .items
            .get(idx)
            .map(|it| self.user_facing_display_path(it))
            .unwrap_or_default();
        // 元画像のピクセル寸法 (ThumbnailState::Loaded.source_dims から取得)
        let dims_str = match self.thumbnails.get(idx) {
            Some(ThumbnailState::Loaded {
                source_dims: Some((w, h)),
                ..
            }) => Some(format!("{} × {}", w, h)),
            _ => None,
        };
        let text = match dims_str {
            Some(d) => format!("{}   {}", d, path),
            None => path,
        };

        // セル幅で配置: セルの左下を基点、セル幅に合わせる
        let cell_w = cell_rect.width();
        let area_pos = cell_rect.left_bottom() + egui::vec2(0.0, 4.0);

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
                        let inner_width = (cell_w - 12.0).max(40.0);
                        ui.set_min_width(inner_width);
                        ui.set_max_width(inner_width);
                        // フルパスをセル幅で折り返し、最大 3 行まで表示する。
                        // パスは空白の無い長い連結文字列なので break_anywhere で
                        // セグメント途中でも改行させる。3 行を超える分は末尾を
                        // overflow_character (…) で省略する。
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
mod compute_cell_size_tests {
    use super::*;

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
