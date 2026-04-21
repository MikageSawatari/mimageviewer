//! Ctrl+G グローバルメタ検索の UI + 状態管理 (docs/search-expansion-design.md §10.3)。
//!
//! - 状態: `GlobalSearchState` (App が所有)
//! - キーバインド: Ctrl+G で toggle
//! - トップパネル: クエリ入力 + 進捗バッジ
//! - streaming 受信: `poll_events()` が SearchStreamEvent を try_recv で消費、
//!   ContainerHit に集約、items を再構築
//!
//! ## v1 スコープ
//!
//! - トップレベル集約ビュー (ヒットを含むフォルダ/ZIP を SearchContainer セルで並べる)
//! - Drill-down view (1 階層降りた先の絞り込み表示) は本モジュールには含まず、
//!   後続コミットで追加予定 (docs §10.3 の [3] 絞り込みビュー)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::app::App;
use crate::global_search::{DoneReason, GlobalHit, SearchStreamEvent};
use crate::grid_item::{GridItem, SearchContainerKind};
use crate::indexer_manager::SearchHandle;

/// クエリ入力後、検索実行までの debounce 間隔 (既存 Ctrl+F と揃える)。
const DEBOUNCE_MS: u64 = 300;
/// 1 フレームで消費するイベント数の上限 (UI ブロックを防ぐ)。
const MAX_EVENTS_PER_FRAME: usize = 8;
/// ContainerHit の再ソート間隔 (チラつき防止、docs §10.4.3)。
const RESORT_INTERVAL_MS: u64 = 1000;

/// Ctrl+G のビュー状態 (docs §10.3 [2]-[3])。
///
/// DrilledInto 時は「現在地 (current_path) 直下に落ちるヒット + ヒットを含む子フォルダ」
/// だけを表示する (ヒットが 1 件もない枝は枝刈り)。Ctrl+↑↓ はこの枝刈り済みツリー
/// 上で移動する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobalSearchView {
    /// トップレベル集約表示。SearchContainer セルがヒット件数降順で並ぶ
    Aggregated,
    /// drill-down 中。
    /// - `container_root`: ドリルインの起点 (SearchContainer のパス)
    /// - `current_path`: 現在地 (container_root と同じか、その配下の子フォルダ)
    /// - `is_zip`: container が ZIP ファイルか
    DrilledInto {
        container_root: PathBuf,
        current_path: PathBuf,
        is_zip: bool,
    },
}

/// Ctrl+G 検索の状態 (App が所有する)。
pub struct GlobalSearchState {
    /// true のとき、トップバー表示 + 検索結果ビュー有効
    pub active: bool,
    /// クエリ入力テキスト
    pub query: String,
    /// 最後に実行したクエリ (変更検知用)
    pub last_executed: String,
    /// 次フレームでフォーカス要求
    pub focus_request: bool,
    /// TextEdit がフォーカスを持っているか (他キーバインドが取らないようにするため)
    pub has_focus: bool,
    /// 入力が変わった時刻 (debounce 用)
    pub last_change_at: Option<Instant>,
    /// 最後に結果をソートし直した時刻
    pub last_sort_at: Option<Instant>,
    /// 起動中の検索
    pub pending: Option<SearchHandle>,
    /// 親コンテナ path → 集約済みヒット (docs §10.4.2 ContainerHit)
    pub containers: HashMap<PathBuf, ContainerHit>,
    /// 生の全ヒット (drill-down 時に container でフィルタするため保持)
    pub all_hits: Vec<GlobalHit>,
    /// 現在のビューモード (Aggregated or DrilledInto)
    pub view: GlobalSearchView,
    /// streaming 経過統計
    pub total_valid: usize,
    pub total_scanned: usize,
    /// 完了フラグ
    pub done: bool,
    /// HARD_MAX で打ち切られたか
    pub truncated: bool,
    /// RejectReason のユーザー向けメッセージ (空クエリ / 短すぎる / NOT-only)
    pub reject_message: Option<String>,
    /// Ctrl+G 以前の current_folder (戻り先として保存)
    pub saved_folder: Option<PathBuf>,
}

impl Default for GlobalSearchState {
    fn default() -> Self {
        Self {
            active: false,
            query: String::new(),
            last_executed: String::new(),
            focus_request: false,
            has_focus: false,
            last_change_at: None,
            last_sort_at: None,
            pending: None,
            containers: HashMap::new(),
            all_hits: Vec::new(),
            view: GlobalSearchView::Aggregated,
            total_valid: 0,
            total_scanned: 0,
            done: false,
            truncated: false,
            reject_message: None,
            saved_folder: None,
        }
    }
}

/// 集約済みコンテナ (v1 トップレベルビュー用)。
#[derive(Clone, Debug)]
pub struct ContainerHit {
    pub path: PathBuf,
    pub kind: SearchContainerKind,
    pub hit_count: usize,
}

impl GlobalSearchState {
    /// 新規検索を開始する (既存 pending があれば cancel してから)。
    pub fn reset_for_new_query(&mut self) {
        // SearchHandle は Drop で cancel するので、take() だけで OK
        self.pending = None;
        self.containers.clear();
        self.all_hits.clear();
        self.view = GlobalSearchView::Aggregated; // 新クエリで drill state もリセット
        self.total_valid = 0;
        self.total_scanned = 0;
        self.done = false;
        self.truncated = false;
        self.reject_message = None;
    }

