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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;

use uuid::Uuid;

use crate::app::{App, is_thumb_adjust_target};
use crate::fts_index::{IndexKind, SearchTarget, SourceKind};
use crate::global_search::{DoneReason, GlobalHit, SearchStreamEvent};
use crate::grid_item::{ContainerRepresentative, GridItem, SearchContainerKind, ThumbnailState};
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
    /// None = 全タイプ (画像/PDF/動画)。Some(k) で単一種別に限定。
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
        IndexKind::Video => "動画ファイル",
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
            TargetChoice::Only(SourceKind::VideoMeta) => "動画メタ情報",
            TargetChoice::Only(SourceKind::Tags) => "タグ",
            TargetChoice::Only(SourceKind::Sidecar) => "サイドカー",
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
    TargetChoice::Only(SourceKind::VideoMeta),
    TargetChoice::Only(SourceKind::Tags),
    TargetChoice::Only(SourceKind::Sidecar),
];

// アイテム検索 (Ctrl+G) の対象は 画像 / PDF / 動画。フォルダ・ZIP はコンテナなので
// コンテナ検索 (Ctrl+S) 側で扱う (docs/search-container-item-redesign.md §3.2, §6)。
pub const KIND_CHOICES: &[Option<IndexKind>] = &[
    None,
    Some(IndexKind::Image),
    Some(IndexKind::Pdf),
    Some(IndexKind::Video),
];

/// クエリ入力後、検索実行までの debounce 間隔 (既存 Ctrl+F と揃える)。
const DEBOUNCE_MS: u64 = 300;
/// 1 フレームで消費するイベント数の上限 (UI ブロックを防ぐ)。
const MAX_EVENTS_PER_FRAME: usize = 8;
/// ContainerHit の再ソート間隔 (チラつき防止、docs §10.4.3)。
const RESORT_INTERVAL_MS: u64 = 1000;
/// Ctrl+G 一覧ビューが自動で集約ビューへ切り替わる総ヒット数の閾値
/// (docs/search-container-item-redesign.md §4.3.2)。
const AGGREGATE_AUTO_THRESHOLD: usize = 1000;
const GLOBAL_SEARCH_STATUS_TEXT_SIZE: f32 = 11.0;
const GLOBAL_SEARCH_STATUS_MIN_WIDTH: f32 = 48.0;
const GLOBAL_SEARCH_STATUS_MAX_WIDTH: f32 = 260.0;
const GLOBAL_SEARCH_STATUS_PADDING_X: f32 = 4.0;

fn global_search_status_width(ui: &egui::Ui, text: &str, color: egui::Color32) -> (f32, bool) {
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(GLOBAL_SEARCH_STATUS_TEXT_SIZE),
        color,
    );
    let needed = (galley.size().x + GLOBAL_SEARCH_STATUS_PADDING_X).ceil();
    let max_width = ui
        .available_width()
        .max(GLOBAL_SEARCH_STATUS_MIN_WIDTH)
        .min(GLOBAL_SEARCH_STATUS_MAX_WIDTH);
    let width = needed.clamp(GLOBAL_SEARCH_STATUS_MIN_WIDTH, max_width);
    let truncated = needed > width + 0.5;
    (width, truncated)
}

/// Ctrl+G の drill-down 状態。コンテナにドリルインしているときだけ `Some` で保持される
/// (docs/search-container-item-redesign.md §4.3.2)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrillState {
    /// ドリルインの起点 (SearchContainer のパス)
    pub container_root: PathBuf,
    /// 現在地 (container_root と同じか、その配下の子フォルダ)
    pub current_path: PathBuf,
    /// container が ZIP ファイルか
    pub is_zip: bool,
}

/// Ctrl+G の実効ビュー (docs/search-container-item-redesign.md §4.3.1)。
///
/// このenum は状態として保持しない。`GlobalSearchState` の `aggregate` / `drill` から
/// [`GlobalSearchState::view`] で導出される。`drill` を None にするだけで `aggregate`
/// に応じて一覧 / 集約へ正しく戻れる、という導出モデルにすることでドリルバックの
/// 戻り先を別途覚える必要をなくしている。
///
/// DrilledInto 時は「現在地 (current_path) 直下に落ちるヒット + ヒットを含む子フォルダ」
/// だけを表示する (ヒットが 1 件もない枝は枝刈り)。Ctrl+↑↓ はこの枝刈り済みツリー
/// 上で移動する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobalSearchView {
    /// 一覧 (Flat): 全ヒットを個々のサムネイルで平置きする。
    Flat,
    /// 集約 (Aggregated): SearchContainer セルがヒット件数降順で並ぶ。
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
    /// 集約トグルの状態 (false = 一覧, true = 集約)。`drill` が Some のときは
    /// ドリルインビューが優先されるので、この値は「ドリルバック先」を決める。
    pub aggregate: bool,
    /// `aggregate` がまだ自動制御下か (true) / ユーザーが固定したか (false)。
    /// ストリーミング中にヒット数が閾値を超えると自動で集約へ切り替わるが、
    /// ユーザーがトグル操作・結果操作・ドリルインをした時点で false に倒れる
    /// (docs/search-container-item-redesign.md §4.3.2)。
    pub aggregate_auto: bool,
    /// drill-down 状態。コンテナにドリルインしていれば Some。
    pub drill: Option<DrillState>,
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
    /// BS で戻ったとき直前に開いていたフォルダ/コンテナを再選択するためのヒント。
    /// `App::select_after_load` と用途は近いが、あちらは `load_folder` 経由で
    /// ファイル名 (String) で照合するのに対し、Ctrl+G drill-back は `load_folder`
    /// を経由せず full path 同一性で復元する必要があるため別フィールド。
    pub restore_select_path: Option<PathBuf>,
    /// Newer/Older ソート用の mtime 非同期取得 (review #9 対応)。
    /// `ensure_container_mtime_populated` が走るのが UI スレッドだったため、
    /// 5000+ コンテナを SMB share 越しに開くと数十秒フリーズした。worker thread に
    /// 投げ、結果は `poll_container_mtime_pending` で drain する。
    pub mtime_lookup_pending: Option<MtimeLookupPending>,
}

/// mtime 非同期取得のハンドル。`spawn_container_mtime_lookup` が作って
/// `mtime_lookup_pending` に格納、`poll_container_mtime_pending` が消費する。
pub struct MtimeLookupPending {
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<MtimeLookupResult>,
}

impl MtimeLookupPending {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for MtimeLookupPending {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Debug, Clone)]
pub struct MtimeLookupResult {
    pub path: PathBuf,
    pub mtime: Option<i64>,
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
            aggregate: false,
            aggregate_auto: true,
            drill: None,
            total_valid: 0,
            total_scanned: 0,
            done: false,
            truncated: false,
            reject_message: None,
            saved_folder: None,
            filters: GlobalSearchFilters::default(),
            sort_mode: ContainerSortMode::HitCount,
            restore_select_path: None,
            mtime_lookup_pending: None,
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
    /// ユーザーに「まだ結果が確定していない」と見せるべき状態か。
    ///
    /// pending worker 実行中だけでなく、入力変更後の debounce 待ちも検索中として扱う。
    /// Ctrl+G を開いただけ / 空クエリ / reject 表示後 / 完了後は false。
    pub(crate) fn is_searching(&self) -> bool {
        self.active
            && !self.done
            && self.reject_message.is_none()
            && !self.query.trim().is_empty()
            && (self.pending.is_some() || self.query != self.last_executed)
    }

    /// 実効ビューを `aggregate` / `drill` から導出する (§4.3.1)。
    pub(crate) fn view(&self) -> GlobalSearchView {
        if let Some(d) = &self.drill {
            GlobalSearchView::DrilledInto {
                container_root: d.container_root.clone(),
                current_path: d.current_path.clone(),
                is_zip: d.is_zip,
            }
        } else if self.aggregate {
            GlobalSearchView::Aggregated
        } else {
            GlobalSearchView::Flat
        }
    }

    /// ストリーミング中の自動ビュー切替 (§4.3.2)。`aggregate_auto` が立っている間は
    /// 総ヒット数が閾値を超えたら集約ビューへ自動で切り替える。`total_valid` は
    /// 増加一方なので実質「一覧 → 集約」の一方向遷移。ユーザーがトグル操作・結果
    /// 操作・ドリルインをすると `aggregate_auto` が false に倒れ、以降は手動操作のみ。
    pub(crate) fn maybe_auto_switch_aggregate(&mut self) {
        if self.aggregate_auto {
            self.aggregate = self.total_valid > AGGREGATE_AUTO_THRESHOLD;
        }
    }

