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

use uuid::Uuid;

use crate::app::App;
use crate::fts_index::{IndexKind, SearchTarget, SourceKind};
use crate::global_search::{DoneReason, GlobalHit, SearchStreamEvent};
use crate::grid_item::{ContainerRepresentative, GridItem, SearchContainerKind};
use crate::indexer_manager::SearchHandle;

// -----------------------------------------------------------------------
// 検索フィルタ (§19.7 ドロップダウン対応)
// -----------------------------------------------------------------------

/// Ctrl+G の 3 ドロップダウン (お気に入り / タイプ / 検索対象) を保持する (§19.2)。
/// 変更時は `reset_for_new_query` と同じ経路で検索を再実行する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalSearchFilters {
    /// None = 登録済み全お気に入りを対象。Some(id) なら単一 favorite に限定。
    pub favorite: Option<Uuid>,
    /// None = 全タイプ (folder/image/zip/pdf)。Some(k) で単一種別に限定。
    pub kind: Option<IndexKind>,
    /// 検索対象ソース。既定は All (= 全ソース OR)。
    pub target: SearchTarget,
    /// OR 検索モード (docs §20)。true で include トークンを OR 結合 (NOT は AND のまま)。
    pub or_mode: bool,
}

impl Default for GlobalSearchFilters {
    fn default() -> Self {
        Self {
            favorite: None,
            kind: None,
            target: SearchTarget::All,
            or_mode: false,
        }
    }
}

/// ドロップダウン表示用ラベル (UI のみで使う)。
pub fn kind_label(k: IndexKind) -> &'static str {
    match k {
        IndexKind::Folder => "フォルダ",
        IndexKind::Image => "画像",
        IndexKind::Zip => "ZIP ファイル",
        IndexKind::Pdf => "PDF ファイル",
    }
}

/// 「検索対象」ドロップダウンに出すソース選択肢 (単一 source / All)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetChoice {
    All,
    Only(SourceKind),
}

impl TargetChoice {
    pub fn label(self) -> &'static str {
        match self {
            TargetChoice::All => "すべての対象",
            TargetChoice::Only(SourceKind::Filename) => "ファイル名",
            TargetChoice::Only(SourceKind::Exif) => "EXIF",
            TargetChoice::Only(SourceKind::XmpTweet) => "mXD ツイート情報",
            TargetChoice::Only(SourceKind::PngPrompt) => "AI プロンプト (PNG)",
            TargetChoice::Only(SourceKind::PdfMeta) => "PDF メタ情報",
            TargetChoice::Only(SourceKind::Tags) => "タグ",
        }
    }

    pub fn to_target(self) -> SearchTarget {
        match self {
            TargetChoice::All => SearchTarget::All,
            TargetChoice::Only(s) => SearchTarget::Only(vec![s]),
        }
    }

    pub fn from_target(t: &SearchTarget) -> Self {
        match t {
            SearchTarget::All => TargetChoice::All,
            SearchTarget::Only(v) if v.len() == 1 => TargetChoice::Only(v[0]),
            // 複数選択は v1 UI ではサポートしない (内部表現としては可能)。All にフォールバック。
            SearchTarget::Only(_) => TargetChoice::All,
        }
    }
}

/// ドロップダウン列挙用。順序はラベル辞書ではなく UI 上の表示順 (ユーザーがよく使う順)。
pub const TARGET_CHOICES: &[TargetChoice] = &[
    TargetChoice::All,
    TargetChoice::Only(SourceKind::Filename),
    TargetChoice::Only(SourceKind::Exif),
    TargetChoice::Only(SourceKind::XmpTweet),
    TargetChoice::Only(SourceKind::PngPrompt),
    TargetChoice::Only(SourceKind::PdfMeta),
    TargetChoice::Only(SourceKind::Tags),
];

pub const KIND_CHOICES: &[Option<IndexKind>] = &[
    None,
    Some(IndexKind::Folder),
    Some(IndexKind::Image),
    Some(IndexKind::Zip),
    Some(IndexKind::Pdf),
];

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
    /// 絞り込みフィルタ (§19.7)。変更時は自動的に検索を再実行する。
    pub filters: GlobalSearchFilters,
    /// アグリゲートビューのコンテナソート順 (v0.8.1)。既定は HitCount。
    pub sort_mode: ContainerSortMode,
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
            filters: GlobalSearchFilters::default(),
            sort_mode: ContainerSortMode::HitCount,
        }
    }
}

/// 集約済みコンテナ (v1 トップレベルビュー用)。
#[derive(Clone, Debug)]
pub struct ContainerHit {
    pub path: PathBuf,
    pub kind: SearchContainerKind,
    pub hit_count: usize,
    /// 代表サムネ候補 (v0.8.1): ヒットのうち最初にサムネ表示可能だった 1 件。
    /// 画像拡張子ヒットがまったくない (= PDF メタ / フォルダ名ヒットだけ) コンテナは None のまま。
    pub representative: Option<ContainerRepresentative>,
    /// コンテナ (フォルダ / ZIP ファイル) の最終更新時刻 (UNIX 秒)。
    /// Newer/Older ソートで参照されるときに遅延取得される (fs::metadata 同期呼び出し)。
    /// HitCount / Name ソートでは使わないので埋まらない。
    pub mtime: Option<i64>,
}

/// Ctrl+G 結果ビューの並び順 (v0.8.1)。
/// 既定は HitCount (件数の多いコンテナから先に見せる)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerSortMode {
    /// ヒット件数降順 → パス昇順 (既定)
    HitCount,
    /// パス昇順
    Name,
    /// mtime 降順 (新しい順)
    Newer,
    /// mtime 昇順 (古い順)
    Older,
}

