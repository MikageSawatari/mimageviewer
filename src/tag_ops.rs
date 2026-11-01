//! タグ付与/削除操作のファサード (docs/archive/search-metadata/tag-feature.md §5)。
//!
//! メニュー・ツールバー・メタデータパネルからのタグ操作のエントリーポイント。
//! tags.db 更新はすべて `tag_write_worker` に委譲する。UI 側は all-or-nothing 判定に
//! 必要な既存タグだけを tags.db から補完し、ファイルメタデータ I/O は行わない。

use std::path::PathBuf;

use crate::app::App;
use crate::grid_item::GridItem;
use crate::tag_write_worker::{
    TagAction, TagJobKind, TagSidecarTarget, TagWriteHandle, TagWriteJob,
};

#[derive(Clone, Debug)]
pub(crate) struct TagTarget {
    pub(crate) path: PathBuf,
    pub(crate) tag_sidecar: Option<TagSidecarTarget>,
}

fn tag_target_for_item(item: &GridItem, fullscreen: bool) -> Option<TagTarget> {
    let (path, sidecar_path) = match item {
        GridItem::Folder(p)
        | GridItem::Image(p)
        | GridItem::Video(p)
        | GridItem::Audio(p)
        | GridItem::ZipFile(p)
        | GridItem::PdfFile(p) => (p.clone(), !matches!(item, GridItem::Folder(_))),
        GridItem::ConvertibleArchive { path: p, .. } => (p.clone(), true),
        GridItem::ZipImage { zip_path, .. } if fullscreen => (zip_path.clone(), true),
        GridItem::PdfPage { pdf_path, .. } if fullscreen => (pdf_path.clone(), true),
        _ => return None,
    };
    Some(TagTarget {
        tag_sidecar: sidecar_path
            .then(|| crate::tag_write_worker::sidecar_target_for_real_file(&path))
            .flatten(),
        path,
    })
}

impl App {
    /// 変換アーカイブ (RAR/7z/LZH) 閲覧中のタグ対象 remap。
    ///
    /// フルスクリーンの ZipImage フォールバックは `zip_path` (= archive_cache の
    /// 変換済み ZIP) を返すが、タグの item_key は **元アーカイブのパス** に紐づける
    /// (docs/tag-catalog-redesign-plan.md §8.3)。さもないとグリッドの
    /// `ConvertibleArchive` セル (元パスキー) とキーが割れてバッジが出ず、
    /// キャッシュ削除後のタグビュー prune でタグ行が恒久消失する。
    /// ★ の `zip_rating_root_path` / ピンの `zip_pin_root_path` と同じ規則。
    pub(crate) fn remap_tag_target_path(&self, path: PathBuf) -> PathBuf {
        if let Some(src) = self.archive_source_override.as_ref()
            && self
                .current_folder
                .as_ref()
                .is_some_and(|cur| crate::folder_tree::path_eq(cur, &path))
        {
            src.clone()
        } else {
            path
        }
    }

    fn remap_tag_target(&self, mut target: TagTarget) -> TagTarget {
        let remapped = self.remap_tag_target_path(target.path.clone());
        if remapped != target.path {
            target.path = remapped;
            // サイドカーの書き先も元アーカイブの隣に追従させる
            // (archive_cache フォルダに mimageviewer.dat を作らない)。
            if target.tag_sidecar.is_some() {
                target.tag_sidecar =
                    crate::tag_write_worker::sidecar_target_for_real_file(&target.path);
            }
        }
        target
    }

    /// 選択解決の共有ポリシー。発火面 (`ActionSurface`) で対象が決まる:
    ///   - **Viewer 面**: ビューアの現在アイテム (`fullscreen_idx`) のみ。古い `checked` が
    ///     グリッドに残っていても無視する (ビューアで見えているのは 1 枚なので、
    ///     ユーザーの「これに操作」期待と必ず一致させる)。
    ///   - **MainWindow 面**: `fullscreen_idx` は見ない (detached viewer 中はグリッドと
    ///     ビューアを同時に操作できるため)。グリッドで `checked` が **selected も含む** 形で
    ///     揃っていれば checked 全件を bulk 対象にする (典型的な multi-select フロー)。
    ///     selected が checked に含まれない場合は「checked は古い残りもの」とみなして
    ///     selected 単体に落とす — クリックしたサムネが対象にならない事故を防ぐ。
    ///     checked が空なら selected 単体。
    ///
    /// タグ操作の全入口がこの 1 実装を使う。bulk_intent の stale-check が入口ごとに
    /// 分かれると、同じ選択でも対象ファイル集合がずれる。
    pub(crate) fn selection_target_indices(
        &self,
        surface: crate::app::ActionSurface,
    ) -> Vec<usize> {
        // Viewer 面はビューアの現在アイテムだけを対象にする。MainWindow 面では
        // fullscreen_idx を見ない: detached viewer 中はグリッドとビューアを同時に
        // 操作できるため、「fullscreen_idx があればフルスクリーン対象」という従来の
        // 推定ではグリッド操作が detached 窓のアイテムに誤爆する (findings-19)。
        if matches!(surface, crate::app::ActionSurface::Viewer) {
            return self.fullscreen_idx.map(|idx| vec![idx]).unwrap_or_default();
        }
        let bulk_intent = match self.selected {
            Some(sel) => !self.checked.is_empty() && self.checked.contains(&sel),
            None => !self.checked.is_empty(),
        };
        if bulk_intent {
            let mut indices: Vec<usize> = self.checked.iter().copied().collect();
            indices.sort_unstable(); // worker のジョブ投入順 = トースト集計順を安定化
            return indices;
        }
        self.selected.map(|idx| vec![idx]).unwrap_or_default()
    }

    pub(crate) fn tag_target_for_index(&self, idx: usize, fullscreen: bool) -> Option<TagTarget> {
        let item = self.items.get(idx)?;
        tag_target_for_item(item, fullscreen).map(|mut target| {
            if self.idx_is_compiled_book_page(idx) {
                target.tag_sidecar = None;
            }
            self.remap_tag_target(target)
        })
    }

    pub(crate) fn tag_target_for_path(&self, path: PathBuf, write_sidecar: bool) -> TagTarget {
        self.remap_tag_target(TagTarget {
            tag_sidecar: write_sidecar
                .then(|| crate::tag_write_worker::sidecar_target_for_real_file(&path))
                .flatten(),
            path,
        })
    }