    /// 新規検索を開始する (既存 pending があれば cancel してから)。
    pub fn reset_for_new_query(&mut self) {
        // SearchHandle は Drop で cancel するので、take() だけで OK
        self.pending = None;
        self.containers.clear();
        self.all_hits.clear();
        // 新クエリは一覧から開始し、自動切替を再有効化、drill state もリセット (§4.3.2)。
        self.aggregate = false;
        self.aggregate_auto = true;
        self.drill = None;
        self.total_valid = 0;
        self.total_scanned = 0;
        self.done = false;
        self.truncated = false;
        self.reject_message = None;
        // **Codex P3-1 対応**: 旧クエリ向けの container mtime 取得 worker を cancel
        // (Drop impl が cancel flag を立てる)。旧 containers は上で .clear() 済みなので、
        // 旧 worker が SMB 越しに走り続けても結果は誰にも適用されない。
        // また `mtime_lookup_pending` が Some のまま居座ると、新クエリの
        // `ensure_container_mtime_populated` が pending 検出で early-return して
        // しまい、新 container 群の mtime ソートが始められなくなる。
        self.mtime_lookup_pending = None;
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

/// ZIP 内エントリを示すヒットパス (`<zippath>\x1F<entry>`) かを判定する。
/// セパレータ文字は `search_norm::ZIP_ENTRY_SEP` (ASCII Unit Separator U+001F)。
/// この文字は通常のファイル名に含められないため、通常パスとの曖昧さは発生しない
/// (Codex P2 対応)。旧実装は `!` 区切りで、ファイル名に `!` を含むケース
/// (Eagle 生成ファイル / 親ディレクトリ / ZIP 名自体に `!`) と衝突していた。
/// INDEX_VERSION bump で旧データは自動再構築される。
fn split_zip_hit_path(hit_path: &str) -> Option<(&str, &str)> {
    hit_path.split_once(crate::search_norm::ZIP_ENTRY_SEP)
}

fn is_zip_hit_path(hit_path: &str) -> bool {
    split_zip_hit_path(hit_path).is_some()
}

/// GlobalHit のパスから「サムネ表示できる代表」の情報を抽出する。
///
/// 画像ファイル / ZIP 内画像 / PDF ファイルのいずれかならサムネイル化できるため
/// 代表として採用する。それ以外 (未対応拡張子 / フォルダ名ヒットのみ) は None。
fn image_representative_from_hit(hit_path: &str) -> Option<ContainerRepresentative> {
    let (file_part, entry) = match split_zip_hit_path(hit_path) {
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
/// - ZIP エントリ (`<zippath>\u{1F}<entry>` 形式、[`crate::search_norm::ZIP_ENTRY_SEP`])
///   → ZIP ファイルパス
/// - 通常ファイル → 親フォルダパス
fn parent_container(hit_path: &str) -> (PathBuf, SearchContainerKind) {
    if let Some((zip_part, _entry)) = split_zip_hit_path(hit_path) {
        return (PathBuf::from(zip_part), SearchContainerKind::Zip);
    }
    let p = PathBuf::from(hit_path);
    if let Some(parent) = p.parent() {
        (parent.to_path_buf(), SearchContainerKind::Folder)
    } else {
        (p, SearchContainerKind::Folder)
    }
}

/// `GlobalHit.path` を `RatingDb` のキー (= `App::rating_path_key` 互換) に変換する。
///
/// - ZIP エントリ (`<zip>\x1F<entry>`) → `<normalize(zip)>::<lower(entry)>`
///   (= `page_path_key` の ZipImage 形式)
/// - 通常ファイル / PDF / フォルダ → `normalize(path)`
///   (= `page_path_key` の Image 形式 / コンテナ rating_path_key と同じ)
///
/// PDF はファイル単位で索引されているのでコンテナ★を引く。
/// ZIP 内画像はページ★を引く。
pub(crate) fn hit_rating_key(hit_path: &str) -> String {
    if let Some((zip_part, entry)) = split_zip_hit_path(hit_path) {
        crate::adjustment_db::zip_entry_key(Path::new(zip_part), entry)
    } else {
        crate::adjustment_db::normalize_path(Path::new(hit_path))
    }
}

/// 単一の★値が rating_filter を通るかを判定 (画像系・コンテナ系の共通ルール)。
///
/// `passes_rating_filter` (app.rs) と同じく `s <= 5 && rf[s]`。
/// 検索ヒットは「★絞り込み対象」のみ (動画 / セパレータは検索ヒットに上らない) なので
/// `accepts_rating()` 分岐は不要。
pub(crate) fn rating_passes_for_stars(stars: u8, rf: &[bool; 6]) -> bool {
    let s = stars as usize;
    s <= 5 && rf[s]
}

/// Ctrl+G drilled view: `current_path` の直下サブフォルダごとに、配下ヒットの
/// per-★ 件数 (★なし..★5、6 バケット) を集計する。`folder_rating_match` から
/// 引かれて、フォルダ右下のフィルタ件数バッジに表示される。
///
/// rating_filter は適用しない (バッジ表示時に rating_filter を当てるため、ここでは
/// 全 raw 件数を返す)。ZIP コンテナを drilled-in している (is_zip=true) ときは
/// サブフォルダの概念がないので空マップを返す。
fn compute_drilled_subfolder_counts(
    state: &GlobalSearchState,
    current_path: &Path,
    is_zip: bool,
) -> HashMap<String, [u32; 6]> {
    if is_zip {
        return HashMap::new();
    }
    let mut sub_counts: HashMap<PathBuf, [u32; 6]> = HashMap::new();
    for h in &state.all_hits {
        if is_zip_hit_path(&h.path) {
            continue;
        }
        let hp = PathBuf::from(&h.path);
        let Some(hp_parent) = hp.parent() else {
            continue;
        };
        if !path_is_under_or_eq(hp_parent, current_path) {
            continue;
        }
        // 直下のヒットはサブフォルダバッジに含めない (current_path 自身のバッジは出さない)
        if hp_parent == current_path {
            continue;
        }
        let rel = match hp_parent.strip_prefix(current_path) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if let Some(first) = rel.components().next() {
            let child = current_path.join(first.as_os_str());
            let bucket = h.stars.min(5) as usize;
            sub_counts.entry(child).or_insert([0u32; 6])[bucket] += 1;
        }
    }
    // App 側のキー形式 (= folder_rating_counts と同じ正規化文字列) に合わせる
    sub_counts
        .into_iter()
        .map(|(p, c)| (crate::adjustment_db::normalize_path(&p), c))
        .collect()
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

/// Newer/Older ソートのために、mtime 未取得のコンテナの fs::metadata 取得を **worker
/// thread に依頼する** (review #9 対応)。`HitCount` / `Name` モードでは何もしない。
///
/// **ストリーミング中 (`state.done == false`) はスキップする** (Codex P3 対応):
/// 検索結果が逐次 append されるたびに本関数が呼ばれるため、そのタイミングで毎回
/// 新コンテナ全部に fs::metadata を掛けると HDD / ネットワーク / 大量お気に入り環境
/// で UI がカクつく。ストリーム完了時 (done=true → rebuild) に 1 回まとめて取得し、
/// 以降はキャッシュが効くので `sort_mode` 切替は即時反映になる。ストリーミング中の
/// Newer/Older ソートは mtime 不明のため path 順で並ぶが、done 後に正しい順序へ snap する。
///
/// **worker offload (review #9)**: 旧実装は UI スレッドで `fs::metadata` を全件
/// 同期実行していたため、5000 コンテナを SMB share 越しに開くと UI が数十秒
/// フリーズしていた。本実装は worker thread に投げ、結果を mpsc 経由で
/// `App::poll_container_mtime_pending` が drain する。すべて drain しきった
/// ところで rebuild がもう 1 度走り、新しい mtime でソートされる。
pub fn ensure_container_mtime_populated(state: &mut GlobalSearchState) {
    if !matches!(
        state.sort_mode,
        ContainerSortMode::Newer | ContainerSortMode::Older
    ) {
        // **Codex P3-3 対応**: ユーザーが Newer/Older から HitCount/Name へソートを
        // 戻した瞬間、走行中の mtime worker は結果が利用されないので止める。Drop impl が
        // cancel flag を立てて、SMB 越しの fs::metadata ループと毎フレーム repaint
        // (poll_container_mtime_pending 由来) を即座に停止する。
        state.mtime_lookup_pending = None;
        return;
    }
    if !state.done {
        return;
    }
    if state.mtime_lookup_pending.is_some() {
        return;
    }
    let missing: Vec<PathBuf> = state
        .containers
        .values()
        .filter(|c| c.mtime.is_none())
        .map(|c| c.path.clone())
        .collect();
    if missing.is_empty() {
        return;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<MtimeLookupResult>();
    let cancel_for_worker = Arc::clone(&cancel);
    let spawn_result = std::thread::Builder::new()
        .name("container-mtime-lookup".into())
        .spawn(move || {
            for path in missing {
                if cancel_for_worker.load(Ordering::Relaxed) {
                    break;
                }
                let mtime = std::fs::metadata(&path)
                    .ok()
                    .map(|m| crate::ui_helpers::mtime_secs(&m));
                if tx.send(MtimeLookupResult { path, mtime }).is_err() {
                    // 受信側が drop された → cancel と同義
                    break;
                }
            }
            // tx drop → rx は Disconnected を返す = 完了サイン
        });
    match spawn_result {
        Ok(_) => {
            state.mtime_lookup_pending = Some(MtimeLookupPending { cancel, rx });
        }
        Err(e) => {
            crate::logger::log(format!(
                "container-mtime-lookup: failed to spawn worker: {e}"
            ));
        }
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

/// Flat (一覧) view の items + image_metas を組み立てる
/// (docs/search-container-item-redesign.md §4.3.1)。
///
/// `state.all_hits` を走査して各ヒットを `GridItem::{Image, PdfFile, Video}` に変換し、
/// メインの `sort_order` で一律ソートする。ZIP 内画像ヒットはアイテム索引に存在しない
/// が (§3.2)、stale な索引が残るケースに備えて防御的にスキップする。
/// `build_drilled_items` と同じく placeholder image_metas を使い、UI スレッドでの
/// `fs::metadata` 同期呼び出しは避ける。
pub(crate) fn build_flat_items(
    state: &GlobalSearchState,
    sort_order: crate::settings::SortOrder,
    rating_filter: &[bool; 6],
) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>) {
    let rf_active = !rating_filter.iter().all(|&b| b);
    // (GridItem, basename, mtime) を集めてからまとめてソートする。
    let mut rows: Vec<(GridItem, String, i64)> = Vec::with_capacity(state.all_hits.len());
    for h in &state.all_hits {
        // ZIP 内エントリはアイテム索引対象外 (§3.2)。stale index 対策で防御的にスキップ。
        if is_zip_hit_path(&h.path) {
            continue;
        }
        if rf_active && !rating_passes_for_stars(h.stars, rating_filter) {
            continue;
        }
        let p = PathBuf::from(&h.path);
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let item = match ext.as_str() {
            "pdf" => GridItem::PdfFile(p.clone()),
            // ZIP ファイル自体もアイテム索引対象外 (§3.2)。防御的にスキップ。
            "zip" => continue,
            _ if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str()) => {
                GridItem::Video(p.clone())
            }
            _ => GridItem::Image(p.clone()),
        };
        let basename = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        rows.push((item, basename, h.mtime));
    }
    rows.sort_by(|a, b| {
        sort_order.compare(&a.1, a.2, &b.1, b.2, crate::ui_helpers::natural_sort_key)
    });
    let placeholder = Some((0_i64, 0_i64));
    let image_metas: Vec<Option<(i64, i64)>> = vec![placeholder; rows.len()];
    let items: Vec<GridItem> = rows.into_iter().map(|(it, _, _)| it).collect();
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
    rating_filter: &[bool; 6],
) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>) {
    if is_zip {
        return build_drilled_zip_items(state, current_path, rating_filter);
    }
    // ── 通常フォルダ配下の絞り込み ──
    // rating_filter が全 ON なら絞り込みスキップ (高速 path)。
    // App::rating_filter_active と同じ判定式を使う (固定 [bool; 6] へのアクセスなので
    // 自動ベクトル化された ~1ns、ヘルパー化するメリットなし)。
    let rf_active = !rating_filter.iter().all(|&b| b);
    let mut direct_files: Vec<PathBuf> = Vec::new();
    // 直下子フォルダ → その配下のヒット件数 (rating_filter 通過後)
    let mut sub_counts: HashMap<PathBuf, usize> = HashMap::new();

    for h in &state.all_hits {
        if is_zip_hit_path(&h.path) {
            continue; // ZIP ヒットはスキップ
        }
        // rating_filter で弾く。直下ファイルもサブフォルダ件数も同じルールで絞る
        // ことで、バッジ件数 (= ここで数える件数) と grid 表示 (= rebuild_visible_indices
        // が同じ rating_filter で再フィルタする件数) が一致する。
        if rf_active && !rating_passes_for_stars(h.stars, rating_filter) {
            continue;
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
            _ if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str()) => {
                GridItem::Video(f.clone())
            }
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
    rating_filter: &[bool; 6],
) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>) {
    let zip_key = crate::search_index_db::normalize_path(zip_path);
    let rf_active = !rating_filter.iter().all(|&b| b);
    // sync I/O 除去 (フォルダ版と同じ理由)。
    let placeholder = Some((0_i64, 0_i64));
    // 上限 = 同じ ZIP の全ヒット。pre-allocate でリアロケを抑える。
    let cap = state.all_hits.len();
    let mut items: Vec<GridItem> = Vec::with_capacity(cap);
    let mut image_metas: Vec<Option<(i64, i64)>> = Vec::with_capacity(cap);
    for h in &state.all_hits {
        let Some((zip_part, entry)) = split_zip_hit_path(&h.path) else {
            continue;
        };
        if zip_part != zip_key {
            continue;
        }
        if rf_active && !rating_passes_for_stars(h.stars, rating_filter) {
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

/// フルスクリーンで開ける image-like アイテムか。Ctrl+↑↓ の飛び先判定に使う。
fn is_fullscreen_target(item: Option<&GridItem>) -> bool {
    matches!(
        item,
        Some(GridItem::Image(_))
            | Some(GridItem::Video(_))
            | Some(GridItem::ZipImage { .. })
            | Some(GridItem::PdfPage { .. })
    )
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
        if is_zip_hit_path(&h.path) {
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

/// `replace_search_view_items` でストリーミング rebuild 間にサムネを使い回すための
/// 識別キー。GridItem の表示同一性 (= 同じテクスチャが使えるか) を表現する。
/// Aggregated/SearchContainer は representative が変わると別物扱いにし、ZipImage /
/// PdfPage は zip 本体パス + entry/page 番号で識別する。
///
/// `GridItem::perf_key()` (`grid_item.rs`) と似ているが意図が違う:
/// - `perf_key` は perf ログ用の安定 ID で、SearchContainer の representative は無視する
/// - こちらはテクスチャ再利用の判定なので、representative が切り替わったら別キー扱いにして
///   再生成させる必要がある
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) enum ThumbReuseKey {
    Folder(PathBuf),
    Image(PathBuf),
    Video(PathBuf),
    ZipFile(PathBuf),
    PdfFile(PathBuf),
    Archive(PathBuf),
    /// SearchContainer は representative も含めて識別。代表ヒットが変わったら
    /// 別テクスチャを再生成させる (古い代表サムネを残さない)。
    Container(
        PathBuf,
        SearchContainerKind,
        Option<ContainerRepresentative>,
    ),
    ZipImage(PathBuf, String),
    PdfPage(PathBuf, u32),
}

pub(crate) fn thumb_reuse_key(item: &GridItem) -> Option<ThumbReuseKey> {
    match item {
        GridItem::Folder(p) => Some(ThumbReuseKey::Folder(p.clone())),
        GridItem::Image(p) => Some(ThumbReuseKey::Image(p.clone())),
        GridItem::Video(p) => Some(ThumbReuseKey::Video(p.clone())),
        GridItem::ZipFile(p) => Some(ThumbReuseKey::ZipFile(p.clone())),
        GridItem::PdfFile(p) => Some(ThumbReuseKey::PdfFile(p.clone())),
        GridItem::ConvertibleArchive { path, .. } => Some(ThumbReuseKey::Archive(path.clone())),
        GridItem::SearchContainer {
            path,
            kind,
            representative,
            ..
        } => Some(ThumbReuseKey::Container(
            path.clone(),
            *kind,
            representative.clone(),
        )),
        GridItem::ZipImage {
            zip_path,
            entry_name,
            ..
        } => Some(ThumbReuseKey::ZipImage(
            zip_path.clone(),
            entry_name.clone(),
        )),
        GridItem::PdfPage {
            pdf_path, page_num, ..
        } => Some(ThumbReuseKey::PdfPage(pdf_path.clone(), *page_num)),
        GridItem::ZipSeparator { .. } => None,
    }
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
        // ストリーミング中に 1 秒毎の rebuild が来るので、同一パスの Loaded サムネを
        // 使い回してテクスチャ再アップロードによる画面ちらつきを防ぐ。
        // 旧 items + thumbnails から path-keyed map を作っておき、後で新位置に転送する。
        let preserved: HashMap<ThumbReuseKey, ThumbnailState> = self
            .items
            .iter()
            .zip(self.thumbnails.iter())
            .filter_map(|(it, st)| match st {
                ThumbnailState::Loaded { .. } => thumb_reuse_key(it).map(|k| (k, st.clone())),
                _ => None,
            })
            .collect();
        let preserved_thumb_pixels: HashMap<ThumbReuseKey, Arc<egui::ColorImage>> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                self.thumb_pixels
                    .get(&i)
                    .cloned()
                    .and_then(|pixels| thumb_reuse_key(it).map(|k| (k, pixels)))
            })
            .collect();
        // 選択中アイテム + チェック済みアイテムの「内容キー」をスナップショット。
        // ストリーミング rebuild で items 並びが変わってもカーソル位置とチェック状態を
        // 同じ内容のセルに追従させる (idx ベースだと別アイテムを指す事故になる)。
        let selected_key = self
            .selected
            .and_then(|i| self.items.get(i))
            .and_then(thumb_reuse_key);
        let checked_keys: HashSet<ThumbReuseKey> = self
            .checked
            .iter()
            .filter_map(|&i| self.items.get(i).and_then(thumb_reuse_key))
            .collect();
        // PDF render pool に残っている旧 search 結果の render ジョブを stale prune
        // する。`bump_render_epoch_only` は cancel_token / catchup を touch しないので、
        // worker 再 spawn なしの replace 経路で worker を殺さない。
        // (docs/pdf-pool-context-epoch-plan.md Phase 4)
        self.bump_render_epoch_only();
        // items_generation bump + thumbnails 初期化を一箇所に集約。
        // これ以降に届く旧ワーカーの ThumbMsg は poll_thumbnails の
        // 世代不一致チェックで破棄される。
        self.install_new_items(items, image_metas);
        // install_new_items は既定で false に倒すので、合成ビューであることを上書き。
        // rating 変更時の rebuild_items_from_global_search 判定で参照される (Codex P2)。
        self.items_are_global_search_view = true;
        // idx ベース状態 + キュー排水 (requested / pending_finalize / texture_backlog /
        // keep_* / rotation / rating / adjustment_cache / thumb_pixels / ai_* / fs_* /
        // reload_queue / heavy_io_queue / tag_prewarm_*) を一括破棄。
        // (`self.checked` も中で clear される。下で content-key から復元)
        self.invalidate_idx_state_and_queues();
        // 旧 thumbnails を新位置にマージ。Loaded のままなら upload backlog に乗らず、
        // 同一フレームで前回のテクスチャがそのまま見え続けるのでちらつかない。
        // 補正用 source pixels も同じ content-key で移し、補正済み tex は再生成させる。
        // 同時に選択 / チェックも content-key で新 idx に再マップする。
        let mut restored_selected: Option<usize> = None;
        if !preserved.is_empty() || selected_key.is_some() || !checked_keys.is_empty() {
            for (i, item) in self.items.iter().enumerate() {
                let Some(key) = thumb_reuse_key(item) else {
                    continue;
                };
                if let Some(state) = preserved.get(&key) {
                    self.thumbnails[i] = state.clone();
                    if is_thumb_adjust_target(Some(item)) {
                        if let Some(pixels) = preserved_thumb_pixels.get(&key) {
                            self.thumb_pixels.insert(i, Arc::clone(pixels));
                        }
                    }
                }
                if selected_key.as_ref() == Some(&key) {
                    restored_selected = Some(i);
                }
                if checked_keys.contains(&key) {
                    self.checked.insert(i);
                }
            }
        }
        self.selected = restored_selected;
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
        // 選択を復元できたときは scroll_to_selected を立てて view を追従させる。
        // 復元できなかった (= 旧選択アイテムが消えた / 初回 install) ときは従来通り
        // 先頭にスクロール。
        if restored_selected.is_some() {
            self.scroll_to_selected = true;
        } else {
            self.scroll_offset_y = 0.0;
            self.scroll_to_selected = false;
        }
        self.scroll_hint.store(0, Ordering::Relaxed);
        self.rebuild_visible_indices();
        // auto_aspect: invalidate で samples を全クリアしたが、Ctrl+G では preserved
        // で thumbnails が新 idx に復元されている。Auto モードならそれらから samples を
        // 再構築 (Codex P3 2026-05)。
        if self.settings.thumb_aspect_auto {
            self.rebuild_auto_aspect_samples_from_loaded();
            self.maybe_apply_auto_aspect(false);
        }
        // tag_prewarm worker は invalidate_idx_state_and_queues で cancel されているので、
        // 検索結果向けに再起動 + fts_meta から tags_cache を再プリウォーム。
        self.prewarm_grid_tags();
        // Ctrl+G 結果の動画サムネ抽出スレッドを (必要に応じて) 再 spawn する。
        // streaming 中は pin 持ち動画のみ、`done=true` 後は全動画を Shell API で展開する。
        self.respawn_search_video_thread();
    }

    /// Ctrl+G 結果ビュー専用の動画サムネ抽出スレッドの候補リストを組み立てる。
    /// 単体テスト容易性のため `respawn_search_video_thread` から切り出した純粋関数。
    ///
    /// - `Loaded` 状態の動画は除外 (= preserve 経路で既にテクスチャを持つ)
    /// - `streaming=true` (= `global_search.done == false`) のときは `pin_paths` に
    ///   含まれる動画だけを残す。これは「streaming 中は Shell API の重い処理を避け、
    ///   `video_pins` DB から WebP を取り出すだけで完結する動画だけサムネ表示する」
    ///   設計に対応 (詳細は `respawn_search_video_thread` の doc を参照)。
    /// - `streaming=false` のときは全 not-Loaded 動画を返す。
    pub(crate) fn compute_search_video_candidates(
        items: &[GridItem],
        thumbnails: &[ThumbnailState],
        pin_paths: &HashSet<std::path::PathBuf>,
        streaming: bool,
    ) -> Vec<(usize, std::path::PathBuf, u64)> {
        debug_assert_eq!(items.len(), thumbnails.len());
        items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                GridItem::Video(p) => {
                    if matches!(thumbnails.get(i), Some(ThumbnailState::Loaded { .. })) {
                        return None;
                    }
                    if streaming && !pin_paths.contains(p) {
                        return None;
                    }
                    // file_size は検索結果ビューでは取得していないので 0 を渡す。
                    Some((i, p.clone(), 0_u64))
                }
                _ => None,
            })
            .collect()
    }

    /// Ctrl+G 結果ビュー専用の動画サムネ抽出スレッドを再 spawn する。
    ///
    /// 通常閲覧 / Ctrl+S と異なり、Ctrl+G の `replace_search_view_items` は
    /// `start_loading_items` を経由しないため `spawn_video_thread` が呼ばれない。
    /// 結果として `GridItem::Video` は `ThumbnailState::Pending` のまま放置される。
    /// このヘルパが補う。
    ///
    /// ## 設計上の妥協 (docs/search-architecture.md と Codex review 反映)
    ///
    /// 1. **`items_generation` バンプとの両立**:
    ///    streaming 中は `replace_search_view_items` が ~1 秒ごとに走り、毎回
    ///    `items_generation` が bump する。spawn 時に snapshot された items_gen で
    ///    ThumbMsg を送るので、次の bump で前回の spawn の結果は破棄される。
    ///    対策として: streaming 中 (`global_search.done == false`) は **pin 持ち
    ///    動画だけを spawn 対象**にする。pin 経路は WebP デコードのみで Shell API を
    ///    呼ばないので 1 spawn が ~100ms 未満で完結し、cancel + 再 spawn による
    ///    重複処理コストが小さい。
    ///    `done=true` (= streaming 終了 / view 切替後の安定状態) では Shell API を
    ///    含む通常チェーンを起動。items_gen はその時点で安定しているためメッセージは
    ///    破棄されない。
    ///
    /// 2. **同名 stem の sidecar 画像 leak 回避**:
    ///    `App::video_thumb_overrides` は前フォルダの sidecar を stem キーで保持する
    ///    (src/app.rs:`hydrate_video_thumb_overrides_from_current_folder`)。Ctrl+G の
    ///    結果は複数フォルダにまたがるため、これを渡すと別フォルダの同 stem 動画に
    ///    sidecar 画像が誤って当たる。よって empty map を渡す (sidecar は Ctrl+G では
    ///    使わない)。Codex P1 #2 指摘。
    ///
    /// 3. **重複 Shell 呼び出し抑制**:
    ///    既存の per-search cancel (`search_video_thread_cancel`) を毎回上書きする
    ///    ことで、前 spawn を素早く終了させる。spawn_video_thread の cancel チェックは
    ///    Shell call の合間に入るので 1 件分のラグはあるが、Shell Thumbcache が hit
    ///    する 2 回目以降は ms オーダーで完了する。Codex P2 #3 指摘の緩和策。
    pub(crate) fn respawn_search_video_thread(&mut self) {
        use std::sync::atomic::Ordering;
        // 旧 spawn を cancel する (worker は次の Shell call 終了後に exit)。
        if let Some(old) = self.search_video_thread_cancel.take() {
            old.store(true, Ordering::Relaxed);
        }
        // 検索結果ビュー以外では何もしない (通常閲覧 / Ctrl+S は start_loading_items が
        // spawn_video_thread を別経路で呼ぶ)。
        if !self.items_are_global_search_view {
            return;
        }
        // 診断ログ (Codex 2026-05-25): respawn が呼ばれたか / Aggregated 等で Video が
        // 1 つも無いか / candidates の内訳がわかるよう、items 内の Video 件数を記録。
        // Aggregated view では items は `GridItem::SearchContainer` になり、
        // `GridItem::Video` が 0 件で「pin 持ち動画だけ出る」現象は起きないはずだが、
        // 万一 view 違いだと早期 return 経路が変わるので追跡できるようにする。
        let view_label = match self.global_search.view() {
            GlobalSearchView::Flat => "Flat",
            GlobalSearchView::Aggregated => "Aggregated",
            GlobalSearchView::DrilledInto { .. } => "DrilledInto",
        };
        let total_videos_in_items = self
            .items
            .iter()
            .filter(|it| matches!(it, GridItem::Video(_)))
            .count();
        // まず not-Loaded 動画 path を全部集めて pin lookup を 1 度で済ませる
        // (Codex P2 #4: not-Loaded 集合に限定して video_pin_db を叩く)。
        let not_loaded_paths: Vec<std::path::PathBuf> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                GridItem::Video(p)
                    if !matches!(self.thumbnails.get(i), Some(ThumbnailState::Loaded { .. })) =>
                {
                    Some(p.clone())
                }
                _ => None,
            })
            .collect();
        if not_loaded_paths.is_empty() {
            crate::logger::log(format!(
                "[search video] respawn skip: view={view_label} done={} total_videos={total_videos_in_items} not_loaded=0",
                self.global_search.done,
            ));
            return;
        }
        // **バルクルックアップ** (Codex P2 #2 指摘): streaming 中の rebuild が ~1 秒
        // 周期で走り、検索結果に動画が数千〜数万件あると 1 件ずつ `db.lookup()` を
        // 呼ぶと UI スレッドで大量の SQLite I/O が発生する。`lookup_webps_many` で
        // `IN` 句に集約することで 500 件チャンク × N 回の prepared statement に圧縮。
        let pin_blobs: std::collections::HashMap<std::path::PathBuf, Vec<u8>> =
            if let Some(db) = self.video_pin_db.as_ref() {
                db.lookup_webps_many(&not_loaded_paths)
            } else {
                std::collections::HashMap::new()
            };
        let pin_paths: HashSet<std::path::PathBuf> = pin_blobs.keys().cloned().collect();
        let streaming = !self.global_search.done;
        let candidates = Self::compute_search_video_candidates(
            &self.items,
            &self.thumbnails,
            &pin_paths,
            streaming,
        );
        let candidates_pin_count = candidates
            .iter()
            .filter(|(_, p, _)| pin_paths.contains(p))
            .count();
        let candidates_shell_count = candidates.len() - candidates_pin_count;
        crate::logger::log(format!(
            "[search video] respawn: view={view_label} done={} streaming={streaming} \
             total_videos={total_videos_in_items} not_loaded={} pins={} \
             candidates={} (pin={candidates_pin_count} shell={candidates_shell_count})",
            self.global_search.done,
            not_loaded_paths.len(),
            pin_paths.len(),
            candidates.len(),
        ));
        if candidates.is_empty() {
            return;
        }
        // 新 cancel を作成して保存。
        let new_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.search_video_thread_cancel = Some(std::sync::Arc::clone(&new_cancel));
        // empty thumb_overrides で sidecar leak を回避 (`respawn_search_video_thread`
        // doc 内 #2 参照)。
        self.spawn_video_thread(
            self.tx.clone(),
            new_cancel,
            candidates,
            std::collections::HashMap::new(),
            pin_blobs,
        );
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
        self.cancel_pending_folder_nav();
        // 他の検索バー (Ctrl+F / Ctrl+S) が開いていれば閉じる (相互排他)
        self.close_other_search_bars(crate::app::SearchMode::Global);
        self.global_search.active = true;
        self.global_search.focus_request = true;
        self.global_search.saved_folder = self.current_folder.clone();
        // Ctrl+G 中は current_folder_rating() が 0 を返す規約なので、
        // 旧フォルダの★が残っているとアドレスバーにちらつく。
        self.current_folder_rating_cache = None;
        // 防御的: 前回 Ctrl+G セッションの cancel が残っていれば落とす
        // (close_global_search で必ずクリアされる想定だが念のため)。
        if let Some(old) = self.search_video_thread_cancel.take() {
            old.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub(crate) fn close_global_search(&mut self) {
        if !self.global_search.active {
            return;
        }
        self.cancel_pending_folder_nav();
        // pending があれば SearchHandle の Drop impl で cancel される
        self.global_search.pending = None;
        // Ctrl+G 結果の動画サムネ抽出スレッドを cancel。後続の load_folder で
        // cancel_token 自体も bump されるが、こちらは Shell call 中の worker に
        // 早めに「やめてよい」を伝える専用フラグ。
        if let Some(old) = self.search_video_thread_cancel.take() {
            old.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        // **Codex P3-1 対応**: container mtime worker も同様に drop して cancel する。
        // 検索ビューを抜けたあとに SMB 越しの fs::metadata が裏で走り続けるのを止める。
        self.global_search.mtime_lookup_pending = None;
        self.global_search.active = false;
        self.global_search.has_focus = false;
        // Ctrl+G 中に 0 で埋めたキャッシュを破棄して、復帰先フォルダの実値を再計算させる。
        self.current_folder_rating_cache = None;
        // Codex round-9 Should-fix #2: drill state / all_hits / done フラグも明示クリア。
        // 旧実装は containers だけクリアしていたため、DrilledInto のまま閉じて再度 Ctrl+G を
        // 開くと、検索前なのに戻るボタンや古い drill-down UI が残る可能性があった。
        self.global_search.containers.clear();
        self.global_search.all_hits.clear();
        self.global_search.drill = None;
        self.global_search.aggregate = false;
        self.global_search.aggregate_auto = true;
        self.global_search.query.clear();
        self.global_search.last_executed.clear();
        self.global_search.reject_message = None;
        self.global_search.done = false;
        self.global_search.truncated = false;
        self.global_search.total_valid = 0;
        self.global_search.total_scanned = 0;
        // Ctrl+G 専用のサブフォルダ件数キャッシュも破棄する。folder_rating_match が
        // 旧データを誤って返さないように、saved_folder への load_folder より先に消す。
        self.search_drilled_folder_counts.clear();
        // 元のフォルダに戻る。この復帰 load_folder は履歴 (back/forward/recent) に
        // 積まない (検索は透明な一時オーバーレイ)。
        if let Some(folder) = self.global_search.saved_folder.take() {
            self.suppress_nav_record_for_search_restore = true;
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

    /// `ensure_container_mtime_populated` が worker thread に投げた mtime 取得結果を
    /// try_recv で drain する (review #9 対応、毎フレーム呼ぶ)。worker 完了時に
    /// `rebuild_items_from_global_search` を 1 度走らせて新しい mtime で並び替える。
    pub(crate) fn poll_container_mtime_pending(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.global_search.mtime_lookup_pending.as_ref() else {
            return;
        };
        let mut applied = 0usize;
        let mut disconnected = false;
        loop {
            match pending.rx.try_recv() {
                Ok(result) => {
                    if let Some(container) = self.global_search.containers.get_mut(&result.path) {
                        if container.mtime.is_none() {
                            container.mtime = result.mtime;
                            applied += 1;
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if disconnected {
            self.global_search.mtime_lookup_pending = None;
            // 全件 drain 完了。新しい mtime でソートし直すために 1 度 rebuild。
            self.rebuild_items_from_global_search();
        } else if applied > 0 {
            // pending 継続中はアイドル寝防止のため次フレームも poll させる。
            ctx.request_repaint();
        } else {
            ctx.request_repaint();
        }
    }

    /// SearchStreamEvent を try_recv で処理する (毎フレーム呼ぶ)。
    pub(crate) fn poll_global_search_events(&mut self, ctx: &egui::Context) {
        if self.global_search.done {
            return;
        }
        // 検索結果への操作 (セル選択 / スクロール) を検出したら自動ビュー切替を止める
        // (§4.3.2 (b))。閾値による自動切替はストリーミング中しか起きないので、done 後に
        // early return する本関数の冒頭で 1 箇所だけ判定すれば足りる。
        if self.global_search.aggregate_auto
            && self.global_search.drill.is_none()
            && (self.selected.is_some() || self.scroll_offset_y > 0.5)
        {
            self.global_search.aggregate_auto = false;
        }
        // rx を clone してから global_search の他フィールドを可変借用可能にする
        // (crossbeam-channel の Receiver は Clone 可能)
        let rx = match self.global_search.pending.as_ref() {
            Some(h) => h.rx.clone(),
            None => return,
        };
        let mut events_processed = 0;
        let mut changed = false;
        let mut stats_changed = false;
        while events_processed < MAX_EVENTS_PER_FRAME {
            match rx.try_recv() {
                Ok(SearchStreamEvent::Batch {
                    mut hits,
                    scanned_candidates,
                    valid_hits,
                }) => {
                    // drilled view のサブフォルダバッジ件数を rating_filter で
                    // 絞り込めるよう、batch ごとに rating DB を bulk lookup して
                    // hit.stars に詰める。1 batch = PAGE_SIZE (=500) 件で IN 句
                    // 1 発、warm SQLite で 1-3ms 程度。
                    // perf::event で span を取り、cold sqlite で重くなったら
                    // analyze_perf.py で検知できるようにしておく。
                    let did_rating_lookup = if let Some(db) = self.rating_db.as_ref() {
                        let perf_enabled = crate::perf::is_enabled();
                        let t0 = std::time::Instant::now();
                        let keys: Vec<String> =
                            hits.iter().map(|h| hit_rating_key(&h.path)).collect();
                        let map = db.get_many(&keys);
                        for (h, k) in hits.iter_mut().zip(keys.iter()) {
                            if let Some(&v) = map.get(k) {
                                h.stars = v;
                            }
                        }
                        if perf_enabled {
                            let ms = t0.elapsed().as_secs_f64() * 1000.0;
                            crate::perf::event(
                                "search",
                                "rating_bulk_lookup",
                                None,
                                0,
                                &[
                                    ("count", serde_json::Value::from(keys.len())),
                                    ("ms", serde_json::Value::from(ms)),
                                ],
                            );
                        }
                        !keys.is_empty()
                    } else {
                        false
                    };
                    for h in &hits {
                        self.global_search.accumulate_hit(h);
                    }
                    self.global_search.total_scanned = scanned_candidates;
                    self.global_search.total_valid = valid_hits;
                    stats_changed = true;
                    if !hits.is_empty() {
                        changed = true;
                    }
                    events_processed += 1;
                    // UI スレッド予算を守るため、rating bulk lookup を含む batch は
                    // 1 フレーム 1 件に制限する (Codex P2)。8 batch 同時着 → ~24ms
                    // を回避し、次フレームに残りを回す (request_repaint 済み)。
                    if did_rating_lookup {
                        break;
                    }
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
                    stats_changed = true;
                    break;
                }
                Ok(SearchStreamEvent::Error(msg)) => {
                    self.global_search.done = true;
                    self.global_search.reject_message = Some(format!("エラー: {msg}"));
                    changed = true;
                    stats_changed = true;
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.global_search.done = true;
                    changed = true;
                    stats_changed = true;
                    break;
                }
            }
        }
        if stats_changed {
            self.update_global_search_address();
        }
        if changed {
            // docs §10.4.3: 順序再評価は 1 秒毎で十分 (頻繁な入れ替えでチラつかない)
            let should_resort = match self.global_search.last_sort_at {
                None => true,
                Some(t) => t.elapsed() >= Duration::from_millis(RESORT_INTERVAL_MS),
            };
            if should_resort || self.global_search.done {
                // 集約状態 (containers / all_hits) は accumulate_hit が更新済み。items
                // 差し替えは「今 items が検索結果ビュー」のときだけ走らせる。実フォルダ /
                // ZIP / PDF を開いている間に rebuild すると、ユーザーが見ているグリッドが
                // 検索結果で上書きされて画面がちらつく (rating 変更経路と同じ条件)。
                if self.items_are_global_search_view {
                    self.rebuild_items_from_global_search();
                }
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
        // mtime が埋まらないまま残る。結果、その状態で `global_search_ctrl_nav`
        // (Ctrl+↑↓) が `sort_containers_with_mode` を Newer/Older ソートを mtime=None
        // で走らせてしまう。view に関わらず rebuild の入口で populate しておく
        // (done 未達なら関数内で no-op)。
        // ストリーミング中の自動ビュー切替 (一覧 ⇄ 集約) を rebuild の都度評価する
        // (§4.3.2)。aggregate_auto が下りていれば no-op。
        self.global_search.maybe_auto_switch_aggregate();
        ensure_container_mtime_populated(&mut self.global_search);
        let rating_filter = self.settings.rating_filter;
        let sort_order = self.settings.sort_order;
        let (items, image_metas) = match self.global_search.view() {
            GlobalSearchView::Flat => {
                // 一覧ビューはサブフォルダバッジを使わない。残骸を破棄する。
                self.search_drilled_folder_counts.clear();
                build_flat_items(&self.global_search, sort_order, &rating_filter)
            }
            GlobalSearchView::Aggregated => {
                // Aggregated ではサブフォルダのバッジ計算なし。残骸を破棄する。
                self.search_drilled_folder_counts.clear();
                build_aggregated_items(&mut self.global_search)
            }
            GlobalSearchView::DrilledInto {
                ref current_path,
                is_zip,
                ..
            } => {
                // サブフォルダごとの per-★ 件数を all_hits から集計し直す。
                // build_drilled_items が rating_filter で表示アイテムを絞る一方、
                // バッジ件数表示には raw 集計が必要 (なし含む 6 バケット)。
                self.search_drilled_folder_counts =
                    compute_drilled_subfolder_counts(&self.global_search, current_path, is_zip);
                build_drilled_items(&self.global_search, current_path, is_zip, &rating_filter)
            }
        };
        self.replace_search_view_items(items, image_metas);
        // BS 戻りのカーソル位置復帰。target が見つからない・非表示のときは
        // 「先頭の表示中アイテム」にフォールバックする (selected=None のままだと
        // 次の方向キーで idx 0 に飛んでしまうため)。
        if let Some(target) = self.global_search.restore_select_path.take() {
            let idx_opt = self.items.iter().position(|it| match it {
                GridItem::SearchContainer { path, .. } => path == &target,
                GridItem::Folder(p)
                | GridItem::Image(p)
                | GridItem::ZipFile(p)
                | GridItem::PdfFile(p) => p == &target,
                GridItem::ZipImage { zip_path, .. } => zip_path == &target,
                GridItem::PdfPage { pdf_path, .. } => pdf_path == &target,
                _ => false,
            });
            let resolved = idx_opt
                .filter(|&idx| self.idx_visible(idx))
                .or_else(|| self.visible_indices.first().copied());
            if let Some(idx) = resolved {
                self.selected = Some(idx);
                self.scroll_to_selected = true;
            }
        }
        self.update_global_search_address();
    }

    /// SearchContainer をダブルクリックしたときの遷移。
    /// 絞り込みビューに切り替える (docs §10.3 [3] 絞り込みビュー)。
    /// 実フォルダ全体ではなく「検索にヒットしたものだけ (+ ヒットを含む子フォルダ)」
    /// を表示する。
    pub(crate) fn drill_into_container(&mut self, container: PathBuf, is_zip: bool) {
        self.cancel_pending_folder_nav();
        // ドリルインはユーザーの明示操作 → 自動ビュー切替を止める (§4.3.2 (c))。
        self.global_search.aggregate_auto = false;
        self.global_search.drill = Some(DrillState {
            container_root: container.clone(),
            current_path: container,
            is_zip,
        });
        self.rebuild_items_from_global_search();
    }

    /// 絞り込みビューで子フォルダのセルをクリックしたとき、そのフォルダに潜る。
    /// container_root と is_zip は不変、current_path だけ更新する。
    pub(crate) fn drill_into_subfolder(&mut self, sub_path: PathBuf) {
        if let Some(d) = self.global_search.drill.clone() {
            self.cancel_pending_folder_nav();
            self.global_search.drill = Some(DrillState {
                current_path: sub_path,
                ..d
            });
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
        self.cancel_pending_folder_nav();
        if let Some(d) = self.global_search.drill.clone() {
            // 既にドリルイン中: current_path を開いた PDF/ZIP/サブフォルダへ進める。
            self.global_search.drill = Some(DrillState {
                current_path: p.to_path_buf(),
                ..d
            });
        } else {
            // 一覧 (Flat) ビューから直接 PDF を開いたケース (Codex P2)。
            // container_root = current_path = p の 1 段ドリルを確立しておくと、
            // BS 1 回で drill_back_to_top → 一覧ビューへ戻れる。これをしないと
            // drill=None のまま PDF ページ表示に居続け、BS が一覧へ戻れなくなる
            // (最上位での BS は no-op、検索の終了は ESC のみ)。
            let is_zip = p
                .extension()
                .map(|e| e.eq_ignore_ascii_case("zip"))
                .unwrap_or(false);
            self.global_search.drill = Some(DrillState {
                container_root: p.to_path_buf(),
                current_path: p.to_path_buf(),
                is_zip,
            });
        }
        // 新しい current_path をブレッドクラムに反映。load_pdf_as_folder 等が
        // 後で self.address を raw パスで上書きするが、そこでも再度
        // `update_global_search_address` を呼んで元に戻す構造にしているので
        // 最終的にこのブレッドクラムが表示される。
        self.update_global_search_address();
    }

    /// トップレベル (一覧 or 集約) に戻る (drill-down 状態から)。
    /// 戻り先は `aggregate` の値で決まる (§4.3.2 の導出モデル)。
    pub(crate) fn drill_back_to_top(&mut self) {
        self.cancel_pending_folder_nav();
        // Ctrl+G drill-back は load_folder を経由しないため、suppression の subtree
        // 外判定が走らない。ユーザー視点では「本から出た」ので復元する (Codex High 指摘)。
        self.restore_rating_filter_suppression();
        // 戻った先で当該 SearchContainer にカーソルを再選択する。
        if let Some(d) = &self.global_search.drill {
            self.global_search.restore_select_path = Some(d.container_root.clone());
        }
        self.global_search.drill = None;
        self.rebuild_items_from_global_search();
    }

    /// BS キー (または drill-back ボタン) が押されたときの「一段戻る」処理。
    /// - current_path == container_root: Aggregated ビューに戻る
    /// - container_root の下に居る: 親フォルダに戻る
    pub(crate) fn drill_back_one_level(&mut self) {
        let Some(d) = self.global_search.drill.clone() else {
            return;
        };
        if d.current_path == d.container_root {
            self.drill_back_to_top();
        } else if let Some(parent) = d.current_path.parent() {
            let parent_pb = parent.to_path_buf();
            // container_root 外へは出さない (出るならトップレベルに戻る)
            if parent_pb.starts_with(&d.container_root) {
                // suppression anchor (= 開いたコンテナ) の subtree 内に
                // 留まっているので、★一時解除を維持する (Codex P2)。
                // 「本の中で上の階層へ戻った」だけで未評価の中身が再非表示に
                // なると挙動が一貫しない。完全に外へ出る (= drill_back_to_top)
                // 経路でだけ復元する。
                // 戻った先 (parent) で「直前に居たサブフォルダ」にカーソル復帰
                self.global_search.restore_select_path = Some(d.current_path.clone());
                self.cancel_pending_folder_nav();
                self.global_search.drill = Some(DrillState {
                    current_path: parent_pb,
                    ..d
                });
                self.rebuild_items_from_global_search();
            } else {
                self.drill_back_to_top();
            }
        } else {
            self.drill_back_to_top();
        }
    }

    /// フルスクリーン中の Ctrl+↑↓: 絞り込みビューの next/prev フォルダに跨って
    /// 移動し、その先頭 image-like をそのままフルスクリーンで開く。
    /// fs ツリー DFS (start_folder_nav) は検索
    /// コンテナの外に出てしまうので、Ctrl+G 中はこちらのルートを使う。
    ///
    /// 移動先に直接の image-like が無い (サブフォルダ配下にしかない) ケースは
    /// スキップしてさらに次の候補に進む。対象が見つかるまで前後方向に進み、
    /// フラットリスト全体に対象が無ければ元の位置に戻して何もしない。
    pub(crate) fn global_search_ctrl_nav_fullscreen(&mut self, ctx: &egui::Context, forward: bool) {
        // global_search_ctrl_nav が動かすのは drill state だけなので、進めたかは
        // drill の変化で判定する。
        let before_drill = self.global_search.drill.clone();
        loop {
            let prev_drill = self.global_search.drill.clone();
            self.global_search_ctrl_nav(forward);
            if self.global_search.drill == prev_drill {
                // これ以上進めない → 元の drill に戻す (fs_cache を綺麗に戻すため再 rebuild)
                if self.global_search.drill != before_drill {
                    self.global_search.drill = before_drill;
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
            // image-like アイテムがあるか判定する。無ければ次の候補へ。
            let image_idx = self
                .visible_indices
                .iter()
                .copied()
                .find(|&i| is_fullscreen_target(self.items.get(i)));
            if let Some(idx) = image_idx {
                self.open_fullscreen_from_fs_navigation(ctx, idx);
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
        let Some(d) = self.global_search.drill.clone() else {
            return;
        };
        let container_root = d.container_root;
        let current_path = d.current_path;

        // 現コンテナを Aggregated と同じ並び順で求め、その中での位置を取る。
        let containers =
            sort_containers_with_mode(&self.global_search.containers, self.global_search.sort_mode);
        let Some(cur_idx) = containers.iter().position(|c| c.path == container_root) else {
            return;
        };
        let cur = &containers[cur_idx];

        // 現コンテナ subtree の DFS リスト。container_root を起点として subtree 内に
        // 閉じて列挙するので、親コンテナと subset コンテナがネストしていても
        // 「2025-11-30 (container_root) → サブフォルダ → 次のコンテナ root」の順で素直に辿れる。
        let dfs: Vec<PathBuf> = match cur.kind {
            SearchContainerKind::Folder => {
                collect_hit_folders_dfs(&self.global_search.all_hits, &container_root)
            }
            SearchContainerKind::Zip => vec![container_root.clone()],
        };
        let dfs_pos = dfs.iter().position(|p| p == &current_path);

        // 1) 現コンテナ subtree 内での次/前を試す。
        if let Some(i) = dfs_pos {
            let within = if forward {
                dfs.get(i + 1).cloned()
            } else if i > 0 {
                Some(dfs[i - 1].clone())
            } else {
                None
            };
            if let Some(next_path) = within {
                self.cancel_pending_folder_nav();
                self.global_search.drill = Some(DrillState {
                    container_root: container_root.clone(),
                    current_path: next_path,
                    is_zip: false,
                });
                self.rebuild_items_from_global_search();
                return;
            }
        }

        // 2) subtree を抜けた → 次/前のコンテナの container_root へジャンプ。
        //    これにより `> output > 2025-12-30-1` のような次コンテナ深部への
        //    ワープを避け、検索結果一覧の「次のヒット」へ素直に進む。
        let next_container_idx = if forward {
            cur_idx.checked_add(1).filter(|&i| i < containers.len())
        } else {
            cur_idx.checked_sub(1)
        };
        let Some(next_idx) = next_container_idx else {
            return;
        };
        let next = &containers[next_idx];
        self.global_search.drill = Some(DrillState {
            container_root: next.path.clone(),
            current_path: next.path.clone(),
            is_zip: matches!(next.kind, SearchContainerKind::Zip),
        });
        self.cancel_pending_folder_nav();
        self.rebuild_items_from_global_search();
    }

    /// Ctrl+G アドレスバー表示を現在の view に合わせて更新する。
    /// - Aggregated: `🌐 アイテム検索: "query" (N 件)`
    /// - DrilledInto: `🌐 アイテム検索: "query" > container_name > sub_path...`
    pub(crate) fn update_global_search_address(&mut self) {
        if !self.global_search.active {
            return;
        }
        let query = if self.global_search.last_executed.is_empty() {
            self.global_search.query.clone()
        } else {
            self.global_search.last_executed.clone()
        };
        match self.global_search.view() {
            GlobalSearchView::Flat | GlobalSearchView::Aggregated => {
                // 集約時はコンテナ数、一覧時はヒット数を件数として表示する。
                let n = if self.global_search.aggregate {
                    self.global_search.containers.len()
                } else {
                    self.global_search.all_hits.len()
                };
                if query.is_empty() {
                    self.address = "🌐 アイテム検索".to_string();
                } else if self.global_search.is_searching() {
                    self.address = format!("🌐 アイテム検索: \"{query}\"  ({n} 件 / 検索中)");
                } else {
                    self.address = format!("🌐 アイテム検索: \"{query}\"  ({n} 件)");
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
                let suffix = if self.global_search.is_searching() {
                    "  (検索中)"
                } else {
                    ""
                };
                self.address = format!(
                    "🌐 アイテム検索: \"{query}\" > {}{suffix}",
                    segs.join(" > ")
                );
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
        let mut toggle_changed = false;

        let combo_popup_height = (ctx.content_rect().height() - 96.0).clamp(240.0, 520.0);
        let mut drill_back = false;
        egui::TopBottomPanel::top("global_search_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            // 絞り込みビュー中は「← 戻る」+ 現在地を検索バーの **上** の行に表示する
            // (下の検索バーの入力欄位置が他のモードとブレないようにするため)
            if let Some(d) = self.global_search.drill.clone() {
                ui.horizontal(|ui| {
                    if ui
                        .button("←")
                        .on_hover_text("1 段戻る (BS でも可)")
                        .clicked()
                    {
                        drill_back = true;
                    }
                    ui.label(
                        egui::RichText::new(format!("📁 {}", d.current_path.display()))
                            .size(11.0)
                            .color(egui::Color32::from_gray(150)),
                    );
                });
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("アイテム検索:").on_hover_text(
                    "Ctrl+G はお気に入りの「アイテム索引」を使い、画像 / PDF / 動画を\n\
                     ファイル名・タグ・EXIF・AI プロンプト等で横断検索します。\n\
                     アイテム索引が作成されていないお気に入りは対象になりません。\n\
                     お気に入り編集で「アイテムを索引化する」を有効にしてください。",
                );
                let response = ui.add_sized(
                    [320.0, 20.0],
                    egui::TextEdit::singleline(&mut self.global_search.query).hint_text(
                        r#"画像・PDF・動画をファイル名やメタ情報で検索 (AND / -除外 / "…")"#,
                    ),
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
                                    append_tag_to_query(&mut self.global_search.query, &t.name);
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
                        .height(combo_popup_height)
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
                        .height(combo_popup_height)
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
                        .height(combo_popup_height)
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

                if crate::ui_helpers::or_mode_checkbox(ui, &mut self.global_search.filters.or_mode)
                {
                    filter_changed = true;
                }

                // ── 集約トグル (一覧 ⇄ 集約) ──
                // ドリルイン中は出さない (上の行に「← 戻る」が出る)。
                if self.global_search.drill.is_none() {
                    ui.separator();
                    let agg = self.global_search.aggregate;
                    if ui
                        .selectable_label(agg, "集約")
                        .on_hover_text(
                            "ヒットを親フォルダ単位でまとめて表示します。\n\
                             ヒット数が多いと自動で ON になります。",
                        )
                        .clicked()
                    {
                        self.global_search.aggregate = !agg;
                        // 手動操作なので以降は自動切替しない (§4.3.2 (a))。
                        self.global_search.aggregate_auto = false;
                        toggle_changed = true;
                    }
                }

                // ── ソート切替 ──
                // 一覧 → 集約の自動切替で行の幅が変わらないよう、トップレベルでは
                // 常に場所を確保し、集約ビューで使えるときだけ有効化する。
                if self.global_search.drill.is_none() {
                    ui.separator();
                    ui.label(egui::RichText::new("ソート:").size(11.0).weak());
                    let current = self.global_search.sort_mode;
                    let mut next = current;
                    let sort_enabled =
                        self.global_search.aggregate && !self.global_search.containers.is_empty();
                    let sort_response = ui
                        .add_enabled_ui(sort_enabled, |ui| {
                            egui::ComboBox::from_id_salt("global_search_sort")
                                .selected_text(current.label())
                                .width(90.0)
                                .height(combo_popup_height)
                                .show_ui(ui, |ui| {
                                    for &mode in SORT_MODES {
                                        ui.selectable_value(&mut next, mode, mode.label());
                                    }
                                })
                                .response
                        })
                        .inner;
                    sort_response.on_hover_text(
                        "新しい/古い: コンテナの更新日時順 (初回選択時に\n\
                         fs::metadata を一括取得するので HDD では一瞬固まります)",
                    );
                    if sort_enabled && next != current {
                        self.global_search.sort_mode = next;
                        // アグリゲート view を即時再ソート (mtime 未取得なら
                        // build_aggregated_items 側で populate される)
                        sort_changed = true;
                    }
                }

                // 進捗/結果バッジ。操作群の一番右側に置き、表示テキストの実測幅だけを
                // 確保する。長い警告だけは上限幅で省略し、hover に全文を出す。
                let status = if let Some(msg) = &self.global_search.reject_message {
                    Some((msg.clone(), egui::Color32::from_rgb(200, 120, 40), None))
                } else if self.global_search.is_searching() {
                    Some((
                        format!("ヒット {} 件（検索中）", self.global_search.total_valid),
                        egui::Color32::from_rgb(180, 180, 80),
                        Some(format!(
                            "候補 {} 件を確認済み。アドレス欄の件数はヒットを含むコンテナ数です。",
                            self.global_search.total_scanned
                        )),
                    ))
                } else if self.global_search.done {
                    let (text, color, hover) = if self.global_search.truncated {
                        let text =
                            format!("ヒット {} 件で打ち切り", self.global_search.total_valid);
                        let hover = format!(
                            "ヒット {} 件で打ち切りました。絞り込みキーワードを追加してください。",
                            self.global_search.total_valid
                        );
                        (text, egui::Color32::from_rgb(200, 140, 40), Some(hover))
                    } else {
                        (
                            format!("ヒット {} 件", self.global_search.total_valid),
                            egui::Color32::from_gray(140),
                            None,
                        )
                    };
                    Some((text, color, hover))
                } else {
                    None
                };
                if let Some((text, color, hover)) = status {
                    ui.separator();
                    let (status_width, truncated) = global_search_status_width(ui, &text, color);
                    let hover_text = hover.or_else(|| truncated.then(|| text.clone()));
                    let response = ui.add_sized(
                        [status_width, ui.spacing().interact_size.y],
                        egui::Label::new(egui::RichText::new(text).size(11.0).color(color))
                            .truncate(),
                    );
                    if let Some(hover) = hover_text {
                        response.on_hover_text(hover);
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
            self.cancel_pending_folder_nav();
            self.global_search.last_change_at = Some(Instant::now());
            // Codex P3 対応: クエリが変わったら drill state を即リセットし、
            // 旧検索の pending / containers / all_hits も直ちに破棄してから空の
            // 一覧ビューとして rebuild する (debounce 完了までの間、旧結果で
            // drill-back 判定が残ったり、旧クエリでの rebuild race が起きないように)。
            // 新クエリなので自動ビュー切替も再有効化する (§4.3.2)。
            self.global_search.drill = None;
            self.global_search.aggregate = false;
            self.global_search.aggregate_auto = true;
            self.global_search.pending = None; // SearchHandle::Drop で cancel
            // **Codex P3-1 対応**: 旧 containers 向けの mtime worker も即 drop。
            // containers は直後に clear するので、worker が SMB 越しに走り続けても
            // 結果適用先が存在しない。新クエリの `ensure_container_mtime_populated`
            // が pending 検出で early-return しないよう、ここでも明示的に外す。
            self.global_search.mtime_lookup_pending = None;
            self.global_search.containers.clear();
            self.global_search.all_hits.clear();
            self.global_search.done = false;
            self.global_search.truncated = false;
            self.global_search.total_valid = 0;
            self.global_search.total_scanned = 0;
            // **review #5 対応**: 旧クエリ結果に対する self.selected / scroll_offset_y が
            // 残っていると、poll_global_search_events の guard (selected.is_some()
            // || scroll_offset_y > 0.5) が次フレームで aggregate_auto を false に
            // 落としてしまい、新クエリで 1000+ hit 時の自動切替が発火しない。
            // 旧クエリ結果から作った items は直後の rebuild で全て無効化されるので、
            // selected / scroll もここで「ユーザー未操作」状態へ戻す。
            self.selected = None;
            self.scroll_offset_y = 0.0;
            // query == last_executed でも debounce → spawn を必ず再走させる。
            // そうしないと、Enter 2 連打で旧検索が cancel されたあと
            // poll_global_search_debounce が「クエリが変わっていない」と判定して
            // 新 spawn を skip し、結果 0 件のまま固着する。
            self.global_search.last_executed.clear();
            self.rebuild_items_from_global_search();
            ctx.request_repaint_after(Duration::from_millis(DEBOUNCE_MS));
        }
        if sort_changed {
            // ソート変更はクエリ再実行不要 — items を並べ替えるだけ。
            self.rebuild_items_from_global_search();
        }
        if toggle_changed {
            // 集約トグルの切替は items を作り直すだけ (クエリ再実行不要)。
            self.rebuild_items_from_global_search();
        }
    }
}

// -----------------------------------------------------------------------
// タグピッカー用ヘルパー (docs/tag-feature.md Phase D)
// -----------------------------------------------------------------------

/// 検索クエリに `#tag` トークンが既に含まれているか (完全一致、空白境界必須)。
/// 大文字小文字は無視する (search_query::parse の小文字化と整合)。
pub(crate) fn query_contains_tag(query: &str, tag_name: &str) -> bool {
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
pub(crate) fn append_tag_to_query(query: &mut String, tag_name: &str) {
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

    const SEP: char = crate::search_norm::ZIP_ENTRY_SEP;

    fn zip_hit(zip: &str, entry: &str) -> String {
        format!("{zip}{SEP}{entry}")
    }

    #[test]
    fn parent_container_parses_zip_entries() {
        let (p, k) = parent_container(&zip_hit("c:/photos/album.zip", "subdir/img.jpg"));
        assert_eq!(p, PathBuf::from("c:/photos/album.zip"));
        assert_eq!(k, SearchContainerKind::Zip);
    }

    #[test]
    fn parent_container_parses_normal_files() {
        let (p, k) = parent_container("c:/photos/sunset/IMG.jpg");
        assert_eq!(p, PathBuf::from("c:/photos/sunset"));
        assert_eq!(k, SearchContainerKind::Folder);
    }

    /// 回帰: ファイル名にリテラル `!` を含む通常ファイル (Eagle が生成する
    /// `...-!fav_loli_A-....png` のような名前) を ZIP エントリと誤判定しない。
    /// 新 separator (U+001F) は通常ファイル名に現れないので `!` は関与しない。
    #[test]
    fn parent_container_handles_bang_in_filename() {
        let (p, k) =
            parent_container("g:/photos/20230418_推し/20230416_181414-1024x1536-!fav_loli_A-8.png");
        assert_eq!(p, PathBuf::from("g:/photos/20230418_推し"));
        assert_eq!(k, SearchContainerKind::Folder);
    }

    /// Codex P2 指摘 (解消): 親ディレクトリや ZIP 名に `!` を含むパスでも、
    /// separator が U+001F になったので単純な split_once で正しく分割される。
    /// 旧実装の `.zip!` 境界スキャンは不要になった。
    #[test]
    fn split_zip_hit_path_tolerates_bang_in_any_component() {
        // 親 dir / ZIP 名 / entry 内のどこに `!` があっても影響しない
        assert_eq!(
            split_zip_hit_path(&zip_hit("c:/a!/book.zip", "entry.jpg")),
            Some(("c:/a!/book.zip", "entry.jpg"))
        );
        assert_eq!(
            split_zip_hit_path(&zip_hit("c:/a/book!.zip", "entry.jpg")),
            Some(("c:/a/book!.zip", "entry.jpg"))
        );
        assert_eq!(
            split_zip_hit_path(&zip_hit("c:/a/book.zip", "sub!dir/img.jpg")),
            Some(("c:/a/book.zip", "sub!dir/img.jpg"))
        );
        // `!` 入り通常ファイル名は separator を含まないので ZIP 扱いされない
        assert_eq!(split_zip_hit_path("c:/a/book.zip!cover.jpg"), None);
        assert_eq!(split_zip_hit_path("c:/a/img-!name.png"), None);
    }

    #[test]
    fn accumulate_aggregates_by_container() {
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/b/1.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/b/2.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/c/1.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
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
        // 既定は一覧ビュー (§4.3.2)
        assert_eq!(state.view(), GlobalSearchView::Flat);
        state.drill = Some(DrillState {
            container_root: PathBuf::from("c:/photos"),
            current_path: PathBuf::from("c:/photos"),
            is_zip: false,
        });
        assert!(matches!(state.view(), GlobalSearchView::DrilledInto { .. }));
        // クエリリセットで drill state もリセットされる契約
        state.reset_for_new_query();
        assert_eq!(state.view(), GlobalSearchView::Flat);
    }

    #[test]
    fn aggregate_toggle_and_drill_derive_view() {
        let mut state = GlobalSearchState::default();
        // drill なし + aggregate=false → 一覧
        assert_eq!(state.view(), GlobalSearchView::Flat);
        // drill なし + aggregate=true → 集約
        state.aggregate = true;
        assert_eq!(state.view(), GlobalSearchView::Aggregated);
        // drill が立っていれば aggregate の値に関わらずドリルイン優先
        state.drill = Some(DrillState {
            container_root: PathBuf::from("c:/a"),
            current_path: PathBuf::from("c:/a"),
            is_zip: false,
        });
        assert!(matches!(state.view(), GlobalSearchView::DrilledInto { .. }));
        // drill を外すと aggregate に応じた戻り先 (ここでは集約) になる
        state.drill = None;
        assert_eq!(state.view(), GlobalSearchView::Aggregated);
    }

    #[test]
    fn auto_switch_to_aggregate_above_threshold() {
        let mut state = GlobalSearchState::default();
        // 既定: 一覧 + 自動制御下
        assert!(!state.aggregate);
        assert!(state.aggregate_auto);
        // 閾値ちょうどでは切り替わらない
        state.total_valid = AGGREGATE_AUTO_THRESHOLD;
        state.maybe_auto_switch_aggregate();
        assert!(!state.aggregate);
        // 閾値超で集約へ自動切替
        state.total_valid = AGGREGATE_AUTO_THRESHOLD + 1;
        state.maybe_auto_switch_aggregate();
        assert!(state.aggregate);
        // ユーザーが固定したら (aggregate_auto=false) 以降は自動で動かない
        state.aggregate_auto = false;
        state.aggregate = false;
        state.total_valid = AGGREGATE_AUTO_THRESHOLD * 100;
        state.maybe_auto_switch_aggregate();
        assert!(!state.aggregate, "aggregate_auto=false なら自動切替しない");
    }

    #[test]
    fn reset_clears_all_transient_state() {
        // Codex round-9 Should-fix #2 回帰: reset_for_new_query で all_hits / view /
        // done / truncated / total_* が全部クリアされる (close 時に相当の処理を走らせる想定)
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/1.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        state.drill = Some(DrillState {
            container_root: PathBuf::from("c:/a"),
            current_path: PathBuf::from("c:/a"),
            is_zip: false,
        });
        state.done = true;
        state.truncated = true;
        state.total_valid = 42;
        state.total_scanned = 100;
        state.reject_message = Some("test".into());

        state.reset_for_new_query();

        assert!(state.all_hits.is_empty());
        assert!(state.containers.is_empty());
        assert_eq!(state.view(), GlobalSearchView::Flat);
        assert!(!state.done);
        assert!(!state.truncated);
        assert_eq!(state.total_valid, 0);
        assert_eq!(state.total_scanned, 0);
        assert!(state.reject_message.is_none());
    }

    #[test]
    fn is_searching_includes_debounce_wait_but_not_done_or_empty() {
        let mut state = GlobalSearchState::default();
        state.active = true;
        assert!(!state.is_searching());

        state.query = "グルグル".to_string();
        state.last_executed.clear();
        assert!(state.is_searching());

        state.last_executed = state.query.clone();
        assert!(!state.is_searching());

        state.done = true;
        assert!(!state.is_searching());
    }

    #[test]
    fn all_hits_preserved_for_drill_down() {
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/1.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/2.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/b/x.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        // containers は 2 つに集約されているが、all_hits は 3 つ保持
        assert_eq!(state.containers.len(), 2);
        assert_eq!(state.all_hits.len(), 3);
    }

    #[test]
    fn accumulate_mixed_folders_and_zips() {
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: zip_hit("c:/album.zip", "0001.jpg"),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        state.accumulate_hit(&GlobalHit {
            path: zip_hit("c:/album.zip", "0002.jpg"),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/photos/x.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
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
                mtime: 0,
                stars: 0,
            },
            GlobalHit {
                path: "C:/root/sub/b.jpg".into(),
                score: 1.0,
                mtime: 0,
                stars: 0,
            },
            GlobalHit {
                path: "C:/root/sub/deeper/c.jpg".into(),
                score: 1.0,
                mtime: 0,
                stars: 0,
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
            mtime: 0,
            stars: 0,
        }];
        let got = collect_hit_folders_dfs(&hits, &PathBuf::from("C:/root"));
        // "no" サブフォルダはヒットを持たないので列挙されない
        assert_eq!(
            got,
            vec![PathBuf::from("C:/root"), PathBuf::from("C:/root/yes")]
        );
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
                mtime: 0,
                stars: 0,
            });
        }
        let (items, metas) = build_drilled_items(&state, Path::new("C:/root"), false, &[true; 6]);
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
                mtime: 0,
                stars: 0,
            });
        }
        let (items, _) = build_drilled_items(&state, Path::new("C:/mix"), false, &[true; 6]);
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

    /// build_flat_items: 全ヒットを sort_order で一律ソートし、拡張子で
    /// Image / PdfFile / Video に分類する。ZIP 系ヒットは除外する (§3.2、§4.3.1)。
    #[test]
    fn build_flat_items_sorts_classifies_and_skips_zip() {
        let mut state = GlobalSearchState::default();
        for p in ["c:/a/2.jpg", "c:/b/1.png", "c:/c/doc.pdf", "c:/d/clip.mp4"] {
            state.accumulate_hit(&GlobalHit {
                path: p.into(),
                score: 1.0,
                mtime: 0,
                stars: 0,
            });
        }
        // ZIP 内エントリ + ZIP ファイル自体は一覧に出さない
        state.accumulate_hit(&GlobalHit {
            path: zip_hit("c:/e/album.zip", "x.jpg"),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/f/plain.zip".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        let (items, metas) =
            build_flat_items(&state, crate::settings::SortOrder::FileName, &[true; 6]);
        assert_eq!(items.len(), 4, "ZIP 系 2 件は除外される");
        assert_eq!(items.len(), metas.len());
        // ファイル名順: 1.png → 2.jpg → clip.mp4 → doc.pdf
        let kinds: Vec<&str> = items
            .iter()
            .map(|it| match it {
                GridItem::Image(_) => "Image",
                GridItem::PdfFile(_) => "PdfFile",
                GridItem::Video(_) => "Video",
                _ => "Other",
            })
            .collect();
        assert_eq!(kinds, vec!["Image", "Image", "Video", "PdfFile"]);
    }

    /// build_flat_items: rating_filter で個々のヒットを絞れる (一覧ビューでは★有効、§6)。
    #[test]
    fn build_flat_items_applies_rating_filter() {
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/keep.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 3,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/drop.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 1,
        });
        let mut rf = [false; 6];
        rf[3] = true;
        let (items, _) = build_flat_items(&state, crate::settings::SortOrder::FileName, &rf);
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], GridItem::Image(p) if p.ends_with("keep.jpg")));
    }

    /// build_flat_items: SortOrder::DateDesc / DateAsc が GlobalHit.mtime を使って
    /// ソートする (§4.3.3、§5.2)。
    #[test]
    fn build_flat_items_sorts_by_mtime_for_date_order() {
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/mid.jpg".into(),
            score: 1.0,
            mtime: 200,
            stars: 0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/new.jpg".into(),
            score: 1.0,
            mtime: 300,
            stars: 0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/old.jpg".into(),
            score: 1.0,
            mtime: 100,
            stars: 0,
        });
        let names = |items: &[GridItem]| -> Vec<String> {
            items
                .iter()
                .map(|it| match it {
                    GridItem::Image(p) => p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_string(),
                    _ => String::new(),
                })
                .collect()
        };
        let (desc, _) = build_flat_items(&state, crate::settings::SortOrder::DateDesc, &[true; 6]);
        assert_eq!(names(&desc), vec!["new.jpg", "mid.jpg", "old.jpg"]);
        let (asc, _) = build_flat_items(&state, crate::settings::SortOrder::DateAsc, &[true; 6]);
        assert_eq!(names(&asc), vec!["old.jpg", "mid.jpg", "new.jpg"]);
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
            mtime: 0,
            stars: 0,
        });
        // 上記以外にもヒットを入れておく (別枝が干渉しないことを確認)
        state.accumulate_hit(&GlobalHit {
            path: "C:/root/year2024/jan/matches/Y.png".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });

        // Level 1: /root でドリル → year2024 のみ
        let (l1, _) = build_drilled_items(&state, Path::new("C:/root"), false, &[true; 6]);
        assert_eq!(l1.len(), 1, "level1 item count");
        assert!(matches!(&l1[0], GridItem::Folder(p) if p == &PathBuf::from("C:/root/year2024")));

        // Level 2: /root/year2024 → jan のみ (feb は枝刈り)
        let (l2, _) = build_drilled_items(&state, Path::new("C:/root/year2024"), false, &[true; 6]);
        assert_eq!(l2.len(), 1, "level2 item count");
        assert!(
            matches!(&l2[0], GridItem::Folder(p) if p == &PathBuf::from("C:/root/year2024/jan"))
        );

        // Level 3: /root/year2024/jan → matches のみ
        let (l3, _) =
            build_drilled_items(&state, Path::new("C:/root/year2024/jan"), false, &[true; 6]);
        assert_eq!(l3.len(), 1, "level3 item count");
        assert!(matches!(&l3[0],
                GridItem::Folder(p) if p == &PathBuf::from("C:/root/year2024/jan/matches")));

        // Level 4: /root/year2024/jan/matches → 画像 2 件が直下に並ぶ
        let (l4, _) = build_drilled_items(
            &state,
            Path::new("C:/root/year2024/jan/matches"),
            false,
            &[true; 6],
        );
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
            zip_hit("c:/archives/target.zip", "folder/pic1.jpg"),
            zip_hit("c:/archives/target.zip", "folder/pic2.jpg"),
            zip_hit("c:/archives/other.zip", "x.jpg"), // 別 ZIP
            "c:/loose/z.jpg".to_string(),              // 通常ファイル
        ] {
            state.accumulate_hit(&GlobalHit {
                path: p,
                score: 1.0,
                mtime: 0,
                stars: 0,
            });
        }
        let (items, _) = build_drilled_items(
            &state,
            Path::new("C:/archives/target.zip"),
            /*is_zip=*/ true,
            &[true; 6],
        );
        assert_eq!(items.len(), 2, "target.zip のエントリ数");
        for it in &items {
            assert!(matches!(it, GridItem::ZipImage { .. }));
            if let GridItem::ZipImage { zip_path, .. } = it {
                assert_eq!(zip_path, &PathBuf::from("C:/archives/target.zip"));
            }
        }
    }

    // -------------------------------------------------------------------
    // rating_filter (★) を build_drilled_items に渡したときの絞り込み挙動。
    // 2026-04 仕様: drilled view では★フィルタが「直下ファイル」と
    // 「サブフォルダのバッジ件数」両方に効く (件数 0 のサブフォルダは枝刈り)。
    // -------------------------------------------------------------------

    /// rating_filter で直下ファイル + サブフォルダ件数の両方が絞り込まれる。
    /// ★3 のみを有効にしたフィルタを渡すと:
    /// - 直下ファイル: ★3 のみ残る
    /// - サブフォルダバッジ: ★3 ヒット数のみカウント、0 件サブフォルダは枝刈り
    #[test]
    fn build_drilled_items_filters_direct_files_and_subfolder_counts_by_rating() {
        let mut state = GlobalSearchState::default();
        // 直下: ★3 1 枚 + ★1 1 枚 + ★なし 1 枚
        state.accumulate_hit(&GlobalHit {
            path: "C:/root/keep.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 3,
        });
        state.accumulate_hit(&GlobalHit {
            path: "C:/root/drop_low.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 1,
        });
        state.accumulate_hit(&GlobalHit {
            path: "C:/root/drop_unrated.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });
        // sub_keep: ★3 が 1 件含まれる → バッジ 1
        state.accumulate_hit(&GlobalHit {
            path: "C:/root/sub_keep/a.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 3,
        });
        state.accumulate_hit(&GlobalHit {
            path: "C:/root/sub_keep/b.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 1,
        });
        // sub_drop: ★3 ヒットなし → 枝刈り
        state.accumulate_hit(&GlobalHit {
            path: "C:/root/sub_drop/a.jpg".into(),
            score: 1.0,
            mtime: 0,
            stars: 0,
        });

        // ★3 のみ ON
        let mut rf = [false; 6];
        rf[3] = true;

        let (items, _) = build_drilled_items(&state, Path::new("C:/root"), false, &rf);
        // 期待: Folder("sub_keep") + Image("keep.jpg") のみ
        assert_eq!(items.len(), 2, "items 数");
        assert!(
            matches!(&items[0], GridItem::Folder(p) if p == &PathBuf::from("C:/root/sub_keep"))
        );
        assert!(matches!(&items[1], GridItem::Image(p) if p == &PathBuf::from("C:/root/keep.jpg")));
    }

    /// rating_filter が全 ON ([true; 6]) のときは何も絞らない (= 旧挙動と同じ)。
    #[test]
    fn build_drilled_items_no_rating_filter_is_passthrough() {
        let mut state = GlobalSearchState::default();
        for p in ["C:/root/a.jpg", "C:/root/b.jpg", "C:/root/sub/c.jpg"] {
            state.accumulate_hit(&GlobalHit {
                path: p.into(),
                score: 1.0,
                mtime: 0,
                stars: 0,
            });
        }
        let (items_all_on, _) =
            build_drilled_items(&state, Path::new("C:/root"), false, &[true; 6]);
        // sub フォルダ + 直下 2 件 = 3
        assert_eq!(items_all_on.len(), 3);
    }

    /// ZIP drilled view も同様に rating_filter で entries を絞れる。
    #[test]
    fn build_drilled_zip_items_filters_by_rating() {
        let sep = crate::search_norm::ZIP_ENTRY_SEP;
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: format!("c:/archives/target.zip{sep}keep.jpg"),
            score: 1.0,
            mtime: 0,
            stars: 3,
        });
        state.accumulate_hit(&GlobalHit {
            path: format!("c:/archives/target.zip{sep}drop.jpg"),
            score: 1.0,
            mtime: 0,
            stars: 1,
        });
        let mut rf = [false; 6];
        rf[3] = true;
        let (items, _) = build_drilled_items(
            &state,
            Path::new("C:/archives/target.zip"),
            /*is_zip=*/ true,
            &rf,
        );
        assert_eq!(items.len(), 1);
        if let GridItem::ZipImage { entry_name, .. } = &items[0] {
            assert_eq!(entry_name, "keep.jpg");
        } else {
            panic!("expected ZipImage");
        }
    }

    /// `hit_rating_key` がレーティング DB キー形式 (App::page_path_key 互換) に変換する。
    #[test]
    fn hit_rating_key_handles_zip_and_regular_paths() {
        // 通常ファイル
        let k = hit_rating_key("C:/photos/a.jpg");
        assert_eq!(
            k,
            crate::adjustment_db::normalize_path(Path::new("C:/photos/a.jpg"))
        );
        // ZIP エントリ (\x1F セパレータ)
        let zip_key = format!("c:/album.zip{}IMG.JPG", crate::search_norm::ZIP_ENTRY_SEP);
        let k2 = hit_rating_key(&zip_key);
        let expected = format!(
            "{}::{}",
            crate::adjustment_db::normalize_path(Path::new("c:/album.zip")),
            "img.jpg"
        );
        assert_eq!(k2, expected);
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

    /// Ctrl+G ストリーミング rebuild でサムネを使い回せるよう、
    /// `thumb_reuse_key` が path / variant ごとに決定的なキーを返すこと。
    /// 同じ Image パスは同じキー、異なる variant は別キーになる。
    #[test]
    fn thumb_reuse_key_distinguishes_variants_and_paths() {
        let img1 = GridItem::Image(PathBuf::from("c:/a.jpg"));
        let img1_dup = GridItem::Image(PathBuf::from("c:/a.jpg"));
        let img2 = GridItem::Image(PathBuf::from("c:/b.jpg"));
        let folder1 = GridItem::Folder(PathBuf::from("c:/a.jpg")); // 同じパスでも variant 別
        assert_eq!(thumb_reuse_key(&img1), thumb_reuse_key(&img1_dup));
        assert_ne!(thumb_reuse_key(&img1), thumb_reuse_key(&img2));
        assert_ne!(thumb_reuse_key(&img1), thumb_reuse_key(&folder1));
        // ZipSeparator はサムネがないので key を返さない (None)
        assert!(
            thumb_reuse_key(&GridItem::ZipSeparator {
                dir_display: "x".into(),
            })
            .is_none()
        );
    }

    /// `SearchContainer` は representative が変わったら別キー扱い (代表サムネが
    /// 切り替わったら使い回さず、新しい代表のサムネを生成し直す)。
    #[test]
    fn thumb_reuse_key_search_container_invalidates_on_representative_change() {
        let path = PathBuf::from("c:/folder");
        let rep1 = ContainerRepresentative {
            path: PathBuf::from("c:/folder/a.jpg"),
            zip_entry: None,
            pdf_page: None,
        };
        let rep2 = ContainerRepresentative {
            path: PathBuf::from("c:/folder/b.jpg"),
            zip_entry: None,
            pdf_page: None,
        };
        let make = |rep: Option<ContainerRepresentative>| GridItem::SearchContainer {
            path: path.clone(),
            kind: SearchContainerKind::Folder,
            hit_count: 0,
            representative: rep,
        };
        assert_eq!(
            thumb_reuse_key(&make(Some(rep1.clone()))),
            thumb_reuse_key(&make(Some(rep1.clone())))
        );
        assert_ne!(
            thumb_reuse_key(&make(Some(rep1))),
            thumb_reuse_key(&make(Some(rep2.clone())))
        );
        assert_ne!(
            thumb_reuse_key(&make(Some(rep2))),
            thumb_reuse_key(&make(None))
        );
    }

    // -----------------------------------------------------------------------
    // compute_search_video_candidates — respawn_search_video_thread 内の
    // 候補選択ロジック (Codex review 反映)。
    // -----------------------------------------------------------------------

    fn make_video(p: &str) -> GridItem {
        GridItem::Video(PathBuf::from(p))
    }

    /// `ThumbnailState::Loaded` を実際の `egui::TextureHandle` 付きで構築する。
    /// egui::Context::default() + load_texture でテスト用ダミーテクスチャを作れる
    /// パターン (src/app/tests.rs:poll_fs_nav_lock_waits_for_items_generation_bump
    /// と同じ手法)。
    fn make_loaded_state() -> ThumbnailState {
        let ctx = eframe::egui::Context::default();
        let tex = ctx.load_texture(
            "test_loaded",
            eframe::egui::ColorImage::filled([1, 1], eframe::egui::Color32::WHITE),
            eframe::egui::TextureOptions::LINEAR,
        );
        ThumbnailState::Loaded {
            tex,
            from_cache: false,
            rendered_at_px: 64,
            source_dims: None,
        }
    }

    #[test]
    fn candidates_skips_already_loaded_videos() {
        // Codex notes #1 反映: 実際の `Loaded` 状態 (TextureHandle 付き) を使い、
        // 「Loaded の動画は候補から除外」を直接検証する。
        let items = vec![make_video("c:/loaded.mp4"), make_video("c:/pending.mp4")];
        let thumbs = vec![make_loaded_state(), ThumbnailState::Pending];
        let pins = HashSet::new();
        let result = App::compute_search_video_candidates(&items, &thumbs, &pins, false);
        assert_eq!(result.len(), 1, "Loaded 動画は除外、Pending のみ候補");
        assert_eq!(result[0].1, PathBuf::from("c:/pending.mp4"));
        assert_eq!(result[0].0, 1, "Pending 動画の元 idx (=1) が保持される");
    }

    #[test]
    fn candidates_includes_pending_evicted_failed_states() {
        // `Loaded` 以外の状態は全て候補に入る (Pending / Evicted / Failed / Requested)。
        // これは「Loaded のみ除外」ルールの対偶確認。
        let items = vec![
            make_video("c:/a.mp4"),
            make_video("c:/b.mp4"),
            make_video("c:/c.mp4"),
        ];
        let thumbs = vec![
            ThumbnailState::Pending,
            ThumbnailState::Evicted,
            ThumbnailState::Failed,
        ];
        let pins = HashSet::new();
        let result = App::compute_search_video_candidates(&items, &thumbs, &pins, false);
        assert_eq!(result.len(), 3, "Pending/Evicted/Failed は全て候補に入る");
    }

    #[test]
    fn candidates_non_video_items_are_ignored() {
        let items = vec![
            GridItem::Image(PathBuf::from("c:/img.jpg")),
            GridItem::Folder(PathBuf::from("c:/folder")),
            make_video("c:/v.mp4"),
        ];
        let thumbs = vec![ThumbnailState::Pending; 3];
        let pins = HashSet::new();
        let result = App::compute_search_video_candidates(&items, &thumbs, &pins, false);
        assert_eq!(result.len(), 1, "非動画 (Image/Folder) は候補に入らない");
        assert_eq!(result[0].0, 2, "Video の元 idx (=2) が保持される");
    }

    #[test]
    fn candidates_streaming_keeps_only_pin_having_videos() {
        // streaming=true のとき、pin DB に登録のない動画は除外される。
        let items = vec![make_video("c:/no_pin.mp4"), make_video("c:/has_pin.mp4")];
        let thumbs = vec![ThumbnailState::Pending, ThumbnailState::Pending];
        let mut pins = HashSet::new();
        pins.insert(PathBuf::from("c:/has_pin.mp4"));
        let result = App::compute_search_video_candidates(&items, &thumbs, &pins, true);
        assert_eq!(result.len(), 1, "streaming 中は pin 持ち動画のみ候補");
        assert_eq!(result[0].1, PathBuf::from("c:/has_pin.mp4"));
        assert_eq!(result[0].0, 1, "pin 持ち動画の元 idx (=1) が保持される");
    }

    #[test]
    fn candidates_done_includes_all_not_loaded_videos() {
        // streaming=false (= global_search.done == true) のとき、pin 有無に関わらず
        // 未 Loaded 動画は全て候補に入る (Shell API 経路で抽出)。
        let items = vec![make_video("c:/no_pin.mp4"), make_video("c:/has_pin.mp4")];
        let thumbs = vec![ThumbnailState::Pending, ThumbnailState::Pending];
        let mut pins = HashSet::new();
        pins.insert(PathBuf::from("c:/has_pin.mp4"));
        let result = App::compute_search_video_candidates(&items, &thumbs, &pins, false);
        assert_eq!(result.len(), 2, "done 状態は pin の有無に関わらず全て候補");
    }

    #[test]
    fn candidates_empty_input_returns_empty() {
        let result = App::compute_search_video_candidates(&[], &[], &HashSet::new(), false);
        assert!(result.is_empty());
    }

    #[test]
    fn candidates_streaming_no_pins_returns_empty() {
        // streaming=true で動画はあるが pin が 1 件もない → spawn 不要。
        let items = vec![make_video("c:/a.mp4"), make_video("c:/b.mp4")];
        let thumbs = vec![ThumbnailState::Pending, ThumbnailState::Pending];
        let pins = HashSet::new();
        let result = App::compute_search_video_candidates(&items, &thumbs, &pins, true);
        assert!(
            result.is_empty(),
            "streaming で pin 持ちゼロのとき候補は空 (spawn しない経路)"
        );
    }

    #[test]
    fn candidates_preserves_original_index_after_filtering() {
        // 候補に入る idx は元の items 内 idx と一致する (filter_map の enumerate)。
        let items = vec![
            GridItem::Image(PathBuf::from("c:/x.jpg")), // idx 0
            make_video("c:/a.mp4"),                     // idx 1
            GridItem::Folder(PathBuf::from("c:/d")),    // idx 2
            make_video("c:/b.mp4"),                     // idx 3
        ];
        let thumbs = vec![ThumbnailState::Pending; 4];
        let pins = HashSet::new();
        let result = App::compute_search_video_candidates(&items, &thumbs, &pins, false);
        let indices: Vec<usize> = result.iter().map(|(i, _, _)| *i).collect();
        assert_eq!(indices, vec![1, 3], "Video の元 idx (1, 3) が保持される");
    }
}
