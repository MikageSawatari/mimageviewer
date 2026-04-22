//! タグ付与/削除操作のファサード (docs/tag-feature.md §5)。
//!
//! メニュー・ツールバー・メタデータパネルからのタグ操作のエントリーポイント。
//! XMP 読み書きはすべて `tag_write_worker` に委譲し、UI スレッドは同期 I/O を
//! 一切行わない。

use std::path::{Path, PathBuf};

use crate::app::App;
use crate::grid_item::GridItem;
use crate::tag_write_worker::{TagJobKind, TagWriteHandle, TagWriteJob};

impl App {
    pub(crate) fn tag_target_paths(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        if let Some(idx) = self.selected {
            if let Some(GridItem::Image(p)) = self.items.get(idx) {
                if crate::xmp_writer::is_writable_format(p) {
                    out.push(p.clone());
                }
            }
        }
        if out.is_empty() {
            if let Some(idx) = self.fullscreen_idx {
                if let Some(GridItem::Image(p)) = self.items.get(idx) {
                    if crate::xmp_writer::is_writable_format(p) {
                        out.push(p.clone());
                    }
                }
            }
        }
        out
    }

    pub(crate) fn tag_target_path_count(&self) -> usize {
        self.tag_target_paths().len()
    }

    pub(crate) fn request_tag_toggle_for_selection(&mut self, name: &str) {
        let paths = self.tag_target_paths();
        if paths.is_empty() {
            return;
        }
        self.ensure_tag_write_handle();
        // Codex P2 #3: indexer_manager が None でタグ書き込みが無効化されている場合は、
        // ジョブを投入したフリだけして黙って落とすのではなくエラートーストを出す。
        let Some(h) = self.tag_write_handle.as_ref() else {
            self.show_feedback_toast(
                "タグ書き込みが初期化されていません (検索インデックスの起動失敗が原因)".to_string(),
            );
            return;
        };
        let favs = self.settings.favorites.clone();
        for p in &paths {
            h.submit(TagWriteJob {
                path: p.clone(),
                kind: TagJobKind::Toggle(name.to_string()),
                favorite_id: find_favorite_id(&favs, p),
            });
        }
        self.show_feedback_toast(format!("{} 件にタグ #{} をトグル", paths.len(), name));
    }

    pub(crate) fn request_tag_clear_for_selection(&mut self) {
        let paths = self.tag_target_paths();
        if paths.is_empty() {
            return;
        }
        self.ensure_tag_write_handle();
        let Some(h) = self.tag_write_handle.as_ref() else {
            self.show_feedback_toast(
                "タグ書き込みが初期化されていません (検索インデックスの起動失敗が原因)".to_string(),
            );
            return;
        };
        let favs = self.settings.favorites.clone();
        for p in &paths {
            h.submit(TagWriteJob {
                path: p.clone(),
                kind: TagJobKind::ClearMiv,
                favorite_id: find_favorite_id(&favs, p),
            });
        }
        self.show_feedback_toast(format!("{} 件から mIV タグをクリア", paths.len()));
    }

    fn ensure_tag_write_handle(&mut self) {
        if self.tag_write_handle.is_some() {
            return;
        }
        let Some(mgr) = self.indexer_manager.as_ref() else {
            return;
        };
        self.tag_write_handle = Some(TagWriteHandle::spawn(
            mgr.clone_fts_meta(),
            mgr.clone_fts_index(),
            mgr.clone_shared_writer(),
        ));
    }
}

impl App {
    /// 毎フレーム呼ぶ: tag_write_worker の結果をドレインしてトーストする。
    /// 完了バッチが揃った瞬間に 1 回だけ tags_cache を invalidate する
    /// (成功 1 件ごとに全消去すると無駄なので末端でまとめる)。
    pub(crate) fn poll_tag_write_results(&mut self) {
        let mut errors: Vec<(PathBuf, String)> = Vec::new();
        let mut success_count: usize = 0;
        let mut just_completed = false;
        if let Some(h) = self.tag_write_handle.as_ref() {
            while let Some(res) = h.try_recv_result() {
                match res.result {
                    Ok(_) => success_count += 1,
                    Err(e) => errors.push((res.path, e)),
                }
            }
            if !h.is_busy() && h.total.load(std::sync::atomic::Ordering::Relaxed) > 0 {
                just_completed = true;
            }
        }
        if !errors.is_empty() {
            let preview = errors
                .iter()
                .take(3)
                .map(|(p, e)| {
                    format!(
                        "{}: {}",
                        p.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
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
        } else if success_count > 0 && just_completed {
            self.show_feedback_toast("タグ書き込み完了".to_string());
        }
        if just_completed {
            self.tags_cache.clear();
            if let Some(h) = self.tag_write_handle.as_ref() {
                h.reset_counters_if_idle();
            }
        }
    }
}

/// 指定 path を含むお気に入りの id を返す (子孫も一致扱い)。
/// Windows の大文字小文字非区別に対応するため `is_under` で正規化比較する。
fn find_favorite_id(
    favorites: &[crate::settings::FavoriteEntry],
    path: &Path,
) -> Option<uuid::Uuid> {
    for fav in favorites {
        if crate::search_index_db::is_under(path, &fav.path) {
            return Some(fav.id);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn recognizes_jpeg_png_webp() {
        assert!(crate::xmp_writer::is_writable_format(std::path::Path::new(
            "a.jpg"
        )));
        assert!(crate::xmp_writer::is_writable_format(std::path::Path::new(
            "A.JPG"
        )));
        assert!(crate::xmp_writer::is_writable_format(std::path::Path::new(
            "b.jpeg"
        )));
        assert!(crate::xmp_writer::is_writable_format(std::path::Path::new(
            "c.png"
        )));
        assert!(crate::xmp_writer::is_writable_format(std::path::Path::new(
            "d.webp"
        )));
    }

    #[test]
    fn rejects_non_writable() {
        assert!(!crate::xmp_writer::is_writable_format(
            std::path::Path::new("a.heic")
        ));
        assert!(!crate::xmp_writer::is_writable_format(
            std::path::Path::new("b.tiff")
        ));
        assert!(!crate::xmp_writer::is_writable_format(
            std::path::Path::new("c.cr2")
        ));
        assert!(!crate::xmp_writer::is_writable_format(
            std::path::Path::new("d.mp4")
        ));
        assert!(!crate::xmp_writer::is_writable_format(
            std::path::Path::new("no_ext")
        ));
    }
}
