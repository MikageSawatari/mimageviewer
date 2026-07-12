//! UI 描画と整形に関する小さなヘルパー関数群。
//!
//! どの関数も `&mut App` には依存せず、純粋な引数だけで動作する。
//! - 整形系: `format_bytes`, `format_count`, `truncate_name`
//! - ソート系: `natural_sort_key`, `NaturalChunk`
//! - 描画系: `draw_play_icon`, `draw_zip_badge`, `draw_pdf_badge`, `draw_histogram`, `draw_format_rows`
//! - ナビ系: `adjacent_navigable_idx`
//! - 外部連携: `open_external_player`, `open_recycle_bin_async`

use std::path::Path;

use eframe::egui;

use crate::grid_item::GridItem;

/// エラー表示の標準テキスト色。
#[allow(dead_code)]
pub(crate) const ERROR_TEXT_COLOR: eframe::egui::Color32 =
    eframe::egui::Color32::from_rgb(220, 60, 60);
/// エラー表示の標準フォントサイズ。
#[allow(dead_code)]
pub(crate) const ERROR_TEXT_SIZE: f32 = 13.0;

/// 進捗バーのラベル色（グリッド/フルスクリーン共通）。
///
/// `#[allow(dead_code)]` は lib クレート側で使用者が見えないため。実体は
/// バイナリクレート側の `ui_main` / `ui_fullscreen` から参照される。
#[allow(dead_code)]
pub(crate) const PROGRESS_LABEL_COLOR: eframe::egui::Color32 =
    eframe::egui::Color32::from_rgb(235, 240, 250);

/// 端ホバーで左右パネル (補正 / メタデータ / 音楽ブックマーク・情報 / 動画ジャンプ・情報) を
/// 「開く」トリガ帯の幅 (px)。**ビュー幅の 5%** に統一する。補正パネル (左) が元々「左端 5%」で
/// 発火していたので、機能ごとにバラバラだった当たり判定を全てこれに揃える (実機 FB 2026-07)。
/// パネル幅ぶん (292〜430px) の広い当たり判定にすると、画像の右クリックページ送りや音楽の
/// 全幅波形 seek を恒常的に食う。固定 40px はウィンドウモードで細すぎるという実機 FB もあり、
/// 幅比例 (5%) にすると小窓でも当てやすさが保たれる。lib (動画 native presenter) と bin
/// (静止画/音楽 fullscreen) の両方から参照するため、共有の `ui_helpers` に置く。
pub(crate) fn panel_edge_trigger_px(view_width: f32) -> f32 {
    view_width.max(0.0) * 0.05
}

/// 一度開いた左右パネルを「維持」するヒステリシス余白 (px)。トリガと同じく **ビュー幅の 5%**。
/// 描画パネル矩形をこの分だけ広げた範囲から出るまで閉じない。パネル内端をわずかに越えた瞬間に
/// パネルが消えるちらつきを防ぐ。
pub(crate) fn panel_hover_sustain_px(view_width: f32) -> f32 {
    view_width.max(0.0) * 0.05
}
/// 進捗バーの背景色（ポップアップ Frame の fill）。
#[allow(dead_code)]
pub(crate) const PROGRESS_BG_COLOR: eframe::egui::Color32 =
    eframe::egui::Color32::from_rgba_premultiplied(20, 25, 35, 230);
/// 通常の先読み進捗バーの塗色（濃い青）。
#[allow(dead_code)]
pub(crate) const PROGRESS_NORMAL_COLOR: eframe::egui::Color32 =
    eframe::egui::Color32::from_rgb(60, 130, 220);
/// 高画質化 / AI 先読み進捗バーの塗色（薄い青）。
#[allow(dead_code)]
pub(crate) const PROGRESS_UPGRADE_COLOR: eframe::egui::Color32 =
    eframe::egui::Color32::from_rgb(100, 170, 240);

/// tooltip と実際のカーソル形状の境界の間に空ける隙間（論理 px）。
///
/// tooltip はカーソル画像の真下（または上に出る場合はホットスポットの真上）に
/// 置き、その境界からこのぶんだけ離す。下に出るときも上に出るときも同じ値で
/// よいよう、アンカーをカーソルの実寸に合わせる（[`cursor_anchor_rect`]）。
#[allow(dead_code)]
const TOOLTIP_GAP: f32 = 6.0;

/// カーソル形状を取得できなかったときのフォールバック。ホットスポットから
/// 下方向への張り出し量（論理 px）。標準サイズの矢印カーソル相当。
#[allow(dead_code)]
const CURSOR_FALLBACK_EXTENT: f32 = 34.0;

/// フルスクリーン（黒背景）用のツールチップ枠。動画 HUD（egui 既定の dark
/// テーマ）と同じ見た目になるよう `Visuals::dark()` から一度だけ生成して使い回す。
#[allow(dead_code)]
fn dark_tooltip_frame() -> egui::Frame {
    static FRAME: std::sync::OnceLock<egui::Frame> = std::sync::OnceLock::new();
    FRAME
        .get_or_init(|| {
            let style = egui::Style {
                visuals: egui::Visuals::dark(),
                ..egui::Style::default()
            };
            egui::Frame::popup(&style)
        })
        .clone()
}

/// 現在のマウスカーソル画像がホットスポットから上下それぞれへ何ピクセル
/// 張り出しているかを物理ピクセルで `(上, 下)` の順に返す。
///
/// 矢印カーソルはホットスポットが先端付近にあるので「上」はほぼ 0 だが、
/// `ResizeHorizontal` のようにホットスポットが中央寄りの形状ではカーソル画像が
/// 上にも伸びる。tooltip を上下どちらへ反転配置してもカーソルと重ねないために
/// 両方向を測る。固定値ではなく実際のカーソルビットマップから測るので、
/// アクセシビリティ設定での拡大や高 DPI でも正しい。
#[allow(dead_code)]
fn cursor_extent_physical() -> Option<(f32, f32)> {
    // 非 Windows では計測不可 → None (呼び出し側が固定 fallback 値を使う)。
    #[cfg(not(windows))]
    {
        return None;
    }

    #[cfg(windows)]
    {
        cursor_extent_physical_win()
    }
}

#[cfg(windows)]
fn cursor_extent_physical_win() -> Option<(f32, f32)> {
    use std::ffi::c_void;
    use windows::Win32::Graphics::Gdi::{BITMAP, DeleteObject, GetObjectW, HGDIOBJ};
    use windows::Win32::UI::WindowsAndMessaging::{
        CURSOR_SHOWING, CURSORINFO, GetCursorInfo, GetIconInfo, HICON, ICONINFO,
    };

    unsafe {
        let mut ci = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        GetCursorInfo(&mut ci).ok()?;
        if (ci.flags.0 & CURSOR_SHOWING.0) == 0 || ci.hCursor.0.is_null() {
            return None;
        }
        let mut ii = ICONINFO::default();
        GetIconInfo(HICON(ci.hCursor.0), &mut ii).ok()?;

        let extent = if !ii.hbmColor.0.is_null() {
            let mut bmp = BITMAP::default();
            let written = GetObjectW(
                HGDIOBJ(ii.hbmColor.0),
                std::mem::size_of::<BITMAP>() as i32,
                Some(&mut bmp as *mut BITMAP as *mut c_void),
            );
            (written != 0).then(|| {
                let above = (ii.yHotspot as i32).max(0);
                let below = (bmp.bmHeight - above).max(0);
                (above as f32, below as f32)
            })
        } else {
            None
        };

        // GetIconInfo が複製したビットマップは呼び出し側で解放する。
        if !ii.hbmColor.0.is_null() {
            let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
        }
        if !ii.hbmMask.0.is_null() {
            let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
        }
        extent
    }
}

/// 現在のカーソル位置を基準に「カーソル画像の縦の占有範囲」を表すアンカー矩形を作る。
///
/// この矩形の真下に tooltip を出せばカーソル画像の下端の外、真上に出せば上端の
/// 外になり、egui がどちらへ反転配置してもカーソルと重ならない。
#[allow(dead_code)]
fn cursor_anchor_rect(ctx: &egui::Context) -> Option<egui::Rect> {
    let pos = ctx.pointer_hover_pos()?;
    let ppp = ctx.pixels_per_point();
    let (above, below) = match cursor_extent_physical() {
        Some((a, b)) => (a / ppp, b / ppp),
        None => (0.0, CURSOR_FALLBACK_EXTENT),
    };
    Some(egui::Rect::from_min_max(
        egui::pos2(pos.x, pos.y - above),
        egui::pos2(pos.x, pos.y + below),
    ))
}

/// hover 中のウィジェットだけカーソル実寸を計測する（毎フレーム全ウィジェットで
/// OS 呼び出しをしないためのゲート）。
#[allow(dead_code)]
fn anchor_for(resp: &egui::Response) -> Option<egui::Rect> {
    resp.contains_pointer()
        .then(|| cursor_anchor_rect(&resp.ctx))
        .flatten()
}

/// tooltip をカーソル画像の外側へずらして表示する共通処理。
#[allow(dead_code)]
fn show_offset_tooltip(
    tip: egui::Tooltip<'_>,
    dark: bool,
    anchor: Option<egui::Rect>,
    text: impl Into<egui::WidgetText>,
) {
    let mut tip = tip.gap(TOOLTIP_GAP);
    match anchor {
        Some(rect) => tip.popup = tip.popup.anchor(rect),
        None => tip = tip.at_pointer(),
    }
    if dark {
        tip.popup = tip.popup.frame(dark_tooltip_frame());
    }
    tip.show(|ui| {
        // 動的な内容で Area が縮まないよう最大幅を固定する（egui issue #5167）。
        ui.set_max_width(ui.spacing().tooltip_width);
        if dark {
            *ui.visuals_mut() = egui::Visuals::dark();
        }
        ui.add(egui::Label::new(text));
    });
}