    /// 集約ロジック (docs §10.4.2): 1 ヒットをコンテナに追加 + 生データも保持
    fn accumulate_hit(&mut self, hit: &GlobalHit) {
        let (container_path, kind) = parent_container(&hit.path);
        let entry = self
            .containers
            .entry(container_path.clone())
            .or_insert_with(|| ContainerHit {
                path: container_path,
                kind,
                hit_count: 0,
            });
        entry.hit_count += 1;
        // drill-down 用に生のヒットも保持 (path で後でフィルタする)
        self.all_hits.push(hit.clone());
    }
}

/// ヒット path から親コンテナを決定する (docs §10.4.2)。
/// - ZIP エントリ (`<zippath>!<entry>` 形式) → ZIP ファイルパス
/// - 通常ファイル → 親フォルダパス
fn parent_container(hit_path: &str) -> (PathBuf, SearchContainerKind) {
    if let Some(idx) = hit_path.find('!') {
        let (zip_part, _entry) = hit_path.split_at(idx);
        return (PathBuf::from(zip_part), SearchContainerKind::Zip);
    }
    let p = PathBuf::from(hit_path);
    if let Some(parent) = p.parent() {
        (parent.to_path_buf(), SearchContainerKind::Folder)
    } else {
        (p, SearchContainerKind::Folder)
    }
}

/// ContainerHit を hit_count 降順 / 名前昇順 でソートした Vec を返す。
pub fn sorted_containers(
    containers: &HashMap<PathBuf, ContainerHit>,
) -> Vec<ContainerHit> {
    let mut v: Vec<ContainerHit> = containers.values().cloned().collect();
    v.sort_by(|a, b| {
        b.hit_count
            .cmp(&a.hit_count)
            .then_with(|| a.path.cmp(&b.path))
    });
    v
}

// -----------------------------------------------------------------------
// 絞り込みビュー (DrilledInto) 用のアイテム構築ヘルパ
// -----------------------------------------------------------------------

/// Aggregated view の items + image_metas を組み立てる。
pub(crate) fn build_aggregated_items(
    state: &GlobalSearchState,
) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>) {
    let containers = sorted_containers(&state.containers);
    let items: Vec<GridItem> = containers
        .iter()
        .map(|c| GridItem::SearchContainer {
            path: c.path.clone(),
            kind: c.kind,
            hit_count: c.hit_count,
        })
        .collect();
    // SearchContainer はサムネイル不要 (make_load_request で None) だが、image_metas は
    // items と同じ長さに揃えておく (Option<(0,0)>) ことで thumb_loader 側の
    // image_metas.get(i).flatten() が panic ではなく「enqueue skip」になる。
    let image_metas: Vec<Option<(i64, i64)>> = vec![None; items.len()];
    (items, image_metas)
}

/// DrilledInto view の items + image_metas を組み立てる。
///
/// `current_path` 直下のヒット (Image/ZipImage) と、`current_path` の直下でヒットを
/// 含む子フォルダ (Folder 枝、件数バッジ付き) だけを並べる。ヒットを含まない枝は
/// 枝刈りされる。
///
/// image_metas は UI スレッドでの `fs::metadata` 同期呼び出しで埋める。ヒット件数は
/// 通常数 〜 数百件で、1 ディレクトリあたり 10ms オーダー以内に収まる想定。
/// 大量ヒット環境でのボトルネック化が観測されたら、GlobalHit に mtime/file_size を
/// 持たせて Tantivy の STORED フィールドから取り出す方式に切り替える (v0.8.x 課題)。
pub(crate) fn build_drilled_items(
    state: &GlobalSearchState,
    current_path: &Path,
    is_zip: bool,
) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>) {
    if is_zip {
        return build_drilled_zip_items(state, current_path);
    }
    // ── 通常フォルダ配下の絞り込み ──
    let mut direct_files: Vec<PathBuf> = Vec::new();
    // 直下子フォルダ → その配下のヒット件数
    let mut sub_counts: HashMap<PathBuf, usize> = HashMap::new();

    for h in &state.all_hits {
        if h.path.contains('!') {
            continue; // ZIP ヒットはスキップ
        }
        let hp = PathBuf::from(&h.path);
        let Some(hp_parent) = hp.parent() else { continue };
        // current_path の配下でなければ無関係
        if !path_is_under_or_eq(hp_parent, current_path) {
            continue;
        }
        if hp_parent == current_path {
            direct_files.push(hp);
        } else {
            // current_path の直下の子フォルダ名を拾う
            let rel = match hp_parent.strip_prefix(current_path) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if let Some(first) = rel.components().next() {
                let child = current_path.join(first.as_os_str());
                *sub_counts.entry(child).or_insert(0) += 1;
            }
        }
    }

    // サブフォルダ (Folder) を名前昇順 → 続いて直下ファイル (Image) を名前昇順
    let mut sub_vec: Vec<(PathBuf, usize)> = sub_counts.into_iter().collect();
    sub_vec.sort_by(|a, b| a.0.cmp(&b.0));
    direct_files.sort();

    let mut items: Vec<GridItem> = Vec::with_capacity(sub_vec.len() + direct_files.len());
    let mut image_metas: Vec<Option<(i64, i64)>> = Vec::with_capacity(items.capacity());

    for (sub_path, _hits) in &sub_vec {
        // Folder セルは通常 make_load_request でフォルダ内代表画像を拾うので、
        // image_metas はフォルダ自身の mtime を入れておけば cache キーが安定する。
        items.push(GridItem::Folder(sub_path.clone()));
        image_metas.push(path_metadata(sub_path));
    }
    for f in &direct_files {
        items.push(GridItem::Image(f.clone()));
        image_metas.push(path_metadata(f));
    }

    (items, image_metas)
}

