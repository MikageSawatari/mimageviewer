//! タグ付与/削除操作のファサード (docs/tag-feature.md §5)。
//!
//! メニュー・ツールバーからの「タグ X をトグル」「すべてクリア」操作の
//! エントリーポイント。XMP 書き込みは `xmp_writer` + `tag_write_worker` に委譲する。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::app::App;
use crate::grid_item::GridItem;
use crate::tag_write_worker::{TagWriteHandle, TagWriteJob};
use crate::xmp_writer::TagOp;

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
    /// 選択中ファイルのタグ状態から Add/Remove を決定し、worker に投入する。
    pub(crate) fn request_tag_toggle_for_selection(&mut self, name: &str) {
        let paths = self.tag_target_paths();
        if paths.is_empty() {
            return;
        }
        let tag_with_hash = format!("#{}", name);
        let current_tags = self.read_current_tags_for_paths(&paths);
        let op = crate::tag_write_worker::decide_toggle_op(
            &tag_with_hash,
            &paths,
            &current_tags,
        );
        let verb = matches!(&op, TagOp::Add(_)).then_some("付与").unwrap_or("削除");
        self.ensure_tag_write_handle();
        let (fav_map_id, fav_map_root) = self.resolve_favorite_map_for_paths(&paths);
        if let Some(h) = self.tag_write_handle.as_ref() {
            for p in &paths {
                let job = TagWriteJob {
                    path: p.clone(),
                    op: op.clone(),
                    favorite_id: fav_map_id.get(p).copied(),
                    favorite_root: fav_map_root.get(p).cloned(),
                };
                h.submit(job);
            }
        }
        self.show_feedback_toast(format!("{}件にタグ {} ({})", paths.len(), tag_with_hash, verb));
    }

    /// 「すべてクリア」が押されたときのハンドラ。
    /// 選択中ファイルから `#` で始まるタグ (mIV 付与タグ) を全削除する。
    pub(crate) fn request_tag_clear_for_selection(&mut self) {
        let paths = self.tag_target_paths();
        if paths.is_empty() {
            return;
        }
        self.ensure_tag_write_handle();
        let (fav_map_id, fav_map_root) = self.resolve_favorite_map_for_paths(&paths);
        if let Some(h) = self.tag_write_handle.as_ref() {
            for p in &paths {
                let job = TagWriteJob {
                    path: p.clone(),
                    op: TagOp::ClearMiv,
                    favorite_id: fav_map_id.get(p).copied(),
                    favorite_root: fav_map_root.get(p).cloned(),
                };
                h.submit(job);
            }
        }
        self.show_feedback_toast(format!("{}件から mIV タグをクリア", paths.len()));
    }

    /// 初回要求時に worker を起動する。
    fn ensure_tag_write_handle(&mut self) {
        if self.tag_write_handle.is_some() {
            return;
        }
        let Some(mgr) = self.indexer_manager.as_ref() else {
            return;
        };
        let meta = mgr.clone_fts_meta();
        let fts = mgr.clone_fts_index();
        self.tag_write_handle = Some(TagWriteHandle::spawn(meta, fts));
    }

    /// 各パスのタグ列を `xmp_reader::read_dc_subject` で読み出してスペース区切りに
    /// 組み立てる。トグル判定に使うので高速である必要はないが、I/O は発生する。
    fn read_current_tags_for_paths(
        &self,
        paths: &[PathBuf],
    ) -> HashMap<PathBuf, String> {
        let mut map = HashMap::new();
        for p in paths {
            let tags = crate::xmp_reader::read_dc_subject(p);
            map.insert(p.clone(), tags.join(" "));
        }
        map
    }

    /// 各パスが所属するお気に入りの (id, root) を解決する (検索インデックス更新用)。
    /// 所属が分からないものは map に入れない (worker 側で None として扱う)。
    fn resolve_favorite_map_for_paths(
        &self,
        paths: &[PathBuf],
    ) -> (HashMap<PathBuf, uuid::Uuid>, HashMap<PathBuf, PathBuf>) {
        let mut id_map = HashMap::new();
        let mut root_map = HashMap::new();
        for p in paths {
            if let Some(fav) = find_favorite_for_path(&self.settings.favorites, p) {
                id_map.insert(p.clone(), fav.0);
                root_map.insert(p.clone(), fav.1);
            }
        }
        (id_map, root_map)
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

impl App {
    /// 毎フレーム呼ぶ: tag_write_worker の結果を取り出してエラー件数をトーストする。
    pub(crate) fn poll_tag_write_results(&mut self) {
        let mut errors: Vec<(PathBuf, String)> = Vec::new();
        let mut done_count: usize = 0;
        let mut just_completed = false;
        if let Some(h) = self.tag_write_handle.as_ref() {
            while let Some(res) = h.try_recv_result() {
                done_count += 1;
                if let Err(e) = res.result {
                    errors.push((res.path, e));
                }
            }
            // 完了: busy → idle に変わった瞬間を検出して完了トースト
            if !h.is_busy() && h.total.load(std::sync::atomic::Ordering::Relaxed) > 0 {
                just_completed = true;
            }
        }
        if !errors.is_empty() {
            // 最大 3 件まで表示
            let preview = errors
                .iter()
                .take(3)
                .map(|(p, e)| {
                    format!(
                        "{}: {}",
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("?"),
                        e
                    )
                })
                .collect::<Vec<_>>()
                .join(" / ");
            self.show_feedback_toast(format!(
                "タグ書き込み失敗 {} 件: {}",
                errors.len(),
                preview
            ));
        } else if done_count > 0 && just_completed {
            self.show_feedback_toast(format!("タグ書き込み完了"));
        }
        if just_completed {
            if let Some(h) = self.tag_write_handle.as_ref() {
                h.reset_counters_if_idle();
            }
        }
    }
}

/// 指定 path を含むお気に入りを探す (パスが子孫の場合も一致扱い)。
/// 複数一致する場合は最初の one を返す。
fn find_favorite_for_path(
    favorites: &[crate::settings::FavoriteEntry],
    path: &Path,
) -> Option<(uuid::Uuid, PathBuf)> {
    for fav in favorites {
        if path.starts_with(&fav.path) {
            return Some((fav.id, fav.path.clone()));
        }
    }
    None
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
