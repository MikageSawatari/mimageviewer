//! ファイル名 prefix スタック (v2.0.0、`docs/filename-stack-plan.md`) の App 側グルー。
//!
//! 純グループ化ロジックは [`crate::filename_stack`]。ここはそれを `App` の状態
//! (`stack_view` / `stack_mode_requested`) とビュー (`self.items`) に橋渡しする。
//!
//! ビューは 3 段:
//! - **集約** (drilled=None): 1 グループ = 1 セル。複数枚画像はスタックセル + バッジ、単独は
//!   通常 Image/Video セル。コンテナ (フォルダ/ZIP/PDF) は先頭に素通し表示。
//! - **メンバーグリッド** (drilled=Some): スタックをクリックで展開。メンバーは実 Image セルで、
//!   通常のフルスクリーン読書・★・タグ・チェック・D&D がそのまま効く。
//! - **フルスクリーン**: メンバーグリッドから通常どおり開く (本フェーズでは特別な配線なし)。
//!
//! 構築は `load_folder_with_scan` の hook 経由 (集約ビューは start_loading_items が動画
//! サムネスレッドを起動するため必ずフォルダ読込を通す)。メンバーグリッドへのドリルは
//! in-memory (`install_new_items`)、集約への戻りは同一フォルダ再読込で行う。

use std::path::PathBuf;

use crate::filename_stack::{StackMember, StackView};
use crate::grid_item::GridItem;
use crate::settings::SortOrder;

/// 通常フォルダの items (コンテナ先頭 + メディア) を集約ビューへ変換し、`StackView` を作る。
///
/// `folder_count` = items 先頭のコンテナ (Folder/ZipFile/PdfFile/ConvertibleArchive) ブロック数。
/// それ以降は Image/Video のメディア。戻り値 = (集約 items, 集約 metas, 集約 video_items, StackView)。
pub(crate) fn build_stack_aggregated(
    folder: PathBuf,
    mut items: Vec<GridItem>,
    mut image_metas: Vec<Option<(i64, i64)>>,
    folder_count: usize,
    separator: char,
    sort: SortOrder,
) -> (
    Vec<GridItem>,
    Vec<Option<(i64, i64)>>,
    Vec<(usize, PathBuf, u64)>,
    StackView,
) {
    let folder_count = folder_count.min(items.len());
    let media_items = items.split_off(folder_count);
    let media_metas = image_metas.split_off(folder_count);
    // items / image_metas は passthrough (コンテナ) ブロックになった。
    let mut media: Vec<StackMember> = Vec::with_capacity(media_items.len());
    for (it, meta) in media_items.into_iter().zip(media_metas) {
        let (mtime, size) = meta.unwrap_or((0, 0));
        match it {
            GridItem::Image(path) => media.push(StackMember {
                path,
                mtime,
                size,
                is_video: false,
            }),
            GridItem::Video(path) => media.push(StackMember {
                path,
                mtime,
                size,
                is_video: true,
            }),
            // 防御的: 通常フォルダのメディアブロックは Image/Video のみのはずだが、
            // 想定外の種別が来たら passthrough 末尾に素通しする (グループ化に混ぜない)。
            other => {
                items.push(other);
                image_metas.push(meta);
            }
        }
    }

    let sv = StackView::build(folder, items, image_metas, media, separator, sort);
    let (agg_items, agg_metas) = sv.materialize_aggregated();
    let video_items = stack_video_items(&agg_items, &agg_metas);
    (agg_items, agg_metas, video_items, sv)
}

/// 集約 / メンバービューの items から動画セルの `(idx, path, size)` を集める
/// (`start_loading_items` の video サムネスレッド用、元の媒体ループと同形式)。
pub(crate) fn stack_video_items(
    items: &[GridItem],
    metas: &[Option<(i64, i64)>],
) -> Vec<(usize, PathBuf, u64)> {
    items
        .iter()
        .enumerate()
        .filter_map(|(idx, it)| {
            if let GridItem::Video(p) = it {
                let size = metas
                    .get(idx)
                    .and_then(|m| *m)
                    .map(|(_, s)| s.max(0) as u64)
                    .unwrap_or(0);
                Some((idx, p.clone(), size))
            } else {
                None
            }
        })
        .collect()
}

