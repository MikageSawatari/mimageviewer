//! ★固定 (Snapshot Lock) 機能の App メソッド群。
//!
//! 型定義・正規化純関数は [`crate::snapshot`] にあり、ここは App との結合
//! (= state 切替・filter 比較・owner lookup) を担当する。
//!
//! 設計: [docs/star-lock-snapshot-design.md](../../docs/star-lock-snapshot-design.md)

use std::path::Path;

use crate::app::App;
use crate::snapshot::{
    FilterState, SnapshotEntry, SnapshotKey, SnapshotSourceLabel, SnapshotState, is_inside_fs,
    snapshot_entry_from_grid_item, snapshot_key_from_grid_item, snapshot_key_from_path,
};

impl App {
    /// snapshot active 中か。
    pub(crate) fn is_snapshot_active(&self) -> bool {
        self.snapshot.is_some()
    }

    /// snapshot 中の件数。inactive なら None。
    pub(crate) fn snapshot_count(&self) -> Option<usize> {
        self.snapshot.as_ref().map(|s| s.items.len())
    }

    /// snapshot の起点フォルダ path。inactive なら None。
    pub(crate) fn snapshot_origin(&self) -> Option<&Path> {
        self.snapshot.as_ref().map(|s| s.origin.as_path())
    }

    /// 現在の ★レベル filter を `FilterState` として取得。
    pub(crate) fn current_filter_state(&self) -> FilterState {
        FilterState::from_rating(self.settings.rating_filter)
    }

    /// snapshot 中、capture 時から ★ filter が変わったか。
    /// inactive なら false。
    pub(crate) fn snapshot_filter_changed_since_capture(&self) -> bool {
        let Some(snap) = self.snapshot.as_ref() else {
            return false;
        };
        snap.filter_at_capture != self.current_filter_state()
    }