    /// タグ書き込みの対象ファイル列 (`selection_target_indices` の解決結果のうち、
    /// 実パスを持つタグ対応 item のみ)。ZIP/PDF ページのビューア面では
    /// コンテナ自身へフォールバックする (変換アーカイブは元パスへ remap)。
    fn tag_targets(&self, surface: crate::app::ActionSurface) -> Vec<TagTarget> {
        let fullscreen = matches!(surface, crate::app::ActionSurface::Viewer);
        self.selection_target_indices(surface)
            .into_iter()
            .filter_map(|idx| self.tag_target_for_index(idx, fullscreen))
            .collect()
    }

    pub(crate) fn tag_target_paths(&self, surface: crate::app::ActionSurface) -> Vec<PathBuf> {
        self.tag_targets(surface)
            .into_iter()
            .map(|target| target.path)
            .collect()
    }

    pub(crate) fn tag_target_path_count(&self, surface: crate::app::ActionSurface) -> usize {
        self.tag_targets(surface).len()
    }

    pub(crate) fn request_tag_toggle_for_selection(
        &mut self,
        name: &str,
        surface: crate::app::ActionSurface,
    ) {
        let mode = match surface {
            crate::app::ActionSurface::Viewer => "fullscreen",
            crate::app::ActionSurface::MainWindow => "grid",
        };
        let targets = self.tag_targets(surface);
        self.request_tag_toggle_for_targets_impl(name, targets, mode, surface);
    }

    /// 「今いるコンテナ」(current_folder = 実フォルダ / ZIP / PDF) にタグをトグルする。
    /// ツールバーのピンタグ Shift+右クリック用 (toolbar-customization-plan.md §1.1)。
    /// 合成ビュー (検索 / タグビュー / 閲覧履歴) は対象コンテナが無いので no-op + 通知。
    pub(crate) fn request_tag_toggle_for_current_container(&mut self, name: &str) {
        // ツールバーのピンタグ専用 (= グリッド面固定)。
        let surface = crate::app::ActionSurface::MainWindow;
        let Some(folder) = self.current_folder.clone() else {
            self.show_feedback_warning_on("タグを付けるコンテナがありません".to_string(), surface);
            return;
        };
        // 合成ビューでは実コンテナが無いので no-op + 通知。判定は経路ごとに異なる:
        //   - Ctrl+G グローバル検索: current_folder は検索前の実フォルダのまま + global_search.active
        //     (擬似パスにならないのでフラグで判定 — これを落として実害が出た)。
        //   - Ctrl+S お気に入り検索 / 閲覧履歴: current_folder が擬似パスになるのでパス一致で判定。
        //   - タグビュー / ドライブ一覧: 専用フラグ。
        if self.global_search.active
            || crate::app::is_synthetic_view_path(&folder)
            || self.items_are_tag_view
            || self.items_are_drive_list
        {
            self.show_feedback_warning_on(
                "この画面ではコンテナにタグを付けられません".to_string(),
                surface,
            );
            return;
        }
        // コンテナ (フォルダ/ZIP/PDF) は XMP サイドカー非対象なので tags.db のみ。
        let target = self.tag_target_for_path(folder, false);
        self.request_tag_toggle_for_targets(name, vec![target], surface);
    }

    pub(crate) fn request_tag_toggle_for_targets(
        &mut self,
        name: &str,
        targets: Vec<TagTarget>,
        surface: crate::app::ActionSurface,
    ) {
        self.request_tag_toggle_for_targets_impl(name, targets, "explicit", surface);
    }

    fn request_tag_toggle_for_targets_impl(
        &mut self,
        name: &str,
        targets: Vec<TagTarget>,
        mode: &str,
        surface: crate::app::ActionSurface,
    ) {
        let name_owned = crate::tags_db::normalize_tag_display_name(name);
        if name_owned.is_empty() {
            return;
        }
        crate::logger::log(format!(
            "[TAG] toggle requested: tag=#{name} mode={mode} \
             selected={:?} fullscreen_idx={:?} checked_count={}",
            self.selected,
            self.fullscreen_idx,
            self.checked.len(),
        ));
        if targets.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = targets.iter().map(|target| target.path.clone()).collect();
        if !self.precheck_tag_write_available("toggle") {
            return;
        }
        self.prepare_page_edit_history_boundary();
        // 複数選択は all-or-nothing: 全対象に付与済みなら全削除、それ以外は全付与。
        // 結果は `poll_tag_write_results` が完了時にまとめてトースト表示する。
        // ラベルは worker キュー全体で共有する 1 フィールドなので、**バリデーション通過後**
        // に設定する。早期 return 経路で上書きすると、in-flight の前バッチの完了トーストが
        // 誤ったタグ名を名乗る (空 targets での中断 → 古いバッチに新ラベル)。
        self.tag_toast_label = Some(format!("#{name_owned}"));
        self.set_tag_toast_surface_for_new_batch(surface);
        self.hydrate_tags_cache_for_paths(&paths);
        let tag_key = crate::tags_db::normalize_tag_key(&name_owned);
        let all_have_tag = paths.iter().all(|path| {
            let key = crate::tags_db::item_key_for_path(path);
            self.tags_cache.get(&key).is_some_and(|tags| {
                tags.iter()
                    .any(|tag| crate::tags_db::normalize_tag_key(tag) == tag_key)
            })
        });

        // 楽観的 UI 更新: tags_cache を「予想した after」に書き換えてグリッドバッジを
        // 即時反映する。**Undo entry はここでは積まない** — worker 結果が
        // 「実 DB の before/after」を持って戻った時点で `poll_tag_write_results` が
        // pending_tag_undos から組み立てて確定する (Codex P3 完全対応)。
        let with_hash = crate::tags_db::format_display_tag(&name_owned);
        let summary = if all_have_tag {
            format!("{with_hash} の削除")
        } else {
            format!("{with_hash} の付与")
        };
        self.optimistic_update_tags_cache(&paths, |before| {
            if all_have_tag {
                before
                    .iter()
                    .filter(|t| crate::tags_db::normalize_tag_key(*t) != tag_key)
                    .cloned()
                    .collect()
            } else {
                let mut after = before.to_vec();
                if !after
                    .iter()
                    .any(|tag| crate::tags_db::normalize_tag_key(tag) == tag_key)
                {
                    after.push(with_hash.clone());
                }
                after
            }
        });
        let tx_id = self.next_tag_tx_id();
        self.register_pending_tag_op(tx_id, summary, paths.len());
        let name_for_jobs = name_owned;
        self.submit_tag_jobs(&targets, "toggle", tx_id, move |_, _| {
            if all_have_tag {
                TagJobKind::Remove(name_for_jobs.clone())
            } else {
                TagJobKind::Add(name_for_jobs.clone())
            }
        });
    }