impl ContainerSortMode {
    pub fn label(self) -> &'static str {
        match self {
            ContainerSortMode::HitCount => "件数順",
            ContainerSortMode::Name => "名前順",
            ContainerSortMode::Newer => "新しい順",
            ContainerSortMode::Older => "古い順",
        }
    }
}

pub const SORT_MODES: &[ContainerSortMode] = &[
    ContainerSortMode::HitCount,
    ContainerSortMode::Name,
    ContainerSortMode::Newer,
    ContainerSortMode::Older,
];

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
    pub(crate) fn accumulate_hit(&mut self, hit: &GlobalHit) {
        let (container_path, kind) = parent_container(&hit.path);
        let representative = image_representative_from_hit(&hit.path);
        let entry = self
            .containers
            .entry(container_path.clone())
            .or_insert_with(|| ContainerHit {
                path: container_path,
                kind,
                hit_count: 0,
                representative: None,
                mtime: None,
            });
        entry.hit_count += 1;
        // サムネ対象のヒットがまだ確定していなければ、今回の候補で埋める (先着優先)。
        if entry.representative.is_none() {
            entry.representative = representative;
        }
        // drill-down 用に生のヒットも保持 (path で後でフィルタする)
        self.all_hits.push(hit.clone());
    }
}

/// GlobalHit のパスから「サムネ表示できる代表」の情報を抽出する。
///
/// 画像ファイル / ZIP 内画像 / PDF ファイルのいずれかならサムネイル化できるため
/// 代表として採用する。それ以外 (未対応拡張子 / フォルダ名ヒットのみ) は None。
fn image_representative_from_hit(hit_path: &str) -> Option<ContainerRepresentative> {
    let (file_part, entry) = match hit_path.split_once('!') {
        Some((zip, ent)) => (zip, Some(ent.to_string())),
        None => (hit_path, None),
    };
    // ZIP エントリなら entry 側、通常ファイルなら file_part から拡張子を取る
    let name_for_ext = entry.as_deref().unwrap_or(file_part);
    let ext = Path::new(name_for_ext)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)?;
    // PDF は 1 ページ目をサムネに。ScanSnap 等 PDF だらけのフォルダでも
    // コンテナに画像アイコンだけでなく中身プレビューが出るようになる。
    if ext == "pdf" {
        return Some(ContainerRepresentative {
            path: PathBuf::from(file_part),
            zip_entry: None,
            pdf_page: Some(0),
        });
    }
    if !crate::folder_tree::is_recognized_image_ext(&ext) {
        return None;
    }
    Some(ContainerRepresentative {
        path: PathBuf::from(file_part),
        zip_entry: entry,
        pdf_page: None,
    })
}

/// ヒット path から親コンテナを決定する (docs §10.4.2)。
/// - ZIP エントリ (`<zippath>!<entry>` 形式) → ZIP ファイルパス
/// - 通常ファイル → 親フォルダパス
fn parent_container(hit_path: &str) -> (PathBuf, SearchContainerKind) {
    if let Some((zip_part, _entry)) = hit_path.split_once('!') {
        return (PathBuf::from(zip_part), SearchContainerKind::Zip);
    }
    let p = PathBuf::from(hit_path);
    if let Some(parent) = p.parent() {
        (parent.to_path_buf(), SearchContainerKind::Folder)
    } else {
        (p, SearchContainerKind::Folder)
    }
}

/// 指定したモードで ContainerHit をソートして返す。
/// Newer/Older 時は `mtime` フィールドを使うので、呼び出し側は
/// [`ensure_container_mtime_populated`] で事前に埋めておくこと。
pub fn sort_containers_with_mode(
    containers: &HashMap<PathBuf, ContainerHit>,
    mode: ContainerSortMode,
) -> Vec<ContainerHit> {
    let mut v: Vec<ContainerHit> = containers.values().cloned().collect();
    match mode {
        ContainerSortMode::HitCount => v.sort_by(|a, b| {
            b.hit_count
                .cmp(&a.hit_count)
                .then_with(|| a.path.cmp(&b.path))
        }),
        ContainerSortMode::Name => v.sort_by(|a, b| a.path.cmp(&b.path)),
        ContainerSortMode::Newer => v.sort_by(|a, b| {
            // mtime 不明 (None) は「最古」扱いで末尾に送る
            let ka = a.mtime.unwrap_or(i64::MIN);
            let kb = b.mtime.unwrap_or(i64::MIN);
            kb.cmp(&ka).then_with(|| a.path.cmp(&b.path))
        }),
        ContainerSortMode::Older => v.sort_by(|a, b| {
            let ka = a.mtime.unwrap_or(i64::MAX);
            let kb = b.mtime.unwrap_or(i64::MAX);
            ka.cmp(&kb).then_with(|| a.path.cmp(&b.path))
        }),
    }
    v
}

/// Newer/Older ソートのために、mtime 未取得のコンテナだけ fs::metadata を呼んで埋める。
/// HitCount/Name モードでは何もしない。
///
/// **ストリーミング中 (`state.done == false`) はスキップする** (Codex P3 対応):
/// 検索結果が逐次 append されるたびに本関数が呼ばれるため、そのタイミングで毎回
/// 新コンテナ全部に fs::metadata を掛けると HDD / ネットワーク / 大量お気に入り環境
/// で UI がカクつく。ストリーム完了時 (done=true → rebuild) に 1 回まとめて取得し、
/// 以降はキャッシュが効くので `sort_mode` 切替は即時反映になる。ストリーミング中の
/// Newer/Older ソートは mtime 不明のため path 順で並ぶが、done 後に正しい順序へ snap する。
pub fn ensure_container_mtime_populated(state: &mut GlobalSearchState) {
    if !matches!(
        state.sort_mode,
        ContainerSortMode::Newer | ContainerSortMode::Older
    ) {
        return;
    }
    if !state.done {
        return;
    }
    for c in state.containers.values_mut() {
        if c.mtime.is_some() {
            continue;
        }
        c.mtime = std::fs::metadata(&c.path)
            .ok()
            .map(|m| crate::ui_helpers::mtime_secs(&m));
    }
}