/// ZIP コンテナをドリルインしたときのアイテム構築 (v0.8.0 はフラット表示)。
fn build_drilled_zip_items(
    state: &GlobalSearchState,
    zip_path: &Path,
) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>) {
    let zip_key = crate::search_index_db::normalize_path(zip_path);
    let zip_meta = path_metadata(zip_path);
    let mut items: Vec<GridItem> = Vec::new();
    let mut image_metas: Vec<Option<(i64, i64)>> = Vec::new();
    for h in &state.all_hits {
        let Some((zip_part, entry)) = h.path.split_once('!') else {
            continue;
        };
        if zip_part != zip_key {
            continue;
        }
        items.push(GridItem::ZipImage {
            zip_path: zip_path.to_path_buf(),
            entry_name: entry.to_string(),
        });
        // ZIP エントリの mtime/size は zip_loader が必要時に解決するので、
        // ここでは外側 ZIP ファイルの mtime を共有値として入れておく (cache キー安定)。
        image_metas.push(zip_meta);
    }
    (items, image_metas)
}

/// ファイルの mtime + file_size を取得する (失敗時 None)。
fn path_metadata(p: &Path) -> Option<(i64, i64)> {
    let md = std::fs::metadata(p).ok()?;
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let size = md.len() as i64;
    Some((mtime, size))
}

/// `child` が `ancestor` と等しいか配下にあれば true。
fn path_is_under_or_eq(child: &Path, ancestor: &Path) -> bool {
    child == ancestor || child.starts_with(ancestor)
}

/// Ctrl+↑↓ 用のナビゲーションエントリ。コンテナ境界を跨ぐフラットリストで使う。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NavEntry {
    pub container_root: PathBuf,
    pub path: PathBuf,
    pub is_zip: bool,
}

/// 全コンテナを束ねた Ctrl+↑↓ ナビゲーションリストを作る。
///
/// - コンテナ順序は Aggregated view と同じ (ヒット件数降順 → 名前昇順)
/// - 各 Folder コンテナは `collect_hit_folders_dfs` で DFS 展開 (親 → 子 → 兄弟)
/// - ZIP コンテナは ZIP ルート 1 点のみ (v0.8.0 は内部 DFS 未対応, ZIP 内は
///   ルートにいる扱いで次のコンテナに跳ぶ)
///
/// 集約ロジックは「ヒットの直上フォルダ」単位なので、`C:/A` と `C:/A/sub` が
/// どちらも独立コンテナとして現れる場合がある (C:/A に直接ヒット + C:/A/sub にも
/// ヒット)。その場合 DFS 展開すると `C:/A/sub` は `sub コンテナのルート` と
/// `A コンテナの子` として二重に現れるので、**path 単位で dedup して最後の
/// 出現を残す**。これで「A → A/sub → B」という自然な (親 → 子 → 兄弟) DFS 順に
/// なり、重複エントリを跨いだ不要な行ったり来たりを防げる。
pub(crate) fn build_cross_container_nav_list(state: &GlobalSearchState) -> Vec<NavEntry> {
    let containers = sorted_containers(&state.containers);
    let mut raw: Vec<NavEntry> = Vec::new();
    for c in &containers {
        match c.kind {
            SearchContainerKind::Folder => {
                for p in collect_hit_folders_dfs(&state.all_hits, &c.path) {
                    raw.push(NavEntry {
                        container_root: c.path.clone(),
                        path: p,
                        is_zip: false,
                    });
                }
            }
            SearchContainerKind::Zip => {
                raw.push(NavEntry {
                    container_root: c.path.clone(),
                    path: c.path.clone(),
                    is_zip: true,
                });
            }
        }
    }
    // path 単位で dedup。最後の出現を残すことで、ネスト構造では「親コンテナ内の
    // 子」として列挙される entry を優先する (container_root の親側が保持される)。
    // `seen_last_pos` を使って最後の出現位置を記録し、それ以外を除外する。
    let mut last_pos: std::collections::HashMap<PathBuf, usize> =
        std::collections::HashMap::with_capacity(raw.len());
    for (i, e) in raw.iter().enumerate() {
        last_pos.insert(e.path.clone(), i);
    }
    raw.into_iter()
        .enumerate()
        .filter(|(i, e)| last_pos.get(&e.path) == Some(i))
        .map(|(_, e)| e)
        .collect()
}

/// `container_root` 配下でヒットを含むフォルダを DFS 順で列挙する。
/// 先頭は常に `container_root` 自身。
pub(crate) fn collect_hit_folders_dfs(
    all_hits: &[GlobalHit],
    container_root: &Path,
) -> Vec<PathBuf> {
    // ヒットの直上フォルダ + その container_root までの祖先を全部集める
    let mut folders: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    folders.insert(container_root.to_path_buf());
    for h in all_hits {
        if h.path.contains('!') {
            continue; // ZIP ヒットはスキップ (container_root が ZIP のときは未対応)
        }
        let hp = PathBuf::from(&h.path);
        let Some(mut cur) = hp.parent().map(Path::to_path_buf) else { continue };
        while path_is_under_or_eq(&cur, container_root) {
            folders.insert(cur.clone());
            if cur == container_root {
                break;
            }
            match cur.parent() {
                Some(parent) => cur = parent.to_path_buf(),
                None => break,
            }
        }
    }
    // PathBuf の lexicographic 順 = DFS pre-order (親 < 子、兄弟は名前順) になる
    folders.into_iter().collect()
}

// -----------------------------------------------------------------------
// App 側との連携 (impl App 拡張)
// -----------------------------------------------------------------------