    pub(crate) fn request_tag_add_for_selection(
        &mut self,
        name: &str,
        surface: crate::app::ActionSurface,
    ) {
        self.request_tag_set_for_selection(name, true, surface);
    }

    pub(crate) fn request_tag_remove_for_selection(
        &mut self,
        name: &str,
        surface: crate::app::ActionSurface,
    ) {
        self.request_tag_set_for_selection(name, false, surface);
    }

    pub(crate) fn request_tag_add_for_targets(
        &mut self,
        name: &str,
        targets: Vec<TagTarget>,
        surface: crate::app::ActionSurface,
    ) {
        self.request_tag_set_for_targets(name, targets, true, surface);
    }

    pub(crate) fn request_tag_remove_for_targets(
        &mut self,
        name: &str,
        targets: Vec<TagTarget>,
        surface: crate::app::ActionSurface,
    ) {
        self.request_tag_set_for_targets(name, targets, false, surface);
    }

    fn request_tag_set_for_selection(
        &mut self,
        name: &str,
        add: bool,
        surface: crate::app::ActionSurface,
    ) {
        let targets = self.tag_targets(surface);
        self.request_tag_set_for_targets(name, targets, add, surface);
    }

    fn request_tag_set_for_targets(
        &mut self,
        name: &str,
        targets: Vec<TagTarget>,
        add: bool,
        surface: crate::app::ActionSurface,
    ) {
        let name = crate::tags_db::normalize_tag_display_name(name);
        if name.is_empty() {
            return;
        }
        if targets.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = targets.iter().map(|target| target.path.clone()).collect();
        let tag_key = crate::tags_db::normalize_tag_key(&name);
        if tag_key.is_empty() {
            return;
        }
        if !self.precheck_tag_write_available(if add { "add" } else { "remove" }) {
            return;
        }
        self.prepare_page_edit_history_boundary();
        let with_hash = crate::tags_db::format_display_tag(&name);
        self.tag_toast_label = Some(with_hash.clone());
        self.set_tag_toast_surface_for_new_batch(surface);
        let summary = if add {
            format!("{with_hash} の付与")
        } else {
            format!("{with_hash} の削除")
        };
        let tx_id = self.next_tag_tx_id();
        self.register_pending_tag_op(tx_id, summary, paths.len());
        self.submit_tag_set_lower(&targets, &[name], add, tx_id, &[]);
    }

    /// UI admission has already checked permission/Remote/modal state. Input
    /// positions refer to the complete original-order record (including skips).
    /// This entry deliberately does not finalize user edits or touch Undo/toast routing.
    pub(crate) fn request_agent_tag_set_for_targets(
        &mut self,
        targets: Vec<(usize, TagTarget)>,
        tags: &[String],
        add: bool,
        client_name: String,
        rows: Vec<crate::agent_ipc::changes::ChangeRow>,
        changes: std::sync::Arc<crate::agent_ipc::changes::ChangeController>,
    ) -> usize {
        use crate::agent_ipc::changes::{ChangeCounts, ChangeRecord, ChangeRecordId};
        let queued = targets.len();
        let skipped = rows.len().saturating_sub(queued);
        let record_id = changes.publish(ChangeRecord {
            id: ChangeRecordId::default(),
            at: std::time::SystemTime::now(),
            client_name,
            tool: if add { "add_tags" } else { "remove_tags" }.into(),
            collection: None,
            counts: ChangeCounts {
                queued,
                skipped,
                ..Default::default()
            },
            summary: format!("処理中 {queued} 件 / 対象外 {skipped} 件"),
            rows,
            notice: format!("AI からのタグの変更を受け付けました ({queued} 件)"),
        });
        let origins: Vec<_> = targets
            .iter()
            .map(|(row_index, _)| crate::tag_write_worker::AgentTagChange {
                controller: changes.clone(),
                record_id,
                row_index: *row_index,
            })
            .collect();
        let targets: Vec<_> = targets.into_iter().map(|(_, target)| target).collect();
        let tx_id = self.next_tag_tx_id();
        self.submit_tag_set_lower(&targets, tags, add, tx_id, &origins);
        queued
    }

    /// Shared normalization, optimistic cache publication and writer submission.
    /// User-only history/Undo/toast setup belongs above this boundary.
    fn submit_tag_set_lower(
        &mut self,
        targets: &[TagTarget],
        tags: &[String],
        add: bool,
        tx_id: u64,
        origins: &[crate::tag_write_worker::AgentTagChange],
    ) {
        let tags: Vec<_> = tags
            .iter()
            .map(|tag| crate::tags_db::normalize_tag_display_name(tag))
            .collect();
        let keys: Vec<_> = tags
            .iter()
            .map(|tag| crate::tags_db::normalize_tag_key(tag))
            .collect();
        let paths: Vec<_> = targets.iter().map(|target| target.path.clone()).collect();
        self.hydrate_tags_cache_for_paths(&paths);
        self.optimistic_update_tags_cache(&paths, |before| {
            let mut after = before.to_vec();
            if add {
                for (tag, key) in tags.iter().zip(&keys) {
                    if !after
                        .iter()
                        .any(|old| crate::tags_db::normalize_tag_key(old) == *key)
                    {
                        after.push(crate::tags_db::format_display_tag(tag));
                    }
                }
            } else {
                after.retain(|old| !keys.contains(&crate::tags_db::normalize_tag_key(old)));
            }
            after
        });
        self.submit_tag_jobs(
            targets,
            if add { "add" } else { "remove" },
            tx_id,
            |index, _| TagJobKind::Delta {
                tags: tags.clone(),
                add,
                agent: origins.get(index).cloned(),
            },
        );
    }

    pub(crate) fn tag_maintenance_in_progress(&self) -> bool {
        self.tag_maintenance_rx.is_some()
    }