impl crate::app::App {
    /// スタックモードのトグルが使える状況か (= 通常フォルダ表示)。
    /// ZIP ツリー / 検索 / タグ / 読書履歴 / ドライブ一覧などの特殊ビューでは無効。
    pub(crate) fn stack_mode_available(&self) -> bool {
        self.current_folder.is_some()
            && self.zip_nav.is_none()
            && !self.items_are_global_search_view
            && !self.items_are_tag_view
            && !self.items_are_reading_history_view
            && !self.items_are_drive_list
            && !self.global_search.active
            && !self.favsearch.active
            && !self.tag_view.active
    }

    /// スタックモードが ON か。トグルボタンの選択状態表示に使う。
    pub(crate) fn stack_mode_on(&self) -> bool {
        self.stack_mode_requested
    }

    /// 集約グリッドを表示中か (= スタックモード ON かつフラット読書フルスクリーン中でない)。
    /// グリッドのセルクリック → フラットフルスクリーンへ入れる状態かの判定に使う。
    pub(crate) fn stack_mode_aggregated(&self) -> bool {
        self.stack_view.is_some() && !self.stack_showing_flat
    }

    /// スタックモードを切り替える。同一フォルダを再読込して集約 / 通常を作り直す
    /// (folder_changes=false なので `stack_mode_requested` は維持される)。
    pub(crate) fn toggle_stack_mode(&mut self) {
        if !self.stack_mode_available() {
            self.show_feedback_toast("スタック表示は通常フォルダでのみ使えます".into());
            return;
        }
        let Some(folder) = self.current_folder.clone() else {
            return;
        };
        self.stack_mode_requested = !self.stack_mode_requested;
        self.load_folder(folder);
        if self.stack_mode_requested
            && self
                .stack_view
                .as_ref()
                .is_some_and(|sv| !sv.has_collapsible_stack())
        {
            self.show_feedback_toast(
                "まとめられるスタックがありません (同じ区切り文字の連番が必要です)".into(),
            );
        }
    }

    /// 集約グリッドでメディアセル (スタック / 単独画像 / 動画) を開いたとき、フラット読書
    /// フルスクリーンへ入る。`agg_idx` は集約 `self.items` の index。コンテナ (passthrough) は
    /// `false` を返して通常ナビ (フォルダ/ZIP/PDF を開く) に委ねる。戻り値 true = ここで処理した。
    ///
    /// `from_double_click` = ダブルクリック経由か (動画の場合に 2 打目の play/pause トグルを
    /// 抑制するため)。通常の grid→fullscreen 経路と同じ開幕ガードをここで張る。
    pub(crate) fn stack_try_open_from_grid(
        &mut self,
        agg_idx: usize,
        from_double_click: bool,
    ) -> bool {
        if !self.stack_mode_aggregated() {
            return false;
        }
        let flat_idx = self
            .stack_view
            .as_ref()
            .and_then(|sv| sv.flat_index_for_aggregated(agg_idx));
        let Some(flat_idx) = flat_idx else {
            // passthrough コンテナ → フルスクリーンでなく通常ナビへ。
            return false;
        };
        // 開幕ガード (通常の grid open 経路と同じ):
        // - Enter で開いた同フレームに fullscreen 側が同じ Enter を拾って即 close するのを防ぐ
        //   (Enter が押下されていなければ fullscreen 側初フレームで自動リセットされるので、
        //    click/gamepad 経由で立てても無害)。
        self.fs_suppress_enter_close_until_release = true;
        // - ダブルクリックで動画を開いたとき、2 打目クリックが fullscreen の動画 play/pause を
        //   トグルしないよう抑制する (静止画は open_fullscreen の focus-regain グレースで足りる)。
        if from_double_click && matches!(self.items.get(agg_idx), Some(GridItem::Video(_))) {
            self.fs_suppress_primary_until_release = true;
        }
        self.stack_enter_flat_fullscreen(flat_idx);
        true
    }

    /// フラット読書ビュー (全画像を展開した並び) へ `self.items` を差し替え、`flat_idx` を
    /// フルスクリーンで開く。
    ///
    /// in-memory な items 差し替えなので、`zip_nav_show_current_level` と同じ軽量ビュー切替の
    /// 後始末 (idx 状態 + キュー破棄 / visible_indices 再構築 / ページ編集状態の再 hydrate /
    /// rating・tag prewarm) を必ず行う。これを怠ると旧 (集約) ビューの stale な
    /// `visible_indices` が範囲外参照 panic を起こす (Codex P1)。
    fn stack_enter_flat_fullscreen(&mut self, flat_idx: usize) {
        let (items, metas) = match self.stack_view.as_ref() {
            Some(sv) => sv.materialize_flat(),
            None => return,
        };
        let Some(folder) = self.current_folder.clone() else {
            return;
        };
        self.swap_stack_view_items(items, metas, &folder, Some(flat_idx));
        self.stack_showing_flat = true;
        self.fs_open_intent_from_grid = true;
        self.open_fullscreen(flat_idx);
    }