impl App {
    /// Ctrl+G 検索結果ビュー専用の軽量 items 差し替え (Codex P2 指摘対応)。
    ///
    /// `start_loading_items` がやるフォルダ切替フルコース (sidecar flush / fullscreen 閉じ /
    /// catalog open / prewarm_rating_cache / worker spawn / settings.last_folder 保存) は
    /// **実行しない**。この関数は以下の最小限を整合させるだけ:
    ///
    /// - items / thumbnails / image_metas を新しい並びに差し替え
    /// - 旧インデックスを参照していた requested / checked / pending_finalize / selected を解除
    /// - search_filter / search_query をクリア (Ctrl+F と共存させないため)
    /// - in-flight の search_pending / metadata_pending を cancel
    /// - visible_indices を再計算
    /// - スクロールを先頭に戻す
    ///
    /// catalog / folder_history / current_folder は触らない — Ctrl+G は `saved_folder` で
    /// 独自に戻り先を保持するため、実フォルダの概念は介入させない。
    pub(crate) fn replace_search_view_items(
        &mut self,
        items: Vec<GridItem>,
        image_metas: Vec<Option<(i64, i64)>>,
    ) {
        debug_assert_eq!(items.len(), image_metas.len());
        // 旧タスク停止: インデックスが付け替わるので in-flight は意味を失う
        if let Some(pending) = self.search_pending.take() {
            pending.cancel();
        }
        if let Some(pending) = self.metadata_pending.take() {
            pending.cancel();
        }
        self.items = items;
        self.image_metas = image_metas;
        self.thumbnails = (0..self.items.len())
            .map(|_| crate::grid_item::ThumbnailState::Pending)
            .collect();
        self.selected = None;
        self.checked.clear();
        self.requested.clear();
        self.pending_finalize.clear();
        self.keep_range = (0, 0);
        // 旧 items 参照の値は意味を失うので metadata / exif / xmp / rotation / rating
        // のキャッシュもリセットする (idx ベース)。
        self.metadata_cache.clear();
        self.exif_cache.clear();
        self.xmp_cache.clear();
        self.rotation_cache.clear();
        self.rating_cache.clear();
        // Codex P2-1: Ctrl+F フィルタの残留を解除
        self.search_filter = None;
        self.search_query.clear();
        self.scroll_offset_y = 0.0;
        self.scroll_to_selected = false;
        self.scroll_hint
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.rebuild_visible_indices();
    }

    /// Ctrl+G を押したときのエントリ (open or close toggle)。
    pub(crate) fn toggle_global_search(&mut self) {
        if self.global_search.active {
            self.close_global_search();
        } else {
            self.open_global_search();
        }
    }

    pub(crate) fn open_global_search(&mut self) {
        if self.global_search.active {
            return;
        }
        self.global_search.active = true;
        self.global_search.focus_request = true;
        self.global_search.saved_folder = self.current_folder.clone();
    }

    pub(crate) fn close_global_search(&mut self) {
        if !self.global_search.active {
            return;
        }
        // pending があれば SearchHandle の Drop impl で cancel される
        self.global_search.pending = None;
        self.global_search.active = false;
        self.global_search.has_focus = false;
        // Codex round-9 Should-fix #2: drill state / all_hits / done フラグも明示クリア。
        // 旧実装は containers だけクリアしていたため、DrilledInto のまま閉じて再度 Ctrl+G を
        // 開くと、検索前なのに戻るボタンや古い drill-down UI が残る可能性があった。
        self.global_search.containers.clear();
        self.global_search.all_hits.clear();
        self.global_search.view = GlobalSearchView::Aggregated;
        self.global_search.query.clear();
        self.global_search.last_executed.clear();
        self.global_search.reject_message = None;
        self.global_search.done = false;
        self.global_search.truncated = false;
        self.global_search.total_valid = 0;
        self.global_search.total_scanned = 0;
        // 元のフォルダに戻る
        if let Some(folder) = self.global_search.saved_folder.take() {
            self.load_folder(folder);
        }
    }

    /// debounce 経過チェック + 新クエリがあれば検索 spawn (App::update から毎フレーム呼ぶ)。
    pub(crate) fn poll_global_search_debounce(&mut self) {
        if !self.global_search.active {
            return;
        }
        // クエリが変わっていないならスキップ
        if self.global_search.query == self.global_search.last_executed {
            return;
        }
        // debounce: 最後の変更から DEBOUNCE_MS 経過するまで待つ
        let Some(t) = self.global_search.last_change_at else {
            return;
        };
        if t.elapsed() < Duration::from_millis(DEBOUNCE_MS) {
            return;
        }
        self.spawn_global_search();
    }

    /// 現在のクエリで検索を spawn する。
    pub(crate) fn spawn_global_search(&mut self) {
        self.global_search.reset_for_new_query();
        self.global_search.last_executed = self.global_search.query.clone();

        let Some(mgr) = self.indexer_manager.as_ref() else {
            self.global_search.reject_message =
                Some("全文検索インデクサが利用できません".to_string());
            self.global_search.done = true;
            self.rebuild_items_from_global_search();
            return;
        };

        // auto_index_metadata=true のお気に入り UUID を集める
        let favs: Vec<uuid::Uuid> = self
            .settings
            .favorites
            .iter()
            .filter(|f| f.auto_index_metadata)
            .map(|f| f.id)
            .collect();

        let handle = mgr.spawn_search(self.global_search.query.clone(), favs);
        self.global_search.pending = Some(handle);
        // items を空にして "検索中" 表示に切り替え
        self.rebuild_items_from_global_search();
    }