    pub(crate) fn request_tag_clear_for_selection(&mut self, surface: crate::app::ActionSurface) {
        let targets = self.tag_targets(surface);
        let count = targets.len();
        if count == 0 {
            crate::logger::log(
                "[TAG] clear requested but tag_target_paths is empty — ignoring".to_string(),
            );
            return;
        }
        let paths: Vec<PathBuf> = targets.iter().map(|target| target.path.clone()).collect();
        crate::logger::log(format!(
            "[TAG] clear requested for {count} file(s) (mIV tags only)"
        ));
        if !self.precheck_tag_write_available("clear") {
            return;
        }
        self.prepare_page_edit_history_boundary();
        // ラベル/発火面はバリデーション通過後に設定する (toggle/add と同じ規約。
        // 早期 return 経路で上書きすると in-flight の前バッチの完了トーストが
        // 誤った面/ラベルで表示される — Codex P3)。
        self.tag_toast_label = None; // clear は付与/削除ラベル不要 (complete 時にクリア件数で集計)
        self.set_tag_toast_surface_for_new_batch(surface);
        // 楽観的 UI 更新: tags.db 上の mIV タグを空にする。
        let summary = format!("{count} 件の mIV タグをクリア");
        self.optimistic_update_tags_cache(&paths, |_before| Vec::new());
        self.show_feedback_toast_on(format!("{count} 件から mIV タグをクリア中"), surface);
        let tx_id = self.next_tag_tx_id();
        self.register_pending_tag_op(tx_id, summary, paths.len());
        self.submit_tag_jobs(&targets, "clear", tx_id, |_, _| TagJobKind::ClearMiv);
    }

    /// 新しいタグバッチの発火面を単一スロット `tag_toast_surface` に記録する。
    /// 完了トーストはキュー全体で 1 本 (label と同じ設計) のため、in-flight バッチと
    /// 発火面が食い違う場合は面を確定できない — その場合は従来の全面表示 (None) に
    /// 落とし、混在バッチの完了が最後の面へ誤着弾しないようにする (Sol P2)。
    ///
    /// in-flight 判定は利用者 origin の未リセット (`has_unconsumed_user_batch`) を使う。
    /// `pending_tag_undos` は `clear_meta_undo` (フォルダ移動等) が worker 稼働中でも
    /// 意図的に破棄するため proxy にならず、`is_busy()` も「完了したが未 poll」の
    /// 結果を取りこぼす。カウンタは完了 poll の `reset_counters_if_idle` まで
    /// 残るので、「前バッチが完了トーストとして未集計」を正しく表す。
    /// この呼び出しは新バッチの submit 前に行われる。
    fn set_tag_toast_surface_for_new_batch(&mut self, surface: crate::app::ActionSurface) {
        let in_flight = self
            .tag_write_handle
            .as_ref()
            .is_some_and(|handle| handle.has_unconsumed_user_batch());
        if !in_flight || self.tag_toast_surface == Some(surface) {
            self.tag_toast_surface = Some(surface);
        } else {
            self.tag_toast_surface = None;
        }
    }

    /// `tag_write_handle` を遅延初期化し、利用可能か確認する。
    /// 利用不可ならエラートーストを表示して `false` を返す。
    /// 呼び出し側は `false` のとき capture / submit を一切スキップすること。
    fn precheck_tag_write_available(&mut self, op_label: &str) -> bool {
        if self.tag_maintenance_in_progress() {
            self.show_feedback_warning("タグの改名/統合を反映中です".into());
            return false;
        }
        // Only request admission uses this precheck; worker result adoption and
        // its follow-up content-identity records must remain runnable.
        if self.book_structural_op.is_some() {
            self.show_feedback_warning("製本処理中です".into());
            return false;
        }
        self.ensure_tag_write_handle();
        if self.tag_write_handle.is_none() {
            crate::logger::log(format!(
                "[TAG] '{op_label}' aborted: tag_write_handle unavailable"
            ));
            self.show_feedback_warning(TAG_WRITE_UNAVAILABLE_MSG.to_string());
            return false;
        }
        // Admission precedes optimistic cache changes and pending Undo capture,
        // not merely the later worker submit. All callers have accepted targets.
        self.admit_book_append_followup_mutation()
    }

    /// タグ書き込みジョブ投入の共通経路。
    /// - 呼び出し側が `tag_targets()` で算出した対象をそのまま渡す。
    /// - `tx_id` は `register_pending_tag_op` で発行したトランザクション ID
    ///   (worker 結果から `pending_tag_undos` を引くためのキー)。Undo 確定不要なら 0。
    /// - 対象 path が 0 件 → 黙って何もしない
    /// - `tag_write_handle` 初期化失敗 → エラートーストを出して失敗を明示
    /// - 正常 → 各 path で `kind_for` を呼んでジョブを作成する (完了トーストは
    ///   `poll_tag_write_results` が集計結果で出す)
    fn submit_tag_jobs(
        &mut self,
        targets: &[TagTarget],
        op_label: &str,
        tx_id: u64,
        kind_for: impl Fn(usize, &PathBuf) -> TagJobKind,
    ) {
        if targets.is_empty() {
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
                "[TAG] submit '{op_label}' aborted: tag_write_handle unavailable"
            ));
            self.show_feedback_warning(TAG_WRITE_UNAVAILABLE_MSG.to_string());
            return;
        };
        crate::logger::log(format!(
            "[TAG] submitting '{op_label}' (tx={tx_id}) for {} item(s):",
            targets.len()
        ));
        for (index, target) in targets.iter().enumerate() {
            let p = &target.path;
            crate::logger::log(format!("[TAG]   → {}", p.display()));
            h.submit(TagWriteJob {
                path: p.clone(),
                tag_sidecar: target.tag_sidecar.clone(),
                kind: kind_for(index, p),
                tx_id,
            });
        }
    }

    pub(crate) fn ensure_tag_write_handle(&mut self) {
        if self.tag_write_handle.is_some() {
            return;
        }
        self.tag_write_handle = Some(TagWriteHandle::spawn());
    }

    pub(crate) fn hydrate_tags_cache_for_paths(&mut self, paths: &[PathBuf]) {
        let mut missing: Vec<String> = paths
            .iter()
            .map(|path| crate::tags_db::item_key_for_path(path))
            .filter(|key| !self.tags_cache.contains_key(key))
            .collect();
        if missing.is_empty() {
            return;
        }
        missing.sort();
        missing.dedup();
        if let Some(db) = self.tags_db.as_ref() {
            let mut loaded = db.get_many_display_tags(&missing);
            for key in missing {
                let tags = loaded.remove(&key).unwrap_or_default();
                self.set_tags_cache_entry(key, tags);
            }
        } else {
            for key in missing {
                self.set_tags_cache_entry(key, Vec::new());
            }
        }
    }
}

