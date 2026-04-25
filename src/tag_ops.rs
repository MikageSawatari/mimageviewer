//! タグ付与/削除操作のファサード (docs/tag-feature.md §5)。
//!
//! メニュー・ツールバー・メタデータパネルからのタグ操作のエントリーポイント。
//! XMP 読み書きはすべて `tag_write_worker` に委譲し、UI スレッドは同期 I/O を
//! 一切行わない。

use std::path::{Path, PathBuf};

use crate::app::App;
use crate::grid_item::GridItem;
use crate::tag_write_worker::{TagAction, TagJobKind, TagWriteHandle, TagWriteJob};

impl App {
    /// タグ書き込みの対象ファイル列。優先順位:
    ///   1. **フルスクリーン中** (`fullscreen_idx` Some) は常にフルスクリーン中のページのみ。
    ///      古い `checked` がグリッドに残っていても無視する (フルスクリーンで見えているのは
    ///      1 枚なので、ユーザーの「これにタグ」期待と必ず一致させる)。
    ///   2. グリッドで `checked` が **selected も含む** 形で揃っていれば、checked 全件を bulk 対象
    ///      にする (典型的な multi-select フロー)。selected が checked に含まれない場合は
    ///      「checked は古い残りもの」とみなして selected 単体に落とす — クリックしたサムネが
    ///      対象にならない事故を防ぐ。
    ///   3. checked が空なら selected 単体。
    ///   4. selected も無ければ fullscreen_idx (フルスクリーンに居なくても残っていれば)。
    /// いずれも `Image` かつ書き込み対応形式 (JPEG/PNG/WebP) のものだけ。
    pub(crate) fn tag_target_paths(&self) -> Vec<PathBuf> {
        let push_writable_image = |out: &mut Vec<PathBuf>, idx: usize, items: &[GridItem]| {
            if let Some(GridItem::Image(p)) = items.get(idx) {
                if crate::xmp_writer::is_writable_format(p) {
                    out.push(p.clone());
                }
            }
        };

        let mut out: Vec<PathBuf> = Vec::new();
        if let Some(fs_idx) = self.fullscreen_idx {
            push_writable_image(&mut out, fs_idx, &self.items);
            return out;
        }
        let bulk_intent = match self.selected {
            Some(sel) => !self.checked.is_empty() && self.checked.contains(&sel),
            None => !self.checked.is_empty(),
        };
        if bulk_intent {
            let mut indices: Vec<usize> = self.checked.iter().copied().collect();
            indices.sort_unstable(); // worker のジョブ投入順 = トースト集計順を安定化
            for idx in indices {
                push_writable_image(&mut out, idx, &self.items);
            }
            return out;
        }
        if let Some(idx) = self.selected {
            push_writable_image(&mut out, idx, &self.items);
        }
        out
    }

    pub(crate) fn tag_target_path_count(&self) -> usize {
        self.tag_target_paths().len()
    }

    pub(crate) fn request_tag_toggle_for_selection(&mut self, name: &str) {
        let name_owned = name.to_string();
        // Toggle は worker 側で XMP を読んで Add/Remove に解決する。
        // 結果 (付与されたか / 削除されたか) は `poll_tag_write_results` が完了時に
        // まとめてトースト表示するため、ここでは事前トーストを出さない。
        self.tag_toast_label = Some(format!("#{name_owned}"));
        let mode = if self.fullscreen_idx.is_some() {
            "fullscreen"
        } else {
            "grid"
        };
        crate::logger::log(format!(
            "[TAG] toggle requested: tag=#{name} mode={mode} \
             selected={:?} fullscreen_idx={:?} checked_count={}",
            self.selected,
            self.fullscreen_idx,
            self.checked.len(),
        ));
        let paths = self.tag_target_paths();
        if paths.is_empty() {
            return;
        }
        // worker handle を **capture より前に** 確保する (Codex P2 #2)。
        // 失敗するなら tags_cache 楽観更新も Undo push もせず早期 return — そうしないと
        // 「実ファイルは変わっていないのに UI バッジと Undo stack だけ進んだ」状態になる。
        if !self.precheck_tag_write_available("toggle") {
            return;
        }
        // tags_cache は grid 表示で既に warm 済みなので追加 I/O は発生しない。
        //
        // **既知制約 (Codex P3, 後続課題)**: capture_tag_undo は worker 投入前に
        // tags_cache から before/after を予測する。worker の Toggle は実ファイルの
        // dc:subject を読み直して Add/Remove を決めるため、cache が stale (= 外部
        // ツールが XMP を書き換え済み) のときや、個別ファイルの XMP 書き込みが失敗
        // した場合に、Undo entry と実ディスク状態がずれ得る。Ctrl+Z 後にユーザが
        // 「変化なし」と感じるが、実ディスクは触っていないので破壊性はない。
        //
        // 完全な解決には worker が `tags_before` (書き込み直前の disk 状態) を結果に
        // 載せ、poll_tag_write_results が成功分だけ Undo entry を事後確定する仕組みが
        // 要る (worker protocol 拡張 + pending undo buffer の追加)。
        let with_hash = format!("#{name_owned}");
        let summary = format!("#{name_owned} のトグル");
        self.capture_tag_undo(&paths, summary, |before| {
            if before.iter().any(|t| t == &with_hash) {
                before.iter().filter(|t| *t != &with_hash).cloned().collect()
            } else {
                let mut after = before.to_vec();
                after.push(with_hash.clone());
                after
            }
        });
        let name_for_jobs = name_owned;
        self.submit_tag_jobs(&paths, "toggle", move |_| {
            TagJobKind::Toggle(name_for_jobs.clone())
        });
    }