    /// フルスクリーン中の `Shift+↓↑`: 次/前のスタックの先頭画像へジャンプする。
    /// フラット読書ビューでないときは `false` (= 呼び出し側が通常のページ送りに委ねる)。
    /// 端では stack ジャンプ可能位置が無いので `true` (消費) のまま no-op にする。
    pub(crate) fn stack_jump(&mut self, ctx: &egui::Context, forward: bool) -> bool {
        if !self.stack_showing_flat {
            return false;
        }
        let Some(cur) = self.fullscreen_idx else {
            return false;
        };
        let target = self
            .stack_view
            .as_ref()
            .and_then(|sv| sv.stack_jump_target(cur, forward));
        if let Some(t) = target {
            self.open_fullscreen_from_fs_navigation(ctx, t);
        }
        true
    }

    /// フラットフルスクリーンが閉じたら集約グリッドへ戻す (毎フレーム reconcile、
    /// `render_grid` の直前で呼ぶ)。スタックモードが解除済み (フォルダナビ等) なら何もしない。
    pub(crate) fn stack_reconcile_after_fullscreen_close(&mut self) {
        if !self.stack_showing_flat || self.fullscreen_idx.is_some() {
            return;
        }
        // フルスクリーンが閉じた → フラグを落とす。
        self.stack_showing_flat = false;
        // 集約を再構築するための材料を取り出す (借用は install 前に閉じる)。
        let Some((items, metas, folder, select_agg)) = ({
            let Some(sv) = self.stack_view.as_ref() else {
                // フォルダナビ等で stack_view が破棄済み → 通常フォルダが表示されている。何もしない。
                return;
            };
            // close_fullscreen が selected に復元した「最後に見ていた flat index」を集約セルへ写す。
            let select_agg = self
                .selected
                .and_then(|flat| sv.group_of_flat_index(flat))
                .map(|g| sv.aggregated_index_of_group(g));
            let (items, metas) = sv.materialize_aggregated();
            Some((items, metas, sv.folder.clone(), select_agg))
        }) else {
            return;
        };
        self.swap_stack_view_items(items, metas, &folder, select_agg);
    }

    /// 集約/フラット間の in-memory ビュー切替の共通後始末。`select` を選択し scroll する。
    fn swap_stack_view_items(
        &mut self,
        items: Vec<GridItem>,
        metas: Vec<Option<(i64, i64)>>,
        folder: &std::path::Path,
        select: Option<usize>,
    ) {
        // 旧ビューの in-flight 検索 / 詳細メタ pending を停止 (idx が付け替わる)。
        if let Some(pending) = self.search_pending.take() {
            pending.cancel();
        }
        if let Some(pending) = self.metadata_pending.take() {
            pending.cancel();
        }
        self.install_new_items(items, metas);
        self.invalidate_idx_state_and_queues();
        self.current_folder_rating_cache = None;
        // セルは実 Image (フラット) / 単独 Image (集約)。実フォルダ prefix でページ編集状態
        // (補正 / crop / view-trim / 消しゴム / ローカル調整 / 隠蔽) を再 hydrate する
        // (page_path_key が実パスキーを返すので folder prefix で正しく載る)。
        self.rehydrate_page_edit_state_for_current_items(folder);
        self.local_adjust_generation.clear();
        self.local_adjust_cache.clear();
        self.metadata_cache.clear();
        self.exif_cache.clear();
        self.xmp_cache.clear();
        self.tags_cache.clear();
        self.search_filter = None;
        self.search_query.clear();
        self.selected = select;
        self.scroll_offset_y = 0.0;
        self.scroll_to_selected = select.is_some();
        self.scroll_hint
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.prewarm_rating_cache();
        // ★ visible_indices 再構築。stale index による範囲外参照 panic を防ぐ (Codex P1)。
        self.rebuild_visible_indices();
        self.prewarm_grid_tags();
    }
}
