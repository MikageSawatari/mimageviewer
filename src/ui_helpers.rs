//! UI 描画と整形に関する小さなヘルパー関数群。
//!
//! どの関数も `&mut App` には依存せず、純粋な引数だけで動作する。
//! - 整形系: `format_bytes`, `format_count`, `truncate_name`
//! - ソート系: `natural_sort_key`, `NaturalChunk`
//! - 描画系: `draw_play_icon`, `draw_histogram`, `draw_format_rows`
//! - ナビ系: `adjacent_navigable_idx`
//! - 外部連携: `open_external_player`

use std::path::Path;

use crate::grid_item::GridItem;

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

/// 統計ダイアログのヒストグラムを ASCII バー + 件数で描画する。
/// `label_fn` がバケットインデックスから左端ラベルを返す。
pub fn draw_histogram(
    ui: &mut egui::Ui,
    hist: &[u64],
    label_fn: impl Fn(usize) -> String,
) {
    const MAX_BAR_WIDTH: usize = 32;
    let max_count = hist.iter().copied().max().unwrap_or(0);
    if max_count == 0 {
        ui.label("  (データなし)");
        return;
    }

    // モノスペースフォントで整列
    let font = egui::FontId::monospace(12.0);
    for (bucket, &count) in hist.iter().enumerate() {
        // 末尾の 0 連続をトリミングしない (分布の全体像が見えるように)
        let label = label_fn(bucket);
        let bar_len = ((count as f64 / max_count as f64) * MAX_BAR_WIDTH as f64) as usize;
        let bar: String = std::iter::repeat('=').take(bar_len).collect();
        let count_str = format_count(count);
        let line = format!(
            "  {label}  {bar:<MAX_BAR_WIDTH$}  {count_str:>8}",
            MAX_BAR_WIDTH = MAX_BAR_WIDTH,
        );
        ui.label(egui::RichText::new(line).font(font.clone()));
    }
}

/// 統計ダイアログのフォーマット別件数を ASCII バー + 件数で描画する。
pub fn draw_format_rows(ui: &mut egui::Ui, rows: &[(&str, u64)]) {
    const MAX_BAR_WIDTH: usize = 32;
    let max_count = rows.iter().map(|(_, c)| *c).max().unwrap_or(0);
    if max_count == 0 {
        ui.label("  (データなし)");
        return;
    }
    let font = egui::FontId::monospace(12.0);
    for (label, count) in rows {
        let bar_len = ((*count as f64 / max_count as f64) * MAX_BAR_WIDTH as f64) as usize;
        let bar: String = std::iter::repeat('=').take(bar_len).collect();
        let count_str = format_count(*count);
        let line = format!(
            "  {label}  {bar:<MAX_BAR_WIDTH$}  {count_str:>8}",
            MAX_BAR_WIDTH = MAX_BAR_WIDTH,
        );
        ui.label(egui::RichText::new(line).font(font.clone()));
    }
}

// -----------------------------------------------------------------------
// アイテムナビゲーション
// -----------------------------------------------------------------------

/// items の中で current から delta 分（±1）移動した「表示可能」アイテム
/// (画像 + 動画 + ZIP 画像 + ZIP セパレータ) の item index を返す。
/// 境界では None を返す（ラップアラウンドなし）。
pub fn adjacent_navigable_idx(items: &[GridItem], current: usize, delta: i32) -> Option<usize> {
    // タスク 3: ZipImage と ZipSeparator もフルスクリーンで切り替え可能にする
    // (セパレータは "章タイトル" 画面として表示される)
    let nav_indices: Vec<usize> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            matches!(
                item,
                GridItem::Image(_)
                    | GridItem::Video(_)
                    | GridItem::ZipImage { .. }
                    | GridItem::ZipSeparator { .. }
            )
            .then_some(i)
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

// -----------------------------------------------------------------------
// 外部連携
// -----------------------------------------------------------------------

/// パスに関連付けられたデフォルトアプリケーション（外部プレイヤー）で開く。
pub fn open_external_player(path: &Path) {
    let path_str = path.to_string_lossy().into_owned();
    crate::logger::log(format!("open_external_player: {path_str}"));
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", "", &path_str])
        .spawn();
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