/// `egui::Response` にカーソルと重ならないツールチップを足す拡張トレイト。
///
/// egui 標準の `on_hover_text` はウィジェット直下 4px に固定表示するため、
/// 画面上部のバーではカーソルの下に隠れてしまう。これらは tooltip を実際の
/// カーソル形状の外側（下に出るときは真下、上に出るときは真上）へ逃がす。
#[allow(dead_code)]
pub(crate) trait HoverTipExt {
    /// `on_hover_text` の差し替え。配色は現在の UI テーマに従う（メイン画面向け）。
    fn hover_tip(self, text: impl Into<egui::WidgetText>) -> Self;
    /// `hover_tip` の暗色固定版。フルスクリーン（黒背景）向け。
    fn hover_tip_dark(self, text: impl Into<egui::WidgetText>) -> Self;
    /// `on_disabled_hover_text` の差し替え（disabled なウィジェット用）。
    fn hover_tip_disabled(self, text: impl Into<egui::WidgetText>) -> Self;
}

impl HoverTipExt for egui::Response {
    fn hover_tip(self, text: impl Into<egui::WidgetText>) -> Self {
        let anchor = anchor_for(&self);
        show_offset_tooltip(egui::Tooltip::for_enabled(&self), false, anchor, text);
        self
    }

    fn hover_tip_dark(self, text: impl Into<egui::WidgetText>) -> Self {
        let anchor = anchor_for(&self);
        show_offset_tooltip(egui::Tooltip::for_enabled(&self), true, anchor, text);
        self
    }

    fn hover_tip_disabled(self, text: impl Into<egui::WidgetText>) -> Self {
        let anchor = anchor_for(&self);
        show_offset_tooltip(egui::Tooltip::for_disabled(&self), false, anchor, text);
        self
    }
}

/// 単一行 `TextEdit` に Windows 標準に近い右クリックメニューを追加する。
///
/// egui 0.33 の `TextEdit` は `show()` の戻り値から cursor state を更新できるため、
/// 入力中の IME composition には触れず、メニュー操作で確定済み文字列だけを編集する。
/// 戻り値はテキスト内容を変更したかどうか。
#[allow(dead_code)]
pub(crate) fn singleline_text_edit_context_menu(
    _ui: &mut egui::Ui,
    output: &mut egui::widgets::text_edit::TextEditOutput,
    text: &mut String,
) -> bool {
    use egui::TextBuffer as _;
    use egui::text::{CCursor, CCursorRange};

    let id = output.response.id;
    let state = output.state.clone();
    let char_count = text.chars().count();
    let selection = clamp_cursor_range(
        output.cursor_range.or_else(|| state.cursor.char_range()),
        char_count,
    );
    let has_selection = !selection.is_empty();
    let selected_text = has_selection.then(|| selection.slice_str(text).to_owned());

    let mut changed = false;
    output.response.context_menu(|ui| {
        let paste_text = read_clipboard_text().map(|s| singleline_clipboard_text(&s));
        let can_paste = paste_text.as_ref().is_some_and(|s| !s.is_empty());

        if ui
            .add_enabled(has_selection, egui::Button::new("切り取り"))
            .clicked()
        {
            if let Some(selected) = selected_text.as_ref() {
                ui.ctx().copy_text(selected.clone());
                let range = selection.as_sorted_char_range();
                let cursor = CCursor::new(range.start);
                text.delete_char_range(range);
                store_text_edit_cursor(ui.ctx(), id, &state, CCursorRange::one(cursor));
                output.response.request_focus();
                changed = true;
            }
            ui.close();
        }
        if ui
            .add_enabled(has_selection, egui::Button::new("コピー"))
            .clicked()
        {
            if let Some(selected) = selected_text.as_ref() {
                ui.ctx().copy_text(selected.clone());
            }
            output.response.request_focus();
            ui.close();
        }
        if ui
            .add_enabled(can_paste, egui::Button::new("貼り付け"))
            .clicked()
        {
            if let Some(paste) = paste_text.as_ref() {
                let range = selection.as_sorted_char_range();
                let insert_at = range.start;
                if range.start != range.end {
                    text.delete_char_range(range);
                }
                let inserted = text.insert_text(paste, insert_at);
                store_text_edit_cursor(
                    ui.ctx(),
                    id,
                    &state,
                    CCursorRange::one(CCursor::new(insert_at + inserted)),
                );
                output.response.request_focus();
                changed = true;
            }
            ui.close();
        }
        if ui
            .add_enabled(char_count > 0, egui::Button::new("すべて選択"))
            .clicked()
        {
            store_text_edit_cursor(
                ui.ctx(),
                id,
                &state,
                CCursorRange::two(CCursor::new(0), CCursor::new(char_count)),
            );
            output.response.request_focus();
            ui.close();
        }
    });

    if changed {
        output.response.mark_changed();
    }
    changed
}

fn clamp_cursor_range(
    range: Option<egui::text::CCursorRange>,
    char_count: usize,
) -> egui::text::CCursorRange {
    use egui::text::{CCursor, CCursorRange};

    let Some(mut range) = range else {
        return CCursorRange::one(CCursor::new(char_count));
    };
    range.primary.index = range.primary.index.min(char_count);
    range.secondary.index = range.secondary.index.min(char_count);
    range
}

fn store_text_edit_cursor(
    ctx: &egui::Context,
    id: egui::Id,
    state: &egui::widgets::text_edit::TextEditState,
    range: egui::text::CCursorRange,
) {
    let mut state = state.clone();
    state.cursor.set_char_range(Some(range));
    state.store(ctx, id);
}

fn singleline_clipboard_text(text: &str) -> String {
    normalize_clipboard_newlines(text).replace('\n', " ")
}

fn normalize_clipboard_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(windows)]
fn read_clipboard_text() -> Option<String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    unsafe {
        if OpenClipboard(Some(HWND::default())).is_err() {
            return None;
        }
        let hmem = match GetClipboardData(CF_UNICODETEXT.0 as u32) {
            Ok(h) => h,
            Err(_) => {
                let _ = CloseClipboard();
                return None;
            }
        };
        if hmem.is_invalid() {
            let _ = CloseClipboard();
            return None;
        }
        let global = windows::Win32::Foundation::HGLOBAL(hmem.0);
        let ptr = GlobalLock(global) as *const u16;
        if ptr.is_null() {
            let _ = CloseClipboard();
            return None;
        }
        let max_u16 = GlobalSize(global) / 2;
        let mut len = 0usize;
        while len < max_u16 && *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        let text = String::from_utf16_lossy(slice);
        let _ = GlobalUnlock(global);
        let _ = CloseClipboard();
        if text.is_empty() { None } else { Some(text) }
    }
}

#[cfg(not(windows))]
fn read_clipboard_text() -> Option<String> {
    None
}

// -----------------------------------------------------------------------
// 検索バーの OR チェック (3 検索バー共通、docs §20)
// -----------------------------------------------------------------------

/// 検索バー右端の `□OR` チェックを描画し、**値が変化したかどうか**を返す。
/// 3 種類の検索バー (Ctrl+F / Ctrl+S / Ctrl+G) で同じ見た目・同じツールチップを使うため共通化。
pub fn or_mode_checkbox(ui: &mut eframe::egui::Ui, or_mode: &mut bool) -> bool {
    let before = *or_mode;
    ui.checkbox(or_mode, "OR")
        .on_hover_text("オン: 語をいずれか含む / オフ: すべて含む (除外 -word は常に AND)");
    before != *or_mode
}

// -----------------------------------------------------------------------
// ファイルメタデータ
// -----------------------------------------------------------------------

/// `std::fs::Metadata` から mtime を UNIX epoch 秒として返す。取得失敗時は 0。
pub fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

// -----------------------------------------------------------------------
// バイト数 / 件数の整形
// -----------------------------------------------------------------------

/// バイト数を MB / GB 単位の文字列にフォーマットする (キャッシュ管理ダイアログ用)。
pub fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// 詳細表示のサイズ列用フォーマット。固定単位では整数部を 3 桁区切りにする。
pub fn format_details_size(bytes: u64, mode: crate::settings::DetailsSizeDisplayMode) -> String {
    use crate::settings::DetailsSizeDisplayMode;

    match mode {
        DetailsSizeDisplayMode::Optimal => format_details_size_optimal(bytes),
        DetailsSizeDisplayMode::FixedBytes => format_count(bytes),
        DetailsSizeDisplayMode::FixedKb => format_grouped_decimal(bytes as f64 / 1024.0, 1, " KB"),
        DetailsSizeDisplayMode::FixedMb => {
            format_grouped_decimal(bytes as f64 / (1024.0 * 1024.0), 2, " MB")
        }
    }
}

fn format_details_size_optimal(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", format_count(bytes))
    } else if bytes < 1024 * 1024 {
        format_grouped_decimal(bytes as f64 / 1024.0, 1, " KB")
    } else if bytes < 1024 * 1024 * 1024 {
        format_grouped_decimal(bytes as f64 / (1024.0 * 1024.0), 1, " MB")
    } else {
        format_grouped_decimal(bytes as f64 / (1024.0 * 1024.0 * 1024.0), 2, " GB")
    }
}

fn format_grouped_decimal(value: f64, decimals: usize, suffix: &str) -> String {
    let raw = format!("{value:.decimals$}");
    let (int_part, frac_part) = raw.split_once('.').unwrap_or((raw.as_str(), ""));
    let grouped = format_count(int_part.parse::<u64>().unwrap_or(0));
    if decimals == 0 {
        format!("{grouped}{suffix}")
    } else {
        format!("{grouped}.{frac_part}{suffix}")
    }
}

