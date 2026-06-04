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
        // 元の selected idx (= items 内のグローバル位置) を snapshot 内の対応 idx に
        // 変換 (= ユーザー要望: ボタンを押したときのカーソル位置を保持)。
        // 元 items[old_idx] の SnapshotKey → membership で snapshot 内 idx を引く。
        // snapshot 対象外 item (ZipSeparator 等) や、そもそも未選択なら None。
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
            list_view_items,
            list_view_thumbnails,
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
        if at_origin {
            // origin のまま解除 → 元の items 復元 (= snapshot 元のフォルダに戻る)
            self.items = snap.saved_items;
            self.thumbnails = snap.saved_thumbnails;
            self.visible_indices = snap.saved_visible_indices;
            self.scroll_offset_y = snap.saved_scroll_offset_y;
            self.selected = snap.saved_selected;
            // idx-based cache を invalidate (= items 入れ替え対応)
            self.rating_cache.clear();
            self.rotation_cache.clear();
            self.tags_cache.clear();
        } else {
            // child folder の中で解除 → 現在の items はそのまま、snapshot state だけ捨てる
            // visible_indices は filter / current_folder に対して再構築
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
        let (snap_origin, list_items, list_thumbs) = {
            let Some(snap) = self.snapshot.as_ref() else {
                return false;
            };
            (
                snap.origin.clone(),
                snap.list_view_items.clone(),
                snap.list_view_thumbnails.clone(),
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
        // items 入れ替え (= 保存したサムネ付き state を復帰)
        self.items = list_items;
        self.thumbnails = list_thumbs;
        self.visible_indices = (0..n).collect();
        self.current_folder = Some(snap_origin.clone());
        self.address = snap_origin.display().to_string();
        self.scroll_offset_y = 0.0;
        self.selected = None;
        // idx-based cache を invalidate (= items 入れ替え対応)
        self.rating_cache.clear();
        self.rotation_cache.clear();
        self.tags_cache.clear();
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
            SnapshotEntryKind::Image | SnapshotEntryKind::Video => {
                let SnapshotTarget::Fs(target_path) = entry_target else {
                    return false;
                };
                // 現在 items の中に同 path があれば直接 open
                if let Some(idx) = self.items.iter().position(|it| match it {
                    GridItem::Image(p) | GridItem::Video(p) => *p == target_path,
                    _ => false,
                }) {
                    self.open_fullscreen(idx);
                    if resume_slideshow {
                        self.slideshow_playing = true;
                    }
                    return true;
                }
                // 該当 path が現在 items に無い → owner folder を load してから open
                if let Some(folder) = target_path.parent().map(|p| p.to_path_buf()) {
                    self.snapshot_load_and_open(folder, resume_slideshow);
                    return true;
                }
                false
            }
            SnapshotEntryKind::ZipImage => {
                let SnapshotTarget::ZipImage {
                    zip_path,
                    entry_name,
                } = entry_target
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
                // 該当 zip が現在開かれていない → zip を load してから open
                self.snapshot_load_and_open(zip_path, resume_slideshow);
                true
            }
            SnapshotEntryKind::PdfPage => {
                let SnapshotTarget::PdfPage { pdf_path, page_num } = entry_target else {
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
                self.snapshot_load_and_open(pdf_path, resume_slideshow);
                true
            }
            SnapshotEntryKind::Folder
            | SnapshotEntryKind::ZipFile
            | SnapshotEntryKind::PdfFile
            | SnapshotEntryKind::ConvertibleArchive => {
                let SnapshotTarget::Fs(container_path) = entry_target else {
                    return false;
                };
                self.snapshot_load_and_open(container_path, resume_slideshow);
                true
            }
        }
    }

    /// snapshot internal nav flag を立てて load_folder + 最初の playable item を fullscreen で開く。
    ///
    /// `load_folder_with_scan` の snapshot guard を bypass するため、`snapshot_internal_nav`
    /// を true にしてから呼ぶ。flag は呼び出し後に false に戻す (= scope guard pattern)。
    fn snapshot_load_and_open(&mut self, folder_path: std::path::PathBuf, resume_slideshow: bool) {
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
        // load_folder 完了後に最初の playable item (= Image/Video) を fullscreen で開く。
        // ※ ZIP/PDF は async enumerate なので、items は ZipSeparator のみで埋まっている可能性が
        //   ある。その場合は次フレーム以降の enumerate 完了で deferred reopen される (= §4.6
        //   末尾説明)。MVP では sync 経路のみ対応、ZIP/PDF への snapshot navigation は実機
        //   確認で評価する。
        if was_fs || resume_slideshow {
            // 最初の image-like item を探して open
            if let Some(first_idx) = self.items.iter().position(|it| {
                matches!(
                    it,
                    crate::grid_item::GridItem::Image(_)
                        | crate::grid_item::GridItem::Video(_)
                        | crate::grid_item::GridItem::ZipImage { .. }
                        | crate::grid_item::GridItem::PdfPage { .. }
                )
            }) {
                self.open_fullscreen(first_idx);
                if resume_slideshow {
                    self.slideshow_playing = true;
                }
            }
        }
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
            self.snapshot_open_entry(idx, resume_slideshow)
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
    /// グリッド中の Ctrl+↑↓ で snapshot 内 folder 間を巡回するために使う。
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
    /// 現在 folder が snapshot 内 container entry の中に居る場合、その entry の owner_idx
    /// を出発点に、次/前の container entry を resolve して `load_folder` で開く。
    /// fullscreen は開かない (= グリッド表示を維持)。
    ///
    /// 戻り値: `true` = navigation した、`false` = 末尾 / inactive。末尾は toast。
    pub(crate) fn snapshot_navigate_grid(&mut self, forward: bool) -> bool {
        if !self.is_snapshot_active() {
            return false;
        }
        // 現在 folder → snapshot 内 container entry の idx を解決
        // (= snapshot_origin に居る場合は None、child folder の中なら owner_entry が hit)
        let current_owner = self
            .current_folder
            .clone()
            .and_then(|p| self.snapshot_owner_entry(&p));
        let Some(next_idx) = self.snapshot_next_container_entry(current_owner, forward) else {
            // snapshot 内に次/前 container が無い (= 末尾、または snapshot 全部 image)
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
        // 次 container entry の path を取得して snapshot_load_and_open
        let Some(entry) = self.snapshot.as_ref().and_then(|s| s.items.get(next_idx)) else {
            return false;
        };
        use crate::snapshot::SnapshotTarget;
        let target_path = match &entry.target {
            SnapshotTarget::Fs(p) => p.clone(),
            // ZipImage/PdfPage は is_container() = false なので next_container_entry が返さない
            // が、防御的に early return
            _ => return false,
        };
        // grid mode = was_fs false + resume_slideshow false で呼ぶ
        // (= snapshot_load_and_open の内部 if was_fs || resume_slideshow ブランチは
        // 通らず、items 入れ替えだけ。fullscreen は開かない。)
        self.snapshot_internal_nav = true;
        self.load_folder(target_path);
        self.snapshot_internal_nav = false;
        // ★一時解除中の自動発動は `maybe_restore_rating_filter_if_out_of_scope`
        // (= load_folder 末尾) が現在 folder の★を見て自動再評価するので、ここで
        // 明示的に呼ぶ必要はない。
        if self.rating_filter_suppressed_at.is_some() {
            self.rebuild_visible_indices();
        }
        true
    }

    /// snapshot navigation のスライドショー版 (= ctx 不要、末尾で slideshow 停止)。
    ///
    /// `try_start_slideshow_next_folder` から呼ばれる。slideshow_playing を維持しつつ
    /// 次の **container entry** (= Folder/Zip/Pdf) を open する (= snapshot 内の次 folder
    /// 巡回がメイン use case)。container 内で先頭画像から slideshow 再開する。
    /// 末尾なら slideshow 停止して true 返す (= caller がループフォールバックしない)。
    ///
    /// 戻り値:
    /// - true = 次 container open or 末尾停止のいずれかで処理完了 (= caller の loop fallback 抑止)
    /// - false = snapshot inactive (= caller が通常 fallback)
    pub(crate) fn snapshot_advance_for_slideshow(&mut self, forward: bool) -> bool {
        if !self.is_snapshot_active() {
            return false;
        }
        // 現在 folder の owner_entry idx を取得。snapshot_current_fullscreen_path は
        // image path を返すので、その image が属する owner container を解決する。
        // current_folder ベースで owner を引いた方が確実なので両方試みて優先順位を付ける。
        let current_owner = self
            .snapshot_current_fullscreen_path()
            .and_then(|p| self.snapshot_owner_entry(&p))
            .or_else(|| {
                self.current_folder
                    .clone()
                    .and_then(|p| self.snapshot_owner_entry(&p))
            });
        // 次の container entry (= ★3 folder の次など) を探す
        let next = self.snapshot_next_container_entry(current_owner, forward);
        if let Some(idx) = next {
            // 次 container を open + 中で先頭画像から slideshow 再開
            // snapshot_open_entry は Folder kind の entry で snapshot_load_and_open を呼び、
            // resume_slideshow=true なら最初の playable item で fullscreen + slideshow_playing
            let _ = self.snapshot_open_entry(idx, /*resume_slideshow=*/ true);
            true
        } else {
            // 末尾: 次 container 無し → slideshow 停止 + nav lock 解除
            // capture_fs_nav_holdover で取得した lock を release しないと次の Ctrl+↑↓ が
            // fs_nav_is_locked() で block される (= ユーザー報告「Ctrl+上下で移動できなくなる」)。
            self.release_fs_nav_lock();
            self.slideshow_playing = false;
            self.slideshow_anchor_idx = None;
            self.show_feedback_toast(
                if forward {
                    "★固定リスト末尾です (スライドショー停止)"
                } else {
                    "★固定リスト先頭です (スライドショー停止)"
                }
                .into(),
            );
            // true 返す: caller (= try_start_slideshow_next_folder) は false だと loop fallback
            // するので、末尾検知は true 扱いで「処理完了」と通知する。
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
        // 元 items: [Sep, Folder, Image] (idx 0/1/2)、selected=2 (= Image)
        // Sep は snapshot 対象外なので snapshot subset は [Folder, Image] (idx 0/1)
        // 元 selected=2 (Image) → snapshot 内 idx=1 (Image) に remap される
        let mut app = test_app_with_items(vec![
            GridItem::ZipSeparator {
                dir_display: "title".into(),
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
        // 元 selected が ZipSeparator (snapshot 対象外) → 新 selected は None
        let mut app = test_app_with_items(vec![
            GridItem::Folder(PathBuf::from(r"E:\folder")),
            GridItem::ZipSeparator {
                dir_display: "title".into(),
            },
        ]);
        app.selected = Some(1); // ZipSeparator
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
}