    pub(crate) fn request_tag_clear_for_selection(&mut self) {
        self.tag_toast_label = None; // clear は付与/削除ラベル不要 (complete 時にクリア件数で集計)
        let paths = self.tag_target_paths();
        let count = paths.len();
        if count == 0 {
            crate::logger::log(
                "[TAG] clear requested but tag_target_paths is empty — ignoring".to_string(),
            );
            return;
        }
        crate::logger::log(format!(
            "[TAG] clear requested for {count} file(s) (mIV tags only)"
        ));
        // worker handle 不在なら capture もせず早期 return (Codex P2 #2)。
        if !self.precheck_tag_write_available("clear") {
            return;
        }
        // ClearMiv 後の dc:subject は `#` 始まり要素を除いたものになる。
        let summary = format!("{count} 件の mIV タグをクリア");
        self.capture_tag_undo(&paths, summary, |before| {
            before
                .iter()
                .filter(|t| !t.starts_with('#'))
                .cloned()
                .collect()
        });
        self.show_feedback_toast(format!("{count} 件から mIV タグをクリア中"));
        self.submit_tag_jobs(&paths, "clear", |_| TagJobKind::ClearMiv);
    }

    /// `tag_write_handle` を遅延初期化し、利用可能か確認する。
    /// 利用不可ならエラートーストを表示して `false` を返す。
    /// 呼び出し側は `false` のとき capture / submit を一切スキップすること。
    fn precheck_tag_write_available(&mut self, op_label: &str) -> bool {
        self.ensure_tag_write_handle();
        if self.tag_write_handle.is_none() {
            crate::logger::log(format!(
                "[TAG] '{op_label}' aborted: tag_write_handle unavailable (no indexer_manager)"
            ));
            self.show_feedback_toast(TAG_WRITE_UNAVAILABLE_MSG.to_string());
            return false;
        }
        true
    }

    /// タグ書き込みジョブ投入の共通経路。
    /// - 呼び出し側が `tag_target_paths()` で算出した `paths` をそのまま渡すこと
    ///   (Undo 用 snapshot と同じ列を使うため、関数内で再算出すると 2 度走査になる)。
    /// - 対象 path が 0 件 → 黙って何もしない
    /// - `tag_write_handle` 初期化失敗 → エラートーストを出して失敗を明示
    /// - 正常 → 各 path で `kind_for` を呼んでジョブを作成する (完了トーストは
    ///   `poll_tag_write_results` が集計結果で出す)
    fn submit_tag_jobs(
        &mut self,
        paths: &[PathBuf],
        op_label: &str,
        kind_for: impl Fn(&PathBuf) -> TagJobKind,
    ) {
        if paths.is_empty() {
            crate::logger::log(format!(
                "[TAG] submit '{op_label}' aborted: tag_target_paths is empty \
                 (selected={:?} fullscreen_idx={:?} checked_count={})",
                self.selected,
                self.fullscreen_idx,
                self.checked.len(),
            ));
            return;
        }
        self.ensure_tag_write_handle();
        let Some(h) = self.tag_write_handle.as_ref() else {
            crate::logger::log(format!(
                "[TAG] submit '{op_label}' aborted: tag_write_handle unavailable (no indexer_manager)"
            ));
            self.show_feedback_toast(TAG_WRITE_UNAVAILABLE_MSG.to_string());
            return;
        };
        crate::logger::log(format!(
            "[TAG] submitting '{op_label}' for {} file(s):",
            paths.len()
        ));
        for p in paths {
            let fav_id = find_favorite_id(&self.settings.favorites, p);
            crate::logger::log(format!(
                "[TAG]   → {} (favorite_id={:?})",
                p.display(),
                fav_id.map(|u| u.to_string())
            ));
            h.submit(TagWriteJob {
                path: p.clone(),
                kind: kind_for(p),
                favorite_id: fav_id,
            });
        }
    }