    /// SearchStreamEvent を try_recv で処理する (毎フレーム呼ぶ)。
    pub(crate) fn poll_global_search_events(&mut self, ctx: &egui::Context) {
        if self.global_search.done {
            return;
        }
        // rx を clone してから global_search の他フィールドを可変借用可能にする
        // (crossbeam-channel の Receiver は Clone 可能)
        let rx = match self.global_search.pending.as_ref() {
            Some(h) => h.rx.clone(),
            None => return,
        };
        let mut events_processed = 0;
        let mut changed = false;
        while events_processed < MAX_EVENTS_PER_FRAME {
            match rx.try_recv() {
                Ok(SearchStreamEvent::Batch {
                    hits,
                    scanned_candidates,
                    valid_hits,
                }) => {
                    for h in &hits {
                        self.global_search.accumulate_hit(h);
                    }
                    self.global_search.total_scanned = scanned_candidates;
                    self.global_search.total_valid = valid_hits;
                    if !hits.is_empty() {
                        changed = true;
                    }
                    events_processed += 1;
                }
                Ok(SearchStreamEvent::Done { truncated, reason }) => {
                    self.global_search.done = true;
                    self.global_search.truncated = truncated;
                    match reason {
                        DoneReason::RejectedQuery(r) => {
                            self.global_search.reject_message =
                                Some(r.as_user_message().to_string());
                        }
                        _ => {}
                    }
                    changed = true;
                    break;
                }
                Ok(SearchStreamEvent::Error(msg)) => {
                    self.global_search.done = true;
                    self.global_search.reject_message = Some(format!("エラー: {msg}"));
                    changed = true;
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.global_search.done = true;
                    changed = true;
                    break;
                }
            }
        }
        if changed {
            // docs §10.4.3: 順序再評価は 1 秒毎で十分 (頻繁な入れ替えでチラつかない)
            let should_resort = match self.global_search.last_sort_at {
                None => true,
                Some(t) => t.elapsed() >= Duration::from_millis(RESORT_INTERVAL_MS),
            };
            if should_resort || self.global_search.done {
                self.rebuild_items_from_global_search();
                self.global_search.last_sort_at = Some(Instant::now());
            }
        }
        if !self.global_search.done {
            // 次イベントを拾うため再描画を要求
            ctx.request_repaint();
        }
    }

    /// 現在の view に応じて items を組み立て、軽量 helper で差し替える。
    ///
    /// Codex P2 指摘対応: フォルダ切替フルコース (catalog open / worker spawn /
    /// sidecar flush / settings save 等) は走らせず、items / image_metas /
    /// thumbnails / visible_indices と、Codex P2-1/P2-2 で指摘された
    /// search_filter / checked を整合させるだけに留める。
    pub(crate) fn rebuild_items_from_global_search(&mut self) {
        let (items, image_metas) = match self.global_search.view.clone() {
            GlobalSearchView::Aggregated => build_aggregated_items(&self.global_search),
            GlobalSearchView::DrilledInto {
                ref current_path,
                is_zip,
                ..
            } => build_drilled_items(&self.global_search, current_path, is_zip),
        };
        self.replace_search_view_items(items, image_metas);
        self.update_global_search_address();
    }

    /// SearchContainer をダブルクリックしたときの遷移。
    /// 絞り込みビューに切り替える (docs §10.3 [3] 絞り込みビュー)。
    /// 実フォルダ全体ではなく「検索にヒットしたものだけ (+ ヒットを含む子フォルダ)」
    /// を表示する。
    pub(crate) fn drill_into_container(&mut self, container: PathBuf, is_zip: bool) {
        self.global_search.view = GlobalSearchView::DrilledInto {
            container_root: container.clone(),
            current_path: container,
            is_zip,
        };
        self.rebuild_items_from_global_search();
    }

    /// 絞り込みビューで子フォルダのセルをクリックしたとき、そのフォルダに潜る。
    /// container_root と is_zip は不変、current_path だけ更新する。
    pub(crate) fn drill_into_subfolder(&mut self, sub_path: PathBuf) {
        if let GlobalSearchView::DrilledInto {
            container_root,
            is_zip,
            ..
        } = self.global_search.view.clone()
        {
            self.global_search.view = GlobalSearchView::DrilledInto {
                container_root,
                current_path: sub_path,
                is_zip,
            };
            self.rebuild_items_from_global_search();
        }
    }

    /// Aggregated view に戻る (drill-down 状態から)。
    pub(crate) fn drill_back_to_aggregated(&mut self) {
        self.global_search.view = GlobalSearchView::Aggregated;
        self.rebuild_items_from_global_search();
    }

    /// BS キー (または drill-back ボタン) が押されたときの「一段戻る」処理。
    /// - current_path == container_root: Aggregated ビューに戻る
    /// - container_root の下に居る: 親フォルダに戻る
    pub(crate) fn drill_back_one_level(&mut self) {
        if let GlobalSearchView::DrilledInto {
            container_root,
            current_path,
            is_zip,
        } = self.global_search.view.clone()
        {
            if current_path == container_root {
                self.drill_back_to_aggregated();
            } else if let Some(parent) = current_path.parent() {
                let parent_pb = parent.to_path_buf();
                // container_root 外へは出さない (出るなら Aggregated に戻る)
                if parent_pb.starts_with(&container_root) {
                    self.global_search.view = GlobalSearchView::DrilledInto {
                        container_root,
                        current_path: parent_pb,
                        is_zip,
                    };
                    self.rebuild_items_from_global_search();
                } else {
                    self.drill_back_to_aggregated();
                }
            } else {
                self.drill_back_to_aggregated();
            }
        }
    }