/// 小さいバイト数 (サムネイル単体) を KB / MB の文字列にフォーマット。
pub fn format_bytes_small(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

/// 秒を mm:ss / hh:mm:ss にフォーマット (動画 HUD / シークラベル / ジャンプ行など共通)。
/// 分・秒は 2 桁ゼロ詰め。時はゼロ詰めなし (= "1:23:45")。
pub fn format_hms(secs: f64) -> String {
    let total = secs.max(0.0).round() as i64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// 平均ビットレートを Mbps / kbps の人間可読表記にフォーマット (上ホバー右側情報用)。
pub fn format_bitrate_bps(bps: i64) -> String {
    if bps >= 1_000_000 {
        format!("{:.1} Mbps", bps as f64 / 1_000_000.0)
    } else if bps >= 1_000 {
        format!("{} kbps", bps / 1_000)
    } else {
        format!("{bps} bps")
    }
}

/// 整数を 3 桁区切りにフォーマット (例: 1234 → "1,234")
pub fn format_count(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

/// 名前を `max_chars` 文字以内にトリミングし、超過時は末尾に "…" を付ける。
pub fn truncate_name(name: &str, max_chars: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max_chars {
        name.to_owned()
    } else {
        chars[..max_chars - 1].iter().collect::<String>() + "…"
    }
}

/// グリッドセル底部にファイル名を描画する。文字の後ろに半透明の角丸プレートを敷いて、
/// 動画サムネの黒帯のような暗部に重なってもファイル名が読めるようにする。
///
/// - 文字幅は `layout_no_wrap` で実測 (CJK / 絵文字混在でも正確)。プレート全体が
///   `inner` を超える場合は `truncate_name` の 18 文字 soft cap から末尾を `…` で
///   削って実幅に収める。極小セル (`MIN_CELL_PX = 32`) で `…` も入らないなら描画諦め
/// - `reserve_left_w` は `draw_zip_badge` / `draw_pdf_badge` / `draw_archive_badge`
///   が左下に描く ASCII 3 文字バッジ分の予約幅 (`estimated_file_badge_width` を渡す)。
///   バッジの無いセル (Folder / Video) では 0.0
/// - プレートは dark mode で半透明黒、light mode で半透明白
/// - 位置は `Align2::CENTER_BOTTOM` 相当、`inner.max.y - 4.0` を底とする
pub fn draw_cell_filename(
    painter: &egui::Painter,
    inner: egui::Rect,
    name: &str,
    text_color: egui::Color32,
    dark: bool,
    reserve_left_w: f32,
) {
    let font = crate::ui_fonts::user_text_font(11.0);
    let plate_pad_x = 4.0;
    let plate_top_pad = 4.0;
    let plate_bottom_pad = 1.0;
    let outer_margin = 3.0;

    // プレート利用可能領域: 左は max(outer_margin, バッジ予約幅) ぶんだけ右に寄せる。
    let avail_left = inner.min.x + reserve_left_w.max(outer_margin);
    let avail_right = inner.max.x - outer_margin;
    let max_text_w = (avail_right - avail_left - plate_pad_x * 2.0).max(0.0);
    if max_text_w < 4.0 {
        return; // 領域不足 → 描画諦め
    }
    let center_x = (avail_left + avail_right) * 0.5;

    // 18 文字までで初回 layout、はみ出していたら末尾を `…` で 1 文字ずつ削って再 layout。
    // CJK と ASCII で文字幅が大きく違うので平均幅近似は使えない (`draw_tag_badges` と同じ手法)。
    let initial = truncate_name(name, 18);
    let mut galley = painter.layout_no_wrap(initial.clone(), font.clone(), text_color);
    if galley.size().x > max_text_w {
        let chars: Vec<char> = initial.chars().collect();
        for take in (1..chars.len()).rev() {
            let candidate: String = chars[..take].iter().collect::<String>() + "…";
            let g = painter.layout_no_wrap(candidate, font.clone(), text_color);
            if g.size().x <= max_text_w {
                galley = g;
                break;
            }
        }
        if galley.size().x > max_text_w {
            galley = painter.layout_no_wrap("…".to_string(), font.clone(), text_color);
            if galley.size().x > max_text_w {
                return; // `…` も入らない極小セル → 諦め
            }
        }
    }

    let text_size = galley.size();
    let bg_h = text_size.y + plate_top_pad + plate_bottom_pad;
    let bg_rect = egui::Rect::from_min_size(
        egui::pos2(
            center_x - (text_size.x + plate_pad_x * 2.0) / 2.0,
            inner.max.y - 3.0 - bg_h,
        ),
        egui::vec2(text_size.x + plate_pad_x * 2.0, bg_h),
    );
    let text_pos = bg_rect.left_top() + egui::vec2(plate_pad_x, plate_top_pad);
    let bg_color = if dark {
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160)
    } else {
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 220)
    };
    painter.rect_filled(bg_rect, 3.0, bg_color);
    painter.galley(text_pos, galley, text_color);
}

/// `draw_zip_badge` / `draw_pdf_badge` / `draw_archive_badge` が左下に描く ASCII 3 文字
/// バッジの幅を pessimistic に見積もって、`draw_cell_filename` の `reserve_left_w`
/// に渡す値を返す。`draw_file_badge` の font_size = `clamp(h*0.10, 9, 16)` に合わせて
/// `badge_w ≈ font_size * 2.65` (3 文字 ASCII 大文字 + pad_h*2) を `font_size * 3.0`
/// に余裕を持たせ、左マージン 3.0 と視覚的隙間 4.0 を加算する。
pub fn estimated_file_badge_width(inner: egui::Rect) -> f32 {
    let font_size = (inner.height() * 0.10).clamp(9.0, 16.0);
    3.0 + font_size * 3.0 + 4.0
}

// -----------------------------------------------------------------------
// 自然順ソート
// -----------------------------------------------------------------------

/// 自然順ソート用のキーを返す。
/// ファイル名を「テキスト部分」と「数字部分」に分割し、
/// 数字部分は数値として比較するので 1 < 2 < 9 < 10 < 11 となる。
///
/// テキスト部分は記号・空白・句読点を除去して英数字 (ASCII letter / CJK 等を含む
/// `char::is_alphanumeric`) のみを残してから比較する。これにより
/// `foo#1.jpg` と `foo# 2.jpg` のように区切り記号や空白の有無が混在しても
/// 番号本体で比較される (空白の有無で `# 2` が `#10` の後ろに回るのを防ぐ)。
///
/// 全文字が記号で構成された区間は空文字列になるため、そのチャンクは破棄する
/// (`#1.jpg` と `1.jpg` の natural key を一致させ、最終順序は呼び出し側の
/// tiebreak (`SortOrder::compare`) に委ねる)。
pub fn natural_sort_key(name: &str) -> Vec<NaturalChunk> {
    let name_lower = name.to_lowercase();
    let mut chunks = Vec::new();
    let mut chars = name_lower.chars().peekable();
    while let Some(&c) = chars.peek() {
        if natural_digit_value(c).is_some() {
            let mut n = 0_u64;
            while let Some(digit) = chars.peek().and_then(|ch| natural_digit_value(*ch)) {
                chars.next();
                n = n.saturating_mul(10).saturating_add(digit as u64);
            }
            chunks.push(NaturalChunk::Num(n));
        } else {
            let mut text = String::new();
            while let Some(&ch) = chars.peek() {
                if natural_digit_value(ch).is_some() {
                    break;
                }
                chars.next();
                if ch.is_alphanumeric() {
                    text.push(ch);
                }
            }
            if !text.is_empty() {
                chunks.push(NaturalChunk::Text(text));
            }
        }
    }
    chunks
}

fn natural_digit_value(ch: char) -> Option<u32> {
    if ch.is_ascii_digit() {
        return Some(ch as u32 - '0' as u32);
    }
    if ('\u{ff10}'..='\u{ff19}').contains(&ch) {
        return Some(ch as u32 - '\u{ff10}' as u32);
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NaturalChunk {
    Text(String),
    Num(u64),
}

// -----------------------------------------------------------------------
// 描画ヘルパー
// -----------------------------------------------------------------------

/// 動画サムネイル中央に表示する「再生ボタン」(半透明黒円 + 白三角) を描画する。
pub fn draw_play_icon(painter: &egui::Painter, center: egui::Pos2, radius: f32) {
    // 背景円
    painter.circle_filled(
        center,
        radius,
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160),
    );
    // 右向き三角形（ポリゴン）
    // 視覚的中心を合わせるため若干右にオフセット
    let tr = radius * 0.45;
    let cx = center.x + tr * 0.12;
    let cy = center.y;
    let points = vec![
        egui::pos2(cx - tr * 0.55, cy - tr * 0.9), // 左上
        egui::pos2(cx - tr * 0.55, cy + tr * 0.9), // 左下
        egui::pos2(cx + tr * 0.95, cy),            // 右頂点
    ];
    painter.add(egui::Shape::convex_polygon(
        points,
        egui::Color32::WHITE,
        egui::Stroke::NONE,
    ));
}

/// グリッドの音声セル用に、固定の音楽アイコン (2 連八分音符) をベクター描画する。
///
/// 絵文字グリフ (🎵 / 🎶 等) は環境依存フォントで tofu 化しうる (CLAUDE.md「UI 文字列の
/// Unicode グリフ選定ルール」)。動画セルの再生アイコン (`draw_play_icon`) と同様に
/// painter プリミティブで描いてフォント依存を避ける。
pub fn draw_music_icon(painter: &egui::Painter, inner: egui::Rect, dark: bool) {
    let side = inner.width().min(inner.height());
    let s = (side * 0.34).clamp(22.0, 64.0);
    let center = inner.center() - egui::vec2(0.0, side * 0.05);
    let color = if dark {
        egui::Color32::from_rgb(150, 182, 222)
    } else {
        egui::Color32::from_rgb(70, 112, 162)
    };
    let head_r = s * 0.22;
    let stem_h = s * 0.92;
    let gap = s * 0.72;
    let stem_w = (s * 0.07).max(1.6);
    let left_head = egui::pos2(center.x - gap * 0.5, center.y + stem_h * 0.32);
    let right_head = egui::pos2(center.x + gap * 0.5, center.y + stem_h * 0.32);
    let left_stem_x = left_head.x + head_r * 0.9;
    let right_stem_x = right_head.x + head_r * 0.9;
    let stem_top_y = left_head.y - stem_h;
    // 符幹 (符頭の右端から上へ)
    painter.line_segment(
        [
            egui::pos2(left_stem_x, left_head.y),
            egui::pos2(left_stem_x, stem_top_y),
        ],
        egui::Stroke::new(stem_w, color),
    );
    painter.line_segment(
        [
            egui::pos2(right_stem_x, right_head.y),
            egui::pos2(right_stem_x, stem_top_y),
        ],
        egui::Stroke::new(stem_w, color),
    );
    // 連桁 (2 本の符幹の上端をつなぐ太線)
    painter.line_segment(
        [
            egui::pos2(left_stem_x - stem_w * 0.5, stem_top_y),
            egui::pos2(right_stem_x + stem_w * 0.5, stem_top_y),
        ],
        egui::Stroke::new(stem_w * 1.9, color),
    );
    // 符頭
    painter.circle_filled(left_head, head_r, color);
    painter.circle_filled(right_head, head_r, color);
}

/// フルスクリーン右パネル共通の ★ レーティング行を描く (画像 / 動画 / 音声で共有)。
///
/// クリックされたら**解決済みの新しい値**を `Some` で返す: 現在値と同じ★を再クリックした
/// 場合は 0 (解除)、それ以外はクリックした★の数。呼び出し側はその値を保存経路
/// (`set_rating` / `NativeOverlayCommand::SetRating` 等) に流すだけでよい。★ は Yu Gothic に
/// 含まれフォント安全 (グリッドセルでも使用、CLAUDE.md グリフポリシー)。
pub fn draw_rating_stars(ui: &mut egui::Ui, current: u8) -> Option<u8> {
    let mut result: Option<u8> = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for star in 1..=5u8 {
            let filled = star <= current;
            let color = if filled {
                egui::Color32::from_rgb(255, 205, 70)
            } else {
                egui::Color32::from_gray(96)
            };
            // Label はクリック可能にしてもホバー時に I 字 (テキスト) カーソルになるため、
            // クリックできる★であることが分かるよう PointingHand を明示する。
            let resp = ui
                .add(
                    egui::Label::new(egui::RichText::new("★").color(color).size(20.0))
                        .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.clicked() {
                result = Some(if star == current { 0 } else { star });
            }
        }
    });
    result
}

/// サムネイル左下にファイル種別バッジを描画する共通関数。
fn draw_file_badge(painter: &egui::Painter, cell_rect: egui::Rect, label: &str, bg: egui::Color32) {
    let font_size = (cell_rect.height() * 0.10).clamp(9.0, 16.0);
    let pad_h = font_size * 0.35;
    let pad_v = font_size * 0.2;
    let galley = painter.layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(font_size),
        egui::Color32::WHITE,
    );
    let text_size = galley.size();
    let badge_w = text_size.x + pad_h * 2.0;
    let badge_h = text_size.y + pad_v * 2.0;
    let badge_rect = egui::Rect::from_min_size(
        egui::pos2(cell_rect.min.x + 3.0, cell_rect.max.y - badge_h - 3.0),
        egui::vec2(badge_w, badge_h),
    );
    painter.rect_filled(badge_rect, 3.0, bg);
    painter.galley(
        egui::pos2(badge_rect.min.x + pad_h, badge_rect.min.y + pad_v),
        galley,
        egui::Color32::WHITE,
    );
}