    pub(crate) fn ensure_tag_write_handle(&mut self) {
        if self.tag_write_handle.is_some() {
            return;
        }
        let Some(mgr) = self.indexer_manager.as_ref() else {
            crate::logger::log(
                "tag_ops: indexer_manager が未初期化のためタグ書き込み worker を起動できない"
                    .to_string(),
            );
            return;
        };
        self.tag_write_handle = Some(TagWriteHandle::spawn(
            mgr.clone_fts_meta(),
            mgr.clone_fts_index(),
            mgr.clone_shared_writer(),
        ));
    }
}

/// タグ書き込みが無効化されている時のユーザー向けエラー文言。
/// `submit_tag_jobs` の None 経路でトースト表示する。
const TAG_WRITE_UNAVAILABLE_MSG: &str =
    "タグ書き込みが初期化されていません (検索インデックスの起動失敗が原因)";

impl App {
    /// 毎フレーム呼ぶ: tag_write_worker の結果をドレインしてトーストする。
    /// 成功した各 path については worker が書いた dc:subject をそのまま `tags_cache` に
    /// 反映する — fts_meta に行があろうと無かろうと grid バッジが即時更新される。
    pub(crate) fn poll_tag_write_results(&mut self) {
        let mut errors: Vec<(PathBuf, String)> = Vec::new();
        let mut added = 0usize;
        let mut removed = 0usize;
        let mut cleared = 0usize;
        let mut restored = 0usize;
        let mut noop = 0usize;
        let mut just_completed = false;
        // worker が返してきた (path, 書き込み後タグ列) を後でまとめて tags_cache に反映する。
        // fts_meta に行が無い favorite (未インデックス) でも、ここで直接書き戻せば
        // 次フレームの `cell_tag_list` が正しい値を拾えるため add/remove 対称になる。
        let mut cache_updates: Vec<(PathBuf, Vec<String>)> = Vec::new();
        if let Some(h) = self.tag_write_handle.as_ref() {
            while let Some(res) = h.try_recv_result() {
                let path_disp = res.path.display().to_string();
                match res.result {
                    Ok(action) => {
                        match action {
                            TagAction::Added => {
                                added += 1;
                                crate::logger::log(format!("[TAG]   ✓ added → {path_disp}"));
                            }
                            TagAction::Removed => {
                                removed += 1;
                                crate::logger::log(format!("[TAG]   ✓ removed → {path_disp}"));
                            }
                            TagAction::Cleared => {
                                cleared += 1;
                                crate::logger::log(format!(
                                    "[TAG]   ✓ cleared mIV tags → {path_disp}"
                                ));
                            }
                            TagAction::Restored => {
                                restored += 1;
                                crate::logger::log(format!(
                                    "[TAG]   ✓ restored dc:subject (undo/redo) → {path_disp}"
                                ));
                            }
                            TagAction::NoOp => {
                                noop += 1;
                                crate::logger::log(format!(
                                    "[TAG]   = no-op (already in target state) → {path_disp}"
                                ));
                            }
                        }
                        cache_updates.push((res.path, res.tags_after));
                    }
                    Err(e) => {
                        crate::logger::log(format!("[TAG]   ✗ FAILED: {e} → {path_disp}"));
                        errors.push((res.path, e));
                    }
                }
            }
            if !h.is_busy() && h.total.load(std::sync::atomic::Ordering::Relaxed) > 0 {
                just_completed = true;
            }
        }
        // 成功分は即座に tags_cache へ反映 (just_completed を待たず)。
        // これで bulk トグルの途中フレームでも、処理済みのセルからバッジが更新されていく。
        for (path, tags) in cache_updates {
            let key = crate::adjustment_db::normalize_path(&path);
            self.tags_cache.insert(key, tags);
        }
        if just_completed {
            crate::logger::log(format!(
                "[TAG] batch complete: added={added} removed={removed} cleared={cleared} \
                 restored={restored} noop={noop} errors={}",
                errors.len()
            ));
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
        } else if just_completed && (added + removed + cleared + restored + noop) > 0 {
            let label = self.tag_toast_label.take();
            let msg = format_completion_toast(
                label.as_deref(),
                added,
                removed,
                cleared,
                restored,
                noop,
            );
            self.show_feedback_toast(msg);
        }
        if just_completed {
            if let Some(h) = self.tag_write_handle.as_ref() {
                h.reset_counters_if_idle();
            }
        }
    }
}

