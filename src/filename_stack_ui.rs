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

    /// スタックモードが ON か (集約 or メンバーグリッド)。トグルボタンの状態表示に使う。
    pub(crate) fn stack_mode_on(&self) -> bool {
        self.stack_mode_requested
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
        self.stack_return_select_key = None;
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

    /// 集約セルのスタックを展開してメンバーグリッドへドリルインする。
    /// `key` は `GridItem::Stack.key`。単独セル (= Image/Video) はここを通らない。
    ///
    /// in-memory な items 差し替えなので、`zip_nav_show_current_level` と同じ軽量ビュー切替の
    /// 後始末 (idx 状態 + キュー破棄 / visible_indices 再構築 / ページ編集状態の再 hydrate /
    /// rating・tag prewarm) を必ず行う。これを怠ると旧 (集約) ビューの stale な
    /// `visible_indices` が `update_keep_range_and_requests` で `thumbnails[i]` を範囲外参照
    /// して panic する (Codex P1)。
    pub(crate) fn stack_drill_into(&mut self, key: &str) {
        let (g, items, metas) = {
            let Some(sv) = self.stack_view.as_ref() else {
                return;
            };
            let Some(g) = sv.group_index_by_key(key) else {
                return;
            };
            // 集約セルが Stack なのは is_stack グループだけだが、防御的に確認する。
            if !sv.groups[g].is_stack() {
                return;
            }
            let (items, metas) = sv.materialize_member(g);
            (g, items, metas)
        };
        let Some(folder) = self.current_folder.clone() else {
            return;
        };

        // 旧 (集約) ビューの in-flight 検索 / 詳細メタ pending を停止 (idx が付け替わる)。
        if let Some(pending) = self.search_pending.take() {
            pending.cancel();
        }
        if let Some(pending) = self.metadata_pending.take() {
            pending.cancel();
        }
        if let Some(sv) = self.stack_view.as_mut() {
            sv.drilled = Some(g);
        }
        self.stack_return_select_key = Some(key.to_string());

        // items / image_metas / thumbnails 差し替え + items_generation bump。
        self.install_new_items(items, metas);
        // idx ベース状態 + in-flight キューを破棄 (replace_search_view_items / zip nav と同じ)。
        self.invalidate_idx_state_and_queues();
        self.current_folder_rating_cache = None;
        // メンバーは実 Image ファイル。実フォルダ prefix でページ編集状態 (補正 / crop /
        // view-trim / 消しゴム / ローカル調整 / 隠蔽) を再 hydrate し、旧 idx の編集が別 entry へ
        // 漏れないようにする (page_path_key は実パスキーを返すので folder prefix で正しく載る)。
        self.rehydrate_page_edit_state_for_current_items(&folder);
        self.local_adjust_generation.clear();
        self.local_adjust_cache.clear();
        // path-keyed メタキャッシュはメンバー用にリセット。
        self.metadata_cache.clear();
        self.exif_cache.clear();
        self.xmp_cache.clear();
        self.tags_cache.clear();
        // Ctrl+F フィルタの残留を解除 (集約ビュー向けなのでメンバーグリッドでは無効)。
        self.search_filter = None;
        self.search_query.clear();
        // 先頭メンバーを選択 (メンバーは常に 2 枚以上)。
        self.selected = Some(0);
        self.scroll_offset_y = 0.0;
        self.scroll_to_selected = true;
        self.scroll_hint
            .store(0, std::sync::atomic::Ordering::Relaxed);
        // rating フィルタ有効時に rebuild_visible_indices が per-item で SQLite を引かないよう
        // 事前 prewarm (start_loading_items / zip nav と同じ)。
        self.prewarm_rating_cache();
        // ★ visible_indices 再構築。stale index による範囲外参照 panic を防ぐ (Codex P1)。
        self.rebuild_visible_indices();
        self.prewarm_grid_tags();
        self.update_stack_address();
    }

    /// メンバーグリッドから集約ビューへ戻る (Backspace)。集約ビューに居る / スタックモードで
    /// ない場合は `false` を返す (= 呼び出し側が通常の親フォルダ遷移に進む)。
    pub(crate) fn stack_drill_back(&mut self) -> bool {
        let Some(sv) = self.stack_view.as_ref() else {
            return false;
        };
        if sv.drilled.is_none() {
            return false;
        }
        let folder = sv.folder.clone();
        // 集約ビューは動画サムネ再生成のため同一フォルダ再読込で作り直す。
        // stack_mode_requested は維持されているので hook が集約を再構築する。
        self.load_folder(folder);
        // 戻り先のスタックセルを再選択して視認性を保つ (ZIP ツリーの BS と同様)。
        if let Some(key) = self.stack_return_select_key.take() {
            self.select_stack_cell_by_key(&key);
        }
        true
    }

    /// 集約ビューで `key` を持つ `GridItem::Stack` セルを選択してスクロールする。
    fn select_stack_cell_by_key(&mut self, key: &str) {
        let idx = self
            .items
            .iter()
            .position(|it| matches!(it, GridItem::Stack { key: k, .. } if k == key));
        if let Some(idx) = idx {
            self.selected = Some(idx);
            self.scroll_to_selected = true;
        }
    }

    /// アドレス欄をスタックのパンくず表示にする (メンバーグリッド時のみ)。
    /// 集約ビューは通常のフォルダパス表示のまま (start_loading_items が設定済み)。
    pub(crate) fn update_stack_address(&mut self) {
        let Some(sv) = self.stack_view.as_ref() else {
            return;
        };
        let Some(g) = sv.drilled else {
            return;
        };
        let Some(group) = sv.groups.get(g) else {
            return;
        };
        let folder = sv.folder.display().to_string();
        self.address = format!("{folder} > {}", group.key);
    }
}