    /// `[★固定]` ボタンが disabled になる理由 (= tooltip 表示用)。
    /// None なら enabled。snapshot active 中も enabled (= 解除ボタンになる)。
    ///
    /// disabled になる条件 (= §4.5):
    /// - Ctrl+G ストリーミング中 (= `global_search.is_searching()`)
    /// - Ctrl+G aggregated view 表示中 (= drill-down 前で SearchContainer 一覧)
    /// - Ctrl+F の pending 入力中 (= dirty で確定前)
    /// - Ctrl+S の pending 入力中
    pub(crate) fn snapshot_button_disabled_reason(&self) -> Option<&'static str> {
        // snapshot 解除中は判定をしない (= 解除ボタン側として常に enabled)
        if self.is_snapshot_active() {
            return None;
        }
        // Ctrl+G ストリーミング中 (= worker が active)
        if self.global_search.pending.is_some() && !self.global_search.done {
            return Some("Ctrl+G の結果取得中は使用不可 (取得完了後にお試しください)");
        }
        // Ctrl+G aggregated view (= drill-down 前のコンテナ一覧)
        if self.global_search.active
            && self.global_search.drill.is_none()
            && self.global_search.aggregate
        {
            return Some("集約表示中はスナップショットできません (コンテナを開いてください)");
        }
        // Ctrl+G dirty (= query が last_executed と異なる、入力中で結果未確定)
        if self.global_search.active
            && !self.global_search.query.is_empty()
            && self.global_search.query != self.global_search.last_executed
        {
            return Some("検索結果の確定後にお試しください (Ctrl+G)");
        }
        // Ctrl+F (= text search) の pending: search_pending が None になるまで待つ
        if self.search_pending.is_some() {
            return Some("検索結果の確定後にお試しください (Ctrl+F)");
        }
        // Ctrl+S (= favsearch) の dirty / pending
        if self.favsearch_pending.is_some() {
            return Some("検索結果の確定後にお試しください (Ctrl+S)");
        }
        if self.favsearch.active && self.favsearch.query != self.favsearch.last_executed {
            return Some("検索結果の確定後にお試しください (Ctrl+S)");
        }
        None
    }

    /// snapshot ON/OFF を切り替える。
    ///
    /// active なら deactivate、inactive なら `source_label` 引数で activate。
    pub(crate) fn toggle_snapshot(&mut self, source_label: SnapshotSourceLabel) {
        if self.is_snapshot_active() {
            self.deactivate_snapshot();
        } else {
            self.activate_snapshot(source_label);
        }
    }

    /// 現在の絞り込み状態から `SnapshotSourceLabel` を推定する。
    ///
    /// 優先順位 (= active な検索が最も具体的、なければ ★ filter、それも無ければ Mixed):
    /// 1. Ctrl+G active → GlobalSearch
    /// 2. Ctrl+S active → FavSearch
    /// 3. Ctrl+F (show_search_bar) → TextSearch
    /// 4. ★ filter で 1 つでも OFF (= 絞り込みあり) → RatingFilter
    /// 5. それ以外 → Mixed
    pub(crate) fn infer_snapshot_source_label(&self) -> SnapshotSourceLabel {
        if self.global_search.active && !self.global_search.last_executed.is_empty() {
            return SnapshotSourceLabel::GlobalSearch {
                query: self.global_search.last_executed.clone(),
            };
        }
        if self.favsearch.active && !self.favsearch.last_executed.is_empty() {
            return SnapshotSourceLabel::FavSearch {
                query: self.favsearch.last_executed.clone(),
            };
        }
        if self.show_search_bar && !self.search_query.is_empty() {
            return SnapshotSourceLabel::TextSearch {
                query: self.search_query.clone(),
            };
        }
        // ★ filter で 1 つでも OFF なら絞り込みあり
        let rating = self.settings.rating_filter;
        let any_off = rating.iter().any(|&b| !b);
        if any_off {
            let levels: Vec<u8> = rating
                .iter()
                .enumerate()
                .filter_map(|(i, &b)| if b { Some(i as u8) } else { None })
                .collect();
            return SnapshotSourceLabel::RatingFilter {
                active_levels: levels,
            };
        }
        SnapshotSourceLabel::Mixed
    }

    /// snapshot 中の filter 変化検出を含むフォルダパス suffix を返す。
    ///
    /// inactive なら None。active なら `(スナップショット中 N件)` または
    /// `(スナップショット中 N件 / filter 変更後)` のような文字列を返す。
    pub(crate) fn snapshot_path_suffix(&self) -> Option<String> {
        let count = self.snapshot_count()?;
        if self.snapshot_filter_changed_since_capture() {
            Some(format!("(スナップショット中 {count}件 / filter 変更後)"))
        } else {
            Some(format!("(スナップショット中 {count}件)"))
        }
    }

    /// snapshot を activate する (= 現在の visible_indices を capture して固定)。
    ///
    /// 実装手順 (§4.5 mutual exclusion lifecycle):
    /// 1. **capture first**: 現在 visible_indices の path 一覧 + filter state を
    ///    local 変数に持つ (= state 上書きの前に確保)
    /// 2. **close search**: 検索系 mode を解除 (= Ctrl+F/S/G の query / active flag clear)
    /// 3. **activate snapshot**: `self.snapshot = Some(SnapshotState { ... })`、
    ///    items / thumbnails / visible_indices / scroll_offset / selected を退避して
    ///    snapshot subset で置き換え
    pub(crate) fn activate_snapshot(&mut self, source_label: SnapshotSourceLabel) {
        // Step 1: capture
        let captured_entries: Vec<SnapshotEntry> = self
            .visible_indices
            .iter()
            .filter_map(|&i| self.items.get(i))
            .filter_map(snapshot_entry_from_grid_item)
            .collect();
        if captured_entries.is_empty() {
            // 0 件の snapshot は意味がない (= scope なし)。toast で警告して早期 return。
            self.show_feedback_toast("固定する items がありません".into());
            return;
        }
        let captured_thumbnails: Vec<crate::grid_item::ThumbnailState> = self
            .visible_indices
            .iter()
            .filter_map(|&i| self.items.get(i))
            .zip(
                self.visible_indices
                    .iter()
                    .filter_map(|&i| self.thumbnails.get(i)),
            )
            .filter_map(|(item, thumb)| {
                // snapshot 対象外 (ZipSeparator / SearchContainer) は entry が None になるので
                // entry と同じ filter で thumbnails 側も削る
                if snapshot_key_from_grid_item(item).is_some() {
                    Some(thumb.clone())
                } else {
                    None
                }
            })
            .collect();
        debug_assert_eq!(captured_entries.len(), captured_thumbnails.len());
        let captured_items_grid: Vec<crate::grid_item::GridItem> = self
            .visible_indices
            .iter()
            .filter_map(|&i| self.items.get(i))
            .filter(|item| snapshot_key_from_grid_item(item).is_some())
            .cloned()
            .collect();
        debug_assert_eq!(captured_items_grid.len(), captured_entries.len());

        let filter_at_capture = self.current_filter_state();
        let origin = self
            .current_folder
            .clone()
            .unwrap_or_else(std::path::PathBuf::new);
        let mut membership: std::collections::HashMap<SnapshotKey, usize> =
            std::collections::HashMap::with_capacity(captured_entries.len());
        for (idx, entry) in captured_entries.iter().enumerate() {
            // 同一 key (= 同 path) が複数あれば後勝ち (= 通常 visible_indices 内では重複しない)
            membership.insert(entry.key.clone(), idx);
        }

        // Step 2: close search (= mutual exclusion)
        let search_was_active = self.close_searches_for_snapshot();

        // Step 3: activate
        let generation_id = self.snapshot_next_generation_id;
        self.snapshot_next_generation_id = self.snapshot_next_generation_id.saturating_add(1);

        let n = captured_entries.len();
        let snapshot_visible_indices: Vec<usize> = (0..n).collect();
        let saved_items = std::mem::replace(&mut self.items, captured_items_grid);
        let saved_thumbnails = std::mem::replace(&mut self.thumbnails, captured_thumbnails);
        let saved_visible_indices =
            std::mem::replace(&mut self.visible_indices, snapshot_visible_indices);
        let saved_scroll_offset_y = std::mem::replace(&mut self.scroll_offset_y, 0.0);
        let saved_selected = std::mem::replace(&mut self.selected, None);

        self.snapshot = Some(SnapshotState {
            items: captured_entries,
            membership,
            origin,
            filter_at_capture,
            source_label,
            generation_id,
            saved_items,
            saved_thumbnails,
            saved_visible_indices,
            saved_scroll_offset_y,
            saved_selected,
        });

        let msg = if search_was_active {
            format!("検索結果をスナップショットに固定しました ({n} 件)")
        } else {
            format!("スナップショットに固定しました ({n} 件)")
        };
        self.show_feedback_toast(msg);
    }

    /// snapshot を deactivate する (= 退避していた items 等を復元)。
    ///
    /// 検索 state は **consume 済み** で復元しない (= §4.5 mutual exclusion の対称性)。
    pub(crate) fn deactivate_snapshot(&mut self) {
        let Some(snap) = self.snapshot.take() else {
            return;
        };
        self.items = snap.saved_items;
        self.thumbnails = snap.saved_thumbnails;
        self.visible_indices = snap.saved_visible_indices;
        self.scroll_offset_y = snap.saved_scroll_offset_y;
        self.selected = snap.saved_selected;
        self.show_feedback_toast("★固定を解除しました".into());
    }

    /// 検索系 (Ctrl+F / Ctrl+S / Ctrl+G) を全部 close する (= snapshot ON 時に呼ぶ)。
    ///
    /// 戻り値: いずれかの検索が active だったか (= toast 文面の分岐用)。
    fn close_searches_for_snapshot(&mut self) -> bool {
        let mut had_active = false;

        // Ctrl+F (= text search): search bar を閉じる + pending cancel + query クリア
        if self.show_search_bar {
            had_active = true;
            self.show_search_bar = false;
            self.search_query.clear();
            self.cancel_search_pending();
        }

        // Ctrl+S (= favsearch): active false + query/last_executed クリア
        if self.favsearch.active {
            had_active = true;
            self.favsearch.active = false;
            self.favsearch.query.clear();
            self.favsearch.last_executed.clear();
            // FavSearchPending は take すれば drop で rx 閉じる (= cancel field は private、
            // 既存パターンも take で済ませている)
            self.favsearch_pending = None;
        }

        // Ctrl+G (= global search): active false + query/results クリア
        if self.global_search.active {
            had_active = true;
            self.global_search.active = false;
            self.global_search.query.clear();
            self.global_search.last_executed.clear();
            self.global_search.containers.clear();
            self.global_search.all_hits.clear();
            self.global_search.drill = None;
            self.global_search.done = false;
            // SearchHandle は Drop impl で cancel.store する (= take で OK)
            self.global_search.pending = None;
        }

        had_active
    }

    /// snapshot 範囲内のどの entry が `path` を own するか。
    ///
    /// - 完全一致 (= image/video/zipimage/pdfpage entry を直接 fullscreen で開いた場合) → O(1)
    /// - prefix 一致 (= Folder/Zip/Pdf entry の中の image を fullscreen で開いた場合) → linear scan
    /// - 範囲外 → None
    ///
    /// 設計: docs/star-lock-snapshot-design.md §4.6 owner-entry lookup (P1-1)
    pub(crate) fn snapshot_owner_entry(&self, path: &Path) -> Option<usize> {
        let snap = self.snapshot.as_ref()?;
        let key = snapshot_key_from_path(path);
        // 1. 完全一致 (HashMap O(1))
        if let Some(&idx) = snap.membership.get(&key) {
            return Some(idx);
        }
        // 2. prefix 一致 (Folder / Zip / Pdf entry のみ対象)
        for (idx, entry) in snap.items.iter().enumerate() {
            if !entry.kind.is_container() {
                continue;
            }
            match &entry.key {
                SnapshotKey::Fs(fs_path) => {
                    if is_inside_fs(&key, fs_path) {
                        return Some(idx);
                    }
                }
                SnapshotKey::Archive { .. } => {
                    // Archive 型を container 扱いすることはないので skip
                    // (= ZipImage / PdfPage は always leaf)
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::grid_item::{GridItem, ThumbnailState};
    use crate::snapshot::SnapshotSourceLabel;
    use std::path::PathBuf;

    /// テスト用の App 構築。
    fn test_app_with_items(items: Vec<GridItem>) -> App {
        let mut app = App::default();
        app.items = items;
        app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
        app.visible_indices = (0..app.items.len()).collect();
        app.current_folder = Some(PathBuf::from(r"E:\test"));
        app
    }

    #[test]
    fn snapshot_inactive_by_default() {
        let app = App::default();
        assert!(!app.is_snapshot_active());
        assert_eq!(app.snapshot_count(), None);
        assert!(app.snapshot_origin().is_none());
    }

    #[test]
    fn activate_captures_visible_indices_paths() {
        let mut app = test_app_with_items(vec![
            GridItem::Folder(PathBuf::from(r"E:\a")),
            GridItem::Image(PathBuf::from(r"E:\b.png")),
        ]);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        assert!(app.is_snapshot_active());
        assert_eq!(app.snapshot_count(), Some(2));
    }

    #[test]
    fn activate_with_empty_visible_indices_is_noop() {
        let mut app = test_app_with_items(vec![]);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        assert!(!app.is_snapshot_active());
    }

    #[test]
    fn activate_skips_unrelated_grid_items() {
        // ZipSeparator は snapshot 対象外なので、items に混ざっていても snapshot からは除外される
        let mut app = test_app_with_items(vec![
            GridItem::Image(PathBuf::from(r"E:\a.png")),
            GridItem::ZipSeparator {
                dir_display: "Title".into(),
            },
            GridItem::Image(PathBuf::from(r"E:\b.png")),
        ]);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        // ZipSeparator は除外されて 2 件
        assert_eq!(app.snapshot_count(), Some(2));
    }

    #[test]
    fn deactivate_restores_saved_state() {
        let original_items = vec![
            GridItem::Folder(PathBuf::from(r"E:\original")),
            GridItem::Image(PathBuf::from(r"E:\img.png")),
        ];
        let mut app = test_app_with_items(original_items.clone());
        app.scroll_offset_y = 123.0;
        app.selected = Some(1);

        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        assert!(app.is_snapshot_active());
        // snapshot 中は scroll / selected がリセットされる
        assert_eq!(app.scroll_offset_y, 0.0);
        assert_eq!(app.selected, None);

        app.deactivate_snapshot();
        assert!(!app.is_snapshot_active());
        // 復元
        assert_eq!(app.items.len(), 2);
        assert_eq!(app.scroll_offset_y, 123.0);
        assert_eq!(app.selected, Some(1));
    }

    #[test]
    fn toggle_alternates_state() {
        let mut app = test_app_with_items(vec![GridItem::Image(PathBuf::from(r"E:\a.png"))]);
        app.toggle_snapshot(SnapshotSourceLabel::Mixed);
        assert!(app.is_snapshot_active());
        app.toggle_snapshot(SnapshotSourceLabel::Mixed);
        assert!(!app.is_snapshot_active());
    }

    #[test]
    fn generation_id_advances_per_activate() {
        let mut app = test_app_with_items(vec![GridItem::Image(PathBuf::from(r"E:\a.png"))]);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        let gen1 = app.snapshot.as_ref().unwrap().generation_id;
        app.deactivate_snapshot();
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        let gen2 = app.snapshot.as_ref().unwrap().generation_id;
        assert!(gen2 > gen1);
    }

    #[test]
    fn filter_change_detection() {
        let mut app = test_app_with_items(vec![GridItem::Image(PathBuf::from(r"E:\a.png"))]);
        app.settings.rating_filter = [true, false, false, false, false, true]; // ★5 + 未評価
        app.activate_snapshot(SnapshotSourceLabel::RatingFilter {
            active_levels: vec![5],
        });
        assert!(!app.snapshot_filter_changed_since_capture());

        // filter を変えた
        app.settings.rating_filter = [true; 6];
        assert!(app.snapshot_filter_changed_since_capture());
    }

    #[test]
    fn owner_entry_exact_match_for_image() {
        let mut app = test_app_with_items(vec![
            GridItem::Image(PathBuf::from(r"E:\a.png")),
            GridItem::Image(PathBuf::from(r"E:\b.png")),
        ]);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        assert_eq!(app.snapshot_owner_entry(Path::new(r"E:\a.png")), Some(0));
        assert_eq!(app.snapshot_owner_entry(Path::new(r"E:\b.png")), Some(1));
        // case-only 差 (Windows) でも owner 解決できる
        assert_eq!(app.snapshot_owner_entry(Path::new(r"e:\A.PNG")), Some(0));
    }

    #[test]
    fn owner_entry_prefix_match_for_folder_inner_image() {
        let mut app = test_app_with_items(vec![
            GridItem::Folder(PathBuf::from(r"E:\folderA")),
            GridItem::Folder(PathBuf::from(r"E:\folderB")),
        ]);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        // folderA 内の image は owner_entry 0
        assert_eq!(
            app.snapshot_owner_entry(Path::new(r"E:\folderA\sub\image.png")),
            Some(0)
        );
        // folderB 内
        assert_eq!(
            app.snapshot_owner_entry(Path::new(r"E:\folderB\other.png")),
            Some(1)
        );
        // 完全に範囲外
        assert_eq!(
            app.snapshot_owner_entry(Path::new(r"E:\folderC\image.png")),
            None
        );
    }

    #[test]
    fn owner_entry_sibling_false_positive_blocked() {
        // P1-1 重要観点: `E:\folder` は `E:\folderXX\image.png` を own しない
        let mut app = test_app_with_items(vec![GridItem::Folder(PathBuf::from(r"E:\folder"))]);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        assert_eq!(
            app.snapshot_owner_entry(Path::new(r"E:\folderXX\image.png")),
            None
        );
    }

    #[test]
    fn owner_entry_returns_none_when_inactive() {
        let app = App::default();
        assert_eq!(app.snapshot_owner_entry(Path::new(r"E:\a.png")), None);
    }

    #[test]
    fn button_disabled_when_inactive_and_clean() {
        // 何も検索していない通常状態では enabled
        let app = App::default();
        assert!(app.snapshot_button_disabled_reason().is_none());
    }

    #[test]
    fn infer_source_label_defaults_to_mixed_for_default_state() {
        let app = App::default();
        // Default state: rating_filter=[true; 6] (= 全部許可) なので Mixed
        let label = app.infer_snapshot_source_label();
        assert_eq!(label, SnapshotSourceLabel::Mixed);
    }

    #[test]
    fn infer_source_label_returns_rating_filter_when_filtering() {
        let mut app = App::default();
        // ★5 + 未評価 のみ
        app.settings.rating_filter = [true, false, false, false, false, true];
        let label = app.infer_snapshot_source_label();
        assert_eq!(
            label,
            SnapshotSourceLabel::RatingFilter {
                active_levels: vec![0, 5],
            }
        );
    }

    #[test]
    fn snapshot_path_suffix_includes_count_and_filter_change_marker() {
        let mut app = test_app_with_items(vec![GridItem::Image(PathBuf::from(r"E:\a.png"))]);
        assert!(app.snapshot_path_suffix().is_none());
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        let suffix = app.snapshot_path_suffix().unwrap();
        assert!(suffix.contains("スナップショット中"));
        assert!(suffix.contains("1件"));
        assert!(!suffix.contains("filter 変更後"));

        // filter を変えると marker が付く
        app.settings.rating_filter = [false; 6];
        let suffix2 = app.snapshot_path_suffix().unwrap();
        assert!(suffix2.contains("filter 変更後"));
    }
}
