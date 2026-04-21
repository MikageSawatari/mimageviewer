//! タグ付与/削除操作のファサード (docs/tag-feature.md §5)。
//!
//! メニュー・ツールバーからの「タグ X をトグル」「すべてクリア」操作の
//! エントリーポイント。実際の XMP 書き込みは Phase C で `xmp_writer` +
//! `tag_write_worker` に委譲する。
//!
//! v1.0 (Phase B) 時点では UI だけ先行実装し、実際の書き込みは未実装 (stub)。
//! 有効化されるとステータスバーに「書き込み機能は未実装」メッセージが表示される。

use std::path::PathBuf;

use crate::app::App;
use crate::grid_item::GridItem;

impl App {
    /// 現在「タグ対象」にできる選択中ファイルのパスを返す。
    ///
    /// タグ書き込み対象は通常画像 (JPEG/PNG/WebP) のみ。ZIP 内画像 / PDF ページ /
    /// フォルダ / HEIC / RAW / 動画はすべて除外。
    pub(crate) fn tag_target_paths(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        // 選択中のグリッドアイテムを列挙
        let selected = self.selected_items_paths_for_tag();
        for p in selected {
            if is_tag_writable_path(&p) {
                out.push(p);
            }
        }
        out
    }

    /// `tag_target_paths` の件数だけを返す軽量版 (メニュー表示用)。
    pub(crate) fn tag_target_path_count(&self) -> usize {
        self.tag_target_paths().len()
    }

    /// メニュー/ツールバーから「タグ `name` をトグル」が押されたときのハンドラ。
    /// Phase C で実装予定 (XMP 書き込み worker 経由)。
    pub(crate) fn request_tag_toggle_for_selection(&mut self, name: &str) {
        let paths = self.tag_target_paths();
        if paths.is_empty() {
            return;
        }
        // Phase C 未実装: トースト通知のみ
        self.show_feedback_toast(format!(
            "タグ付与は未実装です (対象 {} 件、#{})",
            paths.len(),
            name
        ));
    }

    /// 「すべてクリア」が押されたときのハンドラ (Phase C で実装)。
    pub(crate) fn request_tag_clear_for_selection(&mut self) {
        let paths = self.tag_target_paths();
        if paths.is_empty() {
            return;
        }
        self.show_feedback_toast(format!(
            "タグクリアは未実装です (対象 {} 件)",
            paths.len()
        ));
    }

    /// 現在選択中のグリッドアイテムから、通常ファイルパスだけ取り出す。
    /// 現状は単一選択モデル (`self.selected: Option<usize>`) のみ対応。
    /// 選択が空でフルスクリーン中なら現在表示のパスを返す。
    fn selected_items_paths_for_tag(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        if let Some(idx) = self.selected {
            if let Some(GridItem::Image(p)) = self.items.get(idx) {
                out.push(p.clone());
            }
        }
        if out.is_empty() {
            if let Some(idx) = self.fullscreen_idx {
                if let Some(GridItem::Image(p)) = self.items.get(idx) {
                    out.push(p.clone());
                }
            }
        }
        out
    }
}

/// パスの拡張子が Phase C タグ書き込み対応形式か。
/// 現状 JPEG / PNG / WebP のみ。
fn is_tag_writable_path(path: &std::path::Path) -> bool {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "jfif" | "png" | "webp"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_jpeg_png_webp() {
        assert!(is_tag_writable_path(std::path::Path::new("a.jpg")));
        assert!(is_tag_writable_path(std::path::Path::new("A.JPG")));
        assert!(is_tag_writable_path(std::path::Path::new("b.jpeg")));
        assert!(is_tag_writable_path(std::path::Path::new("c.png")));
        assert!(is_tag_writable_path(std::path::Path::new("d.webp")));
    }

    #[test]
    fn rejects_non_writable() {
        assert!(!is_tag_writable_path(std::path::Path::new("a.heic")));
        assert!(!is_tag_writable_path(std::path::Path::new("b.tiff")));
        assert!(!is_tag_writable_path(std::path::Path::new("c.cr2")));
        assert!(!is_tag_writable_path(std::path::Path::new("d.mp4")));
        assert!(!is_tag_writable_path(std::path::Path::new("no_ext")));
    }
}