/// タグ書き込みが無効化されている時のユーザー向けエラー文言。
/// `submit_tag_jobs` の None 経路でトースト表示する。
const TAG_WRITE_UNAVAILABLE_MSG: &str = "タグ書き込みが初期化されていません";

/// User-origin results belong to the writer batch, rather than a single UI poll.
/// Publication may be adopted before `done`, or before later agent jobs finish.
#[derive(Default)]
pub(crate) struct TagWriteCompletion {
    added: usize,
    removed: usize,
    cleared: usize,
    restored: usize,
    noop: usize,
    errors: Vec<(PathBuf, String)>,
}

impl App {
    /// 毎フレーム呼ぶ: tag_write_worker の結果をドレインしてトーストする。
    /// 成功した各 path については worker が書いた mIV タグ一覧をそのまま `tags_cache` に
    /// 反映する。
    pub(crate) fn poll_tag_write_results(&mut self) {
        let mut agent_errors: Vec<(PathBuf, String)> = Vec::new();
        let mut just_completed = false;
        let mut user_batch_completed = false;
        let mut agent_changed = false;
        // worker が返してきた (path, 書き込み後タグ列) を後でまとめて tags_cache に反映する。
        // 次フレームの `cell_tag_list` が正しい値を拾えるため add/remove 対称になる。
        let mut cache_updates: Vec<(PathBuf, Vec<String>)> = Vec::new();
        let mut sidecar_updates: Vec<(TagSidecarTarget, Vec<String>)> = Vec::new();
        let mut identity_sources: Vec<PathBuf> = Vec::new();
        // pending_tag_undos に積み上げる: (tx_id, TagChange or failure marker)。
        // tx_id == 0 は「Undo 確定不要」(Undo/Redo 由来の SetTags 等) なのでスキップ。
        let mut pending_updates: Vec<PendingUpdate> = Vec::new();
        if let Some(h) = self.tag_write_handle.as_ref() {
            while let Some(res) = h.try_recv_result() {
                let agent = res.agent.is_some();
                if let Some(change) = res.agent.as_ref() {
                    change.complete(&res);
                }
                let path_disp = res.path.display().to_string();
                match res.result {
                    Ok(action) => {
                        let changed = !matches!(action, TagAction::NoOp);
                        agent_changed |= agent && changed;
                        match action {
                            TagAction::Added => {
                                self.tag_write_completion.added += usize::from(!agent);
                                crate::logger::log(format!("[TAG]   ✓ added → {path_disp}"));
                            }
                            TagAction::Removed => {
                                self.tag_write_completion.removed += usize::from(!agent);
                                crate::logger::log(format!("[TAG]   ✓ removed → {path_disp}"));
                            }
                            TagAction::Cleared => {
                                self.tag_write_completion.cleared += usize::from(!agent);
                                crate::logger::log(format!(
                                    "[TAG]   ✓ cleared mIV tags → {path_disp}"
                                ));
                            }
                            TagAction::Restored => {
                                self.tag_write_completion.restored += usize::from(!agent);
                                crate::logger::log(format!(
                                    "[TAG]   ✓ restored tags (undo/redo) → {path_disp}"
                                ));
                            }
                            TagAction::NoOp => {
                                self.tag_write_completion.noop += usize::from(!agent);
                                crate::logger::log(format!(
                                    "[TAG]   = no-op (already in target state) → {path_disp}"
                                ));
                            }
                        }
                        if !agent && res.tx_id != 0 {
                            // 実 disk の before/after を確定情報として pending に積む。
                            pending_updates.push(PendingUpdate::Success {
                                tx_id: res.tx_id,
                                change: crate::undo_stack::TagChange {
                                    path: res.path.clone(),
                                    tag_sidecar: res.tag_sidecar.clone(),
                                    before: res.tags_before,
                                    after: res.tags_after.clone(),
                                },
                            });
                        }
                        if (changed || !agent)
                            && let Some(target) = res.tag_sidecar.clone()
                        {
                            sidecar_updates.push((target, res.tags_after.clone()));
                        }
                        if changed {
                            identity_sources.push(res.path.clone());
                        }
                        cache_updates.push((res.path, res.tags_after));
                    }
                    Err(e) => {
                        crate::logger::log(format!("[TAG]   ✗ FAILED: {e} → {path_disp}"));
                        if !agent && res.tx_id != 0 {
                            pending_updates.push(PendingUpdate::Failure { tx_id: res.tx_id });
                        }
                        // 楽観更新済みの tags_cache を実 DB 状態 (= worker が読み取った
                        // tags_before) に巻き戻す。DB 書き込みは失敗しているので、
                        // グリッドのバッジは予測値で更新済みのため放置すると stale
                        // 表示になる (Codex P3 指摘)。
                        cache_updates.push((res.path.clone(), res.tags_before));
                        if agent {
                            agent_errors.push((res.path, e));
                        } else {
                            self.tag_write_completion.errors.push((res.path, e));
                        }
                    }
                }
            }
            if h.is_complete_and_drained() && h.total.load(std::sync::atomic::Ordering::Relaxed) > 0
            {
                just_completed = true;
                user_batch_completed = h.has_unconsumed_user_batch();
            }
        }
        if agent_changed {
            // A later append Undo must not delete a file that AI has edited.
            // Admission stays side-effect-free; a failed/no-op job preserves
            // the receipt. The writer's unpublished batch blocks Undo until
            // this completion is adopted.
            self.invalidate_book_append_undo();
        }
        for path in identity_sources {
            self.record_content_identity_for_path(
                &path,
                crate::content_identity::ContentIdentityTrigger::Edit,
            );
        }
        // 成功分は即座に tags_cache へ反映 (just_completed を待たず)。
        // これで bulk トグルの途中フレームでも、処理済みのセルからバッジが更新されていく。
        let tags_cache_changed = !cache_updates.is_empty();
        for (path, tags) in cache_updates {
            let key = crate::tags_db::item_key_for_path(&path);
            self.set_tags_cache_entry(key, tags);
        }
        if tags_cache_changed {
            self.invalidate_tag_apply_suggestions();
            self.schedule_current_smart_folder_metadata_refresh(
                crate::app::smart_folder::SmartFolderMetadataDependency::Tags,
            );
        }
        for (target, tags) in sidecar_updates {
            self.mirror_tag_sidecar_update(&target, &tags);
        }
        if tags_cache_changed && self.settings.facet_filter.uses_tag_state() {
            self.rebuild_visible_indices();
        }
        // pending_tag_undos に worker 結果を集計し、完了したトランザクションは
        // UndoEntry::Tag を組み立てて meta_undo に push する。
        self.finalize_pending_tag_undos(pending_updates);
        if just_completed && user_batch_completed {
            let TagWriteCompletion {
                added,
                removed,
                cleared,
                restored,
                noop,
                errors,
            } = std::mem::take(&mut self.tag_write_completion);
            crate::logger::log(format!(
                "[TAG] batch complete: added={added} removed={removed} cleared={cleared} \
                 restored={restored} noop={noop} errors={}",
                errors.len()
            ));
            let label = self.tag_toast_label.take();
            let surface = self.tag_toast_surface.take();
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
                let msg = format!("タグ書き込み失敗 {} 件: {}", errors.len(), preview);
                // 発火面が分かるバッチはその面に出す (undo 復元など面未記録は従来どおり全面)。
                match surface {
                    Some(surface) => self.show_feedback_warning_on(msg, surface),
                    None => self.show_feedback_warning(msg),
                }
            } else if (added + removed + cleared + restored + noop) > 0 {
                let msg = format_completion_toast(
                    label.as_deref(),
                    added,
                    removed,
                    cleared,
                    restored,
                    noop,
                );
                match surface {
                    Some(surface) => self.show_feedback_toast_on(msg, surface),
                    None => self.show_feedback_toast(msg),
                }
            }
        }
        if !agent_errors.is_empty() {
            self.show_feedback_warning(format!(
                "AI からのタグ書き込みに失敗しました ({} 件)",
                agent_errors.len()
            ));
        }
        if just_completed {
            if let Some(h) = self.tag_write_handle.as_ref() {
                h.reset_counters_if_idle();
            }
        }
    }
}