    /// Ctrl+↑↓: 絞り込みビューでヒットを含むフォルダを DFS 順で前後に移動する。
    /// - forward=true: 次のフォルダ
    /// - forward=false: 前のフォルダ
    ///
    /// 現在のコンテナツリーの末端に到達したら **次のコンテナのルートへ跨ぐ**
    /// (docs §10.3 Ctrl+↑↓ が全ヒットを 1 本のフラットリストとして巡回する)。
    /// 全体の先頭/末端まで行ったらそこで停止 (循環はしない)。
    pub(crate) fn global_search_ctrl_nav(&mut self, forward: bool) {
        let (container_root, current_path) = match self.global_search.view.clone() {
            GlobalSearchView::DrilledInto {
                container_root,
                current_path,
                ..
            } => (container_root, current_path),
            _ => return,
        };
        // 全コンテナを走査して「(container_root, path, is_zip)」のフラットリストを作る。
        // 表示順はアグリゲート view と同じ (ヒット件数降順 → 名前昇順)。
        let flat = build_cross_container_nav_list(&self.global_search);
        // 現在位置の突き合わせ: まず (container_root, path) の完全一致を試し、
        // 無ければ path のみで一致させる。後者は、dedup で別 container_root の entry に
        // 統合されたパス (例: `C:/A/sub` コンテナで drill-in したが、flat list では
        // container_root=C:/A 側の entry が残っているケース) に到達するため。
        let pos = flat
            .iter()
            .position(|e| e.container_root == container_root && e.path == current_path)
            .or_else(|| flat.iter().position(|e| e.path == current_path));
        let Some(pos) = pos else { return };
        let next_pos = if forward {
            if pos + 1 < flat.len() { pos + 1 } else { return }
        } else if pos > 0 {
            pos - 1
        } else {
            return;
        };
        let next = &flat[next_pos];
        self.global_search.view = GlobalSearchView::DrilledInto {
            container_root: next.container_root.clone(),
            current_path: next.path.clone(),
            is_zip: next.is_zip,
        };
        self.rebuild_items_from_global_search();
    }

    /// Ctrl+G アドレスバー表示を現在の view に合わせて更新する。
    /// - Aggregated: `🌐 全検索: "query" (N 件)`
    /// - DrilledInto: `🌐 全検索: "query" > container_name > sub_path...`
    pub(crate) fn update_global_search_address(&mut self) {
        if !self.global_search.active {
            return;
        }
        let query = if self.global_search.last_executed.is_empty() {
            self.global_search.query.clone()
        } else {
            self.global_search.last_executed.clone()
        };
        match self.global_search.view.clone() {
            GlobalSearchView::Aggregated => {
                let n = self.global_search.containers.len();
                if query.is_empty() {
                    self.address = "🌐 全検索".to_string();
                } else {
                    self.address = format!("🌐 全検索: \"{query}\"  ({n} 件)");
                }
            }
            GlobalSearchView::DrilledInto {
                container_root,
                current_path,
                ..
            } => {
                // container_root までを 1 セグメントで、その下の相対パスを分解して >表示
                let root_name = container_root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| container_root.to_string_lossy().to_string());
                let rel: Vec<String> = if current_path == container_root {
                    Vec::new()
                } else {
                    current_path
                        .strip_prefix(&container_root)
                        .map(|rel| {
                            rel.components()
                                .map(|c| c.as_os_str().to_string_lossy().to_string())
                                .collect()
                        })
                        .unwrap_or_default()
                };
                let mut segs = vec![root_name];
                segs.extend(rel);
                self.address = format!("🌐 全検索: \"{query}\" > {}", segs.join(" > "));
            }
        }
    }

    /// Ctrl+G トップパネルの描画 (既存 render_favsearch_bar と同パターン)。
    pub(crate) fn render_global_search_bar(&mut self, ctx: &egui::Context) {
        if !self.global_search.active {
            return;
        }
        let raw_enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let escape_pressed = self.dialog_escape_pressed(ctx);

        let mut close_requested = false;
        let mut query_changed = false;

        let mut drill_back = false;
        egui::TopBottomPanel::top("global_search_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                // 絞り込みビュー中は「← 戻る」ボタン + 現在地を表示
                if let GlobalSearchView::DrilledInto { current_path, .. } =
                    self.global_search.view.clone()
                {
                    if ui
                        .button("←")
                        .on_hover_text("1 段戻る (BS でも可)")
                        .clicked()
                    {
                        drill_back = true;
                    }
                    ui.label(
                        egui::RichText::new(format!("📁 {}", current_path.display()))
                            .size(11.0)
                            .color(egui::Color32::from_gray(150)),
                    );
                    ui.separator();
                }
                ui.label("🌐 全検索:");
                let response = ui.add_sized(
                    [360.0, 20.0],
                    egui::TextEdit::singleline(&mut self.global_search.query)
                        .hint_text(r#"お気に入り全体のメタデータ検索 (AND / -除外 / "…")"#),
                );
                if self.global_search.focus_request {
                    self.global_search.focus_request = false;
                    response.request_focus();
                }
                self.global_search.has_focus = response.has_focus();
                if response.changed() {
                    query_changed = true;
                }
                if response.lost_focus() && raw_enter {
                    query_changed = true;
                }
                if ui.small_button("×").on_hover_text("検索を閉じる").clicked() {
                    close_requested = true;
                }

                // 進捗/結果バッジ
                ui.separator();
                if let Some(msg) = &self.global_search.reject_message {
                    ui.label(
                        egui::RichText::new(msg)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(200, 120, 40)),
                    );
                } else if self.global_search.pending.is_some() && !self.global_search.done {
                    let progress = format!(
                        "検索中... {} 件 (候補 {})",
                        self.global_search.total_valid, self.global_search.total_scanned
                    );
                    ui.label(
                        egui::RichText::new(progress)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(180, 180, 80)),
                    );
                } else if self.global_search.done {
                    let text = if self.global_search.truncated {
                        format!(
                            "{} 件で打ち切り (絞り込みキーワードを追加してください)",
                            self.global_search.total_valid
                        )
                    } else {
                        format!("{} 件", self.global_search.total_valid)
                    };
                    let color = if self.global_search.truncated {
                        egui::Color32::from_rgb(200, 140, 40)
                    } else {
                        egui::Color32::from_gray(140)
                    };
                    ui.label(egui::RichText::new(text).size(11.0).color(color));
                }
            });
            ui.add_space(2.0);
        });

        if !self.any_dialog_open() && escape_pressed {
            close_requested = true;
        }
        if close_requested {
            self.close_global_search();
            return;
        }
        if drill_back {
            // ボタン押下は「1 段上げる」で統一 (BS と同じ UX)。
            self.drill_back_one_level();
        }
        if query_changed {
            self.global_search.last_change_at = Some(Instant::now());
            // Codex P3 対応: クエリが変わったら drill state を即 Aggregated に戻し、
            // 旧検索の pending / containers / all_hits も直ちに破棄してから空の
            // Aggregated view として rebuild する (debounce 完了までの間、旧結果で
            // drill-back 判定が残ったり、旧クエリでの rebuild race が起きないように)。
            self.global_search.view = GlobalSearchView::Aggregated;
            self.global_search.pending = None; // SearchHandle::Drop で cancel
            self.global_search.containers.clear();
            self.global_search.all_hits.clear();
            self.global_search.done = false;
            self.global_search.truncated = false;
            self.global_search.total_valid = 0;
            self.global_search.total_scanned = 0;
            self.rebuild_items_from_global_search();
        }
    }
}

