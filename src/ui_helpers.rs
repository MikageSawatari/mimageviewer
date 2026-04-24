//! UI 描画と整形に関する小さなヘルパー関数群。
//!
//! どの関数も `&mut App` には依存せず、純粋な引数だけで動作する。
//! - 整形系: `format_bytes`, `format_count`, `truncate_name`
//! - ソート系: `natural_sort_key`, `NaturalChunk`
//! - 描画系: `draw_play_icon`, `draw_zip_badge`, `draw_pdf_badge`, `draw_histogram`, `draw_format_rows`
//! - ナビ系: `adjacent_navigable_idx`
//! - 外部連携: `open_external_player`

use std::path::Path;

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

// -----------------------------------------------------------------------
// F1〜F6 のレーティングキー
// -----------------------------------------------------------------------

/// F1〜F5 / F6 をレーティング値 (1〜5 / 0=解除) に consume する共通処理。
/// グリッドとフルスクリーン、ページ★ (NONE) とコンテナ★ (SHIFT) の 4 箇所で共有。
///
/// egui の `matches_logically` は `NONE` の key を `Shift+` 入りイベントでも拾ってしまう
/// ので、呼び出し側は必ず SHIFT 版を NONE 版より先に呼ぶこと。
pub fn consume_rating_fkey(
    i: &mut eframe::egui::InputState,
    mods: eframe::egui::Modifiers,
) -> Option<u8> {
    use eframe::egui::Key;
    for (key, stars) in [
        (Key::F1, 1u8),
        (Key::F2, 2),
        (Key::F3, 3),
        (Key::F4, 4),
        (Key::F5, 5),
        (Key::F6, 0),
    ] {
        if i.consume_key(mods, key) {
            return Some(stars);
        }
    }
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

/// 小さいバイト数 (サムネイル単体) を KB / MB の文字列にフォーマット。
pub fn format_bytes_small(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
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

// -----------------------------------------------------------------------
// 自然順ソート
// -----------------------------------------------------------------------

/// 自然順ソート用のキーを返す。
/// ファイル名を「テキスト部分」と「数字部分」に分割し、
/// 数字部分は数値として比較するので 1 < 2 < 9 < 10 < 11 となる。
pub fn natural_sort_key(name: &str) -> Vec<NaturalChunk> {
    let name_lower = name.to_lowercase();
    let mut chunks = Vec::new();
    let mut chars = name_lower.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut num_str = String::new();
            while chars.peek().map(|ch| ch.is_ascii_digit()).unwrap_or(false) {
                num_str.push(chars.next().unwrap());
            }
            let n: u64 = num_str.parse().unwrap_or(0);
            chunks.push(NaturalChunk::Num(n));
        } else {
            let mut text = String::new();
            while chars.peek().map(|ch| !ch.is_ascii_digit()).unwrap_or(false) {
                text.push(chars.next().unwrap());
            }
            chunks.push(NaturalChunk::Text(text));
        }
    }
    chunks
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
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

/// 変換対象アーカイブ (7z / LZH) のサムネイルに表示するバッジ（左下、橙系）。
/// `label` は "7z" / "LZH" など形式表示。
pub fn draw_archive_badge(painter: &egui::Painter, cell_rect: egui::Rect, label: &str) {
    draw_file_badge(
        painter,
        cell_rect,
        label,
        egui::Color32::from_rgba_unmultiplied(200, 110, 20, 200),
    );
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
        if badge_w <= max_badge_w || label.len() <= 2 {
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
        // 文字を減らしてリトライ
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
        .num_columns(if avg_times.is_some() { 4 } else { 3 })
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
        .num_columns(4)
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
/// `visible_indices` (フィルタ適用済み) の中からナビゲーション可能な
/// 前後のアイテムインデックスを返す。
pub fn adjacent_navigable_idx(
    items: &[GridItem],
    visible_indices: &[usize],
    current: usize,
    delta: i32,
) -> Option<usize> {
    // visible_indices の中でナビゲーション可能なもの (画像・動画・セパレータ)
    let nav_indices: Vec<usize> = visible_indices
        .iter()
        .copied()
        .filter(|&i| {
            matches!(
                items.get(i),
                Some(GridItem::Image(_))
                    | Some(GridItem::Video(_))
                    | Some(GridItem::ZipImage { .. })
                    | Some(GridItem::ZipSeparator { .. })
                    | Some(GridItem::PdfPage { .. })
            )
        })
        .collect();
    if nav_indices.is_empty() {
        return None;
    }
    // current がフィルタで外されている (例: フルスクリーンで F1-F6 により
    // レーティング変更後) 場合は、items 順で方向側の最寄りを返す。
    // nav_indices は visible_indices (昇順) の部分列なのでこちらも昇順。
    // `partition_point` で insert 位置を求めれば prev/next を O(log n) で取れる。
    let insert_pos = nav_indices.partition_point(|&i| i < current);
    let current_in_list = nav_indices.get(insert_pos).is_some_and(|&i| i == current);
    if current_in_list {
        let pos = insert_pos;
        let new_pos =
            (pos as i32 + delta).clamp(0, nav_indices.len() as i32 - 1) as usize;
        if new_pos == pos {
            None
        } else {
            Some(nav_indices[new_pos])
        }
    } else if delta > 0 {
        nav_indices.get(insert_pos).copied()
    } else if delta < 0 {
        insert_pos.checked_sub(1).map(|p| nav_indices[p])
    } else {
        None
    }
}

/// `visible_indices` の中の「ナビゲーション可能」なアイテム列から、
/// 末尾 (`last=true`) または先頭 (`last=false`) の item index を返す。
/// `adjacent_navigable_idx` と同じフィルタを適用する。
pub fn boundary_navigable_idx(
    items: &[GridItem],
    visible_indices: &[usize],
    last: bool,
) -> Option<usize> {
    let mut iter = visible_indices.iter().copied().filter(|&i| {
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

/// パスに関連付けられたデフォルトアプリケーション（外部プレイヤー）で開く。
pub fn open_external_player(path: &Path) {
    let path_str = path.to_string_lossy().into_owned();
    crate::logger::log(format!("open_external_player: {path_str}"));
    // ShellExecute 相当: cmd.exe のコンソールウィンドウが一瞬見える問題を回避するため
    // CREATE_NO_WINDOW フラグを付与する
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/c", "start", "", &path_str]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let _ = cmd.spawn();
}

/// URL をデフォルトブラウザで開く。
pub fn open_url(url: &str) {
    crate::logger::log(format!("open_url: {url}"));
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/c", "start", "", url]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let _ = cmd.spawn();
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
        return painter.layout_no_wrap(
            String::new(),
            egui::FontId::proportional(min_font),
            color,
        );
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
        let galley =
            painter.layout_no_wrap(full.clone(), egui::FontId::proportional(font), color);
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
        let candidate: String = std::iter::once('…').chain(chars[mid..].iter().copied()).collect();
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
    let candidate: String = std::iter::once('…').chain(chars[lo..].iter().copied()).collect();
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
}