/// `poll_tag_write_results` 内で worker 1 件分の結果を `pending_tag_undos` に
/// どう反映するかを表す中間型。借用衝突を避けるため、ハンドルの drain 中はここに
/// 積み上げて drain 後に `finalize_pending_tag_undos` でまとめて適用する。
enum PendingUpdate {
    /// 成功: Undo entry の `accumulated` に worker の真の before/after を追加する。
    Success {
        tx_id: u64,
        change: crate::undo_stack::TagChange,
    },
    /// 失敗: そのジョブを Undo 対象外にする (failures カウントだけ進める)。
    /// 実ディスクは変わっていないので Undo entry に含めるとずれる。
    Failure { tx_id: u64 },
}

impl App {
    /// テストヘルパー: worker 結果を模擬して `finalize_pending_tag_undos` を駆動する。
    /// 実 worker / handle / channel を持ち込まずに「pending 集計 → meta_undo push」の
    /// パスだけ単体で検証できる。本番コードからは呼ばない。
    #[cfg(test)]
    pub(crate) fn test_finalize_tag_success(
        &mut self,
        tx_id: u64,
        change: crate::undo_stack::TagChange,
    ) {
        self.finalize_pending_tag_undos(vec![PendingUpdate::Success { tx_id, change }]);
    }

    #[cfg(test)]
    pub(crate) fn test_finalize_tag_failure(&mut self, tx_id: u64) {
        self.finalize_pending_tag_undos(vec![PendingUpdate::Failure { tx_id }]);
    }