// -----------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_container_parses_zip_entries() {
        let (p, k) = parent_container("c:/photos/album.zip!subdir/img.jpg");
        assert_eq!(p, PathBuf::from("c:/photos/album.zip"));
        assert_eq!(k, SearchContainerKind::Zip);
    }

    #[test]
    fn parent_container_parses_normal_files() {
        let (p, k) = parent_container("c:/photos/sunset/IMG.jpg");
        assert_eq!(p, PathBuf::from("c:/photos/sunset"));
        assert_eq!(k, SearchContainerKind::Folder);
    }

    #[test]
    fn accumulate_aggregates_by_container() {
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/b/1.jpg".into(),
            score: 1.0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/b/2.jpg".into(),
            score: 1.0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/c/1.jpg".into(),
            score: 1.0,
        });
        assert_eq!(state.containers.len(), 2);
        let b = state
            .containers
            .get(&PathBuf::from("c:/a/b"))
            .expect("c:/a/b bucket");
        assert_eq!(b.hit_count, 2);
    }

    #[test]
    fn sorted_containers_orders_by_hit_count_desc() {
        let mut map = HashMap::new();
        map.insert(
            PathBuf::from("c:/low"),
            ContainerHit {
                path: "c:/low".into(),
                kind: SearchContainerKind::Folder,
                hit_count: 2,
            },
        );
        map.insert(
            PathBuf::from("c:/high"),
            ContainerHit {
                path: "c:/high".into(),
                kind: SearchContainerKind::Folder,
                hit_count: 10,
            },
        );
        map.insert(
            PathBuf::from("c:/mid"),
            ContainerHit {
                path: "c:/mid".into(),
                kind: SearchContainerKind::Folder,
                hit_count: 5,
            },
        );
        let v = sorted_containers(&map);
        assert_eq!(v[0].path, PathBuf::from("c:/high"));
        assert_eq!(v[1].path, PathBuf::from("c:/mid"));
        assert_eq!(v[2].path, PathBuf::from("c:/low"));
    }

    #[test]
    fn drill_state_transitions_roundtrip() {
        let mut state = GlobalSearchState::default();
        assert_eq!(state.view, GlobalSearchView::Aggregated);
        state.view = GlobalSearchView::DrilledInto {
            container_root: PathBuf::from("c:/photos"),
            current_path: PathBuf::from("c:/photos"),
            is_zip: false,
        };
        assert!(matches!(state.view, GlobalSearchView::DrilledInto { .. }));
        // クエリリセットで drill state もリセットされる契約
        state.reset_for_new_query();
        assert_eq!(state.view, GlobalSearchView::Aggregated);
    }

    #[test]
    fn reset_clears_all_transient_state() {
        // Codex round-9 Should-fix #2 回帰: reset_for_new_query で all_hits / view /
        // done / truncated / total_* が全部クリアされる (close 時に相当の処理を走らせる想定)
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/1.jpg".into(),
            score: 1.0,
        });
        state.view = GlobalSearchView::DrilledInto {
            container_root: PathBuf::from("c:/a"),
            current_path: PathBuf::from("c:/a"),
            is_zip: false,
        };
        state.done = true;
        state.truncated = true;
        state.total_valid = 42;
        state.total_scanned = 100;
        state.reject_message = Some("test".into());

        state.reset_for_new_query();

        assert!(state.all_hits.is_empty());
        assert!(state.containers.is_empty());
        assert_eq!(state.view, GlobalSearchView::Aggregated);
        assert!(!state.done);
        assert!(!state.truncated);
        assert_eq!(state.total_valid, 0);
        assert_eq!(state.total_scanned, 0);
        assert!(state.reject_message.is_none());
    }

    #[test]
    fn all_hits_preserved_for_drill_down() {
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/1.jpg".into(),
            score: 1.0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/2.jpg".into(),
            score: 1.0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/b/x.jpg".into(),
            score: 1.0,
        });
        // containers は 2 つに集約されているが、all_hits は 3 つ保持
        assert_eq!(state.containers.len(), 2);
        assert_eq!(state.all_hits.len(), 3);
    }

    #[test]
    fn accumulate_mixed_folders_and_zips() {
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: "c:/album.zip!0001.jpg".into(),
            score: 1.0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/album.zip!0002.jpg".into(),
            score: 1.0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/photos/x.jpg".into(),
            score: 1.0,
        });
        let zip = state.containers.get(&PathBuf::from("c:/album.zip")).unwrap();
        assert_eq!(zip.hit_count, 2);
        assert_eq!(zip.kind, SearchContainerKind::Zip);
        let folder = state.containers.get(&PathBuf::from("c:/photos")).unwrap();
        assert_eq!(folder.hit_count, 1);
        assert_eq!(folder.kind, SearchContainerKind::Folder);
    }

    // ヒット: C:/root/a.jpg, C:/root/sub/b.jpg, C:/root/sub/deeper/c.jpg
    // ドリルルート: C:/root → DFS 順列は [root, sub, sub/deeper]。
    #[test]
    fn collect_hit_folders_dfs_is_preorder() {
        let hits = vec![
            GlobalHit { path: "C:/root/a.jpg".into(), score: 1.0 },
            GlobalHit { path: "C:/root/sub/b.jpg".into(), score: 1.0 },
            GlobalHit { path: "C:/root/sub/deeper/c.jpg".into(), score: 1.0 },
        ];
        let got = collect_hit_folders_dfs(&hits, &PathBuf::from("C:/root"));
        assert_eq!(
            got,
            vec![
                PathBuf::from("C:/root"),
                PathBuf::from("C:/root/sub"),
                PathBuf::from("C:/root/sub/deeper"),
            ]
        );
    }

    // 同階層 2 兄弟のうち片方にしかヒットがない場合、枝刈りされる。
    #[test]
    fn collect_hit_folders_dfs_prunes_empty_branches() {
        let hits = vec![GlobalHit {
            path: "C:/root/yes/found.jpg".into(),
            score: 1.0,
        }];
        let got = collect_hit_folders_dfs(&hits, &PathBuf::from("C:/root"));
        // "no" サブフォルダはヒットを持たないので列挙されない
        assert_eq!(
            got,
            vec![PathBuf::from("C:/root"), PathBuf::from("C:/root/yes")]
        );
    }

    // build_cross_container_nav_list: 複数コンテナの DFS を順番通りに平坦化。
    // Ctrl+↓ が container1 末端から container2 に跨る挙動の根拠。
    #[test]
    fn nav_list_crosses_container_boundary_dfs_order() {
        let mut state = GlobalSearchState::default();
        // container A (ヒット 3) → B (ヒット 1) の順。A は sub を持つ。
        for p in [
            "C:/A/a1.jpg",
            "C:/A/sub/a2.jpg",
            "C:/A/sub/a3.jpg",
            "C:/B/b1.jpg",
        ] {
            state.accumulate_hit(&GlobalHit { path: p.into(), score: 1.0 });
        }
        let flat = build_cross_container_nav_list(&state);
        let paths: Vec<_> = flat.iter().map(|e| e.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("C:/A"),
                PathBuf::from("C:/A/sub"),
                PathBuf::from("C:/B"),
            ]
        );
        // container_root も正しく紐づいている (境界跨ぎ検証)
        assert_eq!(flat[0].container_root, PathBuf::from("C:/A"));
        assert_eq!(flat[1].container_root, PathBuf::from("C:/A"));
        assert_eq!(flat[2].container_root, PathBuf::from("C:/B"));
    }

    // ZIP コンテナは内部展開せず 1 エントリで計上 (v0.8.0 方針)。
    #[test]
    fn nav_list_zip_containers_are_flat_entries() {
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: "C:/book.zip!img1.jpg".into(),
            score: 1.0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "C:/book.zip!img2.jpg".into(),
            score: 1.0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "C:/folder/a.jpg".into(),
            score: 1.0,
        });
        let flat = build_cross_container_nav_list(&state);
        // ヒット件数降順: book.zip (2) → folder (1)
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].path, PathBuf::from("C:/book.zip"));
        assert!(flat[0].is_zip);
        assert_eq!(flat[1].path, PathBuf::from("C:/folder"));
        assert!(!flat[1].is_zip);
    }

    // build_drilled_items: current_path に直接ヒット + ヒット持ちサブを Folder として
    // 並べる。current_path と異なる兄弟 (別 container) のヒットは無視される。
    #[test]
    fn build_drilled_items_mixes_subfolders_and_direct_files() {
        let mut state = GlobalSearchState::default();
        for p in [
            "C:/root/a.jpg",
            "C:/root/b.jpg",
            "C:/root/sub/c.jpg",
            "C:/other/z.jpg", // 別ツリー, current_path 配下ではない
        ] {
            state.accumulate_hit(&GlobalHit { path: p.into(), score: 1.0 });
        }
        let (items, metas) = build_drilled_items(&state, Path::new("C:/root"), false);
        assert_eq!(items.len(), metas.len());
        // 期待: Folder("C:/root/sub") + Image("C:/root/a.jpg") + Image("C:/root/b.jpg")
        let paths: Vec<_> = items
            .iter()
            .map(|it| match it {
                GridItem::Folder(p) => ("Folder", p.clone()),
                GridItem::Image(p) => ("Image", p.clone()),
                _ => ("Other", PathBuf::new()),
            })
            .collect();
        assert_eq!(paths[0], ("Folder", PathBuf::from("C:/root/sub")));
        assert_eq!(paths[1], ("Image", PathBuf::from("C:/root/a.jpg")));
        assert_eq!(paths[2], ("Image", PathBuf::from("C:/root/b.jpg")));
    }
}
