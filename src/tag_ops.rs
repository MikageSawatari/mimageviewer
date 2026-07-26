//! タグ付与/削除操作のファサード (docs/archive/search-metadata/tag-feature.md §5)。
//!
//! メニュー・ツールバー・メタデータパネルからのタグ操作のエントリーポイント。
//! tags.db 更新はすべて `tag_write_worker` に委譲する。UI 側は all-or-nothing 判定に
//! 必要な既存タグだけを tags.db から補完し、ファイルメタデータ I/O は行わない。

use std::path::PathBuf;

use crate::app::App;
use crate::grid_item::GridItem;
use crate::tag_legacy_xmp_worker::{LegacyXmpImportMode, LegacyXmpImportReport};
use crate::tag_write_worker::{
    TagAction, TagJobKind, TagSidecarTarget, TagWriteHandle, TagWriteJob,
};

#[derive(Clone)]
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

fn legacy_xmp_target_for_item(item: &GridItem) -> Option<PathBuf> {
    match item {
        GridItem::Image(p) | GridItem::Video(p) => Some(p.clone()),
        _ => None,
    }
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
    /// タグ操作 (`tag_targets`) と旧XMP取り込み (`legacy_xmp_targets`) の**両方がこの
    /// 1 実装を使う** — bulk_intent の stale-check が片方だけ改良されると、同じ選択でも
    /// 対象ファイル集合が割れ、破壊的な XMP 編集が想定外のファイルに当たる。
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

    fn legacy_xmp_targets(&self) -> Vec<PathBuf> {
        // 旧 XMP 取り込みはグリッドのメニューからのみ起動する (MainWindow 固定)。
        let mut out: Vec<PathBuf> = self
            .selection_target_indices(crate::app::ActionSurface::MainWindow)
            .into_iter()
            .filter(|&idx| !self.idx_is_compiled_book_page(idx))
            .filter_map(|idx| self.items.get(idx).and_then(legacy_xmp_target_for_item))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    pub(crate) fn legacy_xmp_target_path_count(&self) -> usize {
        self.legacy_xmp_targets().len()
    }

    pub(crate) fn request_legacy_xmp_import_for_selection(&mut self, mode: LegacyXmpImportMode) {
        self.request_legacy_xmp_import_for_paths(self.legacy_xmp_targets(), mode);
    }

    pub(crate) fn request_legacy_xmp_import_for_paths(
        &mut self,
        mut targets: Vec<PathBuf>,
        mode: LegacyXmpImportMode,
    ) {
        if let Some(pending) = self.tag_legacy_xmp_pending.as_ref() {
            // 実行中の再実行 = 中止要求。ImportAndRemove はファイルを書き換える
            // 破壊的バッチなので、必ずユーザーが止められる経路を持つ。
            pending.cancel();
            self.show_feedback_toast_on(
                "旧XMPタグの取り込みを中止します (処理済み分は反映されます)".to_string(),
                crate::app::ActionSurface::MainWindow,
            );
            return;
        }
        targets.retain(|path| {
            crate::xmp_writer::is_writable_format(path)
                || crate::xmp_writer::is_video_for_sidecar(path)
        });
        targets.sort();
        targets.dedup();
        if targets.is_empty() {
            self.show_feedback_toast_on(
                "旧XMPタグの取り込み対象がありません".to_string(),
                crate::app::ActionSurface::MainWindow,
            );
            return;
        }
        let count = targets.len();
        self.tag_legacy_xmp_pending = Some(crate::tag_legacy_xmp_worker::spawn(
            crate::data_dir::get(),
            targets,
            mode,
        ));
        self.show_feedback_toast_on(
            format!(
                "{} ({count} 件) — もう一度実行すると中止",
                mode.progress_label()
            ),
            crate::app::ActionSurface::MainWindow,
        );
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
    /// 合成ビュー (検索 / タグビュー / 読書履歴) は対象コンテナが無いので no-op + 通知。
    pub(crate) fn request_tag_toggle_for_current_container(&mut self, name: &str) {
        // ツールバーのピンタグ専用 (= グリッド面固定)。
        let surface = crate::app::ActionSurface::MainWindow;
        let Some(folder) = self.current_folder.clone() else {
            self.show_feedback_toast_on("タグを付けるコンテナがありません".to_string(), surface);
            return;
        };
        // 合成ビューでは実コンテナが無いので no-op + 通知。判定は経路ごとに異なる:
        //   - Ctrl+G グローバル検索: current_folder は検索前の実フォルダのまま + global_search.active
        //     (擬似パスにならないのでフラグで判定 — これを落として実害が出た)。
        //   - Ctrl+S お気に入り検索 / 読書履歴: current_folder が擬似パスになるのでパス一致で判定。
        //   - タグビュー / ドライブ一覧: 専用フラグ。
        if self.global_search.active
            || crate::app::is_synthetic_view_path(&folder)
            || self.items_are_tag_view
            || self.items_are_drive_list
        {
            self.show_feedback_toast_on(
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
        let name_owned = name.to_string();
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
        self.submit_tag_jobs(&targets, "toggle", tx_id, move |_| {
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
        if !self.precheck_tag_write_available(if add { "add" } else { "remove" }) {
            return;
        }
        self.hydrate_tags_cache_for_paths(&paths);
        let tag_key = crate::tags_db::normalize_tag_key(&name);
        if tag_key.is_empty() {
            return;
        }

        let with_hash = crate::tags_db::format_display_tag(&name);
        self.tag_toast_label = Some(with_hash.clone());
        self.set_tag_toast_surface_for_new_batch(surface);
        let summary = if add {
            format!("{with_hash} の付与")
        } else {
            format!("{with_hash} の削除")
        };
        self.optimistic_update_tags_cache(&paths, |before| {
            if add {
                let mut after = before.to_vec();
                if !after
                    .iter()
                    .any(|tag| crate::tags_db::normalize_tag_key(tag) == tag_key)
                {
                    after.push(with_hash.clone());
                }
                after
            } else {
                before
                    .iter()
                    .filter(|tag| crate::tags_db::normalize_tag_key(*tag) != tag_key)
                    .cloned()
                    .collect()
            }
        });
        let tx_id = self.next_tag_tx_id();
        self.register_pending_tag_op(tx_id, summary, paths.len());
        self.submit_tag_jobs(
            &targets,
            if add { "add" } else { "remove" },
            tx_id,
            move |_| {
                if add {
                    TagJobKind::Add(name.clone())
                } else {
                    TagJobKind::Remove(name.clone())
                }
            },
        );
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
        self.submit_tag_jobs(&targets, "clear", tx_id, |_| TagJobKind::ClearMiv);
    }

    /// 新しいタグバッチの発火面を単一スロット `tag_toast_surface` に記録する。
    /// 完了トーストはキュー全体で 1 本 (label と同じ設計) のため、in-flight バッチと
    /// 発火面が食い違う場合は面を確定できない — その場合は従来の全面表示 (None) に
    /// 落とし、混在バッチの完了が最後の面へ誤着弾しないようにする (Sol P2)。
    ///
    /// in-flight 判定は worker カウンタの未リセット (`has_unconsumed_batch`) を使う。
    /// `pending_tag_undos` は `clear_meta_undo` (フォルダ移動等) が worker 稼働中でも
    /// 意図的に破棄するため proxy にならず、`is_busy()` も worker が結果送信前に
    /// done を進めるため「完了したが未 poll」の 1 フレーム窓を取りこぼす
    /// (Sol P2 追指摘×2)。カウンタは完了 poll の `reset_counters_if_idle` まで
    /// 残るので、「前バッチが完了トーストとして未集計」を正しく表す。
    /// この呼び出しは新バッチの submit 前に行われる。
    fn set_tag_toast_surface_for_new_batch(&mut self, surface: crate::app::ActionSurface) {
        let in_flight = self
            .tag_write_handle
            .as_ref()
            .is_some_and(|handle| handle.has_unconsumed_batch());
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
        self.ensure_tag_write_handle();
        if self.tag_write_handle.is_none() {
            crate::logger::log(format!(
                "[TAG] '{op_label}' aborted: tag_write_handle unavailable"
            ));
            self.show_feedback_toast(TAG_WRITE_UNAVAILABLE_MSG.to_string());
            return false;
        }
        true
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
        kind_for: impl Fn(&PathBuf) -> TagJobKind,
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
            self.show_feedback_toast(TAG_WRITE_UNAVAILABLE_MSG.to_string());
            return;
        };
        crate::logger::log(format!(
            "[TAG] submitting '{op_label}' (tx={tx_id}) for {} item(s):",
            targets.len()
        ));
        for target in targets {
            let p = &target.path;
            crate::logger::log(format!("[TAG]   → {}", p.display()));
            h.submit(TagWriteJob {
                path: p.clone(),
                tag_sidecar: target.tag_sidecar.clone(),
                kind: kind_for(p),
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

impl App {
    /// 毎フレーム呼ぶ: tag_write_worker の結果をドレインしてトーストする。
    /// 成功した各 path については worker が書いた mIV タグ一覧をそのまま `tags_cache` に
    /// 反映する。
    pub(crate) fn poll_tag_write_results(&mut self) {
        let mut errors: Vec<(PathBuf, String)> = Vec::new();
        let mut added = 0usize;
        let mut removed = 0usize;
        let mut cleared = 0usize;
        let mut restored = 0usize;
        let mut noop = 0usize;
        let mut just_completed = false;
        // worker が返してきた (path, 書き込み後タグ列) を後でまとめて tags_cache に反映する。
        // 次フレームの `cell_tag_list` が正しい値を拾えるため add/remove 対称になる。
        let mut cache_updates: Vec<(PathBuf, Vec<String>)> = Vec::new();
        let mut sidecar_updates: Vec<(TagSidecarTarget, Vec<String>)> = Vec::new();
        // pending_tag_undos に積み上げる: (tx_id, TagChange or failure marker)。
        // tx_id == 0 は「Undo 確定不要」(Undo/Redo 由来の SetTags 等) なのでスキップ。
        let mut pending_updates: Vec<PendingUpdate> = Vec::new();
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
                                    "[TAG]   ✓ restored tags (undo/redo) → {path_disp}"
                                ));
                            }
                            TagAction::NoOp => {
                                noop += 1;
                                crate::logger::log(format!(
                                    "[TAG]   = no-op (already in target state) → {path_disp}"
                                ));
                            }
                        }
                        if res.tx_id != 0 {
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
                        if let Some(target) = res.tag_sidecar.clone() {
                            sidecar_updates.push((target, res.tags_after.clone()));
                        }
                        cache_updates.push((res.path, res.tags_after));
                    }
                    Err(e) => {
                        crate::logger::log(format!("[TAG]   ✗ FAILED: {e} → {path_disp}"));
                        if res.tx_id != 0 {
                            pending_updates.push(PendingUpdate::Failure { tx_id: res.tx_id });
                        }
                        // 楽観更新済みの tags_cache を実 DB 状態 (= worker が読み取った
                        // tags_before) に巻き戻す。DB 書き込みは失敗しているので、
                        // グリッドのバッジは予測値で更新済みのため放置すると stale
                        // 表示になる (Codex P3 指摘)。
                        cache_updates.push((res.path.clone(), res.tags_before));
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
            let msg = format!("タグ書き込み失敗 {} 件: {}", errors.len(), preview);
            // 発火面が分かるバッチはその面に出す (undo 復元など面未記録は従来どおり全面)。
            match self.tag_toast_surface {
                Some(surface) => self.show_feedback_toast_on(msg, surface),
                None => self.show_feedback_toast(msg),
            }
        } else if just_completed && (added + removed + cleared + restored + noop) > 0 {
            let label = self.tag_toast_label.take();
            let surface = self.tag_toast_surface.take();
            let msg =
                format_completion_toast(label.as_deref(), added, removed, cleared, restored, noop);
            match surface {
                Some(surface) => self.show_feedback_toast_on(msg, surface),
                None => self.show_feedback_toast(msg),
            }
        }
        if just_completed {
            // バッチ完了時は、完了トーストに使わなかった場合 (エラー完了等) でも
            // routing 状態を必ず破棄する。残すと次バッチの完了トーストが古い
            // ラベル/発火面で表示される (Codex P3)。成功分岐の take() 後は no-op。
            self.tag_toast_label = None;
            self.tag_toast_surface = None;
            if let Some(h) = self.tag_write_handle.as_ref() {
                h.reset_counters_if_idle();
            }
        }
    }
}

impl App {
    pub(crate) fn poll_legacy_xmp_import_results(&mut self) {
        let received = match self.tag_legacy_xmp_pending.as_ref() {
            Some(pending) => match pending.rx.try_recv() {
                Ok(result) => Some((pending.mode, result)),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some((
                    pending.mode,
                    Err("旧XMPタグの取り込み処理が終了しました".to_string()),
                )),
            },
            None => None,
        };
        let Some((mode, result)) = received else {
            return;
        };
        self.tag_legacy_xmp_pending = None;

        match result {
            Ok(result) => {
                let report = result.report.clone();
                let mut changed = false;
                for (path, tags) in result.cache_updates {
                    let key = crate::tags_db::item_key_for_path(&path);
                    self.set_tags_cache_entry(key, tags.clone());
                    if let Some(target) =
                        crate::tag_write_worker::sidecar_target_for_real_file(&path)
                    {
                        self.mirror_tag_sidecar_update(&target, &tags);
                    }
                    changed = true;
                }
                if changed && self.settings.facet_filter.uses_tag_state() {
                    self.rebuild_visible_indices();
                }
                if changed && self.tag_view.active {
                    self.execute_tag_view();
                }
                if changed {
                    self.invalidate_tag_apply_suggestions();
                    self.schedule_current_smart_folder_metadata_refresh(
                        crate::app::smart_folder::SmartFolderMetadataDependency::Tags,
                    );
                }

                crate::logger::log(format!(
                    "[TAG] legacy XMP import complete: mode={mode:?} candidates={} read={} \
                     imported_items={} inserted_tags={} marked_empty={} cleaned_files={} \
                     deleted_video_sidecars={} read_errors={} db_errors={} write_errors={}",
                    report.candidate_items,
                    report.read_items,
                    report.imported_items,
                    report.inserted_tags,
                    report.marked_empty_items,
                    report.cleaned_files,
                    report.deleted_video_sidecars,
                    report.read_errors,
                    report.db_errors,
                    report.write_errors
                ));
                self.show_feedback_toast_on(
                    format_legacy_xmp_import_toast(mode, &report, &result.errors),
                    crate::app::ActionSurface::MainWindow,
                );
            }
            Err(e) => {
                self.show_feedback_toast_on(
                    format!("旧XMPタグの取り込みに失敗: {e}"),
                    crate::app::ActionSurface::MainWindow,
                );
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

fn format_legacy_xmp_import_toast(
    mode: LegacyXmpImportMode,
    report: &LegacyXmpImportReport,
    errors: &[(std::path::PathBuf, String)],
) -> String {
    let mut msg = if mode.removes_from_file() {
        if report.cleaned_files > 0 {
            let sidecar = if report.deleted_video_sidecars > 0 {
                format!(" / 動画sidecar {} 件削除", report.deleted_video_sidecars)
            } else {
                String::new()
            };
            format!(
                "旧XMPタグを取り込み、{} 件のファイルから削除しました{sidecar}",
                report.cleaned_files
            )
        } else if report.imported_items > 0 {
            format!(
                "旧XMPタグを取り込みました: {} 件 / 新規 {} 個",
                report.imported_items, report.inserted_tags
            )
        } else {
            "旧XMPタグは見つかりませんでした".to_string()
        }
    } else if report.imported_items > 0 {
        format!(
            "旧XMPタグを取り込みました: {} 件 / 新規 {} 個",
            report.imported_items, report.inserted_tags
        )
    } else {
        "旧XMPタグは見つかりませんでした".to_string()
    };
    if report.cancelled {
        msg = format!("中止しました — {msg}");
    }
    if !errors.is_empty() {
        // どのファイルが失敗したか分からないと、ImportAndRemove では「# タグが
        // 残ったままのファイル」を特定できない (v1.0 のタグ書き込み失敗トーストと
        // 同様にファイル名を上位 3 件まで併記、全件は mimageviewer.log)。
        let preview = errors
            .iter()
            .take(3)
            .map(|(path, e)| {
                format!(
                    "{}: {}",
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string()),
                    e
                )
            })
            .collect::<Vec<_>>()
            .join(" / ");
        msg.push_str(&format!(" / 失敗 {} 件: {}", errors.len(), preview));
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::{format_completion_toast, format_legacy_xmp_import_toast};
    use crate::tag_legacy_xmp_worker::{LegacyXmpImportMode, LegacyXmpImportReport};

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

    #[test]
    fn legacy_xmp_toast_mentions_cleanup_and_errors() {
        let report = LegacyXmpImportReport {
            imported_items: 3,
            inserted_tags: 5,
            cleaned_files: 2,
            deleted_video_sidecars: 1,
            ..LegacyXmpImportReport::default()
        };
        let errors = vec![(
            std::path::PathBuf::from("C:/pics/locked.jpg"),
            "ファイル更新失敗: アクセス拒否".to_string(),
        )];
        assert_eq!(
            format_legacy_xmp_import_toast(LegacyXmpImportMode::ImportAndRemove, &report, &errors),
            "旧XMPタグを取り込み、2 件のファイルから削除しました / 動画sidecar 1 件削除 \
             / 失敗 1 件: locked.jpg: ファイル更新失敗: アクセス拒否"
        );
    }

    /// 中止時はその旨を明示し、処理済み分の集計も出す。
    #[test]
    fn legacy_xmp_toast_mentions_cancellation() {
        let report = LegacyXmpImportReport {
            imported_items: 2,
            inserted_tags: 3,
            cancelled: true,
            ..LegacyXmpImportReport::default()
        };
        assert_eq!(
            format_legacy_xmp_import_toast(LegacyXmpImportMode::ImportOnly, &report, &[]),
            "中止しました — 旧XMPタグを取り込みました: 2 件 / 新規 3 個"
        );
    }
}
