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
pub(crate) const ERROR_TEXT_COLOR: eframe::egui::Color32 = eframe::egui::Color32::from_rgb(220, 60, 60);
/// エラー表示の標準フォントサイズ。
#[allow(dead_code)]
pub(crate) const ERROR_TEXT_SIZE: f32 = 13.0;

/// 進捗バーのラベル色（グリッド/フルスクリーン共通）。
///
/// `#[allow(dead_code)]` は lib クレート側で使用者が見えないため。実体は
/// バイナリクレート側の `ui_main` / `ui_fullscreen` から参照される。
#[allow(dead_code)]
pub(crate) const PROGRESS_LABEL_COLOR: eframe::egui::Color32 = eframe::egui::Color32::from_rgb(235, 240, 250);
/// 進捗バーの背景色（ポップアップ Frame の fill）。
#[allow(dead_code)]
pub(crate) const PROGRESS_BG_COLOR: eframe::egui::Color32 = eframe::egui::Color32::from_rgba_premultiplied(20, 25, 35, 230);
/// 通常の先読み進捗バーの塗色（濃い青）。
#[allow(dead_code)]
pub(crate) const PROGRESS_NORMAL_COLOR: eframe::egui::Color32 = eframe::egui::Color32::from_rgb(60, 130, 220);
/// 高画質化 / AI 先読み進捗バーの塗色（薄い青）。
#[allow(dead_code)]
pub(crate) const PROGRESS_UPGRADE_COLOR: eframe::egui::Color32 = eframe::egui::Color32::from_rgb(100, 170, 240);

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
                let bar_len =
                    ((count as f64 / max_count as f64) * MAX_BAR_WIDTH as f64) as usize;
                let bar: String = "\u{2588}".repeat(bar_len);
                ui.label(
                    egui::RichText::new(format!("{bar:<MAX_BAR_WIDTH$}", MAX_BAR_WIDTH = MAX_BAR_WIDTH))
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
                let bar_len =
                    ((*count as f64 / max_count as f64) * MAX_BAR_WIDTH as f64) as usize;
                let bar: String = "\u{2588}".repeat(bar_len);
                ui.label(
                    egui::RichText::new(format!("{bar:<MAX_BAR_WIDTH$}", MAX_BAR_WIDTH = MAX_BAR_WIDTH))
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
    let pos = nav_indices.iter().position(|&i| i == current)?;
    let new_pos = (pos as i32 + delta).clamp(0, nav_indices.len() as i32 - 1) as usize;
    if new_pos == pos {
        None
    } else {
        Some(nav_indices[new_pos])
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
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024 + 512 * 1024 * 1024), "2.50 GB");
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
        let mut names = vec!["img1.jpg", "img10.jpg", "img2.jpg", "img20.jpg", "img100.jpg"];
        names.sort_by(|a, b| natural_sort_key(a).cmp(&natural_sort_key(b)));
        assert_eq!(
            names,
            vec!["img1.jpg", "img2.jpg", "img10.jpg", "img20.jpg", "img100.jpg"]
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
}