// -----------------------------------------------------------------------
// 絞り込みビュー (DrilledInto) 用のアイテム構築ヘルパ
// -----------------------------------------------------------------------

/// Aggregated view の items + image_metas を組み立てる。
pub(crate) fn build_aggregated_items(
    state: &mut GlobalSearchState,
) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>) {
    // Newer/Older ソートのとき mtime を遅延取得 (初回のみ fs::metadata 同期呼び出し)。
    ensure_container_mtime_populated(state);
    let containers = sort_containers_with_mode(&state.containers, state.sort_mode);
    let items: Vec<GridItem> = containers
        .iter()
        .map(|c| GridItem::SearchContainer {
            path: c.path.clone(),
            kind: c.kind,
            hit_count: c.hit_count,
            representative: c.representative.clone(),
        })
        .collect();
    // 代表サムネを出すコンテナの場合、thumb_loader は `image_metas[i]` が Some で
    // ないと enqueue 前に skip してしまう (`app.rs` の keep_start..keep_end ループ)。
    // build_drilled_items と同じ方針で placeholder `(0, 0)` を使う:
    // キャッシュキーはパスを含むので衝突せず、mtime ベースの invalidate が効かない
    // 副作用は一時ビューのため許容 (通常フォルダ閲覧で新しいサムネに差し替わる)。
    // representative が None のコンテナは make_load_request が None を返すので
    // placeholder があっても何も起きない。
    let placeholder = Some((0_i64, 0_i64));
    let image_metas: Vec<Option<(i64, i64)>> = vec![placeholder; items.len()];
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
        let Some(hp_parent) = hp.parent() else {
            continue;
        };
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

    // image_metas は placeholder (0,0) を使う。path_metadata の同期 fs::metadata は
    // 大量ヒット (最大 HARD_MAX=10000) で UI を止めるため避ける。キャッシュキーはパスを
    // 含むので衝突せず、mtime ベースの invalidate が効かない副作用は一時ビューのため許容。
    let placeholder = Some((0_i64, 0_i64));
    for (sub_path, _hits) in &sub_vec {
        items.push(GridItem::Folder(sub_path.clone()));
        image_metas.push(placeholder);
    }
    for f in &direct_files {
        // 拡張子で GridItem の種類を分岐する。旧実装は無条件に
        // `GridItem::Image` を入れていたため、ScanSnap のような PDF だらけの
        // favorite に drill-in すると全サムネが「画像フォーマット判定不可」で
        // 失敗する現象があった (2026-04 ユーザー報告)。
        let ext = f
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let item = match ext.as_str() {
            "pdf" => GridItem::PdfFile(f.clone()),
            "zip" => GridItem::ZipFile(f.clone()),
            _ => GridItem::Image(f.clone()),
        };
        items.push(item);
        image_metas.push(placeholder);
    }

    (items, image_metas)
}

/// ZIP コンテナをドリルインしたときのアイテム構築 (v0.8.0 はフラット表示)。
fn build_drilled_zip_items(
    state: &GlobalSearchState,
    zip_path: &Path,
) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>) {
    let zip_key = crate::search_index_db::normalize_path(zip_path);
    // sync I/O 除去 (フォルダ版と同じ理由)。
    let placeholder = Some((0_i64, 0_i64));
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
        image_metas.push(placeholder);
    }
    (items, image_metas)
}

/// `child` が `ancestor` と等しいか配下にあれば true。
fn path_is_under_or_eq(child: &Path, ancestor: &Path) -> bool {
    child == ancestor || child.starts_with(ancestor)
}

