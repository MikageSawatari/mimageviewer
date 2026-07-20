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
    #[allow(dead_code)] // 将来の debug log / restore 用、SL-C3 では未使用
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
        // Step 1: capture (= visible_indices から SnapshotEntry を構築)。
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
                // snapshot 対象外 (SearchContainer / 仮想コンテナ) は entry が None になるので
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
        // 検索 view (Ctrl+G / Ctrl+S) から ★固定した場合の戻り先 folder を採用
        // (= deactivate 時に合成 path `__search_results__` から本物 folder に戻るため)。
        // close_searches_for_snapshot が走る前に capture する必要あり。
        let pre_snapshot_search_origin = if self.global_search.active {
            self.global_search.saved_folder.clone()
        } else if self.favsearch.active {
            self.favsearch.saved_folder.clone()
        } else {
            None
        };
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
        // 元の selected idx (= items 内のグローバル位置) を snapshot 内の対応 idx に
        // 変換 (= ユーザー要望: ボタンを押したときのカーソル位置を保持)。
        // 元 items[old_idx] の SnapshotKey → membership で snapshot 内 idx を引く。
        // snapshot 対象外 item や、そもそも未選択なら None。
        let new_selected = self.selected.and_then(|old_idx| {
            self.items
                .get(old_idx)
                .and_then(snapshot_key_from_grid_item)
                .and_then(|key| membership.get(&key).copied())
        });
        // list_view_items / list_view_thumbnails 用に clone を確保
        // (= BS で snapshot list 復帰した時に Pending リセットせず再利用するため)。
        let list_view_items = captured_items_grid.clone();
        let list_view_thumbnails = captured_thumbnails.clone();
        let saved_items = std::mem::replace(&mut self.items, captured_items_grid);
        let saved_thumbnails = std::mem::replace(&mut self.thumbnails, captured_thumbnails);
        let saved_visible_indices =
            std::mem::replace(&mut self.visible_indices, snapshot_visible_indices);
        let saved_scroll_offset_y = std::mem::replace(&mut self.scroll_offset_y, 0.0);
        let saved_selected = std::mem::replace(&mut self.selected, new_selected);
        // ネスト ZIP ツリーナビ状態は必ず take して退避する。snapshot は items を
        // start_loading_items を通さず差し替えるので、残したままだと snapshot 表示中の
        // BS が zip_nav_back() に落ちて stale な ZIP 階層を snapshot ビューへ上書き
        // materialize する (レビュー P2)。at_origin の deactivate で saved_items と
        // 対で復元する。
        let saved_zip_nav = self.zip_nav.take();
        // 新 selected がある場合は次フレームで画面内にスクロール (= 表示されたカーソルが
        // 画面外にあるとユーザー視認できないため)。new_selected が None なら scroll_offset_y
        // = 0.0 のままで先頭表示。
        if new_selected.is_some() {
            self.scroll_to_selected = true;
        }
        // ★情報 / 回転 / タグ / EXIF 等の idx-based cache を全 clear する。
        // items を入れ替えたので idx の意味が変わる (= 旧 idx → stars の対応が壊れ、
        // 「★バッジが全部消える」「★一時解除中が発動しない」原因になる)。
        // 既存の load_folder 経路 (= self.start_loading_items) と同様に invalidate する。
        self.rating_cache.clear();
        self.rotation_cache.clear();
        self.tags_cache.clear();
        self.mark_color_filter_scope_dirty();

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
            saved_zip_nav,
            list_view_items,
            list_view_thumbnails,
            pre_snapshot_search_origin,
        });
        // ★items_generation bump + invalidate_idx_state_and_queues (= Codex P1-1):
        // items を差し替えたので、旧 ThumbMsg / pending / keep_set / idx-keyed cache が
        // 新 idx に着地して「サムネが化ける/消える」事故を防ぐ。`invalidate_idx_state_and_queues`
        // は thumbnails 自体は触らないので、上で入れ替えた captured_thumbnails は維持される。
        self.items_generation = self.items_generation.wrapping_add(1);
        self.invalidate_idx_state_and_queues();
        // tags_cache は invalidate 対象外なので手動 clear (= 既存 clear は冗長になるが
        // 残しておく方が安全、二重 clear は no-op)
        self.tags_cache.clear();
        // ★items を subset へ差し替えたので、idx-keyed なページ編集状態 (補正 / ローカル
        // 調整 / crop / マスク / 隠蔽) を新 idx に合わせ直す (Codex P1)。これをやらないと、
        // 差し替え前 idx に紐付いた補正・マスクが subset の別 idx に乗って表示 / エクスポート /
        // 保存される。`invalidate_idx_state_and_queues` は idx-keyed cache を落とすがユーザー
        // 設定マップ (adjustment_page_params 等) は残すため、ここで明示的に処理する必要がある。
        //
        // cross-folder 検索 view 由来 snapshot (= Ctrl+S/Ctrl+G) は **clear のみ**:
        // - subset が cross-folder で単一 prefix hydrate できない (origin = 検索前の実
        //   current_folder なので、prefix 配下の subset item だけ部分的に hydrate されてしまう)。
        // - 検索 view は元々ページ編集 overlay を出さない設計 (replace_search_view_items が
        //   clear する) なので、その snapshot も overlay 無しで揃える。
        // 判定は `pre_snapshot_search_origin.is_some()` (= Ctrl+S/Ctrl+G でのみ Some)。
        // **`search_was_active` では判定しない**: あれは Ctrl+F (= 単一フォルダの構造フィルタ)
        // でも true になるが、Ctrl+F の subset は origin 配下に収まるので通常どおり rehydrate
        // すべき。`search_was_active` で gate すると Ctrl+F snapshot が誤って clear され、
        // しかも list 復帰 (pre_snapshot_search_origin で判定) との非対称を生む (Codex follow-up)。
        // 通常フォルダ / Ctrl+F filter 由来は origin の DB から subset idx で hydrate し直す。
        // deactivate / list 復帰でも同じ判定で対称に処理する。
        if let Some((origin, is_search_view)) = self
            .snapshot
            .as_ref()
            .map(|s| (s.origin.clone(), s.pre_snapshot_search_origin.is_some()))
        {
            if is_search_view {
                self.clear_page_edit_state();
            } else {
                self.rehydrate_page_edit_state_for_current_items(&origin);
            }
        }

        let msg = if search_was_active {
            format!("検索結果をスナップショットに固定しました ({n} 件)")
        } else {
            format!("スナップショットに固定しました ({n} 件)")
        };
        self.show_feedback_toast(msg);
    }

    /// Leave Snapshot Lock for another top-level view without rebuilding the captured source.
    ///
    /// A snapshot created from Ctrl+S / Ctrl+G owns two locations: `origin` is the result grid
    /// (or its drill target), while `pre_snapshot_search_origin` is the real view to which the
    /// user should return.  Callers that transition directly to another synthetic view must take
    /// the latter as return ownership instead of recording the result-grid path in history.
    pub(crate) fn dismiss_snapshot_without_restore(
        &mut self,
    ) -> Option<crate::app::ViewReturnContext> {
        let snap = self.snapshot.take()?;
        let _ = self.restore_rating_filter_suppression();
        let at_origin = self.current_folder.as_ref().is_some_and(|path| {
            crate::snapshot::snapshot_key_from_path(path)
                == crate::snapshot::snapshot_key_from_path(&snap.origin)
        });
        let path = if at_origin {
            snap.pre_snapshot_search_origin
                .clone()
                .or_else(|| Some(snap.origin.clone()))
        } else {
            self.current_folder.clone()
        };
        let subfolder_restore = if path.as_deref().is_some_and(|path| {
            crate::folder_tree::path_eq(path, &super::subfolder_expansion_synthetic_path())
        }) {
            match snap.source_label {
                SnapshotSourceLabel::FavSearch { .. } => self.favsearch_subfolder_restore.take(),
                SnapshotSourceLabel::GlobalSearch { .. } => {
                    self.global_search_subfolder_restore.take()
                }
                _ => None,
            }
            .or_else(|| self.take_subfolder_expansion_restore_for_synthetic_path(path.as_deref()))
        } else {
            None
        };
        self.show_feedback_toast("★固定を解除しました".into());
        Some(crate::app::ViewReturnContext {
            rating_view_stars: self.view_return_rating_view_stars_for_path(path.as_deref()),
            path,
            subfolder_restore,
        })
    }

    /// snapshot を deactivate する (= 退避していた items 等を復元)。
    ///
    /// 検索 state は **consume 済み** で復元しない (= §4.5 mutual exclusion の対称性)。
    ///
    /// Fix-D (Codex P1-3): user が snapshot 内 child folder の中に居る状態で解除した
    /// 場合、saved_items を強制復元すると address/grid 不整合になる。current_folder が
    /// origin と一致する場合のみ復元、不一致の場合は current_folder の通常 reload を
    /// 行う (= snapshot state を捨てるだけ)。
    pub(crate) fn deactivate_snapshot(&mut self) {
        let Some(snap) = self.snapshot.take() else {
            return;
        };
        // filter suppress も解除 (= snapshot 内 folder enter で発動していた可能性がある)
        let _ = self.restore_rating_filter_suppression();
        // current_folder が snapshot origin と一致するか
        let at_origin = self
            .current_folder
            .as_ref()
            .map(|p| {
                crate::snapshot::snapshot_key_from_path(p)
                    == crate::snapshot::snapshot_key_from_path(&snap.origin)
            })
            .unwrap_or(false);
        // ユーザー報告 fix (3 段階目): Ctrl+G/Ctrl+S 検索 view から ★固定した場合、
        // snapshot は検索 mode を consume したので解除しても自然な表示にならない (= items
        // だけ復元しても active=false で UI 不整合 / 「🌐 アイテム検索: ...」表示が残る)。
        // 検索 view 由来 snapshot + **まだ origin に居る** (= 検索 view の合成 path や
        // drilled container 内で解除) なら、検索開始前の現実 folder に load_folder で戻る。
        //
        // 段階履歴:
        // - c508ca25: origin == __search_results__ (= Ctrl+G flat view のみ) で synthetic
        //   判定 → drilled view (実 path) が漏れて 🌐 表示残るバグ
        // - 021c54fe: pre_snapshot_search_origin.is_some() のみで判定 → drilled view も
        //   救うが、captured child folder の中で解除しても検索前に飛ぶバグ (Codex P2)
        // - 本コミット: pre_snapshot_search_origin Some **かつ** at_origin の場合に限定。
        //   child folder の中で解除した場合は既存「解除直前のフォルダ維持」path に落ちる。
        if at_origin {
            if let Some(restore_to) = snap.pre_snapshot_search_origin.clone() {
                let subfolder_restore = match snap.source_label {
                    SnapshotSourceLabel::FavSearch { .. } => {
                        self.favsearch_subfolder_restore.take()
                    }
                    SnapshotSourceLabel::GlobalSearch { .. } => {
                        self.global_search_subfolder_restore.take()
                    }
                    _ => None,
                }
                .or_else(|| {
                    self.take_subfolder_expansion_restore_for_synthetic_path(Some(&restore_to))
                });
                // Use the same owner-aware restore boundary as search/smart-folder transitions.
                // Besides suppressing history, this restores synthetic subfolder/smart views
                // instead of handing their paths to the real-folder loader.
                self.restore_view_return_context(crate::app::ViewReturnContext {
                    rating_view_stars: self
                        .view_return_rating_view_stars_for_path(Some(&restore_to)),
                    path: Some(restore_to),
                    subfolder_restore,
                });
                self.show_feedback_toast("★固定を解除しました".into());
                return;
            }
        }
        if at_origin {
            // origin のまま解除 → 元の items 復元 (= snapshot 元のフォルダに戻る)。
            // snapshot 中に subset (= self.thumbnails) で thumbnail worker が
            // Pending → Loaded に進めたものを、path key 経由で saved 側に merge する。
            // これをしないと「snapshot 解除後に新生成サムネが消える」(ユーザー報告)。
            // GPU TextureHandle は内部 Arc なので clone は cheap。
            use crate::grid_item::ThumbnailState;
            let mut merged_thumbnails = snap.saved_thumbnails;
            for (subset_idx, subset_thumb) in self.thumbnails.iter().enumerate() {
                if !matches!(subset_thumb, ThumbnailState::Loaded { .. }) {
                    continue; // Pending/Failed/Evicted は merge 対象外
                }
                let Some(subset_item) = self.items.get(subset_idx) else {
                    continue;
                };
                let Some(subset_key) = crate::snapshot::snapshot_key_from_grid_item(subset_item)
                else {
                    continue;
                };
                // saved 内で同じ path key の item を探す (O(saved) linear、実用上問題なし)
                if let Some(saved_idx) = snap.saved_items.iter().position(|it| {
                    crate::snapshot::snapshot_key_from_grid_item(it).as_ref() == Some(&subset_key)
                }) {
                    if let Some(slot) = merged_thumbnails.get_mut(saved_idx) {
                        // 既に Loaded のスロットも上書き OK (= 新しい方が確実に新しい render)
                        *slot = subset_thumb.clone();
                    }
                }
            }
            self.items = snap.saved_items;
            self.thumbnails = merged_thumbnails;
            self.visible_indices = snap.saved_visible_indices;
            self.scroll_offset_y = snap.saved_scroll_offset_y;
            self.selected = snap.saved_selected;
            // ネスト ZIP 由来の snapshot: saved_items (= ZIP 階層の items) と対で
            // ナビ状態も復元する。これをしないと「ZIP items なのに nav なし」になり、
            // BS が階層を戻らず ZIP ごと抜ける / ZipDir セルの Enter が無反応になる。
            self.zip_nav = snap.saved_zip_nav;
            // items_generation bump + invalidate (= Codex P1-1)
            // items を saved に戻したので、snapshot 中に走った ThumbMsg / pending が
            // 新 idx 配置に着地して壊さないよう invalidate。merged_thumbnails は
            // invalidate 後も保持される (= thumbnails 自体は invalidate 対象外)。
            self.items_generation = self.items_generation.wrapping_add(1);
            self.invalidate_idx_state_and_queues();
            self.tags_cache.clear();
            // items を saved (元フォルダ) に戻したので、ページ編集状態も元 idx で hydrate
            // し直す (= activate の subset hydrate と対称、Codex P1)。snapshot 中に編集した分は
            // set_page_params 等が DB に同期保存済みなので、DB から読み直せば反映される。
            // (child folder 経路は load_folder 由来で既に hydrate 済みなので不要。検索 view 由来
            //  解除は上の at_origin + pre_search_origin 分岐で load_folder に入るのでこちらは通らない。)
            let origin = snap.origin.clone();
            self.rehydrate_page_edit_state_for_current_items(&origin);
            if self.color_filter.enabled {
                self.mark_color_filter_scope_dirty();
                self.rebuild_visible_indices();
            }
        } else {
            // child folder の中で解除 → 現在の items はそのまま、snapshot state だけ捨てる
            // visible_indices は filter / current_folder に対して再構築
            self.mark_color_filter_scope_dirty();
            self.rebuild_visible_indices();
        }
        self.show_feedback_toast("★固定を解除しました".into());
    }

    /// Fix-B (ユーザー指摘): snapshot 内 child folder に居る状態で BS が押されたら、
    /// snapshot list view (= snapshot.items を render する状態) に戻る。
    ///
    /// 戻り値: `true` = snapshot list view に戻った、`false` = inactive or 既に snapshot
    /// list view 表示中 (= 通常 BS を続行すべき)。
    ///
    /// 実装:
    /// - snapshot.items から `reconstruct_grid_item` で GridItem を再構築 (= Codex P1-1
    ///   fix で導入した SnapshotTarget 経由なので ZipImage/PdfPage も復元できる)
    /// - thumbnails は Pending 状態でリセット (= 既存 thumbnail cache が path-keyed なので
    ///   即座に hit して再描画される)
    /// - filter suppress を解除 (= snapshot list 自体は filter を suppress しない設計)
    /// - current_folder = snapshot_origin、address bar 表示も更新
    pub(crate) fn snapshot_return_to_list_view(&mut self) -> bool {
        // 必要な snapshot field を最初に clone して借用を切る (= 後段の mut self call と衝突しない)。
        // list_view_items / list_view_thumbnails は activate_snapshot 時に保存した clone を
        // 使う (= reconstruct だと folder 代表サムネが Pending に戻って「フォルダアイコン」
        // 表示になるユーザー報告対応)。
        let (snap_origin, list_items, list_thumbs, is_search_snapshot) = {
            let Some(snap) = self.snapshot.as_ref() else {
                return false;
            };
            (
                snap.origin.clone(),
                snap.list_view_items.clone(),
                snap.list_view_thumbnails.clone(),
                // 検索 view 由来 snapshot は origin が cross-folder prefix なので rehydrate せず
                // clear のみ (= activate と同じ判定。pre_snapshot_search_origin Some が search 由来)。
                snap.pre_snapshot_search_origin.is_some(),
            )
        };
        // 既に snapshot root に居れば何もしない (= 通常 BS の対象)
        let at_origin = self
            .current_folder
            .as_ref()
            .map(|p| {
                crate::snapshot::snapshot_key_from_path(p)
                    == crate::snapshot::snapshot_key_from_path(&snap_origin)
            })
            .unwrap_or(false);
        if at_origin {
            return false;
        }
        let n = list_items.len();
        // filter suppress 解除 (= snapshot list view は filter 通常適用)
        let _ = self.restore_rating_filter_suppression();
        // fullscreen 中なら閉じる
        if self.fullscreen_idx.is_some() {
            self.close_fullscreen();
        }
        // snapshot 内から ZipFile を開いていた場合、その子 ZIP の zip_nav が残っている。
        // list view は start_loading_items を通さず items を差し替えるので、ここで明示的に
        // 破棄しないと stale zip_nav が list view 上の BS で旧 ZIP 階層を復活させる
        // (レビュー P2)。子 ZIP を離れるのでネスト ZIP バイト列キャッシュも合わせて破棄。
        if self.zip_nav.take().is_some() {
            crate::zip_loader::clear_nested_cache();
        }
        // items 入れ替え (= 保存したサムネ付き state を復帰)
        self.items = list_items;
        self.thumbnails = list_thumbs;
        self.visible_indices = (0..n).collect();
        self.current_folder = Some(snap_origin.clone());
        self.address = snap_origin.display().to_string();
        self.scroll_offset_y = 0.0;
        self.pending_grid_scroll = None;
        self.selected = None;
        // items_generation bump + invalidate (= Codex P1-1)
        self.items_generation = self.items_generation.wrapping_add(1);
        self.invalidate_idx_state_and_queues();
        self.tags_cache.clear();
        // snapshot list (= subset) に戻したので、ページ編集状態も subset idx で合わせ直す
        // (= activate と同じ、Codex P1)。通常フォルダ由来は origin の DB から hydrate
        // (child folder で編集した分は DB に同期保存済みなので subset 該当ページに反映される)。
        // 検索 view 由来は cross-folder prefix なので clear のみ (= activate と対称)。
        if is_search_snapshot {
            self.clear_page_edit_state();
        } else {
            self.rehydrate_page_edit_state_for_current_items(&snap_origin);
        }
        if self.color_filter.enabled {
            self.mark_color_filter_scope_dirty();
            self.rebuild_visible_indices();
        }
        self.show_feedback_toast("★固定リストに戻りました".into());
        true
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

    // ─── snapshot navigation (§4.6 混合 nav resolver) ──────────────────────────────────────

    /// 現在 fullscreen で開いている path を返す。
    /// snapshot 中で folder enter 後の場合、items は folder contents なので
    /// items[fullscreen_idx] の path を取り出す。
    pub(crate) fn snapshot_current_fullscreen_path(&self) -> Option<std::path::PathBuf> {
        let idx = self.fullscreen_idx?;
        let item = self.items.get(idx)?;
        // GridItem の path は display_path() ではなく真の path を取り出す必要がある
        use crate::grid_item::GridItem;
        match item {
            GridItem::Folder(p)
            | GridItem::Image(p)
            | GridItem::Video(p)
            | GridItem::ZipFile(p)
            | GridItem::PdfFile(p) => Some(p.clone()),
            GridItem::ConvertibleArchive { path, .. } => Some(path.clone()),
            GridItem::ZipImage {
                zip_path,
                entry_name,
            } => {
                // `<zip>/<entry>` 形式で構築 (= snapshot_key_from_path の split_archive_path
                // と整合)
                let mut p = zip_path.clone();
                p.push(entry_name);
                Some(p)
            }
            GridItem::PdfPage {
                pdf_path, page_num, ..
            } => {
                // `<pdf>/p:<num>` 形式
                let mut p = pdf_path.clone();
                p.push(format!("p:{page_num}"));
                Some(p)
            }
            _ => None,
        }
    }

    /// snapshot 内の **次/前 entry** index を計算する (= 混合 nav の Arrow 用)。
    ///
    /// `current_idx` が None なら snapshot 先頭 / 末尾を返す (= snapshot に最初に入る場合)。
    /// forward=true なら次へ、false なら前へ。末尾 (= wrap せず stop) なら None。
    pub(crate) fn snapshot_next_arrow_entry(
        &self,
        current_idx: Option<usize>,
        forward: bool,
    ) -> Option<usize> {
        let snap = self.snapshot.as_ref()?;
        let n = snap.items.len();
        if n == 0 {
            return None;
        }
        match current_idx {
            None => Some(if forward { 0 } else { n - 1 }),
            Some(i) => {
                if forward {
                    if i + 1 < n { Some(i + 1) } else { None }
                } else {
                    if i > 0 { Some(i - 1) } else { None }
                }
            }
        }
    }

    /// snapshot 内の **次/前 playable image-like entry** index (= Ctrl+PageUp/Down +
    /// slideshow 末尾用)。Folder/Zip/Pdf entry は skip。
    pub(crate) fn snapshot_next_playable_entry(
        &self,
        current_idx: Option<usize>,
        forward: bool,
    ) -> Option<usize> {
        let snap = self.snapshot.as_ref()?;
        let n = snap.items.len();
        if n == 0 {
            return None;
        }
        let start: isize = match current_idx {
            None => {
                if forward {
                    -1
                } else {
                    n as isize
                }
            }
            Some(i) => i as isize,
        };
        let step: isize = if forward { 1 } else { -1 };
        let mut cur = start + step;
        while cur >= 0 && cur < n as isize {
            let idx = cur as usize;
            if snap.items[idx].kind.is_playable_leaf() {
                return Some(idx);
            }
            cur += step;
        }
        None
    }

    /// snapshot 内 entry を open する (= load_folder + open_fullscreen のエントリポイント)。
    ///
    /// 動作:
    /// - playable leaf (Image/Video/ZipImage/PdfPage):
    ///   - 現在 items に同 path があれば `open_fullscreen(items_idx)` で直接
    ///   - 無ければ owner folder を `load_folder` してから fullscreen 開く
    /// - container (Folder/ZipFile/PdfFile):
    ///   - `load_folder(container_path)` で中身を items に展開
    ///   - 完了後に **最初の playable item を fullscreen で開く** (= 設計 §4.6)
    ///   - スライドショー継続の場合は `resume_slideshow` で起動
    pub(crate) fn snapshot_open_entry(&mut self, entry_idx: usize, resume_slideshow: bool) -> bool {
        use crate::grid_item::GridItem;
        use crate::snapshot::{SnapshotEntryKind, SnapshotTarget};
        let Some(snap) = self.snapshot.as_ref() else {
            return false;
        };
        let Some(entry) = snap.items.get(entry_idx) else {
            return false;
        };
        let entry_kind = entry.kind;
        // 構造化 target を使う (Codex P1-1 fix): display 文字列の round-trip 破綻を回避
        let entry_target = entry.target.clone();
        match entry_kind {
            SnapshotEntryKind::Image | SnapshotEntryKind::Video | SnapshotEntryKind::Audio => {
                let SnapshotTarget::Fs(target_path) = entry_target.clone() else {
                    return false;
                };
                // 現在 items の中に同 path があれば直接 open (音声も Inc 3 で音楽ビューを開く)
                if let Some(idx) = self.items.iter().position(|it| match it {
                    GridItem::Image(p) | GridItem::Video(p) | GridItem::Audio(p) => {
                        *p == target_path
                    }
                    _ => false,
                }) {
                    self.open_fullscreen(idx);
                    if resume_slideshow {
                        self.slideshow_playing = true;
                    }
                    return true;
                }
                // 該当 path が現在 items に無い → owner folder を load してから対象 leaf を open
                // (Codex 3rd P2 fix: 旧版は target を渡していなかったので first playable に
                // 着地していた)
                if let Some(folder) = target_path.parent().map(|p| p.to_path_buf()) {
                    self.snapshot_load_and_open(folder, resume_slideshow, Some(entry_target));
                    return true;
                }
                false
            }
            SnapshotEntryKind::ZipImage => {
                let SnapshotTarget::ZipImage {
                    zip_path,
                    entry_name,
                } = entry_target.clone()
                else {
                    return false;
                };
                // 現在 items の中に同 zip/entry があれば直接 open
                if let Some(idx) = self.items.iter().position(|it| match it {
                    GridItem::ZipImage {
                        zip_path: zp,
                        entry_name: en,
                    } => *zp == zip_path && *en == entry_name,
                    _ => false,
                }) {
                    self.open_fullscreen(idx);
                    if resume_slideshow {
                        self.slideshow_playing = true;
                    }
                    return true;
                }
                // 該当 zip が現在開かれていない → zip を load してから対象 entry を open
                self.snapshot_load_and_open(zip_path, resume_slideshow, Some(entry_target));
                true
            }
            SnapshotEntryKind::PdfPage => {
                let SnapshotTarget::PdfPage { pdf_path, page_num } = entry_target.clone() else {
                    return false;
                };
                if let Some(idx) = self.items.iter().position(|it| match it {
                    GridItem::PdfPage {
                        pdf_path: pp,
                        page_num: pn,
                        ..
                    } => *pp == pdf_path && *pn == page_num,
                    _ => false,
                }) {
                    self.open_fullscreen(idx);
                    if resume_slideshow {
                        self.slideshow_playing = true;
                    }
                    return true;
                }
                self.snapshot_load_and_open(pdf_path, resume_slideshow, Some(entry_target));
                true
            }
            SnapshotEntryKind::Folder | SnapshotEntryKind::ZipFile | SnapshotEntryKind::PdfFile => {
                let SnapshotTarget::Fs(container_path) = entry_target else {
                    return false;
                };
                // container 経路は target None (= first playable に着地)
                self.snapshot_load_and_open(container_path, resume_slideshow, None);
                true
            }
            SnapshotEntryKind::ConvertibleArchive => {
                // ConvertibleArchive (= RAR/7z/LZH 等) は format を保持して別経路 (= 変換 cache
                // 確認 + 必要なら変換 dialog) で開く。snapshot 中の自動 open は cache hit 時
                // のみサポート (= dialog が出る case は snapshot scope を逸脱するため skip)。
                let SnapshotTarget::ConvertibleArchive { path, format } = entry_target else {
                    return false;
                };
                let _ = format; // 変換 dialog は snapshot scope 外、cache hit 時のみ自動 open
                if self.settings.archive_file_handling_ignores_convertible() {
                    self.show_feedback_toast(
                        "設定により RAR / 7z / LZH アーカイブを無視しています".into(),
                    );
                    return false;
                }
                if let Some(cached) = self.try_archive_cache_lookup(&path) {
                    self.snapshot_load_and_open(cached, resume_slideshow, None);
                    true
                } else {
                    // cache 無し: snapshot 中は変換 dialog を出さず skip (= 次 entry に進む
                    // 想定だが、ユーザーの代表 use case ではないので MVP として false)
                    self.show_feedback_toast(
                        "変換対応アーカイブは★固定範囲内で開けません (解除してから開いてください)"
                            .into(),
                    );
                    false
                }
            }
        }
    }

    /// snapshot internal nav flag を立てて load_folder + 対象 leaf or 最初の playable item を
    /// fullscreen で開く。
    ///
    /// `target` が `Some` なら、load 完了後に items 内で対象を探して open (Codex 3rd P2 fix:
    /// 対象画像でなくフォルダ先頭に着地するバグの修正)。マッチしなければ first playable に
    /// fallback。`target` が `None` (= container 経路) なら first playable を開く。
    ///
    /// `load_folder_with_scan` の snapshot guard を bypass するため、`snapshot_internal_nav`
    /// を true にしてから呼ぶ。flag は呼び出し後に false に戻す (= scope guard pattern)。
    fn snapshot_load_and_open(
        &mut self,
        folder_path: std::path::PathBuf,
        resume_slideshow: bool,
        target: Option<crate::snapshot::SnapshotTarget>,
    ) {
        let was_fs = self.fullscreen_idx.is_some();
        // 現在 fullscreen を閉じる (= items が入れ替わるので)
        if was_fs {
            self.close_fullscreen();
        }
        // guard bypass
        self.snapshot_internal_nav = true;
        self.load_folder(folder_path);
        self.snapshot_internal_nav = false;
        // ★一時解除中の自動発動は `maybe_restore_rating_filter_if_out_of_scope`
        // (= load_folder 末尾で呼ばれる) が現在 folder の★を見て自動再評価するので、
        // ここで明示的に呼ぶ必要はない (= 旧版で必要だった理由が消えた)。
        // ただし suppress 発動後に rebuild_visible_indices が走らないケースがある
        // (= 一部の load_folder 経路) ので、念のため rebuild を呼ぶ。
        if self.rating_filter_suppressed_at.is_some() {
            self.rebuild_visible_indices();
        }
        // open 意図 (= was_fs / resume_slideshow / 明示 target) が無ければ何もしない。
        // Codex 4th P2 fix: `target.is_some()` も open 条件に含める (grid 経路の snapshot
        // leaf navigation でも target で指定された leaf を開く)。
        if !(was_fs || resume_slideshow || target.is_some()) {
            return;
        }
        // ZIP/PDF は非同期列挙なので、この時点では items がまだ揃っていない。その場合は
        // target を deferred reopen に載せ、`poll_zip_enumerate` /
        // `poll_pdf_enumerate` 完了時に解決して開く (Codex P2 fix: 旧版は同期 lookup のみで
        // first playable / 先頭に着地し、ロックリストから未展開 ZIP/PDF 内ページへの
        // ジャンプ・スライドショー復帰が対象ページに到達しなかった)。
        if self.pdf_enumerate_pending.is_some() || self.zip_enumerate_pending.is_some() {
            self.fs_nav_after_pdf_enumerate = Some(crate::app::DeferredFsReopen {
                resume_slideshow,
                target,
                resume_to_last_page: false,
                from_explicit_open: false,
                preserve_after_password_prompt: false,
            });
            return;
        }
        // 同期に items が揃った場合 (= 通常フォルダ / cache hit PDF placeholder 等) は即 open:
        // - target Some → items 内で対象 leaf を探す
        // - target None or マッチしない → 最初の playable item に fallback
        use crate::grid_item::GridItem;
        let open_idx = target
            .as_ref()
            .and_then(|t| self.resolve_snapshot_target_idx(t))
            .or_else(|| {
                self.items.iter().position(|it| {
                    matches!(
                        it,
                        GridItem::Image(_)
                            | GridItem::Video(_)
                            | GridItem::ZipImage { .. }
                            | GridItem::PdfPage { .. }
                    )
                })
            });
        if let Some(idx) = open_idx {
            self.open_fullscreen(idx);
            if resume_slideshow {
                self.slideshow_playing = true;
            }
        }
    }

    /// `SnapshotTarget` を現在の `items` から解決して idx を返す (Codex P2 fix の共有 helper)。
    ///
    /// `snapshot_load_and_open` の同期 open 経路と、`poll_zip_enumerate` /
    /// `poll_pdf_enumerate` の deferred reopen 経路の両方から使い、target マッチロジックの
    /// 重複を避ける。`ConvertibleArchive` は leaf ではないので常に `None`。
    pub(crate) fn resolve_snapshot_target_idx(
        &self,
        target: &crate::snapshot::SnapshotTarget,
    ) -> Option<usize> {
        use crate::grid_item::GridItem;
        use crate::snapshot::SnapshotTarget;
        match target {
            SnapshotTarget::Fs(p) => self.items.iter().position(|it| match it {
                GridItem::Image(ip) | GridItem::Video(ip) => ip == p,
                _ => false,
            }),
            SnapshotTarget::ZipImage {
                zip_path,
                entry_name,
            } => self.items.iter().position(|it| match it {
                GridItem::ZipImage {
                    zip_path: zp,
                    entry_name: en,
                } => zp == zip_path && en == entry_name,
                _ => false,
            }),
            SnapshotTarget::PdfPage { pdf_path, page_num } => {
                self.items.iter().position(|it| match it {
                    GridItem::PdfPage {
                        pdf_path: pp,
                        page_num: pn,
                        ..
                    } => pp == pdf_path && pn == page_num,
                    _ => false,
                })
            }
            SnapshotTarget::ConvertibleArchive { .. } => None,
        }
    }

    /// `snapshot_open_entry` を呼び、その nav 試行が items reload も deferred reopen も
    /// 起こさなかった場合 (= 直接 open / open 失敗のどちらでも、待つべき非同期変化が無い)
    /// は `capture_fs_nav_holdover` で取得した nav lock を release する。
    ///
    /// fullscreen からの Ctrl+↑↓ / Ctrl+PageUp/Down (`snapshot_navigate`) も スライドショー
    /// 自動送り (`snapshot_advance_for_slideshow`) も、open 前に `capture_fs_nav_holdover` で
    /// lock を立てる。直接 open は `open_fullscreen` が items_generation を進めないため、
    /// 解除しないと `poll_fs_nav_lock` の gen-check に到達せず lock が残り、次のナビが
    /// `fs_nav_is_locked()` で永久 block される (= 末尾経路と同じ理由が成功経路でも起きる)。
    ///
    /// 除外される経路: reload (snapshot_load_and_open の sync open) は items_generation が進む
    /// ので `poll_fs_nav_lock` が gen-check + 新 tex 用意で解除。deferred ZIP/PDF は
    /// `fs_nav_after_pdf_enumerate` が立つので `poll_fs_nav_lock` が enumerate 完了まで lock を
    /// 保持。どちらも gen 変化 / deferred 有無で自動的に除外される。
    /// (grid nav 経路は holdover を取らないので release は idempotent な no-op。)
    fn snapshot_open_entry_release_lock_if_direct(
        &mut self,
        entry_idx: usize,
        resume_slideshow: bool,
    ) -> bool {
        let gen_before = self.items_generation;
        let opened = self.snapshot_open_entry(entry_idx, resume_slideshow);
        if self.items_generation == gen_before && self.fs_nav_after_pdf_enumerate.is_none() {
            self.release_fs_nav_lock();
        }
        opened
    }

    /// snapshot navigation のエントリポイント (Ctrl+↑↓ / Ctrl+PageUp/Down)。
    ///
    /// fullscreen から呼ばれる。`forward=true` で次へ、`page_only=true` で image-like のみ巡回。
    ///
    /// 戻り値: `true` = navigation した、`false` = snapshot inactive / 末尾。
    /// 末尾の場合は boundary hint をセットする。
    pub(crate) fn snapshot_navigate(
        &mut self,
        ctx: &egui::Context,
        forward: bool,
        page_only: bool,
        resume_slideshow: bool,
    ) -> bool {
        if !self.is_snapshot_active() {
            return false;
        }
        // 現在 fullscreen path → owner_entry idx を解決
        let current_owner = self
            .snapshot_current_fullscreen_path()
            .and_then(|p| self.snapshot_owner_entry(&p));
        let next = if page_only {
            self.snapshot_next_playable_entry(current_owner, forward)
        } else {
            self.snapshot_next_arrow_entry(current_owner, forward)
        };
        if let Some(idx) = next {
            // 直接 open 後の nav lock 解除を含めて wrapper に委譲 (= スライドショー経路と共有、
            // 経路漏れ防止)。
            self.snapshot_open_entry_release_lock_if_direct(idx, resume_slideshow)
        } else {
            // 末尾: boundary hint + nav lock 解除
            // snapshot 経路は apply_folder_nav_result を通らないので、capture_fs_nav_holdover
            // で取得した lock が残ったまま (= 次の Ctrl+↑↓ が fs_nav_is_locked() で block される)。
            // 末尾検知時に明示的に release が必要。
            self.release_fs_nav_lock();
            self.fs_boundary_hint = Some(crate::ui_fullscreen::FsBoundaryHint::NoImageFolder {
                forward,
                at: std::time::Instant::now(),
            });
            ctx.request_repaint();
            false
        }
    }

    /// snapshot 内の **次/前 container entry** index を返す
    /// (= Folder / ZipFile / PdfFile / ConvertibleArchive のみ対象、image-like は skip)。
    ///
    /// P2-2 fix 後は内部利用なし (= 全 entry を扱う arrow / playable resolver に統一)。
    /// 将来 container only nav が必要になった場合のために保持。
    #[allow(dead_code)]
    pub(crate) fn snapshot_next_container_entry(
        &self,
        current_idx: Option<usize>,
        forward: bool,
    ) -> Option<usize> {
        let snap = self.snapshot.as_ref()?;
        let n = snap.items.len();
        if n == 0 {
            return None;
        }
        let start: isize = match current_idx {
            None => {
                if forward {
                    -1
                } else {
                    n as isize
                }
            }
            Some(i) => i as isize,
        };
        let step: isize = if forward { 1 } else { -1 };
        let mut cur = start + step;
        while cur >= 0 && cur < n as isize {
            let idx = cur as usize;
            if snap.items[idx].kind.is_container() {
                return Some(idx);
            }
            cur += step;
        }
        None
    }

    /// グリッド中の snapshot navigation (= Ctrl+↑↓ from grid)。
    ///
    /// Codex P2-2 修正: resolver semantics 統一。
    /// - Ctrl+↑↓ → 全 entry (snapshot_next_arrow_entry)
    /// - 次 entry が container なら load_folder で grid 表示
    /// - 次 entry が image-like なら直接 open_fullscreen (= 自然な「次へ」)
    ///
    /// fullscreen 中の snapshot_navigate と semantics 完全一致。
    /// 戻り値: `true` = navigation した、`false` = 末尾 / inactive。末尾は toast。
    pub(crate) fn snapshot_navigate_grid(&mut self, forward: bool) -> bool {
        self.snapshot_navigate_grid_inner(forward, /*page_only=*/ false)
    }

    /// グリッド中の snapshot Ctrl+PageUp/Down 用 (= playable のみ巡回)。
    pub(crate) fn snapshot_navigate_grid_page(&mut self, forward: bool) -> bool {
        self.snapshot_navigate_grid_inner(forward, /*page_only=*/ true)
    }

    fn snapshot_navigate_grid_inner(&mut self, forward: bool, page_only: bool) -> bool {
        if !self.is_snapshot_active() {
            return false;
        }
        let current_owner = self
            .current_folder
            .clone()
            .and_then(|p| self.snapshot_owner_entry(&p));
        let next_idx = if page_only {
            self.snapshot_next_playable_entry(current_owner, forward)
        } else {
            self.snapshot_next_arrow_entry(current_owner, forward)
        };
        let Some(next_idx) = next_idx else {
            self.show_feedback_toast(
                if forward {
                    "★固定リスト末尾です"
                } else {
                    "★固定リスト先頭です"
                }
                .into(),
            );
            return false;
        };
        // kind で動作分岐: container は grid 表示 (load_folder)、playable は fullscreen
        let entry_kind = self
            .snapshot
            .as_ref()
            .and_then(|s| s.items.get(next_idx))
            .map(|e| e.kind);
        let Some(entry_kind) = entry_kind else {
            return false;
        };
        if entry_kind.is_playable_leaf() {
            // image-like → fullscreen で開く (= grid からでも snapshot 内 image を直接 view)
            self.snapshot_open_entry(next_idx, /*resume_slideshow=*/ false)
        } else {
            // container → grid 表示で load_folder (fullscreen は開かない、既存挙動)
            let Some(entry) = self.snapshot.as_ref().and_then(|s| s.items.get(next_idx)) else {
                return false;
            };
            use crate::snapshot::SnapshotTarget;
            let target_path = match &entry.target {
                SnapshotTarget::Fs(p) => p.clone(),
                SnapshotTarget::ConvertibleArchive { path, .. } => {
                    if self.settings.archive_file_handling_ignores_convertible() {
                        self.show_feedback_toast(
                            "設定により RAR / 7z / LZH アーカイブを無視しています".into(),
                        );
                        return false;
                    }
                    if let Some(cached) = self.try_archive_cache_lookup(path) {
                        cached
                    } else {
                        self.show_feedback_toast(
                            "変換対応アーカイブは★固定範囲内で開けません".into(),
                        );
                        return false;
                    }
                }
                _ => return false,
            };
            self.snapshot_internal_nav = true;
            self.load_folder(target_path);
            self.snapshot_internal_nav = false;
            if self.rating_filter_suppressed_at.is_some() {
                self.rebuild_visible_indices();
            }
            true
        }
    }

    /// snapshot navigation のスライドショー版 (= ctx 不要、末尾で slideshow 停止)。
    ///
    /// Codex P2-2 修正: resolver semantics 統一。snapshot_next_arrow_entry (= 全 entry)
    /// で次 entry を探し、kind に応じて:
    /// - container → snapshot_open_entry が snapshot_load_and_open で最初の playable item
    ///   を fullscreen で開く + slideshow_playing=true 再開
    /// - playable leaf → snapshot_open_entry が直接 open_fullscreen + slideshow_playing
    ///
    /// 旧版は container 限定 (snapshot_next_container_entry) だったため、snapshot 内が
    /// image のみだと末尾扱いで停止していた。新挙動では image-only snapshot でも
    /// 次 image に進める。
    ///
    /// 戻り値:
    /// - true = 次 entry open or 末尾停止のいずれかで処理完了 (= caller の loop fallback 抑止)
    /// - false = snapshot inactive (= caller が通常 fallback)
    pub(crate) fn snapshot_advance_for_slideshow(&mut self, forward: bool) -> bool {
        if !self.is_snapshot_active() {
            return false;
        }
        let current_owner = self
            .snapshot_current_fullscreen_path()
            .and_then(|p| self.snapshot_owner_entry(&p))
            .or_else(|| {
                self.current_folder
                    .clone()
                    .and_then(|p| self.snapshot_owner_entry(&p))
            });
        // P2-2 fix: 全 entry を対象に巡回 (= Ctrl+↑↓ と同じ semantics)
        let next = self.snapshot_next_arrow_entry(current_owner, forward);
        if let Some(idx) = next {
            // スライドショー自動送りも fullscreen から holdover を取って呼ばれるので、直接 open
            // 後は nav lock を解除する (= 手動 Ctrl+↑↓ と共有の wrapper、Codex follow-up)。
            let _ = self
                .snapshot_open_entry_release_lock_if_direct(idx, /*resume_slideshow=*/ true);
            true
        } else {
            // 末尾: 次 entry 無し → slideshow 停止 + nav lock 解除
            self.release_fs_nav_lock();
            self.slideshow_playing = false;
            self.slideshow_anchor_idx = None;
            self.slideshow_scroll_anim = None;
            self.slideshow_scroll_range_cache = None;
            self.show_feedback_toast(
                if forward {
                    "★固定リスト末尾です (スライドショー停止)"
                } else {
                    "★固定リスト先頭です (スライドショー停止)"
                }
                .into(),
            );
            true
        }
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

    /// ネスト ZIP の本の中で ★固定 → zip_nav は必ず退避 (take) され、at_origin の解除で
    /// saved_items と対で復元される。残したままだと snapshot 表示中の BS が
    /// `zip_nav_back()` に落ちて stale な ZIP 階層を snapshot ビューへ上書きする
    /// (レビュー P2)。
    #[test]
    fn activate_parks_zip_nav_and_deactivate_restores_it() {
        let zip_path = PathBuf::from(r"E:\test\outer.zip");
        let mut app = test_app_with_items(vec![GridItem::ZipImage {
            zip_path: zip_path.clone(),
            entry_name: "bookA/p1.jpg".to_string(),
        }]);
        app.current_folder = Some(zip_path.clone());

        let entries = vec![
            crate::zip_loader::ZipImageEntry {
                entry_name: "bookA/p1.jpg".to_string(),
                uncompressed_size: 0,
                mtime: 0,
            },
            crate::zip_loader::ZipImageEntry {
                entry_name: "bookB/p1.jpg".to_string(),
                uncompressed_size: 0,
                mtime: 0,
            },
        ];
        let tree = std::sync::Arc::new(crate::zip_tree::ZipTree::build(zip_path, entries));
        let mut nav = crate::zip_tree::ZipNavState::new(tree);
        nav.enter("bookA/"); // 本の中 (スタック深さ 2)
        app.zip_nav = Some(nav);

        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        assert!(app.is_snapshot_active());
        assert!(
            app.zip_nav.is_none(),
            "snapshot 中に zip_nav が残ると BS が ZIP 階層を復活させる"
        );

        // at_origin (= current_folder 不変) の解除で nav も復元される。
        app.deactivate_snapshot();
        assert!(!app.is_snapshot_active());
        let nav = app.zip_nav.as_ref().expect("saved_zip_nav が復元される");
        assert!(
            !nav.at_root(),
            "退避時のスタック深さ (本の中) ごと復元される"
        );
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
        // SearchContainer は snapshot 対象外なので、混ざっていても snapshot からは除外される。
        let mut app = test_app_with_items(vec![
            GridItem::Image(PathBuf::from(r"E:\a.png")),
            GridItem::SearchContainer {
                path: PathBuf::from(r"E:\hits"),
                kind: crate::grid_item::SearchContainerKind::Folder,
                hit_count: 2,
                representative: None,
            },
            GridItem::Image(PathBuf::from(r"E:\b.png")),
        ]);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        // SearchContainer は除外されて 2 件。
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
        // snapshot 中は scroll はリセット、selected は snapshot 内対応 idx に変換される
        // (= 両 items とも snapshot 対象なので元 idx=1 → 新 idx=1)
        assert_eq!(app.scroll_offset_y, 0.0);
        assert_eq!(app.selected, Some(1));
        assert!(
            app.scroll_to_selected,
            "保持した selected が画面外に居る可能性があるので scroll_to_selected を立てる"
        );

        app.deactivate_snapshot();
        assert!(!app.is_snapshot_active());
        // 復元
        assert_eq!(app.items.len(), 2);
        assert_eq!(app.scroll_offset_y, 123.0);
        assert_eq!(app.selected, Some(1));
    }

    #[test]
    fn snapshot_preserves_selected_with_remapped_index() {
        // 元 items: [SearchContainer, Folder, Image]、selected=2 (= Image)。
        // SearchContainer は対象外なので snapshot subset は [Folder, Image]。
        // 元 selected=2 (Image) → snapshot 内 idx=1 (Image) に remap される
        let mut app = test_app_with_items(vec![
            GridItem::SearchContainer {
                path: PathBuf::from(r"E:\hits"),
                kind: crate::grid_item::SearchContainerKind::Folder,
                hit_count: 1,
                representative: None,
            },
            GridItem::Folder(PathBuf::from(r"E:\folder")),
            GridItem::Image(PathBuf::from(r"E:\img.png")),
        ]);
        app.selected = Some(2);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        // snapshot subset 内で Image は idx=1
        assert_eq!(app.selected, Some(1));
    }

    #[test]
    fn snapshot_clears_selected_when_selected_not_in_snapshot() {
        // 元 selected が SearchContainer (snapshot 対象外) → 新 selected は None。
        let mut app = test_app_with_items(vec![
            GridItem::Folder(PathBuf::from(r"E:\folder")),
            GridItem::SearchContainer {
                path: PathBuf::from(r"E:\hits"),
                kind: crate::grid_item::SearchContainerKind::Folder,
                hit_count: 1,
                representative: None,
            },
        ]);
        app.selected = Some(1); // SearchContainer
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        assert_eq!(app.selected, None);
        // scroll_to_selected は new_selected が None なので立たない
        assert!(!app.scroll_to_selected);
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

    // ─── nav resolver tests (§4.6 P1-3 解決) ──────────

    #[test]
    fn next_arrow_entry_forward_steps_through_all_kinds() {
        // [Image, Folder, Video] の混合 entry を Ctrl+↑↓ で全部巡回
        let mut app = test_app_with_items(vec![
            GridItem::Image(PathBuf::from(r"E:\a.png")),
            GridItem::Folder(PathBuf::from(r"E:\folder")),
            GridItem::Video(PathBuf::from(r"E:\b.mp4")),
        ]);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        // current=None で先頭、forward → 0 → 1 → 2 → None
        assert_eq!(app.snapshot_next_arrow_entry(None, true), Some(0));
        assert_eq!(app.snapshot_next_arrow_entry(Some(0), true), Some(1));
        assert_eq!(app.snapshot_next_arrow_entry(Some(1), true), Some(2));
        assert_eq!(app.snapshot_next_arrow_entry(Some(2), true), None);
    }

    #[test]
    fn next_arrow_entry_backward_steps_through_all_kinds() {
        let mut app = test_app_with_items(vec![
            GridItem::Image(PathBuf::from(r"E:\a.png")),
            GridItem::Folder(PathBuf::from(r"E:\folder")),
            GridItem::Video(PathBuf::from(r"E:\b.mp4")),
        ]);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        // current=None で末尾、backward → 2 → 1 → 0 → None
        assert_eq!(app.snapshot_next_arrow_entry(None, false), Some(2));
        assert_eq!(app.snapshot_next_arrow_entry(Some(2), false), Some(1));
        assert_eq!(app.snapshot_next_arrow_entry(Some(1), false), Some(0));
        assert_eq!(app.snapshot_next_arrow_entry(Some(0), false), None);
    }

    #[test]
    fn next_playable_entry_skips_folder_entries() {
        // Ctrl+PageUp/Down は image-like のみ巡回 (Folder/Zip/Pdf skip)
        let mut app = test_app_with_items(vec![
            GridItem::Image(PathBuf::from(r"E:\a.png")),
            GridItem::Folder(PathBuf::from(r"E:\folder")), // skip 対象
            GridItem::ZipFile(PathBuf::from(r"E:\b.zip")), // skip 対象
            GridItem::Video(PathBuf::from(r"E:\c.mp4")),
        ]);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        assert_eq!(app.snapshot_next_playable_entry(None, true), Some(0));
        // 0 (Image) → next playable は 3 (Video)、1 (Folder) と 2 (ZipFile) は skip
        assert_eq!(app.snapshot_next_playable_entry(Some(0), true), Some(3));
        assert_eq!(app.snapshot_next_playable_entry(Some(3), true), None);
    }

    #[test]
    fn next_playable_entry_all_folders_returns_none() {
        // 全部 Folder の snapshot だと playable が無い
        let mut app = test_app_with_items(vec![
            GridItem::Folder(PathBuf::from(r"E:\a")),
            GridItem::Folder(PathBuf::from(r"E:\b")),
        ]);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        assert_eq!(app.snapshot_next_playable_entry(None, true), None);
        assert_eq!(app.snapshot_next_playable_entry(Some(0), true), None);
    }

    #[test]
    fn next_arrow_entry_inactive_returns_none() {
        let app = App::default();
        assert_eq!(app.snapshot_next_arrow_entry(None, true), None);
        assert_eq!(app.snapshot_next_playable_entry(None, true), None);
    }

    // ─── Codex 5th review P3: 回帰テスト追加 ────────────

    /// Ctrl+G 検索 view から ★固定すると `pre_snapshot_search_origin` が
    /// `global_search.saved_folder` から capture されることを保証する回帰テスト
    /// (Codex 4th review で deactivate 時に検索表示が残るバグの根本対策)。
    #[test]
    fn activate_with_global_search_captures_pre_snapshot_origin() {
        let mut app = test_app_with_items(vec![GridItem::Image(PathBuf::from(r"E:\a.png"))]);
        // Ctrl+G が active + saved_folder を持っている状態を模擬
        app.global_search.active = true;
        app.global_search.saved_folder = Some(PathBuf::from(r"E:\original_folder"));
        app.activate_snapshot(SnapshotSourceLabel::GlobalSearch {
            query: "klee".into(),
        });
        let snap = app.snapshot.as_ref().expect("snapshot active");
        assert_eq!(
            snap.pre_snapshot_search_origin,
            Some(PathBuf::from(r"E:\original_folder")),
            "Ctrl+G の saved_folder が pre_snapshot_search_origin に capture される"
        );
        // 検索 mode は consume 済み (= scope mutual exclusion)
        assert!(!app.global_search.active);
    }

    /// Ctrl+S 検索 view から ★固定すると `pre_snapshot_search_origin` が
    /// `favsearch.saved_folder` から capture されることを保証する。
    #[test]
    fn activate_with_favsearch_captures_pre_snapshot_origin() {
        let mut app = test_app_with_items(vec![GridItem::Image(PathBuf::from(r"E:\a.png"))]);
        app.favsearch.active = true;
        app.favsearch.saved_folder = Some(PathBuf::from(r"E:\fav_origin"));
        app.activate_snapshot(SnapshotSourceLabel::FavSearch {
            query: "sun".into(),
        });
        let snap = app.snapshot.as_ref().expect("snapshot active");
        assert_eq!(
            snap.pre_snapshot_search_origin,
            Some(PathBuf::from(r"E:\fav_origin")),
            "Ctrl+S の saved_folder が pre_snapshot_search_origin に capture される"
        );
        assert!(!app.favsearch.active);
    }

    /// 通常 folder (= 検索 view ではない) から ★固定した場合、
    /// `pre_snapshot_search_origin` は None になる (= deactivate で saved_items
    /// 復元 path に入る、検索戻り先 load_folder は走らない)。
    #[test]
    fn activate_from_normal_folder_has_no_pre_snapshot_search_origin() {
        let mut app = test_app_with_items(vec![GridItem::Image(PathBuf::from(r"E:\a.png"))]);
        // 検索系すべて inactive
        assert!(!app.global_search.active);
        assert!(!app.favsearch.active);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        let snap = app.snapshot.as_ref().expect("snapshot active");
        assert!(
            snap.pre_snapshot_search_origin.is_none(),
            "通常 folder からの ★固定では pre_snapshot_search_origin は None"
        );
    }

    /// Codex 6th P3 回帰テスト: 検索 view 由来 snapshot を **child folder 内で** 解除した
    /// 場合は、検索前 folder に飛ばず「解除直前のフォルダ維持」(= 既存仕様) を守る。
    ///
    /// シナリオ:
    /// - Ctrl+G で ★固定 (= pre_snapshot_search_origin Some)
    /// - snapshot 内 child folder X に入る (= current_folder が origin と不一致)
    /// - 解除 → child folder X のままで居る (= restore_to に飛ばない)
    #[test]
    fn deactivate_in_child_folder_keeps_current_when_pre_origin_some() {
        let mut app = test_app_with_items(vec![GridItem::Image(PathBuf::from(r"E:\a.png"))]);
        app.global_search.active = true;
        app.global_search.saved_folder = Some(PathBuf::from(r"E:\before_search"));
        app.activate_snapshot(SnapshotSourceLabel::GlobalSearch {
            query: "test".into(),
        });
        // snapshot 中、child folder の中に navigate した状態を模擬 (= current_folder 変更)
        let child_folder = PathBuf::from(r"E:\drilled\container\subfolder");
        app.current_folder = Some(child_folder.clone());
        // 解除: at_origin = false なので pre_snapshot_search_origin Some でも load_folder
        // しない (= 既存「解除直前のフォルダ維持」path に落ちる)
        app.deactivate_snapshot();
        // current_folder は child folder のまま (= restore_to に飛んでいない)
        assert_eq!(
            app.current_folder,
            Some(child_folder),
            "child folder 内での解除では current_folder を維持 (= P2-1 fix)"
        );
        // suppress_nav_record_for_search_restore は立っていない (= load_folder してない)
        assert!(!app.suppress_nav_record_for_search_restore);
    }

    /// Codex 6th P3 回帰テスト: 検索 view 由来 snapshot を **origin で** 解除すると
    /// load_folder(restore_to) が呼ばれる (= current_folder が restore_to に向かう)。
    /// 同経路で suppress_nav_record_for_search_restore も立てられるが、load_folder 内で
    /// `mem::take` 消費されるので、観測可能な指標 (= snapshot None + current_folder 変化) で
    /// verify する。
    #[test]
    fn deactivate_at_origin_with_pre_search_origin_takes_restore_path() {
        let mut app = test_app_with_items(vec![GridItem::Image(PathBuf::from(r"E:\a.png"))]);
        app.current_folder = Some(PathBuf::from(r"E:\drilled\container"));
        app.global_search.active = true;
        app.global_search.saved_folder = Some(PathBuf::from(r"E:\before_search"));
        app.activate_snapshot(SnapshotSourceLabel::GlobalSearch {
            query: "test".into(),
        });
        // current_folder は origin のまま (= drilled view で固定したケース)
        assert_eq!(
            app.current_folder,
            Some(PathBuf::from(r"E:\drilled\container"))
        );
        let before_cf = app.current_folder.clone();
        // 解除: at_origin = true + pre_search_origin Some → load_folder(restore_to)
        app.deactivate_snapshot();
        // 重要な不変条件: snapshot は解除された
        assert!(app.snapshot.is_none());
        // current_folder は load_folder で restore_to に変わる試行が走る
        // (= test fixture では実 path 無くても load_folder 冒頭で current_folder が
        //  restore_to に向かって更新される、または save 経路を通る)
        // → 少なくとも before_cf と異なる、または restore_to に到達する
        let after_cf = app.current_folder.clone();
        assert!(
            after_cf != before_cf || after_cf == Some(PathBuf::from(r"E:\before_search")),
            "deactivate で current_folder が変化する (= restore path に入った証拠) \
             before={before_cf:?}, after={after_cf:?}"
        );
    }

    /// snapshot_open_entry が現在 items 内に同 path の image leaf を直接 fullscreen で
    /// 開けることを保証する (= 同 folder 内 leaf navigation の最短経路)。
    /// Codex 4th P2 fix の前提となる「items hit時は load_folder せず直接 open」を verify。
    #[test]
    fn snapshot_open_entry_image_leaf_in_current_items_opens_directly() {
        let mut app = test_app_with_items(vec![
            GridItem::Image(PathBuf::from(r"E:\test\a.png")),
            GridItem::Image(PathBuf::from(r"E:\test\b.png")),
        ]);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        // entry idx 1 (= b.png) を open する → items[1] が既にあるので open_fullscreen 直接呼び
        let result = app.snapshot_open_entry(1, /*resume_slideshow=*/ false);
        assert!(result, "items 内 leaf の直接 open は true 返す");
        // fullscreen_idx は items の idx 経由で設定される
        assert_eq!(app.fullscreen_idx, Some(1));
    }

    /// `pre_snapshot_search_origin` の **active flag としての使用** = synthetic 判定では
    /// なく Some/None で deactivate path を分岐 (= 021c54fe の修正対象)。
    /// drilled view (= origin が drill container の実 path) でも検索 view 由来なら
    /// 戻り先 load_folder path に入ることを保証する設計上の不変条件テスト。
    #[test]
    fn pre_snapshot_search_origin_works_for_drilled_view_not_synthetic() {
        // drilled view を模擬: current_folder = drill container 実 path、
        // global_search.active=true + saved_folder=Some
        let mut app = test_app_with_items(vec![GridItem::Image(PathBuf::from(r"E:\a.png"))]);
        app.current_folder = Some(PathBuf::from(r"E:\drilled\container")); // 実 path
        app.global_search.active = true;
        app.global_search.saved_folder = Some(PathBuf::from(r"E:\before_search"));
        app.activate_snapshot(SnapshotSourceLabel::GlobalSearch {
            query: "test".into(),
        });
        let snap = app.snapshot.as_ref().expect("snapshot active");
        // origin は drill container 実 path (= 合成 path ではない)
        assert_eq!(snap.origin, PathBuf::from(r"E:\drilled\container"));
        // しかし pre_snapshot_search_origin は Some なので、deactivate 時に
        // synthetic 判定でなく Some/None 判定で 戻り先 load_folder path に入る
        assert_eq!(
            snap.pre_snapshot_search_origin,
            Some(PathBuf::from(r"E:\before_search"))
        );
    }

    /// Codex P2 (deferred target 解決): `resolve_snapshot_target_idx` が各 leaf 種別の
    /// target を現在 items から正しく idx 解決する。これが snapshot_load_and_open の同期
    /// open 経路と poll_zip/pdf_enumerate の deferred reopen 経路の両方の核。
    #[test]
    fn resolve_snapshot_target_idx_matches_each_leaf_kind() {
        use crate::snapshot::SnapshotTarget;
        let app = test_app_with_items(vec![
            GridItem::Image(PathBuf::from(r"E:\test\a.png")),
            GridItem::PdfPage {
                pdf_path: PathBuf::from(r"E:\test\doc.pdf"),
                page_num: 3,
                content_type: None,
            },
            GridItem::ZipImage {
                zip_path: PathBuf::from(r"E:\test\arc.zip"),
                entry_name: "sub/img.png".into(),
            },
        ]);

        // Fs (画像) target → idx 0
        assert_eq!(
            app.resolve_snapshot_target_idx(&SnapshotTarget::Fs(PathBuf::from(r"E:\test\a.png"))),
            Some(0)
        );
        // PdfPage target → idx 1 (path + page_num 一致)
        assert_eq!(
            app.resolve_snapshot_target_idx(&SnapshotTarget::PdfPage {
                pdf_path: PathBuf::from(r"E:\test\doc.pdf"),
                page_num: 3,
            }),
            Some(1)
        );
        // ZipImage target → idx 2 (zip_path + entry_name 一致)
        assert_eq!(
            app.resolve_snapshot_target_idx(&SnapshotTarget::ZipImage {
                zip_path: PathBuf::from(r"E:\test\arc.zip"),
                entry_name: "sub/img.png".into(),
            }),
            Some(2)
        );
        // 別ページ番号は不一致 → None
        assert_eq!(
            app.resolve_snapshot_target_idx(&SnapshotTarget::PdfPage {
                pdf_path: PathBuf::from(r"E:\test\doc.pdf"),
                page_num: 99,
            }),
            None
        );
        // items に無い path → None
        assert_eq!(
            app.resolve_snapshot_target_idx(&SnapshotTarget::Fs(PathBuf::from(r"E:\test\zzz.png"))),
            None
        );
        // ConvertibleArchive は leaf でない → 常に None
        assert_eq!(
            app.resolve_snapshot_target_idx(&SnapshotTarget::ConvertibleArchive {
                path: PathBuf::from(r"E:\test\arc.7z"),
                format: crate::archive_converter::ArchiveFormat::SevenZ,
            }),
            None
        );
    }

    /// Codex follow-up: snapshot list 内 leaf 間の Ctrl+↑↓ (= 直接 open、reload 無し) の後、
    /// nav lock が解除されること。直接 open は items_generation を進めないので、解除しないと
    /// `poll_fs_nav_lock` の gen-check に到達せず lock が残り、次の Ctrl+↑↓ が永久 block される
    /// (= 末尾経路と同じ理由だが成功経路で漏れていた)。
    #[test]
    fn snapshot_direct_open_nav_releases_fs_nav_lock() {
        let ctx = egui::Context::default();
        let mut app = test_app_with_items(vec![
            GridItem::Image(PathBuf::from(r"E:\test\a.png")),
            GridItem::Image(PathBuf::from(r"E:\test\b.png")),
        ]);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        // a を fullscreen で開く (= 現 items 内なので直接 open)。
        assert!(app.snapshot_open_entry(0, /*resume_slideshow=*/ false));
        assert_eq!(app.fullscreen_idx, Some(0));
        // Ctrl+↑↓ 入口の capture_fs_nav_holdover が lock を立てた状態を再現。
        app.capture_fs_nav_holdover(0);
        assert!(app.fs_nav_is_locked(), "ナビ直前は lock が立つ");
        let gen_before = app.items_generation;

        // 次の leaf (b) へ snapshot_navigate (= 直接 open、reload 無し)。
        let moved = app.snapshot_navigate(&ctx, /*forward=*/ true, false, false);

        assert!(moved, "次の leaf へ移動する");
        assert_eq!(app.fullscreen_idx, Some(1), "b に移動");
        assert_eq!(
            app.items_generation, gen_before,
            "直接 open は items_generation を進めない (= reload していない)"
        );
        assert!(
            !app.fs_nav_is_locked(),
            "直接 open 後は nav lock が解除され、次の Ctrl+↑↓ が block されない"
        );
    }

    /// Codex follow-up (スライドショー経路): snapshot スライドショーの直接 leaf 送りでも
    /// nav lock が解除されること。`snapshot_advance_for_slideshow` も fullscreen から holdover
    /// を取って呼ばれるので、手動 Ctrl+↑↓ と同じ wrapper で release する。
    #[test]
    fn snapshot_slideshow_direct_advance_releases_fs_nav_lock() {
        let mut app = test_app_with_items(vec![
            GridItem::Image(PathBuf::from(r"E:\test\a.png")),
            GridItem::Image(PathBuf::from(r"E:\test\b.png")),
        ]);
        app.activate_snapshot(SnapshotSourceLabel::Mixed);
        assert!(app.snapshot_open_entry(0, /*resume_slideshow=*/ true));
        assert_eq!(app.fullscreen_idx, Some(0));
        app.capture_fs_nav_holdover(0);
        assert!(app.fs_nav_is_locked());
        let gen_before = app.items_generation;

        let advanced = app.snapshot_advance_for_slideshow(/*forward=*/ true);

        assert!(advanced, "次の leaf へ自動送りする");
        assert_eq!(app.fullscreen_idx, Some(1), "b に送られる");
        assert_eq!(
            app.items_generation, gen_before,
            "直接 open は items_generation を進めない"
        );
        assert!(
            !app.fs_nav_is_locked(),
            "スライドショー直接送り後も nav lock が解除される"
        );
    }
}