/// 完了トーストの文言を組み立てる。Toggle で付与/削除が混在するケース (複数選択で
/// 既に付与済のものと未付与のものが混ざる) にも耐える形式で出す。
/// Undo/Redo 由来の SetTags (=`restored`) が混ざる場合は専用文言で出す。
fn format_completion_toast(
    tag_label: Option<&str>,
    added: usize,
    removed: usize,
    cleared: usize,
    restored: usize,
    noop: usize,
) -> String {
    if restored > 0 {
        return format!("{restored} 件のタグを元に戻しました");
    }
    if cleared > 0 || (noop > 0 && added == 0 && removed == 0) {
        let total_clear = cleared + noop;
        return format!("{total_clear} 件から mIV タグをクリア");
    }
    let tag = tag_label.unwrap_or("タグ");
    match (added, removed) {
        (a, 0) if a > 0 => format!("{a} 件に {tag} を付与"),
        (0, r) if r > 0 => format!("{r} 件から {tag} を削除"),
        (a, r) => format!("{tag}: {a} 件付与 / {r} 件削除"),
    }
}

/// 指定 path を含むお気に入りの id を返す (子孫も一致扱い)。
/// Windows の大文字小文字非区別に対応するため `is_under` で正規化比較する。
/// `undo_ops.rs` の Undo/Redo タグ復元ジョブからも使うため `pub(crate)`。
pub(crate) fn find_favorite_id(
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
    use super::format_completion_toast;

    #[test]
    fn toast_single_add() {
        assert_eq!(
            format_completion_toast(Some("#ドール"), 1, 0, 0, 0, 0),
            "1 件に #ドール を付与"
        );
    }

    #[test]
    fn toast_single_remove() {
        assert_eq!(
            format_completion_toast(Some("#ドール"), 0, 1, 0, 0, 0),
            "1 件から #ドール を削除"
        );
    }

    #[test]
    fn toast_mixed_add_remove() {
        assert_eq!(
            format_completion_toast(Some("#tag"), 2, 3, 0, 0, 0),
            "#tag: 2 件付与 / 3 件削除"
        );
    }

    #[test]
    fn toast_clear_miv() {
        assert_eq!(
            format_completion_toast(None, 0, 0, 5, 0, 0),
            "5 件から mIV タグをクリア"
        );
    }

    #[test]
    fn toast_clear_miv_with_noop() {
        assert_eq!(
            format_completion_toast(None, 0, 0, 3, 0, 2),
            "5 件から mIV タグをクリア"
        );
    }

    #[test]
    fn toast_restore_for_undo() {
        // Undo/Redo の SetTags は専用文言。タグラベルや add/remove 件数より優先される。
        assert_eq!(
            format_completion_toast(None, 0, 0, 0, 4, 0),
            "4 件のタグを元に戻しました"
        );
    }

    #[test]
    fn toast_restore_with_noop_does_not_say_clear() {
        // Codex P3 回帰: 部分的に NoOp が混じった Restore でも「元に戻した」を
        // 専用文言として優先する (誤って「mIV タグをクリア」にならない)。
        // 現状 worker は SetTags を必ず Restored 扱いにするため `noop=0` のはずだが、
        // 将来 NoOp が混ざるロジックに変えても文言が壊れないように回帰テストを置く。
        assert_eq!(
            format_completion_toast(None, 0, 0, 0, 3, 2),
            "3 件のタグを元に戻しました"
        );
    }

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