/// ZIP アーカイブ内画像のサムネイルに表示するバッジ（左下、青系）。
pub fn draw_zip_badge(painter: &egui::Painter, cell_rect: egui::Rect) {
    draw_file_badge(
        painter,
        cell_rect,
        "ZIP",
        egui::Color32::from_rgba_unmultiplied(30, 80, 160, 200),
    );
}

/// PDF ページのサムネイルに表示するバッジ（左下、赤系）。
pub fn draw_pdf_badge(painter: &egui::Painter, cell_rect: egui::Rect) {
    draw_file_badge(
        painter,
        cell_rect,
        "PDF",
        egui::Color32::from_rgba_unmultiplied(180, 30, 30, 200),
    );
}

/// 変換対象アーカイブ (RAR / 7z / LZH) のサムネイルに表示するバッジ（左下、橙系）。
/// `label` は "RAR" / "7z" / "LZH" など形式表示。
pub fn draw_archive_badge(painter: &egui::Painter, cell_rect: egui::Rect, label: &str) {
    draw_file_badge(
        painter,
        cell_rect,
        label,
        egui::Color32::from_rgba_unmultiplied(200, 110, 20, 200),
    );
}

/// 左下に描くコンテナバッジ (folder/zip/pdf/archive) の標準的な高さ。
/// 各 `draw_*_badge` 関数と同じ `font_size = (cell.height * 0.10).clamp(9.0, 16.0)` +
/// `pad_v = font_size * 0.2` の計算に追従し、`galley.size().y` を `font_size * 1.4` で
/// 安全側に近似する (= painter を持たない呼び出し元から呼べるよう text metric を見ない)。
///
/// 用途: レーティング ★ バッジなど **左下に並ぶ別オーバーレイ** を、コンテナバッジに
/// 重ねず縦に積むときの y オフセット計算。1 バッジぶん分の高さを取りたいときに使う。
///
/// ⚠ **`cell_rect` 引数には `draw_*_badge` と同じ `inner` (= 外周 padding を引いた
/// 内側 rect) を渡すこと**。outer rect を渡すと `cell.height` がわずかに大きく見えて
/// `font_size` が `draw_*_badge` 側と乖離し、レーティング配置に 1-2 px の食い込みが
/// 発生する (実機フィードバックで判明)。
pub fn container_badge_height(cell_rect: egui::Rect) -> f32 {
    let font_size = (cell_rect.height() * 0.10).clamp(9.0, 16.0);
    let pad_v = font_size * 0.2;
    font_size * 1.4 + pad_v * 2.0
}

/// フォルダサムネイルに表示するバッジ（左下、緑系、フォルダ名表示）。
pub fn draw_folder_badge(painter: &egui::Painter, cell_rect: egui::Rect, folder_name: &str) {
    let font_size = (cell_rect.height() * 0.10).clamp(9.0, 16.0);
    let pad_h = font_size * 0.35;
    let pad_v = font_size * 0.2;
    let max_badge_w = cell_rect.width() * 0.80;
    // フォルダ名が長い場合は切り詰める
    let mut label = folder_name.to_string();
    let bg = egui::Color32::from_rgba_unmultiplied(40, 130, 60, 200);
    loop {
        let galley = painter.layout_no_wrap(
            label.clone(),
            egui::FontId::proportional(font_size),
            egui::Color32::WHITE,
        );
        let badge_w = galley.size().x + pad_h * 2.0;
        // 終了条件は **文字数** で見る。byte 長で `label.len() <= 2` を見てしまうと
        // `"…"` が UTF-8 3 バイトなので `"X…"` の `label.len()` が常に 4 となり、極端に狭い
        // セルでは同じ `"X…"` を作り続けて無限ループになる (`painter.layout_no_wrap` が
        // `egui::Context::write` を毎反復取得して UI が固まる)。
        let chars_count = label.chars().count();
        if badge_w <= max_badge_w || chars_count <= 2 {
            let badge_h = galley.size().y + pad_v * 2.0;
            let badge_rect = egui::Rect::from_min_size(
                egui::pos2(cell_rect.min.x + 3.0, cell_rect.max.y - badge_h - 3.0),
                egui::vec2(badge_w, badge_h),
            );
            painter.rect_filled(badge_rect, 3.0, bg);
            painter.galley(
                egui::pos2(badge_rect.min.x + pad_h, badge_rect.min.y + pad_v),
                galley,
                egui::Color32::WHITE,
            );
            return;
        }
        // chars_count > 2 を上で確認済みなので `keep < chars_count` が保証され、
        // chars_count が必ず減って次反復で停止条件に到達する (= 進捗保証)。
        let chars: Vec<char> = label.chars().collect();
        let keep = chars.len().saturating_sub(2).max(1);
        label = chars[..keep].iter().collect::<String>() + "…";
    }
}