/// フルスクリーンで開ける画像系アイテムか。Ctrl+↑↓ の飛び先判定に使う。
fn is_fullscreen_target(item: Option<&GridItem>) -> bool {
    matches!(
        item,
        Some(GridItem::Image(_)) | Some(GridItem::ZipImage { .. }) | Some(GridItem::PdfPage { .. })
    )
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
    // ナビ順は表示順と揃える。mtime は `build_aggregated_items` が必ず事前に populate
    // しているので、ここでは mut を要求せず既存の mtime キャッシュを参照するだけ。
    let containers = sort_containers_with_mode(&state.containers, state.sort_mode);
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
        let Some(mut cur) = hp.parent().map(Path::to_path_buf) else {
            continue;
        };
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
        use std::sync::atomic::Ordering;
        debug_assert_eq!(items.len(), image_metas.len());
        // 旧タスク停止: インデックスが付け替わるので in-flight は意味を失う
        if let Some(pending) = self.search_pending.take() {
            pending.cancel();
        }
        if let Some(pending) = self.metadata_pending.take() {
            pending.cancel();
        }
        // items_generation bump + thumbnails 初期化を一箇所に集約。
        // これ以降に届く旧ワーカーの ThumbMsg は poll_thumbnails の
        // 世代不一致チェックで破棄される。
        self.install_new_items(items, image_metas);
        self.selected = None;
        // idx ベース状態 + キュー排水 (requested / pending_finalize / texture_backlog /
        // keep_* / rotation / rating / adjustment_cache / thumb_pixels / ai_* / fs_* /
        // reload_queue / heavy_io_queue / tag_prewarm_*) を一括破棄。
        self.invalidate_idx_state_and_queues();
        // 補正・マスクも idx ベース。Ctrl+G は items が総入れ替わりするので
        // 旧フォルダの個別設定は意味を失うため clear する。
        // (削除経路では呼び出し元が idx shift で保持する)
        self.adjustment_page_params.clear();
        self.mask_pages.clear();
        // path-keyed キャッシュも Ctrl+G では items が総入れ替わりするのでリセット。
        self.metadata_cache.clear();
        self.exif_cache.clear();
        self.xmp_cache.clear();
        self.tags_cache.clear();
        // Ctrl+F フィルタの残留を解除 (Ctrl+G と共存させない)
        self.search_filter = None;
        self.search_query.clear();
        self.scroll_offset_y = 0.0;
        self.scroll_to_selected = false;
        self.scroll_hint.store(0, Ordering::Relaxed);
        self.rebuild_visible_indices();
        // tag_prewarm worker は invalidate_idx_state_and_queues で cancel されているので、
        // 検索結果向けに再起動 + fts_meta から tags_cache を再プリウォーム。
        self.prewarm_grid_tags();
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
        // 他の検索バー (Ctrl+F / Ctrl+S) が開いていれば閉じる (相互排他)
        self.close_other_search_bars(crate::app::SearchMode::Global);
        self.global_search.active = true;
        self.global_search.focus_request = true;
        self.global_search.saved_folder = self.current_folder.clone();
        // Ctrl+G 中は current_folder_rating() が 0 を返す規約なので、
        // 旧フォルダの★が残っているとアドレスバーにちらつく。
        self.current_folder_rating_cache = None;
    }

    pub(crate) fn close_global_search(&mut self) {
        if !self.global_search.active {
            return;
        }
        // pending があれば SearchHandle の Drop impl で cancel される
        self.global_search.pending = None;
        self.global_search.active = false;
        self.global_search.has_focus = false;
        // Ctrl+G 中に 0 で埋めたキャッシュを破棄して、復帰先フォルダの実値を再計算させる。
        self.current_folder_rating_cache = None;
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

        // Codex P2 #3: indexer_manager の有無とは独立に filter の健全化を先に行う。
        // 選択中の favorite が削除された / auto_index_metadata を外された場合、UI ラベルと
        // 検索スコープが食い違う (ラベル = 名前表示、スコープ = 全対象) のを避けるため
        // フィルタ側を None に倒して UI も「すべて」に戻す。
        let all_favs: Vec<uuid::Uuid> = self
            .settings
            .favorites
            .iter()
            .filter(|f| f.auto_index_metadata)
            .map(|f| f.id)
            .collect();
        if let Some(id) = self.global_search.filters.favorite {
            if !all_favs.contains(&id) {
                self.global_search.filters.favorite = None;
            }
        }

        let Some(mgr) = self.indexer_manager.as_ref() else {
            self.global_search.reject_message =
                Some("全文検索インデクサが利用できません".to_string());
            self.global_search.done = true;
            self.rebuild_items_from_global_search();
            return;
        };

        let favs: Vec<uuid::Uuid> = match self.global_search.filters.favorite {
            Some(id) => vec![id],
            None => all_favs,
        };
        let scope = crate::global_search::SearchScope {
            kinds: self.global_search.filters.kind.map(|k| vec![k]),
            target: self.global_search.filters.target.clone(),
            mode: self.global_search.filters.or_mode.into(),
        };

        let handle = mgr.spawn_search(self.global_search.query.clone(), favs, scope);
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
        // Codex P3 対応: DrilledInto ビューで done=true rebuild を迎えたとき、
        // `build_drilled_items` は `ensure_container_mtime_populated` を呼ばないため
        // mtime が埋まらないまま残る。結果、その状態で `build_cross_container_nav_list`
        // (Ctrl+↑↓ で呼ばれる) が Newer/Older ソートを mtime=None で走らせてしまう。
        // view に関わらず rebuild の入口で populate しておく (done 未達なら関数内で no-op)。
        ensure_container_mtime_populated(&mut self.global_search);
        let (items, image_metas) = match self.global_search.view.clone() {
            GlobalSearchView::Aggregated => build_aggregated_items(&mut self.global_search),
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

    /// Ctrl+G 絞り込みビューで PDF / ZIP / (通常の load_folder 経路に載るサブフォルダ) を
    /// 開いた時点で、`current_path` をその path に進めるためのヘルパ。
    ///
    /// 2026-04 ユーザー報告バグ: 「Ctrl+G → folder drill → PDF を開く → BS で戻ると
    /// PDF 一覧をスキップして Aggregated に飛んでしまう」の修正。
    ///
    /// 仕組み: `drill_back_one_level` は
    /// - `current_path == container_root` なら Aggregated に戻る
    /// - そうでなければ `current_path.parent()` に戻る
    /// という状態機械。PDF を開いた時点で `current_path=pdf_path` に進めておけば、
    /// BS で `parent(pdf_path)=folder_path` に戻り、folder の drilled view
    /// (ヒット一覧) が再描画される。2 段目 BS で Aggregated に戻る。
    ///
    /// `drill_into_subfolder` と違って `rebuild_items_from_global_search` は
    /// **呼ばない**。呼び出し側の `load_pdf_as_folder` / `load_zip_as_folder` /
    /// `load_folder` が items を PDF ページ / ZIP エントリ / フォルダ内容で埋める
    /// ため、ここで drilled view の items に書き戻すと PDF ページ表示が潰れる。
    ///
    /// `global_search.active` が false、または `view == Aggregated` のときは no-op。
    pub(crate) fn advance_drilled_current_path(&mut self, p: &Path) {
        if !self.global_search.active {
            return;
        }
        if let GlobalSearchView::DrilledInto {
            container_root,
            is_zip,
            ..
        } = self.global_search.view.clone()
        {
            self.global_search.view = GlobalSearchView::DrilledInto {
                container_root,
                current_path: p.to_path_buf(),
                is_zip,
            };
            // 新しい current_path をブレッドクラムに反映。load_pdf_as_folder 等が
            // 後で self.address を raw パスで上書きするが、そこでも再度
            // `update_global_search_address` を呼んで元に戻す構造にしているので
            // 最終的にこのブレッドクラムが表示される。
            self.update_global_search_address();
        }
    }

    /// Aggregated view に戻る (drill-down 状態から)。
    pub(crate) fn drill_back_to_aggregated(&mut self) {
        // Ctrl+G drill-back は load_folder を経由しないため、suppression の subtree
        // 外判定が走らない。ユーザー視点では「本から出た」ので復元する (Codex High 指摘)。
        self.restore_rating_filter_suppression();
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
                    // 上の階層へ移動するだけでも、★コンテナを開いていた場合は
                    // 「本から外へ」出る方向なので suppression 復元を試みる
                    // (Codex High 指摘: load_folder を経由しない経路で leak するのを防ぐ)。
                    self.restore_rating_filter_suppression();
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

    /// フルスクリーン中の Ctrl+↑↓: 絞り込みビューの next/prev フォルダに跨って
    /// 移動し、その先頭 (forward) または末尾 (backward) の画像アイテムを
    /// そのままフルスクリーンで開く。fs ツリー DFS (start_folder_nav) は検索
    /// コンテナの外に出てしまうので、Ctrl+G 中はこちらのルートを使う。
    ///
    /// 移動先に直接のヒット画像が 1 枚も無い (サブフォルダ配下にしかない) ケースは
    /// スキップしてさらに次の候補に進む。画像が見つかるまで前後方向に進み、
    /// フラットリスト全体に画像が無ければ元の位置に戻して何もしない。
    pub(crate) fn global_search_ctrl_nav_fullscreen(&mut self, forward: bool) {
        let before_view = self.global_search.view.clone();
        loop {
            let prev_view = self.global_search.view.clone();
            self.global_search_ctrl_nav(forward);
            if self.global_search.view == prev_view {
                // これ以上進めない → 元の view に戻す (fs_cache を綺麗に戻すため再 rebuild)
                if self.global_search.view != before_view {
                    self.global_search.view = before_view;
                    self.rebuild_items_from_global_search();
                }
                // 「最後/最初の検索結果です」ヒントを中央に表示
                self.fs_boundary_hint = Some(crate::ui_fullscreen::FsBoundaryHint::SearchEnd {
                    forward,
                    at: std::time::Instant::now(),
                });
                return;
            }
            // rebuild_items_from_global_search 済みなので visible_indices を見て
            // 画像アイテムがあるか判定する。無ければ次の候補へ。
            let image_idx = if forward {
                self.visible_indices
                    .iter()
                    .copied()
                    .find(|&i| is_fullscreen_target(self.items.get(i)))
            } else {
                self.visible_indices
                    .iter()
                    .copied()
                    .rev()
                    .find(|&i| is_fullscreen_target(self.items.get(i)))
            };
            if let Some(idx) = image_idx {
                self.open_fullscreen(idx);
                self.selected = Some(idx);
                self.scroll_to_selected = true;
                self.update_last_selected_image();
                return;
            }
            // 画像が無い (Folder 枝しかない) 候補はスキップしてさらに次へ
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
            if pos + 1 < flat.len() {
                pos + 1
            } else {
                return;
            }
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
        let mut filter_changed = false;
        let mut sort_changed = false;

        let mut drill_back = false;
        egui::TopBottomPanel::top("global_search_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            // 絞り込みビュー中は「← 戻る」+ 現在地を検索バーの **上** の行に表示する
            // (下の検索バーの入力欄位置が他のモードとブレないようにするため)
            if let GlobalSearchView::DrilledInto { current_path, .. } =
                self.global_search.view.clone()
            {
                ui.horizontal(|ui| {
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
                });
            }
            ui.horizontal(|ui| {
                ui.label("検索:").on_hover_text(
                    "Ctrl+G はお気に入りの「メタ情報索引」を使って検索します。\n\
                     メタ情報索引が作成されていないお気に入りは対象になりません。\n\
                     お気に入り編集で「メタ情報を索引化する」を有効にしてください。",
                );
                let response = ui.add_sized(
                    [320.0, 20.0],
                    egui::TextEdit::singleline(&mut self.global_search.query)
                        .hint_text(r#"お気に入り配下のメタデータ (AND / -除外 / "…")"#),
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
                // タグピッカー (docs/tag-feature.md Phase D)
                // 登録済みタグを一覧からワンクリックで `#タグ名` として検索クエリに追記する
                let tags_snapshot = self.settings.tags.clone();
                if tags_snapshot.is_empty() {
                    ui.add_enabled(false, egui::Button::new("# タグ…"))
                        .on_hover_text(
                            "メニュー「タグ」→「タグを編集…」から先にタグを\n\
                             登録すると、ここから選択できるようになります。",
                        );
                } else {
                    ui.menu_button("# タグ…", |ui| {
                        ui.set_min_width(160.0);
                        for t in &tags_snapshot {
                            let already_in_query =
                                query_contains_tag(&self.global_search.query, &t.name);
                            let label = if already_in_query {
                                format!("✓ #{}", t.name)
                            } else {
                                format!("  #{}", t.name)
                            };
                            if ui.button(label).clicked() {
                                if !already_in_query {
                                    append_tag_to_query(
                                        &mut self.global_search.query,
                                        &t.name,
                                    );
                                    query_changed = true;
                                }
                                ui.close();
                            }
                        }
                    });
                }

                if ui.small_button("×").on_hover_text("検索を閉じる").clicked() {
                    close_requested = true;
                }

                // ── 絞り込みドロップダウン (§19.7) ──
                // お気に入り (auto_index_metadata=true のもののみ候補にする)
                {
                    let current = self.global_search.filters.favorite;
                    let label_for = |opt: Option<Uuid>| -> String {
                        match opt {
                            None => "すべてのお気に入り".to_string(),
                            Some(id) => self
                                .settings
                                .favorite_by_id(id)
                                .map(|f| f.name.clone())
                                .unwrap_or_else(|| "(削除済)".to_string()),
                        }
                    };
                    let mut next = current;
                    egui::ComboBox::from_id_salt("global_search_fav")
                        .selected_text(label_for(current))
                        .width(160.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut next, None, "すべてのお気に入り");
                            for fav in &self.settings.favorites {
                                if !fav.auto_index_metadata {
                                    continue;
                                }
                                ui.selectable_value(&mut next, Some(fav.id), &fav.name);
                            }
                        });
                    if next != current {
                        self.global_search.filters.favorite = next;
                        filter_changed = true;
                    }
                }

                // タイプ
                {
                    let current = self.global_search.filters.kind;
                    let label_for = |opt: Option<IndexKind>| -> &'static str {
                        match opt {
                            None => "すべての種類",
                            Some(k) => kind_label(k),
                        }
                    };
                    let mut next = current;
                    egui::ComboBox::from_id_salt("global_search_kind")
                        .selected_text(label_for(current))
                        .width(140.0)
                        .show_ui(ui, |ui| {
                            for &choice in KIND_CHOICES {
                                ui.selectable_value(&mut next, choice, label_for(choice));
                            }
                        });
                    if next != current {
                        self.global_search.filters.kind = next;
                        filter_changed = true;
                    }
                }

                // 検索対象
                {
                    let current = TargetChoice::from_target(&self.global_search.filters.target);
                    let mut next = current;
                    egui::ComboBox::from_id_salt("global_search_target")
                        .selected_text(current.label())
                        .width(160.0)
                        .show_ui(ui, |ui| {
                            for &choice in TARGET_CHOICES {
                                ui.selectable_value(&mut next, choice, choice.label());
                            }
                        });
                    if next != current {
                        self.global_search.filters.target = next.to_target();
                        filter_changed = true;
                    }
                }

                if crate::ui_helpers::or_mode_checkbox(
                    ui,
                    &mut self.global_search.filters.or_mode,
                ) {
                    filter_changed = true;
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

                // ── ソート切替 (Aggregated ビューのみ) ──
                // 件数バッジの右側で「ソート: <現在のモード>」のドロップダウンを出す。
                // DrilledInto 中は Aggregated のソートが影響しないので隠す。
                if matches!(self.global_search.view, GlobalSearchView::Aggregated)
                    && !self.global_search.containers.is_empty()
                {
                    ui.separator();
                    ui.label(egui::RichText::new("ソート:").size(11.0).weak());
                    let current = self.global_search.sort_mode;
                    let mut next = current;
                    egui::ComboBox::from_id_salt("global_search_sort")
                        .selected_text(current.label())
                        .width(90.0)
                        .show_ui(ui, |ui| {
                            for &mode in SORT_MODES {
                                ui.selectable_value(&mut next, mode, mode.label());
                            }
                        })
                        .response
                        .on_hover_text(
                            "新しい/古い: コンテナの更新日時順 (初回選択時に\n\
                             fs::metadata を一括取得するので HDD では一瞬固まります)",
                        );
                    if next != current {
                        self.global_search.sort_mode = next;
                        // アグリゲート view を即時再ソート (mtime 未取得なら
                        // build_aggregated_items 側で populate される)
                        sort_changed = true;
                    }
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
        if filter_changed {
            // ドロップダウン変更は debounce せず即座に再実行する
            // (ユーザーの操作は明示的なので待つ必要がない)
            if !self.global_search.query.trim().is_empty() {
                self.global_search.last_executed.clear(); // 強制再実行
                self.spawn_global_search();
            }
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
            // query == last_executed でも debounce → spawn を必ず再走させる。
            // そうしないと、Enter 2 連打で旧検索が cancel されたあと
            // poll_global_search_debounce が「クエリが変わっていない」と判定して
            // 新 spawn を skip し、結果 0 件のまま固着する。
            self.global_search.last_executed.clear();
            self.rebuild_items_from_global_search();
        }
        if sort_changed {
            // ソート変更はクエリ再実行不要 — items を並べ替えるだけ。
            self.rebuild_items_from_global_search();
        }
    }
}

// -----------------------------------------------------------------------
// タグピッカー用ヘルパー (docs/tag-feature.md Phase D)
// -----------------------------------------------------------------------

/// 検索クエリに `#tag` トークンが既に含まれているか (完全一致、空白境界必須)。
/// 大文字小文字は無視する (search_query::parse の小文字化と整合)。
fn query_contains_tag(query: &str, tag_name: &str) -> bool {
    let needle = format!("#{}", tag_name).to_lowercase();
    for tok in query.split_whitespace() {
        // クォート除外の簡易判定: 完全一致だけ見る
        let t = tok.trim_start_matches('-').to_lowercase();
        if t == needle {
            return true;
        }
    }
    false
}

/// クエリ末尾に `#tag_name` を追加 (前に空白を 1 個挟む)。
/// クエリが空なら先頭にそのまま置く。
fn append_tag_to_query(query: &mut String, tag_name: &str) {
    let trimmed_end = query.trim_end();
    if trimmed_end.is_empty() {
        query.clear();
        query.push('#');
        query.push_str(tag_name);
    } else {
        // 末尾の余分な空白は保ちつつ 1 文字空白で区切る
        if !query.ends_with(' ') {
            query.push(' ');
        }
        query.push('#');
        query.push_str(tag_name);
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
                representative: None,
                mtime: None,
            },
        );
        map.insert(
            PathBuf::from("c:/high"),
            ContainerHit {
                path: "c:/high".into(),
                kind: SearchContainerKind::Folder,
                hit_count: 10,
                representative: None,
                mtime: None,
            },
        );
        map.insert(
            PathBuf::from("c:/mid"),
            ContainerHit {
                path: "c:/mid".into(),
                kind: SearchContainerKind::Folder,
                hit_count: 5,
                representative: None,
                mtime: None,
            },
        );
        let v = sort_containers_with_mode(&map, ContainerSortMode::HitCount);
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
        let zip = state
            .containers
            .get(&PathBuf::from("c:/album.zip"))
            .unwrap();
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
            GlobalHit {
                path: "C:/root/a.jpg".into(),
                score: 1.0,
            },
            GlobalHit {
                path: "C:/root/sub/b.jpg".into(),
                score: 1.0,
            },
            GlobalHit {
                path: "C:/root/sub/deeper/c.jpg".into(),
                score: 1.0,
            },
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
            state.accumulate_hit(&GlobalHit {
                path: p.into(),
                score: 1.0,
            });
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
            state.accumulate_hit(&GlobalHit {
                path: p.into(),
                score: 1.0,
            });
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

    // -------------------------------------------------------------------
    // ユーザー要望テスト (2026-04): Ctrl+G 検索結果の表示 / 階層ドリル / Ctrl+↑↓
    // -------------------------------------------------------------------

    /// Ctrl+G drill-in 直下に PDF / ZIP / 画像 のヒットが混在したとき、
    /// `build_drilled_items` が拡張子で正しく `GridItem::{PdfFile, ZipFile, Image}`
    /// に分類すること。
    ///
    /// 2026-04 ユーザー報告: ScanSnap (PDF だらけのフォルダ) で drill-in すると
    /// 全サムネ「画像フォーマット判定不可」で失敗していた。その回帰ガード。
    #[test]
    fn build_drilled_items_classifies_pdf_zip_and_image_by_extension() {
        let mut state = GlobalSearchState::default();
        for p in [
            "C:/mix/a.pdf",
            "C:/mix/b.zip",
            "C:/mix/c.png",
            "C:/mix/d.jpg",
        ] {
            state.accumulate_hit(&GlobalHit {
                path: p.into(),
                score: 1.0,
            });
        }
        let (items, _) = build_drilled_items(&state, Path::new("C:/mix"), false);
        // 期待: 名前昇順で a.pdf → b.zip → c.png → d.jpg
        let kinds: Vec<&'static str> = items
            .iter()
            .map(|it| match it {
                GridItem::PdfFile(_) => "PdfFile",
                GridItem::ZipFile(_) => "ZipFile",
                GridItem::Image(_) => "Image",
                GridItem::Folder(_) => "Folder",
                _ => "Other",
            })
            .collect();
        assert_eq!(kinds, vec!["PdfFile", "ZipFile", "Image", "Image"]);
    }

    /// 多階層のフォルダ構造の一部だけがヒットしたとき、drill-in でヒットを含む
    /// **直接の子フォルダ** のみ (ヒットなしの兄弟 / 枝は枝刈り) が表示されること。
    ///
    /// 構造:
    /// ```
    /// /root/
    ///   year2024/
    ///     jan/  matches/  X.png     ← hit
    ///     feb/            Y.png     ← no hit (pruned)
    ///   year2023/
    ///     jan/            Z.png     ← no hit (pruned)
    /// ```
    /// ヒットは `/root/year2024/jan/matches/X.png` の 1 件のみ。
    /// - `/root` でドリル → `year2024` (1 件持ち) だけが並ぶ (year2023 は枝刈り)
    /// - `/root/year2024` でドリル → `jan` だけが並ぶ (feb は枝刈り)
    /// - `/root/year2024/jan` でドリル → `matches` フォルダだけが並ぶ
    #[test]
    fn build_drilled_items_prunes_branches_with_no_hits() {
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: "C:/root/year2024/jan/matches/X.png".into(),
            score: 1.0,
        });
        // 上記以外にもヒットを入れておく (別枝が干渉しないことを確認)
        state.accumulate_hit(&GlobalHit {
            path: "C:/root/year2024/jan/matches/Y.png".into(),
            score: 1.0,
        });

        // Level 1: /root でドリル → year2024 のみ
        let (l1, _) = build_drilled_items(&state, Path::new("C:/root"), false);
        assert_eq!(l1.len(), 1, "level1 item count");
        assert!(matches!(&l1[0], GridItem::Folder(p) if p == &PathBuf::from("C:/root/year2024")));

        // Level 2: /root/year2024 → jan のみ (feb は枝刈り)
        let (l2, _) = build_drilled_items(&state, Path::new("C:/root/year2024"), false);
        assert_eq!(l2.len(), 1, "level2 item count");
        assert!(
            matches!(&l2[0], GridItem::Folder(p) if p == &PathBuf::from("C:/root/year2024/jan"))
        );

        // Level 3: /root/year2024/jan → matches のみ
        let (l3, _) = build_drilled_items(&state, Path::new("C:/root/year2024/jan"), false);
        assert_eq!(l3.len(), 1, "level3 item count");
        assert!(
            matches!(&l3[0],
                GridItem::Folder(p) if p == &PathBuf::from("C:/root/year2024/jan/matches"))
        );

        // Level 4: /root/year2024/jan/matches → 画像 2 件が直下に並ぶ
        let (l4, _) = build_drilled_items(&state, Path::new("C:/root/year2024/jan/matches"), false);
        assert_eq!(l4.len(), 2, "level4 item count");
        for it in &l4 {
            assert!(matches!(it, GridItem::Image(_)));
        }
    }

    /// ZIP コンテナへ drill-in したときは、その ZIP のエントリだけがフラットに
    /// 並ぶこと (他の ZIP や通常フォルダのヒットは含まれない)。
    ///
    /// 注意: Tantivy 側で hit.path は `normalize_path` 済み (小文字化 + '/')
    /// なのでテスト入力も同じ形で用意する。`build_drilled_zip_items` は
    /// `normalize_path(zip_path)` と hit の zip 部分を文字列比較する。
    #[test]
    fn build_drilled_zip_items_shows_only_entries_of_target_zip() {
        let mut state = GlobalSearchState::default();
        for p in [
            "c:/archives/target.zip!folder/pic1.jpg",
            "c:/archives/target.zip!folder/pic2.jpg",
            "c:/archives/other.zip!x.jpg", // 別 ZIP
            "c:/loose/z.jpg",               // 通常ファイル
        ] {
            state.accumulate_hit(&GlobalHit {
                path: p.into(),
                score: 1.0,
            });
        }
        let (items, _) =
            build_drilled_items(&state, Path::new("C:/archives/target.zip"), /*is_zip=*/ true);
        assert_eq!(items.len(), 2, "target.zip のエントリ数");
        for it in &items {
            assert!(matches!(it, GridItem::ZipImage { .. }));
            if let GridItem::ZipImage { zip_path, .. } = it {
                assert_eq!(zip_path, &PathBuf::from("C:/archives/target.zip"));
            }
        }
    }

    /// Ctrl+↑↓ のフラット ナビリストが、Folder / ZIP / PDF (= SearchContainer として
    /// 立つもの) を混在で並べ、ヒット件数降順 + 名前昇順でソートすること。
    ///
    /// 2026-04 ユーザー要望: Ctrl+↑↓ で folder と ZIP の混在移動を確認したい。
    #[test]
    fn build_cross_container_nav_list_mixes_folder_and_zip_across_containers() {
        let mut state = GlobalSearchState::default();
        // C:/folder_a に 1 件 (少)
        state.accumulate_hit(&GlobalHit {
            path: "C:/folder_a/img.jpg".into(),
            score: 1.0,
        });
        // C:/folder_b に 3 件 (多)
        for n in 0..3 {
            state.accumulate_hit(&GlobalHit {
                path: format!("C:/folder_b/p{n}.jpg"),
                score: 1.0,
            });
        }
        // C:/book.zip に 2 件 (中)
        state.accumulate_hit(&GlobalHit {
            path: "C:/book.zip!e1.jpg".into(),
            score: 1.0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "C:/book.zip!e2.jpg".into(),
            score: 1.0,
        });

        let nav = build_cross_container_nav_list(&state);
        // ソート: 件数降順 (3 → 2 → 1)
        assert!(nav.len() >= 3, "nav entries count: {}", nav.len());
        // 先頭は C:/folder_b (3 件) → folder
        assert_eq!(nav[0].container_root, PathBuf::from("C:/folder_b"));
        assert!(!nav[0].is_zip);
        // 次は C:/book.zip (2 件) → ZIP
        let book_idx = nav
            .iter()
            .position(|e| e.container_root == PathBuf::from("C:/book.zip"))
            .expect("book.zip must be in nav list");
        assert!(nav[book_idx].is_zip);
        // C:/folder_a (1 件) → folder
        let a_idx = nav
            .iter()
            .position(|e| e.container_root == PathBuf::from("C:/folder_a"))
            .expect("folder_a must be in nav list");
        assert!(!nav[a_idx].is_zip);
        // 件数多い方が先に並ぶ (folder_b < book.zip < folder_a の順で見つかる)
        assert!(book_idx < a_idx);
    }

    // ---- タグピッカーヘルパー (Phase D) ----

    #[test]
    fn append_tag_to_empty_query() {
        let mut q = String::new();
        append_tag_to_query(&mut q, "原神");
        assert_eq!(q, "#原神");
    }

    #[test]
    fn append_tag_to_nonempty_query() {
        let mut q = String::from("写真");
        append_tag_to_query(&mut q, "原神");
        assert_eq!(q, "写真 #原神");
    }

    #[test]
    fn append_tag_preserves_trailing_space() {
        let mut q = String::from("写真 ");
        append_tag_to_query(&mut q, "原神");
        assert_eq!(q, "写真 #原神");
    }

    #[test]
    fn query_contains_exact_tag_match() {
        assert!(query_contains_tag("#原神 #風景", "原神"));
        assert!(query_contains_tag("写真 #原神", "原神"));
        assert!(query_contains_tag("-#原神", "原神")); // 除外形式でも検出 (重複防止のため)
    }

    #[test]
    fn query_contains_rejects_substring() {
        // #原 は #原神 の一部だが、別のタグ扱い
        assert!(!query_contains_tag("#原神", "原"));
    }

    #[test]
    fn query_contains_case_insensitive() {
        assert!(query_contains_tag("#HELLO", "hello"));
        assert!(query_contains_tag("#hello", "HELLO"));
    }
}