    /// `poll_tag_write_results` から呼ばれる finalize 補助。
    /// 1) `updates` を `pending_tag_undos` に accumulate する。
    /// 2) `accumulated.len() + failures == expected_total` に達したエントリを `meta_undo`
    ///    に push する (空なら破棄)。
    fn finalize_pending_tag_undos(&mut self, updates: Vec<PendingUpdate>) {
        for u in updates {
            match u {
                PendingUpdate::Success { tx_id, change } => {
                    if let Some(p) = self.pending_tag_undos.get_mut(&tx_id) {
                        p.accumulated.push(change);
                    } else {
                        // pending が消えている = clear_meta_undo 等で boundary を跨いだ
                        // 操作の結果が今頃届いた。Undo entry として復活させない。
                        crate::logger::log(format!(
                            "[TAG] poll: dropped result for tx_id={tx_id} (no pending entry)"
                        ));
                    }
                }
                PendingUpdate::Failure { tx_id } => {
                    if let Some(p) = self.pending_tag_undos.get_mut(&tx_id) {
                        p.failures += 1;
                    }
                }
            }
        }
        // 完了した tx_id を集めて remove → push (借用衝突回避のため 2 段階)。
        let completed: Vec<u64> = self
            .pending_tag_undos
            .iter()
            .filter_map(|(tx, p)| {
                (p.accumulated.len() + p.failures >= p.expected_total).then_some(*tx)
            })
            .collect();
        for tx in completed {
            if let Some(p) = self.pending_tag_undos.remove(&tx) {
                let crate::app::PendingTagUndo {
                    summary,
                    accumulated,
                    ..
                } = p;
                if accumulated.is_empty() {
                    // 全件失敗 or worker 結果が来なかった: Undo entry なし
                    continue;
                }
                self.push_tag_undo_entry(accumulated, summary);
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
    if cleared > 0 || (noop > 0 && added == 0 && removed == 0 && tag_label.is_none()) {
        let total_clear = cleared + noop;
        return format!("{total_clear} 件から mIV タグをクリア");
    }
    let tag = tag_label.unwrap_or("タグ");
    if added == 0 && removed == 0 && noop > 0 {
        return format!("{tag}: {noop} 件は変更なし");
    }
    match (added, removed) {
        (a, 0) if a > 0 => format!("{a} 件に {tag} を付与"),
        (0, r) if r > 0 => format!("{r} 件から {tag} を削除"),
        (a, r) => format!("{tag}: {a} 件付与 / {r} 件削除"),
    }
}

#[cfg(test)]
mod tests {
    use super::format_completion_toast;
    use std::path::PathBuf;

    fn controlled_result(
        job: crate::tag_write_worker::TagWriteJob,
        result: Result<crate::tag_write_worker::TagAction, &str>,
    ) -> crate::tag_write_worker::TagWriteResult {
        let agent = match job.kind {
            crate::tag_write_worker::TagJobKind::Delta { agent, .. } => agent,
            _ => None,
        };
        crate::tag_write_worker::TagWriteResult {
            agent,
            path: job.path,
            tag_sidecar: job.tag_sidecar,
            tags_before: vec![],
            tags_after: if result.is_ok() {
                vec!["#tag".into()]
            } else {
                vec![]
            },
            result: result.map_err(str::to_owned),
            tx_id: job.tx_id,
        }
    }

    fn submit_controlled_user(app: &mut crate::app::App, paths: &[PathBuf]) {
        let targets = paths
            .iter()
            .map(|path| super::TagTarget {
                path: path.clone(),
                tag_sidecar: None,
            })
            .collect();
        app.request_tag_add_for_targets("tag", targets, crate::app::ActionSurface::MainWindow);
    }

    fn submit_controlled_agent(app: &mut crate::app::App, path: PathBuf) {
        use crate::agent_ipc::changes::{ChangeController, ChangeRow, ChangeRowResult};
        app.request_agent_tag_set_for_targets(
            vec![(
                0,
                super::TagTarget {
                    path,
                    tag_sidecar: None,
                },
            )],
            &["agent".into()],
            true,
            "client".into(),
            vec![ChangeRow {
                result: ChangeRowResult::Pending,
                ..Default::default()
            }],
            ChangeController::new(),
        );
    }

    #[test]
    fn user_completion_toast_survives_adoption_before_done() {
        use crate::tag_write_worker::{TagAction, TagWriteHandle};
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let (handle, driver) = TagWriteHandle::controlled_for_test();
        app.tag_write_handle = Some(handle);
        let path = app.tmp.path().join("user.jpg");
        submit_controlled_user(&mut app, &[path]);
        let job = driver.recv_job();
        assert!(app.pending_tag_undos.contains_key(&job.tx_id));
        let notifications = app.feedback_toasts_for_test.len();
        driver.publish_result(controlled_result(job, Ok(TagAction::Added)));
        app.poll_tag_write_results();
        assert_eq!(app.feedback_toasts_for_test.len(), notifications);
        assert_eq!(app.tag_write_completion.added, 1);
        assert_eq!(app.tag_toast_label.as_deref(), Some("#tag"));
        assert_eq!(
            app.tag_toast_surface,
            Some(crate::app::ActionSurface::MainWindow)
        );
        assert!(app.pending_tag_undos.is_empty());
        assert_eq!(app.meta_undo.undo_len(), 1);
        app.poll_tag_write_results();
        assert_eq!(app.feedback_toasts_for_test.len(), notifications);
        driver.finish_job();
        app.poll_tag_write_results();
        assert_eq!(app.feedback_toasts_for_test.len(), notifications + 1);
        assert_eq!(
            app.fs_feedback_toast.as_ref().unwrap().0,
            "1 件に #tag を付与"
        );
        assert_eq!(
            app.fs_feedback_toast_surface,
            Some(crate::app::ActionSurface::MainWindow)
        );
        assert!(app.tag_toast_label.is_none());
        assert!(app.tag_toast_surface.is_none());
        assert_eq!(app.tag_write_completion.added, 0);
        assert!(!app.tag_writer_has_unpublished_jobs());
        app.poll_tag_write_results();
        assert_eq!(app.feedback_toasts_for_test.len(), notifications + 1);
    }

    #[test]
    fn mixed_batch_keeps_all_user_counts_until_later_agent_completion() {
        use crate::tag_write_worker::{TagAction, TagWriteHandle};
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let (handle, driver) = TagWriteHandle::controlled_for_test();
        app.tag_write_handle = Some(handle);
        let paths = [
            app.tmp.path().join("one.jpg"),
            app.tmp.path().join("two.jpg"),
        ];
        submit_controlled_user(&mut app, &paths);
        let first = driver.recv_job();
        let second = driver.recv_job();
        let agent_path = app.tmp.path().join("agent.jpg");
        submit_controlled_agent(&mut app, agent_path);
        let agent = driver.recv_job();
        let pending_tx = app.next_tag_tx_id();
        app.register_pending_tag_op(pending_tx, "unrelated pending".into(), 1);
        let notifications = app.feedback_toasts_for_test.len();
        driver.publish_result(controlled_result(first, Ok(TagAction::Added)));
        driver.finish_job();
        app.poll_tag_write_results();
        assert_eq!(app.tag_write_completion.added, 1);
        driver.publish_result(controlled_result(second, Ok(TagAction::Added)));
        driver.finish_job();
        app.poll_tag_write_results();
        assert_eq!(app.tag_write_completion.added, 2);
        assert_eq!(app.feedback_toasts_for_test.len(), notifications);
        assert_eq!(app.tag_toast_label.as_deref(), Some("#tag"));
        assert_eq!(app.meta_undo.undo_len(), 1);
        assert_eq!(app.pending_tag_undos.len(), 1);
        assert!(app.pending_tag_undos.contains_key(&pending_tx));
        driver.publish_result(controlled_result(agent, Ok(TagAction::Added)));
        app.poll_tag_write_results();
        assert_eq!(app.feedback_toasts_for_test.len(), notifications);
        assert_eq!(
            app.tag_write_completion.added, 2,
            "agent result is excluded"
        );
        driver.finish_job();
        app.poll_tag_write_results();
        assert_eq!(app.feedback_toasts_for_test.len(), notifications + 1);
        assert_eq!(
            app.fs_feedback_toast.as_ref().unwrap().0,
            "2 件に #tag を付与"
        );
        assert_eq!(
            app.fs_feedback_toast_surface,
            Some(crate::app::ActionSurface::MainWindow)
        );
        assert_eq!(app.pending_tag_undos.len(), 1);
        assert!(app.pending_tag_undos.contains_key(&pending_tx));
        assert_eq!(app.meta_undo.undo_len(), 1, "agent adoption adds no Undo");
        app.poll_tag_write_results();
        assert_eq!(app.feedback_toasts_for_test.len(), notifications + 1);
    }

    #[test]
    fn user_failure_survives_later_success_and_notifies_once_at_completion() {
        use crate::tag_write_worker::{TagAction, TagWriteHandle};
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let (handle, driver) = TagWriteHandle::controlled_for_test();
        app.tag_write_handle = Some(handle);
        let paths = [
            app.tmp.path().join("failed.jpg"),
            app.tmp.path().join("ok.jpg"),
        ];
        submit_controlled_user(&mut app, &paths);
        let failed = driver.recv_job();
        let success = driver.recv_job();
        let notifications = app.feedback_toasts_for_test.len();
        driver.publish_result(controlled_result(failed, Err("write rejected")));
        driver.finish_job();
        app.poll_tag_write_results();
        assert_eq!(app.tag_write_completion.errors.len(), 1);
        assert_eq!(app.feedback_toasts_for_test.len(), notifications);
        driver.publish_result(controlled_result(success, Ok(TagAction::Added)));
        driver.finish_job();
        app.poll_tag_write_results();
        assert_eq!(app.feedback_toasts_for_test.len(), notifications + 1);
        assert_eq!(
            app.feedback_toasts_for_test.last().unwrap().1,
            crate::feedback::FeedbackKind::Warning
        );
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .unwrap()
                .0
                .contains("タグ書き込み失敗 1 件")
        );
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .unwrap()
                .0
                .contains("write rejected")
        );
        assert_eq!(
            app.fs_feedback_toast_surface,
            Some(crate::app::ActionSurface::MainWindow)
        );
        assert!(app.pending_tag_undos.is_empty());
        assert_eq!(app.meta_undo.undo_len(), 1);
        assert!(app.tag_write_completion.errors.is_empty());
        assert!(app.tag_toast_label.is_none());
        app.poll_tag_write_results();
        assert_eq!(app.feedback_toasts_for_test.len(), notifications + 1);
    }

    #[test]
    fn agent_tag_submission_preserves_user_boundary_and_records_real_writer_values() {
        use crate::agent_ipc::changes::{ChangeController, ChangeRow};
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let path = app.tmp.path().join("agent.jpg");
        let mut db = crate::tags_db::TagsDb::open().unwrap();
        let key = crate::tags_db::item_key_for_path(&path);
        db.set_item_tags(&key, ["actual"], crate::tags_db::source::EDIT)
            .unwrap();
        app.set_tags_cache_entry(key.clone(), vec!["#stale".into()]);
        app.adjustment_dragging = true;
        app.tag_toast_label = Some("user-label".into());
        app.tag_toast_surface = Some(crate::app::ActionSurface::MainWindow);
        let user_tx = app.next_tag_tx_id();
        app.register_pending_tag_op(user_tx, "user pending".into(), 1);
        let changes = ChangeController::new();
        let append_revision = app.book_append_undo_revision;
        assert_eq!(
            app.request_agent_tag_set_for_targets(
                vec![(
                    0,
                    super::TagTarget {
                        path: path.clone(),
                        tag_sidecar: None
                    }
                )],
                &["one".into(), "two".into()],
                true,
                "client".into(),
                vec![ChangeRow {
                    item: path.display().to_string(),
                    result: crate::agent_ipc::changes::ChangeRowResult::Pending,
                    ..Default::default()
                }],
                changes.clone(),
            ),
            1
        );
        assert!(
            app.adjustment_dragging,
            "agent must not finalize a user's edit session"
        );
        assert_eq!(app.tag_toast_label.as_deref(), Some("user-label"));
        assert_eq!(
            app.tag_toast_surface,
            Some(crate::app::ActionSurface::MainWindow)
        );
        assert_eq!(app.pending_tag_undos.len(), 1);
        assert_eq!(app.meta_undo.undo_len(), 0);
        assert_eq!(
            app.book_append_undo_revision, append_revision,
            "acceptance must not invalidate append Undo"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.poll_tag_write_results();
            if !app.tag_writer_has_unpublished_jobs() {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(app.adjustment_dragging);
        assert_eq!(app.tag_toast_label.as_deref(), Some("user-label"));
        assert_eq!(
            app.tag_toast_surface,
            Some(crate::app::ActionSurface::MainWindow)
        );
        assert!(app.pending_tag_undos.contains_key(&user_tx));
        assert_eq!(app.meta_undo.undo_len(), 0);
        assert!(
            app.book_append_undo_revision > append_revision,
            "an actual AI edit invalidates append Undo"
        );
        let records = changes.records();
        let records = records.lock().unwrap();
        let record = &records.entries()[0];
        assert_eq!(record.counts.changed, 1);
        assert_eq!(record.counts.queued, 0);
        assert_eq!(record.rows[0].before, "#actual");
        assert!(record.rows[0].after.contains("#actual"));
        assert!(record.rows[0].after.contains("#one"));
        assert!(record.rows[0].after.contains("#two"));
        assert!(!record.rows[0].after.contains("stale"));
        drop(records);
        let append_revision = app.book_append_undo_revision;
        app.request_agent_tag_set_for_targets(
            vec![(
                0,
                super::TagTarget {
                    path: path.clone(),
                    tag_sidecar: None,
                },
            )],
            &["one".into(), "two".into()],
            true,
            "client".into(),
            vec![ChangeRow {
                item: path.display().to_string(),
                result: crate::agent_ipc::changes::ChangeRowResult::Pending,
                ..Default::default()
            }],
            changes.clone(),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.poll_tag_write_results();
            if !app.tag_writer_has_unpublished_jobs() {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            app.book_append_undo_revision, append_revision,
            "no-op must preserve append Undo"
        );
        assert_eq!(
            changes.records().lock().unwrap().entries()[0]
                .counts
                .unchanged,
            1
        );
    }

    #[test]
    fn tag_maintenance_blocks_user_submission_before_any_edit_state_changes() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let (_sender, receiver) = std::sync::mpsc::channel();
        app.tag_maintenance_rx = Some(receiver);
        app.adjustment_dragging = true;
        let path = app.tmp.path().join("blocked.jpg");
        app.request_tag_add_for_targets(
            "tag",
            vec![super::TagTarget {
                path,
                tag_sidecar: None,
            }],
            crate::app::ActionSurface::MainWindow,
        );
        assert!(app.adjustment_dragging);
        assert!(app.pending_tag_undos.is_empty());
        assert!(app.tag_write_handle.is_none());
    }

    #[test]
    fn audio_is_a_tag_target() {
        use crate::grid_item::GridItem;
        use std::path::PathBuf;
        // 音声は実ファイルなのでタグ付与対象 (Video と同じ扱い、sidecar あり)。
        // 「タグ対象なし」トーストで付与できなかった退行の回帰テスト。
        let audio = GridItem::Audio(PathBuf::from(r"C:\music\a.mp3"));
        let target = super::tag_target_for_item(&audio, false);
        assert!(target.is_some(), "audio should be a tag target");
        assert_eq!(target.unwrap().path, PathBuf::from(r"C:\music\a.mp3"));
    }

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
    fn toast_tag_noop_does_not_say_clear() {
        assert_eq!(
            format_completion_toast(Some("#tag"), 0, 0, 0, 0, 2),
            "#tag: 2 件は変更なし"
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
}