/// 統計ダイアログのヒストグラムを ASCII バー + 件数で描画する。
/// `label_fn` がバケットインデックスから左端ラベルを返す。
/// 統計ダイアログ用: ヒストグラムを egui::Grid で描画する。
///
/// 各バケットを「ラベル | バー | 件数」の 3 列グリッドで表示。
/// `avg_times` が Some のとき、4 列目に平均ロード時間を表示する。
pub fn draw_histogram(
    ui: &mut egui::Ui,
    hist: &[u64],
    label_fn: impl Fn(usize) -> String,
    avg_times: Option<&[f64]>,
) {
    const MAX_BAR_WIDTH: usize = 24;
    let max_count = hist.iter().copied().max().unwrap_or(0);
    if max_count == 0 {
        ui.label("  (データなし)");
        return;
    }

    let mono = egui::FontId::monospace(12.0);
    egui::Grid::new(ui.next_auto_id())
        .spacing([4.0, 1.0])
        .show(ui, |ui| {
            for (bucket, &count) in hist.iter().enumerate() {
                // ラベル (右寄せ)
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(label_fn(bucket)).font(mono.clone()));
                });
                // バー
                let bar_len = ((count as f64 / max_count as f64) * MAX_BAR_WIDTH as f64) as usize;
                let bar: String = "\u{2588}".repeat(bar_len);
                ui.label(
                    egui::RichText::new(format!(
                        "{bar:<MAX_BAR_WIDTH$}",
                        MAX_BAR_WIDTH = MAX_BAR_WIDTH
                    ))
                    .font(mono.clone())
                    .color(egui::Color32::from_rgb(80, 140, 220)),
                );
                // 件数 (右寄せ)
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format_count(count)).font(mono.clone()));
                });
                // 平均時間 (オプション)
                if let Some(times) = avg_times {
                    let avg = if count > 0 {
                        times.get(bucket).copied().unwrap_or(0.0) / count as f64
                    } else {
                        0.0
                    };
                    let text = if count > 0 {
                        format!("({:.0} ms)", avg)
                    } else {
                        String::new()
                    };
                    ui.label(egui::RichText::new(text).font(mono.clone()).weak());
                }
                ui.end_row();
            }
        });
}

/// 統計ダイアログ用: フォーマット別件数を egui::Grid で描画する。
///
/// 各行を「ラベル | バー | 件数 | 平均時間」の 4 列グリッドで表示。
pub fn draw_format_rows(ui: &mut egui::Ui, rows: &[(&str, u64, f64)]) {
    const MAX_BAR_WIDTH: usize = 24;
    let max_count = rows.iter().map(|(_, c, _)| *c).max().unwrap_or(0);
    if max_count == 0 {
        ui.label("  (データなし)");
        return;
    }
    let mono = egui::FontId::monospace(12.0);
    egui::Grid::new(ui.next_auto_id())
        .spacing([4.0, 1.0])
        .show(ui, |ui| {
            for (label, count, total_time) in rows {
                // ラベル
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(*label).font(mono.clone()));
                });
                // バー
                let bar_len = ((*count as f64 / max_count as f64) * MAX_BAR_WIDTH as f64) as usize;
                let bar: String = "\u{2588}".repeat(bar_len);
                ui.label(
                    egui::RichText::new(format!(
                        "{bar:<MAX_BAR_WIDTH$}",
                        MAX_BAR_WIDTH = MAX_BAR_WIDTH
                    ))
                    .font(mono.clone())
                    .color(egui::Color32::from_rgb(80, 140, 220)),
                );
                // 件数
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format_count(*count)).font(mono.clone()));
                });
                // 平均時間
                let avg_text = if *count > 0 {
                    format!("({:.0} ms)", total_time / *count as f64)
                } else {
                    String::new()
                };
                ui.label(egui::RichText::new(avg_text).font(mono.clone()).weak());
                ui.end_row();
            }
        });
}

// -----------------------------------------------------------------------
// アイテムナビゲーション
// -----------------------------------------------------------------------

/// items の中で current から delta 分（±1）移動した「表示可能」アイテム
/// (画像 + 動画 + ZIP 画像 + ZIP セパレータ) の item index を返す。
/// 境界では None を返す（ラップアラウンドなし）。
/// `display_order` (フィルタ適用済み・詳細ソート適用済み) の中からナビゲーション可能な
/// 前後のアイテムインデックスを返す。
pub fn adjacent_navigable_idx(
    items: &[GridItem],
    display_order: &[usize],
    current: usize,
    delta: i32,
) -> Option<usize> {
    // display_order の中でナビゲーション可能なもの (画像・動画・音声・セパレータ)。
    // 音声 (Audio) は「映像なし動画」として動画と同じ前/次項目ナビの対象にする
    // (plain ↓↑ = VideoNextFile/PrevFile で移動、2026-07-03 実機FB)。スライドショー送り
    // (adjacent_slideshow_idx) と Home/End ページジャンプ (page_jump_nav_indices) は
    // 別フィルタで Audio を除外済み (音声はスライドショー/ページ送り対象外)。
    let nav_indices: Vec<usize> = display_order
        .iter()
        .copied()
        .filter(|&i| {
            matches!(
                items.get(i),
                Some(GridItem::Image(_))
                    | Some(GridItem::Video(_))
                    | Some(GridItem::Audio(_))
                    | Some(GridItem::ZipImage { .. })
                    | Some(GridItem::ZipSeparator { .. })
                    | Some(GridItem::PdfPage { .. })
            )
        })
        .collect();
    if nav_indices.is_empty() {
        return None;
    }
    if let Some(pos) = nav_indices.iter().position(|&i| i == current) {
        let new_pos = (pos as i32 + delta).clamp(0, nav_indices.len() as i32 - 1) as usize;
        if new_pos == pos {
            None
        } else {
            Some(nav_indices[new_pos])
        }
    } else if delta > 0 {
        nav_indices.iter().copied().filter(|&i| i > current).min()
    } else if delta < 0 {
        nav_indices.iter().copied().filter(|&i| i < current).max()
    } else {
        None
    }
}

fn page_jump_nav_indices(items: &[GridItem], display_order: &[usize]) -> Vec<usize> {
    display_order
        .iter()
        .copied()
        .filter(|&i| {
            matches!(
                items.get(i),
                Some(GridItem::Image(_))
                    | Some(GridItem::ZipImage { .. })
                    | Some(GridItem::PdfPage { .. })
            )
        })
        .collect()
}

fn jump_page_idx_from_nav_indices(
    nav_indices: &[usize],
    current: usize,
    step: usize,
    forward: bool,
) -> Option<usize> {
    if nav_indices.is_empty() {
        return None;
    }
    let step = step.max(1);
    if let Some(pos) = nav_indices.iter().position(|&i| i == current) {
        let target_pos = if forward {
            pos.saturating_add(step).min(nav_indices.len() - 1)
        } else {
            pos.saturating_sub(step)
        };
        if target_pos == pos {
            None
        } else {
            Some(nav_indices[target_pos])
        }
    } else if forward {
        nav_indices.iter().copied().filter(|&i| i > current).min()
    } else {
        nav_indices.iter().copied().filter(|&i| i < current).max()
    }
}

/// ページ総数と比率から、割合ジャンプで使うページ数を返す。
/// 端数は切り上げ、薄い本でも最低 1 ページは進む。
pub fn percent_jump_page_step(total_pages: usize, percent: u32) -> usize {
    if total_pages == 0 {
        return 1;
    }
    let percent = percent.max(1) as usize;
    (total_pages * percent).div_ceil(100).max(1)
}

/// items の中で current から `step` 件ぶん前後へ移動した画像ページ
/// (通常画像 + ZIP 画像 + PDF ページ) の item index を返す。
pub fn fixed_jump_page_idx(
    items: &[GridItem],
    display_order: &[usize],
    current: usize,
    step: usize,
    forward: bool,
) -> Option<usize> {
    let nav_indices = page_jump_nav_indices(items, display_order);
    jump_page_idx_from_nav_indices(&nav_indices, current, step, forward)
}

/// 設定に応じた大きめジャンプの target を返す。
pub fn large_jump_page_idx(
    items: &[GridItem],
    display_order: &[usize],
    current: usize,
    mode: crate::settings::FullscreenJumpMode,
    percent: u32,
    fixed_count: usize,
    min_step: usize,
    forward: bool,
) -> Option<usize> {
    let nav_indices = page_jump_nav_indices(items, display_order);
    let step = match mode {
        crate::settings::FullscreenJumpMode::Percent => {
            percent_jump_page_step(nav_indices.len(), percent)
        }
        crate::settings::FullscreenJumpMode::FixedPages => fixed_count.max(1),
    }
    .max(min_step.max(1));
    jump_page_idx_from_nav_indices(&nav_indices, current, step, forward)
}

/// スライドショー送り用の隣接探索。`adjacent_navigable_idx` と同じだが
/// **`GridItem::Video` を除外**する (スライドショー中は動画をスキップして継続するため)。
/// `GridItem::ZipSeparator` は仕様どおり残す (章タイトルも同じ間隔で表示する)。
/// 境界では None を返す (ラップアラウンドなし)。
pub fn adjacent_slideshow_idx(
    items: &[GridItem],
    display_order: &[usize],
    current: usize,
    delta: i32,
) -> Option<usize> {
    let nav_indices: Vec<usize> = display_order
        .iter()
        .copied()
        .filter(|&i| {
            matches!(
                items.get(i),
                Some(GridItem::Image(_))
                    | Some(GridItem::ZipImage { .. })
                    | Some(GridItem::ZipSeparator { .. })
                    | Some(GridItem::PdfPage { .. })
            )
        })
        .collect();
    if nav_indices.is_empty() {
        return None;
    }
    if let Some(pos) = nav_indices.iter().position(|&i| i == current) {
        let new_pos = (pos as i32 + delta).clamp(0, nav_indices.len() as i32 - 1) as usize;
        if new_pos == pos {
            None
        } else {
            Some(nav_indices[new_pos])
        }
    } else if delta > 0 {
        nav_indices.iter().copied().filter(|&i| i > current).min()
    } else if delta < 0 {
        nav_indices.iter().copied().filter(|&i| i < current).max()
    } else {
        None
    }
}

/// スライドショーの折り返し / 先頭着地用に、`display_order` の中で先頭の
/// 静止画系アイテム (Image / ZipImage / PdfPage、Video と ZipSeparator は除外) を返す。
/// LoopFolder の折り返し先は章タイトル (ZipSeparator) ではなく実画像にするため、
/// `adjacent_slideshow_idx` のフィルタとは別に separator も除外する。
pub fn first_slideshow_still_idx(items: &[GridItem], display_order: &[usize]) -> Option<usize> {
    display_order.iter().copied().find(|&i| {
        matches!(
            items.get(i),
            Some(GridItem::Image(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::PdfPage { .. })
        )
    })
}

/// `display_order` の中の「ナビゲーション可能」なアイテム列から、
/// 末尾 (`last=true`) または先頭 (`last=false`) の item index を返す。
/// `adjacent_navigable_idx` と同じフィルタを適用する。
pub fn boundary_navigable_idx(
    items: &[GridItem],
    display_order: &[usize],
    last: bool,
) -> Option<usize> {
    let mut iter = display_order.iter().copied().filter(|&i| {
        matches!(
            items.get(i),
            Some(GridItem::Image(_))
                | Some(GridItem::Video(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::ZipSeparator { .. })
                | Some(GridItem::PdfPage { .. })
        )
    });
    if last { iter.last() } else { iter.next() }
}

// -----------------------------------------------------------------------
// 外部連携
// -----------------------------------------------------------------------

/// パスに関連付けられたデフォルトアプリケーションで開く (動画は外部プレイヤー、
/// ディレクトリは Explorer)。
///
/// 内部は `opener::open` 経由で Windows の `ShellExecuteW` を直接呼ぶ。パスは
/// `lpFile` に wide-string データとして渡るので、`cmd /c start ...` のように
/// シェルメタ文字 (`&` `^` `|` `"` 等) を含むファイル名がコマンドとして解釈される
/// ことはない。`cmd.exe` のコンソールウィンドウも出ない。
///
/// セキュリティ: ファイル名は信頼境界の外なので、シェル経由で起動してはならない。
pub fn open_external_player(path: &Path) {
    crate::logger::log(format!("open_external_player: {}", path.display()));
    if let Err(e) = opener::open(path) {
        crate::logger::log(format!("open_external_player failed: {e}"));
    }
}

/// Windows のゴミ箱を Explorer で開く。
///
/// 固定の Shell namespace URI だけを渡し、ユーザー由来のパスは扱わない。
pub fn open_recycle_bin_async() {
    let spawn_result = std::thread::Builder::new()
        .name("open-recycle-bin".into())
        .spawn(|| {
            #[cfg(windows)]
            {
                const RECYCLE_BIN_SHELL_URI: &str = "shell:RecycleBinFolder";
                crate::logger::log("open_recycle_bin: shell:RecycleBinFolder");
                if let Err(err) = std::process::Command::new("explorer.exe")
                    .arg(RECYCLE_BIN_SHELL_URI)
                    .spawn()
                {
                    crate::logger::log(format!("open_recycle_bin failed: {err}"));
                }
            }
            #[cfg(not(windows))]
            {
                crate::logger::log("open_recycle_bin is only supported on Windows");
            }
        });
    if let Err(err) = spawn_result {
        crate::logger::log(format!("open_recycle_bin worker start failed: {err}"));
    }
}

/// URL をデフォルトブラウザで開く。
pub fn open_url(url: &str) {
    crate::logger::log(format!("open_url: {url}"));
    if let Err(err) = crate::external_links::open_url(url) {
        crate::logger::log(format!("open_url failed: {err}"));
    }
}

/// オンラインマニュアルの URL を作る。
pub fn manual_url(page: &str, anchor: Option<&str>) -> String {
    let mut url = format!(
        "https://www.mikage.to/mimageviewer/manual/{page}?version={}",
        env!("CARGO_PKG_VERSION"),
    );
    if let Some(anchor) = anchor {
        url.push('#');
        url.push_str(anchor);
    }
    url
}

// -----------------------------------------------------------------------
// Ctrl+G 結果コンテナ: 階層パス表示
// -----------------------------------------------------------------------

/// パス文字列を `/` と `\` の両方で分割し、空要素を落としたコンポーネント列を返す。
/// ドライブ文字 (`c:`) は 1 コンポーネントとして保持する。
pub fn split_path_components(path_str: &str) -> Vec<&str> {
    path_str
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .collect()
}

/// `rect` の中央に階層パスを描画する。
///
/// フィット戦略:
/// 1. max_font→min_font を 1pt 刻みで shrink (width/height 両方を満たす font を探す)
/// 2. min_font でも溢れたら先頭コンポーネントを 1 個ずつ削り、先頭行に `…` を置く
/// 3. 末端 1 行すら入らない場合はそのまま描画してはみ出しを許容する
pub fn draw_path_hierarchy(
    painter: &egui::Painter,
    rect: egui::Rect,
    components: &[&str],
    color: egui::Color32,
    max_font: f32,
    min_font: f32,
) {
    let galley = layout_path_hierarchy(painter, components, color, rect.size(), max_font, min_font);
    let gs = galley.size();
    let pos = egui::pos2(
        rect.center().x - gs.x * 0.5,
        rect.min.y + ((rect.height() - gs.y).max(0.0)) * 0.5,
    );
    painter.galley(pos, galley, color);
}

/// `draw_path_hierarchy` のレイアウト部分だけを返す (位置決め / 描画は呼び出し側)。
/// 単体テストしやすいように分離してある。
fn layout_path_hierarchy(
    painter: &egui::Painter,
    components: &[&str],
    color: egui::Color32,
    max_size: egui::Vec2,
    max_font: f32,
    min_font: f32,
) -> std::sync::Arc<egui::Galley> {
    if components.is_empty() {
        return painter.layout_no_wrap(String::new(), egui::FontId::proportional(min_font), color);
    }
    // Phase 1: 全コンポーネントで font を max→min へ shrink。
    // 深いパスで max_font が確実に縦にはみ出す場合、高さベースで推定した上限まで
    // 一気に落として Phase 1 ループの空振りを避ける (層 6+ で効いてくる)。
    const LINE_H_RATIO: f32 = 1.3;
    let height_fit_font = (max_size.y / (components.len() as f32 * LINE_H_RATIO)).floor();
    let start_font = max_font.min(height_fit_font).max(min_font);

    let full = components.join("\n");
    let mut font = start_font;
    while font >= min_font {
        let galley = painter.layout_no_wrap(full.clone(), egui::FontId::proportional(font), color);
        if galley.size().x <= max_size.x && galley.size().y <= max_size.y {
            return galley;
        }
        font -= 1.0;
    }
    // Phase 2: min_font で先頭から削る (末端優先、先頭行は ellipsis)
    for start in 1..components.len() {
        let mut lines: Vec<&str> = Vec::with_capacity(components.len() - start + 1);
        lines.push("…");
        lines.extend_from_slice(&components[start..]);
        let galley = painter.layout_no_wrap(
            lines.join("\n"),
            egui::FontId::proportional(min_font),
            color,
        );
        if galley.size().x <= max_size.x && galley.size().y <= max_size.y {
            return galley;
        }
    }
    // Phase 3: 末端 1 行のみ (components は上で非空を確認済み)
    // 末端名自体が長い PDF/ZIP 名のとき、min_font no-wrap だとセル幅を超えて
    // 隣セル / バッジに重なるので、頭側を `…` で省略して max_size.x に収める。
    let tail = *components.last().expect("components is non-empty above");
    layout_path_tail_elided(
        painter,
        tail,
        egui::FontId::proportional(min_font),
        color,
        max_size.x,
    )
}

// -----------------------------------------------------------------------
// 1 行パス表示 (末端優先でヘッド側に … を付けて縮める)
// -----------------------------------------------------------------------

/// `text` を 1 行でレイアウトし、`max_width` を超える場合は先頭側を 1 文字ずつ削って
/// `…` プレフィクスを付ける。ファイル名 (末端) を優先して残す用途 (フルスクリーン
/// 読込中インジケータでどのファイルを読み込んでいるか見せるとき等)。
pub fn layout_path_tail_elided(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let full = painter.layout_no_wrap(text.to_string(), font.clone(), color);
    if full.size().x <= max_width {
        return full;
    }
    let chars: Vec<char> = text.chars().collect();
    // `…<tail>` が収まる drop 数を二分探索 (線形だと長いパスで layout_no_wrap が O(n) 呼ばれる)。
    let (mut lo, mut hi) = (1usize, chars.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        let candidate: String = std::iter::once('…')
            .chain(chars[mid..].iter().copied())
            .collect();
        let galley = painter.layout_no_wrap(candidate, font.clone(), color);
        if galley.size().x <= max_width {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    if lo >= chars.len() {
        return painter.layout_no_wrap("…".to_string(), font, color);
    }
    let candidate: String = std::iter::once('…')
        .chain(chars[lo..].iter().copied())
        .collect();
    painter.layout_no_wrap(candidate, font, color)
}

/// 中央に水平整列した 1 行ラベルを `rect` 内に描画する。はみ出す場合は頭側を `…` で削る。
/// 用途はフルスクリーン読込中プレースホルダ直下のファイルパス表示。
/// `text` が空なら何もしない。
///
/// - `anchor_y`: テキストベースの基準 y 座標 (このラインを top にしてラベルを置く)
/// - `h_padding`: rect 左右端から確保する水平マージン
pub fn draw_centered_elided_label(
    painter: &egui::Painter,
    rect: egui::Rect,
    text: &str,
    font_size: f32,
    color: egui::Color32,
    anchor_y: f32,
    h_padding: f32,
) {
    if text.is_empty() {
        return;
    }
    let max_w = (rect.width() - h_padding * 2.0).max(40.0);
    let galley = layout_path_tail_elided(
        painter,
        text,
        egui::FontId::proportional(font_size),
        color,
        max_w,
    );
    let gs = galley.size();
    let pos = egui::pos2(rect.center().x - gs.x * 0.5, anchor_y);
    painter.galley(pos, galley, color);
}

// -----------------------------------------------------------------------
// テスト
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_url_places_fragment_after_version_query() {
        assert_eq!(
            manual_url("settings.html", Some("ai-processing-time")),
            format!(
                "https://www.mikage.to/mimageviewer/manual/settings.html?version={}#ai-processing-time",
                env!("CARGO_PKG_VERSION")
            )
        );
        assert_eq!(
            manual_url("index.html", None),
            format!(
                "https://www.mikage.to/mimageviewer/manual/index.html?version={}",
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    #[test]
    fn format_count_basic() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(1), "1");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(1234), "1,234");
        assert_eq!(format_count(999_999), "999,999");
        assert_eq!(format_count(1_000_000), "1,000,000");
        assert_eq!(format_count(1_234_567_890), "1,234,567,890");
    }

    #[test]
    fn format_bytes_units() {
        // < 1 GB → MB
        assert_eq!(format_bytes(0), "0.0 MB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(500 * 1024 * 1024), "500.0 MB");
        // ≥ 1 GB → GB
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(
            format_bytes(2 * 1024 * 1024 * 1024 + 512 * 1024 * 1024),
            "2.50 GB"
        );
    }

    #[test]
    fn format_bytes_small_units() {
        // < 1 MB → KB
        assert_eq!(format_bytes_small(0), "0.0 KB");
        assert_eq!(format_bytes_small(1024), "1.0 KB");
        assert_eq!(format_bytes_small(512 * 1024), "512.0 KB");
        // ≥ 1 MB → MB
        assert_eq!(format_bytes_small(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes_small(2 * 1024 * 1024 + 512 * 1024), "2.50 MB");
    }

    #[test]
    fn format_details_size_modes() {
        use crate::settings::DetailsSizeDisplayMode as Mode;

        assert_eq!(format_details_size(999, Mode::Optimal), "999 B");
        assert_eq!(format_details_size(1536, Mode::Optimal), "1.5 KB");
        assert_eq!(
            format_details_size(5 * 1024 * 1024, Mode::Optimal),
            "5.0 MB"
        );
        assert_eq!(
            format_details_size(1_234_567, Mode::FixedBytes),
            "1,234,567"
        );
        assert_eq!(format_details_size(1_234_567, Mode::FixedKb), "1,205.6 KB");
        assert_eq!(format_details_size(1_234_567, Mode::FixedMb), "1.18 MB");
    }

    #[test]
    fn truncate_name_short_string_unchanged() {
        assert_eq!(truncate_name("abc", 10), "abc");
        assert_eq!(truncate_name("12345", 5), "12345"); // 等しい場合は切らない
    }

    #[test]
    fn truncate_name_long_string_gets_ellipsis() {
        // max_chars = 5 のとき、4 文字 + "…" になる
        assert_eq!(truncate_name("123456", 5), "1234…");
        assert_eq!(truncate_name("hello world", 8), "hello w…");
    }

    #[test]
    fn truncate_name_handles_multibyte() {
        // 日本語は char 単位で扱う
        assert_eq!(truncate_name("あいうえお", 5), "あいうえお");
        assert_eq!(truncate_name("あいうえおか", 5), "あいうえ…");
    }

    #[test]
    fn natural_sort_key_basic_numeric_order() {
        // 数字部分が数値として比較される
        let a = natural_sort_key("file2.jpg");
        let b = natural_sort_key("file10.jpg");
        // 辞書順だと "file10" < "file2" になるが、自然順では逆
        assert!(a < b);
    }

    #[test]
    fn natural_sort_key_treats_fullwidth_digits_as_numbers() {
        let a = natural_sort_key("file２.jpg");
        let b = natural_sort_key("file１０.jpg");
        assert!(a < b);
    }

    #[test]
    fn natural_sort_key_saturates_huge_numbers() {
        let small = natural_sort_key("file9.jpg");
        let huge = natural_sort_key("file18446744073709551616.jpg");
        assert!(small < huge);
    }

    #[test]
    fn natural_sort_key_mixed_chunks() {
        let mut names = vec![
            "img1.jpg",
            "img10.jpg",
            "img2.jpg",
            "img20.jpg",
            "img100.jpg",
        ];
        names.sort_by(|a, b| natural_sort_key(a).cmp(&natural_sort_key(b)));
        assert_eq!(
            names,
            vec![
                "img1.jpg",
                "img2.jpg",
                "img10.jpg",
                "img20.jpg",
                "img100.jpg"
            ]
        );
    }

    #[test]
    fn natural_sort_key_case_insensitive() {
        let a = natural_sort_key("FILE.jpg");
        let b = natural_sort_key("file.jpg");
        assert_eq!(a, b);
    }

    #[test]
    fn natural_sort_key_pure_text() {
        let a = natural_sort_key("apple");
        let b = natural_sort_key("banana");
        assert!(a < b);
    }

    #[test]
    fn natural_sort_key_ignores_whitespace_between_marker_and_number() {
        // `#` の後ろの空白の有無で番号比較に到達できないと
        // `# 1` が `#68` の後ろに回ってしまう問題への回帰テスト。
        let a = natural_sort_key("カードキャプターさくら# 1.mp4");
        let b = natural_sort_key("カードキャプターさくら#68.mp4");
        assert!(a < b);
    }

    #[test]
    fn natural_sort_key_ignores_symbols_in_text_chunks() {
        // 記号・空白は除去して比較されるので、区切り文字違いは数字本体で並ぶ。
        let mut names = vec!["foo#10.jpg", "foo# 2.jpg", "foo_1.jpg", "foo-3.jpg"];
        names.sort_by(|a, b| natural_sort_key(a).cmp(&natural_sort_key(b)));
        assert_eq!(
            names,
            vec!["foo_1.jpg", "foo# 2.jpg", "foo-3.jpg", "foo#10.jpg"]
        );
    }

    #[test]
    fn natural_sort_key_drops_empty_text_chunks() {
        // 記号だけで構成される区間は Text("") として残らず破棄される。
        // 結果として、ファイル先頭が記号で始まる `#1.jpg` も先頭記号がない
        // `1.jpg` と natural key が完全に一致する (最終順序は
        // SortOrder::compare の tiebreak に委ねる)。
        assert_eq!(natural_sort_key("#1.jpg"), natural_sort_key("1.jpg"));
        // 記号差だけのファイル名同士も natural key として同値になる。
        assert_eq!(
            natural_sort_key("foo-bar1.jpg"),
            natural_sort_key("foobar1.jpg"),
        );
        assert_eq!(
            natural_sort_key("foo bar1.jpg"),
            natural_sort_key("foo#bar1.jpg"),
        );
        // 数字の間に挟まる記号は数字を分離するため、`1#2.jpg` (=Num(1),Num(2))
        // と `12.jpg` (=Num(12)) は別キーになる。記号除去はあくまで
        // テキスト区間内の正規化であって、数字チャンクの結合は行わない。
        assert_ne!(natural_sort_key("1#2.jpg"), natural_sort_key("12.jpg"));
    }

    #[test]
    fn split_path_components_windows_forward_slash() {
        assert_eq!(
            split_path_components("c:/home/photo/2025-01-01"),
            vec!["c:", "home", "photo", "2025-01-01"]
        );
    }

    #[test]
    fn split_path_components_mixed_separators() {
        assert_eq!(
            split_path_components(r"c:\home/photo\2025-01-01"),
            vec!["c:", "home", "photo", "2025-01-01"]
        );
    }

    #[test]
    fn split_path_components_strips_empty_segments() {
        // 末尾スラッシュ / 連続スラッシュ由来の空要素は落とす
        assert_eq!(split_path_components("c:/home/"), vec!["c:", "home"]);
        assert_eq!(
            split_path_components("c://home///photo"),
            vec!["c:", "home", "photo"]
        );
    }

    #[test]
    fn split_path_components_empty() {
        assert_eq!(split_path_components(""), Vec::<&str>::new());
        assert_eq!(split_path_components("/"), Vec::<&str>::new());
    }

    #[test]
    fn split_path_components_drive_only() {
        assert_eq!(split_path_components("c:"), vec!["c:"]);
        assert_eq!(split_path_components("c:/"), vec!["c:"]);
    }

    // ── adjacent_navigable_idx ──
    fn img_items(n: usize) -> Vec<GridItem> {
        (0..n)
            .map(|i| GridItem::Image(std::path::PathBuf::from(format!("/a/{}.jpg", i))))
            .collect()
    }

    #[test]
    fn adjacent_navigable_idx_current_in_list_moves_normally() {
        let items = img_items(5);
        let vi = vec![0, 1, 2, 3, 4];
        assert_eq!(adjacent_navigable_idx(&items, &vi, 2, 1), Some(3));
        assert_eq!(adjacent_navigable_idx(&items, &vi, 2, -1), Some(1));
        // 境界: 末尾から +1 / 先頭から -1 は None
        assert_eq!(adjacent_navigable_idx(&items, &vi, 4, 1), None);
        assert_eq!(adjacent_navigable_idx(&items, &vi, 0, -1), None);
    }

    #[test]
    fn adjacent_navigable_idx_respects_display_order_when_reordered() {
        let items = img_items(5);
        let order = vec![4, 2, 0, 3, 1];
        assert_eq!(adjacent_navigable_idx(&items, &order, 0, 1), Some(3));
        assert_eq!(adjacent_navigable_idx(&items, &order, 0, -1), Some(2));
        assert_eq!(adjacent_navigable_idx(&items, &order, 4, -1), None);
        assert_eq!(adjacent_navigable_idx(&items, &order, 1, 1), None);
    }

    /// current が visible_indices から外れている (フィルタで除外された) ときは
    /// items 順で方向側の最寄り visible idx を返す。
    #[test]
    fn adjacent_navigable_idx_current_not_in_list_finds_direction_neighbor() {
        let items = img_items(5);
        // idx=2 だけフィルタで除外された状態
        let vi = vec![0, 1, 3, 4];
        // current=2 で +1 → 2 より大きい最小 = 3
        assert_eq!(adjacent_navigable_idx(&items, &vi, 2, 1), Some(3));
        // current=2 で -1 → 2 より小さい最大 = 1
        assert_eq!(adjacent_navigable_idx(&items, &vi, 2, -1), Some(1));
    }

    #[test]
    fn adjacent_navigable_idx_current_not_in_list_boundary_none() {
        let items = img_items(5);
        let vi = vec![1, 2, 3];
        // current=4 (末尾より後) で +1 → 無し
        assert_eq!(adjacent_navigable_idx(&items, &vi, 4, 1), None);
        // current=0 (先頭より前) で -1 → 無し
        assert_eq!(adjacent_navigable_idx(&items, &vi, 0, -1), None);
    }

    #[test]
    fn adjacent_navigable_idx_empty_list_returns_none() {
        let items = img_items(3);
        let vi: Vec<usize> = Vec::new();
        assert_eq!(adjacent_navigable_idx(&items, &vi, 1, 1), None);
        assert_eq!(adjacent_navigable_idx(&items, &vi, 1, -1), None);
    }

    #[test]
    fn fixed_jump_page_idx_clamps_to_edges() {
        let items = img_items(8);
        let vi = vec![0, 1, 2, 3, 4, 5, 6, 7];
        assert_eq!(fixed_jump_page_idx(&items, &vi, 2, 3, true), Some(5));
        assert_eq!(fixed_jump_page_idx(&items, &vi, 2, 10, true), Some(7));
        assert_eq!(fixed_jump_page_idx(&items, &vi, 2, 10, false), Some(0));
        assert_eq!(fixed_jump_page_idx(&items, &vi, 7, 3, true), None);
        assert_eq!(fixed_jump_page_idx(&items, &vi, 0, 3, false), None);
    }

    #[test]
    fn fixed_jump_page_idx_respects_display_order_and_skips_non_pages() {
        let items = vec![
            GridItem::Folder(std::path::PathBuf::from("/a/folder")),
            GridItem::Image(std::path::PathBuf::from("/a/one.jpg")),
            GridItem::Video(std::path::PathBuf::from("/a/two.mp4")),
            GridItem::ZipSeparator {
                dir_display: "chapter".into(),
            },
            GridItem::Image(std::path::PathBuf::from("/a/three.jpg")),
            GridItem::PdfPage {
                pdf_path: std::path::PathBuf::from("/a/book.pdf"),
                page_num: 0,
                content_type: None,
            },
        ];
        let order = vec![5, 0, 2, 3, 1, 4];
        assert_eq!(fixed_jump_page_idx(&items, &order, 5, 2, true), Some(4));
        assert_eq!(fixed_jump_page_idx(&items, &order, 4, 3, false), Some(5));
    }

    #[test]
    fn percent_jump_page_step_rounds_up_and_has_minimum() {
        assert_eq!(percent_jump_page_step(0, 10), 1);
        assert_eq!(percent_jump_page_step(1, 10), 1);
        assert_eq!(percent_jump_page_step(9, 10), 1);
        assert_eq!(percent_jump_page_step(10, 10), 1);
        assert_eq!(percent_jump_page_step(11, 10), 2);
        assert_eq!(percent_jump_page_step(200, 10), 20);
        assert_eq!(percent_jump_page_step(200, 100), 200);
    }

    #[test]
    fn large_jump_page_idx_uses_percent_or_fixed_mode() {
        let items = img_items(200);
        let vi: Vec<usize> = (0..200).collect();
        assert_eq!(
            large_jump_page_idx(
                &items,
                &vi,
                50,
                crate::settings::FullscreenJumpMode::Percent,
                10,
                3,
                1,
                true,
            ),
            Some(70)
        );
        assert_eq!(
            large_jump_page_idx(
                &items,
                &vi,
                50,
                crate::settings::FullscreenJumpMode::FixedPages,
                10,
                3,
                1,
                true,
            ),
            Some(53)
        );
    }

    #[test]
    fn large_jump_page_idx_honors_min_step_for_spread_views() {
        let items = img_items(9);
        let vi: Vec<usize> = (0..9).collect();
        assert_eq!(
            large_jump_page_idx(
                &items,
                &vi,
                4,
                crate::settings::FullscreenJumpMode::Percent,
                10,
                1,
                2,
                true,
            ),
            Some(6)
        );
        assert_eq!(
            large_jump_page_idx(
                &items,
                &vi,
                4,
                crate::settings::FullscreenJumpMode::FixedPages,
                10,
                1,
                2,
                false,
            ),
            Some(2)
        );
    }

    /// 音声 (Audio) は「映像なし動画」として前/次項目ナビの対象に含める:
    /// image(0) - audio(1) - image(2) で 0 から +1 すると audio(1) に止まれる。
    /// スライドショー送りは音声を飛ばす (画像のみ対象)。
    #[test]
    fn adjacent_navigable_idx_includes_audio_slideshow_skips_it() {
        let items = vec![
            GridItem::Image(std::path::PathBuf::from("/a/0.jpg")),
            GridItem::Audio(std::path::PathBuf::from("/a/1.mp3")),
            GridItem::Image(std::path::PathBuf::from("/a/2.jpg")),
        ];
        let vi = vec![0, 1, 2];
        // 通常ナビは audio(1) に止まれる (前後どちらからも)。
        assert_eq!(adjacent_navigable_idx(&items, &vi, 0, 1), Some(1));
        assert_eq!(adjacent_navigable_idx(&items, &vi, 2, -1), Some(1));
        // audio 自身からの前後移動も可能。
        assert_eq!(adjacent_navigable_idx(&items, &vi, 1, 1), Some(2));
        assert_eq!(adjacent_navigable_idx(&items, &vi, 1, -1), Some(0));
        // スライドショー送りは audio(1) を飛ばして image(2)。
        assert_eq!(adjacent_slideshow_idx(&items, &vi, 0, 1), Some(2));
    }

    /// スライドショー送りは Video を飛ばす: image(0) - video(1) - image(2) で
    /// 0 から +1 すると video(1) を飛ばして image(2)。
    #[test]
    fn adjacent_slideshow_idx_skips_video() {
        let items = vec![
            GridItem::Image(std::path::PathBuf::from("/a/0.jpg")),
            GridItem::Video(std::path::PathBuf::from("/a/1.mp4")),
            GridItem::Image(std::path::PathBuf::from("/a/2.jpg")),
        ];
        let vi = vec![0, 1, 2];
        assert_eq!(adjacent_slideshow_idx(&items, &vi, 0, 1), Some(2));
        assert_eq!(adjacent_slideshow_idx(&items, &vi, 2, -1), Some(0));
        // 通常の隣接探索は video(1) に止まれる (対比)。
        assert_eq!(adjacent_navigable_idx(&items, &vi, 0, 1), Some(1));
    }

    #[test]
    fn adjacent_slideshow_idx_respects_display_order_when_reordered() {
        let items = vec![
            GridItem::Image(std::path::PathBuf::from("/a/0.jpg")),
            GridItem::Video(std::path::PathBuf::from("/a/1.mp4")),
            GridItem::Image(std::path::PathBuf::from("/a/2.jpg")),
            GridItem::Image(std::path::PathBuf::from("/a/3.jpg")),
        ];
        let order = vec![3, 1, 0, 2];
        assert_eq!(adjacent_slideshow_idx(&items, &order, 3, 1), Some(0));
        assert_eq!(adjacent_slideshow_idx(&items, &order, 0, -1), Some(3));
    }

    /// スライドショー送りは ZipSeparator は残す (章タイトルを同間隔で表示)。
    #[test]
    fn adjacent_slideshow_idx_keeps_separator() {
        let items = vec![
            GridItem::Image(std::path::PathBuf::from("/a/0.jpg")),
            GridItem::ZipSeparator {
                dir_display: "chapter".to_string(),
            },
            GridItem::Image(std::path::PathBuf::from("/a/2.jpg")),
        ];
        let vi = vec![0, 1, 2];
        assert_eq!(adjacent_slideshow_idx(&items, &vi, 0, 1), Some(1));
        assert_eq!(adjacent_slideshow_idx(&items, &vi, 1, 1), Some(2));
    }

    /// 末尾が動画でも境界は None (折り返しは呼び出し側で行う)。
    #[test]
    fn adjacent_slideshow_idx_boundary_none_when_only_video_after() {
        let items = vec![
            GridItem::Image(std::path::PathBuf::from("/a/0.jpg")),
            GridItem::Video(std::path::PathBuf::from("/a/1.mp4")),
        ];
        let vi = vec![0, 1];
        assert_eq!(adjacent_slideshow_idx(&items, &vi, 0, 1), None);
    }

    /// 折り返し target は separator を飛ばして先頭の実画像。
    #[test]
    fn first_slideshow_still_idx_skips_separator_and_video() {
        let items = vec![
            GridItem::ZipSeparator {
                dir_display: "chapter".to_string(),
            },
            GridItem::Video(std::path::PathBuf::from("/a/1.mp4")),
            GridItem::Image(std::path::PathBuf::from("/a/2.jpg")),
        ];
        let vi = vec![0, 1, 2];
        assert_eq!(first_slideshow_still_idx(&items, &vi), Some(2));
    }
}
