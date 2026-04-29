use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    mpsc,
};

/// Condvar 付きキュー: ワーカーはキューが空のとき sleep ポーリングではなく wait() で待機し、
/// push 側が notify_one() で起こす。
pub(crate) type NotifyQueue = (Mutex<Vec<LoadRequest>>, Condvar);

/// Ctrl+↑↓ フォルダナビゲーションの発火元モード。DFS 完了後に mode に応じて
/// 異なる後処理 (grid は load_folder のみ、fullscreen は fs 再オープン、favsearch は
/// sibling fallback 付き) を行うため、`FolderNavPending` に記憶させる。
#[derive(Clone, Debug)]
pub(crate) enum FolderNavMode {
    /// 通常グリッド。DFS 結果をそのまま `load_folder` する。
    Grid,
    /// フルスクリーン表示中。DFS 結果で `load_folder` 後、先頭/末尾の画像系
    /// アイテムを `open_fullscreen` で再表示する。
    Fullscreen,
    /// お気に入り検索コンテキスト。DFS 結果が root 配下なら `nav_stack` に積んで
    /// `load_folder`、root 外なら `favsearch_navigate_sibling` にフォールバックする。
    Favsearch { root: PathBuf },
}

/// DFS スレッドから UI スレッドに送るメッセージ。結果 (Option) と、
/// ヒット先が通常ディレクトリだった場合の事前スキャン結果を載せる。
/// 事前スキャンがあれば UI スレッドの `load_folder` は `read_dir` をスキップできる。
pub(crate) struct FolderNavThreadResult {
    outcome: Option<crate::folder_tree::FolderNavOutcome>,
    /// `outcome.path` がディレクトリのときのみ Some。ZIP/PDF ファイルは
    /// 専用ローダーに委譲するので事前スキャンしない。
    scanned: Option<ScannedDir>,
}

/// ZIP 中央ディレクトリ列挙の非同期ハンドル。ネスト ZIP が多いと `enumerate_image_entries`
/// が数秒かかるため worker に逃がす。Drop で `cancel` を立てるので、新しい load_folder が
/// 来たら自動で破棄される (worker は cancel を見て早期 return する)。
pub(crate) struct ZipEnumeratePending {
    pub zip_path: PathBuf,
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<Result<Vec<crate::zip_loader::ZipImageEntry>, String>>,
    /// 結果受信後のグルーピング/ソートで使う。起動時の `settings.sort_order` をキャプチャ
    /// (worker 実行中にユーザーが設定を変えても、このロード分は開始時の値で処理する)。
    pub sort_order: crate::settings::SortOrder,
}

impl Drop for ZipEnumeratePending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// バックグラウンドで走らせる `IndexerManager::new` の状態。
/// 起動フェーズ判定は `App::startup_init.is_some()` で行う (Loading の間だけ Some)。
pub(crate) struct StartupInitPending {
    rx: mpsc::Receiver<Option<crate::indexer_manager::IndexerManager>>,
    started_at: std::time::Instant,
}

impl StartupInitPending {
    fn try_recv(&self) -> Result<Option<crate::indexer_manager::IndexerManager>, mpsc::TryRecvError> {
        self.rx.try_recv()
    }
    fn elapsed_ms(&self) -> f64 {
        self.started_at.elapsed().as_secs_f64() * 1000.0
    }
}

/// 進行中の更新チェックワーカー。`rx` が結果を運び、`manual` は失敗時の UI 表示判定。
/// `App::update_check_pending = None` で両方をまとめて落とせるため、リセットの整合性が
/// 取りやすい (rx だけ落として manual を残すミスが起きない)。
pub(crate) struct UpdateCheckPending {
    pub(crate) rx: mpsc::Receiver<Result<crate::update_check::UpdateInfo, String>>,
    pub(crate) manual: bool,
}

/// 起動オーバーレイ用の `StartupProgressHook` を作る共通ヘルパー。
/// `IndexerManager::new` が各 sub-step の前に呼び、Mutex 内の文字列を更新する。
fn make_progress_hook(progress: Arc<Mutex<String>>) -> crate::indexer_manager::StartupProgressHook {
    Arc::new(move |s: &str| {
        if let Ok(mut p) = progress.lock() {
            *p = s.to_string();
        }
    })
}

/// 非同期で走っている `navigate_folder_with_skip` ワーカーの状態。
pub(crate) struct FolderNavPending {
    /// DFS キャンセル用トークン。連打の累積・モード切替・フォルダ強制切替で立てる。
    cancel: Arc<AtomicBool>,
    /// DFS スレッドからの結果チャネル。
    rx: mpsc::Receiver<FolderNavThreadResult>,
    /// この DFS ステップの方向 (forward=↓, backward=↑)。結果の後処理に使う。
    forward: bool,
    /// この DFS が発火された起点モード。結果の後処理に使う。
    mode: FolderNavMode,
}

/// `poll_folder_nav` がワーカー完了を検知したときに返す情報。
pub(crate) struct FolderNavResult {
    /// DFS が見つけた次フォルダ (None なら DFS が尽きた)。
    pub path: Option<PathBuf>,
    /// `folder_should_stop` をパスしたフォルダ (= 画像/動画/ZIP/PDF あり) か。
    /// `false` のときは skip_limit 尽きまたは DFS 末端でのフォールバック、または
    /// 結果自体が `None` (path も None)。Fullscreen モードの後処理は false なら
    /// 移動を取りやめて境界ヒントを出す。
    pub hit_image_folder: bool,
    /// 起点の方向。
    pub forward: bool,
    /// 起点モード。
    pub mode: FolderNavMode,
    /// DFS スレッドで事前走査した `path` の中身。`path` がディレクトリのときのみ Some。
    /// UI スレッドの `load_folder` で read_dir をスキップするために使う。
    pub scanned: Option<ScannedDir>,
}

/// 非同期メタデータ読み込み (フルスクリーン表示対象画像の AI/EXIF/XMP) の状態。
///
/// `open_fullscreen` から起動され、`poll_metadata_load` で結果を受信する。
/// **背景**: XMP リーダーは JPEG/PNG 全体を読むため (`read_tweet_info`)、
/// 20MP 級の写真で UI スレッドが 100ms 級でブロックしていた
/// (AI metadata と EXIF は数 ms だが、XMP が主犯)。
/// 新 fullscreen idx が開かれたら旧 pending を cancel する (連打時は最新のみ処理)。
pub(crate) struct MetadataLoadPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<MetadataLoadResult>,
}

impl MetadataLoadPending {
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// メタデータ読み込みワーカーの結果。キー (metadata_cache_key の形式) と
/// 3 つのパース結果をまとめて返す。UI 側はそれぞれのキャッシュに投入する。
pub(crate) struct MetadataLoadResult {
    key: String,
    metadata: Option<crate::png_metadata::AiMetadata>,
    exif: Option<crate::exif_reader::ExifInfo>,
    xmp: Option<crate::xmp_reader::XmpTweetInfo>,
}

/// 非同期お気に入り検索 (Ctrl+S) の状態。
///
/// `search_index_db.search()` は SQLite クエリでインデックス化された名前検索だが、
/// 大規模お気に入りツリーでは 10〜100ms 級のブロックになり得る。さらにその後に
/// 走る `start_loading_items` が数百ms の UI ブロック源になるので、DB 問い合わせ
/// だけでもバックグラウンドに退避する。
pub(crate) struct FavSearchPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<rusqlite::Result<Vec<crate::search_index_db::IndexEntry>>>,
}

/// ディレクトリ走査結果 (read_dir + 各エントリ metadata 取得の成果物)。
///
/// Ctrl+↑↓ 移動時は DFS スレッドで事前に走査しておき、UI スレッドの
/// `load_folder` で `read_dir` を走らせずに items を組み立てるために使う。
/// 通常パス (ユーザーが明示的に開いたフォルダ等) では `scan_directory` を
/// UI スレッドで呼んで即座に生成する。
pub(crate) struct ScannedDir {
    /// (GridItem, (mtime, file_size)) の対。GridItem は Folder / ZipFile /
    /// PdfFile / ConvertibleArchive のいずれか。load_folder 内でソートされる。
    pub folders: Vec<(GridItem, Option<(i64, i64)>)>,
    /// (path, is_video, mtime, file_size) のタプル。load_folder 内で sort_order
    /// 設定に基づいてソートされる。
    pub all_media: Vec<(PathBuf, bool, i64, i64)>,
}

/// ディレクトリ走査: `read_dir` + 各エントリの `file_type()` / `metadata()` 呼び出し。
///
/// **Windows パフォーマンス上の注意**: `entry.file_type()` と `entry.metadata()` は
/// `FindFirstFile`/`FindNextFile` が返した WIN32_FIND_DATA をそのまま再利用するので
/// syscall は不要。対して `Path::is_dir()` は都度 `GetFileAttributes` を呼び出すため
/// 数百枚のフォルダで per-entry 1-5ms、合計 500-1000ms のブロック源になる
/// (AI 画像フォルダで計測実績あり)。必ず `entry.file_type()` 側を使うこと。
/// 方針は [docs/ui-responsiveness.md §1.1](../docs/ui-responsiveness.md) にまとめてある。
pub(crate) fn scan_directory(path: &std::path::Path) -> ScannedDir {
    let mut folders: Vec<(GridItem, Option<(i64, i64)>)> = Vec::new();
    let mut all_media: Vec<(PathBuf, bool, i64, i64)> = Vec::new();

    let Ok(entries) = std::fs::read_dir(path) else {
        return ScannedDir { folders, all_media };
    };
    for entry in entries.flatten() {
        // file_type() は FindFirstFile のキャッシュ読み (syscall なし)。
        // metadata() も同様にキャッシュから返るが、失敗しても fallback 0 で続行する。
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        let p = entry.path();
        if is_dir {
            let meta = entry.metadata().ok();
            let mtime = meta
                .as_ref()
                .map_or(0, |m| crate::ui_helpers::mtime_secs(m));
            folders.push((GridItem::Folder(p), Some((mtime, 0))));
        } else if is_apple_double(&p) {
            // macOS/iPhone AppleDouble メタデータ — スキップ
        } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            let ext_lower = ext.to_ascii_lowercase();
            let meta = entry.metadata().ok();
            let mtime = meta
                .as_ref()
                .map_or(0, |m| crate::ui_helpers::mtime_secs(m));
            let file_size = meta.map_or(0, |m| m.len() as i64);
            if crate::folder_tree::is_recognized_image_ext(&ext_lower) {
                all_media.push((p, false, mtime, file_size));
            } else if SUPPORTED_VIDEO_EXTENSIONS.contains(&ext_lower.as_str()) {
                all_media.push((p, true, mtime, file_size));
            } else if ext_lower == "zip" {
                folders.push((GridItem::ZipFile(p), Some((mtime, file_size))));
            } else if ext_lower == "pdf" {
                folders.push((GridItem::PdfFile(p), Some((mtime, file_size))));
            } else if let Some(fmt) =
                crate::archive_converter::ArchiveFormat::from_extension(&ext_lower)
            {
                folders.push((
                    GridItem::ConvertibleArchive {
                        path: p,
                        format: fmt,
                    },
                    Some((mtime, file_size)),
                ));
            }
        }
    }
    ScannedDir { folders, all_media }
}

/// `ScannedDir` の内容シグネチャ (path + mtime + size + 種別) を u64 ハッシュ化する。
/// フォーカス復帰時の差分判定用。`read_dir` の返却順は NTFS で保証されないので
/// 並び順非依存にするため path で明示的にソートしてからハッシュする。
/// プロセス内比較専用 (DefaultHasher は Rust バージョン間で安定でないため永続化しない)。
///
/// 既知の制限: mtime は `mtime_secs` (秒精度) を使うので、同一秒内に同サイズで
/// 上書きされた場合は差分検知できず再ロードがスキップされる。画像ファイルが
/// 偶然同サイズで <1 秒以内に書き換わる現実的なシナリオは稀なため許容している。
/// 必要なら `metadata.modified()` の SystemTime を秒+nanos で取り直す拡張が可能。
pub(crate) fn signature_from_scan(scan: &ScannedDir) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut entries: Vec<(&std::ffi::OsStr, i64, i64, &'static str)> =
        Vec::with_capacity(scan.folders.len() + scan.all_media.len());
    for (item, meta) in &scan.folders {
        let (path, kind) = match item {
            GridItem::Folder(p) => (p.as_os_str(), "folder"),
            GridItem::ZipFile(p) => (p.as_os_str(), "zip"),
            GridItem::PdfFile(p) => (p.as_os_str(), "pdf"),
            GridItem::ConvertibleArchive { path, .. } => (path.as_os_str(), "archive"),
            _ => continue,
        };
        let (mtime, size) = meta.unwrap_or((0, 0));
        entries.push((path, mtime, size, kind));
    }
    for (p, is_video, mtime, size) in &scan.all_media {
        let kind = if *is_video { "video" } else { "image" };
        entries.push((p.as_os_str(), *mtime, *size, kind));
    }
    entries.sort();
    let mut hasher = DefaultHasher::new();
    entries.len().hash(&mut hasher);
    for e in &entries {
        e.hash(&mut hasher);
    }
    hasher.finish()
}

/// 3 種の検索モード。相互排他制御 (`close_other_search_bars`) 用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchMode {
    /// Ctrl+F: ローカルフォルダのメタデータ検索
    LocalMeta,
    /// Ctrl+S: お気に入り配下の名前検索
    Favsearch,
    /// Ctrl+G: お気に入り全体のメタデータ検索
    Global,
}

/// 非同期メタデータ検索 (Ctrl+F) の状態。
///
/// 背景: 検索マッチの判定には `png_metadata::build_searchable_from_path` や
/// `xmp_reader::read_tweet_info` など、ファイル I/O を伴う読み取りが必要。
/// フォルダに数百〜数千枚の画像があると UI スレッドで数秒ブロックしていたため、
/// バックグラウンドスレッドで実行して結果を `poll_search` で受け取る構造に変更した。
pub(crate) struct SearchPending {
    /// 検索キャンセル用トークン。新クエリ / 検索バー閉じ / フォルダ切替で立てる。
    cancel: Arc<AtomicBool>,
    /// ワーカースレッドから結果を受け取るチャネル。
    rx: mpsc::Receiver<SearchThreadResult>,
}

impl SearchPending {
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// 検索ワーカースレッドからの結果。
pub(crate) enum SearchThreadResult {
    /// 検索完了。`matches` は該当アイテムの原インデックス集合、
    /// `xmp_additions` はワーカーが新規に読み取った XMP エントリの (key, value) 対で、
    /// UI スレッドで `xmp_cache` にマージする。
    Done {
        matches: std::collections::HashSet<usize>,
        xmp_additions: Vec<(String, Option<crate::xmp_reader::XmpTweetInfo>)>,
    },
}

impl FolderNavMode {
    /// perf ログに載せる短い識別子。variant 名のみでパスなどを含めず、
    /// ユーザーパスが診断ログに混入しないようにする。
    pub fn perf_tag(&self) -> &'static str {
        match self {
            FolderNavMode::Grid => "grid",
            FolderNavMode::Fullscreen => "fullscreen",
            FolderNavMode::Favsearch { .. } => "favsearch",
        }
    }
}

/// モードの種類 (variant) のみを比較する。`Favsearch { root }` は root 違いでも
/// 同一モードとみなす (同一バースト中に root が変わるのはエッジケースだが、
/// 変わったとしても favsearch 動作は継続するので区別する意味が薄い)。
fn folder_nav_mode_same_kind(a: &FolderNavMode, b: &FolderNavMode) -> bool {
    matches!(
        (a, b),
        (FolderNavMode::Grid, FolderNavMode::Grid)
            | (FolderNavMode::Fullscreen, FolderNavMode::Fullscreen)
            | (
                FolderNavMode::Favsearch { .. },
                FolderNavMode::Favsearch { .. }
            )
    )
}

use eframe::egui;

use crate::folder_tree::{
    SUPPORTED_VIDEO_EXTENSIONS, is_apple_double, navigate_folder_with_skip, next_folder_dfs,
    prev_folder_dfs, walk_dirs_recursive,
};

// キャッシュキー定数は thumb_loader.rs に定義 (ベンチマーク bin からも参照するため)
pub(crate) use crate::thumb_loader::{
    CACHE_KEY_FOLDER, CACHE_KEY_PDF, CACHE_KEY_SEARCH_REP, CACHE_KEY_ZIP,
};

/// レーティングフィルタを 1 アイテムに適用し、可視かを返す。
///
/// レーティング対象 (コンテナ + 画像系) は全 6 バケット (★なし + ★1〜5) で判定し、
/// 非レーティング対象 (Video / Separator / ConvertibleArchive 等) は常に可視。
/// 「★5 のみ表示」操作で未評価フォルダが残らないよう、コンテナもページ系と
/// 同じ厳密フィルタに揃えた (★なしフォルダに入りたいときは「なし」を ON に戻す)。
///
/// 前提: 呼び出し側で `rating_filter` が全 ON でないことを確認済み
/// (全 ON なら全アイテム可視なのでそもそも呼ばない)。
fn passes_rating_filter(
    item: &GridItem,
    stars: u8,
    rating_filter: &[bool; 6],
) -> bool {
    if item.accepts_rating() {
        let s = stars as usize;
        s <= 5 && rating_filter[s]
    } else {
        true
    }
}

/// `current` が `anchor` 配下 (= 同一 or 子孫) かを case-insensitive に判定する。
/// Windows の case-insensitive FS 対応 (`C:\Photos` と `c:\photos` を同一扱い)。
/// ドライブ文字は保持するので、cross-drive の偶然一致を起こさない
/// (例: `C:\books\vol1.zip` と `D:\books\vol1.zip` は別扱い)。
///
/// component-boundary も守る (`/books/book-a` と `/books/book-a-extra` を別扱い)。
fn path_in_subtree_ci(current: &std::path::Path, anchor: &std::path::Path) -> bool {
    let norm = |p: &std::path::Path| p.to_string_lossy().to_lowercase().replace('\\', "/");
    let cur = norm(current);
    let anc = norm(anchor);
    if cur == anc {
        return true;
    }
    cur.strip_prefix(&anc)
        .is_some_and(|tail| tail.starts_with('/'))
}

/// パスからファイル名のステム部分を小文字で取得するヘルパー。
fn stem_lower(path: &std::path::Path) -> String {
    path.file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// ファイルパスが PNG 拡張子か (大文字小文字無視)。
fn is_png_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
}

/// ZIP エントリ名が PNG 拡張子か (大文字小文字無視)。
fn is_png_entry(entry_name: &str) -> bool {
    entry_name
        .rsplit_once('.')
        .is_some_and(|(_, e)| e.eq_ignore_ascii_case("png"))
}

/// フルスクリーン画像のメタデータ (AI プロンプト / EXIF / XMP) を読み込むワーカー本体。
/// ZipImage は ZIP エントリを 1 回だけ開いて 3 パーサー間で bytes を共有する。
/// それ以外 (Image / Video) はファイルを直接パーサーに渡す。パーサー側で
/// 必要に応じて full-file read が行われる (XMP の JPEG/PNG は全体読み)。
fn run_metadata_load(
    key: String,
    item: GridItem,
    hidden: &[String],
    cancel: &AtomicBool,
) -> Option<MetadataLoadResult> {
    // 各段で cancel チェック。ZIP の bytes 読み → AI → EXIF → XMP の順で重い。
    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    let (metadata, exif, xmp) = match &item {
        GridItem::Image(p) => {
            let metadata = crate::png_metadata::extract_metadata(p);
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let exif = crate::exif_reader::read_exif(p, hidden);
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let xmp = crate::xmp_reader::read_tweet_info(p);
            (metadata, exif, xmp)
        }
        GridItem::Video(p) => {
            // 動画は AI/EXIF なし、XMP のみ (mXD が MP4/MOV に X/Twitter 情報を埋める)
            let xmp = crate::xmp_reader::read_tweet_info(p);
            (None, None, xmp)
        }
        GridItem::ZipImage {
            zip_path,
            entry_name,
        } => {
            // ZIP エントリは 1 回展開して bytes を 3 パーサーで共有する
            let bytes = crate::zip_loader::read_entry_bytes(zip_path, entry_name).ok();
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let metadata = bytes
                .as_ref()
                .and_then(|b| crate::png_metadata::extract_metadata_from_bytes(b));
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let exif = bytes
                .as_ref()
                .and_then(|b| crate::exif_reader::read_exif_from_bytes(b, hidden));
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let xmp = bytes
                .as_ref()
                .and_then(|b| crate::xmp_reader::read_tweet_info_from_bytes(b));
            (metadata, exif, xmp)
        }
        _ => (None, None, None),
    };

    Some(MetadataLoadResult {
        key,
        metadata,
        exif,
        xmp,
    })
}

/// Ctrl+F メタデータ検索のワーカー本体。UI スレッドから spawn され、結果は
/// `SearchPending.rx` で受信される。`cancel` が立ったら中断して Cancelled を返す
/// (呼び出し側はキャンセル時に Pending をクリアするので Done のみ送る実装でも OK)。
fn run_metadata_search(
    tokens: &[crate::search_query::Token],
    items: &[GridItem],
    xmp_snapshot: &std::collections::HashMap<String, Option<crate::xmp_reader::XmpTweetInfo>>,
    _fts_meta: Option<&std::sync::Arc<crate::fts_meta::FtsMetaDb>>,
    target: &crate::fts_index::SearchTarget,
    mode: crate::search_query::MatchMode,
    cancel: &AtomicBool,
) -> SearchThreadResult {
    // INDEX_VERSION=5 で fts_meta.db に原文が無くなったため、Ctrl+F は fast path を
    // 持たず常に on-demand (PNG/EXIF/XMP/dc:subject 直読み) で判定する。表示中の
    // 数十〜数千件しか触らないので体感影響は小さい。PDF の target=PdfMeta 絞り込みは
    // off になり、お気に入り未登録の PDF を Ctrl+F する動線に合わせて「常に表示」扱い。
    let mut matches: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut xmp_additions: Vec<(String, Option<crate::xmp_reader::XmpTweetInfo>)> = Vec::new();
    let mut additions_lookup: std::collections::HashMap<
        String,
        Option<crate::xmp_reader::XmpTweetInfo>,
    > = std::collections::HashMap::new();
    let mut zip_png_groups: std::collections::HashMap<PathBuf, Vec<(usize, String)>> =
        std::collections::HashMap::new();

    // Pass 1: 構造アイテム + ZIP/PDF 系 (cheap な分類のみ、I/O なし)
    for (idx, item) in items.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return SearchThreadResult::Done {
                matches,
                xmp_additions,
            };
        }
        match item {
            GridItem::Folder(_)
            | GridItem::ZipFile(_)
            | GridItem::ConvertibleArchive { .. }
            | GridItem::ZipSeparator { .. }
            | GridItem::SearchContainer { .. }
            | GridItem::PdfFile(_) => {
                // PDF は fast path 廃止に伴い、ナビ用途を壊さないために常に残す。
                matches.insert(idx);
            }
            GridItem::Image(_) | GridItem::Video(_) => {
                // Pass 2 で処理
            }
            GridItem::ZipImage {
                zip_path,
                entry_name,
            } => {
                if is_png_entry(entry_name) {
                    zip_png_groups
                        .entry(zip_path.clone())
                        .or_default()
                        .push((idx, entry_name.clone()));
                } else {
                    let name = crate::zip_loader::entry_basename(entry_name);
                    if crate::search_query::matches_with_mode(tokens, name, mode) {
                        matches.insert(idx);
                    }
                }
            }
            GridItem::PdfPage { .. } => {
                if crate::search_query::matches_with_mode(tokens, &item.name(), mode) {
                    matches.insert(idx);
                }
            }
        }
    }

    // Pass 2: Image/Video — cheap hay で決まらなければ XMP を lazy 読み取り (ファイル I/O)
    //
    // §19 target フィルタ対応: target が全ソース (All) なら従来挙動。単一ソース選択時は
    // そのソース由来の文字列だけで hay を作り、非対象ソースの I/O もスキップする。
    let use_name = target.includes(crate::fts_index::SourceKind::Filename);
    let use_png = target.includes(crate::fts_index::SourceKind::PngPrompt);
    let use_exif = target.includes(crate::fts_index::SourceKind::Exif);
    let use_xmp = target.includes(crate::fts_index::SourceKind::XmpTweet);
    let use_tags = target.includes(crate::fts_index::SourceKind::Tags);
    // 画像 / Video 用の fallback 経路は name/png/exif/xmp/tags のいずれかが対象でないと
    // 計算結果が常に空になる。PdfMeta-only 等で無駄な per-file 走査を避ける。
    // Tags も含めておかないと、target=Tags のときに未インデックス画像が全件 skip される。
    let fallback_contributes = use_name || use_png || use_exif || use_xmp || use_tags;
    for (idx, item) in items.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return SearchThreadResult::Done {
                matches,
                xmp_additions,
            };
        }
        let (path, is_image) = match item {
            GridItem::Image(p) => (p, true),
            GridItem::Video(p) => (p, false),
            _ => continue,
        };
        // INDEX_VERSION=5 で fts_meta fast path は廃止 (原文は Tantivy 側 STORED)。
        // Ctrl+F は表示中の数十〜数千件しか触らないので、毎回 on-demand 経路に回す。
        // target が画像系ソース
        // (Filename/PngPrompt/Exif/XmpTweet) を一つも含まないなら、fallback hay は常に
        // 空になり matches は必ず false → file I/O もバイト走査も全て無駄。
        if !fallback_contributes {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        // 最初は PNG tEXt だけ読む (cheap hay)。EXIF / XMP は NeedsMore の時だけ lazy 読み。
        let meta_text = if is_image && use_png && is_png_path(path) {
            crate::png_metadata::build_searchable_from_path(path)
        } else {
            String::new()
        };
        let name_for_hay = if use_name { name.as_str() } else { "" };
        let hay_no_xmp = hay_of(&meta_text, name_for_hay, None);
        match crate::search_query::decide_partial_with_mode(tokens, &hay_no_xmp, mode) {
            crate::search_query::PartialResult::Decided(true) => {
                matches.insert(idx);
            }
            crate::search_query::PartialResult::Decided(false) => {}
            crate::search_query::PartialResult::NeedsMore => {
                let key = crate::adjustment_db::normalize_path(path);
                // XMP は target に含まれる場合のみ読む (I/O 節約)
                let xmp_opt = if use_xmp {
                    if let Some(cached) = xmp_snapshot.get(&key) {
                        cached.clone()
                    } else if let Some(added) = additions_lookup.get(&key) {
                        added.clone()
                    } else {
                        let xmp = crate::xmp_reader::read_tweet_info(path);
                        additions_lookup.insert(key.clone(), xmp.clone());
                        xmp_additions.push((key.clone(), xmp.clone()));
                        xmp
                    }
                } else {
                    None
                };
                // EXIF も同じく target に含まれる時だけ
                let mut extended_meta = meta_text.clone();
                if is_image && use_exif {
                    if let Some(exif) = crate::exif_reader::read_exif(path, &[]) {
                        let exif_part = exif_hay(&exif);
                        if !exif_part.is_empty() {
                            if !extended_meta.is_empty() {
                                extended_meta.push('\n');
                            }
                            extended_meta.push_str(&exif_part);
                        }
                    }
                }
                // target が Tags を含む場合、未インデックスの画像でも dc:subject を
                // 直読みして hay に載せる (Ctrl+F で tag 絞り込みを機能させる)。
                if is_image && use_tags {
                    let tags_text =
                        crate::ingest_text::build_tags_column(&crate::xmp_reader::read_dc_subject(path));
                    if !tags_text.is_empty() {
                        if !extended_meta.is_empty() {
                            extended_meta.push('\n');
                        }
                        extended_meta.push_str(&tags_text);
                    }
                }
                let hay = hay_of(&extended_meta, name_for_hay, xmp_opt.as_ref());
                if crate::search_query::matches_with_mode(tokens, &hay, mode) {
                    matches.insert(idx);
                }
            }
        }
    }

    // Pass 3: ZIP 内 PNG — ZIP を 1 回開いてまとめて処理 (ファイル I/O)
    // target が Filename/PngPrompt/XmpTweet を一つも含まない場合は hay が確定で空になり
    // ヒット不可なので、ZIP を開く前に全スキップする。
    let zip_entry_needs_bytes = use_png || use_xmp;
    if fallback_contributes {
        for (zip_path, entries) in zip_png_groups {
            if cancel.load(Ordering::Relaxed) {
                return SearchThreadResult::Done {
                    matches,
                    xmp_additions,
                };
            }
            // バイト読み取りが不要 (name だけで判定) なら archive は開かない。
            let mut direct_archive = if zip_entry_needs_bytes {
                crate::zip_loader::open_archive(&zip_path).ok()
            } else {
                None
            };
            for (idx, entry_name) in entries {
                if cancel.load(Ordering::Relaxed) {
                    return SearchThreadResult::Done {
                        matches,
                        xmp_additions,
                    };
                }
                let (meta_text, xmp) = if zip_entry_needs_bytes {
                    let is_nested = entry_name.to_ascii_lowercase().contains(".zip/");
                    let bytes_result = if is_nested {
                        crate::zip_loader::read_entry_bytes(&zip_path, &entry_name)
                    } else if let Some(archive) = direct_archive.as_mut() {
                        crate::zip_loader::read_entry_from_archive(archive, &entry_name)
                    } else {
                        crate::zip_loader::read_entry_bytes(&zip_path, &entry_name)
                    };
                    match bytes_result {
                        Ok(bytes) => {
                            let png = if use_png {
                                crate::png_metadata::build_searchable_from_bytes(&bytes)
                            } else {
                                String::new()
                            };
                            let xmp = if use_xmp {
                                crate::xmp_reader::read_tweet_info_from_bytes(&bytes)
                            } else {
                                None
                            };
                            (png, xmp)
                        }
                        Err(_) => (String::new(), None),
                    }
                } else {
                    (String::new(), None)
                };
                let entry_name_str = crate::zip_loader::entry_basename(&entry_name);
                let name_for_hay = if use_name { entry_name_str } else { "" };
                if crate::search_query::matches_with_mode(
                    tokens,
                    &hay_of(&meta_text, name_for_hay, xmp.as_ref()),
                    mode,
                ) {
                    matches.insert(idx);
                }
            }
        }
    }

    SearchThreadResult::Done {
        matches,
        xmp_additions,
    }
}

/// メタデータ文字列とファイル名を改行で繋いだ検索対象文字列を構築する。
/// mXD が埋めた XMP tweet 情報 (本文・投稿者・引用元) があれば末尾に追記する。
///
/// **Codex round-8 Should-fix #1 + round-9 #1 対応**:
/// fts_meta.db fast path (ingest_text::build_all_text_for_file → append_xmp) と互換な
/// 検索対象を作る。EXIF は呼び出し側で meta_text に含めて渡し、XMP 全フィールドは
/// この関数内で `ingest_text::append_xmp` と同じフィールド集合を連結する。
fn hay_of(meta_text: &str, name: &str, xmp: Option<&crate::xmp_reader::XmpTweetInfo>) -> String {
    let mut out = if meta_text.is_empty() {
        name.to_string()
    } else {
        format!("{meta_text}\n{name}")
    };
    if let Some(x) = xmp {
        // ingest_text::append_xmp と同じ 9 フィールドを連結 (Codex round-9 Should-fix #1)
        for field in [
            x.tweet_id.as_deref(),
            x.author_screen_name.as_deref(),
            x.author_display_name.as_deref(),
            x.posted_at.as_deref(),
            x.description.as_deref(),
            x.creator.as_deref(),
            x.quoted_by_screen_name.as_deref(),
            x.quoted_by_tweet_id.as_deref(),
            x.source.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            out.push('\n');
            out.push_str(field);
        }
    }
    out
}

/// EXIF 全タグ値を 1 つの文字列に連結 (空白区切り)。
///
/// Tantivy 側 `exif_text` STORED (ingest_text::append_exif 経由で生成) と同じ形に揃え、
/// Ctrl+F の on-demand fallback 経路でも EXIF を検索対象に含める。
fn exif_hay(info: &crate::exif_reader::ExifInfo) -> String {
    let mut out = String::new();
    for (_group, tags) in &info.sections {
        for (_name, value) in tags {
            if !value.is_empty() {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(value);
            }
        }
    }
    out
}
use crate::fs_animation::{FsCacheEntry, FsLoadResult, decode_apng_frames, decode_gif_frames};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::thumb_loader::{
    CacheDecision, LoadRequest, ThumbMsg, build_and_save_one, compute_display_px, encode_and_save,
    process_load_request,
};
use crate::ui_helpers::{
    draw_folder_badge, draw_pdf_badge, draw_play_icon, draw_zip_badge, natural_sort_key,
    truncate_name,
};

/// 消しゴムモードのツール種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EraseTool {
    /// 選択ツール: クリックでベクタオブジェクトを選択 (描画は行わない)
    Select,
    /// 囲みツール: ドラッグで多角形を描き内側を塗りつぶす
    Lasso,
    /// 縦線ツール: ドラッグ幅の縦全体矩形を塗りつぶす
    VertLine,
    /// 横線ツール: ドラッグ高さの横全体矩形を塗りつぶす
    HorizLine,
    /// 直線ツール: ドラッグ始終点を結ぶ太い直線を塗りつぶす。Shift で幅調整。
    Line,
    /// 筆ツール: 円形ブラシで自由に塗る
    Brush,
}

impl Default for EraseTool {
    fn default() -> Self {
        EraseTool::Brush
    }
}

/// Ctrl+ドラッグ中の基準状態。ドラッグ開始時に記録し、以降のマウス位置との
/// 差分から操作量を算出する。
#[derive(Debug, Clone, Copy)]
pub(crate) enum ShiftDragState {
    /// 筆ツール: 起点 + 基準半径。
    BrushSize {
        origin: (f32, f32),
        base_radius: f32,
    },
    /// 縦線/横線ツール: 起点 + 基準傾き + 基準の線端点。
    LineAdjust {
        origin: (f32, f32),
        base_tilt: f32,
        base_start: (f32, f32),
        base_end: (f32, f32),
    },
}

/// 消しゴムの Undo スタックに積むスナップショット。ビットマップとベクタの両方を持つ。
#[derive(Debug, Clone)]
pub(crate) struct EraseSnapshot {
    pub mask: Vec<bool>,
    pub vectors: Vec<crate::mask_db::LineObject>,
}

/// 見開きから消しゴムに入ったときのコンテキスト。Apply / Cancel で
/// 元の見開き状態 (= `saved_mode` の spread_mode + `pair.0` を中心ページとした
/// 見開き表示) を復元するために保存する。
#[derive(Clone, Copy, Debug)]
pub(crate) struct EraseSpreadCtx {
    pub saved_mode: crate::settings::SpreadMode,
    /// (left_idx, right_idx)。LTR / RTL のいずれでも「画面上の左/右」の意味で固定。
    pub pair: (usize, usize),
}

/// 見開き表示中に補正パネルが編集対象とするページ (画面上の左/右で固定、
/// LTR/RTL は問わない)。Single (単ページ / 表紙単独 / 横長単独) では参照されず、
/// L/R セレクタも UI 上に出ない。`open_fullscreen` (= ページ送り、ペア切替、
/// 初回フルスクリーン) と spread_mode 切替で `Left` にリセットされる。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum AdjustSpreadTarget {
    #[default]
    Left,
    Right,
}

/// 仮想フォルダ (PDF/ZIP) 進入時の親 catalog への write-back ターゲット。
/// `start_loading_items` で仮想フォルダ用 catalog を開くタイミングで構築し、
/// `poll_thumbnails` の finalize シグナル経路で発火する。
pub(crate) struct VirtualFolderWriteback {
    pub parent_catalog: Arc<crate::catalog::CatalogDb>,
    /// 親 catalog 側の保存キー (例: `pdfthumb:foo.pdf` / `zipthumb:foo.zip`)。
    pub parent_key: String,
    /// 仮想 catalog 側の対応キー (= cache_map の lookup に使う、例: `page_0000`)。
    pub target_key: String,
    /// 監視対象の items 内 idx (= 最初の PdfPage / ZipImage の idx)。
    pub target_idx: usize,
    /// このターゲットが有効な items 世代。`fire_*` で `App::items_generation` と
    /// 一致しなければ無視 + クリア (Codex P2 対策)。
    ///
    /// `start_loading_items` でも仮想 writeback はクリアされるが、
    /// `replace_search_view_items` (Ctrl+G 検索ビュー切替、`global_search_ui.rs`) や
    /// `remove_items_batch` (削除直後の idx 詰め、`app.rs`) は `items_generation` を
    /// 直接 +1 するだけで `start_loading_items` を通らない。この場で書き戻しが発火
    /// すると古い PDF/ZIP の親 catalog に書いてしまうので、このフィールドが
    /// 実質的な唯一のガードになる。
    pub items_gen: u64,
    /// 現フォルダの cache_map 参照。worker がここに WebP データを put 後、
    /// 本構造体経由で親 catalog にミラーする。
    pub cache_map: Arc<
        std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
    >,
    /// 親 catalog の `pdfthumb:` / `zipthumb:` 行に書き込む mtime / size。
    /// PDF/ZIP **ファイル本体**の mtime / size を入れる (親 grid 表示時の値と一致)。
    pub parent_entry_mtime: i64,
    pub parent_entry_size: i64,
}

/// 画像補正パネルのスライダードラッグ中に保持するスナップショット。
///
/// ドラッグ中は毎フレーム DB に書かず in-memory のみ更新し、ドラッグ終了時に
/// `before` (= ドラッグ開始時の page_params エントリ) と現在の値を比較して
/// Undo エントリを 1 件積む + 通常の `set_page_params` で DB 永続化する。
/// これにより 60 frames/sec の DB UPSERT を 1 回に削減する。
#[derive(Debug, Clone)]
pub(crate) struct AdjustmentDragSession {
    pub fs_idx: usize,
    pub before: Option<crate::adjustment::AdjustParams>,
}

/// タグ操作の保留 Undo entry を集計するためのバッファ。
///
/// 1 トランザクション (1 回の `request_tag_toggle_for_selection` / `_clear_for_selection`
/// 呼出し) ごとに 1 個作られ、worker 結果が **実 disk の before/after** を伴って
/// 帰ってくるたびに `accumulated` に積まれる。`expected_total` 件全部が完了した時点で
/// `accumulated` から `UndoEntry::Tag` を組み立てて `meta_undo` に push する。
///
/// この遅延構築によって:
/// - 外部ツールが XMP を書き換えて `tags_cache` が stale でも、Undo entry は worker が
///   読んだ真の disk 状態を `before` に持つ → Ctrl+Z 連打で他タグを破壊しない。
/// - 個別ファイルの XMP 書き込み失敗は `accumulated` に入らない (= Undo の対象外) ので、
///   実ディスクと Undo stack が乖離しない。
#[derive(Debug, Clone)]
pub(crate) struct PendingTagUndo {
    pub summary: String,
    /// 投入したジョブ数。`accumulated.len() + failures` がこの値に達したら確定する。
    pub expected_total: usize,
    /// 失敗したジョブ数 (UI トーストの集計とは別軸: ここでは Undo entry に含めない件数を数える)。
    pub failures: usize,
    /// worker 結果 1 件分。`(path, tags_before, tags_after)`。
    pub accumulated: Vec<crate::undo_stack::TagChange>,
}

/// ベクタオブジェクト編集のドラッグ状態。
/// `base` はドラッグ開始時の元オブジェクト、`origin` はそのときのカーソル画像座標。
#[derive(Debug, Clone, Copy)]
pub(crate) enum EraseVectorDrag {
    /// オブジェクト全体を平行移動。
    Pan {
        index: usize,
        base: crate::mask_db::LineObject,
        origin: (f32, f32),
    },
    /// Ctrl+ドラッグ: 垂直成分を回転、水平成分を太さ変更に割り当てる。
    ModAdjust {
        index: usize,
        base: crate::mask_db::LineObject,
        origin: (f32, f32),
    },
    /// 直線の始点/終点ドラッグ。`which_p1=true` で p1、false で p0。
    Endpoint {
        index: usize,
        base: crate::mask_db::LineObject,
        which_p1: bool,
    },
}

// -----------------------------------------------------------------------
// サブ構造体: サムネイル画質 A/B 比較ダイアログの状態
// -----------------------------------------------------------------------

/// サムネイル画質 A/B ダイアログの初期サンプル decode を worker 化するための pending。
/// ダイアログは即座に「読み込み中」表示で開き、decode 完了後にサンプル + A/B プレビューを
/// 構築する (docs/ui-responsiveness.md §4)。
pub(crate) struct ThumbQualityLoadPending {
    pub path: PathBuf,
    pub rx: mpsc::Receiver<Option<(image::DynamicImage, u64)>>,
}

/// A/B プレビューの WebP 再エンコード + ColorImage 変換を worker 化するための pending。
/// スライダー操作で連発された場合、前回 pending は `cancel` を立てて無駄走査を止め、
/// 最新だけ texture に反映する。
pub(crate) struct TqEncodePending {
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<Option<TqEncodeResult>>,
}

pub(crate) struct TqEncodeResult {
    pub bytes: usize,
    pub color_image: egui::ColorImage,
}

#[derive(Default)]
pub(crate) struct ThumbQualityState {
    pub show: bool,
    /// サンプル画像 (デコード済み、ダイアログを閉じるまで保持)。`Arc` で worker 共有し、
    /// clone コストを O(1) にする。20MP 級の DynamicImage を worker 都度コピーするのは高価。
    pub sample: Option<Arc<image::DynamicImage>>,
    /// サンプル画像のパス表示用
    pub sample_path: Option<PathBuf>,
    /// サンプル画像の元ファイルサイズ (bytes)
    pub sample_original_size: u64,
    /// サンプル decode 進行中の pending (ダイアログ開く時に spawn)
    pub load_pending: Option<ThumbQualityLoadPending>,
    /// パネル A: サイズ (long side px)
    pub a_size: u32,
    /// パネル A: 品質 (1–100)
    pub a_quality: u8,
    /// パネル A: プレビューテクスチャ
    pub a_texture: Option<egui::TextureHandle>,
    /// パネル A: エンコード後のバイト数
    pub a_bytes: usize,
    /// パネル A: encode worker pending (スライダー変更中に次々立ち上げ、古いのは cancel)
    pub a_encode_pending: Option<TqEncodePending>,
    /// パネル B: サイズ
    pub b_size: u32,
    /// パネル B: 品質
    pub b_quality: u8,
    /// パネル B: プレビューテクスチャ
    pub b_texture: Option<egui::TextureHandle>,
    /// パネル B: エンコード後のバイト数
    pub b_bytes: usize,
    /// パネル B: encode worker pending
    pub b_encode_pending: Option<TqEncodePending>,
    /// true = A/B 比較の全画面オーバーレイ表示中
    pub fullscreen: bool,
    /// 全画面 A/B 比較時の縦線位置（0.0=すべて B、1.0=すべて A、中央は 0.5）
    pub fs_divider: f32,
}

// -----------------------------------------------------------------------
// サブ構造体: キャッシュ作成バックグラウンドタスクの状態
// -----------------------------------------------------------------------

pub(crate) struct CacheCreatorState {
    pub show: bool,
    /// 各お気に入りのチェック状態（settings.favorites と同じ長さ）
    pub checked: Vec<bool>,
    /// 実行中フラグ（UI ボタンの有効/無効とポーリング制御）
    pub running: bool,
    /// カウントフェーズ中フラグ（total 未確定）
    pub counting: Arc<AtomicBool>,
    /// 対象フォルダ総数（Pass 1 完了後に確定）
    pub total: Arc<AtomicUsize>,
    /// 処理済みフォルダ数
    pub done: Arc<AtomicUsize>,
    /// キャッシュ容量 (バイト単位、累積加算)
    pub cache_size: Arc<AtomicU64>,
    /// キャンセルトークン
    pub cancel: Arc<AtomicBool>,
    /// 現在処理中のフォルダパス表示用
    pub current: Arc<Mutex<String>>,
    /// 完了シグナル（表示切替用）
    pub finished: Arc<AtomicBool>,
    /// 完了後のメッセージ
    pub result: Option<String>,
}

impl Default for CacheCreatorState {
    fn default() -> Self {
        Self {
            show: false,
            checked: Vec::new(),
            running: false,
            counting: Arc::new(AtomicBool::new(false)),
            total: Arc::new(AtomicUsize::new(0)),
            done: Arc::new(AtomicUsize::new(0)),
            cache_size: Arc::new(AtomicU64::new(0)),
            cancel: Arc::new(AtomicBool::new(false)),
            current: Arc::new(Mutex::new(String::new())),
            finished: Arc::new(AtomicBool::new(false)),
            result: None,
        }
    }
}

// -----------------------------------------------------------------------
// サブ構造体: お気に入り検索 (検索インデックス DB 利用) の状態
// -----------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct FavSearchState {
    /// 検索バー表示中
    pub active: bool,
    /// 現在の検索クエリ
    pub query: String,
    /// 最後に検索した文字列 (query が変化したかの判定用)
    pub last_executed: String,
    /// TextEdit にフォーカスを要求するフラグ (1 フレームだけ true)
    pub focus_request: bool,
    /// TextEdit がフォーカスを持っているか
    pub has_focus: bool,
    /// 検索モード開始時の current_folder (×/Esc で戻る先)
    pub saved_folder: Option<PathBuf>,
    /// 検索結果から入ったフォルダのスタック (先頭 = 結果から最初に入ったパス、末尾 = 現在フォルダ)。
    /// BS を押すとここからポップする。空のときは検索結果リストを表示中。
    pub nav_stack: Vec<PathBuf>,
    /// 最後の検索結果に含まれていた対象パス (名前ソート順)。
    /// Ctrl+↑↓ で前後の検索結果アイテムへ移動するときに参照する。結果件数表示にも使う。
    pub results_paths: Vec<PathBuf>,
    /// お気に入り絞り込み (None = すべて、Some(id) = 単一 favorite に限定) — §19.7 準拠。
    pub favorite_filter: Option<uuid::Uuid>,
    /// OR 検索モード (docs §20)。`true` なら include トークンを OR 結合 (NOT は常に AND)。
    pub or_mode: bool,
}

impl FavSearchState {
    /// 検索結果リストを表示している状態か (検索バーを開いていて、まだどこにも潜っていない)。
    pub fn on_results_grid(&self) -> bool {
        self.active && self.nav_stack.is_empty()
    }
}

/// 検索結果モードで current_folder に設定する合成パス。
/// `%APPDATA%/mimageviewer/__search_results__` (実在させない、カタログキーとしてのみ使用)。
pub(crate) fn search_results_synthetic_path() -> PathBuf {
    crate::data_dir::get().join("__search_results__")
}

/// サムネイル色調補正の対象アイテムかどうか。
///
/// 補正は「ページ単位の色調を持つ画像系」だけに掛ける ([docs/display-pipeline.md §1.5](docs/display-pipeline.md))。
/// フォルダ・ZipFile・PdfFile・ConvertibleArchive・Video・ZipSeparator の代表
/// サムネは対象外で、`global_preset` を意図せず適用するとフォルダ表紙が変色する等の
/// バグになる。「ページ単位データを持てるか」と概念的に一致するので
/// [`GridItem::has_page_data`] を流用する。
pub(crate) fn is_thumb_adjust_target(item: Option<&GridItem>) -> bool {
    item.is_some_and(|it| it.has_page_data())
}

// -----------------------------------------------------------------------
// App
// -----------------------------------------------------------------------

pub struct App {
    pub(crate) address: String,
    pub(crate) current_folder: Option<PathBuf>,
    pub(crate) items: Vec<GridItem>,
    pub(crate) thumbnails: Vec<ThumbnailState>,
    pub(crate) selected: Option<usize>,
    pub(crate) settings: crate::settings::Settings,
    pub(crate) tx: mpsc::Sender<ThumbMsg>,
    pub(crate) rx: mpsc::Receiver<ThumbMsg>,
    /// フォルダ移動時に true にセットすると旧ロードタスクが中断する
    pub(crate) cancel_token: Arc<AtomicBool>,
    /// Phase 2b ワーカーが参照する現在の可視先頭アイテムインデックス
    /// UIスレッドが毎フレーム更新し、バックグラウンドワーカーが優先度に使う
    pub(crate) scroll_hint: Arc<AtomicUsize>,
    /// 可視範囲の終端 (exclusive) アイテムインデックス。`scroll_hint` と併せて
    /// ワーカーの前後対称な距離ベース優先度計算に使う。
    pub(crate) visible_end_shared: Arc<AtomicUsize>,

    /// スクロールオフセット（行境界にスナップ済み）。自前管理する
    pub(crate) scroll_offset_y: f32,
    /// 前フレームのセル幅（ = avail_w / cols）
    pub(crate) last_cell_size: f32,
    /// 前フレームのセル高さ（ = last_cell_size * thumb_aspect.height_ratio()）
    pub(crate) last_cell_h: f32,
    /// 前フレームのビューポート高さ（カーソルキースクロールに使用）
    pub(crate) last_viewport_h: f32,
    /// true のとき選択セルが見えるようにオフセットを調整する
    pub(crate) scroll_to_selected: bool,

    /// ウィンドウ状態保存用：最後に確認した outer_rect（最小化・最大化時は更新しない）
    pub(crate) last_outer_rect: Option<egui::Rect>,
    /// ウィンドウ状態保存用：最後に確認した inner_size（クライアント領域）。
    /// `ViewportCommand::InnerSize` / `ViewportBuilder::with_inner_size` の入力と
    /// 直接整合する値。起動時の「outer を保存して inner として適用」によるタイトルバー
    /// 分のサイズ縮小を防ぐために、これを settings.window_size に書き戻す。
    pub(crate) last_inner_size: Option<[f32; 2]>,
    /// 現在のウィンドウの DPI スケール（論理→物理変換に使用）
    pub(crate) last_pixels_per_point: f32,
    /// 初回フレームで適用する inner_size（egui#4918 / winit#923 対策）。
    /// ViewportBuilder 段階では マルチモニタ DPI 混在時に サイズを誤って設定する
    /// ケースがあるため、DPI 確定後の初回フレームで再適用する。
    pub(crate) pending_initial_size: Option<[f32; 2]>,

    /// キャッシュ生成進捗：新規デコードが必要だった画像の総数
    pub(crate) cache_gen_total: usize,
    /// キャッシュ生成進捗：完了した枚数（rayon スレッドからアトミックに更新）
    pub(crate) cache_gen_done: Arc<AtomicUsize>,

    // ── 段階 B: ページ単位先読み / eviction ──────────────────────
    /// アイテム idx → 画像メタデータ (mtime, file_size)。フォルダ・動画は None
    pub(crate) image_metas: Vec<Option<(i64, i64)>>,
    /// 永続ワーカーがサムネイルを処理するためのキュー（UI からは push のみ）
    /// 通常画像 (Image, ZipImage, PdfPage) 用
    pub(crate) reload_queue: Option<Arc<NotifyQueue>>,
    /// 重い I/O (ZipFile, PdfFile, Folder) 用の専用キュー。
    /// 専用 I/O ワーカー (2本) が priority 順に取り出す。
    pub(crate) heavy_io_queue: Option<Arc<NotifyQueue>>,
    /// ロード要求を送ったがまだ応答が来ていない idx 集合（重複要求防止）。
    /// 値は `true` ならアイドル時アップグレード要求、`false` なら通常の読み込み要求。
    pub(crate) requested: std::collections::HashMap<usize, bool>,
    /// 現在の keep range を `[min, max+1)` でくくった bounding box。
    /// worker 側の atomic キャンセル判定 (`keep_start_shared`/`keep_end_shared`) 用。
    /// enqueue / eviction / retain / idle upgrade は `keep_set` の方を使うこと
    /// (docs/async-architecture.md「display list vs filesystem list」参照)。
    pub(crate) keep_range: (usize, usize),
    /// prefetch / eviction / retain 等が実際に対象にする idx 集合。
    /// `visible_indices[vis_keep_start..vis_keep_end]` から毎フレーム構築される。
    /// ★フィルタや Ctrl+F で疎になった `visible_indices` でも、非可視 idx が
    /// 先読みキューに流入しないよう、raw range ではなく set 判定に統一する。
    pub(crate) keep_set: std::collections::HashSet<usize>,
    /// ワーカー共有用: keep_range の start/end をアトミックに公開。
    /// ワーカーは pick 後にこの範囲を確認し、範囲外のリクエストをスキップする。
    /// (set そのものを共有するのは実装負荷が大きいため bounding box で近似する。
    /// set から外れた idx が worker に渡るケースは enqueue で既に弾いているので
    /// 実害はごくわずか。)
    pub(crate) keep_start_shared: Arc<AtomicUsize>,
    pub(crate) keep_end_shared: Arc<AtomicUsize>,
    /// poll_thumbnails で 1 フレームのテクスチャ生成上限を超えた分を次フレームに持ち越す
    pub(crate) texture_backlog: Vec<crate::thumb_loader::ThumbMsg>,

    /// Ctrl+↑↓ のバックグラウンドフォルダナビゲーション結果待ち。
    /// navigate_folder_with_skip をワーカースレッドで実行し、UIスレッドをブロックしない。
    folder_nav_pending: Option<FolderNavPending>,
    /// Ctrl+↑↓ 連打アキュームレータ。in-flight 中の追加プレスをここに貯め、
    /// 現 nav が完了するたびに 1 消費して次の nav を連鎖させる。
    /// 符号: 正=forward (↓), 負=backward (↑)。異方向を押すと相殺される。
    pending_folder_nav_steps: i32,
    /// 連鎖ステップ時に使うモード (grid / fullscreen / favsearch)。
    /// folder_nav_pending が走っている間は pending.mode と一致する。
    /// 一連のバーストが途切れたら `FolderNavMode::Grid` にリセットする。
    pending_folder_nav_mode: FolderNavMode,

    // ── 進捗バー (段階 B/E の合算進捗表示) ─────────────────────
    /// 現フレームで検出された通常読み込みのピーク件数 (current が 0 でリセット)
    pub(crate) progress_normal_peak: usize,
    /// 現フレームで検出された高画質化 (アイドルアップグレード) のピーク件数
    pub(crate) progress_upgrade_peak: usize,

    /// 前フレームで選択中セルが描画された矩形 (スクリーン座標)。
    /// 選択情報オーバーレイをセル直下に配置するために使用。
    /// 選択セルがスクロール圏外だと None。
    pub(crate) selected_cell_rect: Option<egui::Rect>,

    // ── 段階 E: アイドル時の画質向上 ─────────────────────────────
    /// 前フレームでの scroll_offset_y（変化検知用）
    pub(crate) last_scroll_offset_y_tracked: f32,
    /// 最後にスクロールが動いた瞬間の時刻（アイドル検出用）
    pub(crate) last_scroll_change_time: std::time::Instant,
    /// UI とワーカー間で共有する現在の display_px (列数変更時に追従させる)
    /// update_keep_range_and_requests で毎フレーム更新される。
    pub(crate) display_px_shared: Arc<AtomicU32>,

    // ── 統計情報 (起動時から累計) ─────────────────────────────
    /// サムネイル読み込みの統計 (時間分布・サイズ分布・フォーマット)。
    /// ワーカースレッドから Arc 経由で更新され、UI スレッドが読み出す。
    pub(crate) stats: Arc<Mutex<crate::stats::ThumbStats>>,
    /// 統計ダイアログの表示フラグ
    pub(crate) show_stats_dialog: bool,

    // ── フルスクリーン表示・先読みキャッシュ ───────────────────────
    /// Some(idx) = フルスクリーン表示中（self.items のインデックス）
    pub(crate) fullscreen_idx: Option<usize>,
    /// 先読みキャッシュ: item_idx → ロード済みエントリ（静止画 or アニメーション）
    pub(crate) fs_cache: std::collections::HashMap<usize, FsCacheEntry>,
    /// 先読み中: item_idx → (キャンセルトークン, 受信チャネル, load 開始時の input_seq)
    /// `input_seq` は perf の `fs.ready` / `fs.paint` を `fs.load_begin` と同じ
    /// 操作に紐づけるための相関キー。`self.input_seq` を使うと非同期完了時に
    /// 別のユーザー操作にずれる。計装無効時や内部起動は 0。
    pub(crate) fs_pending:
        std::collections::HashMap<usize, (Arc<AtomicBool>, mpsc::Receiver<FsLoadResult>, u64)>,

    /// fs_load ワーカーがヘッダ解析だけで取得した先行寸法 (fullscreen 用)。
    /// `FsLoadResult::DimsOnly` を受信すると登録され、本体 (`Static` など) で
    /// fs_cache が埋まったら削除される。ホバーバーはデコード完了前でも
    /// これを見て即サイズを表示できる (⚠ ダウンスケール警告もここで判定可能)。
    pub(crate) fs_early_dims: std::collections::HashMap<usize, [usize; 2]>,

    /// デコード完了済みだが GPU アップロード未了の先読みエントリ。
    /// 20MP 級 JPEG の `ctx.load_texture` は UI スレッドで 25-60ms/枚かかり、
    /// 10 枚連続で来ると 500ms 超の UI フリーズになる (計測実績あり)。
    /// `poll_prefetch` では受信を drain してここに溜め、フレームあたり最大 1 枚だけ
    /// アップロードする (現在ページは即時)。
    /// `(idx, FsLoadResult, load_seq)` — FIFO 順で消化する。
    pub(crate) fs_upload_backlog: Vec<(usize, FsLoadResult, u64)>,

    /// items が差し替わるたびにインクリメントする世代カウンタ。
    /// `LoadRequest::items_gen` → worker → `ThumbMsg::items_gen` を経由して poll_thumbnails
    /// まで透過され、世代が一致しないメッセージは破棄される。
    /// 旧フォルダ用ワーカーが新 items の同じ idx に違う画像を書き込む race を防ぐ。
    pub(crate) items_generation: u64,

    /// 現在の `items` が Ctrl+G の合成ビュー (Aggregated / DrilledInto の
    /// `build_*_items` で組み立てたもの) かを示すフラグ。
    /// `replace_search_view_items` で true、`install_new_items` で false に倒す。
    /// rating 変更で `rebuild_items_from_global_search` を呼んでいいのは
    /// このフラグが true のときだけ (Codex P2): Ctrl+G から実フォルダ/ZIP/PDF を
    /// 開いた後に rating 変更して合成ビューが書き戻される事故を防ぐ。
    pub(crate) items_are_global_search_view: bool,

    // ── お気に入り編集ポップアップ ────────────────────────────────
    pub(crate) show_favorites_editor: bool,
    /// インデックスサイズ表示用のキャッシュ。ダイアログ open 時に計算し、
    /// close で None に戻す。毎フレーム `std::fs::metadata` + `read_dir` を
    /// 叩かないようにするため。
    pub(crate) favorites_index_size_cache:
        Option<crate::ui_dialogs::favorites_editor::IndexDiskSizes>,
    /// お気に入りごとの索引件数 (名前 / メタ) のキャッシュ。
    /// `Instant` は最終取得時刻で、ダイアログ表示中はバックグラウンド更新を反映するため
    /// 一定間隔 (5s) で worker 経由で再計算する。close で None。
    pub(crate) favorites_index_count_cache:
        Option<(std::time::Instant, crate::ui_dialogs::favorites_editor::IndexFileCounts)>,
    /// 件数 / サイズの再計算 worker (in-flight)。
    /// `Some` の間は重複起動しない。`try_recv()` で結果を回収して None に戻す。
    pub(crate) favorites_index_refresh_rx: Option<
        std::sync::mpsc::Receiver<(
            crate::ui_dialogs::favorites_editor::IndexFileCounts,
            crate::ui_dialogs::favorites_editor::IndexDiskSizes,
        )>,
    >,
    /// バックグラウンドインデクサの全体 ETA 表示用文字列キャッシュ。
    /// レート計算は 100ms 単位で振動するので、表示は 1 秒間隔で更新する。
    /// 内側の `Option<String>` が `None` のときは「ETA 算出不可 (= サンプル不足
    /// または進行中なし)」。close 時に None に戻す。
    pub(crate) favorites_total_eta_cache: Option<(std::time::Instant, Option<String>)>,

    // ── タグ編集ダイアログ (docs/tag-feature.md) ─────────────────
    pub(crate) show_tag_editor: bool,
    /// タグ編集ダイアログ中で編集中のタグ一覧 (キャンセルで破棄するため Settings から分離)
    pub(crate) tag_editor_draft: Vec<crate::settings::TagDef>,
    /// タグ書き込み worker (初回要求時に遅延初期化)。
    pub(crate) tag_write_handle: Option<crate::tag_write_worker::TagWriteHandle>,
    /// レーティング XMP 書き込み worker。設定 ON のとき遅延初期化される。
    /// 書き込み失敗を poll して失敗トーストを出すために保持。
    pub(crate) rating_write_handle: Option<crate::rating_write_worker::RatingWriteHandle>,

    // ── お気に入り追加ダイアログ (名称入力 + 自動インデックス選択) ─────
    pub(crate) show_fav_add_dialog: bool,
    pub(crate) fav_add_name_input: String,
    pub(crate) fav_add_target: Option<PathBuf>,
    // v0.8.0: 追加時にチェックした自動インデックス対象
    // 既存お気に入りは全て false なので、ここもデフォルト false で一貫させる。
    // ユーザは後で「お気に入りの編集」からも切り替えられる。
    pub(crate) fav_add_auto_index_structure: bool,
    pub(crate) fav_add_auto_index_metadata: bool,
    pub(crate) fav_add_auto_index_thumbs: bool,

    // ── 全文検索インデクサ (Ctrl+G グローバルメタ検索用) ────────────
    // auto_index_metadata=true のお気に入り毎に Supervisor を持ち、Tantivy index
    // + fts_meta 管理メタを Tantivy First 順序 (commit + reload 成功後に SQLite
    // を同期更新) で運用する。
    // 起動時 DB オープンに失敗した場合は None (機能なしで動作継続)。
    pub(crate) indexer_manager: Option<crate::indexer_manager::IndexerManager>,

    /// 名前索引 Supervisor のアクティブ handle (favorite_id → handle)。
    ///
    /// `auto_index_structure = true` のお気に入りごとに 1 つ。長期スレッド +
    /// FsWatcher を持ち、初期バルクが終わった後も notify-rs イベントで差分追従する。
    ///
    /// 2026-04 ユーザー指摘: 旧 `name_bulk_handles` はワンショット bulk thread の
    /// JoinHandle だけを保持していたため、初期スキャン後に追加された
    /// フォルダ/ZIP/PDF は Ctrl+S 検索にヒットしなかった。メタ索引側の
    /// `indexer_supervisor` と対称な構造に揃えるため `NameIndexSupervisor` に差し替え。
    pub(crate) name_index_supervisors: std::collections::HashMap<
        uuid::Uuid,
        crate::name_index_supervisor::NameIndexSupervisorHandle,
    >,

    /// 操作中はバックグラウンドインデクサを一時停止するためのゲート (2026-04 F)。
    /// `App::update` の入力検知で `bump()` され、indexer 側が `wait_until_idle()` で待機。
    /// `Arc` で `IndexerManager` / `name_index_supervisor` と共有される。
    pub(crate) activity_gate: Arc<crate::activity_gate::ActivityGate>,

    // ── Ctrl+G グローバルメタ検索 UI 状態 (docs §10.3) ──────────────
    pub(crate) global_search: crate::global_search_ui::GlobalSearchState,

    // ── フォルダを開く ダイアログ (アドレスバーを隠したとき用) ───
    pub(crate) show_open_folder_dialog: bool,
    pub(crate) open_folder_input: String,
    /// フォルダを開くダイアログのエラーメッセージ
    pub(crate) open_folder_error: Option<String>,

    // ── 統合環境設定ダイアログ ─────────────────────────────────────
    pub(crate) show_preferences: bool,
    /// 統合環境設定の一時編集状態
    pub(crate) pref_state: Option<crate::ui_dialogs::preferences::PreferencesState>,

    // ── 複数選択 ──────────────────────────────────────────────────
    /// チェック済みアイテムの集合 (スペースキーで追加/削除)
    pub(crate) checked: std::collections::HashSet<usize>,

    // ── 右クリックコンテキストメニュー ─────────────────────────
    /// コンテキストメニューの対象アイテムインデックス
    pub(crate) context_menu_idx: Option<usize>,
    /// コンテキストメニューの表示座標 (右クリック時に記録)
    pub(crate) context_menu_pos: egui::Pos2,

    // ── フルスクリーン右クリックコンテキストメニュー ─────────
    /// 右クリック長押し検出用: 押下開始時刻と座標
    pub(crate) fs_secondary_press_start: Option<(std::time::Instant, egui::Pos2)>,
    /// フルスクリーン用コンテキストメニューの対象アイテムインデックス
    pub(crate) fs_context_menu_idx: Option<usize>,
    /// フルスクリーン用コンテキストメニューの表示座標
    pub(crate) fs_context_menu_pos: egui::Pos2,

    // ── フルスクリーン 中ボタンドラッグズーム (v0.8.1) ─────────
    /// 中ボタン (ホイール押し込み) ドラッグ中の状態。None ならドラッグしていない。
    /// ドラッグ開始時に (pivot, start_zoom, start_pan, rect_center, is_analysis) を
    /// スナップショットし、押している間は start 値からの差分でズームを計算する
    /// (累積誤差防止 + pivot 位置が安定)。マウスを右手だけで拡大縮小する用途。
    pub(crate) fs_middle_zoom_drag: Option<crate::ui_fullscreen::MiddleZoomDrag>,

    // ── 削除確認ダイアログ ───────────────────────────────────────
    pub(crate) show_delete_confirm: bool,
    /// 削除対象のファイルパスリスト
    pub(crate) delete_targets: Vec<(usize, PathBuf)>,

    // ── ペースト後のフォルダ再読み込みフラグ ──────────────────────
    pub(crate) pending_reload: bool,
    /// 実行中のペースト worker 完了待ち。完了するごとに `pending_reload` を立てる。
    /// PowerShell 経由の paste が完了する前に reload しても無駄走査になるため、
    /// 完了通知を待ってから再読込する (docs/ui-responsiveness.md §4)。
    pub(crate) paste_pending: Vec<std::sync::mpsc::Receiver<()>>,
    /// フォルダ読み込み後に選択するアイテム名（BS で親に戻るとき等）
    pub(crate) select_after_load: Option<String>,

    // ── 同名ファイル処理 ──────────────────────────────────────────
    pub(crate) video_thumb_overrides: std::collections::HashMap<String, PathBuf>,

    // ── 回転リセット確認ダイアログ ─────────────────────────────
    pub(crate) show_rotation_reset_confirm: bool,

    // ── キャッシュ管理ポップアップ ───────────────────────────────
    pub(crate) show_cache_manager: bool,
    /// キャッシュ管理の「◯日以上古い」入力値
    pub(crate) cache_manager_days: u32,
    /// 開いたときに取得するキャッシュ統計: (フォルダ数, 合計バイト)
    pub(crate) cache_manager_stats: Option<(usize, u64)>,
    /// 削除後の結果メッセージ
    pub(crate) cache_manager_result: Option<String>,
    /// 「すべてのキャッシュを削除」の確認ステップ
    pub(crate) cache_manager_confirm_delete_all: bool,
    /// 集計 / 削除のバックグラウンド実行ハンドル。保持中はダイアログのボタンを無効化する。
    pub(crate) cache_maint_pending: Option<crate::cache_maintenance::CacheMaintPending>,
    /// 変換済みアーカイブキャッシュ管理ダイアログのロード / 削除ワーカーのハンドル。
    pub(crate) archive_cache_maint_pending:
        Option<crate::cache_maintenance::ArchiveMaintPending>,

    // ── 変換済みアーカイブキャッシュ (v0.7.0) ───────────────────
    /// 7z / LZH → ZIP 変換キャッシュ DB。初期化失敗時は None。
    pub(crate) archive_cache_db: Option<Arc<crate::archive_cache::ArchiveCacheDb>>,
    /// 進行中の変換ダイアログ状態。None ならダイアログ非表示。
    pub(crate) archive_convert: Option<crate::ui_dialogs::archive_convert::ArchiveConvertState>,
    /// 変換済みアーカイブを開いているとき、元 (7z/LZH) のパスを保持する。
    /// `current_folder` はキャッシュ ZIP を指しているので、UI 表示 / BS /
    /// Ctrl+↑↓ / タイトルバーでは本フィールドを優先する。
    /// 通常フォルダ / 通常 ZIP / PDF を開いたら load_folder が None にリセットする。
    pub(crate) archive_source_override: Option<PathBuf>,

    // ── 変換済みアーカイブキャッシュ管理ダイアログ (v0.7.0) ─────
    /// 「変換済みアーカイブキャッシュ管理」ウィンドウの表示フラグ
    pub(crate) show_archive_cache_manager: bool,
    /// 開いたとき / 削除操作後にリフレッシュされる一覧キャッシュ。
    pub(crate) archive_cache_rows: Option<Vec<crate::archive_cache::ArchiveCacheEntry>>,
    /// LoadRows ワーカーがまとめて返す合計バイト数 (UI で再集計しない)。
    pub(crate) archive_cache_total_bytes: u64,
    /// チェックボックス選択状態。行 index が key。
    pub(crate) archive_cache_selection: std::collections::HashSet<usize>,
    /// 削除操作後のメッセージ
    pub(crate) archive_cache_manager_result: Option<String>,
    /// 「すべて削除」確認ステップ
    pub(crate) archive_cache_confirm_delete_all: bool,

    // ── 最後に選択した画像 (サムネイル画質ダイアログで使用) ──
    pub(crate) last_selected_image_path: Option<PathBuf>,

    // ── サムネイル画質設定ダイアログ ───────────────────────────
    pub(crate) tq: ThumbQualityState,

    // ── キャッシュ作成ポップアップ ───────────────────────────────
    pub(crate) cc: CacheCreatorState,

    // ── お気に入り検索 ────────────────────────────────────────────
    /// 検索インデックス DB (全お気に入り共通、失敗したら None)
    pub(crate) search_index_db: Option<Arc<crate::search_index_db::SearchIndexDb>>,
    /// お気に入り検索バー + 結果モードの状態
    pub(crate) favsearch: FavSearchState,

    // ── メタデータパネル (AI + EXIF) ─────────────────────────────────
    /// フルスクリーンでメタデータパネルを表示するか
    pub(crate) show_metadata_panel: bool,
    /// AI メタデータキャッシュ: 正規化キー → パース結果 (None = メタデータなし)
    /// キーは [`App::metadata_cache_key`] で生成 (ZIP エントリ・PDF ページごとに一意)。
    pub(crate) metadata_cache:
        std::collections::HashMap<String, Option<crate::png_metadata::AiMetadata>>,
    /// EXIF キャッシュ: 正規化キー → パース結果 (None = EXIF なし)
    /// キーは [`App::metadata_cache_key`] で生成 (ZIP エントリ・PDF ページごとに一意)。
    pub(crate) exif_cache: std::collections::HashMap<String, Option<crate::exif_reader::ExifInfo>>,
    /// XMP (mXD X/Twitter メタデータ) キャッシュ: 正規化キー → パース結果。
    /// mXD 以外のファイルには値が入らない前提なので None は「xtw:* なし」。
    pub(crate) xmp_cache:
        std::collections::HashMap<String, Option<crate::xmp_reader::XmpTweetInfo>>,
    /// タグキャッシュ (docs/tag-feature.md): 正規化キー → XMP dc:subject の要素列。
    /// メタデータパネルのタグボタン状態表示 + グリッドのタグバッジで使用。
    /// 充填経路は 3 つ:
    ///  1. `prewarm_grid_tags` が fts_meta から一括 (同期、フォルダ切替時)
    ///  2. `tag_prewarm` ワーカーが XMP から背景読み (非インデックスファイル)
    ///  3. `poll_tag_write_results` が worker の tags_after で上書き (書き込み完了時)
    /// フォルダ切替時のみ全クリアし、prewarm → 背景プリフェッチで埋め直す。
    pub(crate) tags_cache: std::collections::HashMap<String, Vec<String>>,
    /// tag toast 用: 直近の Toggle 操作で UI が使っていたタグ名 (`#ドール` 等)。
    /// worker 完了時に「N 件に #ドール を付与 / 削除」として表示するのに使う。
    pub(crate) tag_toast_label: Option<String>,
    /// ComfyUI Raw Prompt JSON の展開状態
    pub(crate) metadata_show_raw_prompt: bool,
    /// ComfyUI Raw Workflow JSON の展開状態
    pub(crate) metadata_show_raw_workflow: bool,
    /// EXIF セクションの展開状態
    pub(crate) exif_sections_open: std::collections::HashMap<String, bool>,

    // ── アドレスバーフォーカス管理 ───────────────────────────────
    /// true のときアドレスバーが入力中 → キーショートカットを無効化
    pub(crate) address_has_focus: bool,

    // ── フォルダ履歴（スクロール位置・選択状態の復元用）────────────
    /// フォルダパス → (scroll_offset_y, selected_idx)
    pub(crate) folder_history: std::collections::HashMap<PathBuf, (f32, Option<usize>)>,

    // ── メタデータ検索 ────────────────────────────────────────────
    /// 検索バー表示フラグ
    pub(crate) show_search_bar: bool,
    /// 検索キーワード入力
    pub(crate) search_query: String,
    /// 検索結果フィルタ: Some = フィルタ中（表示するアイテムの元インデックス集合）
    pub(crate) search_filter: Option<std::collections::HashSet<usize>>,

    /// 非同期実行中のメタデータ検索。`execute_search` で spawn、`poll_search` で受信。
    /// 大フォルダで `read_tweet_info` / `build_searchable_from_path` が UI スレッドを
    /// ブロックしていた問題 (100ms〜秒単位) を解消する。同期版のインライン処理は廃止。
    pub(crate) search_pending: Option<SearchPending>,
    /// 非同期実行中のお気に入り検索 (Ctrl+S)。`execute_favsearch` で spawn、
    /// `poll_favsearch` で受信 → `start_loading_items` を呼ぶ。
    pub(crate) favsearch_pending: Option<FavSearchPending>,
    /// フルスクリーン画像の AI/EXIF/XMP メタデータを非同期読み込み中。
    /// `open_fullscreen` で spawn、`poll_metadata_load` で受信してキャッシュに投入。
    /// 20MP JPEG の XMP 読み (`read_tweet_info` が full-file 読む) で UI が
    /// 100ms 級にブロックしていた問題を解消する。
    pub(crate) metadata_pending: Option<MetadataLoadPending>,
    /// グリッド表示用 XMP タグのバックグラウンドプリフェッチ (docs/tag-feature.md)。
    /// `fts_meta` 未登録ファイル (非インデックス favorite 等) でも grid バッジが表示される
    /// よう、`prewarm_grid_tags` が spawn する。フォルダ切替時に cancel される。
    pub(crate) tag_prewarm_pending: Option<crate::tag_prewarm::TagPrewarmPending>,
    /// `tag_prewarm_pending` で処理済み / キャッシュ済みの item idx 集合 (二重 push 防止)。
    /// idx キーで持つことで hot-path の `adjustment_db::normalize_path` 呼び出しを
    /// 未処理 idx に対してのみ発生させる。フォルダ切替で `prewarm_grid_tags` が clear する。
    pub(crate) tag_prewarm_queued: std::collections::HashSet<usize>,
    /// バックグラウンドで実行中のゴミ箱移動 (docs/async-architecture.md §5.2.1)。
    /// `start_delete_files` で spawn、`poll_delete_pending` で受信して進捗ダイアログを
    /// 更新、完了時に成功した path を items から一括 remove する。
    pub(crate) delete_pending: Option<crate::delete_worker::DeletePending>,
    /// フィルタ適用後の表示アイテムインデックスリスト（フィルタなしなら全アイテム）。
    /// グリッド表示・フルスクリーンナビ・スライドショーで共有。
    pub(crate) visible_indices: Vec<usize>,

    /// 第2シグナル (`finalized=true`) を受け取ったが、第1シグナルの ColorImage が
    /// `texture_backlog` でアップロード待ちのため `requested` から remove できなかった
    /// idx の集合。次回その idx が Loaded 化したタイミングで `requested` から remove する。
    ///
    /// これがないと: backlog 詰まり中に finalized で requested を remove → 次フレームに
    /// `need_load && !requested.contains` で再エンキュー → 同じ画像を何度も decode する
    /// 無限ループ (重複デコード地獄) になる。
    pub(crate) pending_finalize: std::collections::HashSet<usize>,

    // ── スクロール体感ロギング (--log 時) ──────────────────────────
    /// 直近のフレームで計算した可視範囲 (raw idx)。
    pub(crate) last_vis_range: (usize, usize),
    /// 可視範囲が安定 (= 1 フレーム以上同じ) になった瞬間。
    /// vis 範囲が変わるたびに `None` にリセットされる。
    pub(crate) vis_settle_at: Option<std::time::Instant>,
    /// 安定後、可視範囲内の最初のサムネイルが Loaded 化したことをログ済みか。
    pub(crate) vis_first_logged: bool,
    /// 安定後、可視範囲内の全サムネイルが Loaded 化したことをログ済みか。
    pub(crate) vis_all_logged: bool,
    /// 「サムネイル読み込み中が動かない」固着バグ診断用。
    /// 一定間隔で `requested` に残っているエントリのうち、keep_range 内にいて
    /// かつ state が Loaded でないものを検出してログに出す。Loaded なら
    /// from_cache=false の finalize 待ちなので問題なし。
    pub(crate) last_stuck_scan_at: std::time::Instant,
    /// 検索バーにフォーカスを当てるフラグ（1フレームだけ true）
    pub(crate) search_focus_request: bool,
    /// 検索バーの TextEdit がフォーカスを持っているか（毎フレーム更新）
    pub(crate) search_has_focus: bool,
    /// Ctrl+F の「検索対象」ドロップダウン選択 (§19.7)。既定は全ソース OR。
    pub(crate) search_target: crate::fts_index::SearchTarget,
    /// Ctrl+F の OR 検索モード (docs §20)。`true` で include トークンを OR 結合 (NOT は AND)。
    pub(crate) search_or_mode: bool,

    // ── 回転 DB ──────────────────────────────────────────────────
    /// 回転情報 DB (全体で 1 ファイル)
    pub(crate) rotation_db: Option<crate::rotation_db::RotationDb>,
    /// 現在フォルダのアイテムごとの回転キャッシュ (idx → Rotation)
    pub(crate) rotation_cache: std::collections::HashMap<usize, crate::rotation_db::Rotation>,

    // ── 動画ピン / ブックマーク DB (Phase 5.4 / Phase 6) ──────────
    /// 動画フレーム ピン留め DB。Phase 5 では UI 配線していたが、Phase 6 で
    /// 「動画にピン UI は要らない」方針に変更したため authoring UI は削除。
    /// DB スキーマと open() 自体は維持し、将来再導入が容易な状態を保つ
    /// (= フィールド削除は break 変更が大きいため後回し)。
    #[allow(dead_code)]
    pub(crate) video_pin_db: Option<crate::video_pins::VideoPinDb>,
    /// ユーザーが任意位置に付けた付箋。フルスクリーン左パネルにジャンプサムネとして
    /// 表示される。Phase 5.4 / 5.4.1 で B キー authoring 配線。
    pub(crate) video_bookmark_db: Option<crate::video_bookmarks::VideoBookmarkDb>,

    // ── 動画タイルモード (Phase 5.5) ─────────────────────────────
    /// S キーでトグルされる動画タイルモード状態。再生中に間隔別にサムネを並べて
    /// クリックでシーク。VideoPlayer 切替で自動破棄 (Drop で worker 終了)。
    #[cfg(windows)]
    pub(crate) video_tile_state: Option<crate::ui_video_tile::VideoTileState>,
    /// タイルモードのサムネ texture キャッシュ (slot_idx → (key, tex))。
    /// state Drop 時にこちらも `clear()` で解放する想定。
    #[cfg(windows)]
    pub(crate) video_tile_textures:
        std::collections::HashMap<usize, ((u64, u64, u32, u32), egui::TextureHandle)>,
    /// 動画ジャンプパネル (左) の各行サムネ texture キャッシュ。
    /// key: bucket(pts_secs) — 同一 pts は同じ texture を再利用。動画切替で
    /// `clear()` する。
    #[cfg(windows)]
    pub(crate) video_jump_textures:
        std::collections::HashMap<i64, ((u64, u32, u32), egui::TextureHandle)>,
    /// タイルモードのサムネ WebP 永続キャッシュ (Phase 6.D-2)。
    /// 同 (動画 path, interval, tile_w, slot, video_mtime) のキーで再オープン時に
    /// 即座に表示できる。
    #[cfg(windows)]
    pub(crate) video_tile_cache:
        Option<std::sync::Arc<crate::video::tile_thumb_cache::TileThumbCache>>,

    // ── レーティング DB ──────────────────────────────────────────
    /// レーティング DB (全体で 1 ファイル)
    pub(crate) rating_db: Option<crate::rating_db::RatingDb>,
    /// 現在フォルダのアイテムごとのレーティングキャッシュ (idx → 0..=5)
    pub(crate) rating_cache: std::collections::HashMap<usize, u8>,
    /// ユーザが明示的に set_rating した path (normalize 済みキー) の記録。
    /// tag_prewarm が古い XMP を背景で読み戻してきても、ここに入っている path は
    /// ハイドレーション対象から外す (F6 で 0 にしたのに古い★が蘇る race の防止)。
    /// フォルダ切替で load_folder がクリアする。
    pub(crate) user_set_rating_keys: std::collections::HashSet<String>,
    /// `current_folder` (コンテナ) 自身のレーティングキャッシュ。アドレスバー描画で
    /// 毎フレーム参照されるので SQLite を叩かない。`None` は未計算を意味する。
    /// `load_folder` / `set_current_folder_rating` / `set_rating` (コンテナ変更時) で
    /// `None` に戻して無効化する。
    pub(crate) current_folder_rating_cache: Option<u8>,

    /// ★フィルタの一時解除: 自分の★を持つコンテナ (Folder / ZIP / PDF) を開いた時、
    /// 中身には個別★が付いていないために全項目が非表示になる "空表示" 事故を防ぐ。
    ///
    /// 発動: 開くアクションで [`maybe_suppress_rating_filter_for_opened_container`] が
    /// コンテナ自身の★が現在フィルタを通ることを確認したときのみ。
    /// scope: anchor (= 開いた container の path) の subtree 内にいる間だけ解除。
    /// 復元: `load_folder` 系で `effective_folder()` が anchor scope 外に出たら実行する
    /// ([`maybe_restore_rating_filter_if_out_of_scope`])。
    /// 破棄: ユーザーが手動で filter を変更した時 (意図的操作を尊重して復元せず捨てる)。
    ///
    /// `settings.rating_filter` は suppression 中は in-memory 上 `[true; 6]` (全 ON) に
    /// 書き換えるが **`settings.save()` を呼ばない**。永続化は元のフィルタ値のまま残る
    /// ので、アプリ再起動時はユーザーの saved filter に戻る。
    ///
    /// (anchor_path, 保存した旧フィルタ)
    pub(crate) rating_filter_suppressed_at: Option<(PathBuf, [bool; 6])>,

    /// 再帰レーティングフィルタ: 現フォルダ直下のコンテナごとの子孫★件数バッファ。
    /// key は `adjustment_db::normalize_path` で正規化したパス。
    pub(crate) folder_rating_counts:
        std::collections::HashMap<String, crate::folder_rating_counter::StarCounts>,
    pub(crate) folder_rating_counts_loaded: bool,
    pub(crate) folder_rating_counter_handle:
        Option<crate::folder_rating_counter::FolderRatingCounterHandle>,
    /// worker を起動したときの `current_folder` 正規化キー。`ensure_folder_rating_counter`
    /// が同フォルダで再 spawn しないための change-detection に使う (handle を見ると
    /// worker 終了直後に再 spawn ループになる既知バグを避けるため handle ではなくこれ)。
    pub(crate) folder_rating_counts_folder_key: Option<String>,

    /// Ctrl+G drilled view 限定: サブフォルダ毎の per-★ ヒット件数 (★なし..★5、6 バケット)。
    /// `rebuild_items_from_global_search` の DrilledInto 分岐で `state.all_hits` から
    /// 直接集計する。folder_rating_counter (rating DB を SQL で集計する worker) は
    /// global_search.active 中は走らないため、ここで Ctrl+G 専用の経路を持たせる。
    /// 通常フォルダ閲覧時は空。 keys は `adjustment_db::normalize_path` 正規化済み。
    /// `[u32; 6]` の index 0=★なし, 1..5=★1..★5。`folder_rating_match` がここを優先参照する。
    pub(crate) search_drilled_folder_counts: std::collections::HashMap<String, [u32; 6]>,

    // ── 見開き表示 ──────────────────────────────────────────────
    /// 見開き DB (フォルダごとのモード永続化)
    pub(crate) spread_db: Option<crate::spread_db::SpreadDb>,
    /// 現在のフォルダの見開きモード
    pub(crate) spread_mode: crate::settings::SpreadMode,
    /// 見開きモード切替ポップアップ表示中
    pub(crate) spread_popup_open: bool,

    // ── スライドショー ────────────────────────────────────────────
    /// スライドショー再生中フラグ
    pub(crate) slideshow_playing: bool,
    /// 次の画像に切り替える時刻
    pub(crate) slideshow_next_at: std::time::Instant,

    // ── フルスクリーンビューポート ─────────────────────────────
    /// フルスクリーンビューポートが現在表示中か（Visible+Focus 送信済み）
    pub(crate) fs_viewport_shown: bool,
    /// フルスクリーン開始時刻（フォーカス移行のグレース期間用）
    fs_opened_at: Option<std::time::Instant>,
    /// グレース期間を超えたかのキャッシュ（毎フレーム Instant::elapsed() を避ける）
    fs_focus_grace_elapsed: bool,
    /// フルスクリーンビューポートの前フレームのフォーカス状態。
    /// フォーカス復帰クリックの検出に使う。
    pub(crate) fs_prev_focused: bool,
    /// フルスクリーンビューポートがフォーカスを取り戻した時刻。
    /// この直後のクリックは他アプリからの復帰クリックとみなし、
    /// ナビ・ドラッグ等のアプリ側処理を抑制する。
    pub(crate) fs_focus_regained_at: Option<std::time::Instant>,
    /// フォーカス復帰クリックを検出中で、離されるまで全ての左クリック操作を抑制するフラグ。
    /// 押下 → 離しの間に複数フレームあるため、時間ベースだけでなく状態でも追跡する。
    pub(crate) fs_suppress_primary_until_release: bool,

    // ── 通常フルスクリーン ズーム/パン/任意回転 ──────────────
    /// 通常フルスクリーンのズーム倍率（1.0 = フィット）
    pub(crate) fs_zoom: f32,
    /// 通常フルスクリーンのパンオフセット（スクリーン座標系）
    pub(crate) fs_pan: egui::Vec2,
    /// 通常フルスクリーンのパンドラッグ開始状態
    pub(crate) fs_pan_drag_start: Option<(egui::Pos2, egui::Vec2)>,
    /// 任意角度回転（ラジアン、一時的・保存しない）
    pub(crate) fs_free_rotation: f32,
    /// 回転ドラッグ開始状態（開始位置, 開始時の回転角）
    pub(crate) fs_rotation_drag_start: Option<(egui::Pos2, f32)>,
    /// ルーペ常時表示トグル (M キー)。Shift ホールドは独立に追加表示される。
    /// フルスクリーンを閉じても保持されるが、フォーカス喪失中はレンダリングしない。
    pub(crate) fs_loupe_locked: bool,
    /// 見開きモード描画後のページ矩形。ルーペ描画がカーソル位置から該当ページを
    /// 特定するのに使う。毎フレーム描画後に更新、非見開き時は None。
    pub(crate) fs_spread_layout: Option<crate::ui_fullscreen::FsSpreadLayout>,
    /// 透過画像の背景サイクル (B キー): 0=テーマ既定 / 1=白 / 2=黒 / 3=市松
    /// 画像切替時にリセット。永続化しない。
    pub(crate) fs_transparent_bg_mode: u8,
    /// 16×16 の市松テクスチャ (Wrap=Repeat)。最初に B キーで市松にしたとき lazy init。
    pub(crate) fs_checker_texture: Option<egui::TextureHandle>,
    /// 背景モード変更直後に表示するインジケータの消去期限。
    pub(crate) fs_transparent_bg_indicator_until: Option<std::time::Instant>,
    /// ポストフィルタ (AdjustParams.post_filter) を一時的にバイパスするフラグ。
    /// 消しゴム / 分析モード中は true にして、apply_sync_adjustment が post-filter
    /// 段をスキップし color-only のテクスチャを作るようにする。
    /// モード終了時に false に戻し、adjustment_cache をクリアして post-filter を復帰させる。
    pub(crate) post_filter_bypassed: bool,

    // ── 画像分析パネル ────────────────────────────────────────
    /// フルスクリーンで分析パネルを表示するか
    pub(crate) analysis_mode: bool,
    /// 分析パネル: マウス位置のピクセル色（画像座標系で取得）
    pub(crate) analysis_hover_color: Option<[u8; 4]>,
    /// 分析パネル: 右クリックで固定した比較色
    pub(crate) analysis_pinned_color: Option<[u8; 4]>,
    /// 分析パネル: グレースケール表示モード（G キー）
    pub(crate) analysis_grayscale: bool,
    /// 分析パネル: モザイクグリッド表示（M キー）
    pub(crate) analysis_mosaic_grid: bool,
    /// 分析パネル: 色差強調フィルターの倍率（0=無効, 2/5/10/20）
    pub(crate) analysis_filter_mag: u8,
    /// 分析パネル: ドラッグ計測ライン（開始点, 現在点：画像ピクセル座標, 修飾キー色インデックス）
    pub(crate) analysis_guide_drag: Option<(egui::Pos2, egui::Pos2, u8)>,
    /// 分析パネル: ズーム倍率（1.0 = フィット表示）
    pub(crate) analysis_zoom: f32,
    /// 分析パネル: パンオフセット（画像ピクセル座標系、画像中心からのズレ）
    pub(crate) analysis_pan: egui::Vec2,
    /// 分析パネル: ドラッグ中の開始オフセット
    pub(crate) analysis_pan_drag_start: Option<(egui::Pos2, egui::Vec2)>,
    /// 分析パネル: フィルター/グレースケールのキャッシュテクスチャ
    pub(crate) analysis_overlay_cache: Option<(
        egui::TextureHandle,
        u8,
        Option<[u8; 4]>,
        f32,
        egui::Vec2,
        usize,
    )>,
    /// 分析パネル: ヒストグラムキャッシュ (zoom, pan, image_idx) → 結果
    pub(crate) analysis_hist_cache:
        Option<(f32, egui::Vec2, usize, [u32; 360], [u32; 256], [u32; 256])>,
    /// 分析パネル: SVマップキャッシュ
    pub(crate) analysis_sv_cache: Option<(f32, egui::Vec2, usize, egui::TextureHandle)>,

    // ── 起動時の前回フォルダ復元フラグ ──────────────────────────
    pub(crate) initialized: bool,

    // ── UI テーマ (v0.7.0) ──────────────────────────────────────
    /// 直近に ctx に適用した「解決後」のテーマ (Light / Dark)。
    /// `settings.ui_theme` が `System` のとき、設定値は変わらなくても
    /// Windows 側の Light/Dark 切替で解決後の値が変わるため、毎フレーム
    /// `os_theme::resolve` の結果と比較して再適用する。
    pub(crate) applied_ui_theme: Option<crate::os_theme::ResolvedTheme>,

    /// eframe の wgpu::Device / Queue / Adapter / target_format。動画 GPU レンダリング
    /// で `wgpu::Device::create_texture_from_hal::<Dx12>` 経由で D3D11 NT shared
    /// テクスチャを import するために必要。`main.rs` の creator closure で
    /// `cc.wgpu_render_state` から渡される。`None` なら GPU 動画レンダリング無効。
    #[cfg(windows)]
    pub(crate) wgpu_render_state: Option<egui_wgpu::RenderState>,
    /// 動画 GPU レンダリング用の共有 D3D11 デバイス。VideoPlayer ごとに生成すると
    /// FFmpeg HW デコーダ + ID3D11VideoProcessor を別デバイスに作る重複コストが
    /// 発生するため、App レベルで 1 つ確保して使い回す。`None` の場合は
    /// 旧経路 (CPU readback + swscale) にフォールバック。
    #[cfg(windows)]
    pub(crate) gpu_video_device:
        Option<std::sync::Arc<crate::video::gpu_renderer::GpuVideoDevice>>,

    // ── PDF パスワード管理 ───────────────────────────────────────
    pub(crate) pdf_passwords: crate::pdf_passwords::PdfPasswordStore,
    pub(crate) show_pdf_password_dialog: bool,
    pub(crate) pdf_password_input: String,
    /// 「パスワードを保存する」チェックボックス (デフォルト OFF)
    pub(crate) pdf_password_save: bool,
    pub(crate) pdf_password_error: Option<String>,
    /// パスワード入力待ちの PDF パス
    pub(crate) pdf_password_pending_path: Option<PathBuf>,
    /// 現在開いている PDF のパスワード (セッション中キャッシュ)
    pub(crate) pdf_current_password: Option<String>,
    /// 非同期 enumerate 成功時に DPAPI 保存するための一時記録 (path, password)。
    /// パスワードダイアログで「保存する」を ON にして OK した時にセットされ、
    /// `poll_pdf_enumerate` の成功経路で消費される (path 一致時のみ save)。
    pub(crate) pdf_password_pending_save: Option<(PathBuf, String)>,

    // ── ZIP 非同期ロード ────────────────────────────────────────
    /// ZIP (特にネスト ZIP) の中央ディレクトリ列挙を UI スレッドから外すための pending。
    /// `load_zip_as_folder` が worker を spawn して即 return し、`poll_zip_enumerate` が
    /// 結果を受け取って `start_loading_items` に流す。
    ///
    /// pending 中は `current_folder = zip_path` / `items.is_empty() = true` となり、
    /// grid は「読み込み中…」を表示する。BS / Ctrl+↑↓ はこのタイミングで発火しても
    /// `load_folder` に入って pending を Drop するだけなので、worker は cancel を見て
    /// harmless に抜ける (Drop の自動 cancel と同じ構造)。
    pub(crate) zip_enumerate_pending: Option<ZipEnumeratePending>,

    // ── PDF 非同期ロード ────────────────────────────────────────
    /// PDF レンダリング完了時に content_type を受け取るチャネル
    pub(crate) pdf_content_type_tx: mpsc::Sender<(usize, crate::pdf_loader::PdfPageContentType)>,
    pub(crate) pdf_content_type_rx: mpsc::Receiver<(usize, crate::pdf_loader::PdfPageContentType)>,
    /// ページ列挙の非同期応答待ち: (pdf_path, password, handle)。
    ///
    /// `handle` を drop (新しい pending への置き換え含む) すると `PdfEnumerateHandle::Drop`
    /// が自動的に cancel を立て、pool dispatcher が pop 時に IPC 前で古いジョブを捨てる。
    pub(crate) pdf_enumerate_pending:
        Option<(PathBuf, Option<String>, crate::pdf_loader::PdfEnumerateHandle)>,
    /// Ctrl+↑↓ フォルダナビで非同期 PDF / ZIP に着地したときに保存する方向フラグ。
    /// `poll_pdf_enumerate` / `poll_zip_enumerate` が items を埋めたあとで fullscreen を
    /// 開き直すために使う (poll 側でフラグを take して先頭/末尾画像を open_fullscreen する)。
    /// `Some(forward)`: DFS 方向 (true=前方/下巻方向, false=後方/上巻方向)。
    ///
    /// PDF と ZIP の pending は同時に立つことがない (load_pdf / load_zip が互いにクリア
    /// するため) ので、単一フィールドで両方を賄える。
    pub(crate) fs_nav_after_pdf_enumerate: Option<bool>,

    // ── コンテキストメニュー: enumerate_handlers キャッシュ ────
    /// 拡張子ごとのシステム関連付けアプリ一覧キャッシュ (コンテキストメニュー開閉でクリア)
    pub(crate) cached_handlers: Option<(String, Vec<crate::open_with::AppHandler>)>,

    // ── 見開きペア解決用 nav_indices キャッシュ ────────────────
    /// フレーム内で build_nav_indices の結果をキャッシュ (items/visible_indices 変更でクリア)
    pub(crate) cached_nav_indices: Option<Vec<usize>>,

    // ── AI アップスケール ──────────────────────────────────────────
    /// AI ランタイム (ONNX Runtime)
    pub(crate) ai_runtime: Option<std::sync::Arc<crate::ai::runtime::AiRuntime>>,
    /// AI モデルマネージャ
    pub(crate) ai_model_manager: std::sync::Arc<crate::ai::model_manager::ModelManager>,
    /// AI アップスケール有効フラグ
    pub(crate) ai_upscale_enabled: bool,
    /// AI アップスケールモデルの手動オーバーライド (None = 自動)
    pub(crate) ai_upscale_model_override: Option<crate::ai::ModelKind>,
    /// AI デノイズモデル (Some = 有効)
    pub(crate) ai_denoise_model: Option<crate::ai::ModelKind>,
    /// アップスケール済みキャッシュ: (item_idx, bg_mode) → テクスチャ + ピクセルデータ。
    /// bg_mode は 0 (黒) / 1 (白) のみ (composite-first 方式で背景色が出力に焼き付くため、
    /// 高速切替のために 2 バリアントを保持する)。
    pub(crate) ai_upscale_cache: std::collections::HashMap<(usize, u8), FsCacheEntry>,
    /// アップスケール処理中: (item_idx, bg_mode) → (キャンセルトークン, 受信チャネル)
    pub(crate) ai_upscale_pending: std::collections::HashMap<
        (usize, u8),
        (
            Arc<AtomicBool>,
            mpsc::Receiver<crate::ai::upscale::UpscaleResult>,
        ),
    >,
    /// 画像タイプ分類キャッシュ: item_idx → カテゴリ
    pub(crate) ai_classify_cache: std::collections::HashMap<usize, crate::ai::ImageCategory>,
    /// バージョン情報ダイアログ
    pub(crate) show_about_dialog: bool,
    /// AI アップスケールが失敗した (item_idx, bg_mode) の集合（リトライ防止）
    pub(crate) ai_upscale_failed: std::collections::HashSet<(usize, u8)>,
    /// AI ステータス表示の完了時刻（全処理完了後に記録、一定時間後に非表示）
    pub(crate) ai_status_done_at: Option<std::time::Instant>,

    // ── 画像補正 ──────────────────────────────────────────────────
    /// 補正パネル表示フラグ (左パネルホバーで表示)
    pub(crate) adjustment_mode: bool,
    /// 見開き表示中に補正パネルが操作する側 (画面上の左/右)。Single 表示中は
    /// 参照されない。`open_fullscreen` / spread_mode 切替で Left にリセット。
    pub(crate) adjust_spread_target: AdjustSpreadTarget,
    /// ページ個別の補正パラメータ: item_idx → AdjustParams
    /// ここに登録されていないページは「お気に入り標準 → グローバル (settings.global_preset)」
    /// の順でフォールバックする。解決は [`App::effective_params`] に集約。
    pub(crate) adjustment_page_params:
        std::collections::HashMap<usize, crate::adjustment::AdjustParams>,
    /// お気に入り単位の標準パラメータ: favorite_id → AdjustParams。
    /// 起動時に `adjustment_db.load_all_favorite_params()` から復元。
    /// 解決は [`App::effective_params`] 参照。
    pub(crate) adjustment_favorite_params:
        std::collections::HashMap<uuid::Uuid, crate::adjustment::AdjustParams>,
    /// 現フォルダでマスクを持つページの item_idx 集合 (サムネイル「消」バッジ描画用)。
    /// フォルダロード時に mask_db から一括取得し、save/delete/apply でメンテナンスする。
    pub(crate) mask_pages: std::collections::HashSet<usize>,
    /// 補正済み画像キャッシュ: item_idx → テクスチャ + ピクセルデータ
    pub(crate) adjustment_cache: std::collections::HashMap<usize, FsCacheEntry>,
    /// サムネイル補正用ソースピクセル: idx → Arc<ColorImage>。
    /// keep_range 内の Loaded サムネに対して保持し、補正パラメータ変更で
    /// 同期的に `apply_adjustments_fast` を掛け直すための元ソース。
    /// 範囲外に evict されたら drop する。post_filter は適用しない (色調のみ)。
    pub(crate) thumb_pixels: std::collections::HashMap<usize, std::sync::Arc<egui::ColorImage>>,
    /// サムネイル補正済みテクスチャ: idx → TextureHandle。
    /// `effective_params(idx)` が色調 identity でないときのみ格納。
    /// サムネ描画時は `thumbnails[idx].tex` より優先される。
    pub(crate) thumb_adjust_tex: std::collections::HashMap<usize, egui::TextureHandle>,
    /// 前フレームのスライダードラッグ状態。true→false 遷移を検知して
    /// release 時に `thumb_adjust_tex` を全無効化する。
    pub(crate) thumb_adjust_was_dragging: bool,
    /// スライダードラッグ中フラグ（パネル内ウィジェットのドラッグ検出）
    pub(crate) adjustment_dragging: bool,
    /// 補正 DB ハンドル
    pub(crate) adjustment_db: Option<crate::adjustment_db::AdjustmentDb>,
    /// シャープネス適用済みの idx 集合（再適用防止）
    pub(crate) adjustment_sharpened: std::collections::HashSet<usize>,
    /// スロット保存ダイアログ: (slot_idx, 入力中の名前, 保存対象の補正値)。
    /// 補正値は開始時にキャプチャしておくことで、見開きの L/R 切替や
    /// ダイアログ中にスライダーを動かしても保存内容がブレない。
    pub(crate) slot_save_dialog: Option<(usize, String, crate::adjustment::AdjustParams)>,
    /// IME 変換中フラグ。Ime イベントで更新される持続状態。
    /// Enabled/Preedit(非空) で true、Preedit("")/Commit/Disabled で false。
    pub(crate) ime_composing: bool,
    /// 直近の Ime イベント受信時刻 (時間ベースのガードに使う)。
    /// Windows IME のキャンセル Escape では Ime::Disabled と Key::Escape が別フレームに
    /// 分かれて届くことがあるため、Ime イベント後 300ms は IME 入力中として扱う。
    pub(crate) ime_last_event_at: Option<std::time::Instant>,
    /// 右上フィードバック表示: (テキスト, 表示開始時刻)。フルスクリーン / グリッド共通。
    /// 命名の `fs_` プレフィックスはフルスクリーン専用だった頃の名残。
    pub(crate) fs_feedback_toast: Option<(String, std::time::Instant)>,
    /// TRT worker クラッシュ / 起動失敗の通知バナー (Phase 3 Step 5)。
    /// `AiRuntime::take_worker_notice()` を update 毎にポーリングし、`Some` を
    /// 引いたらここへ転写する。バナー UI で「再起動」/「閉じる」が押されるまで
    /// 残る (時間で消えない、ユーザーが認識する必要があるため)。
    pub(crate) trt_worker_notice: Option<crate::ai::runtime::WorkerNotice>,
    /// セッション中に worker 死亡 → 自動再起動を試みた回数。
    /// `MAX_TRT_AUTO_RESTART_ATTEMPTS` 回まで silent に再 spawn して、それを超えたら
    /// バナーを出してユーザー操作を待つ (= TRT pack 自体の問題等で永久ループしない
    /// ためのガード)。
    pub(crate) trt_auto_restart_attempts: u32,
    /// 自動再起動が現在進行中か (= spawn_trt_worker_pool の background thread が
    /// 起動中で、まだ attach も failure 通知もしていない状態)。
    /// 並行する複数の AI 推論が同じ「死亡」イベントを観測したときに 2 重 spawn を
    /// 防ぐためのガード (Codex P2 指摘)。spawn 完了 (attach 成功 or 失敗通知) で
    /// false に戻す。
    pub(crate) trt_restart_in_flight: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// TRT pack のオンラインインストールダイアログの状態。
    /// Some の間ダイアログが表示され、worker thread が動作している。
    /// 閉じる (Drop) と worker は cancel される。
    pub(crate) trt_install_state: Option<crate::ui_dialogs::trt_install::TrtInstallState>,
    /// フルスクリーン中央のヒントオーバーレイ。
    /// 最後/最初の画像でさらに進もう/戻ろうとしたとき、または Ctrl+↑↓ で
    /// 画像のあるフォルダが skip_limit 以内に見つからなかったときに表示する。
    pub(crate) fs_boundary_hint: Option<crate::ui_fullscreen::FsBoundaryHint>,

    // ── 消しゴム (Erase) モード ───────────────────────────────────
    /// E キーで切り替える消しゴムモード
    pub(crate) erase_mode: bool,
    /// マスクビットマップ（画像と同サイズ、true = マスク済み）
    pub(crate) erase_mask: Option<Vec<bool>>,
    /// マスク対象の画像サイズ [width, height]
    pub(crate) erase_mask_size: [usize; 2],
    /// マスクオーバーレイ用テクスチャ
    pub(crate) erase_mask_texture: Option<egui::TextureHandle>,
    /// 消しゴムモードでドラッグ中か（前フレームのポインタ位置を保持）
    pub(crate) erase_last_paint_pos: Option<egui::Pos2>,
    /// 現在のツール種別
    pub(crate) erase_tool: EraseTool,
    /// 筆ツールの半径 (画像ピクセル)
    pub(crate) erase_brush_radius: f32,
    /// 囲みツールのポイント列 (画像ピクセル座標)
    pub(crate) erase_lasso_points: Vec<(f32, f32)>,
    /// 縦線/横線ツールのドラッグ開始点 (画像ピクセル座標)
    pub(crate) erase_line_start: Option<(f32, f32)>,
    /// 縦線/横線ツールのドラッグ現在点 (画像ピクセル座標)
    pub(crate) erase_line_end: Option<(f32, f32)>,
    /// 縦線/横線ツールの傾き量 (画像ピクセル単位の上端オフセット)。
    /// Ctrl+ドラッグ時のみ非ゼロになる。
    pub(crate) erase_line_tilt: f32,
    /// 直線ツールの幅 (画像ピクセル)。
    pub(crate) erase_line_width: f32,
    /// Ctrl+ドラッグ中の状態 (None なら未アクティブ)。
    pub(crate) erase_shift_drag: Option<ShiftDragState>,
    /// 描画モード (true) / 消去モード (false)
    pub(crate) erase_paint_mode: bool,
    /// inpaint 適用前の元画像キャッシュ: item_idx → ピクセルデータ
    /// inpaint 実行後も元画像を保持し、マスク変更時に常に元画像から再適用する。
    pub(crate) erase_base_cache: std::collections::HashMap<usize, std::sync::Arc<egui::ColorImage>>,
    /// マスク永続化 DB
    pub(crate) mask_db: Option<crate::mask_db::MaskDb>,
    /// 消しゴムの Undo スタック (マスク/ベクタ両方のスナップショット、最大 20 エントリ)
    pub(crate) erase_undo_stack: std::collections::VecDeque<EraseSnapshot>,
    /// メタ操作 (レーティング / タグ / 画像補正) の Undo/Redo スタック。フォルダ移動・
    /// フルスクリーン遷移・フルスクリーン中のページ移動・消しゴムモード遷移でクリアされる。
    pub(crate) meta_undo: crate::undo_stack::UndoStack,
    /// 画像補正パネルのスライダードラッグセッション。`Some` のときはドラッグ中で、
    /// 中身は「ドラッグ開始時の `adjustment_page_params[fs_idx]`」のスナップショット。
    /// ドラッグ中は DB 書き込みをスキップし in-memory のみ更新、ドラッグ終了時に
    /// 一括で `set_page_params` を呼んで永続化 + Undo エントリを 1 件積む。
    pub(crate) adjustment_drag_session: Option<AdjustmentDragSession>,
    /// タグ操作の保留 Undo 集計バッファ。`request_tag_*_for_selection` が tx_id を発行し
    /// `PendingTagUndo` を入れ、`poll_tag_write_results` が worker 結果 1 件ごとに
    /// `TagChange` を accumulate する。完了時に「実 disk の before/after」で構築した
    /// UndoEntry を `meta_undo` に push する。
    /// これにより外部ツールで XMP が書き換わって tags_cache が stale でも、Undo entry は
    /// worker が読んだ真の disk 状態を持つので Ctrl+Z 連打で他タグを破壊しない。
    pub(crate) pending_tag_undos: std::collections::HashMap<u64, PendingTagUndo>,
    /// 次に発行するタグ操作の tx_id。0 は「Undo 確定不要」を表す予約値なので 1 から始める。
    pub(crate) next_tag_tx_id: u64,
    /// 直前の push_undo_snapshot 時刻。矢印/[/]キー連打時のスナップショット重複を抑制する
    /// (OS のキーリピートで毎フレーム full-bitmap clone が走るのを防ぐ)。
    pub(crate) erase_last_undo_at: Option<std::time::Instant>,
    /// 縦線/横線/直線のベクタオブジェクト群 (消しゴムモード中のみ有効)。
    /// 筆/囲みは `erase_mask` 側に直接ラスタライズされる。
    pub(crate) erase_vectors: Vec<crate::mask_db::LineObject>,
    /// 選択中のベクタオブジェクトインデックス (`erase_vectors` への添字)。
    pub(crate) erase_selected_vector: Option<usize>,
    /// ベクタオブジェクト編集のドラッグ状態。
    pub(crate) erase_vector_drag: Option<EraseVectorDrag>,
    /// バックグラウンド実行中の MI-GAN inpaint (idx 別)。
    /// 見開き消しゴムで「左ページ apply → 右ページへ移動 → 右ページ apply」と続けたとき、
    /// 旧実装は `Option` で 1 件しか持たず、右ページの投入が左ページのジョブを
    /// `cancel.store(true)` してしまっていた。idx ごとに独立保持することで両方の
    /// inpaint 結果を確実に受け取る。同じ idx への再投入時のみ前ジョブを cancel する。
    /// UI スレッドが MI-GAN 推論 (300-500ms) で固まらないようにするための非同期化エントリ。
    pub(crate) erase_inpaint_pending: std::collections::HashMap<usize, crate::ui_erase::EraseInpaintPending>,
    /// 見開きから消しゴムに入ったときの状態スナップショット。`Some` の間は終了時に
    /// `spread_mode` を `saved_mode` へ戻し、`fullscreen_idx` を `pair.0` (左ページ) に
    /// 揃えて見開きを復元する。Single から入った場合・見開き中でも片側だけのページ
    /// (表紙・末尾奇数・横長画像) から入った場合は `None`。
    /// 2 値を 1 構造体にまとめてあるのは「mode と pair は常に同時に set / take される」
    /// という不変条件をフィールドの形で表現するため。
    pub(crate) erase_spread_ctx: Option<EraseSpreadCtx>,

    // ── フルスクリーン Ctrl+↑↓ ナビロック ─────────────────────────
    /// ナビ中の「次のページがまだ表示できない」ガード。`Some(gen)` の間は
    /// 新たな Ctrl+↑↓ 入力を無視し、`fs_holdover_tex` を画面に出し続ける。
    /// `gen` はロック発火時の `items_generation` のスナップショット。items が
    /// 入れ替わる (= `install_new_items` で `items_generation` が進む) まで
    /// ロックを解除しない。これをやらないとナビ発火直後 (DFS 完了前で fs_idx も
    /// 旧ページのまま) の `poll_fs_nav_lock` が「現ページはロード済」と判定して
    /// ロックを即解除してしまい、items 入れ替えの瞬間に holdover が失われて
    /// 「ファイル名のみ表示」が出る不具合になる。
    pub(crate) fs_nav_locked_gen: Option<u64>,
    /// ロック中に「現在画面に映っていた」テクスチャを退避しておく場所。
    /// items 入れ替え (`install_new_items`) で fs_cache が全 drop されても、
    /// ナビ発火直前に Arc::clone でここに保持しておけば描画パスから参照可能。
    /// `egui::TextureHandle` は内部 Arc なので clone は refcount inc だけ。
    pub(crate) fs_holdover_tex: Option<egui::TextureHandle>,

    /// 仮想フォルダ (PDF/ZIP) 進入時に「最初のページのサムネを完成させたら親 catalog
    /// にも書き戻す」ための write-back ターゲット。`Some` のとき finalize シグナルが
    /// `target_idx` で届いた時点で `cache_map` から WebP データを取り出して親 catalog
    /// の `parent_key` (= `pdfthumb:foo.pdf` / `zipthumb:foo.zip`) に保存する。
    /// 一度発火したら None に戻して二重書き込みを防ぐ。
    /// items 入れ替え時 (`start_loading_items` 冒頭) に再初期化される。
    pub(crate) virtual_folder_writeback: Option<VirtualFolderWriteback>,

    /// PDFium ワーカープールの thrash 抑制用 grace period。
    /// PDF/ZIP を仮想フォルダとして開いた直後の数十 ms はユーザーが Ctrl+↑↓ で
    /// すぐ次のフォルダに移る確率が高く、prefetch (1 ページ目以外) を即時 enqueue
    /// すると PDFium 側で in-flight cancel ができないため (= ページ単位レンダは
    /// atomic で、dispatch 後はキャンセル不可)、無駄なレンダリングがプールを占有する。
    /// `Some(deadline)` の間は prefetch 対象 (= 現ページ以外の PdfPage) の enqueue を
    /// スキップして worker の暇をユーザーの確定操作のために空けておく。
    pub(crate) pdf_prefetch_grace_until: Option<std::time::Instant>,

    // ── カタログ LRU キャッシュ ───────────────────────────────────
    /// folder_path → Arc<CatalogDb> の小さい LRU。`start_loading_items` の
    /// `CatalogDb::open` cold path が UI スレッドを止めるのを抑えるため、
    /// 直近 N 個の `Arc<CatalogDb>` を保持して revisit 時の open をスキップする。
    /// connection を握っている間は OS-level の file cache も warm に保たれる
    /// 副次効果がある。容量超過時は FIFO で古いものから drop (Connection close)。
    pub(crate) catalog_cache: std::collections::HashMap<PathBuf, Arc<crate::catalog::CatalogDb>>,
    /// `catalog_cache` 用 LRU 順 (古い順)。
    pub(crate) catalog_cache_order: std::collections::VecDeque<PathBuf>,

    // ── パフォーマンス計装 (--perf-log 時のみ有効) ────────────────
    /// ユーザー入力単位で単調増加するシーケンス番号。キー・ホイール・選択変更
    /// などの入力イベントで +1 する。ワーカーに投げるタスクに copy して渡し、
    /// "入力 → 表示" のレイテンシを相関させる。0 は「未設定」の意味。
    pub(crate) input_seq: u64,
    /// 直近のユーザー入力時刻 (`bump_input_seq` で更新)。
    /// 入力直後のクールダウン中はアイドル品質アップグレードを抑制するために使用。
    pub(crate) last_input_at: Option<std::time::Instant>,
    /// フレーム番号 (update 呼出しのたびに +1)
    pub(crate) frame_counter: u64,
    /// 最後に perf::flush() した時刻。約 1 秒に 1 回フラッシュする。
    pub(crate) perf_last_flush: Option<std::time::Instant>,
    /// 直近フレームでフルスクリーンが描画した (idx, texture_id, input_seq)。
    /// 変化を検出したフレームで `fs.paint` イベントを発火する。
    pub(crate) fs_painted_last: Option<(usize, egui::TextureId, u64)>,

    /// 動画シークホバー時のサムネ表示用テクスチャ。1 動画につき 1 つだけ持ち、
    /// hover 中の target_secs key が変わるたびに `set()` で in-place 更新する。
    /// (fs_idx, bucket_key, TextureHandle) — fs_idx 切替時に再作成。
    pub(crate) video_seek_thumb_tex: Option<(usize, i64, egui::TextureHandle)>,
    /// 動画再生位置の自動保存タイマ (5 秒間隔)。
    pub(crate) video_resume_last_save: Option<std::time::Instant>,

    /// フォルダ側サイドカー (`mimageviewer.dat`) のメモリ表現。キーはフォルダの絶対パス。
    /// 中央 DB への書き込みと同じタイミングで更新し、フォルダ切替・終了・5 秒アイドル時に flush する。
    pub(crate) sidecars: std::collections::HashMap<std::path::PathBuf, crate::sidecar::SidecarFile>,

    // ── タスクトレイ常駐 (v0.9) ──────────────────────────────────
    /// タスクトレイコントローラ (専用スレッドで動作)。設定 ON のときだけ初期化される。
    pub(crate) tray_controller: Option<crate::tray::TrayController>,
    /// ウィンドウが現在可視か。[×] による hide で false、トレイ「開く」で true。
    /// 遷移検出で throttle/pause の on/off を切り替える。
    pub(crate) window_visible: bool,
    /// タスクトレイ常駐設定を ON にしたときに 1 回だけ表示する案内ダイアログ。
    /// ユーザーが OK を押すと閉じる。
    pub(crate) show_tray_enabled_notice: bool,

    // ── バージョン更新通知 ────────────────────────────────────────
    /// in-flight な更新チェック (rx + manual モードフラグをまとめる)。`Some` の間は
    /// 重複 spawn しない。`take()` で結果回収時に rx と manual を同時に外せる。
    pub(crate) update_check_pending: Option<UpdateCheckPending>,
    /// 直近の正常な更新チェック結果。`is_newer=true` の間は通知バッジを表示する。
    pub(crate) update_info: Option<crate::update_check::UpdateInfo>,
    /// 直近の manual チェックで失敗したエラーメッセージ (Some の間は失敗ダイアログを開く)。
    /// 既知の `update_info` をクリアしないために別フィールドにしている (Codex P3)。
    pub(crate) update_check_error: Option<String>,
    /// 最後にチェックを spawn した時刻。`UPDATE_CHECK_INTERVAL` 経過で再 spawn。
    pub(crate) update_check_last_spawn: Option<std::time::Instant>,
    /// 「新バージョンがあります」ダイアログを表示中か (バッジクリックで開く)。
    pub(crate) show_update_dialog: bool,
    /// メインウィンドウの HWND (frame.window_handle から最初のフレームで取得)。
    /// Win32 `ShowWindow` でトレイ退避 / 復帰するために必要。
    /// `ViewportCommand::Visible(false)` を使うと eframe が update を呼ばなくなり
    /// トレイメニューから復帰できなくなるため、Win32 直叩きに切り替えた。
    pub(crate) main_hwnd: Option<isize>,
    /// 共有プレースメントスロット。UI スレッドが hide 時にセット、トレイスレッドが
    /// show 時に take して `SetWindowPlacement` する。トレイスレッドから Win32 を直接
    /// 叩けるようにすることで、復帰時の黒フラッシュ / サイズジャンプを防ぐ。
    pub(crate) placement_slot: Option<crate::tray::PlacementSlot>,
    /// 2 重起動検出時に既存インスタンスを前面に出す + インストーラからのクリーン終了
    /// 要求を拾うためのリスナースレッドハンドル。最初のフレームで HWND が取れた
    /// タイミングで spawn し、Drop で join する。
    pub(crate) activation_listener: Option<crate::single_instance::ActivationListener>,
    /// 明示的な終了要求フラグ (`[×]` による close intercept を通過させる)。
    /// set する経路:
    /// - インストーラ (Inno Setup) が Named Event で発火 → activation_listener thread
    /// - メニュー「ファイル → 終了」
    /// いずれもトレイ常駐設定に関係なく、必ず on_exit → プロセス終了まで進める。
    pub(crate) shutdown_requested: Arc<AtomicBool>,

    // ── 外部更新の自動反映 ──────────────────────────────────────
    /// `load_folder` 成功時に記録する current_folder (ディレクトリ実体のみ) の mtime。
    /// トレイ復帰 / フォーカス復帰時に `std::fs::metadata` で新しい mtime を取り、値が
    /// 変わっていたら再ロードする。ZIP / PDF / 検索合成パスには使わないので None のまま。
    pub(crate) current_folder_last_mtime: Option<std::time::SystemTime>,
    /// 直近ロードしたフォルダ内容のシグネチャ (path + mtime + size のハッシュ)。
    /// mtime 変化を検知して再走査した結果が同一なら、items 差し替えをスキップして
    /// 画面ちらつきを防ぐ。`current_folder_last_mtime` と同じくディレクトリ実体のみ。
    pub(crate) current_folder_signature: Option<u64>,
    /// 前フレームの main viewport focus 状態。false → true 遷移で外部更新チェックを走らせる。
    /// 初期値 true: 初回フレームで誤トリガしないため (default focus は true 扱い)。
    pub(crate) last_main_focused: bool,

    // ── 起動オーバーレイ ─────────────────────────────────────────
    /// オーバーレイに表示する短いステータス文字列。バックグラウンド init スレッドが
    /// 各 sub-step の前に書き換える。
    pub(crate) startup_progress: Arc<Mutex<String>>,
    /// 走行中の起動 init スレッドの状態。`update` 内で try_recv して
    /// 完了次第 `indexer_manager` に注入する。
    pub(crate) startup_init: Option<StartupInitPending>,
    /// 起動 init が一度走り切ったか (成功・失敗問わず)。
    /// `indexer_manager.is_none()` だけだと「init 失敗で永続 None」と
    /// 「未起動」を区別できないので別フラグで持つ。
    pub(crate) startup_done: bool,
    /// `fts_meta` の housekeeping (VACUUM) を起動完了後に走らせるための armed フラグ。
    /// `startup_done` で true になり、全 supervisor が idle に達したフレームで
    /// `spawn_housekeeping` を 1 回呼んで false に倒す (Codex 指摘: VACUUM が
    /// supervisor の初回 scan と writer mutex を取り合うのを避ける)。
    pub(crate) housekeeping_armed: bool,
}

impl Default for App {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        let (pdf_ct_tx, pdf_ct_rx) = mpsc::channel();
        let settings = crate::settings::Settings::load();
        let ai_upscale_enabled = settings.ai_upscale_enabled;
        let ai_upscale_model_override = settings
            .ai_upscale_model_override
            .as_deref()
            .and_then(crate::ai::ModelKind::from_str);
        // 操作中はバックグラウンドインデクサを一時停止するためのゲート。
        // IndexerManager / name_index_supervisor の両方に `Arc` で共有される。
        let activity_gate = Arc::new(crate::activity_gate::ActivityGate::new(
            crate::activity_gate::DEFAULT_QUIET_MS,
        ));

        // 全文検索インデクサ (Tantivy) は起動の最大ボトルネック (実測 1 秒級) なので、
        // `App::update` の最初のフレームで `kick_off_startup_init` がバックグラウンド
        // スレッドに spawn する。それまでは None で、検索系 UI は無効として描画される。
        let indexer_manager: Option<crate::indexer_manager::IndexerManager> = None;

        // === 各 SQLite DB の open を計測 ===
        // 起動 UI スレッドで同期実行されるので、cold open が支配的なら
        // ここで個別の所要時間が判明する。--perf-log 無効時は no-op。
        let t = std::time::Instant::now();
        let archive_cache_db = crate::archive_cache::ArchiveCacheDb::open()
            .map_err(|e| crate::logger::log(format!("archive_cache_db open failed: {e}")))
            .ok()
            .map(Arc::new);
        crate::perf::emit_ms("startup", "db_open_archive_cache", 0, t);

        let t = std::time::Instant::now();
        let search_index_db = crate::search_index_db::SearchIndexDb::open()
            .map_err(|e| crate::logger::log(format!("search_index_db open failed: {e}")))
            .ok()
            .map(Arc::new);
        crate::perf::emit_ms("startup", "db_open_search_index", 0, t);

        let t = std::time::Instant::now();
        let rotation_db = crate::rotation_db::RotationDb::open().ok();
        crate::perf::emit_ms("startup", "db_open_rotation", 0, t);

        let t = std::time::Instant::now();
        let video_pin_db = crate::video_pins::VideoPinDb::open().ok();
        crate::perf::emit_ms("startup", "db_open_video_pins", 0, t);

        let t = std::time::Instant::now();
        let video_bookmark_db = crate::video_bookmarks::VideoBookmarkDb::open().ok();
        crate::perf::emit_ms("startup", "db_open_video_bookmarks", 0, t);

        let t = std::time::Instant::now();
        let video_tile_cache = crate::video::tile_thumb_cache::TileThumbCache::open()
            .ok()
            .map(std::sync::Arc::new);
        crate::perf::emit_ms("startup", "db_open_video_tile_cache", 0, t);

        let t = std::time::Instant::now();
        let rating_db = crate::rating_db::RatingDb::open().ok();
        crate::perf::emit_ms("startup", "db_open_rating", 0, t);

        let t = std::time::Instant::now();
        let spread_db = crate::spread_db::SpreadDb::open().ok();
        crate::perf::emit_ms("startup", "db_open_spread", 0, t);

        let t = std::time::Instant::now();
        let pdf_passwords = crate::pdf_passwords::PdfPasswordStore::load();
        crate::perf::emit_ms("startup", "pdf_passwords_load", 0, t);

        let t = std::time::Instant::now();
        let adjustment_db = crate::adjustment_db::AdjustmentDb::open().ok();
        crate::perf::emit_ms("startup", "db_open_adjustment", 0, t);

        let t = std::time::Instant::now();
        let mask_db = crate::mask_db::MaskDb::open().ok();
        crate::perf::emit_ms("startup", "db_open_mask", 0, t);

        Self {
            address: String::new(),
            current_folder: None,
            items: Vec::new(),
            thumbnails: Vec::new(),
            selected: None,
            settings,
            tx,
            rx,
            cancel_token: Arc::new(AtomicBool::new(false)),
            scroll_hint: Arc::new(AtomicUsize::new(0)),
            visible_end_shared: Arc::new(AtomicUsize::new(0)),
            scroll_offset_y: 0.0,
            last_cell_size: 200.0,
            last_cell_h: 200.0,
            last_viewport_h: 600.0,
            scroll_to_selected: false,
            last_outer_rect: None,
            last_inner_size: None,
            last_pixels_per_point: 1.0,
            pending_initial_size: None,
            cache_gen_total: 0,
            cache_gen_done: Arc::new(AtomicUsize::new(0)),
            image_metas: Vec::new(),
            reload_queue: None,
            heavy_io_queue: None,
            requested: std::collections::HashMap::new(),
            keep_range: (0, 0),
            keep_set: std::collections::HashSet::new(),
            keep_start_shared: Arc::new(AtomicUsize::new(0)),
            keep_end_shared: Arc::new(AtomicUsize::new(0)),
            texture_backlog: Vec::new(),
            folder_nav_pending: None,
            pending_folder_nav_steps: 0,
            pending_folder_nav_mode: FolderNavMode::Grid,
            progress_normal_peak: 0,
            progress_upgrade_peak: 0,
            selected_cell_rect: None,
            last_scroll_offset_y_tracked: 0.0,
            last_scroll_change_time: std::time::Instant::now(),
            display_px_shared: Arc::new(AtomicU32::new(512)),
            stats: Arc::new(Mutex::new(crate::stats::ThumbStats::new())),
            show_stats_dialog: false,
            fullscreen_idx: None,
            fs_cache: std::collections::HashMap::new(),
            fs_pending: std::collections::HashMap::new(),
            fs_upload_backlog: Vec::new(),
            fs_early_dims: std::collections::HashMap::new(),
            items_generation: 0,
            items_are_global_search_view: false,
            show_favorites_editor: false,
            favorites_index_size_cache: None,
            favorites_index_count_cache: None,
            favorites_index_refresh_rx: None,
            favorites_total_eta_cache: None,
            show_tag_editor: false,
            tag_editor_draft: Vec::new(),
            tag_write_handle: None,
            rating_write_handle: None,
            show_fav_add_dialog: false,
            fav_add_name_input: String::new(),
            fav_add_target: None,
            fav_add_auto_index_structure: false,
            fav_add_auto_index_metadata: false,
            fav_add_auto_index_thumbs: false,
            indexer_manager,
            name_index_supervisors: std::collections::HashMap::new(),
            activity_gate,
            global_search: crate::global_search_ui::GlobalSearchState::default(),
            show_open_folder_dialog: false,
            open_folder_input: String::new(),
            open_folder_error: None,
            show_preferences: false,
            pref_state: None,
            checked: std::collections::HashSet::new(),
            context_menu_idx: None,
            context_menu_pos: egui::Pos2::ZERO,
            fs_secondary_press_start: None,
            fs_middle_zoom_drag: None,
            fs_context_menu_idx: None,
            fs_context_menu_pos: egui::Pos2::ZERO,
            show_delete_confirm: false,
            delete_targets: Vec::new(),
            pending_reload: false,
            paste_pending: Vec::new(),
            select_after_load: None,
            video_thumb_overrides: std::collections::HashMap::new(),
            show_rotation_reset_confirm: false,
            show_cache_manager: false,
            cache_manager_days: 90,
            cache_manager_stats: None,
            cache_manager_result: None,
            cache_manager_confirm_delete_all: false,
            cache_maint_pending: None,
            archive_cache_maint_pending: None,
            archive_cache_db,
            archive_convert: None,
            archive_source_override: None,
            show_archive_cache_manager: false,
            archive_cache_rows: None,
            archive_cache_total_bytes: 0,
            archive_cache_selection: std::collections::HashSet::new(),
            archive_cache_manager_result: None,
            archive_cache_confirm_delete_all: false,
            last_selected_image_path: None,
            tq: ThumbQualityState {
                fs_divider: 0.5,
                a_size: 512,
                a_quality: 75,
                b_size: 512,
                b_quality: 85,
                ..Default::default()
            },
            cc: CacheCreatorState::default(),
            search_index_db,
            favsearch: FavSearchState::default(),
            show_metadata_panel: false,
            metadata_cache: std::collections::HashMap::new(),
            exif_cache: std::collections::HashMap::new(),
            xmp_cache: std::collections::HashMap::new(),
            tags_cache: std::collections::HashMap::new(),
            tag_toast_label: None,
            metadata_show_raw_prompt: false,
            metadata_show_raw_workflow: false,
            exif_sections_open: std::collections::HashMap::new(),
            address_has_focus: false,
            folder_history: std::collections::HashMap::new(),
            show_search_bar: false,
            search_query: String::new(),
            search_pending: None,
            favsearch_pending: None,
            metadata_pending: None,
            tag_prewarm_pending: None,
            tag_prewarm_queued: std::collections::HashSet::new(),
            delete_pending: None,
            search_filter: None,
            visible_indices: Vec::new(),
            pending_finalize: std::collections::HashSet::new(),
            last_vis_range: (0, 0),
            vis_settle_at: None,
            vis_first_logged: false,
            vis_all_logged: false,
            last_stuck_scan_at: std::time::Instant::now(),
            search_focus_request: false,
            search_has_focus: false,
            search_target: crate::fts_index::SearchTarget::All,
            search_or_mode: false,
            rotation_db,
            rotation_cache: std::collections::HashMap::new(),
            video_pin_db,
            video_bookmark_db,
            #[cfg(windows)]
            video_tile_state: None,
            #[cfg(windows)]
            video_tile_textures: std::collections::HashMap::new(),
            #[cfg(windows)]
            video_jump_textures: std::collections::HashMap::new(),
            #[cfg(windows)]
            video_tile_cache,
            rating_db,
            rating_cache: std::collections::HashMap::new(),
            rating_filter_suppressed_at: None,
            user_set_rating_keys: std::collections::HashSet::new(),
            current_folder_rating_cache: None,
            folder_rating_counts: std::collections::HashMap::new(),
            folder_rating_counts_loaded: false,
            folder_rating_counter_handle: None,
            folder_rating_counts_folder_key: None,
            search_drilled_folder_counts: std::collections::HashMap::new(),
            spread_db,
            spread_mode: crate::settings::SpreadMode::default(),
            spread_popup_open: false,
            slideshow_playing: false,
            slideshow_next_at: std::time::Instant::now(),
            fs_viewport_shown: false,
            fs_opened_at: None,
            fs_focus_grace_elapsed: false,
            fs_prev_focused: false,
            fs_focus_regained_at: None,
            fs_suppress_primary_until_release: false,
            fs_zoom: 1.0,
            fs_pan: egui::Vec2::ZERO,
            fs_pan_drag_start: None,
            fs_free_rotation: 0.0,
            fs_rotation_drag_start: None,
            fs_loupe_locked: false,
            fs_spread_layout: None,
            fs_transparent_bg_mode: 0,
            fs_checker_texture: None,
            post_filter_bypassed: false,
            fs_transparent_bg_indicator_until: None,
            analysis_mode: false,
            analysis_hover_color: None,
            analysis_pinned_color: None,
            analysis_grayscale: false,
            analysis_mosaic_grid: false,
            analysis_filter_mag: 0,
            analysis_guide_drag: None,
            analysis_zoom: 1.0,
            analysis_pan: egui::Vec2::ZERO,
            analysis_pan_drag_start: None,
            analysis_overlay_cache: None,
            analysis_hist_cache: None,
            analysis_sv_cache: None,
            initialized: false,
            applied_ui_theme: None,
            #[cfg(windows)]
            wgpu_render_state: None,
            #[cfg(windows)]
            gpu_video_device: None,
            pdf_passwords,
            show_pdf_password_dialog: false,
            pdf_password_input: String::new(),
            pdf_password_save: false,
            pdf_password_error: None,
            pdf_password_pending_path: None,
            pdf_password_pending_save: None,
            pdf_current_password: None,
            pdf_content_type_tx: pdf_ct_tx,
            pdf_content_type_rx: pdf_ct_rx,
            pdf_enumerate_pending: None,
            zip_enumerate_pending: None,
            fs_nav_after_pdf_enumerate: None,
            cached_handlers: None,
            cached_nav_indices: None,

            // AI (settings から復元)
            ai_runtime: None,
            ai_model_manager: std::sync::Arc::new(crate::ai::model_manager::ModelManager::new()),
            ai_upscale_enabled,
            ai_upscale_model_override,
            ai_denoise_model: None,
            ai_upscale_cache: std::collections::HashMap::new(),
            ai_upscale_pending: std::collections::HashMap::new(),
            ai_classify_cache: std::collections::HashMap::new(),
            show_about_dialog: false,
            ai_upscale_failed: std::collections::HashSet::new(),
            ai_status_done_at: None,

            // 画像補正
            adjustment_mode: false,
            adjust_spread_target: AdjustSpreadTarget::Left,
            adjustment_page_params: std::collections::HashMap::new(),
            adjustment_favorite_params: std::collections::HashMap::new(),
            mask_pages: std::collections::HashSet::new(),
            adjustment_cache: std::collections::HashMap::new(),
            thumb_pixels: std::collections::HashMap::new(),
            thumb_adjust_tex: std::collections::HashMap::new(),
            thumb_adjust_was_dragging: false,
            adjustment_dragging: false,
            adjustment_db,
            adjustment_sharpened: std::collections::HashSet::new(),
            slot_save_dialog: None,
            ime_composing: false,
            ime_last_event_at: None,
            fs_feedback_toast: None,
            trt_worker_notice: None,
            trt_auto_restart_attempts: 0,
            trt_restart_in_flight: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            trt_install_state: None,
            fs_boundary_hint: None,

            // 消しゴムモード
            erase_mode: false,
            erase_mask: None,
            erase_mask_size: [0, 0],
            erase_mask_texture: None,
            erase_last_paint_pos: None,
            erase_tool: EraseTool::default(),
            erase_brush_radius: 0.0, // enter_erase_mode で設定
            erase_lasso_points: Vec::new(),
            erase_line_start: None,
            erase_line_end: None,
            erase_line_tilt: 0.0,
            erase_line_width: 0.0, // enter_erase_mode で設定
            erase_shift_drag: None,
            erase_paint_mode: true,
            erase_base_cache: std::collections::HashMap::new(),
            mask_db,
            erase_undo_stack: std::collections::VecDeque::new(),
            meta_undo: crate::undo_stack::UndoStack::new(),
            adjustment_drag_session: None,
            pending_tag_undos: std::collections::HashMap::new(),
            next_tag_tx_id: 1,
            erase_last_undo_at: None,
            erase_vectors: Vec::new(),
            erase_selected_vector: None,
            erase_vector_drag: None,
            erase_inpaint_pending: std::collections::HashMap::new(),
            erase_spread_ctx: None,
            fs_nav_locked_gen: None,
            fs_holdover_tex: None,
            virtual_folder_writeback: None,
            pdf_prefetch_grace_until: None,
            catalog_cache: std::collections::HashMap::new(),
            catalog_cache_order: std::collections::VecDeque::new(),
            input_seq: 0,
            last_input_at: None,
            frame_counter: 0,
            perf_last_flush: None,
            fs_painted_last: None,
            video_seek_thumb_tex: None,
            video_resume_last_save: None,
            sidecars: std::collections::HashMap::new(),
            tray_controller: None,
            window_visible: true,
            show_tray_enabled_notice: false,
            update_check_pending: None,
            update_info: None,
            update_check_error: None,
            update_check_last_spawn: None,
            show_update_dialog: false,
            main_hwnd: None,
            placement_slot: None,
            activation_listener: None,
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            current_folder_last_mtime: None,
            current_folder_signature: None,
            last_main_focused: true,

            startup_progress: Arc::new(Mutex::new("起動中…".to_string())),
            startup_init: None,
            startup_done: false,
            housekeeping_armed: false,
        }
    }
}

/// `App::new_for_test` に渡すテスト設定。
///
/// Phase C では実プロセスの `data_dir::init` を経由せず、`set_test_override` で
/// `TempDir` を差し込む。App 内の全 DB/インデクサ open はその data_dir を参照する。
#[cfg(test)]
pub struct AppTestConfig {
    /// テスト用データディレクトリ。`data_dir::set_test_override(Some(...))` に設定済みの
    /// パスを渡す。(呼び出し側の `TempDir` が App より長生きする必要あり)
    pub data_dir: std::path::PathBuf,
    /// 起動時に `settings.json` をこの内容で上書きしてから App::default を呼ぶ。
    /// None なら `Settings::load` が空ファイルから default 設定を作る。
    pub settings: Option<crate::settings::Settings>,
}

#[cfg(test)]
impl App {
    /// テスト用コンストラクタ。本番の `App::default` と同じ DB/indexer open 経路を
    /// 通すが、以下が異なる:
    ///
    /// 1. `config.data_dir` を `data_dir::set_test_override` 経由で強制する前提 (呼び出し側で)
    /// 2. `config.settings` があれば `settings.json` に書き出してから load する
    /// 3. 名前索引 supervisor の初期 spawn は行わない
    ///    (呼び出し側が `spawn_initial_name_index_supervisors()` を明示的に呼ぶ)
    /// 4. 初期サイズ / font / theme は設定しない (テスト側で Context を用意する想定)
    ///
    /// 注意: Tantivy / SQLite / notify-rs などの実スレッドは通常どおり起動するので、
    /// テスト終了時には `drop(app)` で正しく停止すること (IndexerManager::drop が
    /// supervisor を signal_stop→join で止める)。
    pub fn new_for_test(config: AppTestConfig) -> Self {
        // settings.json をあらかじめ書いておく (App::default 内の Settings::load が拾う)
        if let Some(settings) = &config.settings {
            std::fs::create_dir_all(&config.data_dir).ok();
            let json = serde_json::to_string_pretty(settings).expect("serialize settings");
            std::fs::write(config.data_dir.join("settings.json"), json)
                .expect("write settings.json");
        }
        // data_dir::get() はこの時点で config.data_dir を返さなければならない
        debug_assert_eq!(
            crate::data_dir::get(),
            config.data_dir,
            "data_dir::set_test_override(Some(config.data_dir)) を先に呼ぶこと"
        );
        let app = App::default();
        // `spawn_initial_name_index_supervisors` はテスト側で必要なときだけ呼ぶ契約
        app
    }
}

impl App {
    /// 指定 idx の GridItem から perf 相関キーを生成する (範囲外なら None)。
    pub(crate) fn perf_item_key(&self, idx: usize) -> Option<String> {
        self.items.get(idx).map(|g| g.perf_key())
    }

    /// items と thumbnails を常にセットで push するヘルパー (docs §10.4.2)。
    ///
    /// 既存コードは `items.len() == thumbnails.len()` を前提にしている箇所が多く
    /// (virtual scrolling のセル描画、load_request 組み立て等)、Ctrl+G の streaming で
    /// items を途中拡張するときはこのヘルパー経由で両者の不変条件を保つこと。
    ///
    /// 戻り値は追加された item の idx。
    // Ctrl+G UI 実装 (後続コミット) で使用。現状は API 先行追加。
    #[allow(dead_code)]
    pub(crate) fn push_grid_item_pending(&mut self, item: GridItem) -> usize {
        let idx = self.items.len();
        self.items.push(item);
        self.thumbnails.push(ThumbnailState::Pending);
        idx
    }

    /// パフォーマンス計装用の input_seq を +1 してユーザー入力イベントを記録する。
    /// `--perf-log` 無効時はカウンタのみインクリメントし、イベント発火はしない。
    /// 戻り値は新しい seq 値 (ワーカーに渡すタスクに埋め込む用)。
    pub(crate) fn bump_input_seq(&mut self, kind: &str, key: Option<&str>) -> u64 {
        self.input_seq = self.input_seq.wrapping_add(1);
        if self.input_seq == 0 {
            // 0 は "未設定" として予約しているのでスキップ
            self.input_seq = 1;
        }
        self.last_input_at = Some(std::time::Instant::now());
        if crate::perf::is_enabled() {
            crate::perf::event("input", kind, key, self.input_seq, &[]);
        }
        self.input_seq
    }

    /// アイテム idx 上のユーザー入力イベントを記録する `bump_input_seq` の薄いラッパ。
    /// perf 無効時は `perf_item_key` の String 生成を省く。
    pub(crate) fn bump_input_seq_for_item(&mut self, kind: &str, idx: usize) -> u64 {
        let key = crate::perf::is_enabled()
            .then(|| self.perf_item_key(idx))
            .flatten();
        self.bump_input_seq(kind, key.as_deref())
    }

    /// IME 変換状態を更新する (毎フレーム先頭で呼ぶ)。
    /// `ime_input_active_this_frame` に「今フレームを IME 入力として扱うか」を設定する。
    ///
    /// 判定は以下の 3 条件の OR:
    /// 1. 前フレーム末で composition 状態だった (`was_composing`)
    /// 2. 今フレームに Ime イベントが来ている (`had_ime_event`)
    /// 3. 直近 300ms 以内に Ime イベントがあった (時間ベースの余韻)
    ///
    /// 3 が必要な理由: Windows の一部環境では、IME キャンセル時 (Escape) に
    /// Ime イベントが先行フレームで発行されて `ime_composing = false` になり、
    /// Key::Escape 自体は 1〜数フレーム遅れで届くことがある。
    /// その隙間を埋めるためのガード。
    /// 現在のビューポートの Ime イベントを処理して `ime_composing` / `ime_last_event_at` を更新する。
    ///
    /// **重要**: egui の各ビューポートは独立したイベントキューを持つ。
    /// `show_viewport_immediate` で別ビューポートを出している場合は、その closure の
    /// 先頭でも呼ばないと、そのビューポート内の IME を取り逃がす。
    pub(crate) fn update_ime_state(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            for event in &i.events {
                if let egui::Event::Ime(ime) = event {
                    self.ime_last_event_at = Some(std::time::Instant::now());
                    match ime {
                        egui::ImeEvent::Enabled => self.ime_composing = true,
                        egui::ImeEvent::Preedit(s) => self.ime_composing = !s.is_empty(),
                        egui::ImeEvent::Commit(_) => self.ime_composing = false,
                        egui::ImeEvent::Disabled => self.ime_composing = false,
                    }
                }
            }
        });
    }

    /// IME 変換中か (または直近 300ms 以内に Ime イベントがあったか)。
    /// true の間は Enter / Escape をショートカット・ダイアログ確定/キャンセルとして拾ってはいけない。
    /// 300ms グレースは Windows IME で `Ime::Disabled` と `Key::Escape` が別フレームに
    /// 届くケースを吸収するため。
    pub(crate) fn ime_input_active(&self) -> bool {
        if self.ime_composing {
            return true;
        }
        self.ime_last_event_at
            .map(|t| t.elapsed() < std::time::Duration::from_millis(300))
            .unwrap_or(false)
    }

    /// ダイアログ確定用の Enter が押されたか。IME 変換中は常に false を返す。
    pub(crate) fn dialog_enter_pressed(&self, ctx: &egui::Context) -> bool {
        !self.ime_input_active() && ctx.input(|i| i.key_pressed(egui::Key::Enter))
    }

    /// ダイアログキャンセル用の Escape が押されたか。IME 変換中は常に false を返す。
    pub(crate) fn dialog_escape_pressed(&self, ctx: &egui::Context) -> bool {
        !self.ime_input_active() && ctx.input(|i| i.key_pressed(egui::Key::Escape))
    }

    /// いずれかのモーダルダイアログが開いているか。
    /// true の場合、キーボードショートカットやスクロールを無効化する。
    pub(crate) fn any_dialog_open(&self) -> bool {
        self.show_stats_dialog
            || self.show_favorites_editor
            || self.show_tag_editor
            || self.show_fav_add_dialog
            || self.show_open_folder_dialog
            || self.show_preferences
            || self.show_cache_manager
            || self.show_delete_confirm
            || self.show_rotation_reset_confirm
            || self.show_pdf_password_dialog
            || self.show_about_dialog
            || self.show_update_dialog
            || self.slot_save_dialog.is_some()
            || self.context_menu_idx.is_some()
            || self.delete_pending.is_some()
    }

    /// ユーザー視点でのカレントフォルダ。変換済みアーカイブを開いているときは
    /// 元 (7z/LZH) のパスを返す。通常時は `current_folder` と同じ。
    /// BS / Ctrl+↑↓ / タイトルバー / アドレスバー表示で使うこと。
    pub(crate) fn effective_folder(&self) -> Option<PathBuf> {
        self.archive_source_override
            .clone()
            .or_else(|| self.current_folder.clone())
    }

    /// 現在のフォルダを再読み込みする。変換済みアーカイブ閲覧中 (キャッシュ ZIP を
    /// `current_folder` に、元 7z/LZH を `archive_source_override` に持っている状態)
    /// は再読み込み後も override/address を元 7z/LZH に戻し、UI コンテキストを維持する。
    ///
    /// 用途: 環境設定 OK 押下時の同名ファイル設定変更・Susie 設定変更など、再読み込みは
    /// 必要だが元アーカイブの文脈を保ちたいケース。
    /// トレイ復帰 / フォーカス復帰時に呼び、現在表示中のフォルダが外部 (ComfyUI 等) で
    /// 変化していれば再ロードする。選択中アイテムはパスで追跡して復元し、再ロード後に
    /// ビューポート外にはみ出していれば次フレームで可視範囲に収める。
    ///
    /// スキップ条件:
    /// - `current_folder` が None
    /// - Ctrl+G 検索中 (別ビューで動いている)
    /// - ZIP / PDF / 検索合成パスなどディレクトリ実体でない (= `current_folder_last_mtime`
    ///   が None のまま)
    /// - mtime が保存値と同じ (= 外部更新なし)
    /// - mtime は変わったが、再走査した内容シグネチャ (path+mtime+size) が一致する
    ///   (Windows Search / ウイルススキャン等が触っただけ → ちらつき抑止)
    ///
    /// 再ロードは `load_folder_with_scan` に走らせた `scan` を渡して再 read_dir を避ける。
    /// UI スレッドでの syscall は `metadata()` + 1 回の `read_dir` のみ。
    pub(crate) fn check_external_folder_changes(&mut self) {
        if self.global_search.active {
            return;
        }
        // 削除進行中はフォーカス復帰による自動再読み込みを抑止する。削除自身が
        // フォルダ mtime を更新するので素通しすると load_folder が走り、items が
        // 差し替わって poll_delete_pending が generation 不一致で結果適用をスキップ、
        // 加えて未完了ファイルに対するサムネ/AI 先読みが発生して「読み込み失敗」が
        // 出る。削除完了ダイアログが閉じてから次のフレームで再検知される。
        if self.delete_pending.is_some() {
            return;
        }
        let Some(folder) = self.current_folder.clone() else {
            return;
        };
        let Some(prev_mtime) = self.current_folder_last_mtime else {
            return;
        };
        let Ok(meta) = folder.metadata() else {
            return;
        };
        if !meta.is_dir() {
            return;
        }
        let Ok(new_mtime) = meta.modified() else {
            return;
        };
        if new_mtime == prev_mtime {
            return;
        }
        // mtime が変わっていても、フォルダ内容 (paths + mtimes + sizes) が同一なら
        // items 差し替えをスキップして画面ちらつきを防ぐ。Windows Search や
        // ウイルススキャン等が触っただけで mtime が更新されるケースを救済する。
        let scan = scan_directory(&folder);
        let new_sig = signature_from_scan(&scan);
        if self.current_folder_signature == Some(new_sig) {
            self.current_folder_last_mtime = Some(new_mtime);
            return;
        }
        // 選択中アイテムのパスを保存 (非選択 / パス取れないアイテムは None)。
        let selected_path: Option<PathBuf> = self
            .selected
            .and_then(|idx| self.items.get(idx))
            .and_then(|item| match item {
                GridItem::Folder(p)
                | GridItem::Image(p)
                | GridItem::Video(p)
                | GridItem::ZipFile(p)
                | GridItem::PdfFile(p) => Some(p.clone()),
                GridItem::ConvertibleArchive { path, .. } => Some(path.clone()),
                _ => None,
            });
        crate::logger::log(format!(
            "auto-refresh: folder content changed ({}), reloading",
            folder.display()
        ));
        // 既に走らせた scan を pre_scan として渡し、UI スレッドで再 read_dir しない。
        self.load_folder_with_scan(folder, Some(scan));
        // 再ロード後に選択パスを探し、見つかればそこにカーソルを戻してスクロール依頼。
        // 見つからない (消えた) / そもそも未選択ならスクロール位置は触らない。
        if let Some(path) = selected_path {
            let new_idx = self
                .items
                .iter()
                .position(|it| matches!(it,
                    GridItem::Folder(p)
                    | GridItem::Image(p)
                    | GridItem::Video(p)
                    | GridItem::ZipFile(p)
                    | GridItem::PdfFile(p) if p == &path)
                    || matches!(it, GridItem::ConvertibleArchive { path: p, .. } if p == &path));
            if let Some(idx) = new_idx {
                self.selected = Some(idx);
                self.scroll_to_selected = true;
            }
        }
    }

    pub(crate) fn reload_current_folder_preserving_override(&mut self) {
        let Some(folder) = self.current_folder.clone() else {
            return;
        };
        let saved_override = self.archive_source_override.clone();
        self.load_folder(folder);
        if let Some(src) = saved_override {
            self.address = src.to_string_lossy().to_string();
            self.archive_source_override = Some(src);
        }
    }

    /// 通常の (事前スキャンなしの) フォルダロード。`scan_directory` を UI スレッドで
    /// 同期実行する。初期化 / 履歴復元 / 直接パス指定など、Ctrl+↑↓ 連打以外の
    /// すべての呼び出しが通る。
    pub fn load_folder(&mut self, path: PathBuf) {
        self.load_folder_with_scan(path, None);
    }

    /// 事前スキャン済みディレクトリを受け取れる load_folder の本体。
    /// Ctrl+↑↓ の DFS スレッドが `scan_directory` を済ませている場合、
    /// `pre_scan=Some(...)` を渡すことで UI スレッドの read_dir (= 最大 179ms)
    /// をスキップできる。`path` が ZIP/PDF ファイルのときは仮想フォルダとして
    /// 別ルートに入るため `pre_scan` は無視される (None 相当で委譲)。
    pub fn load_folder_with_scan(&mut self, path: PathBuf, pre_scan: Option<ScannedDir>) {
        // perf: UI スレッドをブロックする load_folder 全体の wall time を計測する。
        // Ctrl+↑↓ 連打時の引っかかりの主要因がここに集まる想定。
        let lf_t0 = std::time::Instant::now();
        let lf_seq = self.input_seq;
        let pre_scanned = pre_scan.is_some();
        // path.display().to_string() のアロケーションを perf-log 有効時に限定する
        // (通常起動では空文字のまま放置する)。
        let lf_path_disp: String = if crate::perf::is_enabled() {
            path.display().to_string()
        } else {
            String::new()
        };
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "load_folder_begin",
                None,
                lf_seq,
                &[
                    ("path", serde_json::Value::from(lf_path_disp.clone())),
                    ("pre_scanned", serde_json::Value::from(pre_scanned)),
                ],
            );
        }
        // パスが .zip / .pdf ファイルなら仮想フォルダとして開く
        if path.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default();
            if ext == "zip" {
                self.load_zip_as_folder(path);
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "nav",
                        "load_folder_end",
                        None,
                        lf_seq,
                        &[
                            (
                                "ms",
                                serde_json::Value::from(lf_t0.elapsed().as_secs_f64() * 1000.0),
                            ),
                            ("kind", serde_json::Value::from("zip")),
                            ("path", serde_json::Value::from(lf_path_disp)),
                        ],
                    );
                }
                return;
            }
            if ext == "pdf" {
                self.load_pdf_as_folder(path);
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "nav",
                        "load_folder_end",
                        None,
                        lf_seq,
                        &[
                            (
                                "ms",
                                serde_json::Value::from(lf_t0.elapsed().as_secs_f64() * 1000.0),
                            ),
                            ("kind", serde_json::Value::from("pdf")),
                            ("path", serde_json::Value::from(lf_path_disp)),
                        ],
                    );
                }
                return;
            }
        }

        crate::logger::log(format!("=== load_folder: {} ===", path.display()));

        // フォルダ移動でメタ操作の Undo/Redo スタックを破棄する (移動先で過去の
        // レーティング操作を巻き戻すと UX が混乱するため)。
        self.clear_meta_undo();

        // 外側の ZIP/PDF/フォルダを切り替えたので、ネスト ZIP バイト列キャッシュを破棄する。
        // これで古い外側アーカイブのバイト列が RAM に居残るのを防ぐ。
        crate::zip_loader::clear_nested_cache();

        // ── ディレクトリ走査（画像はメタデータも収集）────────────────
        // pre_scan が与えられていれば DFS スレッドで既に走査済み (UI 非ブロック)。
        // 無ければ UI スレッドで scan_directory を呼ぶ (従来挙動)。
        let scan_t0 = std::time::Instant::now();
        let scan = pre_scan.unwrap_or_else(|| scan_directory(&path));
        // フォーカス復帰時の差分判定用シグネチャ。scan を消費する前に計算しておき、
        // `start_loading_items` に引数として渡す。
        let folder_signature = signature_from_scan(&scan);
        let (mut folders, mut folder_metas): (Vec<GridItem>, Vec<Option<(i64, i64)>>) =
            scan.folders.into_iter().unzip();
        let mut all_media = scan.all_media;

        let scan_ms = scan_t0.elapsed().as_secs_f64() * 1000.0;
        let scan_folders = folders.len();
        let scan_media = all_media.len();
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "lf_scan",
                None,
                lf_seq,
                &[
                    ("ms", serde_json::Value::from(scan_ms)),
                    ("folders", serde_json::Value::from(scan_folders)),
                    ("media", serde_json::Value::from(scan_media)),
                    ("pre_scanned", serde_json::Value::from(pre_scanned)),
                ],
            );
        }

        let sort_t0 = std::time::Instant::now();
        let sort = self.settings.sort_order;
        // folders (Folder / ZipFile / PdfFile / ConvertibleArchive) も sort_order に
        // 従って並べる (Explorer / Finder と同じ慣習で 2 段構成は維持)。
        crate::grid_item::sort_folder_block(&mut folders, &mut folder_metas, sort);
        all_media.sort_by(|(a, _, a_mt, _), (b, _, b_mt, _)| {
            let an = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let bn = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
            sort.compare(an, *a_mt, bn, *b_mt, natural_sort_key)
        });
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "lf_sort",
                None,
                lf_seq,
                &[(
                    "ms",
                    serde_json::Value::from(sort_t0.elapsed().as_secs_f64() * 1000.0),
                )],
            );
        }

        // ── 同名ファイルフィルタ ─────────────────────────────────────
        let dup_t0 = std::time::Instant::now();
        self.video_thumb_overrides.clear();
        self.apply_duplicate_filters(&mut folders, &mut folder_metas, &mut all_media);
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "lf_dup_filter",
                None,
                lf_seq,
                &[(
                    "ms",
                    serde_json::Value::from(dup_t0.elapsed().as_secs_f64() * 1000.0),
                )],
            );
        }

        // items: フォルダ先頭 → メディア（画像・動画を名前順混在）
        let folder_count = folders.len();
        let mut items: Vec<GridItem> = folders;
        let mut image_metas: Vec<Option<(i64, i64)>> = folder_metas;
        let mut video_items: Vec<(usize, PathBuf, u64)> = Vec::new();

        for (offset, (p, is_video, mtime, file_size)) in all_media.iter().enumerate() {
            let item_idx = folder_count + offset;
            if *is_video {
                items.push(GridItem::Video(p.clone()));
                image_metas.push(None);
                video_items.push((item_idx, p.clone(), (*file_size).max(0) as u64));
            } else {
                items.push(GridItem::Image(p.clone()));
                image_metas.push(Some((*mtime, *file_size)));
            }
        }

        // 画像ファイル名集合 (カタログ掃除用キー)
        let existing_keys: std::collections::HashSet<String> = items
            .iter()
            .filter_map(|it| match it {
                GridItem::Image(p) => p.file_name()?.to_str().map(String::from),
                GridItem::ZipFile(p) => {
                    let fname = p.file_name()?.to_str()?;
                    Some(format!("{}{fname}", CACHE_KEY_ZIP))
                }
                GridItem::PdfFile(p) => {
                    let fname = p.file_name()?.to_str()?;
                    Some(format!("{}{fname}", CACHE_KEY_PDF))
                }
                GridItem::Folder(p) => {
                    let fname = p.file_name()?.to_str()?;
                    Some(format!("{}{fname}", CACHE_KEY_FOLDER))
                }
                _ => None,
            })
            .collect();

        // 訪問時自動索引化は廃止。「名前」フル索引化 ON のお気に入りのみ
        // name_bulk_indexer 経由で全走査する (検索結果が閲覧履歴に依らないように)。

        let items_len = items.len();
        self.start_loading_items(
            path,
            items,
            image_metas,
            existing_keys,
            video_items,
            Some(folder_signature),
        );

        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "load_folder_end",
                None,
                lf_seq,
                &[
                    (
                        "ms",
                        serde_json::Value::from(lf_t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("kind", serde_json::Value::from("folder")),
                    ("path", serde_json::Value::from(lf_path_disp)),
                    ("items", serde_json::Value::from(items_len)),
                ],
            );
        }
    }

    /// 名前索引フラグ (`auto_index_structure`) の OFF→ON / ON→OFF 遷移を即時反映する。
    /// 呼び出し側はすでに `settings.favorites[*].auto_index_structure` を更新した後に呼ぶ。
    ///
    /// - false → true: 既存 supervisor があれば先に drop、新規 supervisor を spawn。
    ///   supervisor が初期バルクを走らせ、その後 notify-rs で差分追従する。
    /// - true → false: supervisor を drop し、`search_index_db` をクリア。
    ///   **順序重要**: supervisor の drop (cancel + join) を先に完了させないと、
    ///   in-flight upsert が clear_for_favorite 後に走って索引を復活させる race が
    ///   発生する。
    pub(crate) fn apply_favorite_name_index_change(
        &mut self,
        fav_id: uuid::Uuid,
        fav_path: &std::path::Path,
        new_on: bool,
    ) {
        // 既存 supervisor があれば drop (OFF 遷移だけでなく ON→ON でも念のため:
        // path 変更等で spawn し直すシナリオ)。
        // `drop(handle)` は `thread.join()` を待つため、bulk scan 進行中は UI が
        // 数百 ms ブロックする。signal_stop で cancel は立ててから、実際の join は
        // バックグラウンドスレッドに逃がす。spawn 失敗時は closure が現スレッドで drop
        // されるので同期 join にフォールバックする (UI ブロックするが整合性は保たれる)。
        if let Some(handle) = self.name_index_supervisors.remove(&fav_id) {
            handle.signal_stop();
            if let Err(e) = std::thread::Builder::new()
                .name(format!("name-index-joiner-{}", fav_id.as_simple()))
                .spawn(move || drop(handle))
            {
                crate::logger::log(format!(
                    "name-index-joiner spawn failed, sync join instead: {e}"
                ));
            }
        }

        let Some(db) = self.search_index_db.as_ref() else {
            return;
        };
        if new_on {
            crate::logger::log(format!(
                "favorites: spawning name index supervisor for {}",
                fav_path.display()
            ));
            let handle = crate::name_index_supervisor::spawn(
                fav_id,
                fav_path.to_path_buf(),
                Arc::clone(db),
                Some(Arc::clone(&self.activity_gate)),
            );
            self.name_index_supervisors.insert(fav_id, handle);
        } else if let Err(e) = db.clear_for_favorite(fav_path) {
            crate::logger::log(format!(
                "favorites: clear name index for {} failed: {e}",
                fav_path.display()
            ));
        }
    }

    /// 起動時 IndexerManager 初期化をバックグラウンドスレッドで開始する。
    /// 1 度しか走らせない。
    pub(crate) fn kick_off_startup_init(&mut self) {
        if self.startup_init.is_some() || self.startup_done {
            return;
        }
        let favorites = self.settings.favorites.clone();
        let speed = self.settings.indexer_speed_profile;
        let activity_gate = Arc::clone(&self.activity_gate);
        let progress = Arc::clone(&self.startup_progress);
        let (tx, rx) = mpsc::channel();
        let started_at = std::time::Instant::now();
        let hook = make_progress_hook(Arc::clone(&progress));
        let spawn_result = std::thread::Builder::new()
            .name("startup-init".to_string())
            .spawn({
                let hook = Arc::clone(&hook);
                move || {
                    let t = std::time::Instant::now();
                    let mgr = crate::indexer_manager::IndexerManager::new(
                        &favorites,
                        speed,
                        activity_gate,
                        Some(hook),
                    );
                    crate::perf::emit_ms("startup", "indexer_manager_new", 0, t);
                    let _ = tx.send(mgr);
                }
            });
        if let Err(e) = spawn_result {
            // フォールバック: スレッド起動に失敗したら同期で実行する
            // (スレッド数制限の極端な環境向け保険)。
            crate::logger::log(format!("startup-init spawn failed: {e} — running synchronously"));
            self.indexer_manager = crate::indexer_manager::IndexerManager::new(
                &self.settings.favorites,
                self.settings.indexer_speed_profile,
                Arc::clone(&self.activity_gate),
                Some(hook),
            );
            self.startup_done = true;
            self.housekeeping_armed = true;
            return;
        }
        self.startup_init = Some(StartupInitPending { rx, started_at });
    }

    /// 起動 init の完了を確認する。完了していれば `indexer_manager` に注入し
    /// `startup_init = None`、`startup_done = true` に遷移する。
    /// `housekeeping_armed=true` の間、毎フレーム supervisor の idle 状態を見て、
    /// 全 supervisor が初回 scan を完了して idle になったら一度だけ spawn する。
    /// これにより VACUUM が初回 ingest と Mutex を取り合わないようにする。
    pub(crate) fn poll_housekeeping_arm(&mut self) {
        if !self.housekeeping_armed {
            return;
        }
        let Some(mgr) = self.indexer_manager.as_ref() else {
            // indexer 初期化失敗時はそもそも housekeeping も走らせない
            self.housekeeping_armed = false;
            return;
        };
        if mgr.all_supervisors_idle() {
            mgr.spawn_housekeeping(&crate::data_dir::get());
            self.housekeeping_armed = false;
        }
    }

    pub(crate) fn poll_startup_init(&mut self) {
        let Some(pending) = &self.startup_init else {
            return;
        };
        match pending.try_recv() {
            Ok(mgr) => {
                crate::logger::log(format!(
                    "startup: IndexerManager init completed in {:.0} ms",
                    pending.elapsed_ms()
                ));
                self.indexer_manager = mgr;
                self.startup_init = None;
                self.startup_done = true;
                self.housekeeping_armed = true;
                if let Ok(mut p) = self.startup_progress.lock() {
                    *p = "起動完了".to_string();
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                // bg スレッドが panic 等で落ちた: 検索機能なしで継続させる。
                crate::logger::log("startup: init thread disconnected unexpectedly");
                self.indexer_manager = None;
                self.startup_init = None;
                self.startup_done = true;
            }
        }
    }

    /// auto / manual の更新チェックを spawn する。
    ///
    /// - `manual=true`: ユーザー操作 (環境設定の「今すぐ確認」)。`update_check_enabled`
    ///   を無視して常に実行、結果ダイアログも常に開く
    /// - `manual=false`: 起動 5 秒後 + 24 時間ごとの auto。`update_check_enabled=false`
    ///   なら no-op、結果は新バージョン検知時のみバッジで通知
    ///
    /// 既に spawn 中 (rx が残っている) なら多重起動を防ぐため no-op。
    pub(crate) fn kick_update_check(&mut self, manual: bool) {
        if self.update_check_pending.is_some() {
            return;
        }
        if !manual && !self.settings.update_check_enabled {
            return;
        }
        self.update_check_last_spawn = Some(std::time::Instant::now());
        match crate::update_check::spawn_check(env!("CARGO_PKG_VERSION")) {
            Ok(rx) => {
                self.update_check_pending = Some(UpdateCheckPending { rx, manual });
            }
            Err(e) => {
                // スレッド起動自体の失敗。auto なら silent fail (ログのみ)、
                // manual ならユーザーが押した結果なのでエラーダイアログを出す。
                crate::logger::log(format!("update_check: spawn failed: {e}"));
                if manual {
                    self.update_check_error = Some(e);
                    self.show_update_dialog = true;
                }
            }
        }
    }

    /// 起動完了後の初回 + 24 時間ごとの auto kick。`App::update` の毎フレーム冒頭で呼ばれる。
    pub(crate) fn maybe_auto_update_check(&mut self) {
        if !self.settings.update_check_enabled {
            return;
        }
        if !self.startup_done {
            return; // 起動中はネットワークを走らせない
        }
        const PERIODIC: std::time::Duration = std::time::Duration::from_secs(24 * 3600);
        let due = match self.update_check_last_spawn {
            None => true, // 起動完了直後の初回
            Some(t) => t.elapsed() >= PERIODIC,
        };
        if due {
            self.kick_update_check(false);
        }
    }

    /// `update_check_pending` を非ブロッキング poll する。結果が来ていれば
    /// `update_info` に反映し、manual のときは結果ダイアログを開く。auto かつ
    /// 「新バージョンあり」のときはバッジで気付かせるだけ (ダイアログは開かない)。
    pub(crate) fn poll_update_check(&mut self) {
        let Some(pending) = self.update_check_pending.as_ref() else {
            return;
        };
        let result = match pending.rx.try_recv() {
            Ok(r) => Some(r),
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                // worker が送信前に panic した等の異常終了。manual ならユーザー操作が
                // 黙って終わらないようエラーダイアログを出す (auto は silent + log)。
                let was_manual = pending.manual;
                crate::logger::log(
                    "update_check: worker channel disconnected before sending result",
                );
                self.update_check_pending = None;
                if was_manual {
                    self.update_check_error =
                        Some("更新チェックの内部エラー (スレッド異常終了)".to_string());
                    self.show_update_dialog = true;
                }
                return;
            }
        };
        let pending = self.update_check_pending.take().unwrap();
        match result.unwrap() {
            Ok(info) => {
                let is_newer = info.is_newer;
                let tag = info.latest_tag.clone();
                // auto かつ更新確認 OFF の場合: spawn と結果到着の間で OFF に切り替わった
                // ケース。バッジを出さない契約を守るため、結果は破棄する (manual は常に
                // 結果を保持してダイアログで見せる)。
                if !pending.manual && !self.settings.update_check_enabled {
                    crate::logger::log(format!(
                        "update_check: dropping auto result ({tag}); user disabled checks"
                    ));
                    return;
                }
                self.update_info = Some(info);
                self.update_check_error = None;
                if pending.manual {
                    self.show_update_dialog = true;
                } else if is_newer {
                    crate::logger::log(format!("update_check: newer version found ({tag})"));
                }
            }
            Err(e) => {
                crate::logger::log(format!("update_check: failed: {e}"));
                if pending.manual {
                    // 既知の update_info はクリアしない (バッジを消さない)。
                    // 失敗ダイアログでは update_check_error を見て分岐する。
                    self.update_check_error = Some(e);
                    self.show_update_dialog = true;
                }
            }
        }
    }

    /// 通知バッジを表示すべきか (新バージョンあり、かつユーザーが skip していない)。
    /// `update_check_enabled=false` の間は結果を持っていてもバッジは出さない (OFF=
    /// 「通知も止める」と読める方が自然なため)。
    pub(crate) fn should_show_update_badge(&self) -> bool {
        if !self.settings.update_check_enabled {
            return false;
        }
        let Some(ref info) = self.update_info else {
            return false;
        };
        if !info.is_newer {
            return false;
        }
        match &self.settings.update_check_dismissed_version {
            Some(d) => d != &info.latest_tag,
            None => true,
        }
    }

    pub(crate) fn render_startup_overlay(&self, ctx: &egui::Context) {
        let progress_text = self
            .startup_progress
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| "起動中…".to_string());
        // 背景全面に半透明の幕を引いてから中央に文字を出す。
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(ctx.style().visuals.panel_fill))
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::ZERO,
                    ctx.style().visuals.panel_fill,
                );
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(rect)
                        .layout(egui::Layout::centered_and_justified(
                            egui::Direction::TopDown,
                        )),
                    |ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(rect.height() * 0.3);
                            ui.spinner();
                            ui.add_space(12.0);
                            ui.label(
                                egui::RichText::new("起動中…")
                                    .size(20.0)
                                    .strong(),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new(progress_text)
                                    .size(14.0)
                                    .color(ui.visuals().weak_text_color()),
                            );
                        });
                    },
                );
            });
    }

    /// 起動中の入力イベントを消費する (× ボタン以外)。Loading 終了後に
    /// 残ったキー入力が再生されてしまうのを防ぐため、events を空にする。
    /// `ViewportEvent::Close` だけは eframe 側で別経路を通るので影響しない。
    pub(crate) fn consume_input_during_startup(&self, ctx: &egui::Context) {
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                // window 関連 (フォーカス・サイズ変更等) は通したい。Key/Mouse/Text のみ捨てる。
                !matches!(
                    e,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::PointerMoved(_)
                        | egui::Event::MouseWheel { .. }
                        | egui::Event::Touch { .. }
                        | egui::Event::Ime(_)
                )
            });
        });
    }

    /// 起動時に `auto_index_structure = true` のお気に入りごとに name index supervisor を
    /// spawn する。`indexer_manager.sync_with_favorites` のメタ側の挙動に対応する。
    /// 呼び出しは `App` 構築後 (settings が load 済みで search_index_db が開いている状態) に
    /// 1 回だけ。
    pub(crate) fn spawn_initial_name_index_supervisors(&mut self) {
        let Some(db) = self.search_index_db.as_ref().cloned() else {
            return;
        };
        for fav in &self.settings.favorites {
            if !fav.auto_index_structure {
                continue;
            }
            if self.name_index_supervisors.contains_key(&fav.id) {
                continue;
            }
            crate::logger::log(format!(
                "startup: spawning name index supervisor for {}",
                fav.path.display()
            ));
            let handle = crate::name_index_supervisor::spawn(
                fav.id,
                fav.path.clone(),
                Arc::clone(&db),
                Some(Arc::clone(&self.activity_gate)),
            );
            self.name_index_supervisors.insert(fav.id, handle);
        }
    }

    /// タイトルバーの「(インデックス更新中)」表示用。
    /// 名前索引 / メタ索引のいずれかが `in_full_scan=true` を返しているなら true。
    /// notify-rs の watcher で待機中 (監視中) は false。
    pub(crate) fn any_indexer_in_full_scan(&self) -> bool {
        // 名前索引
        for h in self.name_index_supervisors.values() {
            if h.snapshot_stats().in_full_scan {
                return true;
            }
        }
        // メタ索引
        if let Some(mgr) = self.indexer_manager.as_ref() {
            for v in mgr.all_stats() {
                if v.stats.in_full_scan {
                    return true;
                }
            }
        }
        false
    }

    // name_index_supervisors は HashMap の Drop で各 handle が個別に cancel + join
    // される。名前索引は SQLite ベースで Tantivy writer のような共有リソースが
    // 無いため、1 体ずつ drop してもデッドロックしない (メタ側の writer 共有とは違う)。

    /// メタデータ索引フラグ (`auto_index_metadata`) の OFF→ON / ON→OFF 遷移を即時反映する。
    /// 呼び出し側はすでに `settings.favorites[*].auto_index_metadata` を更新した後に呼ぶ。
    ///
    /// - false → true: 呼び出し側で `sync_with_favorites` を呼べば supervisor が spawn される
    /// - true → false: 当 favorite の fts_meta 行を tombstone 化 → `sync_with_favorites` で
    ///   supervisor を停止
    pub(crate) fn apply_favorite_meta_index_change(&mut self, fav_id: uuid::Uuid, new_on: bool) {
        if !new_on {
            if let Some(mgr) = self.indexer_manager.as_ref() {
                mgr.purge_favorite_metadata(fav_id);
            }
        }
        // spawn/stop は sync_with_favorites 側
        if let Some(mgr) = self.indexer_manager.as_mut() {
            mgr.sync_with_favorites(&self.settings.favorites);
        }
    }

    /// お気に入り検索バーを開く (メニューや Ctrl+S から呼ばれる)。
    /// 他の検索バー (Ctrl+F / Ctrl+G) が開いていれば閉じて相互排他を保つ。
    pub(crate) fn open_favsearch(&mut self) {
        self.close_other_search_bars(SearchMode::Favsearch);
        self.favsearch.active = true;
        self.favsearch.focus_request = true;
        self.favsearch.nav_stack.clear();
        self.favsearch.results_paths.clear();
        // 検索モードに入る際、現在のフォルダを保存して戻れるようにする
        if self.favsearch.saved_folder.is_none() {
            self.favsearch.saved_folder = self.current_folder.clone();
        }
    }

    /// Ctrl+F のローカルメタデータ検索バーを開く。
    /// 他の検索バーが開いていれば閉じる (相互排他)。
    pub(crate) fn open_local_metadata_search(&mut self) {
        self.close_other_search_bars(SearchMode::LocalMeta);
        self.show_search_bar = true;
        self.search_focus_request = true;
    }

    /// 指定した検索モード以外の検索バー 3 種をすべて閉じる。
    /// Ctrl+F / Ctrl+S / Ctrl+G はユーザー操作がややこしくなるため同時には 1 つだけ
    /// アクティブにする方針 (2026-04 ユーザー指摘)。
    pub(crate) fn close_other_search_bars(&mut self, keep: SearchMode) {
        if !matches!(keep, SearchMode::LocalMeta) && self.show_search_bar {
            self.show_search_bar = false;
            self.search_query.clear();
            self.search_filter = None;
            self.search_has_focus = false;
            self.cancel_search_pending();
            self.rebuild_visible_indices();
        }
        if !matches!(keep, SearchMode::Favsearch) && self.favsearch.active {
            self.close_favsearch();
        }
        if !matches!(keep, SearchMode::Global) && self.global_search.active {
            self.close_global_search();
        }
    }

    /// お気に入り検索バーを閉じて、元のフォルダに戻る。
    pub(crate) fn close_favsearch(&mut self) {
        self.favsearch.active = false;
        self.favsearch.has_focus = false;
        self.favsearch.query.clear();
        self.favsearch.last_executed.clear();
        self.favsearch.nav_stack.clear();
        self.favsearch.results_paths.clear();
        if let Some(pending) = self.favsearch_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }

        // 検索モードで保存していた元フォルダがあれば戻す
        if let Some(saved) = self.favsearch.saved_folder.take() {
            self.load_folder(saved);
        }
    }

    /// 検索コンテキスト中の Ctrl+↑↓ ナビゲーション。
    ///
    /// 振る舞い:
    /// 1. 現在のスタック先頭 (検索結果から入ったフォルダ) を「サブツリールート」とし、
    ///    そのサブツリー内では通常の DFS (next_folder_dfs / prev_folder_dfs) で移動する。
    /// 2. DFS の結果がサブツリー外に出る場合、検索結果リスト内の前後アイテムへジャンプする
    ///    (`favsearch_navigate_sibling`)。
    ///
    /// サブツリー内の移動はスタックに push するので、BS で元の位置に戻れる。
    ///
    /// 実装上: `navigate_folder_with_skip` は DFS + `read_dir` で UI スレッドを
    /// ブロックし得るので、`start_folder_nav` に投げて非同期実行する。結果は
    /// `apply_folder_nav_result` が `FolderNavMode::Favsearch` ブランチで
    /// 受け取り、sibling fallback も含めた後処理を行う。
    pub(crate) fn favsearch_ctrl_nav(&mut self, forward: bool) {
        if self.favsearch.nav_stack.is_empty() {
            // 検索結果リスト上ではなにもしない
            return;
        }
        let root = self.favsearch.nav_stack[0].clone();
        let Some(current) = self.favsearch.nav_stack.last().cloned() else {
            return;
        };
        self.start_folder_nav(current, forward, FolderNavMode::Favsearch { root });
    }

    /// 検索結果の前後アイテムへ移動する (Ctrl+↑↓ 用、`delta` は +1 / -1)。
    /// スタックがある場合は root (nav_stack[0]) を基準に検索結果内を前後する。
    /// 移動後は新しい root 1 つだけのスタックになる。
    pub(crate) fn favsearch_navigate_sibling(&mut self, delta: isize) {
        let results = &self.favsearch.results_paths;
        if results.is_empty() {
            return;
        }
        let cur_root = match self.favsearch.nav_stack.first() {
            Some(p) => p.clone(),
            None => return, // 検索結果リスト上では何もしない
        };
        let idx = match results.iter().position(|p| p == &cur_root) {
            Some(i) => i,
            None => {
                // 現在 root が結果リストに見つからない (データ更新等)。素直に先頭へ。
                0
            }
        };
        let next_idx = idx as isize + delta;
        if next_idx < 0 || next_idx >= results.len() as isize {
            return; // 端で止める
        }
        let next_path = results[next_idx as usize].clone();
        self.favsearch.nav_stack = vec![next_path.clone()];
        self.load_folder(next_path);
        self.update_favsearch_address();
    }

    /// BS が押されたときの検索コンテキスト内の「戻る」処理。
    /// スタックを 1 段ポップし、空になれば検索結果リストへ戻る。
    /// 抜けてきた子フォルダ/ファイル名は `select_after_load` に渡し、
    /// 戻り先で該当アイテムがカーソル選択されるようにする。
    pub(crate) fn favsearch_back(&mut self) {
        if self.favsearch.nav_stack.is_empty() {
            // 検索結果リスト上では BS は何もしない (ファイルシステム遡りを禁止)
            return;
        }
        let popped = self.favsearch.nav_stack.pop();
        if let Some(popped_path) = popped {
            if let Some(name) = popped_path.file_name().and_then(|n| n.to_str()) {
                self.select_after_load = Some(name.to_string());
            }
        }
        if let Some(top) = self.favsearch.nav_stack.last().cloned() {
            self.load_folder(top);
            self.update_favsearch_address();
        } else {
            // スタックが空になった = 検索結果リストに戻る。
            self.execute_favsearch();
        }
    }

    /// アドレスバー表示を検索コンテキストに応じて更新する。
    /// 現在地を包含するお気に入りフォルダを特定し、その配下の相対パスを表示する
    /// (例: "🔍 検索結果: "query" > Photos > 2024 > Vacation")。
    pub(crate) fn update_favsearch_address(&mut self) {
        if !self.favsearch.active {
            return;
        }
        let query = self.favsearch.last_executed.clone();
        let count = self.favsearch.results_paths.len();
        if self.favsearch.nav_stack.is_empty() {
            self.address = format!("🔍 検索結果: \"{}\"  ({} 件)", query, count);
            return;
        }
        // 現在地 = スタックの末尾 (深い場所)
        let current = self.favsearch.nav_stack.last().cloned().unwrap();
        // 包含するお気に入り根を探し、その直下からの相対パスを元のケースで組み立てる
        let segments: Vec<String> = match self.find_nearest_favorite(&current).map(|f| f.path.clone()) {
            Some(root) => {
                let root_name = root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string);
                let rel_components = current.components().skip(root.components().count());
                root_name
                    .into_iter()
                    .chain(rel_components.map(|c| c.as_os_str().to_string_lossy().to_string()))
                    .collect()
            }
            None => current
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| vec![s.to_string()])
                .unwrap_or_default(),
        };
        self.address = format!("🔍 検索結果: \"{}\" > {}", query, segments.join(" > "));
    }

    /// 現在のクエリで検索を実行し、結果をグリッドに反映する。
    /// お気に入り検索 (Ctrl+S) をバックグラウンドスレッドで開始する。
    /// SQLite FTS クエリは通常数 ms だが、インデックスが大きいと数十〜数百 ms に
    /// 達し UI を止め得る。結果は `poll_favsearch` で受信する。
    pub(crate) fn execute_favsearch(&mut self) {
        self.favsearch.last_executed = self.favsearch.query.clone();
        let query = self.favsearch.query.trim().to_string();
        self.favsearch.nav_stack.clear();

        // 既存 in-flight をキャンセル
        if let Some(pending) = self.favsearch_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }

        // 空クエリ: 空結果で即座に start_loading_items (待ち時間なし)
        if query.is_empty() {
            self.apply_favsearch_results(Vec::new());
            return;
        }

        let Some(db) = self.search_index_db.clone() else {
            return;
        };
        // §19.7: favorite_filter で単一お気に入りに絞り込む。
        // Codex P2 #3: 選択中 favorite が auto_index_structure=false になった / 削除された場合、
        // UI との食い違いを防ぐため filter を None に倒して UI も「すべて」に戻す。
        if let Some(id) = self.favsearch.favorite_filter {
            let still_valid = self
                .settings
                .favorite_by_id(id)
                .is_some_and(|f| f.auto_index_structure);
            if !still_valid {
                self.favsearch.favorite_filter = None;
            }
        }
        let fav_roots: Vec<PathBuf> = match self.favsearch.favorite_filter {
            Some(id) => self
                .settings
                .favorite_by_id(id)
                .map(|f| vec![f.path.clone()])
                .unwrap_or_default(),
            None => self
                .settings
                .favorites
                .iter()
                .map(|f| f.path.clone())
                .collect(),
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_w = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();

        let mode: crate::search_query::MatchMode = self.favsearch.or_mode.into();
        std::thread::Builder::new()
            .name("favsearch-db".to_string())
            .spawn(move || {
                if cancel_w.load(Ordering::Relaxed) {
                    return;
                }
                let result = db.search(&query, &fav_roots, mode);
                // キャンセル後の送信は無意味なので捨てる (UI 側 pending も None に戻っている)
                if cancel_w.load(Ordering::Relaxed) {
                    return;
                }
                let _ = tx.send(result);
            })
            .ok();

        self.favsearch_pending = Some(FavSearchPending { cancel, rx });
    }

    /// お気に入り検索の結果をポーリングする。
    pub(crate) fn poll_favsearch(&mut self) {
        let Some(pending) = self.favsearch_pending.as_ref() else {
            return;
        };
        match pending.rx.try_recv() {
            Ok(Ok(results)) => {
                self.favsearch_pending = None;
                self.apply_favsearch_results(results);
            }
            Ok(Err(e)) => {
                crate::logger::log(format!("favsearch query failed: {e}"));
                self.favsearch_pending = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.favsearch_pending = None;
            }
        }
    }

    /// SQLite 検索結果を `start_loading_items` に流し込む共通処理。
    fn apply_favsearch_results(&mut self, results: Vec<crate::search_index_db::IndexEntry>) {
        let items: Vec<GridItem> = results
            .iter()
            .map(|e| match e.kind {
                crate::search_index_db::IndexKind::Folder => GridItem::Folder(e.path.clone()),
                crate::search_index_db::IndexKind::ZipFile => GridItem::ZipFile(e.path.clone()),
                crate::search_index_db::IndexKind::PdfFile => GridItem::PdfFile(e.path.clone()),
            })
            .collect();
        let image_metas: Vec<Option<(i64, i64)>> =
            results.iter().map(|e| Some((e.mtime, 0))).collect();
        self.favsearch.results_paths = results.iter().map(|e| e.path.clone()).collect();

        let synthetic = search_results_synthetic_path();
        let existing_keys: std::collections::HashSet<String> = items
            .iter()
            .filter_map(|it| match it {
                GridItem::Folder(p) => p
                    .file_name()?
                    .to_str()
                    .map(|n| format!("{}{n}", CACHE_KEY_FOLDER)),
                GridItem::ZipFile(p) => p
                    .file_name()?
                    .to_str()
                    .map(|n| format!("{}{n}", CACHE_KEY_ZIP)),
                GridItem::PdfFile(p) => p
                    .file_name()?
                    .to_str()
                    .map(|n| format!("{}{n}", CACHE_KEY_PDF)),
                _ => None,
            })
            .collect();
        self.start_loading_items(synthetic, items, image_metas, existing_keys, Vec::new(), None);
        self.update_favsearch_address();
    }

    /// `path` を包含する最も近いお気に入り (パスが最長一致するもの) を返す。
    /// ZIP/PDF を開いている最中に呼ぶと ZIP/PDF 本体のパスで判定されるので、
    /// 仮想フォルダ内ページにもお気に入り標準が適用される。
    /// 入れ子 (例: `C:\pics` と `C:\pics\AI` が両方登録済み) のときは深い方を優先。
    pub(crate) fn find_nearest_favorite(
        &self,
        path: &std::path::Path,
    ) -> Option<&crate::settings::FavoriteEntry> {
        let mut best: Option<&crate::settings::FavoriteEntry> = None;
        let mut best_len = 0usize;
        for fav in &self.settings.favorites {
            if !crate::search_index_db::is_under(path, &fav.path) {
                continue;
            }
            let len = fav.path.as_os_str().len();
            if best.is_none() || len > best_len {
                best = Some(fav);
                best_len = len;
            }
        }
        best
    }

    /// タスク 3: ZIP ファイルを仮想フォルダとして開く (非同期)。
    ///
    /// ネスト ZIP の中央ディレクトリ列挙は数秒かかるため worker に逃がす。
    /// UI 側は即座に「読み込み中…」を表示し、BS / Ctrl+↑↓ で中断可能。
    /// 結果は `poll_zip_enumerate` が受け取り `start_loading_items` を呼ぶ。
    pub fn load_zip_as_folder(&mut self, zip_path: PathBuf) {
        crate::logger::log(format!(
            "=== load_zip_as_folder: {} ===",
            zip_path.display()
        ));

        // 旧 ZIP / PDF pending を Drop (cancel 自動伝搬)。
        // fs_nav_after_pdf_enumerate も一緒に捨てないと、後続の別 PDF で古い方向で
        // fullscreen が勝手に開いてしまう (Codex P2)。
        self.zip_enumerate_pending = None;
        self.pdf_enumerate_pending = None;
        self.fs_nav_after_pdf_enumerate = None;

        // 旧サムネイルワーカーを即キャンセル (pending 完了後に start_loading_items でも
        // 再度 cancel されるが、先に止めておくことで worker キューの渋滞を防ぐ)。
        self.cancel_token.store(true, Ordering::Relaxed);
        self.wake_all_workers();

        // ネスト ZIP キャッシュのクリアは **UI スレッドで** 行う。worker 内で行うと、
        // 複数 ZIP を連打したときに旧 worker の遅れた clear が新 worker の populate を
        // 上書き消去するレースが起きる。ここで同期実行すれば load_zip_as_folder 呼び出し
        // の直列順で確実にクリアされる (clear 自体は軽量: NESTED_CACHE.clear())。
        crate::zip_loader::clear_nested_cache();

        // UI 即応: items をクリアして grid が「読み込み中…」を出せる状態にし、
        // address / current_folder を zip_path にして breadcrumb と BS (parent) を正しく動かす。
        // catalog / sidecar / worker spawn などの重い設定は poll_zip_enumerate の
        // start_loading_items で行う (二重にやらない)。
        self.items.clear();
        self.thumbnails.clear();
        self.image_metas.clear();
        self.visible_indices.clear();
        self.selected = None;
        self.scroll_offset_y = 0.0;
        self.current_folder = Some(zip_path.clone());
        self.current_folder_rating_cache = None;
        self.address = zip_path.to_string_lossy().to_string();
        self.update_global_search_address();

        // worker spawn
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let path_w = zip_path.clone();
        let cancel_w = Arc::clone(&cancel);
        let spawn_result = std::thread::Builder::new()
            .name("zip-enumerate".into())
            .spawn(move || {
                if cancel_w.load(Ordering::Relaxed) {
                    return;
                }
                let result = crate::zip_loader::enumerate_image_entries(&path_w)
                    .map_err(|e| e.to_string());
                let _ = tx.send(result);
            });

        if let Err(e) = spawn_result {
            // リソース枯渇等でスレッド spawn に失敗した場合は panic せず、UI スレッドで
            // 同期的に enumerate してそのまま start_loading_items に流す
            // (PDF 側と揃えた fallback)。UI は多少詰まるが「アプリが落ちる」よりは遥かにマシ。
            crate::logger::log(format!(
                "zip-enumerate: spawn failed ({e}), falling back to synchronous enumerate"
            ));
            drop(rx);
            let result = crate::zip_loader::enumerate_image_entries(&zip_path)
                .map_err(|e| e.to_string());
            self.finalize_zip_enumerate(zip_path, self.settings.sort_order, result);
            return;
        }

        self.zip_enumerate_pending = Some(ZipEnumeratePending {
            zip_path,
            cancel,
            rx,
            sort_order: self.settings.sort_order,
        });
    }

    /// `zip_enumerate_pending` の結果を毎フレーム polling する。
    /// 完了時に group/sort + items 構築を行い `start_loading_items` に流す。
    pub(crate) fn poll_zip_enumerate(&mut self) {
        let Some(pending) = self.zip_enumerate_pending.as_ref() else {
            return;
        };
        // 新しい load_folder 等で cancel が立った (実質 Drop 直前) 場合は結果を適用しない
        if pending.cancel.load(Ordering::Relaxed) {
            self.zip_enumerate_pending = None;
            return;
        }
        let result = match pending.rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                crate::logger::log("zip enumerate: worker disconnected");
                let zip_path = pending.zip_path.clone();
                self.zip_enumerate_pending = None;
                self.start_loading_items(
                    zip_path,
                    Vec::new(),
                    Vec::new(),
                    std::collections::HashSet::new(),
                    Vec::new(),
                    None,
                );
                return;
            }
        };
        let zip_path = pending.zip_path.clone();
        let sort = pending.sort_order;
        self.zip_enumerate_pending = None;
        self.finalize_zip_enumerate(zip_path, sort, result);
    }

    /// enumerate 結果 → group/sort → `start_loading_items`。
    /// async (poll_zip_enumerate) と sync fallback (spawn 失敗時) の両方から呼ぶ。
    fn finalize_zip_enumerate(
        &mut self,
        zip_path: PathBuf,
        sort: crate::settings::SortOrder,
        result: Result<Vec<crate::zip_loader::ZipImageEntry>, String>,
    ) {
        let entries = match result {
            Ok(e) => e,
            Err(e) => {
                crate::logger::log(format!("  zip enumerate failed: {e}"));
                self.start_loading_items(
                    zip_path,
                    Vec::new(),
                    Vec::new(),
                    std::collections::HashSet::new(),
                    Vec::new(),
                    None,
                );
                return;
            }
        };
        crate::logger::log(format!("  zip: {} image entries", entries.len()));

        // サブディレクトリごとにグルーピング + 各グループ内ソート (純粋 in-memory)
        let mut groups: std::collections::BTreeMap<String, Vec<crate::zip_loader::ZipImageEntry>> =
            std::collections::BTreeMap::new();
        for e in entries {
            let dir = crate::zip_loader::entry_dir(&e.entry_name).to_string();
            groups.entry(dir).or_default().push(e);
        }
        for list in groups.values_mut() {
            list.sort_by(|a, b| {
                let an = crate::zip_loader::entry_basename(&a.entry_name);
                let bn = crate::zip_loader::entry_basename(&b.entry_name);
                sort.compare(an, a.mtime, bn, b.mtime, natural_sort_key)
            });
        }

        let insert_separators = groups.len() > 1;
        let mut items: Vec<GridItem> = Vec::new();
        let mut image_metas: Vec<Option<(i64, i64)>> = Vec::new();
        let mut existing_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (dir, list) in groups {
            if insert_separators {
                let display = if dir.is_empty() {
                    "(ルート)".to_string()
                } else {
                    dir.clone()
                };
                items.push(GridItem::ZipSeparator {
                    dir_display: display,
                });
                image_metas.push(None);
            }
            for e in list {
                existing_keys.insert(e.entry_name.clone());
                items.push(GridItem::ZipImage {
                    zip_path: zip_path.clone(),
                    entry_name: e.entry_name,
                });
                image_metas.push(Some((e.mtime, e.uncompressed_size as i64)));
            }
        }

        self.start_loading_items(zip_path, items, image_metas, existing_keys, Vec::new(), None);

        // Ctrl+↑↓ フォルダナビから fullscreen で ZIP に遷移してきた場合、items が
        // 揃った今 fullscreen を開き直す (Codex P1: PDF と同じ処理を ZIP にも適用)。
        if let Some(_forward) = self.fs_nav_after_pdf_enumerate.take() {
            if let Some(new_idx) = self.find_fullscreen_nav_target() {
                self.open_fullscreen(new_idx);
                self.selected = Some(new_idx);
                self.scroll_to_selected = true;
                self.update_last_selected_image();
            }
        }
    }

    /// PDF ファイルを仮想フォルダとして開く (非同期)。
    ///
    /// ワーカーにページ列挙リクエストを送り、即座に return する。
    /// 結果は `poll_pdf_enumerate` が次フレーム以降にポーリングして処理する。
    /// パスワード付き PDF の場合はダイアログで入力を求める。
    pub fn load_pdf_as_folder(&mut self, pdf_path: PathBuf) {
        crate::logger::log(format!(
            "=== load_pdf_as_folder: {} ===",
            pdf_path.display()
        ));

        // PDF を開く際、直前に ZIP を見ていた可能性があるためネスト ZIP キャッシュを破棄する。
        crate::zip_loader::clear_nested_cache();

        // 旧サムネイルワーカーを即座にキャンセルして PDF ワーカーキューの渋滞を防ぐ。
        // start_loading_items は enumerate 完了後に呼ばれるため、ここで先行キャンセルする。
        self.cancel_token.store(true, Ordering::Relaxed);
        self.wake_all_workers();

        // 旧 pending を drop すると `PdfEnumerateHandle::Drop` が cancel を立て、
        // pool dispatcher は pop 時に IPC 前で古いジョブを捨てる。
        // ZIP 側 pending も一緒に捨てる (ZIP → PDF 遷移時の取り残し防止、Codex P2)。
        self.pdf_enumerate_pending = None;
        self.zip_enumerate_pending = None;

        // ── パスワード確認 ──
        let password: Option<String> = self
            .pdf_passwords
            .get(&pdf_path)
            .or_else(|| self.pdf_current_password.clone());

        // パスワードチェックも非同期化したいが、ダイアログ表示のフローが複雑になるため
        // ここでは簡易判定: 保存済みパスワードがなければ非同期で check_password を含めて
        // enumerate を試みる。パスワードエラーは結果受信時にハンドルする。

        // ── 非同期でページ列挙をリクエスト ──
        let handle = crate::pdf_loader::enumerate_pages_async(&pdf_path, password.as_deref());
        self.pdf_enumerate_pending = Some((pdf_path.clone(), password, handle));

        // アドレスバーを即座に更新 (ローディング中であることを示す)
        self.address = pdf_path.to_string_lossy().to_string();
        // Ctrl+G 絞り込みビュー中なら生の PDF パスを「🌐 全検索: "query" > scansnap >
        // ファイル名.pdf」のブレッドクラム形式で上書きし直す (2026-04 ユーザー報告)。
        // no-op: Ctrl+G 非アクティブ / Aggregated 時は何もしない。
        self.update_global_search_address();
    }

    /// PDF ページ列挙の非同期応答をポーリングする。
    /// 毎フレーム `update()` から呼び出す。
    pub(crate) fn poll_pdf_enumerate(&mut self) {
        let Some((ref pdf_path, _, ref handle)) = self.pdf_enumerate_pending else {
            return;
        };

        // Generation guard: cancel が立っている pending の結果は破棄する。
        // load_pdf_as_folder は旧 pending を置き換える (= Drop で cancel) ため通常は
        // ここに None で到達するが、将来 pending を cancel 後に再利用する経路が
        // 追加されても古い結果を適用しないための念押し。
        if handle.cancel.load(Ordering::Relaxed) {
            self.pdf_enumerate_pending = None;
            return;
        }

        let result = match handle.rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return, // まだ結果が来ていない
            Err(mpsc::TryRecvError::Disconnected) => {
                // ワーカーが切断 (通常起きない)
                crate::logger::log("  pdf enumerate: worker disconnected");
                let path = pdf_path.clone();
                self.pdf_enumerate_pending = None;
                self.fs_nav_after_pdf_enumerate = None;
                self.start_loading_items(
                    path,
                    Vec::new(),
                    Vec::new(),
                    std::collections::HashSet::new(),
                    Vec::new(),
                    None,
                );
                return;
            }
        };

        let (pdf_path, password, _handle) = self.pdf_enumerate_pending.take().unwrap();

        // cancel 経由の Interrupted は late-arriving な stale 結果なので適用しない
        // (pool dispatcher が cancel を見て IPC 前に Err で返してくるパス)
        if let Err(ref e) = result {
            if e.kind() == std::io::ErrorKind::Interrupted {
                crate::logger::log(format!(
                    "  pdf enumerate: cancelled result dropped for {}",
                    pdf_path.display()
                ));
                return;
            }
        }

        match result {
            Ok(pages) => {
                crate::logger::log(format!("  pdf: {} pages", pages.len()));
                self.pdf_current_password = password;

                // パスワードダイアログで「保存する」を ON にして OK した場合、
                // enumerate 成功時点で DPAPI に保存する (ダイアログでの sync 検証を
                // やめた代替経路)。パスが一致しないのは別 PDF が割り込んだケースなので保存しない。
                if let Some((save_path, pw)) = self.pdf_password_pending_save.take() {
                    if save_path == pdf_path {
                        self.pdf_passwords.set(&save_path, &pw);
                        self.pdf_passwords.save();
                    }
                }

                let mut items: Vec<GridItem> = Vec::new();
                let mut image_metas: Vec<Option<(i64, i64)>> = Vec::new();
                let mut existing_keys: std::collections::HashSet<String> =
                    std::collections::HashSet::new();

                for page in &pages {
                    let key = crate::grid_item::pdf_page_cache_key(page.page_num);
                    existing_keys.insert(key);
                    items.push(GridItem::PdfPage {
                        pdf_path: pdf_path.clone(),
                        page_num: page.page_num,
                        content_type: None, // render 時に解析
                    });
                    image_metas.push(Some((page.mtime, page.file_size as i64)));
                }

                self.start_loading_items(pdf_path, items, image_metas, existing_keys, Vec::new(), None);

                // Ctrl+↑↓ フォルダナビから遷移してきた場合はここで fullscreen を開き直す。
                if let Some(_forward) = self.fs_nav_after_pdf_enumerate.take() {
                    if let Some(new_idx) = self.find_fullscreen_nav_target() {
                        self.open_fullscreen(new_idx);
                        self.selected = Some(new_idx);
                        self.scroll_to_selected = true;
                        self.update_last_selected_image();
                    }
                }
            }
            Err(e) => {
                let err_msg = format!("{e}");
                // パスワードエラーかどうかを判定 (エラーメッセージに "Password" が含まれる)
                if err_msg.contains("Password") || err_msg.contains("password") {
                    // 誤ったパスワードを DPAPI/セッションキャッシュに保存していた場合は破棄し、
                    // ユーザーに再入力してもらう。DPAPI も破棄しないと、次の load_pdf_as_folder が
                    // また stale を拾って無限ループになる。
                    self.pdf_password_pending_save = None;
                    if password.is_some() {
                        self.pdf_current_password = None;
                        self.pdf_passwords.remove(&pdf_path);
                        self.pdf_passwords.save();
                    }
                    // Ctrl+↑↓ 由来の deferred fullscreen 意図は破棄する
                    // (パスワード入力後に再び fullscreen にしたければユーザーが手動で開く)。
                    self.fs_nav_after_pdf_enumerate = None;
                    self.pdf_password_pending_path = Some(pdf_path);
                    self.show_pdf_password_dialog = true;
                    self.pdf_password_input.clear();
                    // password が渡されていた = 入力済みのパスワードが誤っていた
                    self.pdf_password_error = if password.is_some() {
                        Some("パスワードが正しくありません".to_string())
                    } else {
                        None
                    };
                    self.pdf_password_save = false;
                    return;
                }
                // その他の失敗: deferred 意図をクリアしてグリッド表示にフォールバック
                self.fs_nav_after_pdf_enumerate = None;
                crate::logger::log(format!("  pdf enumerate failed: {e}"));
                self.start_loading_items(
                    pdf_path,
                    Vec::new(),
                    Vec::new(),
                    std::collections::HashSet::new(),
                    Vec::new(),
                    None,
                );
            }
        }
    }

    /// load_folder と load_zip_as_folder の共通処理。
    /// PDF / ZIP を仮想フォルダとして開いた直後の seed + write-back ターゲット設定 +
    /// PDFium prefetch grace 設定をまとめて行う。
    ///
    /// 背景:
    /// - 親フォルダのグリッド表示中に PdfFile/ZipFile のサムネは
    ///   親 catalog の `pdfthumb:foo.pdf` / `zipthumb:foo.zip` キーに保存される。
    /// - 仮想 catalog (PDF/ZIP path で別 SQLite) は別で `page_0000` / 最初の
    ///   エントリ name キーを使う。
    /// - 二つの catalog は独立しているので、毎回同じ画像を 2 回生成していた
    ///   (PDF は PDFium 経由 200-500ms)。
    ///
    /// この関数は 3 つの方向で対策する:
    /// 1. **seed**: 親 catalog にエントリがあれば仮想 catalog にコピー (= 再生成回避)。
    ///    既に grid を経由していたケースは即時ヒット。
    /// 2. **writeback ターゲット設定**: 仮想 catalog 側で worker が page_0000 を
    ///    完成させた後、`poll_thumbnails` の finalize で親 catalog にもミラー書き込み。
    ///    grid を経由しないルート (= Ctrl+↑↓ で直接 PDF を開く流れ) では、初回の
    ///    PDFium レンダリングコストは取り戻せないが、2 回目以降の同じフォルダ進入で
    ///    seed 経路がヒットするようになる。
    /// 3. **prefetch grace**: PDF を開いた直後 100ms は prefetch を抑止して、
    ///    Ctrl+↑↓ 連打の最中に PDFium プールが「破棄される予定の prefetch render」
    ///    で詰まらないようにする。ユーザーがその場で停止したらそこから prefetch 開始。
    fn setup_virtual_folder_seed_and_writeback(
        &mut self,
        source_path: &Path,
        target_db: &crate::catalog::CatalogDb,
        cache_map: &Arc<
            std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        >,
    ) {
        let ext = source_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase());
        let (parent_prefix, target_key, target_idx, is_pdf) = match ext.as_deref() {
            Some("pdf") => {
                // 最初の PdfPage の idx (通常 0、ZipSeparator 等が入らない PDF では 0 確定)。
                let Some(target_idx) = self.items.iter().position(|it| {
                    matches!(it, GridItem::PdfPage { page_num: 0, .. })
                }) else {
                    return;
                };
                (
                    crate::thumb_loader::CACHE_KEY_PDF,
                    crate::grid_item::pdf_page_cache_key(0),
                    target_idx,
                    true,
                )
            }
            Some("zip") => {
                let Some((target_idx, entry_name)) =
                    self.items.iter().enumerate().find_map(|(i, it)| match it {
                        GridItem::ZipImage { entry_name, .. } => Some((i, entry_name.clone())),
                        _ => None,
                    })
                else {
                    return;
                };
                (
                    crate::thumb_loader::CACHE_KEY_ZIP,
                    entry_name,
                    target_idx,
                    false,
                )
            }
            _ => return,
        };

        // PDF/ZIP 開いた直後の prefetch 抑制 (= worker thrash 防止) を仕掛ける。
        // PDF だけに適用 (ZIP は decode が軽いので大した負荷にならない)。
        if is_pdf {
            self.pdf_prefetch_grace_until = Some(
                std::time::Instant::now() + std::time::Duration::from_millis(100),
            );
        }

        let Some(parent) = source_path.parent() else {
            return;
        };
        let Some(fname) = source_path.file_name().and_then(|n| n.to_str()) else {
            return;
        };
        let parent_key = format!("{parent_prefix}{fname}");
        let Some(parent_db) = self.get_or_open_catalog(parent) else {
            return;
        };

        // ── seed 経路 ──────────────────────────────────────────────
        // 仮想 catalog 側に既にあれば何もしない (再 seed 不要)。
        let already_in_virtual = cache_map
            .read()
            .map(|m| m.contains_key(&target_key))
            .unwrap_or(false);
        let parent_entry = if !already_in_virtual {
            parent_db.load_one(&parent_key).ok().flatten()
        } else {
            None
        };
        if let Some(entry) = parent_entry {
            // ヘッダのみで寸法を取り、target_db に書き込み。
            // バイト列が壊れていた (= save_thumb_bytes が false 返却) ケースでは
            // cache_map にも入れない — 入れるとサムネ表示時にデコード失敗で
            // `Failed` 状態に陥る (Codex P2)。
            // SQLite I/O エラーは Ok(false) と区別してログに残す (= 静かに丸めると
            // disk full / lock 競合の調査が困難になるため)。
            let saved = match target_db.save_thumb_bytes(
                &target_key,
                entry.mtime,
                entry.file_size,
                entry.source_dims,
                &entry.jpeg_data,
            ) {
                Ok(b) => b,
                Err(e) => {
                    crate::logger::log(format!(
                        "seed virtual folder: save_thumb_bytes failed for {target_key}: {e}"
                    ));
                    false
                }
            };
            if saved {
                if let Ok(mut m) = cache_map.write() {
                    m.insert(target_key.clone(), entry);
                }
                crate::perf::event("nav", "virtual_seed_hit", Some(&parent_key), 0, &[]);
            } else {
                crate::perf::event(
                    "nav",
                    "virtual_seed_undecodable",
                    Some(&parent_key),
                    0,
                    &[],
                );
            }
        } else if !already_in_virtual {
            crate::perf::event("nav", "virtual_seed_miss", Some(&parent_key), 0, &[]);
        }

        // ── writeback ターゲット設定 ──────────────────────────────
        // 親エントリの mtime / size を取得 (= PDF/ZIP ファイル本体のメタ)。
        let (entry_mtime, entry_size) = match std::fs::metadata(source_path) {
            Ok(m) => {
                let mt = crate::ui_helpers::mtime_secs(&m);
                let sz = m.len() as i64;
                (mt, sz)
            }
            Err(_) => (0, 0),
        };
        self.virtual_folder_writeback = Some(VirtualFolderWriteback {
            parent_catalog: parent_db,
            parent_key,
            target_key,
            target_idx,
            items_gen: self.items_generation,
            cache_map: Arc::clone(cache_map),
            parent_entry_mtime: entry_mtime,
            parent_entry_size: entry_size,
        });
    }

    /// `poll_thumbnails` の finalize シグナル経路から呼ばれ、
    /// `virtual_folder_writeback` の対象 idx が完成していたら親 catalog にミラー書き込みする。
    /// 一度発火したら writeback を None にして二重書き込みを防ぐ。
    ///
    /// 古い items 世代の writeback が残っていた (= `items_generation` 進行後) 場合は
    /// idx 一致でも発火させずクリアのみする (Codex P2: 別フォルダの親 catalog への
    /// 誤書き込み防止)。
    fn fire_virtual_folder_writeback_if_ready(&mut self, finalized_idx: usize) {
        let Some(wb) = self.virtual_folder_writeback.as_ref() else {
            return;
        };
        if wb.items_gen != self.items_generation {
            self.virtual_folder_writeback = None;
            return;
        }
        if finalized_idx != wb.target_idx {
            return;
        }
        // cache_map から WebP データを取り出す (worker が直前に書き込んでいるはず)。
        // RwLock の read guard を save() 越しに保持しないために clone する
        // (save 中はファイル I/O で blocking、guard 保持は他スレッドの読み書きを止める)。
        // None なら from_cache 経路 (cache hit で finalize 不発) など、worker が
        // cache_map にエントリを残さなかったケース。再発火させても効果がないので
        // 1 回試してクリアする (Codex P2: 失敗で stale 残置を防ぐ)。
        let entry_opt = wb
            .cache_map
            .read()
            .ok()
            .and_then(|m| m.get(&wb.target_key).cloned());
        if let Some(entry) = entry_opt {
            let parent_key = wb.parent_key.clone();
            let _ = wb.parent_catalog.save_thumb_bytes(
                &parent_key,
                wb.parent_entry_mtime,
                wb.parent_entry_size,
                entry.source_dims,
                &entry.jpeg_data,
            );
            crate::perf::event("nav", "virtual_writeback", Some(&parent_key), 0, &[]);
        }
        self.virtual_folder_writeback = None;
    }

    /// `catalog_cache` の全エントリを drop し、SQLite Connection を閉じる。
    /// キャッシュマネージャの「削除」操作 (DeleteOld / DeleteAll / DeleteFolder)
    /// 呼び出し前に必須。Windows では Connection が握っている `.db` ファイルは
    /// `remove_file` で削除できず silent fail するので、削除前に Connection を
    /// 全部畳む必要がある (Codex P3)。LRU が小さいので評価コストは無視可能。
    pub(crate) fn evict_all_catalog_cache(&mut self) {
        self.catalog_cache.clear();
        self.catalog_cache_order.clear();
    }

    /// `catalog_cache` から `Arc<CatalogDb>` を取得し、無ければ open して挿入する。
    /// LRU 最大 16 件、超過時は FIFO で古いものから drop (= SQLite Connection close)。
    /// `start_loading_items` の毎ステップ open を吸収するためのキャッシュ。
    /// open 失敗時は cache に挿入せず None を返す (次回も open を再試行)。
    pub(crate) fn get_or_open_catalog(
        &mut self,
        folder_path: &Path,
    ) -> Option<Arc<crate::catalog::CatalogDb>> {
        const MAX_CACHED: usize = 16;
        // hit: LRU 末尾に移動して返す。
        if self.catalog_cache.contains_key(folder_path) {
            if let Some(pos) = self
                .catalog_cache_order
                .iter()
                .position(|p| p == folder_path)
            {
                let key = self.catalog_cache_order.remove(pos).unwrap();
                self.catalog_cache_order.push_back(key);
            }
            return self.catalog_cache.get(folder_path).cloned();
        }
        // miss: open + insert。失敗時は cache を汚さない。
        let cache_dir = crate::catalog::default_cache_dir();
        let catalog = match crate::catalog::CatalogDb::open(&cache_dir, folder_path) {
            Ok(c) => Arc::new(c),
            Err(e) => {
                crate::logger::log(format!("  catalog open failed: {e}"));
                return None;
            }
        };
        // 容量超過なら最古を evict (Arc が外部で保持されていなければ Connection が close)。
        while self.catalog_cache_order.len() >= MAX_CACHED {
            if let Some(oldest) = self.catalog_cache_order.pop_front() {
                self.catalog_cache.remove(&oldest);
            } else {
                break;
            }
        }
        let key: PathBuf = folder_path.to_path_buf();
        self.catalog_cache.insert(key.clone(), Arc::clone(&catalog));
        self.catalog_cache_order.push_back(key);
        Some(catalog)
    }

    ///
    /// 与えられた `items` / `image_metas` を新しい状態として設定し、
    /// 旧タスクをキャンセル → カタログを開く → 永続ワーカー + 動画スレッドを起動 →
    /// 履歴復元 → last_folder 保存 までを行う。
    fn start_loading_items(
        &mut self,
        source_path: PathBuf,
        items: Vec<GridItem>,
        image_metas: Vec<Option<(i64, i64)>>,
        catalog_existing_keys: std::collections::HashSet<String>,
        video_items: Vec<(usize, PathBuf, u64)>,
        folder_signature: Option<u64>,
    ) {
        // 通常フォルダ / ZIP / 検索結果など PDF 以外への遷移では、残存する PDF
        // enumerate pending を無効化する。放置すると遅れて届いた結果を
        // `poll_pdf_enumerate` が適用して現在表示を古い PDF 仮想フォルダに戻す。
        // pending.Drop で自動 cancel されるので take するだけでよい。
        if let Some((pending_path, _, _)) = self.pdf_enumerate_pending.as_ref() {
            if pending_path != &source_path {
                self.pdf_enumerate_pending = None;
                self.fs_nav_after_pdf_enumerate = None;
            }
        }
        // ZIP も同様: 別パスへの遷移 (BS で親フォルダ、Ctrl+↑↓ で兄弟フォルダ等) が起きたら、
        // 途中結果で上書き戻されないよう pending を捨てる (Drop で worker cancel)。
        // 同じ zip_path への `start_loading_items` は poll_zip_enumerate 自身が呼ぶので残す。
        if let Some(pending) = self.zip_enumerate_pending.as_ref() {
            if pending.zip_path != source_path {
                self.zip_enumerate_pending = None;
            }
        }

        // 残存する仮想フォルダ writeback / PDF prefetch grace をリセットする (Codex P2)。
        // 前 items に対して仕掛けた writeback は、次に同じ target_idx (通常 0) の finalize
        // が来た瞬間に「全く違う PDF/ZIP の親 catalog」へ書き込みを発火させてしまう。
        // setup_virtual_folder_seed_and_writeback が新フォルダ用に再設定する場合は
        // それが上書きするので問題ない。
        self.virtual_folder_writeback = None;
        self.pdf_prefetch_grace_until = None;

        // perf: start_loading_items 全体 + 内訳 (sidecar_flush / close_fullscreen /
        // state_reset / prewarm_rating / adjustment_db / mask_db / catalog_open /
        // catalog_load_all / catalog_delete_missing / spawn_workers / settings_save)。
        // UI スレッドブロックの真犯人を特定するため、区間ごとに ms を記録する。
        let sli_t0 = std::time::Instant::now();
        let sli_seq = self.input_seq;
        let items_len = items.len();
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "sli_begin",
                None,
                sli_seq,
                &[("items", serde_json::Value::from(items_len))],
            );
        }

        // ── サイドカーをフラッシュしてメモリから降ろす ──
        // フォルダ切替前に dirty なサイドカーをディスクに書き出す。メモリ上の表現は
        // 破棄して再読み込みに任せる (長時間稼働時のメモリリーク防止)。
        let sidecar_t0 = std::time::Instant::now();
        self.flush_all_sidecars();
        self.sidecars.clear();
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "sli_sidecar_flush",
                None,
                sli_seq,
                &[(
                    "ms",
                    serde_json::Value::from(sidecar_t0.elapsed().as_secs_f64() * 1000.0),
                )],
            );
        }

        // ── 履歴保存 + 旧タスクキャンセル + 状態リセット ──
        if let Some(cur) = self.current_folder.clone() {
            self.folder_history
                .insert(cur, (self.scroll_offset_y, self.selected));
        }
        self.close_fullscreen();

        // close_fullscreen_end から sli_prewarm_rating までの区間を 3 つに分割して
        // 計測する (nav cancel / items 割当 / キャッシュ clear)。UI が止まる潜在箇所を
        // 特定できるようにするため。
        let cancel_t0 = std::time::Instant::now();
        // 進行中のフォルダナビゲーションをキャンセル
        // (他の経路でフォルダが変更された場合に不要な結果を破棄する)
        if let Some(pending) = self.folder_nav_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        // モード・累積もリセット (新しい load_folder が走る = 連打バースト中断)
        self.pending_folder_nav_steps = 0;
        self.pending_folder_nav_mode = FolderNavMode::Grid;

        self.cancel_token.store(true, Ordering::Relaxed);
        self.wake_all_workers();
        crate::logger::log("  cancel_token -> true (old tasks will stop)");
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel_token = Arc::clone(&cancel);

        let (tx, rx) = mpsc::channel();
        self.tx = tx.clone();
        self.rx = rx;
        crate::perf::emit_ms("nav", "sli_nav_cancel", sli_seq, cancel_t0);

        let assign_t0 = std::time::Instant::now();
        self.current_folder = Some(source_path.clone());
        self.current_folder_rating_cache = None;
        // フォルダ切替でユーザ明示設定の記録もリセット (別フォルダでは無関係)
        self.user_set_rating_keys.clear();
        self.reset_folder_rating_counts();
        // ★フィルタ suppression scope の判定: anchor の subtree 外に出たら復元する。
        // 判定は current_folder 反映直後に行うため、`effective_folder()` が最新値を返す。
        // archive_source_override は呼び出し元で再設定されるまで None なので、
        // 7z/LZH キャッシュ閲覧中の判定はここでは current_folder=cache_zip 基準になる。
        // `archive_convert` 経由のケースでは後段で override が設定され、次回 load_folder
        // 時にもう一度評価されるため最終的に一致する。
        self.maybe_restore_rating_filter_if_out_of_scope();
        // 外部更新の自動反映で使う mtime。ディレクトリ実体のみ (ZIP / PDF / 検索合成は
        // 仮想フォルダなのでファイル追加イベントの対象外)。metadata 失敗時は None のまま。
        self.current_folder_last_mtime = source_path
            .metadata()
            .ok()
            .filter(|m| m.is_dir())
            .and_then(|m| m.modified().ok());
        // mtime が None の経路 (ZIP / PDF / 検索結果) ではシグネチャも持たない:
        // フォーカス復帰の差分判定は `current_folder_last_mtime` の有無でゲートされるため
        // 副次的にシグネチャ比較も無効化される。
        self.current_folder_signature = self
            .current_folder_last_mtime
            .is_some()
            .then_some(folder_signature)
            .flatten();
        self.address = source_path.to_string_lossy().to_string();
        // Ctrl+G 絞り込みビュー中はブレッドクラム形式を維持する
        // (2026-04 ユーザー報告: PDF 開くと raw パスに戻ってしまうバグ)。
        // no-op: Ctrl+G 非アクティブ / Aggregated 時は何もしない。
        self.update_global_search_address();
        // 通常ロードでは変換済みアーカイブ override を解除 (呼び出し元が後で再設定する)。
        self.archive_source_override = None;
        self.selected = None;
        self.scroll_offset_y = 0.0;
        self.scroll_to_selected = false;
        self.scroll_hint.store(0, Ordering::Relaxed);

        self.install_new_items(items, image_metas);
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "sli_items_assign",
                None,
                sli_seq,
                &[
                    (
                        "ms",
                        serde_json::Value::from(assign_t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("items", serde_json::Value::from(self.items.len())),
                ],
            );
        }

        let clear_t0 = std::time::Instant::now();
        let texture_backlog_len = self.texture_backlog.len();
        let fs_upload_backlog_len = self.fs_upload_backlog.len();
        self.requested.clear();
        self.pending_finalize.clear();
        self.texture_backlog.clear();
        self.keep_range = (0, 0);
        self.keep_set.clear();
        self.metadata_cache.clear();
        self.exif_cache.clear();
        self.xmp_cache.clear();
        self.tags_cache.clear();
        self.checked.clear();
        self.rotation_cache.clear();
        self.rating_cache.clear();
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "sli_cache_clear",
                None,
                sli_seq,
                &[
                    (
                        "ms",
                        serde_json::Value::from(clear_t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    (
                        "texture_backlog",
                        serde_json::Value::from(texture_backlog_len),
                    ),
                    (
                        "fs_upload_backlog",
                        serde_json::Value::from(fs_upload_backlog_len),
                    ),
                ],
            );
        }
        // 1 回のクエリで全アイテムのレーティングを引いてキャッシュに載せる。
        // これにより rebuild_visible_indices や draw_cell からの初回 get_rating が
        // SQLite を叩かずに済む (大量フォルダで初フレームが詰まるのを防ぐ)。
        let prewarm_t0 = std::time::Instant::now();
        self.prewarm_rating_cache();
        // タグバッジ用も同様に fts_meta から一括 prewarm (indexed favorite のみ)。
        self.prewarm_grid_tags();
        self.rebuild_visible_indices();
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "sli_prewarm_rating",
                None,
                sli_seq,
                &[(
                    "ms",
                    serde_json::Value::from(prewarm_t0.elapsed().as_secs_f64() * 1000.0),
                )],
            );
        }
        // 見開きモード: DB から読み込み、なければデフォルト値
        self.spread_mode = self
            .spread_db
            .as_ref()
            .and_then(|db| db.get(&source_path))
            .unwrap_or(self.settings.default_spread_mode);
        self.spread_popup_open = false;
        self.search_filter = None;
        self.search_query.clear();
        // 非同期検索 in-flight があればキャンセル (フォルダ切替で items が変わるため
        // 検索結果インデックスが意味を失う)
        if let Some(pending) = self.search_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        // メタデータ読み込みも idx ベースなのでフォルダ切替時にキャンセル
        if let Some(pending) = self.metadata_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }

        // 画像補正: ページ個別パラメータを DB から復元
        self.adjustment_cache.clear();
        self.thumb_pixels.clear();
        self.thumb_adjust_tex.clear();
        self.thumb_adjust_was_dragging = false;
        self.adjustment_page_params.clear();
        self.adjustment_dragging = false;
        self.adjustment_mode = false;
        self.mask_pages.clear();

        // ── サイドカー → 中央 DB のインポート ──
        // フォルダ丸ごと移動された場合など、中央 DB に無いエントリがサイドカーにあれば
        // 取り込む。DB にあるエントリは authoritative なので上書きしない。
        // 下の `db.load_page_params` はインポート後に走るので、補填されたエントリも拾える。
        let sidecar_import_t0 = std::time::Instant::now();
        if self.settings.sidecar_backup_enabled {
            let sidecar_folder = if source_path.is_dir() {
                source_path.clone()
            } else {
                source_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| source_path.clone())
            };
            self.import_sidecar_to_dbs(&sidecar_folder);
        }
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "sli_sidecar_import",
                None,
                sli_seq,
                &[(
                    "ms",
                    serde_json::Value::from(sidecar_import_t0.elapsed().as_secs_f64() * 1000.0),
                )],
            );
        }

        let adj_t0 = std::time::Instant::now();
        if let Some(db) = &self.adjustment_db {
            let prefix = crate::adjustment_db::normalize_path(&source_path);
            let page_map = db.load_page_params(&prefix);
            if !page_map.is_empty() {
                for idx in 0..self.items.len() {
                    if let Some(key) = self.page_path_key(idx) {
                        if let Some(params) = page_map.get(&key) {
                            self.adjustment_page_params.insert(idx, params.clone());
                        }
                    }
                }
            }
        }
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "sli_adjustment_db",
                None,
                sli_seq,
                &[(
                    "ms",
                    serde_json::Value::from(adj_t0.elapsed().as_secs_f64() * 1000.0),
                )],
            );
        }

        // 消しゴムマスク: フォルダ内でマスクを持つページを列挙
        let mask_t0 = std::time::Instant::now();
        if let Some(db) = &self.mask_db {
            let prefix = crate::adjustment_db::normalize_path(&source_path);
            let mask_keys = db.load_mask_keys(&prefix);
            if !mask_keys.is_empty() {
                for idx in 0..self.items.len() {
                    if let Some(key) = self.page_path_key(idx) {
                        if mask_keys.contains(&key) {
                            self.mask_pages.insert(idx);
                        }
                    }
                }
            }
        }
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "sli_mask_db",
                None,
                sli_seq,
                &[(
                    "ms",
                    serde_json::Value::from(mask_t0.elapsed().as_secs_f64() * 1000.0),
                )],
            );
        }
        // visible_indices はアイテム設定後 (下の行) に再計算される

        // ── カタログを開く + cache_map ロード + 削除掃除 ──
        let catalog_open_t0 = std::time::Instant::now();
        let catalog_arc = self.get_or_open_catalog(&source_path);
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "sli_catalog_open",
                None,
                sli_seq,
                &[(
                    "ms",
                    serde_json::Value::from(catalog_open_t0.elapsed().as_secs_f64() * 1000.0),
                )],
            );
        }

        let catalog_load_t0 = std::time::Instant::now();
        let cache_map: Arc<
            std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        > = Arc::new(std::sync::RwLock::new(
            catalog_arc
                .as_ref()
                .and_then(|c| c.load_all().ok())
                .unwrap_or_default(),
        ));
        let catalog_entries = cache_map.read().unwrap().len();
        crate::logger::log(format!("  catalog: {catalog_entries} entries in DB"));

        // PDF / ZIP を仮想フォルダとして開いた場合、親フォルダ catalog の
        // pdfthumb: / zipthumb: 先頭サムネを仮想 catalog の page_0000 / 最初の
        // entry name キーに seed する (PDFium 再レンダリング 200-500ms を回避)。
        // 同時に write-back ターゲットを設定して「仮想 catalog で完成した先頭サムネを
        // 親 catalog にミラー保存」する経路も開く (次回親フォルダ表示時の grid サムネ
        // 生成を省略するため)。
        // `pdf_prefetch_grace_until` も初期化して、仮想フォルダ開いた直後の数十 ms は
        // PDFium ワーカーを現ページ専用に空けておく (Ctrl+↑↓ 連打時の thrash 抑制)。
        if let Some(ref cat) = catalog_arc {
            self.setup_virtual_folder_seed_and_writeback(
                &source_path,
                cat,
                &cache_map,
            );
        }
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "sli_catalog_load_all",
                None,
                sli_seq,
                &[
                    (
                        "ms",
                        serde_json::Value::from(catalog_load_t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("entries", serde_json::Value::from(catalog_entries)),
                ],
            );
        }

        let catalog_del_t0 = std::time::Instant::now();
        if source_path != search_results_synthetic_path() {
            if let Some(ref cat) = catalog_arc {
                if let Err(e) = cat.delete_missing(&catalog_existing_keys) {
                    crate::logger::log(format!("  catalog delete_missing failed: {e}"));
                }
            }
        }
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "sli_catalog_delete_missing",
                None,
                sli_seq,
                &[(
                    "ms",
                    serde_json::Value::from(catalog_del_t0.elapsed().as_secs_f64() * 1000.0),
                )],
            );
        }

        // ── 進捗カウンタリセット + 共有 display_px 更新 ──
        self.cache_gen_total = 0;
        self.cache_gen_done = Arc::new(AtomicUsize::new(0));

        let initial_display_px = compute_display_px(
            self.last_cell_size,
            self.last_cell_h,
            self.last_pixels_per_point,
        );
        self.display_px_shared
            .store(initial_display_px, Ordering::Relaxed);
        crate::logger::log(format!(
            "  display_px = {initial_display_px}  cache_policy = {}",
            self.settings.cache_policy.label()
        ));

        // ── 永続ワーカー + (必要なら) 動画スレッドを起動 ──
        let spawn_t0 = std::time::Instant::now();
        let reload_queue: Arc<NotifyQueue> = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let heavy_io_queue: Arc<NotifyQueue> = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        self.reload_queue = Some(Arc::clone(&reload_queue));
        self.heavy_io_queue = Some(Arc::clone(&heavy_io_queue));

        self.spawn_thumbnail_workers(
            &tx,
            Arc::clone(&cancel),
            reload_queue,
            heavy_io_queue,
            cache_map,
            catalog_arc,
        );
        if !video_items.is_empty() {
            self.spawn_video_thread(tx, cancel, video_items, self.video_thumb_overrides.clone());
        }
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "sli_spawn_workers",
                None,
                sli_seq,
                &[(
                    "ms",
                    serde_json::Value::from(spawn_t0.elapsed().as_secs_f64() * 1000.0),
                )],
            );
        }

        // ── 履歴復元 + last_folder 保存 ──
        // `select_after_load` (BS 戻り / 親フォルダボタン / `favsearch_back`) が指定されている
        // ときは履歴より優先する: 深い階層から BS で戻ったとき、最初にフォルダへ入った位置
        // ではなく「今いる位置」にカーソルを合わせるため。指定アイテムが items に見つから
        // なかった場合 (削除等) のみ履歴へフォールバック。
        let restored = self.try_select_after_load();
        if !restored {
            if let Some(&(scroll, sel)) = self.folder_history.get(&source_path) {
                self.scroll_offset_y = scroll;
                self.selected = sel;
                if sel.is_some() {
                    self.scroll_to_selected = true;
                }
                // 前回保存時は可視だった sel が、現在のフィルタ状態では非可視かもしれない。
                // `rebuild_visible_indices` 時点では selected が None だったので redirect が
                // 走っておらず、ここで再度 redirect して WYSIWYG 不変条件を保つ (Codex P2)。
                self.redirect_selected_to_visible();
            }
        }
        // 検索結果用の合成パスは last_folder に記録しない (次回起動時に復元しないため)
        let save_t0 = std::time::Instant::now();
        if source_path != search_results_synthetic_path() {
            self.settings.last_folder = Some(source_path);
            self.settings.save();
        }
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "sli_settings_save",
                None,
                sli_seq,
                &[(
                    "ms",
                    serde_json::Value::from(save_t0.elapsed().as_secs_f64() * 1000.0),
                )],
            );
            crate::perf::event(
                "nav",
                "sli_end",
                None,
                sli_seq,
                &[
                    (
                        "ms",
                        serde_json::Value::from(sli_t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("items", serde_json::Value::from(items_len)),
                ],
            );
        }
    }

    /// items / image_metas / thumbnails を新しい並びで差し替え、items_generation を bump する。
    ///
    /// 世代更新と thumbnails の Pending 初期化を 1 箇所に集約するためのヘルパ。
    /// 呼び出し側 (`start_loading_items` / `replace_search_view_items`) はこの後で
    /// セッションの残り (キャッシュ clear 等) を進める。世代 bump を忘れると、旧
    /// ワーカーの ThumbMsg が新 items の同じ idx に適用されてサムネが化ける race
    /// が起きる。
    pub(crate) fn install_new_items(
        &mut self,
        items: Vec<GridItem>,
        image_metas: Vec<Option<(i64, i64)>>,
    ) {
        debug_assert_eq!(items.len(), image_metas.len());
        self.items = items;
        self.image_metas = image_metas;
        self.thumbnails = (0..self.items.len())
            .map(|_| ThumbnailState::Pending)
            .collect();
        self.items_generation = self.items_generation.wrapping_add(1);
        // 既定は「実ビュー」。`replace_search_view_items` (= 合成ビュー経路) は
        // この後で true に上書きする。
        self.items_are_global_search_view = false;
    }

    /// items の idx 参照がまとめて無効になった後に呼ぶ共通クリーンアップ。
    ///
    /// 呼び出し元: `remove_items_batch` (削除完了時の idx シフト) と
    /// `replace_search_view_items` (Ctrl+G 結果差し替え)。事前条件として
    /// 呼び出し側で `items_generation` の bump 済みであること (そうしないと
    /// 旧ワーカーの `ThumbMsg` が新 items の同じ idx に着地してサムネが化ける)。
    ///
    /// クリーンアップ内容:
    /// - requested / pending_finalize / texture_backlog / fs_upload_backlog: idx を含む
    ///   キューイング状態
    /// - keep_range / keep_set / keep_start_shared / keep_end_shared: ワーカーの
    ///   in_range 判定境界。新しい update_keep_range_and_requests が次フレームで
    ///   正しい値を入れ直すまで保守的に 0 にしておく
    /// - idx-keyed HashMap 群 (rotation / rating / adjustment / thumb_pixels / ai_* / fs_*)
    /// - in-flight pending (fs_pending / ai_upscale_pending) のキャンセル
    /// - reload_queue / heavy_io_queue の排水: 旧 idx 向け重い I/O (ZIP/PDF/Folder 代表画)
    ///   が worker スロットを占有し続けるのを防ぐ。次フレームの
    ///   `update_keep_range_and_requests` が新 items に対応した request を再投入する
    ///
    /// items / thumbnails / image_metas / search_filter / selected / checked /
    /// scroll_offset_y / visible_indices / path-keyed キャッシュは呼び出し元の責務。
    pub(crate) fn invalidate_idx_state_and_queues(&mut self) {
        use std::sync::atomic::Ordering;

        self.requested.clear();
        self.pending_finalize.clear();
        self.texture_backlog.clear();
        self.checked.clear();
        self.keep_range = (0, 0);
        self.keep_set.clear();
        self.keep_start_shared.store(0, Ordering::Relaxed);
        self.keep_end_shared.store(0, Ordering::Relaxed);

        // idx-keyed caches (HashMap<usize, ...>)
        // 注: `adjustment_page_params` / `mask_pages` は DB ロード済みの「ユーザーが付けた」
        // idx-keyed 状態なので、ここでは触らない。削除経路では呼び出し元が idx shift し、
        // 差し替え経路 (Ctrl+G) では呼び出し元が明示的に clear する。
        self.rotation_cache.clear();
        self.rating_cache.clear();
        self.adjustment_cache.clear();
        self.thumb_pixels.clear();
        self.thumb_adjust_tex.clear();
        self.ai_upscale_cache.clear();
        self.ai_upscale_failed.clear();
        self.ai_classify_cache.clear();
        self.erase_base_cache.clear();
        self.fs_early_dims.clear();
        self.fs_cache.clear();
        self.fs_upload_backlog.clear();

        // in-flight pending (idx-keyed) をキャンセル
        for (cancel, _, _) in self.fs_pending.values() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.fs_pending.clear();
        for (cancel, _) in self.ai_upscale_pending.values() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.ai_upscale_pending.clear();

        // タグプリウォーム: idx-keyed な queued 集合 + worker handle。worker は旧 idx の
        // 画像パスを参照し続けるので、items 差し替え・削除のどちらでも取り消す。
        // 再 spawn は呼び出し元 (replace_search_view_items / 削除まとめ後) の責務。
        if let Some(pending) = self.tag_prewarm_pending.take() {
            pending.cancel();
        }
        self.tag_prewarm_queued.clear();

        // キューに残った旧 idx リクエストを排水。items_gen 差異で最終的には破棄されるが、
        // worker が pop した直後は decode を走らせ始めてしまうので、明示的に捨てる。
        if let Some(ref q) = self.reload_queue {
            if let Ok(mut guard) = q.0.lock() {
                guard.clear();
            }
        }
        if let Some(ref q) = self.heavy_io_queue {
            if let Ok(mut guard) = q.0.lock() {
                guard.clear();
            }
        }
    }

    /// 複数 idx をまとめて items から取り除く共通ルーチン。
    ///
    /// `sorted_desc_idxs` は降順ソート済み・重複なしの idx 配列を期待する。
    /// 大量削除 (★1 画像数千件一括削除など) で 1 件ずつ処理すると
    /// `adjustment_page_params` の再構築が O(N·M) になってしまうため、
    /// `partition_point` を使った O(K log K) の idx shift で一括処理する。
    ///
    /// 事前条件: `sorted_desc_idxs` は降順・重複なし・すべて `self.items.len()` 未満。
    pub(crate) fn remove_items_batch(&mut self, sorted_desc_idxs: &[usize]) {
        if sorted_desc_idxs.is_empty() {
            return;
        }

        // 物理 shift: 降順なので items.remove(i) の再 shift は発生しない。
        for &i in sorted_desc_idxs {
            if i < self.items.len() {
                self.items.remove(i);
            }
            if i < self.thumbnails.len() {
                self.thumbnails.remove(i);
            }
            if i < self.image_metas.len() {
                self.image_metas.remove(i);
            }
        }
        self.items_generation = self.items_generation.wrapping_add(1);

        // 各残存 old_idx に対する new_idx を partition_point で O(log K) 算出。
        // 削除 idx 集合は降順入力だが、partition_point には昇順が要るので一度昇順化する。
        // 削除済み判定も partition_point の位置で `sorted_asc[p] == old` を見れば済み、
        // HashSet<usize> を別途作る必要はない。
        let mut sorted_asc: Vec<usize> = sorted_desc_idxs.to_vec();
        sorted_asc.sort_unstable();
        let shift = |old: usize| -> Option<usize> {
            let p = sorted_asc.partition_point(|&x| x < old);
            if sorted_asc.get(p) == Some(&old) {
                return None;
            }
            Some(old - p)
        };

        if let Some(ref mut filter) = self.search_filter {
            let new_filter: std::collections::HashSet<usize> =
                filter.iter().filter_map(|&i| shift(i)).collect();
            *filter = new_filter;
        }

        // adjustment_page_params / mask_pages は DB 復元済みのユーザ設定なので
        // clear せず idx shift で残存ページの分を保持する。
        self.adjustment_page_params = std::mem::take(&mut self.adjustment_page_params)
            .into_iter()
            .filter_map(|(i, v)| shift(i).map(|ni| (ni, v)))
            .collect();
        self.mask_pages = self.mask_pages.iter().filter_map(|&i| shift(i)).collect();

        // selected の詰め動作: `sel - count(removed idx < sel)` は残存 / 削除どちらの
        // ケースでも「old idx の位置に収まる新 idx」(= 繰り上がった次 item) を返す。
        //   - sel が残存: new_idx = sel - p (shift と同じ)
        //   - sel が削除対象: そのスロットは次の surviving item が詰まるので同じく sel - p
        // 末尾を削除したケース (sel - p が新 len を超える) は後続の `sel >= n` clamp で
        // n-1 にフォールバックする。
        // 例: [a,b,c,d,e,f] で selected=3(d), 削除=[1,3] → p=1, new_sel = 3-1 = 2 = e。
        self.selected = self
            .selected
            .map(|sel| sel - sorted_asc.partition_point(|&x| x < sel));

        self.invalidate_idx_state_and_queues();

        let n = self.items.len();
        if n == 0 {
            self.selected = None;
        } else if let Some(sel) = self.selected {
            if sel >= n {
                self.selected = Some(n - 1);
            }
        }

        self.rebuild_visible_indices();
    }

    /// 削除ワーカーを spawn し、`delete_pending` に保持する。
    /// 既に実行中の場合は何もしない (UI 側でダイアログ表示中はボタン無効化する前提)。
    pub(crate) fn start_delete_files(&mut self, paths: Vec<std::path::PathBuf>) {
        if paths.is_empty() || self.delete_pending.is_some() {
            return;
        }
        // 削除中に items を差し替える恐れのある in-flight pending を停止。poll_delete_pending
        // は path で現 items に引き直すので世代一致は厳密には不要だが、余計な再描画・
        // キャッシュ無効化を避けるため事前に静かにしておく。
        if let Some(p) = self.search_pending.take() {
            p.cancel();
        }
        if let Some(p) = self.favsearch_pending.take() {
            p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.global_search.pending = None;
        self.delete_pending = Some(crate::delete_worker::spawn(paths));
    }

    /// 毎フレーム `delete_pending` の進捗メッセージを受信する。
    ///
    /// 進捗バッチを受けるたびに `succeeded` / `failed` / `processed` を更新し、
    /// `DeleteMsg::Done` 受信で items への反映 (成功 path を現在の items から引き直して
    /// `remove_items_batch`) + `prewarm_grid_tags` 再起動 + ダイアログクローズを行う。
    ///
    /// 完了時に `items_generation` が開始時と変わっていれば items への反映をスキップする
    /// (フォルダ切替などで items が入れ替わった場合、ゴミ箱移動済みの path を現在の items
    /// から引こうとしても自然に空振るが、早期 return で無駄処理を省く)。
    pub(crate) fn poll_delete_pending(&mut self) {
        let Some(pending) = self.delete_pending.as_mut() else {
            return;
        };
        let mut done = false;
        let mut canceled = false;
        loop {
            match pending.rx.try_recv() {
                Ok(crate::delete_worker::DeleteMsg::Batch { succeeded, failed }) => {
                    pending.succeeded.extend(succeeded);
                    pending.failed.extend(failed);
                }
                Ok(crate::delete_worker::DeleteMsg::Done { canceled: c }) => {
                    done = true;
                    canceled = c;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    done = true;
                    break;
                }
            }
        }

        if !done {
            return;
        }

        let pending = self.delete_pending.take().expect("guarded above");
        let succeeded = pending.succeeded;
        let failed_count = pending.failed.len();

        crate::logger::log(format!(
            "[delete] done: canceled={canceled} succeeded={} failed={failed_count}",
            succeeded.len(),
        ));

        if failed_count > 0 {
            let first = pending.failed.first().map(|(p, m)| format!("{}: {m}", p.display()));
            crate::logger::log(format!(
                "[delete] first failed = {}",
                first.unwrap_or_default()
            ));
            self.show_feedback_toast(format!("{} 件の削除に失敗しました", failed_count));
        }

        if succeeded.is_empty() {
            return;
        }

        // 成功 path を現在の items から引き直して idx 配列を作る。
        // items が途中で入れ替わっても (Ctrl+G 結果差し替え等) 現 items に残っている分だけ
        // 引き当てられる。items_generation の一致チェックは不要 (空振り最適化のためだけの
        // 早期 return を入れると、Ctrl+G 結果に削除済み path が含まれていたとき反映漏れになる)。
        let success_set: std::collections::HashSet<&std::path::Path> =
            succeeded.iter().map(|p| p.as_path()).collect();
        let mut idxs: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                crate::grid_item::GridItem::Image(p)
                | crate::grid_item::GridItem::Video(p)
                | crate::grid_item::GridItem::ZipFile(p)
                | crate::grid_item::GridItem::PdfFile(p) => {
                    if success_set.contains(p.as_path()) {
                        Some(i)
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect();
        idxs.sort_unstable_by(|a, b| b.cmp(a));
        if idxs.is_empty() {
            return;
        }
        self.remove_items_batch(&idxs);
        self.prewarm_grid_tags();

        // 削除自身がフォルダ mtime を更新しているので、`current_folder_last_mtime` を
        // 新しい値に進めて `check_external_folder_changes` の自動再読込をスキップさせる。
        // これを怠ると次フレームで削除済み path を grid から抜いたのとほぼ同じ結果を
        // `load_folder()` で再計算することになり、数千件削除直後に UI が再度ブロックする。
        // また、stale な `current_folder_signature` (削除前の状態) が残ると後の
        // 外部 mtime 変更時に「内容変化あり」と誤判定して reload が走るので無効化する。
        // 次の reload でフル再走査されてシグネチャが新しく構築される。
        if let Some(folder) = self.current_folder.clone() {
            if let Ok(meta) = folder.metadata() {
                if meta.is_dir() {
                    if let Ok(new_mtime) = meta.modified() {
                        self.current_folder_last_mtime = Some(new_mtime);
                    }
                }
            }
        }
        self.current_folder_signature = None;
    }

    /// PowerShell ペースト worker の完了を拾い、完了ごとに `pending_reload` を立てる。
    /// worker はデタッチ実行なので受信チャネル Disconnected == 完了とみなす (send か drop いずれも).
    pub(crate) fn poll_paste_pending(&mut self) {
        if self.paste_pending.is_empty() {
            return;
        }
        let before = self.paste_pending.len();
        self.paste_pending.retain(|rx| match rx.try_recv() {
            Ok(()) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
        });
        if self.paste_pending.len() < before {
            self.pending_reload = true;
        }
    }

    /// 変換済みアーカイブキャッシュ管理ダイアログのワーカー完了を拾う。
    /// Rows 結果はダイアログ表示用、Deleted* 結果はメッセージ反映 + 再ロード spawn。
    pub(crate) fn poll_archive_cache_maint_pending(&mut self) {
        let Some(pending) = self.archive_cache_maint_pending.as_ref() else {
            return;
        };
        let msg = match pending.rx.try_recv() {
            Ok(m) => m,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.archive_cache_maint_pending = None;
                return;
            }
        };
        self.archive_cache_maint_pending = None;
        match msg {
            crate::cache_maintenance::ArchiveMaintResult::Rows {
                entries,
                total_bytes,
            } => {
                self.archive_cache_rows = Some(entries);
                self.archive_cache_total_bytes = total_bytes;
                self.archive_cache_selection.clear();
            }
            crate::cache_maintenance::ArchiveMaintResult::DeletedSelected { removed } => {
                if removed > 0 {
                    self.archive_cache_manager_result =
                        Some(format!("{} 件のキャッシュを削除しました。", removed));
                }
                self.reload_archive_cache_rows();
            }
            crate::cache_maintenance::ArchiveMaintResult::DeletedMissing { removed } => {
                self.archive_cache_manager_result = Some(format!(
                    "{} 件のキャッシュ (元ファイル消失) を削除しました。",
                    removed
                ));
                self.reload_archive_cache_rows();
            }
            crate::cache_maintenance::ArchiveMaintResult::DeletedAll { removed } => {
                self.archive_cache_manager_result =
                    Some(format!("{} 件のキャッシュを削除しました。", removed));
                self.reload_archive_cache_rows();
            }
            crate::cache_maintenance::ArchiveMaintResult::Error(e) => {
                self.archive_cache_manager_result = Some(format!("操作に失敗しました: {e}"));
            }
        }
    }

    /// archive_cache_rows を再ロードするワーカーを spawn する (既存ハンドルがあれば上書き)。
    pub(crate) fn reload_archive_cache_rows(&mut self) {
        let Some(db) = self.archive_cache_db.clone() else {
            return;
        };
        self.archive_cache_rows = None;
        self.archive_cache_selection.clear();
        self.archive_cache_maint_pending = Some(crate::cache_maintenance::spawn_archive(
            crate::cache_maintenance::ArchiveMaintTask::LoadRows,
            db,
        ));
    }

    /// サムネイルキャッシュ管理ダイアログの集計 / 削除ワーカーの完了を拾う。
    /// 受信したら stats / result を反映してハンドルを drop。
    pub(crate) fn poll_cache_maint_pending(&mut self) {
        let Some(pending) = self.cache_maint_pending.as_ref() else {
            return;
        };
        let msg = match pending.rx.try_recv() {
            Ok(m) => m,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.cache_maint_pending = None;
                return;
            }
        };
        match msg {
            crate::cache_maintenance::CacheMaintResult::Stats { folders, bytes } => {
                self.cache_manager_stats = Some((folders, bytes));
            }
            crate::cache_maintenance::CacheMaintResult::DeleteOldDone {
                deleted,
                new_stats,
            } => {
                self.cache_manager_stats = Some(new_stats);
                self.cache_manager_result =
                    Some(format!("{} 件のキャッシュを削除しました。", deleted));
            }
            crate::cache_maintenance::CacheMaintResult::DeleteAllDone => {
                self.cache_manager_stats = Some((0, 0));
                self.cache_manager_result = Some("すべてのキャッシュを削除しました。".to_string());
            }
            crate::cache_maintenance::CacheMaintResult::DeleteFolderDone {
                existed,
                folder_name,
                new_stats,
            } => {
                self.cache_manager_stats = Some(new_stats);
                self.cache_manager_result = Some(if existed {
                    format!("「{folder_name}」のキャッシュを削除しました。")
                } else {
                    "現在のフォルダにはキャッシュがありません。".to_string()
                });
            }
        }
        self.cache_maint_pending = None;
    }

    /// condvar.wait() 中の全ワーカーを起床させる。
    /// cancel_token を true にした直後に呼び、ワーカーが即座にキャンセルを検知できるようにする。
    pub(crate) fn wake_all_workers(&self) {
        if let Some(ref q) = self.reload_queue {
            let (_, ref cvar) = **q;
            cvar.notify_all();
        }
        if let Some(ref q) = self.heavy_io_queue {
            let (_, ref cvar) = **q;
            cvar.notify_all();
        }
    }

    /// 永続サムネイルワーカープールを `parallelism.thread_count()` 個 spawn する。
    /// 各ワーカーは `reload_queue` を `scroll_hint` 優先度で消費し続け、
    /// `cancel` が立つまで動作する。
    fn spawn_thumbnail_workers(
        &self,
        tx: &mpsc::Sender<ThumbMsg>,
        cancel: Arc<AtomicBool>,
        reload_queue: Arc<NotifyQueue>,
        heavy_io_queue: Arc<NotifyQueue>,
        cache_map: Arc<
            std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        >,
        catalog_arc: Option<Arc<crate::catalog::CatalogDb>>,
    ) {
        let total_threads = self.settings.parallelism.thread_count();
        // I/O ワーカー数: 2 本 (HDD シーク競合と並列性のバランス)
        // ただし全体が 4 本以下なら 1 本に制限
        let io_threads = if total_threads <= 4 { 1 } else { 2 };
        let regular_threads = total_threads.saturating_sub(io_threads).max(1);
        let thumb_px = self.settings.thumb_px;
        let thumb_quality = self.settings.thumb_quality;
        let cache_decision = CacheDecision::from_settings(&self.settings);
        let scroll_hint = Arc::clone(&self.scroll_hint);
        let display_px_shared = Arc::clone(&self.display_px_shared);
        let stats = Arc::clone(&self.stats);
        let cache_gen_done = Arc::clone(&self.cache_gen_done);
        let keep_start_shared = Arc::clone(&self.keep_start_shared);
        let keep_end_shared = Arc::clone(&self.keep_end_shared);
        let visible_end_shared = Arc::clone(&self.visible_end_shared);

        crate::logger::log(format!(
            "  spawning {} regular + {} I/O workers",
            regular_threads, io_threads,
        ));

        // ── 共通のワーカーループ本体 ──
        // queue を受け取り、priority 順に取り出して process_load_request を呼ぶ。
        let spawn_worker = |worker_idx: usize, prefix: &str, queue: Arc<NotifyQueue>| {
            let tx_w = tx.clone();
            let cancel_w = Arc::clone(&cancel);
            let hint_w = Arc::clone(&scroll_hint);
            let cache_map_w = Arc::clone(&cache_map);
            let catalog_w = catalog_arc.clone();
            let done_w = Arc::clone(&cache_gen_done);
            let display_px_w = Arc::clone(&display_px_shared);
            let stats_w = Arc::clone(&stats);
            let ks_w = Arc::clone(&keep_start_shared);
            let ke_w = Arc::clone(&keep_end_shared);
            let ve_w = Arc::clone(&visible_end_shared);
            let tag = format!("{prefix}{worker_idx}");

            std::thread::spawn(move || {
                let (ref mtx, ref cvar) = *queue;
                crate::logger::log(format!("  {tag} started"));
                loop {
                    // priority (可視範囲) を最優先、次に scroll_hint に近い順。
                    // キューが空なら condvar で待機し、push 側の notify で起床する。
                    let req = {
                        let mut q = mtx.lock().unwrap();
                        loop {
                            if cancel_w.load(Ordering::Relaxed) {
                                break None;
                            }
                            if !q.is_empty() {
                                let vis = hint_w.load(Ordering::Relaxed);
                                let vis_end = ve_w.load(Ordering::Relaxed);
                                let best = q
                                    .iter()
                                    .enumerate()
                                    .min_by_key(|(_, r)| {
                                        crate::thumb_loader::worker_priority_key(
                                            r.priority, r.idx, vis, vis_end,
                                        )
                                    })
                                    .map(|(pos, _)| pos)
                                    .unwrap();
                                break Some(q.swap_remove(best));
                            }
                            // キューが空 → condvar で待機（spurious wakeup は外側ループで再チェック）
                            q = cvar.wait(q).unwrap();
                        }
                    };

                    let Some(req) = req else {
                        break;
                    };

                    let ks = ks_w.load(Ordering::Relaxed);
                    let ke = ke_w.load(Ordering::Relaxed);
                    if req.idx < ks || req.idx >= ke {
                        crate::logger::log(format!(
                            "  {tag} SKIP idx={:>4} (out of keep [{ks}..{ke}))  {}",
                            req.idx,
                            req.path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                        ));
                        if crate::perf::is_enabled() {
                            crate::perf::event(
                                "thumb",
                                "skip",
                                None,
                                req.input_seq,
                                &[
                                    ("idx", serde_json::Value::from(req.idx)),
                                    ("reason", serde_json::Value::from("out_of_keep")),
                                ],
                            );
                        }
                        // canceled=true を送信: UI 側の requested を cleanup し、
                        // Evicted (retriable) に戻す。これを送らないと keep_range が
                        // 戻ったときに再エンキューされず idx がスタックする。
                        let _ = tx_w.send(crate::thumb_loader::ThumbMsg {
                            idx: req.idx,
                            image: None,
                            from_cache: false,
                            source_dims: None,
                            canceled: true,
                            finalized: false,
                            input_seq: req.input_seq,
                            items_gen: req.items_gen,
                        });
                        continue;
                    }
                    let vis = hint_w.load(Ordering::Relaxed);
                    let dist = if req.idx < vis {
                        vis - req.idx
                    } else {
                        req.idx - vis
                    };
                    crate::logger::log(format!(
                        "  {tag} pick idx={:>4} pri={} dist={dist:>4}  {}",
                        req.idx,
                        if req.priority { "H" } else { "L" },
                        req.path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                    ));
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "thumb",
                            "pick",
                            None,
                            req.input_seq,
                            &[
                                ("idx", serde_json::Value::from(req.idx)),
                                ("priority", serde_json::Value::from(req.priority)),
                                ("dist", serde_json::Value::from(dist)),
                                ("worker", serde_json::Value::from(tag.clone())),
                            ],
                        );
                    }
                    let display_px = display_px_w.load(Ordering::Relaxed);
                    process_load_request(
                        &req,
                        &cache_map_w,
                        &tx_w,
                        catalog_w.as_deref(),
                        thumb_px,
                        thumb_quality,
                        display_px,
                        cache_decision,
                        &done_w,
                        &stats_w,
                        Some(&cancel_w),
                        &ks_w,
                        &ke_w,
                    );
                }
                crate::logger::log(format!("  {tag} stopped"));
            });
        };

        // 通常ワーカー: reload_queue (Image, ZipImage, PdfPage)
        for i in 0..regular_threads {
            spawn_worker(i, "w", Arc::clone(&reload_queue));
        }
        // I/O ワーカー: heavy_io_queue (ZipFile, PdfFile, Folder)
        for i in 0..io_threads {
            spawn_worker(i, "io", Arc::clone(&heavy_io_queue));
        }
    }

    /// 動画サムネイル取得スレッドを起動する。
    /// 各動画について Windows Shell API でサムネを取り出し、tx 経由で UI に送信する。
    ///
    /// 取得順は固定ではなく、毎回 `scroll_hint` / `visible_end_shared` を見て
    /// 現在の可視範囲に最も近い動画を優先する。動画が多いフォルダで下にスクロール
    /// しても、表示中ページの動画が先に埋まるようにする。
    fn spawn_video_thread(
        &self,
        tx: mpsc::Sender<ThumbMsg>,
        cancel: Arc<AtomicBool>,
        video_items: Vec<(usize, PathBuf, u64)>,
        thumb_overrides: std::collections::HashMap<String, PathBuf>,
    ) {
        let thumb_size = self.last_cell_size.max(256.0) as i32;
        let display_px = compute_display_px(
            self.last_cell_size,
            self.last_cell_h,
            self.last_pixels_per_point,
        );
        let stats = Arc::clone(&self.stats);
        let hint = Arc::clone(&self.scroll_hint);
        let vis_end = Arc::clone(&self.visible_end_shared);
        // 世代スナップショット: items が差し替わる前にフリーズ。以降 ThumbMsg に載せ、
        // UI 側は自 items_generation と一致しないものを破棄する (旧 items 混入防止)。
        let items_gen = self.items_generation;

        std::thread::spawn(move || {
            use std::time::{Duration, Instant};

            struct PendingVideo {
                idx: usize,
                path: PathBuf,
                file_size: u64,
                retries: u32,
                next_attempt: Instant,
            }

            // Shell がまだサムネ抽出中の動画に対して `SIIGBF_THUMBNAILONLY` は
            // エラーを返す (= ここで None)。バックオフしながらリトライすると、
            // Windows のバックグラウンド抽出完了後に本物のサムネが取れる。
            //
            // 動画本数が多いフォルダでは Shell の抽出キューが詰まり、特定の
            // ファイルに対して 30〜60 秒以上「未抽出」を返し続けるケースが実測で
            // 見つかった (バッチ内の他ファイルは成功するので Shell 自体は生きている)。
            // そこで以下の 2 段構えにする:
            //
            // 1. 連続失敗は 8 回 (最大 25.5 秒) までは指数バックオフで待つ。
            // 2. 他の動画が 1 件でも成功したら、同じフォルダ内の全残アイテムの
            //    retries を 0 にリセットし next_attempt を now に戻す。Shell が
            //    前進している限り何度でもリトライする (= 他動画が成功し続ける限り
            //    実質無制限)。
            // 3. スレッド全体で 1 件も成功していない場合は (1) の上限で打ち切る。
            const MAX_CONSEC_RETRIES: u32 = 8;

            let override_count = video_items
                .iter()
                .filter(|(_, p, _)| thumb_overrides.contains_key(&stem_lower(p)))
                .count();
            crate::logger::log(format!(
                "[video thread] start: {} videos (override={}, shell={}), thumb_size={}, display_px={}",
                video_items.len(),
                override_count,
                video_items.len() - override_count,
                thumb_size,
                display_px,
            ));

            let now0 = Instant::now();
            let mut remaining: Vec<PendingVideo> = video_items
                .into_iter()
                .map(|(idx, path, file_size)| PendingVideo {
                    idx,
                    path,
                    file_size,
                    retries: 0,
                    next_attempt: now0,
                })
                .collect();
            let mut success_count = 0u32;
            let mut fail_count = 0u32;

            while !remaining.is_empty() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }

                // worker_priority_key と同じロジックで可視アイテムを tier 0、
                // その他を距離順に並べる。next_attempt が未来のアイテムは除外。
                let now = Instant::now();
                let vis = hint.load(Ordering::Relaxed);
                let ve = vis_end.load(Ordering::Relaxed);
                let best_pos = remaining
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| v.next_attempt <= now)
                    .min_by_key(|(_, v)| {
                        let priority = v.idx >= vis && v.idx < ve;
                        crate::thumb_loader::worker_priority_key(priority, v.idx, vis, ve)
                    })
                    .map(|(i, _)| i);

                let Some(best_pos) = best_pos else {
                    // 全件リトライ待機中: 最も近い next_attempt まで寝る。
                    // キャンセル応答のため 200ms 刻みで区切る。
                    let earliest = remaining
                        .iter()
                        .map(|v| v.next_attempt)
                        .min()
                        .expect("remaining is non-empty");
                    let delay = earliest
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(200));
                    std::thread::sleep(delay);
                    continue;
                };
                let PendingVideo {
                    idx,
                    path,
                    file_size,
                    retries,
                    ..
                } = remaining.swap_remove(best_pos);
                let fname = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string();
                let call_t0 = Instant::now();

                let stem = stem_lower(&path);
                // Phase 5.3 動画サムネイル優先順位 (詳細は docs/video-engine-redesign.md
                // §5.3 / §5.4.1):
                //   1. ユーザーがピン留めしたフレーム (video_pins DB) — ★優先度最上
                //      実装は Phase 5.4.1 で配線する (DB スキーマは crate::video_pins
                //      に用意済)。ここでは fall-through する。
                //   2. 同名ファイル名画像 (sidecar、`thumb_overrides` map) —
                //      Settings.video_thumb_use_sidecar_image で gating される。
                //   3. Windows Shell の動画自身のデフォルトサムネ。
                let (ci, source_tag) = if let Some(img_path) = thumb_overrides.get(&stem) {
                    if retries == 0 {
                        crate::logger::log(format!(
                            "  video thumb override: idx={idx} stem={stem} img={}",
                            img_path.display()
                        ));
                    }
                    (
                        crate::thumb_loader::decode_image_for_thumb(img_path, display_px),
                        "override",
                    )
                } else {
                    let (img, diag) = crate::video_thumb::get_video_thumbnail(&path, thumb_size);
                    // 失敗ケースだけ Shell の生データをログする (毎回出すと動画多数の
                    // フォルダで mimageviewer.log が肥大するため)。成功時は下の
                    // 最終 OK 行に dims を載せる。
                    if img.is_none() {
                        crate::logger::log(format!(
                            "  video shell FAIL: idx={idx} retry={retries} req_sz={thumb_size} \
                             stage={} hr={} get_ms={}  {fname}",
                            diag.stage_label(),
                            diag.hresult_hex(),
                            diag.get_image_ms,
                        ));
                    } else if let (Some((w, h)), Some(avg), Some(span)) =
                        (diag.dims, diag.avg_rgb, diag.span_rgb)
                    {
                        // 成功時は avg/span が極端 (真っ黒疑い) のときだけ注記する。
                        let avg_max = avg.0.max(avg.1).max(avg.2);
                        let span_max = span.0.max(span.1).max(span.2);
                        if avg_max < 16 || span_max < 10 {
                            crate::logger::log(format!(
                                "  video shell suspicious: idx={idx} dims={w}x{h} \
                                 avg_rgb=[{},{},{}] span_rgb=[{},{},{}]  {fname}",
                                avg.0, avg.1, avg.2, span.0, span.1, span.2,
                            ));
                        }
                    }
                    (img, "shell")
                };
                let call_ms = call_t0.elapsed().as_millis();

                if ci.is_none() && retries < MAX_CONSEC_RETRIES {
                    // Shell がサムネ抽出中。バックオフして末尾にリトライ予約。
                    // 200ms → 400ms → 800ms → 1.6s → 3.2s → 6.4s → 6.4s (頭打ち)
                    let backoff = Duration::from_millis(200u64 << retries.min(5));
                    let next = Instant::now() + backoff;
                    crate::logger::log(format!(
                        "  video thumb retry: idx={idx} src={source_tag} \
                         retries={}/{} backoff={}ms call_ms={call_ms} remaining={}  {fname}",
                        retries + 1,
                        MAX_CONSEC_RETRIES,
                        backoff.as_millis(),
                        remaining.len(),
                    ));
                    remaining.push(PendingVideo {
                        idx,
                        path,
                        file_size,
                        retries: retries + 1,
                        next_attempt: next,
                    });
                    continue;
                }

                if ci.is_some() {
                    // 成功: 残アイテムの retries をリセットし即時再試行を許可する。
                    // Shell が反応している証拠 = 他のアイテムも抽出が進んでいる可能性が高い。
                    let reset_count = remaining.iter().filter(|v| v.retries > 0).count();
                    if reset_count > 0 {
                        crate::logger::log(format!(
                            "  video thumb progress: reset {reset_count} retry counters  (trigger idx={idx})"
                        ));
                        for v in remaining.iter_mut() {
                            v.retries = 0;
                            v.next_attempt = now;
                        }
                    }
                }

                crate::logger::log(format!(
                    "  video thumb {}: idx={idx} src={source_tag} vis={vis} ve={ve} \
                     retries={retries} call_ms={call_ms} remaining={}  {fname}",
                    if ci.is_some() { "OK" } else { "FAIL-final" },
                    remaining.len(),
                ));
                if ci.is_some() {
                    success_count += 1;
                    if let Ok(mut s) = stats.lock() {
                        s.record_video(file_size);
                    }
                } else {
                    fail_count += 1;
                    if let Ok(mut s) = stats.lock() {
                        s.record_failed();
                    }
                }
                // 動画 Shell API はアップグレード経路を持たないので from_cache = false。
                // ピクセル寸法は取得できない。動画ロードは LoadRequest を経由しないため
                // input_seq は動画スレッドでは未使用 (計装経路でエンキューされない)。
                let _ = tx.send(crate::thumb_loader::ThumbMsg {
                    idx,
                    image: ci,
                    from_cache: false,
                    source_dims: None,
                    canceled: false,
                    finalized: false,
                    input_seq: 0,
                    items_gen,
                });
            }
            crate::logger::log(format!(
                "[video thread] end: success={success_count} fail={fail_count} canceled={}",
                cancel.load(Ordering::Relaxed),
            ));
        });
    }

    fn poll_thumbnails(&mut self, ctx: &egui::Context) {
        // 1 フレームあたりのテクスチャ生成数を制限する。
        // load_texture は GPU テクスチャアップロードを伴い、1 枚 0.5-2 ms かかる。
        // キャッシュヒット時にワーカー全員が一気に結果を返すと 1 フレームで
        // 数十枚の upload が走りフレーム落ちするため、上限を設ける。
        // 上限超過分は texture_backlog に ColorImage のまま保持し次フレームで処理する。
        const MAX_TEXTURES_PER_FRAME: u32 = 8;
        let mut textures_created = 0u32;
        let mut received = 0u32;
        let (keep_start, keep_end) = self.keep_range;

        // バックログ + チャネルから受信した結果を統合して処理する。
        // バックログを先に処理（既にデコード済みなので優先）。
        // ループ内で `self` を mutably 借りる経路 (writeback 等) があるため、
        // ここで Vec に collect してから iterate する (借用衝突回避)。
        let backlog = std::mem::take(&mut self.texture_backlog);
        let drain: Vec<_> = backlog
            .into_iter()
            .chain(std::iter::from_fn(|| self.rx.try_recv().ok()))
            .collect();

        for msg in drain {
            let crate::thumb_loader::ThumbMsg {
                idx: i,
                image: color_image_opt,
                from_cache,
                source_dims,
                canceled,
                finalized,
                input_seq: req_input_seq,
                items_gen: msg_items_gen,
            } = msg;
            // 世代不一致 (旧 items 由来) のメッセージは破棄する。items 差し替え後に
            // 旧ワーカー結果が同じ idx の新 items に適用されるとサムネが化けるため。
            // 全エンキュー経路 (通常 / idle upgrade / 動画スレッド) で現世代をスナップ
            // ショットして載せるので、不一致 = 旧経路と判定してよい。
            if msg_items_gen != self.items_generation {
                continue;
            }
            if i >= self.thumbnails.len() {
                self.requested.remove(&i);
                continue;
            }

            let in_keep_range = self.keep_set.contains(&i);

            // finalized = true: 第 2 シグナル (decode 成功 + cache save 完了)。
            // **`thumbnails[i]` の状態は変更しない**。
            //
            // 状態別の処理:
            //   - Loaded: 正常完了 → requested.remove。
            //   - Pending: 第1シグナルが texture_backlog でアップロード待ち中。
            //     requested をここで抜くと次フレームに Pending && !requested.contains
            //     で再エンキュー → 同じ画像を何度も decode する無限ループになる。
            //     `pending_finalize` に idx を積み、アップロード完了時 (Loaded 遷移時)
            //     にまとめて requested を抜く。
            //   - Evicted / Failed: 第1シグナルは既に処理されており (Loaded 経由で
            //     Evicted になった or ワーカーが失敗を返した)、ワーカーもこのシグナルで
            //     終了。ここで requested を抜いておかないと、再スクロールで戻った時に
            //     `update_keep_range_and_requests` の `requested.contains_key` ガードで
            //     再エンキューが無限にブロックされ、サムネが Evicted のまま固着する。
            //
            // `requested` に無い idx への finalized は、第1シグナルが keep 範囲外で
            // drop された後の幽霊シグナル。pending_finalize に挿入すると stale 状態が
            // 残り続けるので無視する。
            if finalized {
                // 仮想フォルダの先頭ページ完成を親 catalog にミラー (PDFium 再レンダ
                // 防止のための writeback)。requested の cleanup より前にやることで、
                // cache_map から WebP データが消える前に確実に読み出せる。
                self.fire_virtual_folder_writeback_if_ready(i);
                if !self.requested.contains_key(&i) {
                    received += 1;
                    continue;
                }
                match self.thumbnails[i] {
                    ThumbnailState::Loaded { .. } => {
                        self.requested.remove(&i);
                    }
                    ThumbnailState::Pending => {
                        self.pending_finalize.insert(i);
                    }
                    ThumbnailState::Evicted | ThumbnailState::Failed => {
                        // 固着防止: Evicted/Failed で finalize が来たら再エンキュー
                        // 可能な状態に戻す。
                        let state_name = match self.thumbnails[i] {
                            ThumbnailState::Evicted => "Evicted",
                            ThumbnailState::Failed => "Failed",
                            _ => unreachable!(),
                        };
                        self.requested.remove(&i);
                        self.pending_finalize.remove(&i);
                        crate::logger::log(format!(
                            "  [poll] finalize on {state_name} idx={i} → cleanup requested (was stuck candidate)"
                        ));
                    }
                }
                received += 1;
                continue;
            }

            // canceled = true: ワーカーが処理を中断 (STALE 等)。
            // Failed にはせず、Evicted に戻して再試行可能にする。
            // Loaded 状態は維持 (既にテクスチャがある場合は破棄しない)。
            if canceled {
                self.requested.remove(&i);
                self.pending_finalize.remove(&i);
                if !matches!(self.thumbnails[i], ThumbnailState::Loaded { .. }) {
                    self.thumbnails[i] = ThumbnailState::Evicted;
                }
                received += 1;
                continue;
            }

            // 動画は「最初に作った動画サムネは以後ずっと保持する」設計 —
            // update_keep_range_and_requests は Video を Evicted 化しないし
            // make_load_request も Video に None を返すため、一度 Evicted に
            // 落とすと再リクエストされず永遠に復帰しない。下の分岐で keep_range
            // 外でも常に in_range 扱いにすることで、out-of-range 受信でも
            // 必ずテクスチャ化する。
            let is_video = matches!(self.items.get(i), Some(GridItem::Video(_)));
            let treat_as_in_range = in_keep_range || is_video;

            match color_image_opt {
                Some(color_image) => {
                    if treat_as_in_range && textures_created < MAX_TEXTURES_PER_FRAME {
                        // from_cache=true: 1 ショット経路 (cache save なし) → 即 remove。
                        // from_cache=false: from-source 経路。cache save 完了後の
                        //   第 2 シグナル (canceled=true) 到着まで `requested` を保持。
                        //   cache save 進行中の再エンキュー + 二重レンダを防ぐ。
                        if from_cache {
                            self.requested.remove(&i);
                        }
                        let [w, h] = color_image.size;
                        let rendered_at_px = w.max(h) as u32;
                        let prev_state_was_loaded =
                            matches!(self.thumbnails[i], ThumbnailState::Loaded { .. });
                        let upload_t0 = std::time::Instant::now();
                        // サムネ補正用に `color_image` を Arc で保持してからテクスチャ化する。
                        // テクスチャアップロードは ColorImage を消費するので、保持用に clone が必要。
                        // 補正対象は Image / ZipImage / PdfPage のみ
                        // (display-pipeline.md §1.5)。動画 / フォルダ / ZIP・PDF
                        // 代表サムネに global_preset が漏れないようゲートする。
                        let retain_pixels_for_adjust = is_thumb_adjust_target(self.items.get(i));
                        let (arc_pixels_opt, image_to_upload) = if retain_pixels_for_adjust {
                            let arc = std::sync::Arc::new(color_image);
                            let img = (*arc).clone();
                            (Some(arc), img)
                        } else {
                            (None, color_image)
                        };
                        let handle = ctx.load_texture(
                            format!("thumb_{i}"),
                            image_to_upload,
                            egui::TextureOptions::LINEAR,
                        );
                        let upload_ms = upload_t0.elapsed().as_secs_f64() * 1000.0;
                        if let Some(arc) = arc_pixels_opt {
                            self.thumb_pixels.insert(i, arc);
                        }
                        // 新しいピクセルに差し替わったので、古い補正済みテクスチャは捨てる。
                        // 次フレームで `maybe_apply_thumb_adjustment` により再生成される。
                        self.thumb_adjust_tex.remove(&i);
                        self.thumbnails[i] = ThumbnailState::Loaded {
                            tex: handle,
                            from_cache,
                            rendered_at_px,
                            source_dims,
                        };
                        textures_created += 1;
                        // 第2シグナル先着の場合の遅延 requested.remove を実行。
                        if self.pending_finalize.remove(&i) {
                            self.requested.remove(&i);
                        }
                        // Pending/Evicted → Loaded の遷移時のみ ready を emit
                        // (アップグレードで Loaded→Loaded の遷移も起きるためフィルタ)。
                        // seq はエンキュー元のを透過し、decode 中のスクロールで
                        // ready が別操作に紐づくのを防ぐ。
                        if crate::perf::is_enabled() && !prev_state_was_loaded {
                            let perf_key = self.perf_item_key(i);
                            crate::perf::event(
                                "thumb",
                                "ready",
                                perf_key.as_deref(),
                                req_input_seq,
                                &[
                                    ("idx", serde_json::Value::from(i)),
                                    ("w", serde_json::Value::from(w)),
                                    ("h", serde_json::Value::from(h)),
                                    ("from_cache", serde_json::Value::from(from_cache)),
                                    ("upload_ms", serde_json::Value::from(upload_ms)),
                                ],
                            );
                        }
                        // ── スクロール体感: vis 内の Pending → Loaded 遷移を計測 ──
                        if !prev_state_was_loaded
                            && i >= self.last_vis_range.0
                            && i < self.last_vis_range.1
                            && let Some(settle_t) = self.vis_settle_at
                        {
                            let elapsed_ms = settle_t.elapsed().as_secs_f64() * 1000.0;
                            if !self.vis_first_logged {
                                crate::logger::log(format!(
                                    "[vis] first_visible idx={i} +{elapsed_ms:.1}ms after settle",
                                ));
                                self.vis_first_logged = true;
                            }
                            if !self.vis_all_logged {
                                let all_loaded = (self.last_vis_range.0..self.last_vis_range.1)
                                    .all(|j| {
                                        matches!(
                                            self.thumbnails.get(j),
                                            Some(ThumbnailState::Loaded { .. })
                                        )
                                    });
                                if all_loaded {
                                    crate::logger::log(format!(
                                        "[vis] all_loaded vis=[{}..{}) +{elapsed_ms:.1}ms after settle",
                                        self.last_vis_range.0, self.last_vis_range.1,
                                    ));
                                    self.vis_all_logged = true;
                                }
                            }
                        }
                    } else if treat_as_in_range {
                        // 上限到達だが keep_range 内 (or 動画): 次フレームに持ち越す。
                        // requested は除去しない (重複リクエスト防止)
                        self.texture_backlog.push(crate::thumb_loader::ThumbMsg {
                            idx: i,
                            image: Some(color_image),
                            from_cache,
                            source_dims,
                            canceled: false,
                            finalized: false,
                            input_seq: req_input_seq,
                            items_gen: msg_items_gen,
                        });
                    } else {
                        // 範囲外: ColorImage を drop し Evicted にしておく。
                        // from_cache / from_source に関わらず、UI 側の要求はここで完了扱い。
                        // from_source 用に requested を残すと、後から届く finalized=true と
                        // 合わせて `pending_finalize` が stale 化し、スクロールで戻っても
                        // `requested.contains_key(&i)` により再エンキューが阻まれる
                        // (= サムネイルが復帰しない) 状態に陥る。
                        // pending_finalize も念のため掃除する (第 2 シグナル先着の場合)。
                        crate::logger::log(format!(
                            "  [main] drop out-of-range thumb: idx={i} keep=[{keep_start}..{keep_end})"
                        ));
                        self.requested.remove(&i);
                        self.pending_finalize.remove(&i);
                        self.thumb_pixels.remove(&i);
                        self.thumb_adjust_tex.remove(&i);
                        self.thumbnails[i] = ThumbnailState::Evicted;
                    }
                }
                None => {
                    self.requested.remove(&i);
                    self.pending_finalize.remove(&i);
                    self.thumbnails[i] = ThumbnailState::Failed;
                }
            }
            received += 1;
        }
        if received > 0 || !self.texture_backlog.is_empty() {
            crate::logger::log(format!(
                "  [main] poll_thumbnails: received {received} ({textures_created} textures, {} backlog)",
                self.texture_backlog.len()
            ));
            ctx.request_repaint();
        }
    }

    /// 段階 B: ページ単位先読み + eviction のメインロジック。
    /// 段階 D: VRAM 安全ネット (上限超過時に keep_range を縮小)。
    ///
    /// 毎フレーム呼ぶ想定。現在のスクロール位置から keep_range を算出し、
    /// 範囲外の Loaded を Evicted 化し、範囲内の Pending/Evicted を reload_queue に push する。
    fn update_keep_range_and_requests(
        &mut self,
        ctx: &egui::Context,
        frame_t0: std::time::Instant,
    ) {
        // 削除進行中は prefetch / eviction 調整も一時停止する。items にはまだ削除対象の
        // path が残っており、worker が keep_range 内の idx を enqueue するとサムネ再デコードが
        // 走って「Failed」表示が出る (ゴミ箱移動済みなので File::open が失敗する)。
        // Modal で入力は止まっているので keep_range は基本動かず、再 enqueue は発生しない想定。
        if self.delete_pending.is_some() {
            return;
        }
        let total = self.items.len();
        if total == 0 {
            // keep_set と worker atomic boundary も同時にクリアしないと、
            // 空フォルダへの再読み込み直後に前フォルダの bbox が worker に残り続け、
            // 古い enqueue 済みリクエストが in-range 判定で処理されてしまう。
            self.keep_range = (0, 0);
            self.keep_set.clear();
            self.keep_start_shared.store(0, Ordering::Relaxed);
            self.keep_end_shared.store(0, Ordering::Relaxed);
            return;
        }

        // 毎フレーム display_px を更新してワーカーに追従させる
        // (列数変更やウィンドウリサイズに対応)
        let current_display_px = compute_display_px(
            self.last_cell_size,
            self.last_cell_h,
            self.last_pixels_per_point,
        );
        self.display_px_shared
            .store(current_display_px, Ordering::Relaxed);

        let cols = self.settings.grid_cols.max(1);
        // 極端に小さい cell_h (1〜数 px) では `viewport_h / cell_h` が数千行に膨れて
        // keep_set が数万 idx に達し、下流の make_load_request / texture_backlog で
        // UI スレッドが固まる。render_grid 側の MIN_CELL_PX=32 と揃える。
        let cell_h = self.last_cell_h.max(32.0);
        let viewport_h = self.last_viewport_h.max(cell_h);

        let rows_per_page = (viewport_h / cell_h).ceil() as usize;
        let items_per_page = (rows_per_page * cols).max(1);
        // 設計: prefetch / eviction 対象は「display list = visible_indices の一部」を
        // そのまま使い、raw idx の連続範囲には**絶対に潰さない**。
        // `visible_indices` が★フィルタや Ctrl+F で疎になったとき、raw range で扱うと
        // 非可視 idx を大量 (フォルダ 1300 件中 3 件表示 → 991 件の非可視) に enqueue
        // してしまう。詳細は docs/async-architecture.md「display list vs filesystem list」。
        let vis_count = self.visible_indices.len();
        // スクロール位置が vis_count を超えるとき (フィルタ直後の縮退ケース) は末尾に寄せる。
        // さらに vis_count=0 は冒頭で早期 return 済みなのでここでは 1 以上。
        let vis_first_raw = (self.scroll_offset_y / cell_h) as usize * cols;
        let vis_first = vis_first_raw.min(vis_count.saturating_sub(1));

        let prev_pages = self.settings.thumb_prev_pages as usize;
        let next_pages = self.settings.thumb_next_pages as usize;

        let vis_keep_start = vis_first.saturating_sub(prev_pages * items_per_page);
        let mut vis_keep_end = vis_first
            .saturating_add((1 + next_pages) * items_per_page)
            .min(vis_count);

        // ── 段階 D: VRAM 安全ネット ──────────────────────────────────
        // display_px から 1 枚あたりの推定バイト数を算出し、cap を超えそうなら
        // visible slice を vis_first 中心に縮小する (前方 2/3 優先、後方 1/3)。
        // 上限は "プライマリ GPU VRAM × 設定 %" (0 で無制限)。
        let mut vis_keep_start_capped = vis_keep_start;
        let cap_percent = self.settings.thumb_vram_cap_percent;
        if cap_percent > 0 {
            let est_per_thumb: u64 = (current_display_px as u64)
                .saturating_mul(current_display_px as u64)
                .saturating_mul(4);
            let cap_bytes = crate::gpu_info::vram_cap_from_percent(cap_percent);
            if est_per_thumb > 0 {
                let max_items = (cap_bytes / est_per_thumb).max(1) as usize;
                let desired = vis_keep_end.saturating_sub(vis_keep_start);
                if max_items < desired {
                    let half_back = max_items / 3;
                    let half_forward = max_items - half_back;
                    vis_keep_start_capped = vis_first.saturating_sub(half_back);
                    vis_keep_end = vis_first.saturating_add(half_forward).min(vis_count);
                    crate::logger::log(format!(
                        "  VRAM cap hit: desired={desired} max_items={max_items} \
                         (display_px={current_display_px} est/thumb={} MB cap={} MB @ {}%)",
                        est_per_thumb / (1024 * 1024),
                        cap_bytes / (1024 * 1024),
                        cap_percent,
                    ));
                }
            }
        }

        // keep_set: prefetch / eviction / retain / idle upgrade がこれを使う。
        let keep_slice = self
            .visible_indices
            .get(vis_keep_start_capped..vis_keep_end)
            .unwrap_or(&[]);
        self.keep_set.clear();
        self.keep_set.extend(keep_slice.iter().copied());

        // keep_range: keep_set の bounding box。worker atomic キャンセル判定で使われる。
        // `visible_indices` は `rebuild_visible_indices` が `for i in 0..n { push(i) }` で
        // 構築するため昇順。その部分列である `keep_slice` も昇順なので、min/max は
        // 端の要素を直接参照すれば O(1)。将来 display list をソート以外の順で構築する
        // 改修が入るときはこの前提ごと再考すること。
        let (keep_start, keep_end) = match (keep_slice.first(), keep_slice.last()) {
            (Some(&mn), Some(&mx)) => (mn, (mx + 1).min(total)),
            _ => (0, 0),
        };
        self.keep_range = (keep_start, keep_end);
        self.keep_start_shared.store(keep_start, Ordering::Relaxed);
        self.keep_end_shared.store(keep_end, Ordering::Relaxed);

        // (1) 範囲外の Loaded を Evicted にする (TextureHandle を drop)
        //     動画サムネイルは一度ロードしたら維持する (別パスのため再要求できない)
        //
        //     重要: ここでは `requested` を触らない。ワーカーが処理中の idx を
        //     requested から抜くと、scroll 戻り時に同じ idx が再エンキューされ、
        //     二重レンダ (特に PDF) を引き起こすため。
        //     requested の cleanup は以下で行う:
        //       - エンキュー済・pop 前の取消: 下の q.retain が dropped idx を remove
        //       - ワーカー pop 後の STALE: worker が canceled=true を送信し
        //         poll_thumbnails が remove
        //       - 正常完了: poll_thumbnails が remove
        let t1 = frame_t0.elapsed();
        for i in 0..total {
            if self.keep_set.contains(&i) {
                continue;
            }
            if matches!(self.items.get(i), Some(GridItem::Video(_))) {
                continue;
            }
            if matches!(self.thumbnails[i], ThumbnailState::Loaded { .. }) {
                self.thumbnails[i] = ThumbnailState::Evicted;
            }
            // サムネ補正用ピクセル/テクスチャも keep_set 外では破棄する。
            // 動画は上で skip 済みなのでここには来ない。
            self.thumb_pixels.remove(&i);
            self.thumb_adjust_tex.remove(&i);
        }
        let t2 = frame_t0.elapsed();

        // (2) reload_queue 内の keep_range 外リクエストを除去し、
        //     範囲内の Pending/Evicted を新たに push する。
        //     スクロール中にキューに溜まった古いリクエストをワーカーが無駄に
        //     処理するのを防ぎ、新しい可視領域のリクエストを優先させる。
        //
        //     可視範囲 (1 ページ分) のリクエストは priority=true でマークし、
        //     ワーカーが先読み要求より常に先に処理するようにする。
        let Some(queue_arc) = self.reload_queue.clone() else {
            return;
        };

        // 可視範囲の raw index 範囲を計算 (1 ページ分 + 上下 1 行のマージン)
        let vis_visible_start = vis_first.saturating_sub(cols);
        let vis_visible_end = vis_first
            .saturating_add(items_per_page + cols)
            .min(vis_count);
        let visible_raw_start = self
            .visible_indices
            .get(vis_visible_start)
            .copied()
            .unwrap_or(0);
        let visible_raw_end = self
            .visible_indices
            .get(vis_visible_end.saturating_sub(1))
            .copied()
            .map(|i| i + 1)
            .unwrap_or(total)
            .min(total);

        // ── スクロール体感ロギング ─────────────────────────────────────
        // vis 範囲が変化した瞬間 → 安定した瞬間 → 範囲内サムネイルが揃った瞬間を
        // ログに残し、「スクロール停止後にサムネイルが描かれるまでの遅延」を可視化する。
        let cur_vis_range = (visible_raw_start, visible_raw_end);
        if cur_vis_range != self.last_vis_range {
            self.last_vis_range = cur_vis_range;
            self.vis_settle_at = None;
            self.vis_first_logged = false;
            self.vis_all_logged = false;
        } else if self.vis_settle_at.is_none() && cur_vis_range.0 < cur_vis_range.1 {
            self.vis_settle_at = Some(std::time::Instant::now());
            crate::logger::log(format!(
                "[vis] settle vis=[{}..{}) ({} cells)",
                visible_raw_start,
                visible_raw_end,
                visible_raw_end - visible_raw_start,
            ));
            // 安定した瞬間に既に全 Loaded ならその場で all_loaded ログ。
            // 可視セルは visible_indices[vis_visible_start..vis_visible_end] なので
            // そちらを直接走査する (raw range で走ると疎な visible_indices では
            // フィルタで隠れた idx を含んでしまい、all_loaded_now が永久に false になる)。
            let all_loaded_now = self
                .visible_indices
                .get(vis_visible_start..vis_visible_end)
                .map(|slice| {
                    slice.iter().all(|&j| {
                        matches!(self.thumbnails.get(j), Some(ThumbnailState::Loaded { .. }))
                    })
                })
                .unwrap_or(false);
            if all_loaded_now {
                crate::logger::log(
                    "[vis] all_loaded 0ms after settle (already cached)".to_string(),
                );
                self.vis_first_logged = true;
                self.vis_all_logged = true;
            }
        }

        // 通常リクエストと重い I/O リクエストを分けて収集。
        // 反復対象は raw range ではなく keep_set (display list の部分列)。
        // これで ★フィルタ / Ctrl+F で疎になっても非可視 idx が流入しない。
        let mut new_regular: Vec<LoadRequest> = Vec::new();
        let mut new_heavy: Vec<LoadRequest> = Vec::new();
        // PDFium プールの thrash 抑制: PDF 仮想フォルダを開いた直後の grace 期間中は
        // 「現在見えていない PdfPage」(= prefetch) の enqueue をスキップする。
        // 一旦 dispatch すると PDFium は in-flight cancel できないので、ユーザーが
        // 数十 ms 内に Ctrl+↑↓ で次のフォルダに行く場合に「破棄される予定の prefetch
        // render が worker を 200-500ms 占有して、ユーザーが本当に見たいページの render を
        // 後ろに押し出す」という事態を防ぐ。grace を過ぎたら通常通り enqueue する
        // (request_repaint_after で grace 終了時に再実行されるよう確保する)。
        let pdf_prefetch_blocked = self
            .pdf_prefetch_grace_until
            .is_some_and(|t| std::time::Instant::now() < t);
        let mut deferred_pdf_prefetch = false;
        for i in self.keep_set_sorted() {
            if self.requested.contains_key(&i) {
                continue;
            }
            let need_load = matches!(
                self.thumbnails[i],
                ThumbnailState::Pending | ThumbnailState::Evicted
            );
            if !need_load {
                continue;
            }
            let Some((mtime, file_size)) = self.image_metas.get(i).copied().flatten() else {
                continue;
            };
            let Some(mut req) = self.items.get(i).and_then(|item| {
                make_load_request(
                    item,
                    i,
                    mtime,
                    file_size,
                    false,
                    self.pdf_current_password.as_deref(),
                    Some(self.settings.folder_thumb_sort),
                    self.settings.folder_thumb_depth,
                )
            }) else {
                continue;
            };
            req.priority = i >= visible_raw_start && i < visible_raw_end;
            // grace 中は PdfPage の prefetch (= 非 priority) のみスキップ。現在表示中の
            // PdfPage (= priority=true) は通常通り enqueue する。
            if pdf_prefetch_blocked
                && !req.priority
                && req.pdf_page.is_some()
            {
                deferred_pdf_prefetch = true;
                continue;
            }
            req.input_seq = self.input_seq;
            req.items_gen = self.items_generation;
            // ZipFile / PdfFile / Folder → heavy_io_queue、それ以外 → reload_queue
            let is_heavy = matches!(
                self.items.get(i),
                Some(GridItem::ZipFile(_) | GridItem::PdfFile(_) | GridItem::Folder(_))
            );
            // perf: エンキューイベント (タスク種別 + 優先度 + 相関 seq)
            if crate::perf::is_enabled() {
                let perf_key = self.perf_item_key(i);
                crate::perf::event(
                    "thumb",
                    "enqueue",
                    perf_key.as_deref(),
                    self.input_seq,
                    &[
                        ("idx", serde_json::Value::from(i)),
                        ("priority", serde_json::Value::from(req.priority)),
                        (
                            "queue",
                            serde_json::Value::from(if is_heavy { "heavy" } else { "regular" }),
                        ),
                    ],
                );
            }
            if is_heavy {
                new_heavy.push(req);
            } else {
                new_regular.push(req);
            }
        }
        let new_hi = new_regular
            .iter()
            .chain(new_heavy.iter())
            .filter(|r| r.priority)
            .count();
        let new_lo = new_regular.len() + new_heavy.len() - new_hi;
        let t3 = frame_t0.elapsed();
        let regular_count = new_regular.len();
        {
            let (ref mtx, ref cvar) = *queue_arc;
            let mut q = mtx.lock().unwrap();
            // 範囲外 (keep_set から外れた) キューエントリは取消扱い。pop 前なので worker が
            // 始めてすらいない → requested からも抜いて良い (再入範囲なら次フレームで
            // 再エンキューされる)。keep_set と requested は App の別フィールドなので
            // 事前に分離して disjoint field borrow にする (closure 内で両方必要)。
            let keep_set = &self.keep_set;
            let requested = &mut self.requested;
            q.retain(|r| {
                let keep = keep_set.contains(&r.idx);
                if !keep {
                    requested.remove(&r.idx);
                }
                keep
            });
            for r in q.iter_mut() {
                r.priority = r.idx >= visible_raw_start && r.idx < visible_raw_end;
            }
            let _q_before = q.len();
            for r in new_regular {
                requested.insert(r.idx, false);
                q.push(r);
            }
            drop(q);
            for _ in 0..regular_count {
                cvar.notify_one();
            }
        }
        // heavy_io_queue にも同様に push
        let heavy_count = new_heavy.len();
        if let Some(hq) = self.heavy_io_queue.clone() {
            let (ref mtx, ref cvar) = *hq;
            let mut q = mtx.lock().unwrap();
            let keep_set = &self.keep_set;
            let requested = &mut self.requested;
            q.retain(|r| {
                let keep = keep_set.contains(&r.idx);
                if !keep {
                    requested.remove(&r.idx);
                }
                keep
            });
            for r in q.iter_mut() {
                r.priority = r.idx >= visible_raw_start && r.idx < visible_raw_end;
            }
            for r in new_heavy {
                requested.insert(r.idx, false);
                q.push(r);
            }
            drop(q);
            for _ in 0..heavy_count {
                cvar.notify_one();
            }
        }
        if new_hi > 0 || new_lo > 0 {
            crate::logger::log(format!(
                "  [queue] push +{new_hi}H +{new_lo}L  keep=[{keep_start}..{keep_end})  vis=[{visible_raw_start}..{visible_raw_end})  requested={}",
                self.requested.len(),
            ));
        }
        let t4 = frame_t0.elapsed();

        // (3) 段階 E: アイドル時の画質アップグレード (対象は `self.keep_set`)
        self.enqueue_idle_upgrades();
        let t5 = frame_t0.elapsed();

        // (4) 進捗ピーク値の更新 (プログレスバー表示用)
        self.update_progress_peaks();
        let t6 = frame_t0.elapsed();

        // (5) 固着診断 (5 秒に 1 回): keep_set 内で state が Pending/Evicted
        //     なのに `requested` に居座っているエントリを検出する。
        //     正常時は Pending/Evicted なら再エンキューされて Loaded に進むはず。
        //     この状態が続くと、サムネが「読み込み中」のまま永遠に戻らない。
        if self.last_stuck_scan_at.elapsed().as_secs() >= 5 {
            self.last_stuck_scan_at = std::time::Instant::now();
            let mut stuck: Vec<(usize, &'static str)> = Vec::new();
            for (&idx, _) in &self.requested {
                if !self.keep_set.contains(&idx) {
                    continue;
                }
                let label = match self.thumbnails.get(idx) {
                    Some(ThumbnailState::Pending) => Some("Pending"),
                    Some(ThumbnailState::Evicted) => Some("Evicted"),
                    Some(ThumbnailState::Failed) => Some("Failed"),
                    _ => None,
                };
                if let Some(l) = label {
                    stuck.push((idx, l));
                }
            }
            if !stuck.is_empty() {
                stuck.sort_by_key(|(i, _)| *i);
                let pf = self.pending_finalize.len();
                let bl = self.texture_backlog.len();
                crate::logger::log(format!(
                    "  [stuck] {} entries in requested but not Loaded (keep=[{}..{}) pending_finalize={} backlog={}): {:?}",
                    stuck.len(),
                    keep_start,
                    keep_end,
                    pf,
                    bl,
                    stuck.iter().take(20).collect::<Vec<_>>(),
                ));
            }
        }

        if (t6 - t1).as_millis() > 5 {
            crate::logger::log(format!(
                "    [keep detail] evict={:.1}ms scan={:.1}ms lock+push={:.1}ms idle={:.1}ms peaks={:.1}ms",
                (t2 - t1).as_secs_f64() * 1000.0,
                (t3 - t2).as_secs_f64() * 1000.0,
                (t4 - t3).as_secs_f64() * 1000.0,
                (t5 - t4).as_secs_f64() * 1000.0,
                (t6 - t5).as_secs_f64() * 1000.0,
            ));
        }

        // grace 中に prefetch を defer したなら、grace 終了時刻に repaint を要求して
        // 自然な再 enqueue タイミングを確保する (= 次フレームでこの関数が再走しても
        // 通常のスクロール repaint が来ない場合でも prefetch が再開される)。
        if pdf_prefetch_blocked && deferred_pdf_prefetch {
            if let Some(deadline) = self.pdf_prefetch_grace_until {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                ctx.request_repaint_after(remaining);
            }
        }
        // grace 期限切れなら field をクリア (毎フレーム比較を省く)。
        if !pdf_prefetch_blocked && self.pdf_prefetch_grace_until.is_some() {
            self.pdf_prefetch_grace_until = None;
        }
    }

    /// 段階 E: アイドル時に画質アップグレードの要求を投入する。
    ///
    /// 発動条件:
    /// - 設定 `thumb_idle_upgrade` が有効
    /// - スクロールが一定時間 (500 ms) 停止している
    /// - `reload_queue` が空で `requested` も空 (他の作業が全て終わっている)
    ///
    /// アップグレード対象:
    /// 1. `Loaded { from_cache: true }` — キャッシュ (WebP q=75) 由来で画質劣化
    /// 2. `Loaded { rendered_at_px < current_display_px * 0.8 }` —
    ///    列数変更などで現在のセルサイズより 20% 以上小さい解像度で生成されている
    ///
    /// `keep_set` 内の該当セルを最大 `BATCH` 件ずつ、`skip_cache = true` の
    /// LoadRequest として push する。スクロール優先度付きの worker が visible
    /// 側から先に処理する。
    fn enqueue_idle_upgrades(&mut self) {
        const SCROLL_IDLE_SECS: f64 = 0.5;
        /// ユーザー入力から何秒アイドル経過したらアップグレードを許可するか。
        /// スクロール以外のキー操作やフルスクリーン遷移の直後も PDF ワーカーを
        /// 占有させないために、`last_input_at` ベースのクールダウンを追加している。
        const INPUT_IDLE_SECS: f64 = 0.5;

        if !self.settings.thumb_idle_upgrade {
            return;
        }

        // スクロール変化の検出
        if (self.scroll_offset_y - self.last_scroll_offset_y_tracked).abs() > 0.5 {
            self.last_scroll_change_time = std::time::Instant::now();
            self.last_scroll_offset_y_tracked = self.scroll_offset_y;
        }
        let scroll_idle = self.last_scroll_change_time.elapsed().as_secs_f64() >= SCROLL_IDLE_SECS;
        if !scroll_idle {
            return;
        }

        // 入力クールダウン: スクロール以外の入力 (キー・フルスクリーン遷移・ホイール等)
        // の直後もアップグレード起動を抑制する。scroll_offset_y は変わらないが
        // fs_open 等で PDF ワーカーが Critical 要求を処理中かもしれない。
        if let Some(t) = self.last_input_at
            && t.elapsed().as_secs_f64() < INPUT_IDLE_SECS
        {
            return;
        }

        // キューと in-flight が両方空のときだけ走らせる
        if !self.requested.is_empty() {
            return;
        }
        let Some(queue_arc) = self.reload_queue.clone() else {
            return;
        };
        {
            let (ref mtx, _) = *queue_arc;
            let q = mtx.lock().unwrap();
            if !q.is_empty() {
                return;
            }
        }

        // 現在の display_px (アイドル判定とサイズ比較に使用)
        let current_display_px = self.display_px_shared.load(Ordering::Relaxed);

        // 候補集め: keep_range 内で from_cache=true or 解像度不足のものを全件
        //
        // ※ 以前は BATCH=4 で小分け push していたが、進捗バーが「0/4 → 4/4 → 消える」
        //    を繰り返すちらつき現象が発生していた。現在は keep_range 内の全候補を
        //    一度に push し、進捗バーが 1 本だけ綺麗に伸びるようにする。
        //    - スクロール時は scroll_idle ガードで新規 push されない
        //    - 通常ロードが必要なら requested ガードで先送り
        //    - フォルダ切替は cancel_token で全停止
        //    なので大量 push しても害は無い (古い結果は poll_thumbnails で
        //    keep_set 外なら自動破棄される)。
        let mut upgrade_reqs: Vec<LoadRequest> = Vec::new();
        for i in self.keep_set_sorted() {
            let needs_upgrade = match self.thumbnails.get(i) {
                Some(ThumbnailState::Loaded {
                    from_cache,
                    rendered_at_px,
                    source_dims,
                    ..
                }) => {
                    // 1. キャッシュ由来 (品質アップグレード)
                    // 2. 現在のセルに対して解像度不足 (rendered < target * 0.8)
                    //    u32 オーバーフロー対策で u64 で比較
                    //
                    // target は `min(source_long_edge, current_display_px)`:
                    // 元画像が display_px より小さい場合、どれだけ再デコードしても
                    // 物理的に display_px まで拡大できない (fast_resize は upscale
                    // しない)。source で頭打ちにしないと永久に「解像度不足」判定が
                    // 続き、アイドル時に同じセルを毎サイクル re-enqueue する無限
                    // ループ (進捗バーが高速に左右を往復する原因) になる。
                    let source_long_edge = source_dims.map(|(w, h)| w.max(h));
                    let target_px = source_long_edge
                        .map(|src| src.min(current_display_px))
                        .unwrap_or(current_display_px);
                    *from_cache || (*rendered_at_px as u64) * 5 < (target_px as u64) * 4
                }
                _ => false,
            };
            if !needs_upgrade {
                continue;
            }
            let Some((mtime, file_size)) = self.image_metas.get(i).copied().flatten() else {
                continue;
            };
            let Some(mut req) = self.items.get(i).and_then(|item| {
                make_load_request(
                    item,
                    i,
                    mtime,
                    file_size,
                    true,
                    self.pdf_current_password.as_deref(),
                    Some(self.settings.folder_thumb_sort),
                    self.settings.folder_thumb_depth,
                )
            }) else {
                continue;
            };
            // 通常エンキューと同じく現世代を載せる (旧 items への upgrade 混入防止)
            req.items_gen = self.items_generation;
            upgrade_reqs.push(req);
        }

        if upgrade_reqs.is_empty() {
            return;
        }

        crate::logger::log(format!(
            "  idle upgrade: queued {} items (display_px={})",
            upgrade_reqs.len(),
            current_display_px,
        ));
        let upgrade_count = upgrade_reqs.len();
        let (ref mtx, ref cvar) = *queue_arc;
        let mut q = mtx.lock().unwrap();
        for r in upgrade_reqs {
            // true = 高画質化要求
            self.requested.insert(r.idx, true);
            q.push(r);
        }
        drop(q);
        for _ in 0..upgrade_count {
            cvar.notify_one();
        }
    }

    /// in-flight + キュー内の通常/アップグレード件数を返す。
    ///
    /// `requested` マップは queue に push した時点で insert され、
    /// 結果を受信した時点で remove されるので、キュー待ち + ワーカー処理中の
    /// 全件を正確に反映している。queue を別途カウントすると二重計上になるため
    /// `requested` のみを集計する。
    fn count_pending(&self) -> (usize, usize) {
        let (mut in_normal, mut in_upgrade) = (0usize, 0usize);
        for (&idx, &is_upgrade) in &self.requested {
            // keep_set 外 (非可視 + 先読み範囲外) のリクエストは「処理中だがスクロール
            // またはフィルタ変更で不要になった」もの。進捗バーに含めない
            // (ワーカー完了時に除去される)。
            if !self.keep_set.contains(&idx) {
                continue;
            }
            if is_upgrade {
                in_upgrade += 1;
            } else {
                in_normal += 1;
            }
        }
        (in_normal, in_upgrade)
    }

    /// 現在の要求状況からプログレスバーのピーク値を更新する。
    fn update_progress_peaks(&mut self) {
        let backlog_count = self.texture_backlog.len();
        let (cur_normal_raw, cur_upgrade) = self.count_pending();
        // backlog 内のアイテムは requested に残っており count_pending でカウント
        // 済みだが、実際にはデコード完了済み。pending として見せると分母が膨らむので
        // 差し引く (ただし 0 以下にはしない)。
        let cur_normal = cur_normal_raw.saturating_sub(backlog_count);

        if cur_normal == 0 {
            self.progress_normal_peak = 0;
        } else if cur_normal > self.progress_normal_peak {
            // 新しいスクロール位置で新規リクエストが発生した場合は
            // peak を現在値にリセットする (古い peak が蓄積し続けるのを防ぐ)
            self.progress_normal_peak = cur_normal;
        }
        if cur_upgrade == 0 {
            self.progress_upgrade_peak = 0;
        } else if cur_upgrade > self.progress_upgrade_peak {
            self.progress_upgrade_peak = cur_upgrade;
        }
    }

    /// プログレスバーの現在値を計算して返す。
    /// `(normal (cur, peak), upgrade (cur, peak))`
    pub(crate) fn progress_snapshot(&self) -> ((usize, usize), (usize, usize)) {
        let (cur_normal, cur_upgrade) = self.count_pending();
        (
            (cur_normal, self.progress_normal_peak),
            (cur_upgrade, self.progress_upgrade_peak),
        )
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        // ウィンドウにフォーカスがない場合はキー入力を無視
        let has_focus = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        if !has_focus {
            return None;
        }
        // フルスクリーン、ダイアログ、テキスト入力中はショートカットを無効化
        if self.shortcuts_blocked_by_text_input() {
            return None;
        }

        // IME 変換直後の Enter 漏れ対策:
        // 検索バー / お気に入り検索バー / 全文検索バーが表示中で、かつ直近に IME イベントが
        // あった場合、この `handle_keyboard` 呼び出しをスキップして Enter がグリッドの
        // フルスクリーン起動ショートカットに回らないようにする。
        // (IME が Commit を吐いたフレームで TextEdit が一瞬 lost_focus → *_has_focus
        //  が false になり、次フレームで Enter が grid 側に漏れるレースをガードする)。
        if (self.show_search_bar || self.favsearch.active || self.global_search.active)
            && self.ime_input_active()
        {
            return None;
        }

        let cols = self.settings.grid_cols.max(1);

        // Ctrl の状態判定。AutoHotKey 等の外部ツールが Ctrl+矢印を送信する場合、
        // Ctrl と矢印が別フレームで届くことがある。直前フレームの Ctrl 押下も
        // 考慮するため、egui の全イベントから Ctrl 修飾子を探す。
        let ctrl_held = ctx.input(|i| {
            // 現在のフレームで Ctrl が押されている
            if i.modifiers.ctrl {
                return true;
            }
            // イベントの中に Ctrl 修飾子付きのキーイベントがあるか
            i.events.iter().any(|e| match e {
                egui::Event::Key { modifiers, .. } => modifiers.ctrl,
                _ => false,
            })
        });

        let (
            right,
            left,
            down,
            up,
            enter,
            backspace,
            _ctrl_up_raw,
            _ctrl_down_raw,
            home,
            end,
            page_up,
            page_down,
            space,
            key_r,
            key_l,
        ) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Backspace),
                i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowUp),
                i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Home),
                i.key_pressed(egui::Key::End),
                i.key_pressed(egui::Key::PageUp),
                i.key_pressed(egui::Key::PageDown),
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::R),
                i.key_pressed(egui::Key::L),
            )
        });

        // Ctrl+矢印: modifiers.ctrl に加え ctrl_held (key_down) でも判定
        let ctrl_up = ctrl_held && up;
        let ctrl_down = ctrl_held && down;

        // Ctrl+G の DrilledInto で drill-back する手段は BS (↓で始まる通常ハンドラ)
        // と検索バーの ← ボタンに限定する。
        // 旧実装: ← キーでも drill-back させていたが、グリッドのカーソル左移動を
        // 奪ってしまい「上右下は動くのに左だけ親階層へ戻る」という現象になっていた
        // (ユーザー報告 2026-04)。カーソル移動を優先する設計に戻す。

        let vi = &self.visible_indices;
        let vi_len = vi.len();

        if vi_len > 0 {
            let sel = self
                .selected
                .unwrap_or_else(|| vi.first().copied().unwrap_or(0));
            // visible_indices 内での現在位置
            let vis_pos = vi.iter().position(|&i| i == sel).unwrap_or(0);
            let cell_h = self.last_cell_h.max(1.0);
            let visible_rows = (self.last_viewport_h / cell_h).floor() as usize;
            let page_items = visible_rows.max(1) * cols;

            // visible_indices 上で移動し、raw index に変換
            // Ctrl+矢印はフォルダ移動に使うので、通常カーソル移動から除外
            let new_vis_pos = if right && !ctrl_held {
                Some((vis_pos + 1).min(vi_len - 1))
            } else if left && !ctrl_held {
                Some(vis_pos.saturating_sub(1))
            } else if down && !ctrl_down {
                Some((vis_pos + cols).min(vi_len - 1))
            } else if up && !ctrl_up {
                Some(vis_pos.saturating_sub(cols))
            } else if home {
                Some(0)
            } else if end {
                Some(vi_len - 1)
            } else if page_down {
                Some((vis_pos + page_items).min(vi_len - 1))
            } else if page_up {
                Some(vis_pos.saturating_sub(page_items))
            } else {
                None
            };

            let shift = ctx.input(|i| i.modifiers.shift);
            let new_sel = new_vis_pos.and_then(|vp| vi.get(vp).copied());

            if let Some(s) = new_sel {
                // Shift+カーソル: 移動元から移動先までの画像をチェックに追加
                if shift && new_vis_pos.is_some() {
                    let old_pos = vis_pos;
                    let new_pos = new_vis_pos.unwrap();
                    let (start, end) = if old_pos <= new_pos {
                        (old_pos, new_pos)
                    } else {
                        (new_pos, old_pos)
                    };
                    for vp in start..=end {
                        if let Some(&idx) = vi.get(vp) {
                            match self.items.get(idx) {
                                Some(GridItem::Image(_))
                                | Some(GridItem::Video(_))
                                | Some(GridItem::ZipImage { .. })
                                | Some(GridItem::PdfPage { .. }) => {
                                    self.checked.insert(idx);
                                }
                                _ => {}
                            }
                        }
                    }
                }

                self.selected = Some(s);
                self.scroll_to_selected = true;
                self.update_last_selected_image();
                // perf: グリッドのカーソル移動イベントを記録
                self.bump_input_seq("grid_key", Some(&format!("sel={s}")));
            }

            // スペースキー: チェック ON/OFF
            if space {
                if let Some(idx) = self.selected {
                    if self.checked.contains(&idx) {
                        self.checked.remove(&idx);
                    } else {
                        // フォルダ・セパレータはチェック対象外
                        match self.items.get(idx) {
                            Some(GridItem::Image(_))
                            | Some(GridItem::Video(_))
                            | Some(GridItem::ZipImage { .. })
                            | Some(GridItem::PdfPage { .. }) => {
                                self.checked.insert(idx);
                            }
                            _ => {}
                        }
                    }
                }
            }

            // L/R: 選択画像を回転
            if key_r {
                if let Some(idx) = self.selected {
                    self.rotate_image_cw(idx);
                }
            }
            if key_l {
                if let Some(idx) = self.selected {
                    self.rotate_image_ccw(idx);
                }
            }

            // F1-F5: レーティング 1〜5 を適用 / F6: レーティング解除
            // (チェック済みアイテムがあれば一括、なければ選択にのみ)
            // Shift+F1-F5 / F6: 現在一覧表示中のフォルダ / ZIP / PDF 本体に評価を付与。
            // matches_logically 対策で Shift 版を先に consume する (NONE は Shift 入りも拾う)。
            {
                let shift_rating_key = ctx.input_mut(|i| {
                    crate::ui_helpers::consume_rating_fkey(i, egui::Modifiers::SHIFT)
                });
                if let Some(stars) = shift_rating_key
                    && self.set_current_folder_rating(stars)
                {
                    self.show_container_rating_toast(stars);
                }
                let rating_key = ctx.input_mut(|i| {
                    crate::ui_helpers::consume_rating_fkey(i, egui::Modifiers::NONE)
                });
                if let Some(stars) = rating_key {
                    self.apply_rating_to_selection(stars);
                }
            }

            // F7/F8: マスクスロット 1/2 を一括適用
            // (チェック済みアイテムがあれば一括、なければ選択 1 件に)
            // フルスクリーン側 (ui_fullscreen.rs) と揃えて修飾キー無しのみ受け付ける。
            {
                let slot_key = ctx.input_mut(|i| {
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::F7) {
                        Some(1usize)
                    } else if i.consume_key(egui::Modifiers::NONE, egui::Key::F8) {
                        Some(2)
                    } else {
                        None
                    }
                });
                if let Some(slot) = slot_key {
                    self.apply_slot_to_selection(slot);
                }
            }

            // Ctrl+1〜0: 補正プリセットスロットを一括適用
            {
                const SLOT_KEYS: [egui::Key; 10] = [
                    egui::Key::Num1,
                    egui::Key::Num2,
                    egui::Key::Num3,
                    egui::Key::Num4,
                    egui::Key::Num5,
                    egui::Key::Num6,
                    egui::Key::Num7,
                    egui::Key::Num8,
                    egui::Key::Num9,
                    egui::Key::Num0,
                ];
                let preset_slot = ctx.input_mut(|i| {
                    if !i.modifiers.ctrl {
                        return None;
                    }
                    SLOT_KEYS
                        .iter()
                        .position(|k| i.consume_key(egui::Modifiers::CTRL, *k))
                });
                if let Some(slot) = preset_slot {
                    // capture_adjust_full でラップして、N ページの一括書き換えを
                    // 1 つの UndoEntry にまとめて Ctrl+Z で全部戻せるようにする (Codex P2)。
                    self.capture_adjust_full(
                        format!(
                            "スロット{}を一括適用",
                            crate::adjustment::slot_key_label(slot)
                        ),
                        |app| app.apply_slot_to_grid_selection(slot),
                    );
                }
            }

            // Q / Ctrl+Backspace: チェック済み (なければ選択 1 件) の個別補正を一括解除
            // フルスクリーン側 (ui_fullscreen.rs) と同じキー割当。
            // Ctrl+Backspace は consume_key しておかないと後段の「BS で親フォルダ」と衝突する。
            {
                let clear_key = ctx.input_mut(|i| {
                    i.consume_key(egui::Modifiers::CTRL, egui::Key::Backspace)
                        || i.consume_key(egui::Modifiers::NONE, egui::Key::Q)
                });
                if clear_key {
                    self.capture_adjust_full(
                        "個別設定を一括解除".to_string(),
                        |app| app.clear_page_params_for_selection(),
                    );
                }
            }

            self.handle_meta_undo_keys(ctx);

            if enter {
                if let Some(idx) = self.selected {
                    match self.items.get(idx) {
                        Some(GridItem::Folder(p))
                        | Some(GridItem::ZipFile(p))
                        | Some(GridItem::PdfFile(p)) => {
                            let p = p.clone();
                            self.maybe_suppress_rating_filter_for_opened_container(idx);
                            return Some(p);
                        }
                        Some(GridItem::Image(_))
                        | Some(GridItem::ZipImage { .. })
                        | Some(GridItem::ZipSeparator { .. })
                        | Some(GridItem::PdfPage { .. })
                        | Some(GridItem::Video(_)) => {
                            // 動画も画像と同じくフルスクリーン化 → インライン再生。
                            // 外部プレイヤー起動はフルスクリーン中の Shift+Enter から。
                            self.bump_input_seq_for_item("grid_enter", idx);
                            self.open_fullscreen(idx);
                        }
                        Some(GridItem::ConvertibleArchive { path, format }) => {
                            let pf = path.clone();
                            let fmt = *format;
                            self.maybe_suppress_rating_filter_for_opened_container(idx);
                            if let Some(cached) = self.try_archive_cache_lookup(&pf) {
                                self.open_archive_via_cache(pf, cached);
                                return None;
                            }
                            self.request_archive_convert(pf, fmt);
                        }
                        Some(GridItem::SearchContainer { path, kind, .. }) => {
                            // Ctrl+G 結果ビュー (Aggregated) でコンテナを Enter
                            // → drill-down view に切り替え (v1: 1 階層のみ, docs §10.3)
                            let p = path.clone();
                            let is_zip = matches!(kind, crate::grid_item::SearchContainerKind::Zip);
                            // ★コンテナを開いた時、内部画像が未評価で空表示にならないよう
                            // フィルタを一時解除する (Codex P2)。
                            self.maybe_suppress_rating_filter_for_opened_container_path(&p);
                            self.drill_into_container(p, is_zip);
                            return None;
                        }
                        None => {}
                    }
                }
            }
        }

        // 検索コンテキスト中はファイルシステム遡りを禁止する:
        // - BS はスタックベースで検索ルートまで戻る (favsearch_back)
        // - Ctrl+↑↓ によるフォルダナビゲーションも無効化
        let in_favsearch = self.favsearch.active;

        // BS: 親フォルダへ (検索中はスタックを戻る)
        // Ctrl+BS は個別補正の解除に使うので除外する
        if backspace && !ctrl_held {
            // Ctrl+G 絞り込みビュー中なら 1 段上げる (current_path != container_root) か、
            // Aggregated に戻る。自由な fs 遡行は許さない (docs §10.3)。
            if self.global_search.active {
                if matches!(
                    self.global_search.view,
                    crate::global_search_ui::GlobalSearchView::DrilledInto { .. }
                ) {
                    self.drill_back_one_level();
                    return None;
                }
                // Aggregated 状態で BS: Ctrl+G を閉じて saved_folder へ戻る。
                // ここで catch しないと FS 親遡行に流れ、search bar が active のまま
                // saved_folder の親 → ドライブ root の中身が表示される事故になる。
                self.close_global_search();
                return None;
            }
            if in_favsearch {
                self.favsearch_back();
                // favsearch_back 内で load_folder 済み。navigate 経路には流さない。
                return None;
            }
            if let Some(ref cur) = self.effective_folder() {
                if let Some(parent) = cur.parent() {
                    // 親に戻ったとき、元のフォルダ名を選択するようにヒントを設定
                    self.select_after_load = cur
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.to_string());
                    return Some(parent.to_path_buf());
                }
            }
        }

        // Ctrl+↓: 深さ優先で次のフォルダへ（画像なしはスキップ）
        // バックグラウンドスレッドで navigate_folder_with_skip を実行し、
        // 結果は poll_folder_nav で非同期に受信する。
        // 検索コンテキスト中は検索結果内での前後移動に置き換える。
        // Ctrl+G 中は file system 遡行を禁止して Ctrl+G の範囲に閉じる
        // (Aggregated 時は Ctrl+↑↓ は no-op、DrilledInto 時は drill-tree DFS)。
        let in_global_search = self.global_search.active;
        let in_global_search_drilled = in_global_search
            && matches!(
                self.global_search.view,
                crate::global_search_ui::GlobalSearchView::DrilledInto { .. }
            );
        if ctrl_down {
            // perf: グリッドの Ctrl+↓ を input イベントとして記録 (fullscreen 側と対称)。
            // これで入力 → DFS → load_folder → 初フレームまでを seq で相関できる。
            self.bump_input_seq("grid_ctrl_nav", Some("forward"));
            if in_global_search_drilled {
                self.global_search_ctrl_nav(true);
            } else if in_global_search {
                // Aggregated 中は何もしない (fs ツリー遡行はユーザ期待に反する)
            } else if in_favsearch {
                self.favsearch_ctrl_nav(true);
            } else if let Some(cur) = self.effective_folder() {
                self.start_folder_nav(cur, true, FolderNavMode::Grid);
            }
        }

        // Ctrl+↑: 深さ優先で前のフォルダへ（画像なしはスキップ）
        if ctrl_up {
            self.bump_input_seq("grid_ctrl_nav", Some("backward"));
            if in_global_search_drilled {
                self.global_search_ctrl_nav(false);
            } else if in_global_search {
                // Aggregated 中は何もしない
            } else if in_favsearch {
                self.favsearch_ctrl_nav(false);
            } else if let Some(cur) = self.effective_folder() {
                self.start_folder_nav(cur, false, FolderNavMode::Grid);
            }
        }

        None
    }

    /// Ctrl+↑↓ のフォルダナビゲーションをバックグラウンドスレッドで開始する。
    /// `navigate_folder_with_skip` はフォルダツリーの DFS 走査 + `folder_should_stop`
    /// (`read_dir`) を行うためディスク I/O を伴い、HDD では 20-120ms かかる。
    /// UI スレッドをブロックしないよう、結果は `poll_folder_nav` で非同期に受信する。
    ///
    /// **連打の扱い (Step B アキュームレート)**:
    /// in-flight 中に追加の Ctrl+↑↓ が来たら、旧スレッドをキャンセルせず
    /// `pending_folder_nav_steps` に累積する (forward=+1, backward=-1)。
    /// 現 nav が完了したあと `chain_folder_nav_if_pending` で次のステップを連鎖実行する。
    /// これにより「30Hz 連打で 30 ステップ進める」挙動になり、かつ
    /// 同時並行 DFS のファイルシステム競合を避ける。
    ///
    /// **モードの扱い**:
    /// `mode` は DFS の発火元 (grid / fullscreen / favsearch) を示し、
    /// DFS 完了時に `apply_folder_nav_result` がこの mode を見て後処理を分岐する。
    /// in-flight 中に異なるモードで start_folder_nav が呼ばれた場合は、
    /// 旧モードをキャンセルして新モードで仕切り直す (モード混在を避ける)。
    pub(crate) fn start_folder_nav(
        &mut self,
        current: PathBuf,
        forward: bool,
        mode: FolderNavMode,
    ) {
        // 連打アキュームレータの上限。キーを離した後に余韻で追加遷移が続くと
        // 体感上「離したのに動く」違和感になるため、溜められる量を制限する。
        // 5 なら画像フォルダの load_folder (~100ms/step) で 500ms 弱で drain され、
        // リピート離脱直後のレスポンスが保たれる。超過分のプレスは捨てる。
        const MAX_PENDING_NAV: i32 = 5;

        if let Some(pending) = self.folder_nav_pending.as_ref() {
            if folder_nav_mode_same_kind(&pending.mode, &mode) {
                // 既に同一モードで nav 進行中: 連打を累積するだけ
                let delta: i32 = if forward { 1 } else { -1 };
                self.pending_folder_nav_steps = (self.pending_folder_nav_steps + delta)
                    .clamp(-MAX_PENDING_NAV, MAX_PENDING_NAV);
                return;
            }
            // モードが変わった (例: fullscreen → grid) → 旧 DFS をキャンセル
            pending.cancel.store(true, Ordering::Relaxed);
            self.folder_nav_pending = None;
        }
        // 新しい nav 列開始: アキュームレータをリセットしてから spawn
        self.pending_folder_nav_steps = 0;
        self.pending_folder_nav_mode = mode.clone();
        self.spawn_folder_nav(current, forward, mode);
    }

    /// 内部ヘルパー: DFS ワーカースレッドを spawn する。
    /// `start_folder_nav` (ユーザー押下) と `chain_folder_nav_if_pending` (累積消化) の
    /// 両方から呼ばれる。cancel トークンは `navigate_folder_with_skip` にも渡し、
    /// 次のユーザー操作で即座に DFS を畳めるようにする。
    fn spawn_folder_nav(&mut self, current: PathBuf, forward: bool, mode: FolderNavMode) {
        let skip_limit = self.settings.folder_skip_limit;
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_w = Arc::clone(&cancel);

        // perf: DFS のユーザー入力相関 seq を thread に渡す。
        // chain_folder_nav_if_pending 経由の連鎖 DFS でも同じ seq が伝搬するので、
        // 1 回のキー押下で起きた DFS バーストを 1 つの seq でまとめて追える。
        let perf_seq = self.input_seq;
        let perf_mode = mode.perf_tag();
        let start_path_disp = if crate::perf::is_enabled() {
            current.display().to_string()
        } else {
            String::new()
        };

        std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "nav",
                    "dfs_begin",
                    None,
                    perf_seq,
                    &[
                        ("forward", serde_json::Value::from(forward)),
                        ("mode", serde_json::Value::from(perf_mode)),
                        ("start", serde_json::Value::from(start_path_disp.clone())),
                    ],
                );
            }
            let outcome = if forward {
                navigate_folder_with_skip(&current, next_folder_dfs, skip_limit, Some(&cancel_w))
            } else {
                navigate_folder_with_skip(&current, prev_folder_dfs, skip_limit, Some(&cancel_w))
            };
            let dfs_ms = t0.elapsed().as_secs_f64() * 1000.0;

            // DFS スレッドで事前スキャンまで済ませる: ヒットした先が通常ディレクトリなら、
            // ここで `read_dir` + メタデータ取得を終わらせて UI スレッドの lf_scan
            // (HDD で 100-180ms 級) を除去する。ZIP/PDF ファイルは専用ローダーが
            // 別経路で処理するのでここではスキャンしない。
            let scanned = if cancel_w.load(Ordering::Relaxed) {
                None
            } else if let Some(o) = outcome.as_ref() {
                if o.path.is_dir() {
                    let scan_t0 = std::time::Instant::now();
                    let s = scan_directory(&o.path);
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "nav",
                            "dfs_scan",
                            None,
                            perf_seq,
                            &[
                                (
                                    "ms",
                                    serde_json::Value::from(
                                        scan_t0.elapsed().as_secs_f64() * 1000.0,
                                    ),
                                ),
                                ("folders", serde_json::Value::from(s.folders.len())),
                                ("media", serde_json::Value::from(s.all_media.len())),
                            ],
                        );
                    }
                    Some(s)
                } else {
                    None
                }
            } else {
                None
            };

            let cancelled = cancel_w.load(Ordering::Relaxed);
            if crate::perf::is_enabled() {
                let hit = outcome
                    .as_ref()
                    .map(|o| o.path.display().to_string())
                    .unwrap_or_else(|| "-".to_string());
                let hit_image_folder = outcome
                    .as_ref()
                    .map(|o| o.hit_image_folder)
                    .unwrap_or(false);
                crate::perf::event(
                    "nav",
                    "dfs_end",
                    None,
                    perf_seq,
                    &[
                        ("ms", serde_json::Value::from(dfs_ms)),
                        (
                            "total_ms",
                            serde_json::Value::from(t0.elapsed().as_secs_f64() * 1000.0),
                        ),
                        ("forward", serde_json::Value::from(forward)),
                        ("mode", serde_json::Value::from(perf_mode)),
                        ("cancelled", serde_json::Value::from(cancelled)),
                        ("found", serde_json::Value::from(outcome.is_some())),
                        (
                            "hit_image_folder",
                            serde_json::Value::from(hit_image_folder),
                        ),
                        ("hit_path", serde_json::Value::from(hit)),
                        ("pre_scanned", serde_json::Value::from(scanned.is_some())),
                    ],
                );
            }
            if !cancelled {
                let _ = tx.send(FolderNavThreadResult { outcome, scanned });
            }
        });

        self.folder_nav_pending = Some(FolderNavPending {
            cancel,
            rx,
            forward,
            mode,
        });
    }

    /// 直前の folder_nav 完了後に呼ぶ。累積ステップが残っていれば次の DFS を連鎖実行。
    /// モードは直前バーストと同じ (`pending_folder_nav_mode`) を引き継ぐ。
    fn chain_folder_nav_if_pending(&mut self) {
        if self.pending_folder_nav_steps == 0 {
            return;
        }
        let forward = self.pending_folder_nav_steps > 0;
        // 1 ステップ消費
        self.pending_folder_nav_steps += if forward { -1 } else { 1 };
        let mode = self.pending_folder_nav_mode.clone();
        // mode に応じて「次のステップの起点」は変わる:
        //   Grid / Fullscreen → self.current_folder
        //   Favsearch        → favsearch.nav_stack.last() (= 現在位置)
        let current = match mode {
            FolderNavMode::Favsearch { .. } => self.favsearch.nav_stack.last().cloned(),
            _ => self.current_folder.clone(),
        };
        if let Some(cur) = current {
            self.spawn_folder_nav(cur, forward, mode);
        }
    }

    /// バックグラウンドフォルダナビゲーションの結果を非同期にポーリングする。
    /// 結果が到着していれば `Some(FolderNavResult)` を返し、未完了なら `None` を返す。
    fn poll_folder_nav(&mut self) -> Option<FolderNavResult> {
        let pending = self.folder_nav_pending.as_ref()?;
        match pending.rx.try_recv() {
            Ok(thread_result) => {
                let pending = self.folder_nav_pending.take().unwrap();
                let (path, hit_image_folder) = match thread_result.outcome {
                    Some(o) => (Some(o.path), o.hit_image_folder),
                    None => (None, false),
                };
                Some(FolderNavResult {
                    path,
                    hit_image_folder,
                    forward: pending.forward,
                    mode: pending.mode,
                    scanned: thread_result.scanned,
                })
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                let pending = self.folder_nav_pending.take().unwrap();
                Some(FolderNavResult {
                    path: None,
                    hit_image_folder: false,
                    forward: pending.forward,
                    mode: pending.mode,
                    scanned: None,
                })
            }
        }
    }

    /// DFS 完了時の後処理。モードに応じて load_folder / open_fullscreen /
    /// favsearch の stack push や sibling fallback を使い分ける。
    fn apply_folder_nav_result(&mut self, ctx: &egui::Context, result: FolderNavResult) {
        // perf: DFS 結果を UI スレッドで適用する区間 (close_fullscreen + load_folder +
        // open_fullscreen 等) の wall time を計測する。Ctrl+↑↓ 連打中に UI が詰まる
        // 原因がここに集まるため、ms を必ず記録する。
        let apply_t0 = std::time::Instant::now();
        let apply_seq = self.input_seq;
        let apply_mode_tag = result.mode.perf_tag();
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "apply_begin",
                None,
                apply_seq,
                &[
                    ("forward", serde_json::Value::from(result.forward)),
                    ("mode", serde_json::Value::from(apply_mode_tag)),
                    ("found", serde_json::Value::from(result.path.is_some())),
                    (
                        "hit_image_folder",
                        serde_json::Value::from(result.hit_image_folder),
                    ),
                ],
            );
        }
        // 内部関数として展開して、全 early-return 前に emit_end を呼ぶ。
        let emit_end =
            |t0: std::time::Instant, seq: u64, mode: &'static str, reason: &'static str| {
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "nav",
                        "apply_end",
                        None,
                        seq,
                        &[
                            (
                                "ms",
                                serde_json::Value::from(t0.elapsed().as_secs_f64() * 1000.0),
                            ),
                            ("mode", serde_json::Value::from(mode)),
                            ("reason", serde_json::Value::from(reason)),
                        ],
                    );
                }
            };
        let Some(path) = result.path else {
            // DFS が尽きた (forward で末尾、backward で先頭に達した等)
            match result.mode {
                FolderNavMode::Favsearch { .. } => {
                    // favsearch では DFS 尽きた場合は検索結果の前後アイテムへ
                    let delta: isize = if result.forward { 1 } else { -1 };
                    self.favsearch_navigate_sibling(delta);
                }
                FolderNavMode::Fullscreen => {
                    // DFS がツリー末端に達した: フルスクリーンは維持して中央にヒントを出す。
                    // 累積された連打は打ち切る (次回以降も同じ末端に向かうだけ)。
                    self.fs_boundary_hint =
                        Some(crate::ui_fullscreen::FsBoundaryHint::NoImageFolder {
                            forward: result.forward,
                            at: std::time::Instant::now(),
                        });
                    self.pending_folder_nav_steps = 0;
                    // items_generation が進まないので poll_fs_nav_lock では解除されない
                    // → 明示的に release して以降の Ctrl+↑↓ を効くようにする (Codex P1)。
                    self.release_fs_nav_lock();
                }
                FolderNavMode::Grid => {}
            }
            emit_end(apply_t0, apply_seq, apply_mode_tag, "dfs_empty");
            return;
        };
        // Fullscreen モードで skip_limit 尽きフォールバックの場合は、画像の無い
        // フォルダへ飛ばしてフルスクリーンが解除されるのを避けるため、現状維持で
        // 中央ヒントを出す。Grid モードは従来通り移動 (段階的に進める導線)。
        if matches!(result.mode, FolderNavMode::Fullscreen) && !result.hit_image_folder {
            self.fs_boundary_hint = Some(crate::ui_fullscreen::FsBoundaryHint::NoImageFolder {
                forward: result.forward,
                at: std::time::Instant::now(),
            });
            self.pending_folder_nav_steps = 0;
            // 同上: items_generation 不変のまま return するので明示 release (Codex P1)。
            self.release_fs_nav_lock();
            emit_end(apply_t0, apply_seq, apply_mode_tag, "fs_boundary");
            return;
        }
        // DFS スレッドで事前スキャン済みなら UI スレッドの read_dir を省ける。
        let scanned = result.scanned;
        match result.mode {
            FolderNavMode::Grid => {
                self.load_folder_with_scan(path, scanned);
            }
            FolderNavMode::Fullscreen => {
                // fs_cache / ai_upscale_cache は item index がキーで、
                // load_folder で items を入れ替えると古い画像を新しい idx で
                // 誤って引く危険がある。close_fullscreen で一括破棄してから
                // 新フォルダを読み直す (PDF Critical 予約は open_fullscreen で再取得)。
                self.close_fullscreen();
                self.load_folder_with_scan(path, scanned);
                // PDF / ZIP は enumerate_*_async が非同期なので、load_folder 直後は
                // items が空のことがある。結果は poll_*_enumerate が受信するので、
                // そこで fullscreen を開き直す。find_fullscreen_nav_target も内部で
                // load_folder を呼んで PDF/ZIP に自動進入することがあるため、その前後の
                // 両方でチェックする (Codex P1: ZIP 非同期化で ZIP への Ctrl+↑↓ fullscreen
                // 継続が壊れていたのを修正)。
                if self.pdf_enumerate_pending.is_some() || self.zip_enumerate_pending.is_some() {
                    self.fs_nav_after_pdf_enumerate = Some(result.forward);
                    emit_end(apply_t0, apply_seq, apply_mode_tag, "enumerate_defer");
                    return;
                }
                let target_idx = self.find_fullscreen_nav_target();
                if self.pdf_enumerate_pending.is_some() || self.zip_enumerate_pending.is_some() {
                    self.fs_nav_after_pdf_enumerate = Some(result.forward);
                    emit_end(apply_t0, apply_seq, apply_mode_tag, "enumerate_defer");
                    return;
                }
                if let Some(new_idx) = target_idx {
                    self.open_fullscreen(new_idx);
                    self.selected = Some(new_idx);
                    self.scroll_to_selected = true;
                    self.update_last_selected_image();
                } else {
                    // navigate_folder_with_skip は画像ありフォルダを返す前提だが、
                    // レーティングフィルタ等で visible_indices が空の場合はここに来る。
                    // fullscreen は close 済みなので、メインビューポートに
                    // キーボードフォーカスを戻す (旧同期実装と同じ挙動)。
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    // フルスクリーン再オープン無しなら nav ロックを使う相手がいないので
                    // 明示 release (Codex P1)。次の Ctrl+↑↓ がブロックされない。
                    self.release_fs_nav_lock();
                }
            }
            FolderNavMode::Favsearch { root } => {
                let delta: isize = if result.forward { 1 } else { -1 };
                if crate::search_index_db::is_under(&path, &root) {
                    // サブツリー内 — 通常の DFS 移動としてスタックに push
                    self.favsearch.nav_stack.push(path.clone());
                    self.load_folder_with_scan(path, scanned);
                    self.update_favsearch_address();
                } else {
                    // サブツリー外へ出ようとしている → 検索結果の前後へ移動
                    self.favsearch_navigate_sibling(delta);
                }
            }
        }
        emit_end(apply_t0, apply_seq, apply_mode_tag, "done");
    }

    /// Ctrl+↑↓ フルスクリーン遷移後の表示対象 item index を決める。
    ///
    /// 1. まず可視アイテムから画像系 (Image/Video/ZipImage/PdfPage) を探す。
    /// 2. 見つからず、ZIP/PDF ファイルだけ置かれているフォルダだった場合は
    ///    最初 (backward 時は最後) の ZIP/PDF に入り、その中の画像系を返す。
    ///    これにより「ZIP/PDF しか入っていない中間フォルダ」でフルスクリーン表示が
    ///    切れず、マンガ/コミックの連続閲覧が続く。
    fn find_fullscreen_nav_target(&mut self) -> Option<usize> {
        // Ctrl+↑↓ は方向に関わらず常にフォルダ先頭の画像系アイテムへ着地する
        // (= 一般的なビューワ慣習に合わせ、フォルダ識別性を優先)。後方ナビでも
        // 先頭着地にすることで Ctrl+矢印の mental model が「フォルダにジャンプして
        // 冒頭から見る」の 1 つに統一される。前ページ末尾を見たい場合は Ctrl+↑ で
        // 戻った後 End キーで末尾に飛べる。
        let find_image = |app: &Self| -> Option<usize> {
            let items = &app.items;
            let is_image_like = |i: usize| {
                matches!(
                    items.get(i),
                    Some(GridItem::Image(_))
                        | Some(GridItem::Video(_))
                        | Some(GridItem::ZipImage { .. })
                        | Some(GridItem::PdfPage { .. })
                )
            };
            app.visible_indices
                .iter()
                .copied()
                .find(|&i| is_image_like(i))
        };

        if let Some(idx) = find_image(self) {
            return Some(idx);
        }

        // 画像系が無い: 先頭の ZIP/PDF ファイルを仮想フォルダとして開いて中身を取り出す。
        // ZIP/PDF の選び方も同様に「先頭」固定。
        let pick_virtual = |i: usize, items: &[GridItem]| -> Option<PathBuf> {
            match items.get(i) {
                Some(GridItem::ZipFile(p)) | Some(GridItem::PdfFile(p)) => Some(p.clone()),
                _ => None,
            }
        };
        let virtual_path = self
            .visible_indices
            .iter()
            .copied()
            .find_map(|i| pick_virtual(i, &self.items))?;
        self.load_folder(virtual_path);
        find_image(self)
    }

    /// マウスホイールイベントを消費し、行単位でスナップしたオフセットに変換する。
    /// Ctrl+ホイールの場合はグリッド列数を変更する。
    fn process_scroll(&mut self, ctx: &egui::Context) {
        // ダイアログやフルスクリーン表示中はスクロールを消費しない
        // (ダイアログ内の ScrollArea が正しく動くようにする)
        if self.fullscreen_idx.is_some() || self.any_dialog_open() {
            return;
        }

        let cell_h = self.last_cell_h.max(1.0);

        // マウスホイールイベントだけを取り出し、egui には渡さない
        let (scroll_delta_y, ctrl) = ctx.input(|i| (i.raw_scroll_delta.y, i.modifiers.ctrl));
        if scroll_delta_y.abs() > 0.5 {
            ctx.input_mut(|i| {
                i.raw_scroll_delta = egui::Vec2::ZERO;
                i.smooth_scroll_delta = egui::Vec2::ZERO;
                // MouseWheel イベントも消費
                i.events
                    .retain(|e| !matches!(e, egui::Event::MouseWheel { .. }));
            });

            if ctrl {
                // Ctrl+ホイール: 列数を増減（1〜10 の範囲）
                let delta = -scroll_delta_y.signum() as i32;
                let new_cols = (self.settings.grid_cols as i32 + delta).clamp(
                    crate::settings::MIN_GRID_COLS as i32,
                    crate::settings::MAX_GRID_COLS as i32,
                ) as usize;
                if new_cols != self.settings.grid_cols {
                    self.settings.grid_cols = new_cols;
                    self.settings.save();
                    self.bump_input_seq("grid_cols", None);
                }
            } else {
                // 上スクロール(delta>0) → オフセット減、下スクロール(delta<0) → オフセット増
                let direction = -scroll_delta_y.signum();
                let prev_offset = self.scroll_offset_y;
                self.scroll_offset_y = (self.scroll_offset_y + direction * cell_h).max(0.0);
                // 行境界にスナップ
                self.scroll_offset_y = (self.scroll_offset_y / cell_h).round() * cell_h;
                if (self.scroll_offset_y - prev_offset).abs() > 0.5 {
                    self.bump_input_seq(
                        "grid_wheel",
                        Some(&format!("offset={:.0}", self.scroll_offset_y)),
                    );
                }
            }
        }
    }

    /// カーソルキー移動後、選択行がビューポートに収まるようオフセットを調整する
    pub(crate) fn apply_scroll_to_selected(&mut self, cols: usize, cell_h: f32) {
        let sel = match self.selected {
            Some(s) => s,
            None => return,
        };
        // フィルタ中は visible_indices 内での位置から行を計算する
        let vis_pos = self
            .visible_indices
            .iter()
            .position(|&i| i == sel)
            .unwrap_or(sel);
        let row = vis_pos / cols;
        let row_top = row as f32 * cell_h;
        let row_bottom = row_top + cell_h;
        let vp_top = self.scroll_offset_y;
        let vp_bottom = self.scroll_offset_y + self.last_viewport_h;

        if row_top < vp_top {
            // 選択行が上に隠れている → 選択行が最上行になるようスクロール
            self.scroll_offset_y = row_top;
        } else if row_bottom > vp_bottom {
            // 選択行が下に隠れている → 選択行が最下行になるようスクロール
            self.scroll_offset_y = (row_bottom - self.last_viewport_h).max(0.0);
            // 行境界にスナップ
            self.scroll_offset_y = (self.scroll_offset_y / cell_h).ceil() * cell_h;
        }
    }

    /// ウィンドウ位置を記録する（最小化・最大化中は更新しない）。
    fn track_window_rect(&mut self, ctx: &egui::Context) {
        let (outer_rect, inner_rect, pixels_per_point, minimized, maximized) = ctx.input(|i| {
            let vp = i.viewport();
            (
                vp.outer_rect,
                vp.inner_rect,
                i.pixels_per_point,
                vp.minimized.unwrap_or(false),
                vp.maximized.unwrap_or(false),
            )
        });

        if outer_rect.is_none() && self.last_outer_rect.is_none() {
            crate::logger::log(format!(
                "[viewport] outer_rect=None  inner_rect={:?}  pixels_per_point={pixels_per_point:.2}",
                inner_rect.map(|r| format!(
                    "pos=({:.0},{:.0}) size={:.0}x{:.0}",
                    r.min.x,
                    r.min.y,
                    r.width(),
                    r.height()
                ))
            ));
        }

        let best_rect = outer_rect.or(inner_rect);
        self.last_pixels_per_point = pixels_per_point;

        if !minimized && !maximized {
            if let Some(rect) = best_rect {
                let changed = self
                    .last_outer_rect
                    .map(|r| {
                        (r.min - rect.min).length() > 1.0 || (r.size() - rect.size()).length() > 1.0
                    })
                    .unwrap_or(true);
                if changed {
                    crate::logger::log(format!(
                        "[viewport] rect updated: pos=({:.0},{:.0}) size={:.0}x{:.0}  \
                         outer={:?}  inner={:?}  ppp={pixels_per_point:.2}",
                        rect.min.x,
                        rect.min.y,
                        rect.width(),
                        rect.height(),
                        outer_rect.map(|_| "Some"),
                        inner_rect.map(|_| "Some"),
                    ));
                    self.last_outer_rect = Some(rect);
                }
            }
            // inner_rect が取れていればそのサイズも別途保持する。
            // 保存/再適用の値として outer を使うとタイトルバー分だけ毎回小さくなるので、
            // 再起動時の InnerSize 再適用と整合する inner のサイズを優先して保存する。
            if let Some(ir) = inner_rect {
                self.last_inner_size = Some([ir.width(), ir.height()]);
            }
        }
    }

    /// グリッドショートカット (`handle_keyboard` / `handle_clipboard_shortcuts`) を
    /// テキスト入力フォーカス・ダイアログ・フルスクリーン中にブロックするための共通判定。
    /// 検索バー (Ctrl+F / Ctrl+S / Ctrl+G) のいずれかにフォーカスがある状態で
    /// BS / Enter / Ctrl+C 等が grid に漏れないようにするためのゲート。
    pub(crate) fn shortcuts_blocked_by_text_input(&self) -> bool {
        self.fullscreen_idx.is_some()
            || self.any_dialog_open()
            || self.address_has_focus
            || self.search_has_focus
            || self.favsearch.has_focus
            || self.global_search.has_focus
    }

    /// Ctrl+C / Ctrl+X / Ctrl+V ショートカットを処理する。
    fn handle_clipboard_shortcuts(&mut self, ctx: &egui::Context) {
        let main_focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        if !main_focused || self.shortcuts_blocked_by_text_input() {
            return;
        }

        let (ctrl_c, ctrl_x) = ctx.input(|i| {
            let mut c = false;
            let mut x = false;
            for event in &i.events {
                match event {
                    egui::Event::Copy => c = true,
                    egui::Event::Cut => x = true,
                    _ => {}
                }
            }
            (c, x)
        });

        let ctrl_v = {
            #[cfg(windows)]
            {
                let ctrl =
                    unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x11) };
                let v =
                    unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x56) };
                (ctrl & (0x8000u16 as i16)) != 0 && (v & (0x8000u16 as i16)) != 0 && (v & 1) != 0
            }
            #[cfg(not(windows))]
            false
        };

        if ctrl_c || ctrl_x {
            let paths = if !self.checked.is_empty() {
                self.collect_checked_paths()
            } else if let Some(idx) = self.selected {
                match self.items.get(idx) {
                    Some(GridItem::Image(p)) | Some(GridItem::Video(p)) => vec![p.clone()],
                    _ => vec![],
                }
            } else {
                vec![]
            };
            if !paths.is_empty() {
                if ctrl_x {
                    crate::ui_dialogs::context_menu::cut_files_to_clipboard(&paths);
                } else {
                    crate::ui_dialogs::context_menu::copy_files_to_clipboard(&paths);
                }
            }
        }

        if ctrl_v {
            if let Some(ref folder) = self.current_folder.clone() {
                let rx =
                    crate::ui_dialogs::context_menu::paste_files_from_clipboard(folder);
                self.paste_pending.push(rx);
            }
        }
    }

    // -----------------------------------------------------------------------
    // フルスクリーン表示
    // -----------------------------------------------------------------------

    /// フルスクリーン表示を開始する。
    /// キャッシュ済みなら即座に表示し、そうでなければ読み込みを開始する。
    /// 動画アイテムの場合はサムネイル＋再生ボタンを表示するだけで読み込みは不要。
    ///
    /// **perf 計装の注意**: この関数は `input_seq` を bump しない。呼び出し元が
    /// ユーザー入力起点の場合は事前に `bump_input_seq` すること。slideshow や
    /// 見開き正規化のような内部起動は bump しないので、fs load は現在の
    /// `self.input_seq` (= 直近のユーザー入力) に紐づく。
    pub fn open_fullscreen(&mut self, idx: usize) {
        crate::logger::log(format!("=== open_fullscreen: idx={idx} ==="));
        // フルスクリーン側ではページ単位 (= 1 ファイル/1 ページ) 操作中心になるため、
        // グリッド側で積み上げた Undo は破棄する。フルスクリーン中の操作は新しい
        // スタックで管理する (画像移動でさらにクリアされる)。
        self.clear_meta_undo();
        self.fullscreen_idx = Some(idx);
        self.adjust_spread_target = AdjustSpreadTarget::Left;
        // PDF pool の Critical 予約を ON: 現在ページのレンダリング用に 1 ワーカー確保。
        // グリッドに戻ったら OFF に戻し、全 3 ワーカーを Normal に使えるようにする。
        crate::pdf_loader::set_critical_reservation(true);
        self.fs_opened_at = Some(std::time::Instant::now());
        self.fs_focus_grace_elapsed = false;
        // 初回フレームでフォーカス遷移 (false→true) が誤検出されないように
        // true 始まりにする (グレース期間中にフォーカス復帰扱いされるのを防ぐ)
        self.fs_prev_focused = true;
        self.fs_focus_regained_at = None;
        self.fs_suppress_primary_until_release = false;
        self.reset_erase_mode();

        // ページに個別補正があればトースト表示
        if self.adjustment_page_params.contains_key(&idx) {
            self.show_feedback_toast("ページ補正適用".to_string());
        }
        // ページ切替時に補正キャッシュをクリア（前ページの補正結果を残さない）
        // ただし ai_upscale_cache は消さない（再処理が重いため）
        self.adjustment_cache.remove(&idx);

        // 画像切り替え時にズーム/パン/キャッシュをリセット
        self.analysis_zoom = 1.0;
        self.analysis_pan = egui::Vec2::ZERO;
        self.analysis_pan_drag_start = None;
        self.analysis_guide_drag = None;
        self.analysis_overlay_cache = None;
        self.analysis_hist_cache = None;
        self.analysis_sv_cache = None;
        self.fs_zoom = 1.0;
        self.fs_pan = egui::Vec2::ZERO;
        self.fs_pan_drag_start = None;
        self.fs_free_rotation = 0.0;
        self.fs_rotation_drag_start = None;
        // 透過背景は「一時的な好み」なので画像切替時にリセット (plan-v0.7.0.md の方針)
        self.fs_transparent_bg_mode = 0;
        self.fs_transparent_bg_indicator_until = None;

        match self.items.get(idx) {
            Some(GridItem::Image(_))
            | Some(GridItem::ZipImage { .. })
            | Some(GridItem::PdfPage { .. }) => {
                if self.fs_cache.contains_key(&idx) {
                    crate::logger::log(format!("  cache hit idx={idx} → instant display"));
                } else if !self.fs_pending.contains_key(&idx) {
                    self.start_fs_load(idx);
                }
                self.update_prefetch_window(idx);
            }
            Some(GridItem::Video(_)) => {
                // 動画はインライン再生 (フルスクリーン化と同時に VideoPlayer を起動)。
                // start_fs_load の早期分岐で GridItem::Video を検出し、
                // FsCacheEntry::Video を fs_cache に挿入する。
                if self.fs_cache.contains_key(&idx) {
                    crate::logger::log(format!("  video cache hit idx={idx} → resume playback"));
                } else {
                    crate::logger::log(format!("  video idx={idx} → start inline playback"));
                    self.start_fs_load(idx);
                }
            }
            Some(GridItem::ZipSeparator { dir_display }) => {
                // セパレータはテキスト表示のみ (デコード不要)
                crate::logger::log(format!(
                    "  zip separator idx={idx} → title mode: {dir_display}"
                ));
            }
            _ => {}
        }

        // AI / EXIF / XMP メタデータ読み込みは **バックグラウンドスレッド** で実行する。
        // XMP は JPEG/PNG 全体を読むため UI スレッドで同期実行すると 20MP 画像で
        // 100ms 級にブロックする。メタデータパネルは値到着まで空表示。
        self.start_metadata_load(idx);
    }

    /// メタデータ / EXIF / XMP キャッシュ用の正規化キーを返す。
    ///
    /// `Image` / `Video` は正規化パス、`ZipImage` は `zip_path::entry_name`、
    /// `PdfPage` は `pdf_path::page_N` の形式で、ZIP エントリ・PDF ページごとに
    /// 衝突しないキーを返す ([`App::page_path_key`] と同じ規約)。
    /// 動画は EXIF/AI metadata は持たないが、mXD が埋めた XMP (X ツイート情報)
    /// を表示する必要があるため、メタデータキーを発行する。
    pub(crate) fn metadata_cache_key(&self, idx: usize) -> Option<String> {
        let item = self.items.get(idx)?;
        let key = match item {
            GridItem::Image(p) | GridItem::Video(p) => crate::adjustment_db::normalize_path(p),
            GridItem::ZipImage {
                zip_path,
                entry_name,
            } => {
                format!(
                    "{}::{}",
                    crate::adjustment_db::normalize_path(zip_path),
                    entry_name.to_lowercase()
                )
            }
            GridItem::PdfPage {
                pdf_path, page_num, ..
            } => {
                format!(
                    "{}::page_{}",
                    crate::adjustment_db::normalize_path(pdf_path),
                    page_num
                )
            }
            _ => return None,
        };
        Some(key)
    }

    /// 指定 idx の AI/EXIF/XMP メタデータ読み込みをバックグラウンドで開始する。
    /// 全キャッシュが既にヒットしていれば spawn しない (no-op)。
    /// 既存 pending があれば cancel して置き換える (連打時は最新だけ処理)。
    ///
    /// UI 側 (`ui_metadata_panel`) はキャッシュヒット時のみ内容を表示するので、
    /// 結果到着まではパネルは空表示のまま (None 扱い) になる。
    fn start_metadata_load(&mut self, idx: usize) {
        let Some(key) = self.metadata_cache_key(idx) else {
            return;
        };

        // 全キャッシュが既に揃っているなら何もしない
        let ai_hit = self.metadata_cache.contains_key(&key);
        let exif_hit = self.exif_cache.contains_key(&key);
        let xmp_hit = self.xmp_cache.contains_key(&key);
        if ai_hit && exif_hit && xmp_hit {
            return;
        }

        // 既存の in-flight を cancel (新 idx なら旧結果は不要)
        if let Some(pending) = self.metadata_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }

        // スレッドに渡すため item を snapshot (clone)
        let item = match self.items.get(idx) {
            Some(g) => g.clone(),
            None => return,
        };
        let hidden: Vec<String> = self.settings.exif_hidden_tags.clone();
        let key_owned = key.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_w = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();

        std::thread::Builder::new()
            .name(format!("metadata-load-{idx}"))
            .spawn(move || {
                let result = run_metadata_load(key_owned, item, &hidden, &cancel_w);
                if cancel_w.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(r) = result {
                    let _ = tx.send(r);
                }
            })
            .ok();

        self.metadata_pending = Some(MetadataLoadPending { cancel, rx });
    }

    /// メタデータ読み込み結果を非同期に受信してキャッシュに投入する。
    /// 毎フレーム呼ばれる。ヒット時はパネルが自動的に内容を表示する。
    pub(crate) fn poll_metadata_load(&mut self) {
        let Some(pending) = self.metadata_pending.as_ref() else {
            return;
        };
        match pending.rx.try_recv() {
            Ok(r) => {
                self.metadata_cache.insert(r.key.clone(), r.metadata);
                self.exif_cache.insert(r.key.clone(), r.exif);
                self.xmp_cache.insert(r.key, r.xmp);
                self.metadata_pending = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.metadata_pending = None;
            }
        }
    }

    /// 同名ファイルフィルタを適用する。
    fn apply_duplicate_filters(
        &mut self,
        folders: &mut Vec<GridItem>,
        folder_metas: &mut Vec<Option<(i64, i64)>>,
        all_media: &mut Vec<(PathBuf, bool, i64, i64)>,
    ) {
        if self.settings.skip_zip_if_folder_exists {
            Self::filter_virtual_folder_duplicates(folders, folder_metas);
        }
        if self.settings.skip_image_if_video_exists {
            self.filter_video_image_duplicates(all_media);
        }
        if self.settings.skip_duplicate_images {
            Self::filter_image_ext_duplicates(all_media, &self.settings.image_ext_priority);
        }
    }

    /// ZIP/PDF + フォルダの重複: 同名フォルダがあれば ZIP/PDF エントリをスキップ。
    /// folders と folder_metas は同じ順序で対応しているため、同期して削除する。
    ///
    /// 仮想フォルダ (ZIP/PDF) 判定は [`folder_tree::sorted_subdirs`] の Ctrl+↑↓ 用
    /// フィルタと揃える必要がある。片側だけ PDF を除外すると、グリッドに表示されている
    /// PDF が folder tree 側では sibling 扱いされず Ctrl+↑↓ の位置検索が失敗する。
    fn filter_virtual_folder_duplicates(
        folders: &mut Vec<GridItem>,
        folder_metas: &mut Vec<Option<(i64, i64)>>,
    ) {
        let real_folder_names: std::collections::HashSet<String> = folders
            .iter()
            .filter_map(|item| {
                if let GridItem::Folder(p) = item {
                    return p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.to_lowercase());
                }
                None
            })
            .collect();

        let mut keep = vec![true; folders.len()];
        for (i, item) in folders.iter().enumerate() {
            let p = match item {
                GridItem::ZipFile(p)
                | GridItem::PdfFile(p)
                | GridItem::ConvertibleArchive { path: p, .. } => p,
                _ => continue,
            };
            if real_folder_names.contains(&stem_lower(p)) {
                keep[i] = false;
            }
        }
        let mut ki = keep.iter();
        folders.retain(|_| *ki.next().unwrap());
        let mut ki = keep.iter();
        folder_metas.retain(|_| *ki.next().unwrap());
    }

    /// 動画 + 画像の重複: 同名の動画があれば画像をスキップし、
    /// 画像ファイルを動画のサムネイルソースとして記録する。
    ///
    /// Phase 5.3: `Settings.video_thumb_use_sidecar_image` で有効/無効を切替可能に。
    /// OFF のときは override を記録しない (= グリッドではシェル既定サムネのみ採用)
    /// が、画像ファイルの listing からの除外は維持する (= 同名画像が動画と並んで二重
    /// 表示されるのは望ましくないため)。
    fn filter_video_image_duplicates(&mut self, all_media: &mut Vec<(PathBuf, bool, i64, i64)>) {
        let video_stems: std::collections::HashSet<String> = all_media
            .iter()
            .filter(|(_, is_video, _, _)| *is_video)
            .map(|(p, _, _, _)| stem_lower(p))
            .collect();

        if video_stems.is_empty() {
            return;
        }

        let use_sidecar = self.settings.video_thumb_use_sidecar_image;
        for (p, is_video, _, _) in all_media.iter() {
            if *is_video {
                continue;
            }
            let stem = stem_lower(p);
            if video_stems.contains(&stem) && use_sidecar {
                self.video_thumb_overrides.insert(stem, p.clone());
            }
        }

        all_media.retain(|(p, is_video, _, _)| *is_video || !video_stems.contains(&stem_lower(p)));
    }

    /// 同名画像の拡張子重複: 優先度リストに基づいてフィルタ。
    fn filter_image_ext_duplicates(
        all_media: &mut Vec<(PathBuf, bool, i64, i64)>,
        priority: &[String],
    ) {
        // ステム → (最優先の拡張子の優先度, インデックス)
        let mut best: std::collections::HashMap<String, (usize, usize)> =
            std::collections::HashMap::new();

        for (i, (p, is_video, _, _)) in all_media.iter().enumerate() {
            if *is_video {
                continue;
            }
            let stem = stem_lower(p);
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let prio = priority
                .iter()
                .position(|e| e == &ext)
                .unwrap_or(usize::MAX);
            match best.get(&stem) {
                Some(&(existing_prio, _)) if prio >= existing_prio => {}
                _ => {
                    best.insert(stem, (prio, i));
                }
            }
        }

        // 同名ステムの画像が複数あるか判定
        let mut stem_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (p, is_video, _, _) in all_media.iter() {
            if *is_video {
                continue;
            }
            *stem_counts.entry(stem_lower(p)).or_insert(0) += 1;
        }

        let keep_indices: std::collections::HashSet<usize> = best
            .iter()
            .filter(|(stem, _)| stem_counts.get(stem.as_str()).copied().unwrap_or(0) > 1)
            .map(|(_, &(_, idx))| idx)
            .collect();

        if !keep_indices.is_empty() {
            let mut i = 0;
            all_media.retain(|(p, is_video, _, _)| {
                let current_i = i;
                i += 1;
                if *is_video {
                    return true;
                }
                let stem = stem_lower(p);
                if stem_counts.get(&stem).copied().unwrap_or(0) <= 1 {
                    return true;
                }
                keep_indices.contains(&current_i)
            });
        }
    }

    /// `search_filter` とレーティングフィルタに基づいて `visible_indices` を再計算する。
    /// 両者は AND 結合。レーティングフィルタはレーティング対象に適用される:
    /// ページ単位 (Image / ZipImage / PdfPage) + コンテナ (Folder / ZipFile / PdfFile) の両方。
    /// 動画 / セパレータ / ConvertibleArchive などは常に通す。
    pub(crate) fn rebuild_visible_indices(&mut self) {
        let search_filter = self.search_filter.clone();
        let rating_filter = self.settings.rating_filter;
        // すべてのバケットが true ならレーティングフィルタは無効 (常に通す)
        let rating_filter_active = !rating_filter.iter().all(|&b| b);

        let n = self.items.len();
        let mut result = Vec::with_capacity(n);
        for i in 0..n {
            if let Some(ref f) = search_filter {
                if !f.contains(&i) {
                    continue;
                }
            }
            if rating_filter_active {
                let stars = self.get_rating(i);
                if let Some(item) = self.items.get(i) {
                    if !passes_rating_filter(item, stars, &rating_filter) {
                        continue;
                    }
                }
            }
            result.push(i);
        }
        self.visible_indices = result;
        self.cached_nav_indices = None;
        // WYSIWYG 原則: 非表示になったアイテムは checked / selected の対象から外す。
        // これで `handle_grid_keys` の `position().unwrap_or(0)` 起因の
        // 「F1 で非表示にした後、矢印キーで一覧先頭に飛ぶ」挙動も解消される。
        if !self.checked.is_empty() {
            let vi = &self.visible_indices;
            self.checked.retain(|idx| vi.binary_search(idx).is_ok());
        }
        self.redirect_selected_to_visible();
    }

    /// `visible_indices` に含まれる (= フィルタ後の一覧に出ている) かを返す。
    /// `visible_indices` は items 順 (昇順) なので binary_search で O(log n)。
    /// より狭い「prefetch 対象」判定が欲しい場合は [`Self::keep_set`] を直接参照する。
    pub(crate) fn idx_visible(&self, idx: usize) -> bool {
        self.visible_indices.binary_search(&idx).is_ok()
    }

    /// `keep_set` の内容を idx 昇順の Vec で返す。enqueue ログの順序安定や
    /// idle upgrade / tag prewarm / 補正テクスチャなど「先頭から順に処理したい」
    /// 背景処理用。worker 側は priority + distance で並べ直すので機能影響はない。
    fn keep_set_sorted(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.keep_set.iter().copied().collect();
        v.sort_unstable();
        v
    }

    /// `select_after_load` のヒントで items 内をケース無視で検索し、見つかれば
    /// selected を更新して true を返す。フィルタで隠れていれば直近の可視 idx に逃がす
    /// (`redirect_selected_to_visible` と同じ WYSIWYG 不変条件)。
    fn try_select_after_load(&mut self) -> bool {
        let Some(name) = self.select_after_load.take() else {
            return false;
        };
        let name_lower = name.to_lowercase();
        let Some(idx) = self
            .items
            .iter()
            .position(|item| item.name().to_lowercase() == name_lower)
        else {
            return false;
        };
        self.selected = Some(idx);
        self.scroll_to_selected = true;
        self.redirect_selected_to_visible();
        true
    }

    /// `self.selected` が visible_indices から外れていたら、items 順で直近の
    /// visible idx にリダイレクトする (手前優先、無ければ次)。
    /// `rebuild_visible_indices` の末尾と、`load_folder` の履歴から selected を
    /// 復元した直後で呼ぶ (Codex P2: 履歴復元で stale な hidden idx が入り得るため)。
    pub(crate) fn redirect_selected_to_visible(&mut self) {
        let Some(sel) = self.selected else {
            return;
        };
        let vi = &self.visible_indices;
        if vi.binary_search(&sel).is_ok() {
            return;
        }
        let pos = vi.partition_point(|&i| i < sel);
        let prev = pos.checked_sub(1).map(|p| vi[p]);
        let next = vi.get(pos).copied();
        self.selected = prev.or(next);
        if self.selected.is_some() {
            self.scroll_to_selected = true;
        }
    }

    /// メタデータキーワード検索を実行する。
    /// フォルダ内の全 PNG 画像の tEXt チャンクを読み、
    /// キーワード（大文字小文字無視）にマッチするアイテムのみをフィルタ表示する。
    /// メタデータ検索 (Ctrl+F) をバックグラウンドスレッドで開始する。
    ///
    /// 旧実装は UI スレッドで `read_tweet_info` / `build_searchable_from_path` を
    /// 同期実行しており、大フォルダで数秒フリーズしていた。このメソッドでは
    /// スレッドを spawn し、結果は `poll_search` で受け取る。連打/新クエリで
    /// 既存検索をキャンセルできるよう `SearchPending.cancel` を立てる。
    pub(crate) fn execute_search(&mut self) {
        // 既存の in-flight 検索をキャンセル (新クエリ / 再 Enter)。
        if let Some(pending) = self.search_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }

        // クエリ構文: space = AND / `-word` = NOT / `"..."` = フレーズ (`-"..."` も可)。
        let tokens = crate::search_query::parse(&self.search_query);
        if tokens.is_empty() {
            self.search_filter = None;
            self.rebuild_visible_indices();
            return;
        }

        // スレッドに渡すスナップショット: items は中身 PathBuf を含むので clone コストは
        // あるが、検索は低頻度操作なので許容範囲。xmp_cache は既読分のルックアップ
        // 専用で、スレッドは自分が読み取った分は `xmp_additions` で UI に返す。
        let items_snapshot = self.items.clone();
        let xmp_snapshot = self.xmp_cache.clone();
        // INDEX_VERSION=5 で fts_meta から原文 (`*_norm`) が無くなったため、Ctrl+F は
        // 表示中アイテムを on-demand 経路 (PNG / EXIF / XMP / dc:subject 直読み) で
        // 判定する。`run_metadata_search` は引数として fts_meta を受け取るが現在は
        // 未使用 (将来 Tantivy STORED への切替路の余地として残してある)。
        let fts_meta_clone: Option<std::sync::Arc<crate::fts_meta::FtsMetaDb>> = self
            .indexer_manager
            .as_ref()
            .map(|mgr| mgr.clone_fts_meta());
        let target = self.search_target.clone();
        let mode: crate::search_query::MatchMode = self.search_or_mode.into();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_w = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        // perf 計装上、検索開始を input イベントとして記録する (input_seq は副作用で更新)。
        self.bump_input_seq("search", Some(&self.search_query.clone()));

        std::thread::Builder::new()
            .name("metadata-search".to_string())
            .spawn(move || {
                let result = run_metadata_search(
                    &tokens,
                    &items_snapshot,
                    &xmp_snapshot,
                    fts_meta_clone.as_ref(),
                    &target,
                    mode,
                    &cancel_w,
                );
                let _ = tx.send(result);
            })
            .ok();

        self.search_pending = Some(SearchPending { cancel, rx });
    }

    /// in-flight 検索があればキャンセルする (検索バーを閉じる等の経路で呼ぶ)。
    pub(crate) fn cancel_search_pending(&mut self) {
        if let Some(pending) = self.search_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// in-flight 検索の結果を非同期に受信する。
    /// フォルダ切替 / 検索バー閉じ / 新検索で旧スレッドはキャンセルされるため、
    /// 受信時は「まだ同じ検索コンテキストか」の追加チェックは不要 (cancel されれば
    /// スレッドは結果を送らない)。
    ///
    /// 検索 in-flight 中は毎フレーム repaint を要求する必要がある (egui はアイドルで
    /// 寝るため、ワーカーが送信しても UI が拾いに来ない)。呼び出し元の update() が
    /// ctx.request_repaint() を最後に呼ぶのでそちらに任せる (個別 ctx 保持不要)。
    pub(crate) fn poll_search(&mut self) {
        let Some(pending) = self.search_pending.as_ref() else {
            return;
        };
        match pending.rx.try_recv() {
            Ok(SearchThreadResult::Done {
                matches,
                xmp_additions,
            }) => {
                // ワーカーが新規に読み取った XMP をキャッシュにマージする。
                // 既存キーは上書きしない (並行して別スレッドが先に書き込んだケースを尊重)。
                for (key, xmp) in xmp_additions {
                    self.xmp_cache.entry(key).or_insert(xmp);
                }
                self.search_filter = Some(matches);
                self.rebuild_visible_indices();
                self.selected = None;
                self.scroll_offset_y = 0.0;
                self.search_pending = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                // キャンセル済みで送信されなかった or スレッドパニック。
                self.search_pending = None;
            }
        }
    }

    /// 指定 idx の回転角度を取得する（キャッシュ + DB）。
    pub(crate) fn get_rotation(&mut self, idx: usize) -> crate::rotation_db::Rotation {
        if let Some(&rot) = self.rotation_cache.get(&idx) {
            return rot;
        }
        let path = match self.items.get(idx) {
            Some(GridItem::Image(p)) => p.clone(),
            Some(GridItem::Video(p)) => p.clone(),
            _ => return crate::rotation_db::Rotation::None,
        };
        let rot = self
            .rotation_db
            .as_ref()
            .and_then(|db| db.get(&path))
            .unwrap_or(crate::rotation_db::Rotation::None);
        self.rotation_cache.insert(idx, rot);
        rot
    }

    /// 指定 idx の画像を時計回りに 90° 回転する。
    pub(crate) fn rotate_image_cw(&mut self, idx: usize) {
        let current = self.get_rotation(idx);
        let new_rot = current.rotate_cw();
        self.apply_rotation(idx, new_rot);
    }

    /// 指定 idx の画像を反時計回りに 90° 回転する。
    pub(crate) fn rotate_image_ccw(&mut self, idx: usize) {
        let current = self.get_rotation(idx);
        let new_rot = current.rotate_ccw();
        self.apply_rotation(idx, new_rot);
    }

    fn apply_rotation(&mut self, idx: usize, rot: crate::rotation_db::Rotation) {
        let path = match self.items.get(idx) {
            Some(GridItem::Image(p)) => p.clone(),
            Some(GridItem::Video(p)) => p.clone(),
            _ => return,
        };
        self.rotation_cache.insert(idx, rot);
        if let Some(ref db) = self.rotation_db {
            let _ = db.set(&path, rot);
        }
    }

    // ── レーティング ───────────────────────────────────────────────

    /// レーティング DB キーを返す。
    /// ページ単位 (画像 / ZIP 内画像 / PDF ページ) は `page_path_key` と同じ形式。
    /// コンテナ (フォルダ / ZIP / PDF 本体) はそのパスを `normalize_path` したもの。
    /// `::` セパレータの有無でページとコンテナを区別するので衝突しない。
    pub(crate) fn rating_path_key(&self, idx: usize) -> Option<String> {
        let item = self.items.get(idx)?;
        match item {
            GridItem::Image(_) | GridItem::ZipImage { .. } | GridItem::PdfPage { .. } => {
                self.page_path_key(idx)
            }
            // Video は単一ファイルだがパス正規化はコンテナと同じ規則 (DB キーは
            // フルパス normalize)。leaf vs container の分類は `is_rating_leaf` /
            // `is_container_ratable` で行う。
            GridItem::Folder(p)
            | GridItem::ZipFile(p)
            | GridItem::PdfFile(p)
            | GridItem::Video(p) => Some(crate::adjustment_db::normalize_path(p)),
            _ => None,
        }
    }

    /// レーティング操作の対象となる物理パスを返す (XMP 書き込み先 / Undo の source_path 用)。
    /// `rating_path_key` と同じアイテム範囲を扱うので、対応するアイテム種別を増減する
    /// 場合は両方を一緒に更新すること。
    pub(crate) fn rating_source_path(&self, idx: usize) -> Option<PathBuf> {
        match self.items.get(idx)? {
            GridItem::Image(p)
            | GridItem::Video(p)
            | GridItem::Folder(p)
            | GridItem::ZipFile(p)
            | GridItem::PdfFile(p) => Some(p.clone()),
            GridItem::ZipImage { zip_path, .. } => Some(zip_path.clone()),
            GridItem::PdfPage { pdf_path, .. } => Some(pdf_path.clone()),
            _ => None,
        }
    }

    /// 指定 idx のレーティング (0..=5) を取得する (キャッシュ + DB)。
    /// 動画 / セパレータ等は常に 0 を返す (レーティング対象外)。
    /// フォルダ / ZIP / PDF ファイル本体も対象 (コンテナレーティング)。
    /// 非対象アイテムはキャッシュを汚さないように insert しない。
    pub(crate) fn get_rating(&mut self, idx: usize) -> u8 {
        if let Some(&v) = self.rating_cache.get(&idx) {
            return v;
        }
        let accepts = matches!(self.items.get(idx), Some(it) if it.accepts_rating());
        if !accepts {
            return 0;
        }
        let key = match self.rating_path_key(idx) {
            Some(k) => k,
            None => return 0,
        };
        let stars = self.rating_db.as_ref().map(|db| db.get(&key)).unwrap_or(0);
        self.rating_cache.insert(idx, stars);
        stars
    }

    /// 指定 idx のレーティングを設定する (0..=5)。
    /// レーティング対象外アイテムの場合は何もしない。
    /// フォルダ / ZIP / PDF ファイル本体も対象 (コンテナレーティング)。
    pub(crate) fn set_rating(&mut self, idx: usize, stars: u8) {
        let accepts = matches!(self.items.get(idx), Some(it) if it.accepts_rating());
        if !accepts {
            return;
        }
        let stars = stars.min(5);
        let key = match self.rating_path_key(idx) {
            Some(k) => k,
            None => return,
        };
        // prewarm で全 item が cache に載っている前提。未取得なら 0 扱いで OK。
        let old_stars = self.rating_cache.get(&idx).copied().unwrap_or(0);
        self.rating_cache.insert(idx, stars);
        // ユーザが明示的に値を書いた path として記録。tag_prewarm の古い XMP 読み戻しで
        // 値を上書きされないように hydrate_ratings_from_xmp が参照する。
        self.user_set_rating_keys.insert(key.clone());
        if let Some(db) = self.rating_db.as_ref() {
            let _ = db.set(&key, stars);
        }
        // コンテナ自身の★は子孫集計と別軸なので、単一ファイルレーティング (画像 / 動画 /
        // ZIP 内画像 / PDF ページ) のみ親フォルダの集計に伝搬する。
        if matches!(self.items.get(idx), Some(it) if it.is_rating_leaf()) {
            self.apply_rating_delta_to_folder_counts(&key, old_stars, stars);
        }
        // コンテナへの rating 変更は current_folder と同じパスを指す可能性があるので
        // アドレスバーキャッシュを lazy 再計算させる。
        if matches!(
            self.items.get(idx),
            Some(GridItem::Folder(_) | GridItem::ZipFile(_) | GridItem::PdfFile(_))
        ) {
            self.current_folder_rating_cache = None;
        }
        // 設定 ON + Image (JPEG/PNG/WebP) のときだけ XMP にも書き込む。
        // コンテナや ZIP 内画像・PDF ページには書き込み先がないので DB 止まり。
        if self.settings.write_rating_to_xmp {
            let writable_path: Option<PathBuf> = match self.items.get(idx) {
                Some(GridItem::Image(p)) if crate::xmp_writer::is_writable_format(p) => {
                    Some(p.clone())
                }
                _ => None,
            };
            if let Some(path) = writable_path {
                self.ensure_rating_write_handle();
                if let Some(h) = self.rating_write_handle.as_ref() {
                    h.submit(crate::rating_write_worker::RatingWriteJob {
                        path,
                        rating: if stars == 0 { None } else { Some(stars) },
                    });
                }
            }
        }
    }

    /// レーティング XMP 書き込み worker を必要なら起動する (遅延初期化)。
    pub(crate) fn ensure_rating_write_handle(&mut self) {
        if self.rating_write_handle.is_none() {
            self.rating_write_handle = Some(crate::rating_write_worker::RatingWriteHandle::spawn());
        }
    }

    /// rating worker の結果を回収し、失敗があればトースト表示する。
    /// タグ書き込み ([`poll_tag_write_results`]) と同じポリシー。
    pub(crate) fn poll_rating_write_results(&mut self) {
        let mut errors: Vec<(PathBuf, String)> = Vec::new();
        if let Some(h) = self.rating_write_handle.as_ref() {
            while let Some(res) = h.try_recv_result() {
                match res.result {
                    Ok(()) => {
                        crate::logger::log(format!(
                            "[RATING] ✓ XMP written → {}",
                            res.path.display()
                        ));
                    }
                    Err(e) => {
                        crate::logger::log(format!(
                            "[RATING] ✗ XMP write failed: {e} → {}",
                            res.path.display()
                        ));
                        errors.push((res.path, e));
                    }
                }
            }
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
            self.show_feedback_toast(format!(
                "レーティング書き込み失敗 {} 件: {}",
                errors.len(),
                preview
            ));
        }
    }

    // ── 再帰レーティングフィルタ (子孫★集計) ─────────────────────

    /// `folder_rating_counts` を空にし、worker を cancel する。フォルダ切替時に呼ぶ。
    pub(crate) fn reset_folder_rating_counts(&mut self) {
        self.folder_rating_counts.clear();
        self.folder_rating_counts_loaded = false;
        self.folder_rating_counts_folder_key = None;
        if let Some(h) = self.folder_rating_counter_handle.take() {
            h.cancel();
        }
    }

    /// レーティングフィルタが有効か (バケットが 1 つでも OFF なら true)。
    /// フィルタが全 ON の状態は「フィルタ未使用」として worker を動かさない。
    pub(crate) fn rating_filter_active(&self) -> bool {
        !self.settings.rating_filter.iter().all(|&b| b)
    }

    /// `maybe_suppress_rating_filter_for_opened_container` の path 版。
    /// Ctrl+G の `SearchContainer` を開く経路 (`drill_into_container`) は idx 経由の
    /// `container_path()` / `get_rating()` で対応していない (SearchContainer は
    /// page_path_key にも rating_path_key にも載っていない疑似アイテム)。
    /// パスから直接 rating_db を引いて、コンテナ★がフィルタを通るなら一時解除する。
    ///
    /// 効果: ★5 を本に付けて★5 フィルタ ON のまま Ctrl+G 集約結果からそのフォルダを
    /// 開いても、中の画像が未評価でも空にならない (Codex P2 対応)。
    pub(crate) fn maybe_suppress_rating_filter_for_opened_container_path(
        &mut self,
        anchor: &Path,
    ) {
        if self.rating_filter_suppressed_at.is_some() {
            return;
        }
        if !self.rating_filter_active() {
            return;
        }
        let Some(db) = self.rating_db.as_ref() else {
            return;
        };
        let key = crate::adjustment_db::normalize_path(anchor);
        let stars = db.get(&key);
        if !(1..=5).contains(&stars) {
            return;
        }
        // Folder/ZipFile/PdfFile と同じく `s <= 5 && rf[s]` 規約。SearchContainer
        // 自身は accepts_rating=false なので passes_rating_filter は使えず、ここで
        // 直接判定する。
        let s = stars as usize;
        if s > 5 || !self.settings.rating_filter[s] {
            return;
        }
        let saved = self.settings.rating_filter;
        self.rating_filter_suppressed_at = Some((anchor.to_path_buf(), saved));
        self.settings.rating_filter = [true; 6];
    }

    /// 「自身に★が付いたコンテナ (Folder / ZIP / PDF) を開く」操作時に呼ぶ。
    /// コンテナ自身の★が現在フィルタを通るなら、フィルタを一時解除して anchor を記録する。
    ///
    /// これにより `★5 の ZIP を絞り込みで見つけて開く → 中身が未評価で全部消える`
    /// 事故を防ぐ。階層ナビ用のフォルダ (自身★なし) では発動しないので、
    /// `★5 を保って深く掘る` ワークフローは壊れない。
    ///
    /// 既に suppression 中ならネストさせず no-op (単一階層だけで十分)。
    pub(crate) fn maybe_suppress_rating_filter_for_opened_container(&mut self, idx: usize) {
        if self.rating_filter_suppressed_at.is_some() {
            return;
        }
        if !self.rating_filter_active() {
            return;
        }
        // item borrow は get_rating (mut) と競合するので先に path を取り出してから release する。
        let Some(anchor) = self
            .items
            .get(idx)
            .and_then(|it| it.container_path())
            .map(Path::to_path_buf)
        else {
            return;
        };
        let stars = self.get_rating(idx);
        if !(1..=5).contains(&stars) {
            return;
        }
        // bucket の index safety + `accepts_rating` 判定を passes_rating_filter に委譲。
        // ConvertibleArchive (7z/LZH) は accepts_rating=false で get_rating が必ず 0 を
        // 返すため、上の 1..=5 check で先に弾かれる (このブランチには到達しない)。
        let item = &self.items[idx];
        if !passes_rating_filter(item, stars, &self.settings.rating_filter) {
            // 自身の★が現在フィルタに通っていない = 親ビューで見えなかったはず
            // (= ユーザーが address bar や Ctrl+↑↓ で直接辿り着いた)。意図した操作と
            // 見なしてフィルタは保持する。
            return;
        }
        let saved = self.settings.rating_filter;
        self.rating_filter_suppressed_at = Some((anchor, saved));
        self.settings.rating_filter = [true; 6];
        // settings.save() は呼ばない (in-memory のみ、永続化は saved のまま維持)。
        self.show_feedback_toast("★フィルタ一時解除中 (親へ戻ると復元)".to_string());
    }

    /// `load_folder` の最後に呼ぶ: suppression anchor の subtree から外に出ていれば
    /// 保存したフィルタを復元する。subtree の内側にいれば no-op。
    pub(crate) fn maybe_restore_rating_filter_if_out_of_scope(&mut self) {
        let Some((anchor, _)) = self.rating_filter_suppressed_at.as_ref() else {
            return;
        };
        let Some(current) = self.effective_folder() else {
            // 現在フォルダが取れない状態 (起動直後など) は触らない
            return;
        };
        if path_in_subtree_ci(&current, anchor) {
            return;
        }
        self.restore_rating_filter_suppression();
    }

    /// suppression anchor を取り出し、保存されていたフィルタを復元する。
    /// suppression 中でなければ `false` を返す (呼び出し側が no-op を検出できる)。
    ///
    /// `maybe_restore_rating_filter_if_out_of_scope` (scope 外自動復元) と
    /// toolbar の「★一時解除中」バッジクリック (即時復元) の両方から使う。
    pub(crate) fn restore_rating_filter_suppression(&mut self) -> bool {
        if let Some((_, saved)) = self.rating_filter_suppressed_at.take() {
            self.settings.rating_filter = saved;
            // save() は呼ばない: suppression 中の書き換えも in-memory のみだったため、
            // 復元は単に元の saved 値に戻すだけで永続化する必要がない。
            true
        } else {
            false
        }
    }

    /// ユーザーが手動でフィルタを変更した時に呼ぶ: suppression anchor を破棄する。
    /// 復元はせず (現在のフィルタはユーザー自身の操作なのでそれが正)、anchor だけ消す。
    pub(crate) fn drop_rating_filter_suppression_on_user_edit(&mut self) {
        self.rating_filter_suppressed_at = None;
    }

    /// 毎フレーム呼ぶ: フィルタ ON かつ現フォルダ未スキャンなら worker 起動、
    /// フィルタ OFF ならバッファを片付ける。Ctrl+G 検索結果ビュー / rating_db 無し /
    /// current_folder 無しは no-op。
    ///
    /// change-detection は `folder_rating_counts_folder_key` のみを見る。handle の有無を
    /// 条件に入れると worker 終了直後 (handle=None) に再 spawn するループで点滅する。
    pub(crate) fn ensure_folder_rating_counter(&mut self) {
        if !self.rating_filter_active() {
            if self.folder_rating_counter_handle.is_some()
                || !self.folder_rating_counts.is_empty()
                || self.folder_rating_counts_folder_key.is_some()
            {
                self.reset_folder_rating_counts();
            }
            return;
        }

        let Some(current) = self.current_folder.clone() else {
            return;
        };
        if current == search_results_synthetic_path() || self.global_search.active {
            return;
        }
        if self.rating_db.is_none() {
            return;
        }
        let folder_key = crate::adjustment_db::normalize_path(&current);
        if self.folder_rating_counts_folder_key.as_deref() == Some(&folder_key) {
            return;
        }
        if let Some(h) = self.folder_rating_counter_handle.take() {
            h.cancel();
        }
        self.folder_rating_counts.clear();
        self.folder_rating_counts_loaded = false;
        let db_path = crate::data_dir::get().join("rating.db");
        self.folder_rating_counter_handle = Some(
            crate::folder_rating_counter::spawn_for_folder(db_path, folder_key.clone()),
        );
        self.folder_rating_counts_folder_key = Some(folder_key);
    }

    /// worker からの部分結果を取り込む。毎フレーム呼ぶ。
    /// 再描画は update ループ末尾の repaint ゲートが担当する。
    ///
    /// 100k+ 件の DB でキューに batch が溜まっていると `try_recv` 連続 drain が UI
    /// フレーム hitch になるため、1 フレームで最大 `MAX_BATCHES_PER_FRAME` 件までに
    /// 制限する (残りは次フレーム)。
    pub(crate) fn poll_folder_rating_counts(&mut self) {
        const MAX_BATCHES_PER_FRAME: usize = 8;
        let Some(h) = self.folder_rating_counter_handle.as_ref() else {
            return;
        };
        for _ in 0..MAX_BATCHES_PER_FRAME {
            match h.rx.try_recv() {
                Ok(batch) => {
                    for (key, counts) in batch.entries {
                        let e = self
                            .folder_rating_counts
                            .entry(key)
                            .or_insert([0u32; 5]);
                        for i in 0..5 {
                            e[i] = e[i].saturating_add(counts[i]);
                        }
                    }
                    if batch.finished {
                        self.folder_rating_counts_loaded = true;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.folder_rating_counts_loaded = true;
                    self.folder_rating_counter_handle = None;
                    return;
                }
            }
        }
    }

    /// 指定 idx (Folder / ZipFile / PdfFile) のフィルタ一致件数と per-star 内訳を返す。
    /// worker 未起動 / キー無し / フィルタ対象外 / 合計 0 件 は `None`。
    /// `per_star` はマスク前の全★別件数で、tooltip 内訳表示に使う。
    ///
    /// Ctrl+G drilled view の場合は `search_drilled_folder_counts` (★なし含む 6 バケット)
    /// を優先参照する。folder_rating_counter worker が Ctrl+G 中は走らないため、
    /// `state.all_hits` から直接集計した値をここで使う。
    ///
    /// バッジ合計は通常フォルダと同じく `rf[1..5]` (★1..★5) のみで集計する。
    /// `rf[0]` (なし) はフォルダ自身の表示可否 (`rebuild_visible_indices`) 側でしか
    /// 使わない — 通常表示と検索結果で見た目が揃うようにするため。
    pub(crate) fn folder_rating_match(&self, idx: usize) -> Option<(u32, [u32; 5])> {
        // フィルタ全 ON (= 非アクティブ) のときはバッジを出さない。通常フォルダは
        // ensure_folder_rating_counter がそもそも worker を起動せずカウントが空なので
        // 自然に None になる。検索結果 drilled view は search_drilled_folder_counts を
        // 常に保持しているため、ここで明示的に揃える必要がある。
        if !self.rating_filter_active() {
            return None;
        }
        let path = match self.items.get(idx)? {
            GridItem::Folder(p) | GridItem::ZipFile(p) | GridItem::PdfFile(p) => p,
            _ => return None,
        };
        let key = crate::adjustment_db::normalize_path(path);
        let rf = &self.settings.rating_filter;
        // 合成 drilled view (検索結果一覧そのもの) のときだけ search_drilled_folder_counts
        // を参照する。Ctrl+G 経由で実フォルダ/ZIP/PDF を開いた実体ビューでは
        // items_are_global_search_view=false になっており、items は実体側の中身なので
        // 検索ヒット由来のキャッシュを引くと別フォルダの件数を流用してしまう。
        let in_global_drilled = self.global_search.active
            && self.items_are_global_search_view
            && matches!(
                self.global_search.view,
                crate::global_search_ui::GlobalSearchView::DrilledInto { .. }
            );
        // tooltip 用 per_star は ★1..★5 (旧インターフェース維持)。
        let per_star: [u32; 5] = if in_global_drilled {
            let counts6 = *self.search_drilled_folder_counts.get(&key)?;
            [counts6[1], counts6[2], counts6[3], counts6[4], counts6[5]]
        } else {
            *self.folder_rating_counts.get(&key)?
        };
        // rating_filter[i+1] が ★(i+1) に対応。rf[0] は未評価フィルタなので無視。
        let total: u32 = (0..5)
            .filter(|i| *rf.get(i + 1).unwrap_or(&true))
            .map(|i| per_star[i])
            .sum();
        // 0 件はバッジ表示しない (「0→N」遷移で UI が揺れないように)。
        if total == 0 {
            return None;
        }
        Some((total, per_star))
    }

    /// rating が変わったとき、子孫集計に反映する。
    ///
    /// スキャン完了後 (`loaded=true`) なら old→new の delta で直下コンテナに増減を加える。
    /// スキャン進行中は、worker の batch と bump の加算順序で二重計上・取りこぼしが起きる
    /// (Codex レビュー P1) ので、局所 bump は諦めて worker を restart する。
    fn apply_rating_delta_to_folder_counts(&mut self, key: &str, old_stars: u8, new_stars: u8) {
        if old_stars == new_stars {
            return;
        }
        if self.folder_rating_counts_folder_key.is_none() {
            return;
        }
        if !self.folder_rating_counts_loaded {
            // スキャン中は restart で確実に最新値を反映させる。handle を落として
            // folder_key もクリアすることで次フレームの ensure_folder_rating_counter が
            // まっさらに spawn しなおす。
            self.reset_folder_rating_counts();
            return;
        }
        let folder_key = self.folder_rating_counts_folder_key.clone().unwrap();
        let prefix = format!("{folder_key}/");
        let Some(agg_key) =
            crate::folder_rating_counter::aggregation_key_for(key, &prefix).map(|s| s.to_string())
        else {
            return;
        };
        let entry = self
            .folder_rating_counts
            .entry(agg_key)
            .or_insert([0u32; 5]);
        if (1..=5).contains(&old_stars) {
            let i = (old_stars - 1) as usize;
            entry[i] = entry[i].saturating_sub(1);
        }
        if (1..=5).contains(&new_stars) {
            let i = (new_stars - 1) as usize;
            entry[i] = entry[i].saturating_add(1);
        }
    }

    /// フォルダ読み込み直後に rating_cache を一括プリウォームする。
    /// 単発の SELECT を N 回投げる代わりに `WHERE path IN (...)` を 1 回で済ませる。
    /// 結果に含まれないキーは 0 (未評価) としてキャッシュに入れ、以後の DB アクセスを抑制する。
    /// ページ単位とコンテナの両方を対象とする。
    pub(crate) fn prewarm_rating_cache(&mut self) {
        let db = match self.rating_db.as_ref() {
            Some(db) => db,
            None => return,
        };
        let mut idx_keys: Vec<(usize, String)> = Vec::with_capacity(self.items.len());
        for (idx, item) in self.items.iter().enumerate() {
            if !item.accepts_rating() {
                continue;
            }
            if let Some(k) = self.rating_path_key(idx) {
                idx_keys.push((idx, k));
            }
        }
        let keys: Vec<String> = idx_keys.iter().map(|(_, k)| k.clone()).collect();
        let map = db.get_many(&keys);
        for (idx, key) in idx_keys {
            let stars = map.get(&key).copied().unwrap_or(0);
            self.rating_cache.insert(idx, stars);
        }
    }

    /// `current_folder` (現在一覧表示中のフォルダ / ZIP / PDF) のレーティングを取得する。
    /// アドレスバー右端の★表示で毎フレーム呼ばれるため、`current_folder_rating_cache`
    /// でメモ化して SQLite クエリと path 正規化コストを回避する。
    /// 検索結果ビューなど、実在しない合成パスに対しては 0 を返す。
    /// Ctrl+G 検索中は `current_folder` が検索前のフォルダを指したままなので、
    /// 直前に開いていた実フォルダの★が表示されないように 0 を返す。
    pub(crate) fn current_folder_rating(&mut self) -> u8 {
        if let Some(v) = self.current_folder_rating_cache {
            return v;
        }
        let value = if self.global_search.active {
            0
        } else {
            match self.current_folder.as_ref() {
                Some(folder) if folder != &search_results_synthetic_path() => {
                    let key = crate::adjustment_db::normalize_path(folder);
                    self.rating_db.as_ref().map(|db| db.get(&key)).unwrap_or(0)
                }
                _ => 0,
            }
        };
        self.current_folder_rating_cache = Some(value);
        value
    }

    /// Shift+F1〜F6 成功時のトースト表示 (グリッド / フルスクリーン共通)。
    /// `stars == 0` は解除、1〜5 は★の並び。
    pub(crate) fn show_container_rating_toast(&mut self, stars: u8) {
        let msg = if stars == 0 {
            "[フォルダ★解除]".to_string()
        } else {
            format!("[フォルダ{}]", "★".repeat(stars as usize))
        };
        self.show_feedback_toast(msg);
    }

    /// `current_folder` にレーティングを設定する (Shift+F1〜F6 用)。
    /// 合成パス (検索結果ビュー等) はスキップして false を返す。
    /// Ctrl+G 検索中は `current_folder` が検索前のフォルダを指したままなので、
    /// 直前に開いていた実フォルダを誤って書き換えないよう false を返す。
    /// 成功時は rating_cache も同期し、visible_indices を rebuild する。
    pub(crate) fn set_current_folder_rating(&mut self, stars: u8) -> bool {
        if self.global_search.active {
            return false;
        }
        let Some(folder) = self.current_folder.clone() else {
            return false;
        };
        if folder == search_results_synthetic_path() {
            return false;
        }
        let stars = stars.min(5);
        let key = crate::adjustment_db::normalize_path(&folder);
        // Undo 用に変更前の値をキャッシュ or DB から取得 (なければ 0)。
        let before = self
            .current_folder_rating_cache
            .or_else(|| self.rating_db.as_ref().map(|db| db.get(&key)))
            .unwrap_or(0);
        if before != stars {
            self.capture_container_rating_undo(before, stars);
        }
        if let Some(db) = self.rating_db.as_ref() {
            let _ = db.set(&key, stars);
        }
        self.current_folder_rating_cache = Some(stars);
        // items 内の同じコンテナパスを指す Folder/ZipFile/PdfFile があればキャッシュ更新。
        // (1 階層上に戻ったときに cached な古い値を表示しないため。通常 current_folder は
        // items に含まれないが、search container 絞り込みビュー中は含まれうる。)
        let matching: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                let p = match it {
                    GridItem::Folder(p) | GridItem::ZipFile(p) | GridItem::PdfFile(p) => p,
                    _ => return None,
                };
                if crate::adjustment_db::normalize_path(p) == key {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        for i in matching {
            self.rating_cache.insert(i, stars);
        }
        self.rebuild_visible_indices();
        true
    }

    /// グリッドのタグバッジ表示用キャッシュを埋める。フォルダ切替時に 1 回呼ぶ。
    ///
    /// 1. **fts_meta.db から一括取得** (同期・高速): `auto_index_metadata=true` な
    ///    お気に入り配下のファイルは、SQLite `IN (...)` 1 回ですべて引ける。
    ///    非可視分も含めて一気に cache に載せる (HDD アクセスなし、DB は 1 クエリ)。
    /// 2. **残りの path の XMP 直読みは `enqueue_visible_tag_prewarms` に任せる**:
    ///    ここでは空キューの worker だけ用意しておき、以降のフレームで `keep_range`
    ///    分を逐次 push する。大規模フォルダで全 XMP をフォルダ開時に読む暴走を防ぐ。
    ///
    /// **重要**: `tags_cache` は `ui_metadata_panel::get_current_tags_cached` とも共有で、
    /// 値が `Some(Vec)` ならキャッシュヒットとして扱われ XMP 直読みをスキップする。
    pub(crate) fn prewarm_grid_tags(&mut self) {
        // 旧プリフェッチ (別フォルダの残り) は cancel: 以降の XMP 読みを無駄にしない。
        if let Some(pending) = self.tag_prewarm_pending.take() {
            pending.cancel();
        }
        self.tag_prewarm_queued.clear();

        // INDEX_VERSION=5 でタグ原文の SQLite 一括取得は廃止。tag_prewarm worker が
        // XMP dc:subject を 1 ファイルずつバックグラウンドで読み、`keep_range` 分を
        // 順次 `tags_cache` に載せる。グリッドのタグバッジは worker 完了後に出る。

        // worker は常に起動 (非インデックスファイルが 1 枚でもあれば必要になるし、
        // 空ループで 200ms ごとに timeout する程度なので常駐コストは無視できる)。
        self.tag_prewarm_pending = Some(crate::tag_prewarm::spawn());
    }

    /// 毎フレーム呼ぶ: 現在の `keep_set` (可視範囲 + prev/next ページの display list 部分列) に
    /// 含まれる Image アイテムのうち、`tags_cache` にまだ無いものを `tag_prewarm` worker に push する。
    /// `tag_prewarm_queued` (idx セット) で二重 push を防ぐので、毎フレームの走査は
    /// ~175 件 × HashSet lookup で十分軽い (過去は keep_range bounding box 比較で
    /// アイドルフレームを早期終了していたが、フィルタ変更で bbox 不変 × 内部構成変化 の
    /// ケースを取り逃す不具合があり廃止した)。
    pub(crate) fn enqueue_visible_tag_prewarms(&mut self) {
        // 削除進行中は新規 XMP プリウォームを停止。削除予定パスを worker が読もうとすると
        // File::open が失敗して無駄なキャッシュ汚染や redundant log になる。
        if self.delete_pending.is_some() {
            return;
        }
        let Some(pending) = self.tag_prewarm_pending.as_ref() else {
            return;
        };
        if self.keep_set.is_empty() {
            return;
        }
        for idx in self.keep_set_sorted() {
            if idx >= self.items.len() {
                continue;
            }
            // idx ベースで dedup: 処理済み idx は cache_key 文字列を組み立てずスキップ。
            if self.tag_prewarm_queued.contains(&idx) {
                continue;
            }
            let Some(GridItem::Image(p)) = self.items.get(idx) else {
                // 非 Image (Folder/Zip/Pdf/Video) もキャッシュ対象外。idx を記録して再走回避。
                self.tag_prewarm_queued.insert(idx);
                continue;
            };
            if !crate::xmp_writer::is_writable_format(p) {
                self.tag_prewarm_queued.insert(idx);
                continue;
            }
            let cache_key = crate::adjustment_db::normalize_path(p);
            // fts_meta / 書き込み worker で既に埋まっていれば push 不要だが idx は処理済み扱い。
            if self.tags_cache.contains_key(&cache_key) {
                self.tag_prewarm_queued.insert(idx);
                continue;
            }
            self.tag_prewarm_queued.insert(idx);
            // 設定 ON かつ DB で rating 未登録 (= 0) のときだけ XMP から rating も読んで
            // ハイドレートする。既に DB に値がある場合は XMP 読みを節約。
            let read_rating = self.settings.write_rating_to_xmp
                && self.rating_cache.get(&idx).copied().unwrap_or(0) == 0;
            pending.push_job(p.clone(), cache_key, read_rating);
        }
    }

    /// `tag_prewarm` ワーカーからの XMP プリフェッチ結果を UI スレッドで回収する。
    /// 既に `tags_cache` にエントリがある path (タグ書き込み worker が先に入れた新鮮な
    /// 状態 / 同フォルダ内で既に読み終えた path) は上書きしない。
    /// 毎フレーム `App::update` から呼ぶ。`on_result_drained` で in_flight を減らし、
    /// 残ジョブがあれば呼び出し側が `request_repaint()` する。
    pub(crate) fn poll_tag_prewarm_results(&mut self) {
        let Some(pending) = self.tag_prewarm_pending.as_ref() else {
            return;
        };
        // このフレームで届いた分だけ drain (or_insert 挙動は stale XMP 防御)。
        let mut rating_hydrations: Vec<(PathBuf, u8)> = Vec::new();
        loop {
            match pending.rx.try_recv() {
                Ok(res) => {
                    // XMP から rating > 0 が読めたらハイドレート候補に積む
                    // (DB を UI スレッドで触らないように後段でまとめて書く)。
                    if let Some(stars) = res.rating {
                        if stars > 0 {
                            rating_hydrations.push((res.path.clone(), stars));
                        }
                    }
                    self.tags_cache.entry(res.cache_key).or_insert(res.tags);
                    pending.on_result_drained();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.tag_prewarm_pending = None;
                    break;
                }
            }
        }
        // DB + rating_cache に反映。既に DB に非ゼロ値があれば (= ユーザーが mIV で
        // 付けた値) 上書きしない (旧ファイル移動先 → 新フォルダで XMP ハイドレートを
        // 期待するのは DB が 0 = 未登録のときだけ)。
        if !rating_hydrations.is_empty() {
            self.hydrate_ratings_from_xmp(rating_hydrations);
        }
    }

    /// tag_prewarm から回収した XMP 由来 rating を rating_db + rating_cache に反映する。
    /// DB に非ゼロ値がある idx はスキップ (mIV 側で付けた値を XMP で踏まない)。
    ///
    /// 実装: normalize_path を 1 回だけ走らせ、items の逆引き HashMap を 1 回構築、
    /// `db.get_many` でバッチ SELECT、何も変化しなければ `rebuild_visible_indices` は
    /// 呼ばない (大フォルダで UI スレッドに来たときの per-frame 再構築コスト回避)。
    fn hydrate_ratings_from_xmp(&mut self, hydrations: Vec<(PathBuf, u8)>) {
        let Some(db) = self.rating_db.as_ref() else {
            return;
        };
        if hydrations.is_empty() {
            return;
        }
        // key → XMP 由来★ の最終マップ (path の重複 push があっても 1 エントリに縮む)
        let mut target: std::collections::HashMap<String, u8> =
            std::collections::HashMap::with_capacity(hydrations.len());
        for (path, stars) in hydrations {
            target.insert(crate::adjustment_db::normalize_path(&path), stars);
        }
        // DB の現在値を 1 クエリでまとめて引く
        let keys: Vec<String> = target.keys().cloned().collect();
        let current = db.get_many(&keys);
        // DB が 0 (未登録) のエントリだけ実際にハイドレート対象にする。
        // ただし user_set_rating_keys にある path はユーザが明示的に書いたので、
        // 古い XMP 由来の値で上書きしない (F6 でクリア直後にレースで蘇るのを防止)。
        let mut to_write: Vec<(String, u8)> = Vec::new();
        for (key, stars) in &target {
            if current.get(key).copied().unwrap_or(0) == 0
                && !self.user_set_rating_keys.contains(key)
            {
                to_write.push((key.clone(), *stars));
            }
        }
        if to_write.is_empty() {
            return;
        }
        let write_keys: std::collections::HashSet<&String> =
            to_write.iter().map(|(k, _)| k).collect();
        // items の逆引き: path → idx (Image だけ)
        for (idx, item) in self.items.iter().enumerate() {
            if let GridItem::Image(p) = item {
                let k = crate::adjustment_db::normalize_path(p);
                if write_keys.contains(&k) {
                    let stars = target[&k];
                    self.rating_cache.insert(idx, stars);
                }
            }
        }
        for (key, stars) in &to_write {
            let _ = db.set(key, *stars);
        }
        // 子孫★集計バッジにも反映 (to_write は DB が 0 だった path のみ = old_stars=0)。
        for (key, stars) in &to_write {
            self.apply_rating_delta_to_folder_counts(key, 0, *stars);
        }
        // 実際に rating_cache が更新されたのでフィルタ影響あり
        self.rebuild_visible_indices();
    }

    /// 指定 idx の grid cell に描くタグ列。`tags_cache` のみを引く (同期 I/O を避ける)。
    /// キャッシュに載っていない = fts_meta 未登録 → 空を返す (バッジ非表示)。
    pub(crate) fn cell_tag_list(&self, idx: usize) -> &[String] {
        let Some(GridItem::Image(p)) = self.items.get(idx) else {
            return &[];
        };
        let key = crate::adjustment_db::normalize_path(p);
        self.tags_cache.get(&key).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// F 系・Ctrl+Num 系の一括適用で使う、グリッド上の対象 idx を決める共通規則。
    /// チェック済みがあればそれら、無ければカーソル位置 (selected)、それも無ければ空。
    /// 述語で「受け入れる GridItem 種別」を切り替える:
    /// - レーティング (F1〜F6) は `accepts_rating` (コンテナ含む)
    /// - 補正プリセット / マスクスロット (Ctrl+1〜0 / F7F8) は `has_page_data` (ページ専用)
    ///
    /// `rebuild_visible_indices` が `checked` / `selected` を可視範囲内に保つ不変条件を
    /// 維持しているが、`load_folder` の履歴復元で `selected` を後から差し戻すケース等で
    /// 一時的に非可視な idx を指す可能性があるので、ここでも `idx_visible` で防御する。
    fn targets_matching(&self, pred: impl Fn(&GridItem) -> bool) -> Vec<usize> {
        if !self.checked.is_empty() {
            self.checked
                .iter()
                .copied()
                .filter(|&idx| self.idx_visible(idx) && self.items.get(idx).is_some_and(&pred))
                .collect()
        } else if let Some(idx) = self.selected
            && self.idx_visible(idx)
            && self.items.get(idx).is_some_and(&pred)
        {
            vec![idx]
        } else {
            Vec::new()
        }
    }

    fn ratable_targets(&self) -> Vec<usize> {
        self.targets_matching(GridItem::accepts_rating)
    }

    fn ratable_page_targets(&self) -> Vec<usize> {
        self.targets_matching(GridItem::has_page_data)
    }

    /// グリッド画面から F7/F8 で呼ばれるマスクスロット一括適用。
    /// 実際の inpaint は各ページをフルスクリーンで開いたときに `auto_apply_saved_mask`
    /// が走る。ここでは DB + サイドカーにスロットの内容 (ビットマップ + ベクタ) を
    /// 書き込むだけで十分。
    pub(crate) fn apply_slot_to_selection(&mut self, slot: usize) {
        let targets = self.ratable_page_targets();

        if targets.is_empty() {
            self.show_feedback_toast("[適用対象なし]".to_string());
            return;
        }

        // スロットのデータを一度だけ取得 (元サイズのまま)。
        // 各ページに書き込むとき w/h はスロットの元サイズとして記録し、
        // フルスクリーンで開かれたときに get_full 側で自動リスケールされる。
        let (slot_bitmap, slot_vectors, sw, sh) = match self.mask_db.as_ref() {
            Some(db) => {
                let Some((sw, sh)) = db.slot_size(slot) else {
                    self.show_feedback_toast(format!("[スロット{slot}は空です]"));
                    return;
                };
                let Some((mask, vectors)) = db.get_slot_full(slot, sw, sh) else {
                    self.show_feedback_toast(format!("[スロット{slot}は空です]"));
                    return;
                };
                if !mask.iter().any(|&m| m) && vectors.is_empty() {
                    self.show_feedback_toast(format!("[スロット{slot}は空です]"));
                    return;
                }
                (mask, vectors, sw, sh)
            }
            None => {
                self.show_feedback_toast("[マスクDB未初期化]".to_string());
                return;
            }
        };

        // 圧縮・JSON 化はループ外で 1 回だけ: N ページに同じマスクを配るので
        // deflate を共有する (N=100 等で実測効果が大きい)。
        let compressed = crate::mask_db::compress_mask(&slot_bitmap);
        let vectors_json = crate::mask_db::vectors_to_json(&slot_vectors);
        let total = targets.len();
        for idx in &targets {
            // フルスクリーンで現在これらのページを開いている可能性は低い (grid モード) が、
            // 念のため inpaint 結果キャッシュを落として次回開いたときに再適用させる。
            self.erase_base_cache.remove(idx);
            self.fs_cache.remove(idx);
            self.save_mask_raw_with_sidecar(
                *idx,
                &compressed,
                &slot_vectors,
                vectors_json.as_deref(),
                sw,
                sh,
            );
        }

        self.checked.clear();
        crate::logger::log(format!("[ERASE] Bulk apply slot {slot} to {total} items"));
        self.show_feedback_toast(format!("[スロット{slot} を{total}枚に適用]"));
    }

    pub(crate) fn apply_rating_to_selection(&mut self, stars: u8) {
        let stars = stars.min(5);
        let targets = self.ratable_targets();
        if targets.is_empty() {
            return;
        }
        // set_rating 内で rating_cache が書き換わるので、Undo 用 before は事前に確定させる。
        let mut undo_records: Vec<(usize, u8, u8)> = Vec::with_capacity(targets.len());
        for &idx in &targets {
            let before = self.rating_cache.get(&idx).copied().unwrap_or(0);
            undo_records.push((idx, before, stars));
        }
        let summary = if targets.len() > 1 {
            if stars == 0 {
                format!("★解除を {} 件に適用", targets.len())
            } else {
                format!("★{stars} を {} 件に付与", targets.len())
            }
        } else if stars == 0 {
            "★解除".to_string()
        } else {
            format!("★{stars}")
        };
        self.capture_rating_undo(undo_records, summary);
        for &idx in &targets {
            self.set_rating(idx, stars);
        }
        let bulk = targets.len() > 1;
        if bulk {
            crate::logger::log(format!(
                "[RATING] Bulk apply {stars} stars to {} items",
                targets.len(),
            ));
            self.checked.clear();
        } else {
            crate::logger::log(format!("[RATING] Set {stars} stars on idx {}", targets[0]));
        }
        // Ctrl+G ヒットの stars はバッチ受信時の snapshot。レーティング変更後に
        // drilled view のサブフォルダ件数 / 枝刈りが古いまま残らないよう、
        // 該当する all_hits エントリを最新値で書き換える (Codex P3)。
        // 合成ビューを再構築するのは現在 items が合成ビュー由来のときだけ:
        // Ctrl+G から実フォルダ/ZIP/PDF を開いた直後は items が PDF ページ等の
        // 実体ビューなので、ここで rebuild_items_from_global_search を呼ぶと
        // ページ一覧が検索 drilled view の合成 items に置き換わる事故になる
        // (Codex P2)。
        if self.global_search.active && !targets.is_empty() {
            self.refresh_global_search_hit_stars(&targets);
        }
        if self.global_search.active && self.items_are_global_search_view {
            self.rebuild_items_from_global_search();
        } else {
            self.rebuild_visible_indices();
        }
    }

    /// `set_rating` で書き換えた idx 群について、Ctrl+G の `all_hits` 中の対応する
    /// hit エントリの `stars` を最新値で更新する。drilled view バッジ件数 / 枝刈り /
    /// 直下ファイルフィルタが snapshot のまま古くならないようにする (Codex P3)。
    fn refresh_global_search_hit_stars(&mut self, idxs: &[usize]) {
        // 対象 idx → (rating_db キー, 新 stars) の対応を作る
        let mut updates: std::collections::HashMap<String, u8> =
            std::collections::HashMap::new();
        for &idx in idxs {
            let Some(key) = self.rating_path_key(idx) else {
                continue;
            };
            let new_stars = self.rating_cache.get(&idx).copied().unwrap_or(0);
            updates.insert(key, new_stars);
        }
        if updates.is_empty() {
            return;
        }
        for h in self.global_search.all_hits.iter_mut() {
            let hit_key = crate::global_search_ui::hit_rating_key(&h.path);
            if let Some(&v) = updates.get(&hit_key) {
                h.stars = v;
            }
        }
    }

    /// 1枚のフルサイズ画像を非同期で読み込み開始する。
    /// 通常画像 / ZIP エントリ / PDF ページ の全てに対応。
    /// GIF / APNG はアニメーションフレームを全デコードして FsLoadResult::Animated を送信する。
    pub(crate) fn start_fs_load(&mut self, idx: usize) {
        // 動画は専用パス: VideoPlayer を作って fs_cache に直接入れる。
        // 通常の画像デコードスレッドは起動しない。
        if let Some(GridItem::Video(vp)) = self.items.get(idx).cloned() {
            // 1 動画につき 1 cpal stream に限定する。`contains_key(idx)` チェックの
            // 外で実行することで、再 start_fs_load (= fullscreen 戻り遷移) でも
            // 古い video エントリが残らないことを保証する (Codex 助言)。
            let other_video_idxs: Vec<usize> = self
                .fs_cache
                .iter()
                .filter_map(|(k, v)| {
                    if *k != idx && matches!(v, FsCacheEntry::Video { .. }) {
                        Some(*k)
                    } else {
                        None
                    }
                })
                .collect();
            if !other_video_idxs.is_empty() {
                self.save_all_video_resume_positions();
                for k in other_video_idxs {
                    self.fs_cache.remove(&k);
                }
            }
            if !self.fs_cache.contains_key(&idx) {
                // start_muted の場合も volume はそのままにし、Player 内 atomic で
                // ミュートだけ切る (= 解除時に元の音量に戻る)。0.0 を渡してしまうと
                // 「ミュート解除しても音が出ない」状態になり Codex に指摘された。
                let vol = self.settings.video_volume;
                let autoplay = self.settings.video_autoplay;
                // resume 位置の取得: 直近に保存した位置を渡して最初の info 受領後に
                // 自動シークさせる。Windows の case-insensitive パスを揃えるため
                // adjustment_db::normalize_path に合わせる。
                let path_key = crate::adjustment_db::normalize_path(&vp);
                let resume = self
                    .settings
                    .video_resume_positions
                    .get(&path_key)
                    .copied();
                let player = crate::video::VideoPlayer::open(
                    vp,
                    vol,
                    autoplay,
                    resume,
                    self.settings.video_hw_decode,
                    // GPU レンダリングが利用可能なら渡す。HW デコード OFF の場合や
                    // GpuVideoDevice 作成失敗時は None で旧経路 (CPU readback)。
                    #[cfg(windows)]
                    if self.settings.video_hw_decode {
                        self.gpu_video_device.clone()
                    } else {
                        None
                    },
                );
                if self.settings.video_start_muted {
                    player.set_muted(true);
                }
                self.fs_cache.insert(
                    idx,
                    FsCacheEntry::Video {
                        player: Box::new(player),
                        load_seq: self.input_seq,
                    },
                );
            }
            return;
        }

        let (path, zip_entry, pdf_page, pdf_password) = match self.items.get(idx) {
            Some(GridItem::Image(p)) => (p.clone(), None, None, None),
            Some(GridItem::ZipImage {
                zip_path,
                entry_name,
            }) => (zip_path.clone(), Some(entry_name.clone()), None, None),
            Some(GridItem::PdfPage {
                pdf_path, page_num, ..
            }) => (
                pdf_path.clone(),
                None,
                Some(*page_num),
                self.pdf_current_password.clone(),
            ),
            _ => return,
        };

        // フルスクリーン現在ページ (ユーザーが待っているもの) は Critical、
        // それ以外 (先読み) は Normal。Critical はプールの予約ワーカーで即処理される。
        let pdf_priority = if self.fullscreen_idx == Some(idx) {
            crate::pdf_loader::JobPriority::Critical
        } else {
            crate::pdf_loader::JobPriority::Normal
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<FsLoadResult>();
        let pdf_ct_tx = self.pdf_content_type_tx.clone();
        let perf_key = self.perf_item_key(idx);
        let perf_seq = self.input_seq;
        self.fs_pending
            .insert(idx, (Arc::clone(&cancel), rx, perf_seq));
        if crate::perf::is_enabled() {
            crate::perf::event(
                "fs",
                "load_begin",
                perf_key.as_deref(),
                perf_seq,
                &[
                    ("idx", serde_json::Value::from(idx)),
                    ("is_pdf", serde_json::Value::from(pdf_page.is_some())),
                    ("is_zip", serde_json::Value::from(zip_entry.is_some())),
                    (
                        "priority",
                        serde_json::Value::from(format!("{pdf_priority:?}")),
                    ),
                ],
            );
        }
        let perf_key_worker = perf_key.clone();

        std::thread::spawn(move || {
            // perf: スレッド起動直後の cancel 状態を記録し、早期終了した本数を把握する
            let early_cancelled = cancel.load(Ordering::Relaxed);
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "fs",
                    "cancel_check",
                    perf_key_worker.as_deref(),
                    perf_seq,
                    &[("cancelled", serde_json::Value::from(early_cancelled))],
                );
            }
            // スレッド出口で reason を記録する小ヘルパー (全 return 直前に呼ぶ)
            let emit_exit = |reason: &'static str| {
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "fs",
                        "thread_exit",
                        perf_key_worker.as_deref(),
                        perf_seq,
                        &[("reason", serde_json::Value::from(reason))],
                    );
                }
            };
            if early_cancelled {
                emit_exit("early_cancel");
                return;
            }
            let t = std::time::Instant::now();
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "fs",
                    "decode_begin",
                    perf_key_worker.as_deref(),
                    perf_seq,
                    &[],
                );
            }

            // ローカル画像ファイルはヘッダ数バイトで寸法が取れる (数 ms)。
            // 本デコード前にホバーバーへサイズ / ダウンスケール警告を出すため先行送信。
            if pdf_page.is_none() && zip_entry.is_none() {
                if let Some(dims) = crate::fast_resize::probe_dims(&path) {
                    let _ = tx.send(FsLoadResult::DimsOnly { source_dims: dims });
                }
            }

            // 表示名と拡張子を取得
            let (name, ext) = if let Some(page_num) = pdf_page {
                (format!("Page {}", page_num + 1), "pdf".to_string())
            } else if let Some(ref entry_name) = zip_entry {
                let base = crate::zip_loader::entry_basename(entry_name).to_string();
                let ext = base.rsplit('.').next().unwrap_or("").to_lowercase();
                (base, ext)
            } else {
                let n = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string();
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                (n, ext)
            };

            // PDF ページの場合はラスタライズ
            if let Some(page_num) = pdf_page {
                let target_px = 4096u32;
                let mut pdf_exit = "pdf_ok";
                match crate::pdf_loader::render_page(
                    &path,
                    page_num,
                    target_px,
                    pdf_password.as_deref(),
                    Some(cancel.clone()),
                    pdf_priority,
                ) {
                    Ok((img, content_type)) => {
                        let elapsed = t.elapsed().as_secs_f64() * 1000.0;
                        crate::logger::log(format!(
                            "  fs load pdf: {elapsed:.0}ms  idx={idx}  {name}  {}x{}  {:?}",
                            img.width(),
                            img.height(),
                            content_type
                        ));
                        // GPU テクスチャ上限に収まらない巨大レンダ結果をここで縮小しておく。
                        // (request_pdf_rerender は 8192 に clamp 済なので通常は no-op)
                        let source_dims = [img.width() as usize, img.height() as usize];
                        let img = clamp_dynamic_for_gpu(img);
                        let (w, h) = (img.width(), img.height());
                        let ci = dynamic_image_to_color_image(&img);
                        if crate::perf::is_enabled() {
                            crate::perf::event(
                                "fs",
                                "decode_end",
                                perf_key_worker.as_deref(),
                                perf_seq,
                                &[
                                    ("ms", serde_json::Value::from(elapsed)),
                                    ("format", serde_json::Value::from("pdf")),
                                    ("w", serde_json::Value::from(w)),
                                    ("h", serde_json::Value::from(h)),
                                ],
                            );
                        }
                        let _ = tx.send(FsLoadResult::Static { ci, source_dims });
                        // content_type をメインスレッドに送る
                        let _ = pdf_ct_tx.send((idx, content_type));
                    }
                    Err(e) => {
                        if cancel.load(Ordering::Relaxed) {
                            crate::logger::log(format!("  fs pdf render cancelled  {name}"));
                            if crate::perf::is_enabled() {
                                crate::perf::event(
                                    "fs",
                                    "decode_cancel",
                                    perf_key_worker.as_deref(),
                                    perf_seq,
                                    &[],
                                );
                            }
                            pdf_exit = "pdf_cancel";
                        } else {
                            crate::logger::log(format!("  fs pdf render FAIL: {e}  {name}"));
                            if crate::perf::is_enabled() {
                                crate::perf::event(
                                    "fs",
                                    "decode_fail",
                                    perf_key_worker.as_deref(),
                                    perf_seq,
                                    &[("format", serde_json::Value::from("pdf"))],
                                );
                            }
                            let _ = tx.send(FsLoadResult::Failed);
                            pdf_exit = "pdf_fail";
                        }
                    }
                }
                emit_exit(pdf_exit);
                return;
            }

            // ZIP エントリの場合は先にバイト列を抽出
            let zip_bytes: Option<Vec<u8>> = if let Some(ref entry_name) = zip_entry {
                match crate::zip_loader::read_entry_bytes(&path, entry_name) {
                    Ok(b) => Some(b),
                    Err(e) => {
                        crate::logger::log(format!("  fs zip read FAIL: {e}  {name}"));
                        if crate::perf::is_enabled() {
                            crate::perf::event(
                                "fs",
                                "decode_fail",
                                perf_key_worker.as_deref(),
                                perf_seq,
                                &[("format", serde_json::Value::from("zip_read"))],
                            );
                        }
                        emit_exit("zip_read_fail");
                        return;
                    }
                }
            } else {
                None
            };

            // GIF: アニメーション試行 (通常パスのみ, ZIP は未対応)
            if ext == "gif" && zip_bytes.is_none() {
                if let Some(frames) = decode_gif_frames(&path) {
                    let elapsed = t.elapsed().as_secs_f64() * 1000.0;
                    crate::logger::log(format!(
                        "  fs load anim-gif: {elapsed:.0}ms  idx={idx}  {name}  {} frames",
                        frames.len()
                    ));
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "fs",
                            "decode_end",
                            perf_key_worker.as_deref(),
                            perf_seq,
                            &[
                                ("ms", serde_json::Value::from(elapsed)),
                                ("format", serde_json::Value::from("gif_anim")),
                                ("frames", serde_json::Value::from(frames.len())),
                            ],
                        );
                    }
                    let _ = tx.send(FsLoadResult::Animated(frames));
                    emit_exit("gif_anim");
                    return;
                }
            }

            // PNG: APNG アニメーション試行 (通常パスのみ, ZIP は未対応)
            if ext == "png" && zip_bytes.is_none() {
                if let Some(frames) = decode_apng_frames(&path) {
                    let elapsed = t.elapsed().as_secs_f64() * 1000.0;
                    crate::logger::log(format!(
                        "  fs load anim-png: {elapsed:.0}ms  idx={idx}  {name}  {} frames",
                        frames.len()
                    ));
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "fs",
                            "decode_end",
                            perf_key_worker.as_deref(),
                            perf_seq,
                            &[
                                ("ms", serde_json::Value::from(elapsed)),
                                ("format", serde_json::Value::from("png_anim")),
                                ("frames", serde_json::Value::from(frames.len())),
                            ],
                        );
                    }
                    let _ = tx.send(FsLoadResult::Animated(frames));
                    emit_exit("png_anim");
                    return;
                }
            }

            // 静止画フォールバック
            // image クレート → WIC → Susie プラグインの順で試す
            // ZIP エントリは SHCreateMemStream + CreateDecoderFromStream 経由で WIC へフォールバック
            let open_result = if let Some(bytes) = zip_bytes {
                let hint = zip_entry.as_deref().unwrap_or("");
                match image::load_from_memory(&bytes) {
                    Ok(img) => Ok(img),
                    Err(e) => {
                        match crate::wic_decoder::decode_to_dynamic_image_from_bytes(&bytes) {
                            Some(img) => Ok(img),
                            // フルスクリーン画像ロードは現在表示中のため priority=true
                            None => {
                                match crate::susie_loader::decode_bytes(hint, &bytes, true, None) {
                                    Ok(img) => Ok(img),
                                    Err(_) => Err(e),
                                }
                            }
                        }
                    }
                }
            } else {
                match image::open(&path) {
                    Ok(img) => Ok(img),
                    Err(e) => match crate::wic_decoder::decode_to_dynamic_image(&path) {
                        Some(img) => Ok(img),
                        // フルスクリーン画像ロードは現在表示中のため priority=true
                        None => match crate::susie_loader::decode_file(&path, true, None) {
                            Ok(img) => Ok(img),
                            Err(_) => Err(e),
                        },
                    },
                }
            };
            match open_result {
                Ok(img) => {
                    // EXIF Orientation 自動回転 (ZIP 以外)
                    let img = if zip_entry.is_none() {
                        crate::thumb_loader::apply_exif_orientation(img, &path)
                    } else {
                        img
                    };
                    // GPU テクスチャ上限 (MAX_TEXTURE_DIM=8192) を超える巨大画像は
                    // worker で DynamicImage のまま Triangle リサイズしておく。
                    // UI スレッド側の clamp_for_gpu は ColorImage↔DynamicImage の
                    // premultiply/unmultiply ループが重く、7K-9K クラスで 5s/回ブロックする。
                    let source_dims = [img.width() as usize, img.height() as usize];
                    let img = clamp_dynamic_for_gpu(img);
                    let (w, h) = (img.width(), img.height());
                    let ci = dynamic_image_to_color_image(&img);
                    let elapsed = t.elapsed().as_secs_f64() * 1000.0;
                    crate::logger::log(format!(
                        "  fs load: {elapsed:.0}ms  idx={idx}  {name}  {w}x{h}"
                    ));
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "fs",
                            "decode_end",
                            perf_key_worker.as_deref(),
                            perf_seq,
                            &[
                                ("ms", serde_json::Value::from(elapsed)),
                                ("format", serde_json::Value::from(ext.clone())),
                                ("w", serde_json::Value::from(w)),
                                ("h", serde_json::Value::from(h)),
                            ],
                        );
                    }
                    let _ = tx.send(FsLoadResult::Static { ci, source_dims });
                    emit_exit("static_ok");
                }
                Err(e) => {
                    crate::logger::log(format!("  fs load FAIL: {e}  {name}"));
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "fs",
                            "decode_fail",
                            perf_key_worker.as_deref(),
                            perf_seq,
                            &[("format", serde_json::Value::from(ext.clone()))],
                        );
                    }
                    // UI が「読込中...」のまま固まらないよう、失敗を明示的に通知する
                    let _ = tx.send(FsLoadResult::Failed);
                    emit_exit("static_fail");
                }
            }
        });
    }

    /// PDF ページをズーム倍率に応じた解像度で非同期再レンダリングする。
    ///
    /// ワーカーに直接リクエストを送り、結果は `poll_pdf_rerender` で受け取る。
    /// UI スレッドを一切ブロックしない。
    pub(crate) fn request_pdf_rerender(&mut self, idx: usize, zoom: f32) {
        let (pdf_path, page_num, password, content_type) = match self.items.get(idx) {
            Some(GridItem::PdfPage {
                pdf_path,
                page_num,
                content_type,
            }) => (
                pdf_path.clone(),
                *page_num,
                self.pdf_current_password.clone(),
                *content_type,
            ),
            _ => return,
        };

        // 上限 8192: これ以上大きいとテクスチャメモリが巨大になりクラッシュする
        // (8192px 正方形 ≈ 256 MB RGBA、16384px ≈ 1 GB)
        // ラスターページでネイティブ解像度が AI アップスケール対象内の場合のみ
        // 原寸基準でレンダリング。それ以外は従来通り 4096px 基準。
        let base_px = match content_type {
            Some(crate::pdf_loader::PdfPageContentType::Raster { w, h }) => {
                let native_long = w.max(h);
                let threshold = self.settings.ai_upscale_skip_px;
                if native_long < threshold {
                    native_long as f32
                } else {
                    4096.0
                }
            }
            _ => 4096.0, // Vector or not yet analyzed
        };
        let target_px = ((base_px * zoom) as u32).clamp(256, 8192);

        // 既に同じ解像度のキャッシュがあれば不要
        if let Some(FsCacheEntry::Static { pixels, .. }) = self.fs_cache.get(&idx) {
            let cached_long = pixels.size[0].max(pixels.size[1]) as u32;
            let ratio = cached_long as f32 / target_px as f32;
            if (0.9..=1.1).contains(&ratio) {
                return;
            }
        }

        // 進行中の再レンダリングがあればキャンセル
        if let Some((cancel, _, _)) = self.fs_pending.remove(&idx) {
            cancel.store(true, Ordering::Relaxed);
        }

        // ワーカーに非同期リクエスト (UI スレッドをブロックしない)
        let (cancel, render_rx) = crate::pdf_loader::render_page_async(
            &pdf_path,
            page_num,
            target_px,
            password.as_deref(),
        );

        // render_page_async は DynamicImage チャネルを返すが、fs_pending は
        // FsLoadResult チャネルを期待するため、ブリッジスレッドで変換する
        let (fs_tx, fs_rx) = mpsc::channel::<FsLoadResult>();
        let perf_seq = self.input_seq;
        self.fs_pending
            .insert(idx, (Arc::clone(&cancel), fs_rx, perf_seq));

        std::thread::spawn(move || {
            match render_rx.recv() {
                Ok(Ok((img, _content_type))) => {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    crate::logger::log(format!(
                        "  pdf rerender done: page={} target_px={target_px} {}x{}",
                        page_num + 1,
                        img.width(),
                        img.height()
                    ));
                    // PDF 再レンダ結果は request_pdf_rerender が 8192 に clamp してから
                    // 投げているので `clamp_dynamic_for_gpu` は実質 no-op だが、不変条件
                    // (fs_cache.pixels ≤ MAX_TEXTURE_DIM) を型レベルで保つため通す。
                    let source_dims = [img.width() as usize, img.height() as usize];
                    let img = clamp_dynamic_for_gpu(img);
                    let ci = dynamic_image_to_color_image(&img);
                    let _ = fs_tx.send(FsLoadResult::Static { ci, source_dims });
                }
                Ok(Err(e)) => {
                    crate::logger::log(format!("  pdf rerender FAIL: {e}"));
                    let _ = fs_tx.send(FsLoadResult::Failed);
                }
                Err(_) => {
                    crate::logger::log("  pdf rerender: cancelled (channel closed)".to_string());
                    // キャンセル時は fs_tx を drop して poll_prefetch が Disconnected で除去
                }
            }
        });
    }

    /// 先読みウィンドウを更新する。
    /// settings の prefetch_back / prefetch_forward に従って先読みを開始し、
    /// ウィンドウ外のキャッシュ・読み込みを破棄する。
    fn update_prefetch_window(&mut self, current_idx: usize) {
        let image_indices = Self::collect_image_indices(&self.items);
        let Some(pos) = image_indices.iter().position(|&i| i == current_idx) else {
            return;
        };
        let n = image_indices.len();

        let pf_back = self.settings.prefetch_back;
        let pf_forward = self.settings.prefetch_forward;
        // KEEP はそれぞれ +1 だけ広く保持してテクスチャ破棄を遅延させる
        let keep_back = pf_back + 1;
        let keep_forward = pf_forward + 1;

        let keep_set: std::collections::HashSet<usize> = (pos.saturating_sub(keep_back)
            ..=((pos + keep_forward).min(n - 1)))
            .map(|p| image_indices[p])
            .collect();

        let prefetch_targets: Vec<usize> =
            interleaved_prefetch_targets(&image_indices, pos, n, pf_forward, pf_back);

        // 退避前に動画の再生位置を保存 (退避された Video エントリは drop で消える)
        self.save_all_video_resume_positions();
        // KEEP 範囲外のテクスチャを破棄（VRAM 節約）
        self.fs_cache.retain(|k, _| keep_set.contains(k));

        // 現在表示中の画像がまだデコード中 (fs_cache に入っていない) なら、
        // その画像に CPU を独占させるため他の pending をすべてキャンセルする。
        // 現在画像が完了したあと poll_prefetch から再度 update_prefetch_window が
        // 呼ばれ、そこで先読みが開始される。これにより 1→2 遷移 (サムネ → フル解像度) が
        // 先読みスレッドに待たされなくなる。
        let current_loading = !self.fs_cache.contains_key(&current_idx);

        let to_cancel: Vec<usize> = self
            .fs_pending
            .keys()
            .filter(|&&k| {
                if k == current_idx {
                    return false;
                }
                // 現在画像がロード中なら全 pending をキャンセル。そうでなければ KEEP 範囲外のみ。
                current_loading || !keep_set.contains(&k)
            })
            .cloned()
            .collect();
        for k in to_cancel {
            if let Some((cancel, _, _)) = self.fs_pending.remove(&k) {
                cancel.store(true, Ordering::Relaxed);
            }
            // 先行 dims ヒントはキャンセルされた idx では意味がないので破棄。
            self.fs_early_dims.remove(&k);
        }

        if current_loading {
            return;
        }

        // まだキャッシュにも pending にもない先読み対象を読み込み開始
        for idx in prefetch_targets {
            if !self.fs_cache.contains_key(&idx) && !self.fs_pending.contains_key(&idx) {
                crate::logger::log(format!("  prefetch start idx={idx}"));
                self.start_fs_load(idx);
            }
        }
    }

    /// items の中の画像アイテム (通常 + ZIP 内) の item_idx 一覧を返す（先読みウィンドウ用）
    fn collect_image_indices(items: &[GridItem]) -> Vec<usize> {
        items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                matches!(
                    item,
                    GridItem::Image(_) | GridItem::ZipImage { .. } | GridItem::PdfPage { .. }
                )
                .then_some(i)
            })
            .collect()
    }

    /// フルスクリーン表示を終了し、先読みキャッシュを全クリアする。
    ///
    /// `fs_viewport_shown` は意図的に残す: 次フレームの
    /// `keep_fullscreen_viewport_alive` がこのフラグを見て Visible(false) を
    /// 送信し、その直後に false に落とす。ここで先に落とすと送信が抑止される。
    pub(crate) fn close_fullscreen(&mut self) {
        // フルスクリーン解除前に動画再生位置を保存 (drop で消える前に)
        self.save_all_video_resume_positions();
        // perf: close_fullscreen は fs_cache / ai_upscale_cache / pending スレッドの
        // キャンセル通知を行うため、Ctrl+↑↓ (Fullscreen モード) の sync パスで
        // 実行される。ms を計測してブロックの所在を特定する。
        let cf_t0 = std::time::Instant::now();
        let cf_seq = self.input_seq;
        let cf_was_open = self.fullscreen_idx.is_some();
        let cf_fs_cache = self.fs_cache.len();
        let cf_ai_cache = self.ai_upscale_cache.len();
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "close_fullscreen_begin",
                None,
                cf_seq,
                &[
                    ("was_open", serde_json::Value::from(cf_was_open)),
                    ("fs_cache", serde_json::Value::from(cf_fs_cache)),
                    ("ai_cache", serde_json::Value::from(cf_ai_cache)),
                ],
            );
        }
        // フルスクリーン発起点の Ctrl+↑↓ DFS が走っていたら、ユーザーが
        // フルスクリーンを抜けた = フルスクリーン復帰の意図がなくなったとみなし
        // キャンセルする。apply_folder_nav_result 内の close_fullscreen
        // (Fullscreen ブランチ) は既に poll で folder_nav_pending を取り出した後
        // なので self.folder_nav_pending は None = ここでキャンセルされない。
        if let Some(pending) = self.folder_nav_pending.as_ref() {
            if matches!(pending.mode, FolderNavMode::Fullscreen) {
                pending.cancel.store(true, Ordering::Relaxed);
                self.folder_nav_pending = None;
                self.pending_folder_nav_steps = 0;
                self.pending_folder_nav_mode = FolderNavMode::Grid;
                // ユーザーが Esc 等で DFS 中にフルスクリーンを抜けた場合、items は
                // 入れ替わらないので poll_fs_nav_lock では release されない。
                // ここで明示的に lock / holdover を捨てる (Codex P1)。
                // apply_folder_nav_result 内の close_fullscreen は事前に
                // pending を take しているので folder_nav_pending=None でこの分岐に
                // 入らず、内部 close→open 遷移のロックは保持される。
                self.release_fs_nav_lock();
            }
        }
        self.fullscreen_idx = None;
        // ※ ここで `fs_nav_locked` / `fs_holdover_tex` を即時クリアしてはいけない:
        //   `apply_folder_nav_result` の Fullscreen 分岐は close_fullscreen → load_folder
        //   → open_fullscreen と直列に呼ぶので、その途中で lock を捨てると新ページが
        //   thumb_tex 未準備のまま「ファイル名のみ」表示になる。`poll_fs_nav_lock` 側で
        //   フルスクリーンが本当に閉じた (= 次フレームに `fullscreen_idx` が None のまま)
        //   ことを確認してから解除する。
        // グリッドに戻るので Critical 予約を解除し、全 3 ワーカーを Normal に開放。
        crate::pdf_loader::set_critical_reservation(false);
        // フルスクリーン中の Undo スタックはここで破棄。グリッドに戻ったら新しい操作を積む。
        self.clear_meta_undo();
        self.slideshow_playing = false;
        self.fs_opened_at = None;
        self.fs_focus_grace_elapsed = false;
        self.fs_prev_focused = false;
        self.fs_focus_regained_at = None;
        self.fs_suppress_primary_until_release = false;
        self.fs_secondary_press_start = None;
        self.fs_middle_zoom_drag = None;
        self.fs_context_menu_idx = None;
        // Phase 5.5: タイルモードもフルスクリーン解除と同時に閉じる (Codex H2 反映)。
        // Phase 6.C: 左ジャンプパネルのサムネ texture キャッシュもクリア。
        #[cfg(windows)]
        {
            self.video_tile_state = None;
            self.video_tile_textures.clear();
            self.video_jump_textures.clear();
        }
        self.reset_erase_mode();
        self.erase_base_cache.clear();
        for (cancel, _, _) in self.fs_pending.values() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.fs_pending.clear();
        self.fs_early_dims.clear();
        self.fs_cache.clear();
        // backlog はフォルダ外の画像 ColorImage を保持しているため、閉じたら破棄する。
        // 保持していると次フォルダで同 idx に違う画像が割当たって表示が化ける。
        self.fs_upload_backlog.clear();
        // AI キャッシュもクリア
        for (cancel, _) in self.ai_upscale_pending.values() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.ai_upscale_pending.clear();
        self.ai_upscale_cache.clear();
        self.ai_classify_cache.clear();
        // フルスクリーン向けメタデータ読み込みもキャンセル (閉じた後のキャッシュ書き込みを防ぐ)
        if let Some(pending) = self.metadata_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        if crate::perf::is_enabled() {
            crate::perf::event(
                "nav",
                "close_fullscreen_end",
                None,
                cf_seq,
                &[
                    (
                        "ms",
                        serde_json::Value::from(cf_t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("was_open", serde_json::Value::from(cf_was_open)),
                    ("fs_cache", serde_json::Value::from(cf_fs_cache)),
                    ("ai_cache", serde_json::Value::from(cf_ai_cache)),
                ],
            );
        }
    }

    // -------------------------------------------------------------------
    // AI アップスケール
    // -------------------------------------------------------------------

    /// AI アップスケール時の有効な背景モード (0=黒 / 1=白)。
    /// composite-first 方式では市松 (mode 2) は使えないので 0 に丸める。
    /// アップスケール無効時 (デノイズのみ or 完全 OFF) は出力が背景非依存になるので、
    /// キャッシュキーを単一 (0) に固定する。
    pub(crate) fn effective_upscale_bg_mode(&self) -> u8 {
        if !self.ai_upscale_enabled {
            0
        } else if self.fs_transparent_bg_mode == 1 {
            1
        } else {
            0
        }
    }

    /// 指定 idx の AI アップスケールキャッシュ・pending・failed を全 bg バリアント分まとめて削除する。
    /// pending はキャンセルトークンも立てる。
    pub(crate) fn purge_upscale_for_idx(&mut self, idx: usize) {
        for bg in [0u8, 1u8] {
            self.ai_upscale_cache.remove(&(idx, bg));
            self.ai_upscale_failed.remove(&(idx, bg));
            if let Some((cancel, _)) = self.ai_upscale_pending.remove(&(idx, bg)) {
                cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    /// AI バックエンド設定変更をホットリロードする (Phase 3、再起動不要)。
    ///
    /// 引数 `new_backend_str`: 新しい設定値 (`"directml"` / `"tensorrt"` / `"cpu"` / None)。
    ///
    /// 動作:
    /// - TensorRT に変わった: TrtWorkerPool を spawn して runtime に attach
    /// - TensorRT から DirectML に戻った: 既存の worker pool を detach (子プロセスは
    ///   Drop で shutdown される)
    /// - 同じ → 何もしない
    pub(crate) fn apply_ai_backend_change(&mut self, new_backend_str: Option<&str>) {
        let Some(runtime) = self.ai_runtime.as_ref() else {
            crate::logger::log("[AI] runtime 未初期化のためバックエンド変更はスキップ".to_string());
            return;
        };
        let want_trt = new_backend_str
            .and_then(crate::ai::AiBackend::from_str)
            == Some(crate::ai::AiBackend::TensorRt);
        let has_trt = runtime.has_worker_pool();

        match (want_trt, has_trt) {
            (true, false) => {
                crate::logger::log(
                    "[AI] AI バックエンド変更: → TensorRT (worker pool 起動)".to_string(),
                );
                Self::spawn_trt_worker_pool(runtime);
            }
            (false, true) => {
                crate::logger::log(
                    "[AI] AI バックエンド変更: → DirectML (worker pool 停止)".to_string(),
                );
                runtime.detach_worker_pool();
            }
            _ => {
                // 変更なし (TRT pack 未インストール等で TRT を選んでも spawn 失敗、
                // または同じ設定値) — ログのみ
                crate::logger::log(format!(
                    "[AI] AI バックエンド変更: state 変化なし (want_trt={want_trt}, has_trt={has_trt})"
                ));
            }
        }
    }

    /// AI ランタイムとモデルマネージャを遅延初期化する。
    ///
    /// Phase 3 アーキテクチャ:
    /// - メイン: 常に DirectML で AiRuntime を作る (`ai_backend` 設定値に依らない)
    /// - 設定が TensorRt: 別プロセス TrtWorkerPool を起動して runtime に attach
    ///   (失敗時は DirectML のみで動作、UI で通知)
    pub(crate) fn ensure_ai_runtime(&mut self) {
        if self.ai_runtime.is_none() {
            // 常に DirectML で起動 (Phase 3)
            match crate::ai::runtime::AiRuntime::new_with_backend(
                crate::ai::AiBackend::DirectMl,
            ) {
                Ok(rt) => {
                    let active = rt.active_backend();
                    crate::logger::log(format!(
                        "[AI] Runtime initialized (DirectML always in main, requested={:?}, effective={:?})",
                        active.requested, active.effective
                    ));
                    let runtime_arc = std::sync::Arc::new(rt);

                    // 設定が TRT のとき、子プロセスでワーカープールを起動して attach
                    let want_trt = self
                        .settings
                        .ai_backend
                        .as_deref()
                        .and_then(crate::ai::AiBackend::from_str)
                        == Some(crate::ai::AiBackend::TensorRt);
                    if want_trt {
                        Self::spawn_trt_worker_pool(&runtime_arc);
                    }

                    self.ai_runtime = Some(runtime_arc);
                }
                Err(e) => {
                    crate::logger::log(format!("[AI] Runtime init failed: {e}"));
                }
            }
        }
    }

    /// TRT ワーカープールをバックグラウンドで起動して runtime に attach する。
    ///
    /// 起動には ORT init + DLL preload + ハンドシェイクで数秒かかるので、
    /// メインスレッドで同期実行すると UI が固まる。`std::thread::spawn` で
    /// 別スレッドに逃がし、起動完了したら attach する。
    ///
    /// 起動失敗時は AiRuntime は DirectML のままで動作続行し、ログにエラーを残す。
    ///
    /// **多重 spawn ガード**: `App.trt_restart_in_flight` を CAS で立てる。
    /// 既に立っていたら早期 return (= 別の spawn が進行中)。worker 死亡時に
    /// 並行する複数の AI 推論が同じ DiedDuringInfer を観測しても 1 回しか
    /// 新 pool を起こさない (Codex P2 指摘)。
    pub(crate) fn spawn_trt_worker_pool(
        runtime: &std::sync::Arc<crate::ai::runtime::AiRuntime>,
    ) {
        Self::spawn_trt_worker_pool_inner(runtime, /* guard = */ None);
    }

    /// `spawn_trt_worker_pool` の guard 付き版。worker 死亡時の自動再起動から
    /// 呼ぶ際に使う (= 複数の死亡通知が並行して走る場合の多重 spawn を防ぐ)。
    pub(crate) fn spawn_trt_worker_pool_guarded(
        runtime: &std::sync::Arc<crate::ai::runtime::AiRuntime>,
        guard: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        Self::spawn_trt_worker_pool_inner(runtime, Some(guard));
    }

    fn spawn_trt_worker_pool_inner(
        runtime: &std::sync::Arc<crate::ai::runtime::AiRuntime>,
        guard: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) {
        // CAS: false → true。すでに in-flight なら true のままで、ここは false 戻り → skip。
        if let Some(g) = guard.as_ref() {
            if g.compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_err()
            {
                crate::logger::log(
                    "[AI] TRT worker pool 起動はすでに進行中、新規 spawn を skip".to_string(),
                );
                return;
            }
        }
        let runtime_for_thread = runtime.clone();
        let guard_for_thread = guard;
        std::thread::Builder::new()
            .name("trt-worker-spawn".to_string())
            .spawn(move || {
                crate::logger::log(
                    "[AI] TRT worker pool バックグラウンド起動を試行中...".to_string(),
                );
                match crate::ai::trt_worker_pool::TrtWorkerPool::start() {
                    Ok(pool) => {
                        runtime_for_thread.attach_worker_pool(std::sync::Arc::new(pool));
                        crate::logger::log(
                            "[AI] TRT worker pool 起動成功、attach 完了".to_string(),
                        );
                    }
                    Err(e) => {
                        // 起動失敗 (TRT pack 不在 / engine 不整合 / DLL ロード失敗 等)。
                        // DirectML で動作続行するが、UI に通知して気付かせる
                        // (Phase 3 Step 5)。ログだけだとユーザーが「なぜ TRT に
                        // ならないか」を追えない。
                        runtime_for_thread.report_worker_spawn_failed(e);
                    }
                }
                // spawn 試行が完了 (success / failure 両方) → guard を解放。
                if let Some(g) = guard_for_thread {
                    g.store(false, std::sync::atomic::Ordering::SeqCst);
                }
            })
            .ok();
    }

    /// AI アップスケールの完了をポーリングし、テクスチャに変換してキャッシュする。
    pub(crate) fn poll_ai_upscale(&mut self, ctx: &egui::Context) {
        if !self.ai_upscale_enabled && self.ai_denoise_model.is_none() {
            return;
        }

        let mut completed: Vec<((usize, u8), crate::ai::upscale::UpscaleResult)> = Vec::new();
        let mut disconnected: Vec<(usize, u8)> = Vec::new();

        for (&key, (_, rx)) in &self.ai_upscale_pending {
            match rx.try_recv() {
                Ok(result) => completed.push((key, result)),
                Err(mpsc::TryRecvError::Disconnected) => disconnected.push(key),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        for key in disconnected {
            // cancel フラグが立っているなら「ユーザー操作によるキャンセル」であり、
            // 真の失敗ではない。failed に登録すると次回以降の upscale が永久に
            // 起動しなくなる (start_ai_upscale_pending が failed を見て早期 return)。
            //
            // 連続モデル切替で worker mutex キューが詰まっているとき、新しい
            // 要求が前の要求を cancel → 前の thread がキューから取り出されて
            // 1 タイル走った後に cancel 検知 → 結果送らず disconnect、というパターンで
            // ここに来ると idx が誤って永久 failed になる (Apr 28 のユーザー報告)。
            let was_cancelled = self
                .ai_upscale_pending
                .get(&key)
                .map(|(cancel, _)| cancel.load(Ordering::Relaxed))
                .unwrap_or(false);
            self.ai_upscale_pending.remove(&key);
            if !was_cancelled {
                // 真の失敗 (panic / ORT エラー等) のみ failed に登録してリトライ防止
                self.ai_upscale_failed.insert(key);
            }
        }

        let repaint = !completed.is_empty();
        for (key, result) in completed {
            let (idx, bg) = key;
            self.ai_upscale_pending.remove(&key);
            let pixels = std::sync::Arc::new(result.image);
            let upload = clamp_for_gpu(&pixels);
            let [w, h] = pixels.size;
            let upload_t0 = std::time::Instant::now();
            let handle = ctx.load_texture(
                format!("ai_fs_{idx}_{bg}"),
                upload.into_owned(),
                egui::TextureOptions::LINEAR,
            );
            let upload_ms = upload_t0.elapsed().as_secs_f64() * 1000.0;
            if crate::perf::is_enabled() {
                let perf_key = self.perf_item_key(idx);
                crate::perf::event(
                    "ai",
                    "job_ready",
                    perf_key.as_deref(),
                    self.input_seq,
                    &[
                        ("idx", serde_json::Value::from(idx)),
                        ("bg", serde_json::Value::from(bg)),
                        ("w", serde_json::Value::from(w)),
                        ("h", serde_json::Value::from(h)),
                        ("upload_ms", serde_json::Value::from(upload_ms)),
                    ],
                );
            }
            // AI 完了時、fs_cache ベースで先に作られた仮 adjustment_cache を無効化する
            // (そのまま残ると次回来訪時に低解像度の補正結果が使われてしまう)。
            // 表示中かつ現在の bg と一致するもののみ、AI 結果に対して色調補正を即座に適用（チラつき防止）
            self.adjustment_cache.remove(&idx);
            if self.fullscreen_idx == Some(idx) && self.effective_upscale_bg_mode() == bg {
                self.apply_sync_adjustment(ctx, idx, &pixels);
            }
            // 派生キャッシュ。fs.paint は fs_cache 側から load_seq を拾うためここでは 0。
            // source_dims はダウンスケール警告用で、派生エントリは元画像の fs_cache 側を
            // 参照すればよいのでここでは None を入れておく。
            self.ai_upscale_cache.insert(
                key,
                FsCacheEntry::Static {
                    tex: handle,
                    pixels,
                    source_dims: None,
                    load_seq: 0,
                },
            );
            crate::logger::log(format!("[AI] Upscale complete for idx={idx} bg={bg}"));
        }

        if repaint {
            ctx.request_repaint();
        }
    }

    /// 現在のフルスクリーン画像に対して AI 処理（デノイズ / アップスケール）を開始する。
    /// - 先読みが全完了している場合のみ開始
    /// - すでに処理済み or pending の場合はスキップ
    /// - アップスケール時: 2K 以上の画像はスキップ
    pub(crate) fn maybe_start_ai_upscale(&mut self, current_idx: usize) {
        let denoise_enabled = self.ai_denoise_model.is_some();
        let upscale_enabled = self.ai_upscale_enabled;

        if !denoise_enabled && !upscale_enabled {
            return;
        }

        // composite-first 方式: 現在の bg と一致するエントリのみが対象。
        // 他 bg バリアントは別キャッシュに残るので独立にスケジュールする。
        let bg = self.effective_upscale_bg_mode();
        let cur_key = (current_idx, bg);

        // すでに処理済み、処理中、または失敗済み
        if self.ai_upscale_cache.contains_key(&cur_key)
            || self.ai_upscale_pending.contains_key(&cur_key)
            || self.ai_upscale_failed.contains(&cur_key)
        {
            return;
        }

        // 同時実行は 1 枚まで（GPU メモリと帯域の制約）。ただし現在表示中の
        // 画像を処理するケースでは、ユーザーが既に先に進んでいるので古い
        // 先読み（別 idx or 別 bg）を優先キャンセルして枠を空ける。
        if !self.ai_upscale_pending.is_empty() {
            if self.fullscreen_idx == Some(current_idx) {
                let to_cancel: Vec<(usize, u8)> = self
                    .ai_upscale_pending
                    .keys()
                    .filter(|&&k| k != cur_key)
                    .copied()
                    .collect();
                for k in to_cancel {
                    // **cancel フラグだけ立てて pending entry は残す**。worker は次の
                    // tile boundary で cancel を見て Err(Cancelled) を返す (= channel
                    // disconnect → poll_ai_upscale で silent remove)。
                    // ただし最後の tile が完了直前ならそのまま Ok で返ることもあり、
                    // その場合 tx.send で結果が rx に届く → cache に保存される。
                    // (Apr 29: 高速スクロール時に最後の tile 完了直後で entry を remove
                    // していたため Ok 結果が捨てられて、戻り再訪時にアップスケールが
                    // 効かない問題があった)。
                    if let Some((cancel, _)) = self.ai_upscale_pending.get(&k) {
                        if !cancel.load(Ordering::Relaxed) {
                            cancel.store(true, Ordering::Relaxed);
                            crate::logger::log(format!(
                                "[AI] Cancelled prefetch {:?} to prioritize current {:?}",
                                k, cur_key
                            ));
                        }
                    }
                }
            }
            // 「pending に何か残っている → 新規 spawn しない」のガード。
            // ただしキャンセル済み entry は既に終わりに向かっているので新規 spawn の
            // 妨げにしない (= 1〜数 ms のオーバーラップを許可)。これにより上の
            // 「Cancelled prefetch」で entry を残したままでも新しい current_idx の
            // upscale を即時開始できる。
            let has_active_pending = self
                .ai_upscale_pending
                .values()
                .any(|(cancel, _)| !cancel.load(Ordering::Relaxed));
            if has_active_pending {
                return;
            }
        }

        // 元画像がキャッシュにあるか確認
        let source_image = match self.fs_cache.get(&current_idx) {
            Some(FsCacheEntry::Static { pixels, .. }) => pixels.clone(),
            _ => return,
        };

        let (w, h) = (source_image.size[0] as u32, source_image.size[1] as u32);
        let upscale_in_range =
            crate::ai::upscale::should_process(w, h, self.settings.ai_upscale_skip_px);
        let denoise_in_range =
            crate::ai::upscale::should_process(w, h, self.settings.ai_denoise_skip_px);
        // 両方の範囲外ならスキップ
        if (!upscale_enabled || !upscale_in_range) && (!denoise_enabled || !denoise_in_range) {
            return;
        }

        // AI ランタイム / モデルマネージャを遅延初期化
        self.ensure_ai_runtime();

        let Some(runtime) = self.ai_runtime.clone() else {
            return;
        };
        let manager = self.ai_model_manager.clone();

        // デノイズモデル選択・ロード
        let denoise_model = if denoise_enabled && denoise_in_range {
            let kind = match self.ai_denoise_model {
                Some(k) => k,
                None => return, // デノイズ有効だがモデル未設定
            };
            let Some(model_path) = manager.model_path(kind) else {
                // モデル未ダウンロード → スキップ（起動時ダイアログでダウンロード）
                return;
            };
            if !runtime.is_loaded(kind) {
                if let Err(e) = runtime.load_model(kind, &model_path) {
                    crate::logger::log(format!("[AI] Denoise model load failed: {e}"));
                    return;
                }
            }
            Some(kind)
        } else {
            None
        };

        // アップスケールモデル選択・ロード
        let upscale_model = if upscale_enabled && upscale_in_range {
            let kind = match self.ai_upscale_model_override {
                Some(k) => k,
                None => {
                    let category = self
                        .ai_classify_cache
                        .get(&current_idx)
                        .copied()
                        .unwrap_or_else(|| {
                            let dynimg = color_image_to_dynamic(&source_image);
                            let cat = crate::ai::classify::classify_heuristic(&dynimg);
                            self.ai_classify_cache.insert(current_idx, cat);
                            cat
                        });
                    category.preferred_upscale_model()
                }
            };
            match manager.model_path(kind) {
                Some(model_path) => {
                    if !runtime.is_loaded(kind) {
                        if let Err(e) = runtime.load_model(kind, &model_path) {
                            crate::logger::log(format!("[AI] Upscale model load failed: {e}"));
                            if denoise_model.is_none() {
                                return;
                            }
                            None
                        } else {
                            Some(kind)
                        }
                    } else {
                        Some(kind)
                    }
                }
                None => {
                    crate::logger::log(format!(
                        "[AI] Upscale model {:?} not available, skipping for idx={current_idx}",
                        kind
                    ));
                    if denoise_model.is_none() {
                        return;
                    }
                    None
                }
            }
        } else {
            None
        };

        if denoise_model.is_none() && upscale_model.is_none() {
            return;
        }

        // バックグラウンドスレッドで AI 処理実行
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let cancel_clone = cancel.clone();
        let idx = current_idx;
        // composite-first 用の背景色 (bg=1 のみ白、それ以外は黒)
        let bg_rgb: [u8; 3] = if bg == 1 { [255, 255, 255] } else { [0, 0, 0] };
        // composite-first はアップスケールパスのみ必要 (1x のデノイズは Lanczos 拡大の
        // ズレが生じないため、アルファ保持パスでそのまま処理できる)。
        let composite_first = upscale_model.is_some();

        std::thread::spawn(move || {
            // composite-first: アップスケールあり時のみ bg 単色に合成してから AI に渡す。
            // これにより AI モデルがアルファ境界の RGB ガベージに引きずられず、
            // 輪郭がきれいにアップスケールされる。背景色は出力に焼き付くため、
            // bg 切替時は別キャッシュエントリとして再生成する。
            // デノイズのみの場合はアルファ保持パスを通って透明度がそのまま残る。
            let mut dynimg = if composite_first {
                color_image_to_dynamic_composited(&source_image, bg_rgb)
            } else {
                color_image_to_dynamic(&source_image)
            };

            // Step 1: デノイズ (1x)
            if let Some(denoise_kind) = denoise_model {
                match crate::ai::denoise::denoise(&runtime, denoise_kind, &dynimg, &cancel_clone) {
                    Ok(denoised) => {
                        if upscale_model.is_some() {
                            // アップスケールが後続する場合のみ DynamicImage に変換
                            // (denoised は補正済み RGB なのでアルファ合成は不要)
                            dynimg = color_image_to_dynamic(&denoised);
                        } else {
                            // デノイズのみ: 結果をそのまま送信（変換を省略）
                            let _ = tx.send(crate::ai::upscale::UpscaleResult {
                                idx,
                                image: denoised,
                            });
                            return;
                        }
                    }
                    Err(e) => {
                        crate::logger::log(format!("[AI] Denoise failed for idx={idx}: {e}"));
                        if upscale_model.is_none() {
                            return;
                        }
                    }
                }
            }

            // Step 2: アップスケール (4x)
            if let Some(upscale_kind) = upscale_model {
                match crate::ai::upscale::upscale(&runtime, upscale_kind, &dynimg, &cancel_clone) {
                    Ok(upscaled) => {
                        let _ = tx.send(crate::ai::upscale::UpscaleResult {
                            idx,
                            image: upscaled,
                        });
                    }
                    Err(e) => {
                        crate::logger::log(format!("[AI] Upscale failed for idx={idx}: {e}"));
                    }
                }
            }
        });

        self.ai_upscale_pending.insert(cur_key, (cancel, rx));
        crate::logger::log(format!(
            "[AI] AI processing started for idx={current_idx} bg={bg} denoise={:?} upscale={:?}",
            denoise_model, upscale_model
        ));
        if crate::perf::is_enabled() {
            let perf_key = self.perf_item_key(current_idx);
            crate::perf::event(
                "ai",
                "job_start",
                perf_key.as_deref(),
                self.input_seq,
                &[
                    ("idx", serde_json::Value::from(current_idx)),
                    ("bg", serde_json::Value::from(bg)),
                    (
                        "denoise",
                        serde_json::Value::from(format!("{:?}", denoise_model)),
                    ),
                    (
                        "upscale",
                        serde_json::Value::from(format!("{:?}", upscale_model)),
                    ),
                ],
            );
        }
    }

    /// 先読み範囲内の item_idx 集合を計算する。
    fn compute_keep_set(&self, current_idx: usize) -> std::collections::HashSet<usize> {
        let image_indices = Self::collect_image_indices(&self.items);
        let Some(pos) = image_indices.iter().position(|&i| i == current_idx) else {
            return std::collections::HashSet::new();
        };
        let n = image_indices.len();
        let keep_back = self.settings.prefetch_back + 1;
        let keep_forward = self.settings.prefetch_forward + 1;
        (pos.saturating_sub(keep_back)..=((pos + keep_forward).min(n - 1)))
            .map(|p| image_indices[p])
            .collect()
    }

    /// AI アップスケールキャッシュの eviction（先読み範囲外を破棄）。
    fn evict_ai_upscale_cache(&mut self, current_idx: usize) {
        let keep_set = self.compute_keep_set(current_idx);
        // 全 bg バリアントとも、idx が範囲外なら破棄
        self.ai_upscale_cache.retain(|k, _| keep_set.contains(&k.0));

        // 範囲外の pending をキャンセル
        let to_cancel: Vec<(usize, u8)> = self
            .ai_upscale_pending
            .keys()
            .filter(|k| !keep_set.contains(&k.0))
            .cloned()
            .collect();
        for k in to_cancel {
            // **cancel フラグだけ立てて pending entry は残す** (= worker が完了寸前
            // なら Ok の結果が rx 経由で届いて cache に保存される)。entry 自体は
            // poll_ai_upscale が channel disconnect を見たときに削除する。
            if let Some((cancel, _)) = self.ai_upscale_pending.get(&k) {
                cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    /// AI 先読み対象の item_idx を前方優先（+1..+pf_forward, -1..-pf_back）で返す。
    pub(crate) fn ai_prefetch_targets(&self, current_idx: usize) -> Vec<usize> {
        let image_indices = Self::collect_image_indices(&self.items);
        let Some(pos) = image_indices.iter().position(|&i| i == current_idx) else {
            return Vec::new();
        };
        let n = image_indices.len();
        let pf_back = self.settings.ai_upscale_prefetch_back;
        let pf_forward = self.settings.ai_upscale_prefetch_forward;
        interleaved_prefetch_targets(&image_indices, pos, n, pf_forward, pf_back)
    }

    /// AI アップスケールの先読み（表示中画像の前後）。
    fn prefetch_ai_upscale(&mut self, current_idx: usize) {
        if !self.ai_upscale_enabled && self.ai_denoise_model.is_none() {
            return;
        }
        for idx in self.ai_prefetch_targets(current_idx) {
            self.maybe_start_ai_upscale(idx);
        }
    }

    // ── 画像補正 ──────────────────────────────────────────────────

    /// ページの正規化キーを返す（DB 保存用）。
    pub(crate) fn page_path_key(&self, idx: usize) -> Option<String> {
        let item = self.items.get(idx)?;
        let key = match item {
            GridItem::Image(p) => crate::adjustment_db::normalize_path(p),
            GridItem::ZipImage {
                zip_path,
                entry_name,
            } => crate::adjustment_db::zip_entry_key(zip_path, entry_name),
            GridItem::PdfPage {
                pdf_path, page_num, ..
            } => crate::adjustment_db::zip_entry_key(pdf_path, &format!("page_{page_num}")),
            _ => return None,
        };
        Some(key)
    }

    /// ページのサイドカー置き場 (= 対応する `mimageviewer.dat` が置かれるフォルダ) を返す。
    /// `page_path_key` と対になるヘルパー。3 バリアント対応漏れを避けるため同じ構造で書いている。
    pub(crate) fn sidecar_folder(&self, idx: usize) -> Option<std::path::PathBuf> {
        let item = self.items.get(idx)?;
        match item {
            GridItem::Image(p) => p.parent().map(|d| d.to_path_buf()),
            GridItem::ZipImage { zip_path, .. } => zip_path.parent().map(|d| d.to_path_buf()),
            GridItem::PdfPage { pdf_path, .. } => pdf_path.parent().map(|d| d.to_path_buf()),
            _ => None,
        }
    }

    /// サイドカー内のフォルダ相対キーを返す。小文字化され、`page_path_key` で使われる
    /// セパレータと整合する形式になる。
    pub(crate) fn sidecar_relative_key(&self, idx: usize) -> Option<String> {
        let item = self.items.get(idx)?;
        match item {
            GridItem::Image(p) => {
                let name = p.file_name()?.to_string_lossy().to_lowercase();
                Some(name)
            }
            GridItem::ZipImage {
                zip_path,
                entry_name,
            } => {
                let name = zip_path.file_name()?.to_string_lossy().to_lowercase();
                Some(format!("{name}::{}", entry_name.to_lowercase()))
            }
            GridItem::PdfPage {
                pdf_path, page_num, ..
            } => {
                let name = pdf_path.file_name()?.to_string_lossy().to_lowercase();
                Some(format!("{name}::page_{page_num}"))
            }
            _ => None,
        }
    }

    /// (sidecar_folder, sidecar_relative_key) のペアを取得する。
    fn sidecar_coords(&self, idx: usize) -> Option<(std::path::PathBuf, String)> {
        Some((self.sidecar_folder(idx)?, self.sidecar_relative_key(idx)?))
    }

    /// 指定フォルダのサイドカーへ可変参照を取得する。メモリ上に未ロードならロードする。
    /// `sidecar_backup_enabled` が OFF なら None を返す (呼び出し側は no-op になる)。
    fn sidecar_mut(
        &mut self,
        folder: &std::path::Path,
    ) -> Option<&mut crate::sidecar::SidecarFile> {
        if !self.settings.sidecar_backup_enabled {
            return None;
        }
        if !self.sidecars.contains_key(folder) {
            let loaded = crate::sidecar::SidecarFile::load(folder);
            self.sidecars.insert(folder.to_path_buf(), loaded);
        }
        self.sidecars.get_mut(folder)
    }

    /// 指定 `idx` のページに対応するサイドカーに対し `op` を実行する。
    /// 設定 OFF・idx が画像系でない・フォルダ解決不能のいずれかなら黙って no-op。
    /// 書き込みミラーの 1 行化に使う。
    fn with_sidecar_mut<F>(&mut self, idx: usize, op: F)
    where
        F: FnOnce(&mut crate::sidecar::SidecarFile, &str),
    {
        if let Some((folder, rel)) = self.sidecar_coords(idx) {
            if let Some(sc) = self.sidecar_mut(&folder) {
                op(sc, &rel);
            }
        }
    }

    /// すべての dirty なサイドカーをディスクにフラッシュする。
    /// 呼び出し側: フォルダ切替時・アプリ終了時・5 秒アイドル時。
    pub(crate) fn flush_all_sidecars(&mut self) {
        for sidecar in self.sidecars.values_mut() {
            sidecar.flush();
        }
    }

    /// アプリ終了時・トレイ退避時の共通永続化処理:
    /// ウィンドウ位置・サイズを settings に書き戻し、save + サイドカーを flush する。
    ///
    /// - サイズは `ViewportBuilder::with_inner_size` と整合する inner_size のみを書く
    ///   (不明なら前回保存値を維持。outer に fallback すると titlebar 分だけ縮小する
    ///    問題が再発するので fallback しない)。
    /// - 位置は outer_rect から取る (`with_position` は outer 座標を受け取る)。
    pub(crate) fn persist_window_state_and_flush(&mut self) {
        if let Some(rect) = self.last_outer_rect {
            self.settings.window_pos = Some([rect.min.x, rect.min.y]);
        }
        if let Some(size) = self.last_inner_size {
            self.settings.window_size = Some(size);
        }
        self.settings.save();
        self.flush_all_sidecars();
    }

    /// マスクとベクタを DB に保存し、サイドカーにもミラーする。消しゴムモード終了時に呼ぶ。
    /// ビットマップが全 false かつベクタが空なら DB からもサイドカーからも削除する
    /// (mask_db.set の仕様と揃える)。サムネイルバッジ用の `mask_pages` もここで更新する。
    pub(crate) fn save_mask_with_sidecar(
        &mut self,
        idx: usize,
        mask: &[bool],
        vectors: &[crate::mask_db::LineObject],
        w: usize,
        h: usize,
    ) {
        let bitmap_empty = !mask.iter().any(|&m| m);
        if bitmap_empty && vectors.is_empty() {
            // 空マスクは DB + サイドカーから削除
            self.delete_mask_with_sidecar(idx);
            return;
        }
        // 圧縮とベクタ JSON 化を 1 回だけ行い、DB 書き込みとサイドカー両方で共有。
        // 一括適用 (apply_slot_to_selection) で N ページに同じマスクを配るときの
        // N 倍 deflate を回避する。
        let compressed = crate::mask_db::compress_mask(mask);
        let vectors_json = crate::mask_db::vectors_to_json(vectors);
        self.save_mask_raw_with_sidecar(idx, &compressed, vectors, vectors_json.as_deref(), w, h);
    }

    /// 既に deflate 済みのビットマップ + JSON 済みベクタを DB + サイドカーに書き込む。
    /// `save_mask_with_sidecar` と一括適用パスの共通バックエンド。
    fn save_mask_raw_with_sidecar(
        &mut self,
        idx: usize,
        compressed: &[u8],
        vectors: &[crate::mask_db::LineObject],
        vectors_json: Option<&str>,
        w: usize,
        h: usize,
    ) {
        let key = match self.page_path_key(idx) {
            Some(k) => k,
            None => return,
        };
        if let Some(db) = &self.mask_db {
            let _ = db.set_raw(&key, compressed, vectors_json, w, h);
        }
        self.mask_pages.insert(idx);
        let sidecar_mask =
            crate::sidecar::SidecarMask::from_raw(compressed, vectors, w as u32, h as u32);
        self.with_sidecar_mut(idx, move |sc, rel| sc.set_mask(rel, sidecar_mask));
    }

    /// マスクを DB から削除し、サイドカーからも削除する。「マスク全削除」ボタン用。
    pub(crate) fn delete_mask_with_sidecar(&mut self, idx: usize) {
        let key = match self.page_path_key(idx) {
            Some(k) => k,
            None => return,
        };
        if let Some(db) = &self.mask_db {
            let _ = db.delete(&key);
        }
        self.mask_pages.remove(&idx);
        self.with_sidecar_mut(idx, |sc, rel| sc.remove_mask(rel));
    }

    /// サイドカーからまだ DB に無いエントリを取り込み、両 DB を更新する。
    /// フォルダ丸ごと移動で中央 DB のパスキーが無効化された場合の復旧経路。
    /// サイドカー自体はメモリに残し、以降の書き込みミラーに使う。
    /// 実際のインポートロジックは [`crate::sidecar::import_to_dbs`] に委譲。
    ///
    /// ## Fast-path (2026-04): サイドカー mtime ガード
    ///
    /// 通常 DB 側が authoritative で、サイドカー側が新しいのはフォルダ移動・
    /// リネーム直後のレアケース。全フォルダ切替で `read_to_string` + parse +
    /// import を走らせると HDD 競合で 100-500ms のヒッチになる (perf-log で
    /// `sli_sidecar_import max=456ms` を観測)。そこで:
    ///
    /// 1. `fs::metadata(sidecar)` で mtime を取得 (1 syscall、通常 <1ms)
    /// 2. `adjustment_db.sidecar_sync` に記録した前回 import 時の mtime と比較
    /// 3. 一致するなら読まずに return (common case)
    /// 4. 不一致 / 未登録 / ファイル削除 のときだけ既存の slow-path に入る
    fn import_sidecar_to_dbs(&mut self, sidecar_folder: &std::path::Path) {
        let sidecar_path = sidecar_folder.join(crate::sidecar::SIDECAR_FILENAME);
        let folder_key = crate::adjustment_db::normalize_path(sidecar_folder);

        // Step 1: サイドカーの fs 状態を確認
        let fs_mtime: Option<i64> = match std::fs::metadata(&sidecar_path) {
            Ok(m) => m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64),
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => {
                // サイドカー無し: 以前に import 済みの記録があれば消して整合させる
                // (サイドカーが外部削除されたフォルダに追従)
                if let Some(db) = &self.adjustment_db {
                    let _ = db.sidecar_sync_clear(&folder_key);
                }
                return;
            }
            Err(_) => return,
        };

        // Step 2: DB 側に記録された mtime と比較
        if let (Some(db), Some(fs_mt)) = (&self.adjustment_db, fs_mtime) {
            if db.sidecar_sync_get(&folder_key) == Some(fs_mt) {
                // 前回 import と同じ mtime → 読む必要なし (common case)
                return;
            }
        }

        // Step 3: Slow-path (初回 / 外部変更された場合のみ)
        //
        // **Codex P1 修正**: mtime が一致しないということは、ディスク上のサイドカーが
        // 外部変更されている可能性がある。この場合、`self.sidecars` 内のメモリキャッシュは
        // stale なので、使い続けてはいけない (古い内容を DB に書き戻し、新 mtime を記録すると
        // 以降 fast-path で永久にスキップされて外部更新が反映されないというデータ取込漏れ
        // バグになる)。
        //
        // 例外として、メモリキャッシュが `is_dirty()` (= アプリ内で未保存の編集がある)
        // 場合は再ロードで edits を破壊してしまうので、slow-path を抜けて何もせず帰る。
        // ここで `sidecar_sync_upsert` もスキップするので、次回ナビゲート時 (通常は
        // `flush_idle_sidecars` で dirty が解消された後) に改めて判定される。
        let cached_dirty = self
            .sidecars
            .get(sidecar_folder)
            .map(|s| s.is_dirty())
            .unwrap_or(false);
        if cached_dirty {
            crate::logger::log(format!(
                "sidecar: slow-path hit but in-memory cache is dirty — skip reload (sync record not updated): {}",
                sidecar_folder.display()
            ));
            return;
        }
        // キャッシュを無効化して強制再ロード (外部変更されたサイドカーの新内容を取り込む)
        self.sidecars.insert(
            sidecar_folder.to_path_buf(),
            crate::sidecar::SidecarFile::load(sidecar_folder),
        );
        let Some(sidecar) = self.sidecars.get(sidecar_folder) else {
            return;
        };
        // 空サイドカーでも mtime は記録して次回以降スキップできるようにする
        if !sidecar.items().is_empty() {
            let stats = crate::sidecar::import_to_dbs(
                sidecar_folder,
                sidecar,
                self.adjustment_db.as_ref(),
                self.mask_db.as_ref(),
            );
            if stats.imported_adjust > 0 || stats.imported_mask > 0 {
                crate::logger::log(format!(
                    "sidecar: imported {} adjust + {} mask entries from {}",
                    stats.imported_adjust,
                    stats.imported_mask,
                    sidecar_folder.display()
                ));
            }
        }
        if let (Some(db), Some(fs_mt)) = (&self.adjustment_db, fs_mtime) {
            let _ = db.sidecar_sync_upsert(&folder_key, fs_mt);
        }
    }

    /// アイドル時 (5 秒間変更がない) に dirty なサイドカーをフラッシュする。
    /// 長時間の編集セッション中にクラッシュや電源断で失う事故への保険。
    pub(crate) fn flush_idle_sidecars(&mut self) {
        let now = std::time::Instant::now();
        const IDLE_THRESHOLD_SECS: u64 = 5;
        for sidecar in self.sidecars.values_mut() {
            if !sidecar.is_dirty() {
                continue;
            }
            if let Some(last) = sidecar.last_change() {
                if now.duration_since(last).as_secs() >= IDLE_THRESHOLD_SECS {
                    sidecar.flush();
                }
            }
        }
    }

    /// 現在の有効パラメータに基づいて AI アップスケール/デノイズの状態を更新する。
    pub(crate) fn sync_upscale_from_preset(&mut self, idx: usize) {
        // effective_params は self を不変借用するので、派生値だけコピーして解放してから代入する
        let upscale_kind = self.effective_params(idx).upscale_model_kind();
        let denoise_kind = self.effective_params(idx).denoise_model_kind();
        match upscale_kind {
            None => {
                self.ai_upscale_enabled = false;
                self.ai_upscale_model_override = None;
            }
            Some(None) => {
                self.ai_upscale_enabled = true;
                self.ai_upscale_model_override = None;
            }
            Some(Some(kind)) => {
                self.ai_upscale_enabled = true;
                self.ai_upscale_model_override = Some(kind);
            }
        }
        self.ai_denoise_model = denoise_kind;
        // composite-first: アップスケール有効時は市松 bg を使えないので黒に丸める。
        // (デノイズのみの場合はアルファ保持パスを通るので市松 OK)
        if self.ai_upscale_enabled && self.fs_transparent_bg_mode == 2 {
            self.fs_transparent_bg_mode = 0;
        }
    }

    /// 右上フィードバック表示を設定する。
    pub(crate) fn show_feedback_toast(&mut self, text: String) {
        self.fs_feedback_toast = Some((text, std::time::Instant::now()));
    }

    /// 指定ページの有効パラメータへの参照を返す。
    ///
    /// 解決順:
    /// 1. `adjustment_page_params[idx]` (ページ個別)
    /// 2. `adjustment_favorite_params[nearest_fav_id]` (お気に入り単位の標準)
    /// 3. `settings.global_preset` (全体の標準)
    ///
    /// 所有権が必要な呼び出し側は `.clone()` する。毎フレーム呼ばれるので無用なクローンを避ける。
    pub(crate) fn effective_params(&self, idx: usize) -> &crate::adjustment::AdjustParams {
        if let Some(p) = self.adjustment_page_params.get(&idx) {
            return p;
        }
        if let Some(p) = self.favorite_default_for_idx(idx) {
            return p;
        }
        &self.settings.global_preset
    }

    /// 指定 idx のコンテキストにおける「ページ個別を除いた有効パラメータ」。
    /// 個別設定の冗長判定 (個別を保存する意味があるか) に使う。
    /// お気に入り配下なら favorite 標準、そうでなければ global。
    pub(crate) fn effective_default_for_idx(
        &self,
        idx: usize,
    ) -> &crate::adjustment::AdjustParams {
        self.favorite_default_for_idx(idx)
            .unwrap_or(&self.settings.global_preset)
    }

    /// 指定 idx が属するお気に入り (最も近い祖先) の標準パラメータへの参照を返す。
    /// ZIP/PDF ページは ZIP/PDF 本体のパスでお気に入り判定される。
    fn favorite_default_for_idx(&self, idx: usize) -> Option<&crate::adjustment::AdjustParams> {
        let fav_id = self.current_favorite_id_for_idx(idx)?;
        self.adjustment_favorite_params.get(&fav_id)
    }

    /// 指定 idx が属するお気に入り (最も近い祖先) の id を返す。
    pub(crate) fn current_favorite_id_for_idx(&self, idx: usize) -> Option<uuid::Uuid> {
        let item = self.items.get(idx)?;
        let container_path: std::path::PathBuf = match item {
            GridItem::Image(p) => p.parent()?.to_path_buf(),
            GridItem::Video(p) => p.parent()?.to_path_buf(),
            GridItem::ZipImage { zip_path, .. } => zip_path.clone(),
            GridItem::PdfPage { pdf_path, .. } => pdf_path.clone(),
            _ => return None,
        };
        self.find_nearest_favorite(&container_path).map(|f| f.id)
    }

    /// U/N/P 補正ショートカットで書き換える対象のスコープを決定する。
    /// 個別設定 → お気に入り標準 → global の順で最初に存在する層を選ぶ。
    pub(crate) fn resolve_adjust_scope(
        &self,
        fs_idx: usize,
    ) -> crate::ui_fullscreen::AdjustScope {
        use crate::ui_fullscreen::AdjustScope;
        if self.adjustment_page_params.contains_key(&fs_idx) {
            return AdjustScope::PageOverride;
        }
        if let Some(fav_id) = self.current_favorite_id_for_idx(fs_idx) {
            if self.adjustment_favorite_params.contains_key(&fav_id) {
                return AdjustScope::FavoriteDefault(fav_id);
            }
        }
        AdjustScope::Global
    }

    /// `resolve_adjust_scope` で決めたスコープに向けて `params` を書き込む。
    pub(crate) fn write_params_for_scope(
        &mut self,
        fs_idx: usize,
        scope: crate::ui_fullscreen::AdjustScope,
        params: crate::adjustment::AdjustParams,
    ) {
        use crate::ui_fullscreen::AdjustScope;
        match scope {
            AdjustScope::PageOverride => self.set_page_params(fs_idx, params),
            AdjustScope::FavoriteDefault(id) => self.set_favorite_default(id, params),
            AdjustScope::Global => self.copy_params_to_global(params),
        }
    }

    /// 指定ページに個別パラメータを書込む (DB にも保存)。
    ///
    /// `params` が「そのページで効く標準」(= お気に入り標準 or global_preset) と
    /// 完全一致するなら個別設定として保存しない (= フォールバックで標準が使われる)。
    /// 旧来は `is_removable()` (= identity かつ AI 未使用) で削除判定していたが、
    /// グローバルが AI ON の状態で個別に「AI OFF」を設定したいケースを取りこぼしたため、
    /// 標準との等価比較に変更した。お気に入り標準もこれで同じ扱いになる。
    pub(crate) fn set_page_params(&mut self, idx: usize, params: crate::adjustment::AdjustParams) {
        let matches_default = params == *self.effective_default_for_idx(idx);
        if matches_default {
            self.adjustment_page_params.remove(&idx);
            if let Some(key) = self.page_path_key(idx) {
                if let Some(db) = &self.adjustment_db {
                    let _ = db.remove_page_params(&key);
                }
            }
            self.with_sidecar_mut(idx, |sc, rel| sc.remove_adjust(rel));
        } else {
            self.adjustment_page_params.insert(idx, params.clone());
            if let Some(key) = self.page_path_key(idx) {
                if let Some(db) = &self.adjustment_db {
                    let _ = db.set_page_params(&key, &params);
                }
            }
            self.with_sidecar_mut(idx, move |sc, rel| sc.set_adjust(rel, params));
        }
        // 補正値が変わった可能性があるので色調キャッシュは常に落とす。
        // Undo/Redo 経路 (apply_adjustment_change_to_app) もここを通って正しく invalidate される。
        // AI キャッシュは ai_settings_eq 差分判定が必要なので呼び出し側に任せる。
        self.adjustment_cache.remove(&idx);
        self.thumb_adjust_tex.remove(&idx);
    }

    /// 指定ページの個別設定を解除する (DB からも削除)。
    ///
    /// 個別 → グローバル へのフォールバックで AI 設定 (upscale / denoise) が
    /// 切り替わる場合は、その idx の AI キャッシュ / 失敗マーカ / pending を
    /// クリアして次フレームで再実行されるようにする。
    /// 色調 (adjustment) キャッシュは常にクリアする。
    pub(crate) fn clear_page_params(&mut self, idx: usize) {
        let old_params = self.effective_params(idx).clone();
        self.adjustment_page_params.remove(&idx);
        let new_params = self.effective_params(idx).clone();
        if let Some(key) = self.page_path_key(idx) {
            if let Some(db) = &self.adjustment_db {
                let _ = db.remove_page_params(&key);
            }
        }
        self.with_sidecar_mut(idx, |sc, rel| sc.remove_adjust(rel));
        self.adjustment_cache.remove(&idx);
        self.thumb_adjust_tex.remove(&idx);
        if !old_params.ai_settings_eq(&new_params) {
            self.purge_upscale_for_idx(idx);
        }
    }

    /// 画像系グリッドアイテム (`Image` / `ZipImage` / `PdfPage`) の (idx, DB キー) 一覧を集める。
    fn collect_image_page_keys(&self) -> (Vec<usize>, Vec<String>) {
        let mut indices: Vec<usize> = Vec::new();
        let mut keys: Vec<String> = Vec::new();
        for idx in 0..self.items.len() {
            match self.items.get(idx) {
                Some(GridItem::Image(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::PdfPage { .. }) => {
                    if let Some(key) = self.page_path_key(idx) {
                        indices.push(idx);
                        keys.push(key);
                    }
                }
                _ => {}
            }
        }
        (indices, keys)
    }

    /// 画像系グリッドアイテムをサイドカーフォルダでグループ化した (folder, Vec<rel_key>) を返す。
    /// 一括書き込み系 (apply/clear all) で folder 単位に sidecar を更新するために使う。
    fn collect_image_sidecar_coords(
        &self,
    ) -> std::collections::HashMap<std::path::PathBuf, Vec<String>> {
        let mut map: std::collections::HashMap<std::path::PathBuf, Vec<String>> =
            std::collections::HashMap::new();
        for idx in 0..self.items.len() {
            match self.items.get(idx) {
                Some(GridItem::Image(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::PdfPage { .. }) => {
                    if let (Some(folder), Some(rel)) =
                        (self.sidecar_folder(idx), self.sidecar_relative_key(idx))
                    {
                        map.entry(folder).or_default().push(rel);
                    }
                }
                _ => {}
            }
        }
        map
    }

    /// 現在の一覧 (フォルダ/ZIP/PDF) の全画像ページに同じパラメータを適用する。
    /// `params` がこの一覧の「ページ個別を除いた有効標準」(お気に入り標準 or global_preset) と
    /// 等価なら個別設定は削除され、全画像がその標準に戻る。
    /// 書換の前後で AI 設定 (upscale/denoise) が変わったページは ai_upscale_cache /
    /// failed / pending もクリアして、次フレームで再実行されるようにする。
    pub(crate) fn apply_params_to_all_pages(&mut self, params: crate::adjustment::AdjustParams) {
        let (indices, keys) = self.collect_image_page_keys();
        let sidecar_coords = self.collect_image_sidecar_coords();
        // 書換後の effective params は全画像ページで `params` になる。
        // 書換前に AI 設定が異なるページを拾っておく。
        let ai_changed_indices: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|idx| !self.effective_params(*idx).ai_settings_eq(&params))
            .collect();
        // 一覧内の画像は同じコンテナに属する = 同じ「お気に入り or global」標準を共有する。
        // 先頭 idx の標準で代表させて matches_default を判定する。
        let matches_default = indices
            .first()
            .map(|&idx| params == *self.effective_default_for_idx(idx))
            .unwrap_or_else(|| params == self.settings.global_preset);
        if matches_default {
            for idx in &indices {
                self.adjustment_page_params.remove(idx);
            }
            if let Some(db) = self.adjustment_db.as_mut() {
                let _ = db.remove_page_params_bulk(&keys);
            }
            for (folder, rels) in sidecar_coords {
                if let Some(sc) = self.sidecar_mut(&folder) {
                    sc.remove_adjust_bulk(rels);
                }
            }
        } else {
            for idx in &indices {
                self.adjustment_page_params.insert(*idx, params.clone());
            }
            if let Some(db) = self.adjustment_db.as_mut() {
                let _ = db.set_page_params_bulk(&keys, &params);
            }
            for (folder, rels) in sidecar_coords {
                if let Some(sc) = self.sidecar_mut(&folder) {
                    sc.set_adjust_bulk(rels, &params);
                }
            }
        }
        self.clear_all_color_caches();
        self.clear_ai_caches_for_indices(&ai_changed_indices);
    }

    /// 現在の一覧の全画像ページから個別設定を削除する (= 全画像を標準設定に戻す)。
    /// ここでいう「標準」はこの一覧のコンテナに対応するお気に入り標準 or global_preset。
    /// 個別解除で AI 設定が標準に戻って変わるページは AI キャッシュもクリアする。
    pub(crate) fn clear_all_page_params(&mut self) {
        let (indices, keys) = self.collect_image_page_keys();
        let sidecar_coords = self.collect_image_sidecar_coords();
        let default_params = indices
            .first()
            .map(|&idx| self.effective_default_for_idx(idx).clone())
            .unwrap_or_else(|| self.settings.global_preset.clone());
        let ai_changed_indices: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|idx| !self.effective_params(*idx).ai_settings_eq(&default_params))
            .collect();
        for idx in &indices {
            self.adjustment_page_params.remove(idx);
        }
        if let Some(db) = self.adjustment_db.as_mut() {
            let _ = db.remove_page_params_bulk(&keys);
        }
        for (folder, rels) in sidecar_coords {
            if let Some(sc) = self.sidecar_mut(&folder) {
                sc.remove_adjust_bulk(rels);
            }
        }
        self.clear_all_color_caches();
        self.clear_ai_caches_for_indices(&ai_changed_indices);
    }

    /// 指定述語にマッチする画像ページ (Image / ZipImage / PdfPage) で個別設定を持たない
    /// idx を集める。`copy_params_to_global` / `set_favorite_default` /
    /// `clear_favorite_default` が「標準設定側を書き換えたときに AI 再実行が必要なページ」を
    /// 拾うために使う共通ヘルパー。
    fn image_idx_inheriting_default<F>(&self, favorite_pred: F) -> Vec<usize>
    where
        F: Fn(&Self, usize) -> bool,
    {
        (0..self.items.len())
            .filter(|idx| {
                matches!(
                    self.items.get(*idx),
                    Some(GridItem::Image(_))
                        | Some(GridItem::ZipImage { .. })
                        | Some(GridItem::PdfPage { .. })
                ) && !self.adjustment_page_params.contains_key(idx)
                    && favorite_pred(self, *idx)
            })
            .collect()
    }

    /// 指定パラメータを settings.global_preset にコピーして保存する。
    /// global の AI 設定が変わった場合、個別設定を持たない (= global を継承している)
    /// 画像ページの AI キャッシュもクリアして、新 global での再実行を促す。
    pub(crate) fn copy_params_to_global(&mut self, params: crate::adjustment::AdjustParams) {
        let ai_changed = !self.settings.global_preset.ai_settings_eq(&params);
        let ai_changed_indices: Vec<usize> = if ai_changed {
            // global を継承しているページ (個別 / お気に入り標準のどちらもない) だけ影響
            self.image_idx_inheriting_default(|app, idx| {
                app.favorite_default_for_idx(idx).is_none()
            })
        } else {
            Vec::new()
        };
        self.settings.global_preset = params;
        self.settings.save();
        self.clear_all_color_caches();
        self.clear_ai_caches_for_indices(&ai_changed_indices);
    }

    /// お気に入り単位の標準設定を変更する共通パス。
    /// `new_value = Some(params)` で設定、`None` で削除。
    fn apply_favorite_change(
        &mut self,
        favorite_id: uuid::Uuid,
        new_value: Option<crate::adjustment::AdjustParams>,
    ) {
        let old = self.adjustment_favorite_params.get(&favorite_id).cloned();
        let new_effective = new_value
            .clone()
            .unwrap_or_else(|| self.settings.global_preset.clone());
        let old_effective = old.unwrap_or_else(|| self.settings.global_preset.clone());
        let ai_changed = !old_effective.ai_settings_eq(&new_effective);
        let ai_changed_indices: Vec<usize> = if ai_changed {
            // このお気に入り傘下かつ個別設定なしのページのみ AI 影響あり
            self.image_idx_inheriting_default(|app, idx| {
                app.current_favorite_id_for_idx(idx) == Some(favorite_id)
            })
        } else {
            Vec::new()
        };
        match &new_value {
            Some(p) => {
                self.adjustment_favorite_params.insert(favorite_id, p.clone());
                if let Some(db) = &self.adjustment_db {
                    let _ = db.set_favorite_params(favorite_id, p);
                }
            }
            None => {
                self.adjustment_favorite_params.remove(&favorite_id);
                if let Some(db) = &self.adjustment_db {
                    let _ = db.remove_favorite_params(favorite_id);
                }
            }
        }
        // 標準が動いた後、このお気に入り傘下で個別設定が新標準と一致するページは冗長。
        // set_page_params と同じ不変条件 (個別 == effective_default なら個別削除) を
        // 維持する。これをやらないと「このお気に入りの標準にする」押下後もページが
        // PageOverride スコープに残り、スコープ表示や U/N/P ショートカットの挙動が
        // 想定外になる (Codex P2 指摘)。
        let redundant: Vec<usize> = (0..self.items.len())
            .filter(|idx| {
                self.adjustment_page_params
                    .get(idx)
                    .is_some_and(|p| p == &new_effective)
                    && self.current_favorite_id_for_idx(*idx) == Some(favorite_id)
            })
            .collect();
        for idx in redundant {
            self.adjustment_page_params.remove(&idx);
            if let Some(key) = self.page_path_key(idx) {
                if let Some(db) = &self.adjustment_db {
                    let _ = db.remove_page_params(&key);
                }
            }
            self.with_sidecar_mut(idx, |sc, rel| sc.remove_adjust(rel));
        }
        self.clear_all_color_caches();
        self.clear_ai_caches_for_indices(&ai_changed_indices);
    }

    /// 指定パラメータをお気に入りの標準設定として保存する。
    /// そのお気に入り配下で、個別設定を持たないページの表示が新しい標準に切り替わる。
    pub(crate) fn set_favorite_default(
        &mut self,
        favorite_id: uuid::Uuid,
        params: crate::adjustment::AdjustParams,
    ) {
        self.apply_favorite_change(favorite_id, Some(params));
    }

    /// お気に入りの標準設定を解除する (= そのお気に入りでは global_preset にフォールバック)。
    pub(crate) fn clear_favorite_default(&mut self, favorite_id: uuid::Uuid) {
        self.apply_favorite_change(favorite_id, None);
    }

    /// 起動時に `adjustment_db` からお気に入り標準を全件ロードし、
    /// `settings.favorites` に存在しない orphan 行を掃除する。
    /// main.rs の `App::default()` 直後で 1 回呼ばれる。
    pub(crate) fn hydrate_adjustment_favorite_params(&mut self) {
        let Some(db) = &self.adjustment_db else {
            return;
        };
        self.adjustment_favorite_params = db.load_all_favorite_params();
        let keep: std::collections::HashSet<uuid::Uuid> =
            self.settings.favorites.iter().map(|f| f.id).collect();
        if let Ok(removed) = db.prune_favorite_params(&keep) {
            if removed > 0 {
                self.adjustment_favorite_params
                    .retain(|id, _| keep.contains(id));
            }
        }
    }

    /// 保存スロット slot_idx のパラメータを `idx` のページに適用する。
    /// 見開き L/R 切替を考慮するため、適用先 idx は呼び出し元が決定する。
    pub(crate) fn apply_slot_to_idx(&mut self, slot_idx: usize, idx: usize) {
        let Some(slot) = self.settings.preset_slots.slots[slot_idx].clone() else {
            return;
        };
        let ai_changed = !self.effective_params(idx).ai_settings_eq(&slot.params);
        self.set_page_params(idx, slot.params);
        if ai_changed {
            self.clear_all_adjustment_and_ai_caches(idx);
        } else {
            self.clear_adjustment_caches(idx);
        }
        let key_label = crate::adjustment::slot_key_label(slot_idx);
        self.show_feedback_toast(format!("[スロット{}:{}]", key_label, slot.name));
    }

    /// 現在のフルスクリーンページに保存スロットを適用する (Ctrl+0..9 等のショートカット用)。
    /// 見開き Double のときは `adjust_spread_target` の選択 (左/右) を尊重する。
    pub(crate) fn apply_slot_to_current_page(&mut self, slot_idx: usize) {
        let Some(idx) = self.current_adjust_target_idx() else {
            return;
        };
        self.apply_slot_to_idx(slot_idx, idx);
    }

    /// フルスクリーン補正パネルが現在編集対象としているページ idx を返す。
    /// 単ページ表示なら `fullscreen_idx`、見開き Double なら `adjust_spread_target`
    /// に応じた左/右ページの idx。フルスクリーンを開いていなければ None。
    pub(crate) fn current_adjust_target_idx(&mut self) -> Option<usize> {
        let fs_idx = self.fullscreen_idx?;
        Some(match self.resolve_spread_pair(fs_idx) {
            crate::ui_fullscreen::SpreadPair::Double { left, right } => match self.adjust_spread_target {
                AdjustSpreadTarget::Left => left,
                AdjustSpreadTarget::Right => right,
            },
            crate::ui_fullscreen::SpreadPair::Single => fs_idx,
        })
    }

    /// 保存スロットをグリッド上の対象に適用する。対象はページ単位のみ
    /// (`ratable_page_targets`; 補正プリセットはコンテナに適用できない)。
    pub(crate) fn apply_slot_to_grid_selection(&mut self, slot_idx: usize) {
        let targets = self.ratable_page_targets();
        if targets.is_empty() {
            self.show_feedback_toast("[適用対象なし]".to_string());
            return;
        }

        let key_label = crate::adjustment::slot_key_label(slot_idx);
        let Some(slot) = self.settings.preset_slots.slots[slot_idx].clone() else {
            self.show_feedback_toast(format!("[スロット{key_label}は空です]"));
            return;
        };

        let mut any_ai_changed = false;
        for &idx in &targets {
            if !self.effective_params(idx).ai_settings_eq(&slot.params) {
                any_ai_changed = true;
            }
            self.set_page_params(idx, slot.params.clone());
            self.clear_adjustment_caches(idx);
        }
        // AI 設定が変わった target が 1 件でもあれば AI キャッシュ / 進行中 pending を
        // 全体単位で畳む (AI キャッシュは item idx キーでフラグメント化できないため)。
        if any_ai_changed {
            self.clear_all_adjustment_and_ai_caches(targets[0]);
        }

        let count = targets.len();
        if count == 1 {
            self.show_feedback_toast(format!("[スロット{}:{}]", key_label, slot.name));
        } else {
            self.show_feedback_toast(format!(
                "[スロット{}:{} を{}枚に適用]",
                key_label, slot.name, count
            ));
            self.checked.clear();
        }
    }

    /// グリッド上の対象 (チェック済み、なければ選択 1 件) の個別補正を一括解除する。
    /// Q / Ctrl+Backspace から呼ばれる。フルスクリーン側の単発版 (`clear_page_params`) と同じく
    /// AI 設定が変わる idx については AI キャッシュ / pending も落とす (clear_page_params 内で処理)。
    pub(crate) fn clear_page_params_for_selection(&mut self) {
        let targets = self.ratable_page_targets();
        if targets.is_empty() {
            self.show_feedback_toast("[対象なし]".to_string());
            return;
        }
        let to_clear: Vec<usize> = targets
            .iter()
            .copied()
            .filter(|idx| self.adjustment_page_params.contains_key(idx))
            .collect();
        if to_clear.is_empty() {
            self.show_feedback_toast("[個別設定なし]".to_string());
            return;
        }
        for &idx in &to_clear {
            self.clear_page_params(idx);
        }
        let count = to_clear.len();
        if count == 1 {
            self.show_feedback_toast("[個別設定を解除]".to_string());
        } else {
            self.show_feedback_toast(format!("[{}枚の個別設定を解除]", count));
            self.checked.clear();
        }
    }

    /// 指定ピクセルデータに色調補正を同期適用して adjustment_cache に格納する。
    /// poll_prefetch / poll_ai_upscale の完了時に呼ばれ、
    /// 補正済み画像を即座にテクスチャ化してチラつきを防止する。
    fn apply_sync_adjustment(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
        pixels: &std::sync::Arc<egui::ColorImage>,
    ) {
        let params = self.effective_params(idx).clone();
        // post-filter をバイパスする場合: 色調も identity ならスキップ可能
        let apply_pf =
            !self.post_filter_bypassed && params.post_filter != crate::adjustment::PostFilter::None;
        if params.is_color_identity() && !apply_pf {
            return;
        }
        let adjusted = crate::adjustment::apply_adjustments_fast(pixels, &params);
        let post_filtered = if apply_pf {
            crate::post_filter::apply(&adjusted, params.post_filter)
        } else {
            adjusted
        };
        let adjusted_pixels = std::sync::Arc::new(post_filtered);
        let upload = clamp_for_gpu(&adjusted_pixels);
        let tex_opts = if apply_pf && params.post_filter.needs_nearest_sampler() {
            egui::TextureOptions::NEAREST
        } else {
            egui::TextureOptions::LINEAR
        };
        let tex = ctx.load_texture(format!("adj_{idx}"), upload.into_owned(), tex_opts);
        // 派生キャッシュ。fs.paint は fs_cache 側から load_seq を拾うためここでは 0。
        // source_dims は fs_cache 側に保存されているのでここは None でよい。
        self.adjustment_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex,
                pixels: adjusted_pixels,
                source_dims: None,
                load_seq: 0,
            },
        );
    }

    /// 表示中画像の adjustment_cache がない場合、補正を同期適用する。
    /// 表示中の画像のみ処理し、先読み分はページ切替時に処理する。
    /// 見開き Double 表示中はペアの両方の idx について実行を許可する。
    /// `spread_pair` は呼び出し側で計算済みのものを受け取って毎フレームの再計算を避ける。
    pub(crate) fn maybe_apply_adjustment(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
        spread_pair: crate::ui_fullscreen::SpreadPair,
    ) {
        // 表示中の画像 (= 見開き時はペアの片方も含む) でなければスキップ。
        // 先読み分はページ切替時に処理。
        let in_view = match self.fullscreen_idx {
            Some(fs) if fs == idx => true,
            Some(_) => matches!(
                spread_pair,
                crate::ui_fullscreen::SpreadPair::Double { left, right }
                    if left == idx || right == idx
            ),
            None => false,
        };
        if !in_view {
            return;
        }
        // 既にキャッシュがあればスキップ
        if self.adjustment_cache.contains_key(&idx) {
            return;
        }
        let bg = self.effective_upscale_bg_mode();
        // AI 処理中でも fs_cache ベースの仮 adjustment_cache を用意する。
        // AI 完了時に poll_ai_upscale が adjustment_cache を無効化し、AI 結果で再生成する。
        // これがないと AI 完了まで「補正前の fs_cache」がそのまま表示され、
        // 完了瞬間に補正適用で濃度が跳ねて見えてしまう。
        // 短絡: 個別設定なし かつ グローバルが identity かつ お気に入り標準なし なら何もしない。
        // 順序はコスト昇順: HashMap 参照 → global_preset 構造体参照 → favorite 解決 (path 正規化)。
        // `favorite_default_for_idx` は path 正規化+ `find_nearest_favorite` のループで
        // 一番重いので、`&&` の短絡で前段の cheap 条件が外れたときにスキップされるようにする。
        if !self.adjustment_page_params.contains_key(&idx)
            && self.settings.global_preset.is_identity()
            && self.favorite_default_for_idx(idx).is_none()
        {
            return;
        }
        // bypass 中は post-filter を考慮せず、色調のみで判定する
        let params_ref = self.effective_params(idx);
        let apply_pf = !self.post_filter_bypassed
            && params_ref.post_filter != crate::adjustment::PostFilter::None;
        if params_ref.is_color_identity() && !apply_pf {
            return;
        }
        // ソース画像を取得 (AI アップスケール済み or 元画像)
        let source = if self.ai_upscale_enabled || self.ai_denoise_model.is_some() {
            self.ai_upscale_cache
                .get(&(idx, bg))
                .or_else(|| self.fs_cache.get(&idx))
        } else {
            self.fs_cache.get(&idx)
        };
        let Some(FsCacheEntry::Static { pixels, .. }) = source else {
            return;
        };
        let pixels = std::sync::Arc::clone(pixels);
        self.apply_sync_adjustment(ctx, idx, &pixels);
    }

    /// 指定ページの補正関連キャッシュをクリアする。
    /// 色調パラメータ変更時は adjustment_cache のみクリア。
    /// AI モデル設定変更時は ai_upscale_cache もクリアする。
    pub(crate) fn clear_adjustment_caches(&mut self, idx: usize) {
        self.adjustment_cache.remove(&idx);
        // サムネ側の補正済みテクスチャも同時に落とす (ピクセルは保持)。
        // 次フレーム以降 maybe_apply_thumb_adjustment で再生成される。
        self.thumb_adjust_tex.remove(&idx);
    }

    /// フルスクリーン補正キャッシュとサムネ補正テクスチャを同時に全クリアする。
    /// バルク系操作 (apply_params_to_all_pages / clear_all_page_params /
    /// copy_params_to_global) で表示優先順位の上位キャッシュを一掃するためのヘルパー。
    /// `thumb_pixels` (ソースピクセル) は keep_range で管理されているのでここでは触らない。
    pub(crate) fn clear_all_color_caches(&mut self) {
        self.adjustment_cache.clear();
        self.thumb_adjust_tex.clear();
    }

    /// サムネイル補正を同期適用する (色調のみ、post_filter は対象外)。
    /// 既に `thumb_adjust_tex[idx]` があればスキップ。ピクセル未保持ならスキップ
    /// (keep_range 外で evict 済み)。identity (無補正) なら再描画時に生サムネを
    /// そのまま使うため、キャッシュ生成も行わない。
    pub(crate) fn maybe_apply_thumb_adjustment(&mut self, ctx: &egui::Context, idx: usize) {
        if !is_thumb_adjust_target(self.items.get(idx)) {
            return;
        }
        if self.thumb_adjust_tex.contains_key(&idx) {
            return;
        }
        let Some(pixels) = self.thumb_pixels.get(&idx).cloned() else {
            return;
        };
        let adjusted = {
            let params = self.effective_params(idx);
            if params.is_color_identity() {
                return;
            }
            crate::adjustment::apply_adjustments_fast(&pixels, params)
        };
        let handle = ctx.load_texture(
            format!("thumb_adj_{idx}"),
            adjusted,
            egui::TextureOptions::LINEAR,
        );
        self.thumb_adjust_tex.insert(idx, handle);
    }

    /// keep_set 内の「ピクセルは持っているが補正テクスチャがまだ無い」idx を
    /// 最大 `budget` 件処理する。可視範囲は ui_main 側で同期適用済みなので、
    /// ここでは主に先読み範囲 (可視外) の補正を背後で埋めるのに使う。
    /// スライダードラッグ中は 1 件も処理しない (リリース時にまとめて再生成)。
    pub(crate) fn process_thumb_adjust_budget(&mut self, ctx: &egui::Context, budget: usize) {
        if self.adjustment_dragging {
            return;
        }
        let mut processed = 0usize;
        for idx in self.keep_set_sorted() {
            if processed >= budget {
                break;
            }
            if !is_thumb_adjust_target(self.items.get(idx)) {
                continue;
            }
            if self.thumb_adjust_tex.contains_key(&idx) {
                continue;
            }
            if !self.thumb_pixels.contains_key(&idx) {
                continue;
            }
            if self.effective_params(idx).is_color_identity() {
                continue;
            }
            self.maybe_apply_thumb_adjustment(ctx, idx);
            processed += 1;
        }
        if processed > 0 {
            ctx.request_repaint();
        }
    }

    /// スライダードラッグの true → false 遷移を検知して `thumb_adjust_tex`
    /// を全無効化する。ピクセル (`thumb_pixels`) は保持し続けるので、次フレーム
    /// 以降の `maybe_apply_thumb_adjustment` が新パラメータで再生成する。
    /// 毎フレーム update() の終盤に呼ぶ。
    ///
    /// フルスクリーン外 (補正パネル非描画) では `adjustment_dragging` が真の値に
    /// 更新されないため、ここで強制的に false に戻す。フルスクリーンをドラッグ
    /// 途中で閉じても、release 検知が正しく走ってサムネ補正が再生成される。
    pub(crate) fn update_thumb_adjust_drag_state(&mut self) {
        if self.fullscreen_idx.is_none() {
            self.adjustment_dragging = false;
        }
        let was = self.thumb_adjust_was_dragging;
        let now = self.adjustment_dragging;
        if was && !now {
            self.thumb_adjust_tex.clear();
        }
        self.thumb_adjust_was_dragging = now;
    }

    /// AI モデル設定が変わった場合に AI キャッシュを含めてクリアする。
    pub(crate) fn clear_all_adjustment_and_ai_caches(&mut self, idx: usize) {
        self.adjustment_cache.remove(&idx);
        // サムネ側の補正済みテクスチャも該当 idx のみクリア (色調系と同じ粒度)。
        self.thumb_adjust_tex.remove(&idx);
        self.ai_upscale_cache.clear();
        self.ai_upscale_failed.clear();
        for (_, (cancel, _)) in self.ai_upscale_pending.drain() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// 見開き表示で `src_idx` の有効な補正値を `dst_idx` にコピーし、Undo に積む。
    /// コピー先がカスケード解決後のデフォルトと一致するなら page_params エントリは作られない。
    pub(crate) fn copy_spread_adjust(&mut self, src_idx: usize, dst_idx: usize) {
        if src_idx == dst_idx {
            return;
        }
        let new_params = self.effective_params(src_idx).clone();
        let ai_changed = !self.effective_params(dst_idx).ai_settings_eq(&new_params);

        self.capture_adjust_full("見開き補正コピー".to_string(), |app| {
            app.set_page_params(dst_idx, new_params);
        });

        if ai_changed {
            self.clear_all_adjustment_and_ai_caches(dst_idx);
        } else {
            self.clear_adjustment_caches(dst_idx);
        }
    }

    /// 指定された複数 idx について AI キャッシュ (ai_upscale_cache / failed / pending) を
    /// まとめてクリアする。bulk / global 系の補正操作で「AI 設定が変わった idx だけ
    /// AI 出力を再実行させたい」ときに使う。
    pub(crate) fn clear_ai_caches_for_indices(&mut self, indices: &[usize]) {
        for idx in indices {
            self.purge_upscale_for_idx(*idx);
        }
    }

    /// 補正キャッシュを evict する（prefetch 範囲外）。
    pub(crate) fn evict_adjustment_cache(&mut self, current_idx: usize) {
        let keep_set = self.compute_keep_set(current_idx);
        self.adjustment_cache.retain(|k, _| keep_set.contains(k));
    }

    /// `self.selected` に対応するアイテムが画像の場合、パスを last_selected_image_path に保存する。
    /// (フォルダ移動後もサムネイル画質ダイアログで使えるよう、セッション内で保持)
    pub(crate) fn update_last_selected_image(&mut self) {
        if let Some(idx) = self.selected {
            if let Some(GridItem::Image(p)) = self.items.get(idx) {
                self.last_selected_image_path = Some(p.clone());
            }
        }
    }

    /// pending の読み込みをポーリングし、完了したものをキャッシュに取り込む。
    ///
    /// **GPU アップロード ペーシング**: デコード完了した `FsLoadResult` は
    /// 一旦 `fs_upload_backlog` に積み、1 フレームあたり最大 1 枚だけ `ctx.load_texture`
    /// する。現在フルスクリーン表示中の idx は即時アップロードして表示遅延ゼロ。
    /// これにより 20MP JPEG 連続 prefetch 時の 500ms 級 UI フリーズを回避する。
    pub(crate) fn poll_prefetch(&mut self, ctx: &egui::Context) {
        // PDF ページの content_type を更新 (render 完了時にワーカーから受信)
        while let Ok((idx, ct)) = self.pdf_content_type_rx.try_recv() {
            if let Some(GridItem::PdfPage { content_type, .. }) = self.items.get_mut(idx) {
                *content_type = Some(ct);
            }
        }

        // `DimsOnly` は非終端 (後続メッセージあり) なので fs_pending は維持して
        // drain を続ける。fs_early_dims への書き込みは fs_pending 借用中はできないので
        // ローカル vec に積んでから後段で apply する。
        let mut completed: Vec<(usize, FsLoadResult, u64)> = Vec::new();
        let mut disconnected: Vec<usize> = Vec::new();
        let mut early_dims_updates: Vec<(usize, [usize; 2])> = Vec::new();
        for (&key, (_, rx, seq)) in &self.fs_pending {
            loop {
                match rx.try_recv() {
                    Ok(FsLoadResult::DimsOnly { source_dims }) => {
                        early_dims_updates.push((key, source_dims));
                        continue;
                    }
                    Ok(result) => {
                        completed.push((key, result, *seq));
                        break;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected.push(key);
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
        }
        let early_dims_repaint = !early_dims_updates.is_empty();
        for (key, dims) in early_dims_updates {
            self.fs_early_dims.insert(key, dims);
        }
        // 送信側が drop されたエントリを除去 (キャンセル済みスレッドが送信せずに終了)
        for key in disconnected {
            self.fs_pending.remove(&key);
            self.fs_early_dims.remove(&key);
        }
        // fs_pending からは即座に除去 (「先読み中 / 完了」状態の重複を防ぐ)。
        // backlog に積まれた時点で fs_cache に載る手前の最終中継点として扱う。
        for (key, _, _) in &completed {
            self.fs_pending.remove(key);
            self.fs_early_dims.remove(key);
        }
        // 既存 backlog の重複エントリ (同 idx で再ロードされたケース) は新しい方で置換。
        for (key, result, load_seq) in completed {
            if let Some(pos) = self
                .fs_upload_backlog
                .iter()
                .position(|(k, _, _)| *k == key)
            {
                self.fs_upload_backlog[pos] = (key, result, load_seq);
            } else {
                self.fs_upload_backlog.push((key, result, load_seq));
            }
        }

        // ── ペーシング: このフレームで何枚アップロードするか決める ──
        // 1. 現在フルスクリーン表示中の idx (= ユーザーが待っている画像) は即時に処理
        // 2. 他の先読み分は FIFO 先頭から 1 枚だけ取り出す
        // backlog は通常 <10 要素なので `Vec::remove` の O(n) は実質コストなし。
        let cur = self.fullscreen_idx;
        let (mut cur_pos, mut other_pos) = (None, None);
        for (i, (k, _, _)) in self.fs_upload_backlog.iter().enumerate() {
            if Some(*k) == cur {
                cur_pos = Some(i);
            } else if other_pos.is_none() {
                other_pos = Some(i);
            }
            if cur_pos.is_some() && other_pos.is_some() {
                break;
            }
        }

        // 両方ある場合は元の位置順 (FIFO) を維持するため、大きい位置から remove する。
        let mut positions: Vec<usize> = [cur_pos, other_pos].into_iter().flatten().collect();
        positions.sort_unstable_by(|a, b| b.cmp(a));
        let mut to_process: Vec<(usize, FsLoadResult, u64)> = positions
            .into_iter()
            .map(|pos| self.fs_upload_backlog.remove(pos))
            .collect();
        // descending で取り出したので元の位置順に直す
        to_process.reverse();

        let has_more_backlog = !self.fs_upload_backlog.is_empty();
        let repaint = !to_process.is_empty() || early_dims_repaint || has_more_backlog;
        for (key, result, load_seq) in to_process {
            // 本体メッセージで fs_cache が埋まるので先行ヒントはもう不要。
            // (ホバーバーは fs_cache.source_dims を優先して見るため、残っていても
            // 実害はないが HashMap が膨張しないようクリーンアップする)
            self.fs_early_dims.remove(&key);
            let perf_key_str = self.perf_item_key(key);
            let upload_t0 = std::time::Instant::now();
            let entry = match result {
                FsLoadResult::Static { ci, source_dims } => {
                    let pixels = std::sync::Arc::new(ci);
                    let upload = clamp_for_gpu(&pixels);
                    let [w, h] = pixels.size;
                    let handle = ctx.load_texture(
                        format!("fs_{key}"),
                        upload.into_owned(),
                        egui::TextureOptions::LINEAR,
                    );
                    let upload_ms = upload_t0.elapsed().as_secs_f64() * 1000.0;
                    // `load_seq` を使うのは、decode 中に別操作が入っても
                    // ready が load_begin と同じシーケンスに紐づくようにするため。
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "fs",
                            "ready",
                            perf_key_str.as_deref(),
                            load_seq,
                            &[
                                ("idx", serde_json::Value::from(key)),
                                ("upload_ms", serde_json::Value::from(upload_ms)),
                                ("w", serde_json::Value::from(w)),
                                ("h", serde_json::Value::from(h)),
                                ("result_kind", serde_json::Value::from("static")),
                            ],
                        );
                    }
                    // 表示中の画像のみ色調補正を即座に適用（チラつき防止）
                    // 先読み分は maybe_apply_adjustment に委ねる
                    if self.fullscreen_idx == Some(key) {
                        self.apply_sync_adjustment(ctx, key, &pixels);
                    }
                    FsCacheEntry::Static {
                        tex: handle,
                        pixels,
                        source_dims: Some(source_dims),
                        load_seq,
                    }
                }
                FsLoadResult::Animated(frames) => {
                    let textures: Vec<(egui::TextureHandle, f64)> = frames
                        .into_iter()
                        .enumerate()
                        .map(|(fi, (ci, delay))| {
                            let handle = ctx.load_texture(
                                format!("fs_{key}_f{fi}"),
                                ci,
                                egui::TextureOptions::LINEAR,
                            );
                            (handle, delay)
                        })
                        .collect();
                    let now = ctx.input(|i| i.time);
                    let first_delay = textures.first().map(|(_, d)| *d).unwrap_or(0.1);
                    FsCacheEntry::Animated {
                        frames: textures,
                        current_frame: 0,
                        next_frame_at: now + first_delay,
                        load_seq,
                    }
                }
                FsLoadResult::Failed => FsCacheEntry::Failed,
                FsLoadResult::DimsOnly { .. } => {
                    unreachable!("DimsOnly should be drained before reaching completion match")
                }
            };
            self.fs_cache.insert(key, entry);
            // 保存済みマスクがあれば自動で inpaint 適用
            self.auto_apply_saved_mask(ctx, key);
            if self.fullscreen_idx == Some(key) {
                self.update_prefetch_window(key);
            }
        }
        if repaint {
            ctx.request_repaint();
        }
    }

    /// 動画再生プレイヤーの tick。
    /// fs_cache 内の `FsCacheEntry::Video` ごとに [`crate::video::VideoPlayer::tick`] を呼ぶ。
    /// 通常はフルスクリーン中の 1 つだけが入っている (動画は先読みしないため)。
    pub(crate) fn save_all_video_resume_positions(&mut self) {
        let mut updates: Vec<(String, f64, f64)> = Vec::new();
        for entry in self.fs_cache.values() {
            if let FsCacheEntry::Video { player, .. } = entry {
                let key = crate::adjustment_db::normalize_path(player.path());
                updates.push((key, player.position(), player.duration()));
            }
        }
        for (key, pos, dur) in updates {
            save_video_resume_position(&mut self.settings.video_resume_positions, key, pos, dur);
        }
    }

    pub(crate) fn poll_video(&mut self, ctx: &egui::Context) {
        let mut next_repaint: Option<std::time::Duration> = None;
        // 5 秒ごとに動画再生位置を settings に書き戻す (drop 漏れ救済)。
        let now = std::time::Instant::now();
        let do_save = self
            .video_resume_last_save
            .map(|t| now.duration_since(t).as_secs_f64() >= 5.0)
            .unwrap_or(true);
        let mut updates: Vec<(String, f64, f64)> = Vec::new();
        let loop_enabled = self.settings.video_loop;
        for entry in self.fs_cache.values_mut() {
            if let FsCacheEntry::Video { player, .. } = entry {
                player.set_loop_enabled(loop_enabled);
                if let Some(d) = player.tick(ctx) {
                    next_repaint = Some(match next_repaint {
                        Some(prev) => prev.min(d),
                        None => d,
                    });
                }
                if do_save {
                    let path_key = crate::adjustment_db::normalize_path(player.path());
                    updates.push((path_key, player.position(), player.duration()));
                }
            }
        }
        if do_save {
            self.video_resume_last_save = Some(now);
            for (key, pos, dur) in updates {
                save_video_resume_position(&mut self.settings.video_resume_positions, key, pos, dur);
            }
        }
        if let Some(d) = next_repaint {
            ctx.request_repaint_after(d);
        }
    }

    // -------------------------------------------------------------------
    // サムネイル画質設定ダイアログ (A/B 比較)
    // -------------------------------------------------------------------
    pub(crate) fn open_thumb_quality_dialog(&mut self, _ctx: &egui::Context) {
        // 既存状態をリセット
        self.tq.sample = None;
        self.tq.sample_path = None;
        self.tq.sample_original_size = 0;
        self.tq.a_texture = None;
        self.tq.b_texture = None;
        self.tq.a_bytes = 0;
        self.tq.b_bytes = 0;
        self.tq.load_pending = None;

        // A/B スライダー初期値はダイアログを開いた瞬間に確定しておく
        // (decode 待ち中にユーザがスライダーを触っても同期的に反映できるように)
        self.tq.a_size = self.settings.thumb_px;
        self.tq.a_quality = self.settings.thumb_quality;
        self.tq.b_size = self.settings.thumb_px;
        self.tq.b_quality = (self.settings.thumb_quality as u32 + 10).min(95) as u8;

        // 最後に選択した画像を取得
        let Some(path) = self.last_selected_image_path.clone() else {
            // None のままダイアログを開く (メッセージだけ出る)
            self.tq.show = true;
            return;
        };

        // decode を worker に回す。20MP 超や巨大 RAW の image::open + metadata は UI を
        // 数百ms〜秒単位止めるため同期実行しない。ダイアログは即座に「読み込み中」で開く。
        let (tx, rx) = mpsc::channel();
        let path_for_worker = path.clone();
        std::thread::Builder::new()
            .name("thumb-quality-sample-decode".into())
            .spawn(move || {
                let result = image::open(&path_for_worker).ok().map(|img| {
                    let orig = std::fs::metadata(&path_for_worker)
                        .map(|m| m.len())
                        .unwrap_or(0);
                    (img, orig)
                });
                let _ = tx.send(result);
            })
            .ok();
        self.tq.load_pending = Some(ThumbQualityLoadPending { path, rx });
        self.tq.show = true;
    }

    /// worker からの decode 結果を拾い、サンプル確定時に A/B プレビューを初期生成する。
    pub(crate) fn poll_thumb_quality_pending(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.tq.load_pending.as_ref() else {
            return;
        };
        let msg = match pending.rx.try_recv() {
            Ok(m) => m,
            Err(mpsc::TryRecvError::Empty) => {
                if self.tq.show {
                    // decode 待ちの間はプログレス表示更新のために再描画要求
                    ctx.request_repaint();
                }
                return;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.tq.load_pending = None;
                return;
            }
        };
        let path = pending.path.clone();
        self.tq.load_pending = None;
        let Some((img, orig_size)) = msg else {
            // decode 失敗 — 空状態のまま残す
            return;
        };
        self.tq.sample = Some(Arc::new(img));
        self.tq.sample_path = Some(path);
        self.tq.sample_original_size = orig_size;
        self.reencode_tq_panel(true);
        self.reencode_tq_panel(false);
    }

    /// A/B プレビューの再エンコードを worker に依頼する。
    /// `encode_thumb_webp` + resize + webp + `decode_thumb_to_color_image` は 20MP 級で
    /// 合計 100-300ms かかる。スライダー操作で連射される場合、前回 pending は cancel して
    /// 最新だけ texture に反映する。
    pub(crate) fn reencode_tq_panel(&mut self, is_a: bool) {
        let Some(sample) = self.tq.sample.clone() else {
            return;
        };
        let (size, quality) = if is_a {
            (self.tq.a_size, self.tq.a_quality)
        } else {
            (self.tq.b_size, self.tq.b_quality)
        };

        // 前回の encode pending は cancel。まだ走っていれば send 前に早期 return してくれる。
        if let Some(prev) = if is_a {
            self.tq.a_encode_pending.as_ref()
        } else {
            self.tq.b_encode_pending.as_ref()
        } {
            prev.cancel.store(true, Ordering::Relaxed);
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let cancel_worker = Arc::clone(&cancel);
        std::thread::Builder::new()
            .name("thumb-quality-encode".into())
            .spawn(move || {
                if cancel_worker.load(Ordering::Relaxed) {
                    return;
                }
                let encoded = crate::catalog::encode_thumb_webp(&sample, size, quality as f32);
                if cancel_worker.load(Ordering::Relaxed) {
                    return;
                }
                let result = encoded.and_then(|(data, _w, _h)| {
                    let bytes = data.len();
                    crate::catalog::decode_thumb_to_color_image(&data)
                        .map(|color_image| TqEncodeResult { bytes, color_image })
                });
                if cancel_worker.load(Ordering::Relaxed) {
                    return;
                }
                let _ = tx.send(result);
            })
            .ok();

        let pending = TqEncodePending { cancel, rx };
        if is_a {
            self.tq.a_encode_pending = Some(pending);
        } else {
            self.tq.b_encode_pending = Some(pending);
        }
    }

    /// A/B encode worker の完了を拾う。`load_texture` だけ UI スレッドで実行する。
    /// 未完了の pending が残っている間は再描画要求する。
    pub(crate) fn poll_tq_encode_pending(&mut self, ctx: &egui::Context) {
        // A 側
        let mut a_repaint_needed = false;
        if let Some(pending) = self.tq.a_encode_pending.as_ref() {
            match pending.rx.try_recv() {
                Ok(Some(result)) => {
                    let tex = ctx.load_texture(
                        "tq_preview_a",
                        result.color_image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.tq.a_bytes = result.bytes;
                    self.tq.a_texture = Some(tex);
                    self.tq.a_encode_pending = None;
                }
                Ok(None) => {
                    // encode 失敗。旧テクスチャは残す (0 にリセットだけ)。
                    self.tq.a_bytes = 0;
                    self.tq.a_encode_pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    a_repaint_needed = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.tq.a_encode_pending = None;
                }
            }
        }
        // B 側
        let mut b_repaint_needed = false;
        if let Some(pending) = self.tq.b_encode_pending.as_ref() {
            match pending.rx.try_recv() {
                Ok(Some(result)) => {
                    let tex = ctx.load_texture(
                        "tq_preview_b",
                        result.color_image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.tq.b_bytes = result.bytes;
                    self.tq.b_texture = Some(tex);
                    self.tq.b_encode_pending = None;
                }
                Ok(None) => {
                    self.tq.b_bytes = 0;
                    self.tq.b_encode_pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    b_repaint_needed = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.tq.b_encode_pending = None;
                }
            }
        }
        if a_repaint_needed || b_repaint_needed {
            ctx.request_repaint();
        }
    }

    pub(crate) fn close_thumb_quality_dialog(&mut self) {
        self.tq.show = false;
        self.tq.sample = None;
        self.tq.sample_path = None;
        self.tq.a_texture = None;
        self.tq.b_texture = None;
        self.tq.fullscreen = false;
        self.tq.load_pending = None;
        // encode worker 停止要求。pending 構造体が drop されると tx も落ちるので追加送信は無視される。
        if let Some(p) = self.tq.a_encode_pending.as_ref() {
            p.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(p) = self.tq.b_encode_pending.as_ref() {
            p.cancel.store(true, Ordering::Relaxed);
        }
        self.tq.a_encode_pending = None;
        self.tq.b_encode_pending = None;
    }

    // -------------------------------------------------------------------
    // キャッシュ作成（バックグラウンドで選択フォルダ以下を再帰処理）
    // -------------------------------------------------------------------
    pub(crate) fn start_cache_creation(&mut self) {
        // 選択されたお気に入りを集める（名前とパスのペア）
        let targets: Vec<(String, PathBuf)> = self
            .settings
            .favorites
            .iter()
            .zip(self.cc.checked.iter())
            .filter_map(|(f, &c)| {
                if c {
                    Some((f.name.clone(), f.path.clone()))
                } else {
                    None
                }
            })
            .collect();

        if targets.is_empty() {
            return;
        }

        // 状態リセット
        self.cc.running = true;
        self.cc.counting.store(true, Ordering::Relaxed);
        self.cc.total.store(0, Ordering::Relaxed);
        self.cc.done.store(0, Ordering::Relaxed);
        self.cc.finished.store(false, Ordering::Relaxed);
        self.cc.result = None;
        *self.cc.current.lock().unwrap() = String::new();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cc.cancel = Arc::clone(&cancel);

        // ベースラインは worker 側で取得する。`cache_stats` は read_dir + metadata の全走査で
        // キャッシュフォルダが大きいと UI スレッドで数百 ms ブロックしうるため、開始ボタン直後は
        // 0 のまま返して worker 内で書き換える (docs/ui-responsiveness.md §4 チェックリスト)。
        self.cc.cache_size.store(0, Ordering::Relaxed);

        // atomic クローン
        let counting = Arc::clone(&self.cc.counting);
        let total = Arc::clone(&self.cc.total);
        let done = Arc::clone(&self.cc.done);
        let size_atomic = Arc::clone(&self.cc.cache_size);
        let finished = Arc::clone(&self.cc.finished);
        let current = Arc::clone(&self.cc.current);
        let thumb_px = self.settings.thumb_px;
        let thumb_quality = self.settings.thumb_quality;
        let threads = self.settings.parallelism.thread_count();
        let batch_zip = self.settings.batch_cache_zip_contents;
        let batch_pdf = self.settings.batch_cache_pdf_contents;

        std::thread::spawn(move || {
            // baseline: worker 冒頭で取得 (UI スレッドブロッキング回避)
            let cache_dir = crate::catalog::default_cache_dir();
            let (_, baseline) = crate::catalog::cache_stats(&cache_dir);
            size_atomic.store(baseline, Ordering::Relaxed);

            // Pass 1: カウント
            let mut all_folders: Vec<PathBuf> = Vec::new();
            for (_, path) in &targets {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                walk_dirs_recursive(path, &mut all_folders, &cancel);
            }
            total.store(all_folders.len(), Ordering::Relaxed);
            counting.store(false, Ordering::Relaxed);

            if cancel.load(Ordering::Relaxed) {
                finished.store(true, Ordering::Relaxed);
                return;
            }

            // 処理用 rayon プール
            let pool = match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
                Ok(p) => p,
                Err(_) => {
                    finished.store(true, Ordering::Relaxed);
                    return;
                }
            };

            // Pass 2: フォルダを順次処理、内部画像は並列デコード
            for folder in &all_folders {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }

                // お気に入り名 > 相対パス の形式で表示用文字列を生成
                let folder_display = targets
                    .iter()
                    .find(|(_, base)| folder.starts_with(base))
                    .map(|(name, base)| match folder.strip_prefix(base) {
                        Ok(rel) if rel.as_os_str().is_empty() => name.clone(),
                        Ok(rel) => format!("{} > {}", name, rel.to_string_lossy()),
                        Err(_) => folder.to_string_lossy().to_string(),
                    })
                    .unwrap_or_else(|| folder.to_string_lossy().to_string());
                *current.lock().unwrap() = folder_display.clone();

                // ファイル列挙（単一フォルダ、再帰なし — 画像・ZIP・PDF を1パスで分類）
                let mut images: Vec<(PathBuf, i64, i64)> = Vec::new();
                let mut zip_files: Vec<(PathBuf, i64, i64)> = Vec::new();
                let mut pdf_files: Vec<(PathBuf, i64, i64)> = Vec::new();
                if let Ok(entries) = std::fs::read_dir(folder) {
                    for entry in entries.flatten() {
                        // entry.file_type() は FindFirstFile/FindNextFile の戻りを再利用するので
                        // per-entry GetFileAttributes syscall を避けられる
                        // (docs/ui-responsiveness.md §4)。キャッシュ作成の大量フォルダ走査で効く。
                        let Ok(ft) = entry.file_type() else {
                            continue;
                        };
                        if !ft.is_file() {
                            continue;
                        }
                        let p = entry.path();
                        if is_apple_double(&p) {
                            continue;
                        }
                        let Some(ext) = p.extension().and_then(|e| e.to_str()) else {
                            continue;
                        };
                        let ext_lower = ext.to_ascii_lowercase();
                        let meta = || {
                            let m = entry.metadata().ok()?;
                            let mtime = crate::ui_helpers::mtime_secs(&m);
                            let file_size = m.len() as i64;
                            Some((mtime, file_size))
                        };
                        if crate::folder_tree::is_recognized_image_ext(&ext_lower) {
                            if let Some((mt, fs)) = meta() {
                                images.push((p, mt, fs));
                            }
                        } else if ext_lower == "zip" {
                            if let Some((mt, fs)) = meta() {
                                zip_files.push((p, mt, fs));
                            }
                        } else if ext_lower == "pdf" {
                            if let Some((mt, fs)) = meta() {
                                pdf_files.push((p, mt, fs));
                            }
                        }
                    }
                }

                if images.is_empty() && zip_files.is_empty() && pdf_files.is_empty() {
                    done.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                // カタログを開く（1フォルダ1DB）
                let Ok(catalog) = crate::catalog::CatalogDb::open(&cache_dir, folder) else {
                    done.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let cache_map = catalog.load_all().unwrap_or_default();

                // ── 画像を並列でデコード + 保存 ──
                if !images.is_empty() {
                    pool.install(|| {
                        use rayon::prelude::*;
                        images.par_iter().for_each(|(path, mtime, file_size)| {
                            if cancel.load(Ordering::Relaxed) {
                                return;
                            }
                            let filename = match path.file_name().and_then(|n| n.to_str()) {
                                Some(n) => n,
                                None => return,
                            };
                            if let Some(entry) = cache_map.get(filename) {
                                if entry.mtime == *mtime && entry.file_size == *file_size {
                                    return;
                                }
                            }
                            if let Some(bytes) = build_and_save_one(
                                path,
                                &catalog,
                                *mtime,
                                *file_size,
                                thumb_px,
                                thumb_quality,
                            ) {
                                size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                            }
                        });
                    });
                }

                // ── ZIP ファイルの中身をキャッシュ ──
                for (zip_path, zip_mtime, zip_file_size) in &zip_files {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let zip_fname = match zip_path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    let folder_key = format!("{}{}", CACHE_KEY_ZIP, zip_fname);

                    if batch_zip {
                        *current.lock().unwrap() = format!("{} > {}", folder_display, zip_fname);
                        let entries = match crate::zip_loader::enumerate_image_entries(zip_path) {
                            Ok(e) => e,
                            Err(_) => continue,
                        };
                        let zip_catalog =
                            match crate::catalog::CatalogDb::open(&cache_dir, zip_path) {
                                Ok(c) => c,
                                Err(_) => continue,
                            };
                        let zip_cache_map = zip_catalog.load_all().unwrap_or_default();
                        let entry_count = entries.len();

                        // 先頭エントリの WebP を並列処理中にキャプチャ
                        let first_webp: Arc<Mutex<Option<(image::DynamicImage, String)>>> =
                            Arc::new(Mutex::new(None));

                        pool.install(|| {
                            use rayon::prelude::*;
                            entries.par_iter().enumerate().for_each(|(i, entry)| {
                                if cancel.load(Ordering::Relaxed) {
                                    return;
                                }
                                *current.lock().unwrap() = format!(
                                    "{} > {} ({}/{})",
                                    folder_display,
                                    zip_fname,
                                    i + 1,
                                    entry_count
                                );
                                if let Some(existing) = zip_cache_map.get(&entry.entry_name) {
                                    if existing.mtime == entry.mtime
                                        && existing.file_size == entry.uncompressed_size as i64
                                    {
                                        return;
                                    }
                                }
                                let raw = match crate::zip_loader::read_entry_bytes(
                                    zip_path,
                                    &entry.entry_name,
                                ) {
                                    Ok(b) => b,
                                    Err(_) => return,
                                };
                                let img = match image::load_from_memory(&raw) {
                                    Ok(i) => i,
                                    Err(_) => return,
                                };
                                // 先頭エントリをキャプチャ（親フォルダ用サムネイル再利用）
                                if i == 0 {
                                    *first_webp.lock().unwrap() =
                                        Some((img.clone(), entry.entry_name.clone()));
                                }
                                if let Some(bytes) = encode_and_save(
                                    &img,
                                    &entry.entry_name,
                                    &zip_catalog,
                                    entry.mtime,
                                    entry.uncompressed_size as i64,
                                    thumb_px,
                                    thumb_quality,
                                ) {
                                    size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                }
                            });
                        });

                        // 先頭1枚を親フォルダの DB にも保存（フォルダ一覧用サムネイル）
                        if !cache_map.contains_key(&folder_key) {
                            let captured = first_webp.lock().unwrap().take();
                            if let Some((img, _)) = captured {
                                if let Some(bytes) = encode_and_save(
                                    &img,
                                    &folder_key,
                                    &catalog,
                                    *zip_mtime,
                                    *zip_file_size,
                                    thumb_px,
                                    thumb_quality,
                                ) {
                                    size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                }
                            }
                        }
                    } else {
                        // 先頭1枚のみ（フォルダ一覧用サムネイル）
                        if cache_map.contains_key(&folder_key) {
                            continue;
                        }
                        if let Some(first_entry) =
                            crate::zip_loader::first_image_entry(zip_path, None)
                        {
                            if let Ok(raw) =
                                crate::zip_loader::read_entry_bytes(zip_path, &first_entry)
                            {
                                if let Ok(img) = image::load_from_memory(&raw) {
                                    if let Some(bytes) = encode_and_save(
                                        &img,
                                        &folder_key,
                                        &catalog,
                                        *zip_mtime,
                                        *zip_file_size,
                                        thumb_px,
                                        thumb_quality,
                                    ) {
                                        size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                }

                // ── PDF ファイルの中身をキャッシュ ──
                if !pdf_files.is_empty() && !cancel.load(Ordering::Relaxed) {
                    let pw_store = crate::pdf_passwords::PdfPasswordStore::load();

                    for (pdf_path, pdf_mtime, pdf_file_size) in &pdf_files {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let pdf_fname = match pdf_path.file_name().and_then(|n| n.to_str()) {
                            Some(n) => n.to_string(),
                            None => continue,
                        };
                        *current.lock().unwrap() = format!("{} > {}", folder_display, pdf_fname);
                        let password = pw_store.get(pdf_path);
                        let pw_ref = password.as_deref();
                        let folder_key = format!("{}{}", CACHE_KEY_PDF, pdf_fname);

                        if batch_pdf {
                            // enumerate_pages がパスワード不正時に Err を返すので
                            // check_password_needed は不要
                            let pages = match crate::pdf_loader::enumerate_pages(pdf_path, pw_ref) {
                                Ok(p) => p,
                                Err(_) => continue,
                            };
                            let pdf_catalog =
                                match crate::catalog::CatalogDb::open(&cache_dir, pdf_path) {
                                    Ok(c) => c,
                                    Err(_) => continue,
                                };
                            let pdf_cache_map = pdf_catalog.load_all().unwrap_or_default();
                            let page_count = pages.len();

                            // PDFium ワーカーはシングルスレッド → 順次処理
                            for i in 0..page_count {
                                if cancel.load(Ordering::Relaxed) {
                                    break;
                                }
                                let page_num = i as u32;
                                *current.lock().unwrap() = format!(
                                    "{} > {} ({}/{})",
                                    folder_display,
                                    pdf_fname,
                                    i + 1,
                                    page_count
                                );
                                let key = crate::grid_item::pdf_page_cache_key(page_num);
                                if let Some(existing) = pdf_cache_map.get(&key) {
                                    if existing.mtime == *pdf_mtime
                                        && existing.file_size == *pdf_file_size
                                    {
                                        continue;
                                    }
                                }
                                if let Some(bytes) = crate::thumb_loader::build_and_save_one_pdf(
                                    pdf_path,
                                    page_num,
                                    pw_ref,
                                    &pdf_catalog,
                                    *pdf_mtime,
                                    *pdf_file_size,
                                    thumb_px,
                                    thumb_quality,
                                ) {
                                    size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                }
                            }

                            // 先頭1ページを親フォルダの DB にも保存
                            if page_count > 0 && !cache_map.contains_key(&folder_key) {
                                if let Ok((img, _)) = crate::pdf_loader::render_page(
                                    pdf_path,
                                    0,
                                    thumb_px,
                                    pw_ref,
                                    None,
                                    crate::pdf_loader::JobPriority::Normal,
                                ) {
                                    if let Some(bytes) = encode_and_save(
                                        &img,
                                        &folder_key,
                                        &catalog,
                                        *pdf_mtime,
                                        *pdf_file_size,
                                        thumb_px,
                                        thumb_quality,
                                    ) {
                                        size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                    }
                                }
                            }
                        } else {
                            // 先頭1ページのみ（フォルダ一覧用サムネイル）
                            if cache_map.contains_key(&folder_key) {
                                continue;
                            }
                            // render_page がパスワード不正時に Err を返すのでそのままスキップ
                            if let Ok((img, _)) = crate::pdf_loader::render_page(
                                pdf_path,
                                0,
                                thumb_px,
                                pw_ref,
                                None,
                                crate::pdf_loader::JobPriority::Normal,
                            ) {
                                if let Some(bytes) = encode_and_save(
                                    &img,
                                    &folder_key,
                                    &catalog,
                                    *pdf_mtime,
                                    *pdf_file_size,
                                    thumb_px,
                                    thumb_quality,
                                ) {
                                    size_atomic.fetch_add(bytes as u64, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                }

                done.fetch_add(1, Ordering::Relaxed);
            }

            finished.store(true, Ordering::Relaxed);
        });
    }
}

// -----------------------------------------------------------------------
// eframe::App 実装
// -----------------------------------------------------------------------

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // メインウィンドウの HWND を最初のフレームで取得 (Win32 ShowWindow 用)。
        // eframe::Frame::window_handle() は raw_window_handle::WindowHandle を返す。
        // Windows では Win32WindowHandle の hwnd フィールドに HWND が入る。
        #[cfg(windows)]
        if self.main_hwnd.is_none() {
            use eframe::wgpu::rwh::{HasWindowHandle, RawWindowHandle};
            if let Ok(wh) = frame.window_handle() {
                if let RawWindowHandle::Win32(h) = wh.as_raw() {
                    let hwnd_raw = h.hwnd.get();
                    self.main_hwnd = Some(hwnd_raw);
                    crate::logger::log(format!("tray: captured main HWND = {hwnd_raw:#x}"));
                    // アクティベーションリスナーに placement_slot を共有するため、
                    // ここでスロットを作成しておく (sync_tray_with_settings での遅延作成と
                    // 競合しないよう、どちらも is_none チェック付き)。
                    if self.placement_slot.is_none() {
                        self.placement_slot = Some(crate::tray::new_placement_slot());
                    }
                    let slot = self.placement_slot.clone().unwrap();
                    // 2 重起動時に既存インスタンスを復帰させるリスナーを起動。
                    // placement_slot もトレイ Open と同じ経路で SetWindowPlacement するため共有。
                    self.activation_listener =
                        crate::single_instance::spawn_activation_listener(
                            hwnd_raw,
                            ctx.clone(),
                            slot,
                            self.shutdown_requested.clone(),
                        );
                }
            }
        }

        // タスクトレイ関連の毎フレーム処理は、設定 ON のときのみ走らせる。
        // 既定 OFF のユーザーには IsWindowVisible の syscall やイベント polling を
        // 走らせないで済むようここで一括 gate する。
        let tray_active =
            self.settings.minimize_to_tray_on_close || self.tray_controller.is_some();
        if tray_active {
            // 実際の Win32 可視状態と App の `window_visible` を毎フレーム同期する。
            // トレイスレッドや 2 重起動アクティベーションリスナーが `ShowWindow` を直接呼ぶ
            // 経路があるので、こちらは flag を追従させる責務を持つ。
            #[cfg(windows)]
            if let Some(hwnd_raw) = self.main_hwnd {
                use windows::Win32::Foundation::HWND;
                use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
                let is_visible_now =
                    unsafe { IsWindowVisible(HWND(hwnd_raw as *mut _)).as_bool() };
                if is_visible_now && !self.window_visible {
                    crate::logger::log(
                        "tray: detected external ShowWindow — running sync_after_restore",
                    );
                    self.sync_after_restore();
                } else if !is_visible_now && self.window_visible {
                    self.window_visible = false;
                }
            }

            // 設定変更反映 + メニューイベントをポーリング + 閉じるボタンの乗っ取り。
            self.sync_tray_with_settings(ctx);
            self.poll_tray_events();
            if self.maybe_intercept_close(ctx) {
                return;
            }
        }

        // フォーカス復帰検出 (Alt+Tab で他アプリから mIV に戻った等) で、
        // 外部 (ComfyUI 等) による current_folder のファイル追加を自動反映する。
        // トレイ復帰は `sync_after_restore` 側で別経路で呼ぶので、tray_active に限らず
        // 全ユーザで動かす。
        let main_focused_now = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        if main_focused_now && !self.last_main_focused && self.window_visible {
            self.check_external_folder_changes();
        }
        self.last_main_focused = main_focused_now;

        // アイドル 5 秒で dirty なサイドカーをフラッシュ (電源断や強制終了への保険)。
        // 頻繁なフレームで呼ばれるが is_dirty 判定で大半は no-op になる。
        self.flush_idle_sidecars();


        // パフォーマンス計装: フレーム境界。--perf-log 無効時は is_enabled() 読みのみ
        self.frame_counter = self.frame_counter.wrapping_add(1);
        if crate::perf::is_enabled() {
            crate::perf::event(
                "frame",
                "begin",
                None,
                self.input_seq,
                &[("n", serde_json::Value::from(self.frame_counter))],
            );
            // 起動時間計測: 最初の update() 呼び出し = winit が初回描画に入った瞬間。
            // `total_ms` に main() 入口からの累計経過を載せる。creator_exit との差分が
            // wgpu パイプライン構築 + winit 初期描画準備の時間になる。
            if self.frame_counter == 1
                && let Some(prog) = crate::perf::program_start()
            {
                crate::perf::event(
                    "startup",
                    "first_frame",
                    None,
                    0,
                    &[(
                        "total_ms",
                        serde_json::Value::from(prog.elapsed().as_secs_f64() * 1000.0),
                    )],
                );
            }
            // 約 1 秒に 1 回 flush (BufWriter のデータをディスクへ)
            let now = std::time::Instant::now();
            let should_flush = self
                .perf_last_flush
                .map(|t| now.duration_since(t).as_millis() >= 1000)
                .unwrap_or(true);
            if should_flush {
                crate::perf::flush();
                self.perf_last_flush = Some(now);
            }
        }

        // ===== 起動オーバーレイ =====
        // IndexerManager の重い初期化はバックグラウンドスレッドで実行する。
        // 進行中は中央に「起動中…」+ 現在ステップを表示し、× ボタン以外の
        // 入力イベントを破棄する。完了したら通常 update に進む。
        self.poll_housekeeping_arm();
        if !self.startup_done {
            self.kick_off_startup_init();
            self.poll_startup_init();
            if self.startup_init.is_some() {
                self.render_startup_overlay(ctx);
                self.consume_input_during_startup(ctx);
                ctx.request_repaint();
                return;
            }
            // Ready に遷移したフレーム: Loading 中に積もった入力イベントが
            // 誤発火しないよう一掃してから通常 update に進む。
            self.consume_input_during_startup(ctx);
        }

        // バージョン更新通知: 起動完了後の初回 + 24h 周期で auto kick。
        self.maybe_auto_update_check();
        self.poll_update_check();
        // worker の結果を取りこぼさないよう、pending 中はアイドルでも repaint を要求。
        // 通常 update() はマウス操作等が無いと走らないため (Codex P2)。
        if self.update_check_pending.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }

        // メインビューポートの IME 状態を更新 (ここで Ime イベントを拾う)。
        // フルスクリーンビューポートは別イベントキューなので render_fullscreen_viewport 内で別途呼ぶ。
        self.update_ime_state(ctx);

        // 入力があればバックグラウンドインデクサを一時停止させる。
        // `keys_down` は「今押されている」セットなので、Ctrl や Shift を指で押しっぱなしに
        // したまま読んでいると毎フレーム true になり、indexer が永久に再開できなくなる
        // (ユーザー報告される前に /simplify で発見、2026-04)。代わりに「今フレームで
        // 発生したイベント」だけを見る。マウス移動・フォーカス変化・Ime 等は除外。
        let has_activity = ctx.input(|i| {
            i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::MouseWheel { .. }
                        | egui::Event::Touch { .. }
                )
            })
        });
        if has_activity {
            self.activity_gate.bump();
        }

        // UI テーマを適用 (変化したときだけ set_visuals を呼ぶ)。
        // `UiTheme::System` 選択時は設定値が変わらなくても Windows の Light/Dark
        // 切替に追従する必要があるので、毎フレーム resolve して解決後の値で比較する。
        let resolved_theme = crate::os_theme::resolve(self.settings.ui_theme);
        if self.applied_ui_theme != Some(resolved_theme) {
            crate::os_theme::apply_resolved(ctx, resolved_theme);
            self.applied_ui_theme = Some(resolved_theme);
        }

        // 初回フレームで前回フォルダを復元
        // ZIP ファイルや、削除済み・取り外し済みのパスでもクラッシュしないよう
        // resolve_openable_path で最も近い既存ディレクトリに解決する。
        if !self.initialized {
            self.initialized = true;

            // テーマは Settings::ui_theme が `System` であれば OS 設定に追従し、
            // 明示的な Light / Dark 選択はそのまま使う。起動ダイアログは出さない
            // (v0.7.0 フィードバック反映で撤去)。

            if let Some(folder) = self.settings.last_folder.clone() {
                if let Some(resolved) = crate::folder_tree::resolve_openable_path(&folder) {
                    self.load_folder(resolved);
                }
            }

            // AI ランタイムを初期化
            self.ensure_ai_runtime();

            // ViewportBuilder 段階では マルチモニタ DPI 混在時に
            // 論理/物理ピクセルの取り違えで異常サイズのウィンドウが
            // 生成されるケースがある (egui#4918 / winit#923)。
            // DPI が確定した初回フレームで意図したサイズを再適用して矯正する。
            if let Some([w, h]) = self.pending_initial_size.take() {
                let ppp = ctx.input(|i| i.pixels_per_point);
                crate::logger::log(format!(
                    "[viewport] deferred InnerSize apply: {w:.0}x{h:.0} ppp={ppp:.2}"
                ));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
            }
        }

        self.track_window_rect(ctx);

        // 毎フレームリセット: 選択セルが描画された時に再設定される
        self.selected_cell_rect = None;

        let frame_t0 = std::time::Instant::now();

        self.poll_thumbnails(ctx);
        // poll_thumbnails の直後にロック解除判定を入れる (= サムネ Loaded が
        // 立った同フレームで holdover を捨てて新画面に切り替える)。
        self.poll_fs_nav_lock();
        let t_poll = frame_t0.elapsed();

        self.update_keep_range_and_requests(ctx, frame_t0);
        let t_keep = frame_t0.elapsed();

        // keep_range が確定した直後に可視範囲分のタグ prewarm を push。
        // スクロールすると新しく入ってきた idx 分が少しずつキューに積まれる。
        self.enqueue_visible_tag_prewarms();

        self.poll_prefetch(ctx);
        self.poll_video(ctx);
        self.poll_ai_upscale(ctx);
        self.poll_erase_inpaint(ctx);
        self.poll_search();
        self.poll_favsearch();
        self.poll_metadata_load();
        self.poll_tag_prewarm_results();
        self.poll_delete_pending();
        self.poll_paste_pending();
        if !self.paste_pending.is_empty() {
            ctx.request_repaint();
        }
        self.poll_cache_maint_pending();
        self.poll_archive_cache_maint_pending();
        self.ensure_folder_rating_counter();
        self.poll_folder_rating_counts();
        // Ctrl+G (docs §10.4): debounce 後に spawn、streaming 受信 → items 更新
        self.poll_global_search_debounce();
        self.poll_global_search_events(ctx);
        // 非同期 pending が走っている間は次フレームも poll させる (egui アイドル寝防止)。
        // tag_prewarm_pending は常駐 handle になったので `is_some()` ではなく
        // 実ジョブ残数 (is_busy) を見る。アイドル時の無限 repaint を避けるため。
        if self.search_pending.is_some()
            || self.favsearch_pending.is_some()
            || self.metadata_pending.is_some()
            || self
                .tag_prewarm_pending
                .as_ref()
                .is_some_and(|p| p.is_busy())
            || (self.folder_rating_counter_handle.is_some()
                && !self.folder_rating_counts_loaded)
        {
            ctx.request_repaint();
        }

        // フルスクリーン表示中なら AI アップスケール + 画像補正を検討
        if let Some(fs_idx) = self.fullscreen_idx {
            // プリセットに基づいてアップスケール設定を同期
            self.sync_upscale_from_preset(fs_idx);

            // 表示中画像を最優先でアップスケール
            self.maybe_start_ai_upscale(fs_idx);
            // 表示中画像のアップスケールが完了 or 不要なら先読みもアップスケール
            let cur_bg = self.effective_upscale_bg_mode();
            let current_done = self.ai_upscale_cache.contains_key(&(fs_idx, cur_bg))
                || self.ai_upscale_failed.contains(&(fs_idx, cur_bg))
                || (!self.ai_upscale_enabled && self.ai_denoise_model.is_none())
                || (self.ai_upscale_enabled
                    && self.ai_denoise_model.is_none()
                    && self
                        .fs_cache
                        .get(&fs_idx)
                        .map(|e| {
                            if let FsCacheEntry::Static { pixels, .. } = e {
                                !crate::ai::upscale::should_process(
                                    pixels.size[0] as u32,
                                    pixels.size[1] as u32,
                                    self.settings.ai_upscale_skip_px,
                                )
                            } else {
                                true
                            }
                        })
                        .unwrap_or(true));
            if current_done && self.ai_upscale_pending.is_empty() {
                self.prefetch_ai_upscale(fs_idx);
            }
            self.evict_ai_upscale_cache(fs_idx);

            // 画像補正の適用（アップスケール後に適用）
            // adjustment_cache がないがパラメータがある場合、フル解像度で補正を適用。
            // 見開き Double 表示中は対のページについても補正適用を走らせる。
            let spread_pair = self.resolve_spread_pair(fs_idx);
            self.maybe_apply_adjustment(ctx, fs_idx, spread_pair);
            if let crate::ui_fullscreen::SpreadPair::Double { left, right } = spread_pair {
                let other = if left == fs_idx { right } else { left };
                self.maybe_apply_adjustment(ctx, other, spread_pair);
            }
            self.evict_adjustment_cache(fs_idx);
        }

        // タイトルバーに現在のフォルダパスを表示する。
        // フォルダ未選択時や読み込み途中はアプリ名のみ。
        // 変換済みアーカイブを開いているときは元 (7z/LZH) のパスを表示する。
        //
        // 名前索引 / メタ索引の supervisor が `in_full_scan=true` を示している間は
        // 「(インデックス更新中)」をサフィックスに付け、検索結果が不完全である
        // 可能性をユーザーに示唆する (notify-rs の watcher 待ちだけなら付けない)。
        let indexing_active = self.any_indexer_in_full_scan();
        let base = match self.effective_folder() {
            Some(p) => format!("{} - mimageviewer", p.display()),
            None => "mimageviewer".to_string(),
        };
        let title = if indexing_active {
            format!("{base}  (インデックス更新中)")
        } else {
            base
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        // スクロールは egui に触れる前に処理（イベントを消費）
        self.process_scroll(ctx);

        // ── フルスクリーン中にメインウィンドウへフォーカスが来たら閉じる ──
        // ボーダーレスウィンドウなので Alt-Tab 等でメインに戻れるが、
        // そのままだと両方のウィンドウがキー入力を無視して操作不能に見える。
        // メインにフォーカスが来た = ユーザーがサムネイル一覧に戻りたい意図と解釈し、
        // フルスクリーンを閉じてメインウィンドウで通常操作を再開する。
        //
        // ただし open_fullscreen() 直後はフルスクリーンビューポートへの
        // ViewportCommand::Focus が反映されるまで数フレームかかるため、
        // 500ms のグレース期間中はチェックをスキップする。
        const FS_FOCUS_GRACE_MS: u128 = 500;
        if self.fullscreen_idx.is_some() {
            if !self.fs_focus_grace_elapsed {
                self.fs_focus_grace_elapsed = self
                    .fs_opened_at
                    .map(|t| t.elapsed().as_millis() > FS_FOCUS_GRACE_MS)
                    .unwrap_or(true);
            }
            let main_has_focus =
                self.fs_focus_grace_elapsed && ctx.input(|i| i.viewport().focused).unwrap_or(false);
            if main_has_focus {
                self.close_fullscreen();
            }
        }

        self.handle_clipboard_shortcuts(ctx);

        let keyboard_nav = self.handle_keyboard(ctx);

        // ── フルスクリーンビューポート ──────────────────────────────────
        // 非アクティブ時も非表示でビューポートを維持（次回表示のちらつき防止）
        self.keep_fullscreen_viewport_alive(ctx);
        self.render_fullscreen_viewport(ctx);

        // 補正パネルでスライダーをドラッグ中に true → release で false の遷移を検知し、
        // サムネ補正テクスチャを全無効化する (次フレームに visible は同期適用、
        // 先読み分は process_thumb_adjust_budget がフレーム分割で埋める)。
        self.update_thumb_adjust_drag_state();

        // ── メニューバー ─────────────────────────────────────────────
        let (fav_nav, _) = self.render_menubar(ctx);

        // ── 進捗バー (左下フローティングオーバーレイ) ────────────────
        self.render_progress_overlay(ctx);

        // タグ書き込み worker の結果ポーリング (docs/tag-feature.md §5.6)
        self.poll_tag_write_results();
        self.poll_rating_write_results();

        // ── ダイアログ群 ─────────────────────────────────────────────
        self.show_favorites_editor_dialog(ctx);
        self.show_tag_editor_dialog(ctx);
        self.show_fav_add_dialog_window(ctx);
        let open_folder_nav = self.show_open_folder_dialog_window(ctx);
        self.show_cache_manager_dialog(ctx);
        self.show_archive_cache_manager_dialog(ctx);
        self.show_cache_creator_dialog(ctx);
        self.show_archive_convert_dialog(ctx);
        self.poll_thumb_quality_pending(ctx);
        self.poll_tq_encode_pending(ctx);
        self.show_thumb_quality_dialog_window(ctx);
        self.show_thumb_quality_fullscreen_overlay(ctx);
        self.show_preferences_dialog(ctx);
        // Phase 3 Step 5: TRT ワーカー関連の通知バナー (起動失敗 / 推論中の死亡)。
        // poll で AiRuntime の通知キューを 1 回引き、show でバナー描画。
        self.poll_trt_worker_notice();
        self.show_trt_worker_notice_dialog(ctx);
        // TRT pack オンラインインストールダイアログ (環境設定から起動)。
        self.show_trt_install_dialog(ctx);
        self.show_stats_dialog_window(ctx);
        self.show_rotation_reset_confirm_dialog(ctx);
        let context_nav = self.show_context_menu(ctx);
        self.show_delete_confirm_dialog(ctx);
        self.show_delete_progress_dialog(ctx);
        self.show_pdf_password_dialog_window(ctx);
        self.show_about_dialog_window(ctx);
        self.show_update_dialog_window(ctx);
        self.show_tray_enabled_notice_dialog(ctx);
        self.poll_pdf_enumerate();
        self.poll_zip_enumerate();
        if self.zip_enumerate_pending.is_some() {
            // 結果到着までフレーム駆動で待つ (worker 完了通知は poll で拾う)
            ctx.request_repaint();
        }

        // ── ツールバー ───────────────────────────────────────────────
        let toolbar_fav_nav = self.render_toolbar(ctx);
        // ツールバーお気に入りクリックは「指定フォルダへ飛ぶ」操作なので、検索系
        // (Ctrl+F フォルダ内ファイル名フィルタ / Ctrl+S お気に入り横断検索 /
        //  Ctrl+G 全文検索) が立っていれば全部抜けてからナビゲートする。
        // close_*() の saved_folder 復帰で旧フォルダへ無駄にロードが走らないよう、
        // 先に saved_folder を捨てておく。
        if toolbar_fav_nav.is_some() {
            if self.global_search.active {
                self.global_search.saved_folder = None;
                self.close_global_search();
            }
            if self.favsearch.active {
                self.favsearch.saved_folder = None;
                self.close_favsearch();
            }
            if self.show_search_bar {
                self.show_search_bar = false;
                self.search_query.clear();
                self.search_filter = None;
                self.search_has_focus = false;
                self.cancel_search_pending();
                self.rebuild_visible_indices();
            }
        }

        // ── アドレスバー ─────────────────────────────────────────────
        let address_nav = self.render_address_bar(ctx);

        // ── Ctrl+F: 検索バー表示 ─────────────────────────────────────
        // Ctrl+G (グローバルメタ検索) と相互排他 (docs §10.3):
        //   - Ctrl+G active 中は Ctrl+F を無効化する (Codex round-8 Must-fix #3)
        //     理由: 結果ビューに SearchContainer が並ぶ状態で Ctrl+F を使うと、
        //     run_metadata_search が SearchContainer を常に一致扱いするためフィルタ不能になる
        //   - Codex P2 #2: `has_focus` だけだと Ctrl+G active でグリッドにフォーカスが
        //     落ちた状態で Ctrl+F が通ってしまう → `active` も条件に入れる
        if !self.address_has_focus
            && self.fullscreen_idx.is_none()
            && !self.any_dialog_open()
            && !self.favsearch.has_focus
            && !self.global_search.has_focus
            && !self.global_search.active
        {
            let ctrl_f = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::F));
            if ctrl_f {
                // 他の検索バーが開いていれば閉じて Ctrl+F に切り替え (相互排他)
                self.open_local_metadata_search();
            }
        }

        // ── Ctrl+A: 表示中の全アイテムをチェック ─────────────────────
        // Ctrl+D / Ctrl+Shift+A: 選択解除 (右クリックメニュー「選択解除」と同等)。
        // Ctrl+D を primary、Ctrl+Shift+A を alias として両方受け付ける。メニュー側の
        // ヘルプ表記は Ctrl+D に統一 (3 キー同時押しより 2 キーの方が提示しやすい)。
        // address/search にフォーカスがあるときはテキスト選択を優先する。
        if !self.address_has_focus
            && !self.search_has_focus
            && !self.favsearch.has_focus
            && self.fullscreen_idx.is_none()
            && !self.any_dialog_open()
        {
            let (ctrl_a, deselect) = ctx.input(|i| {
                let ctrl = i.modifiers.ctrl;
                let shift = i.modifiers.shift;
                let a = i.key_pressed(egui::Key::A);
                let d = i.key_pressed(egui::Key::D);
                // Ctrl+Shift+A は Ctrl+A と同一フレームに見えるので、shift 付きなら
                // Ctrl+A ではなく deselect 側にだけ立てる (全選択の暴発を防ぐ)。
                let select_all = ctrl && !shift && a;
                let deselect = ctrl && (d || (shift && a));
                (select_all, deselect)
            });
            if ctrl_a {
                for &idx in &self.visible_indices {
                    if self.items.get(idx).is_some_and(|it| it.is_checkable()) {
                        self.checked.insert(idx);
                    }
                }
            }
            if deselect {
                self.checked.clear();
            }
        }

        // ── Ctrl+S: お気に入り検索バー表示 ───────────────────────────
        if !self.address_has_focus
            && self.fullscreen_idx.is_none()
            && !self.any_dialog_open()
            && !self.search_has_focus
            && !self.favsearch.has_focus
            && !self.global_search.has_focus
        {
            let ctrl_s = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::S));
            if ctrl_s {
                // 他の検索バーを閉じるのは open_favsearch 内で行う (相互排他)
                self.open_favsearch();
            }
        }

        // ── Ctrl+G: グローバルメタ検索バー表示 (docs §10.3) ──────────
        if !self.address_has_focus
            && self.fullscreen_idx.is_none()
            && !self.any_dialog_open()
            && !self.search_has_focus
            && !self.favsearch.has_focus
        {
            let ctrl_g = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::G));
            if ctrl_g {
                // 相互排他は toggle_global_search → open_global_search 内で行う
                self.toggle_global_search();
            }
        }

        // ── Ctrl+O: フォルダを開く ───────────────────────────────────
        if self.fullscreen_idx.is_none()
            && !self.any_dialog_open()
            && !self.address_has_focus
            && !self.search_has_focus
            && !self.favsearch.has_focus
        {
            let ctrl_o = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::O));
            if ctrl_o {
                self.open_folder_input = self
                    .current_folder
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.show_open_folder_dialog = true;
            }
        }

        // ── Alt+1〜0: 列数切り替え ──────────────────────────────────
        if !self.address_has_focus
            && !self.search_has_focus
            && !self.favsearch.has_focus
            && self.fullscreen_idx.is_none()
            && !self.any_dialog_open()
        {
            let alt_col = ctx.input(|i| {
                if !i.modifiers.alt {
                    return None;
                }
                let keys = [
                    (egui::Key::Num1, 1),
                    (egui::Key::Num2, 2),
                    (egui::Key::Num3, 3),
                    (egui::Key::Num4, 4),
                    (egui::Key::Num5, 5),
                    (egui::Key::Num6, 6),
                    (egui::Key::Num7, 7),
                    (egui::Key::Num8, 8),
                    (egui::Key::Num9, 9),
                    (egui::Key::Num0, 10),
                ];
                keys.iter()
                    .find(|(k, _)| i.key_pressed(*k))
                    .map(|&(_, c)| c)
            });
            if let Some(cols) = alt_col {
                if cols != self.settings.grid_cols {
                    self.settings.grid_cols = cols;
                    self.settings.save();
                }
            }
        }

        // ── 検索バー ─────────────────────────────────────────────────
        self.render_search_bar(ctx);

        // ── お気に入り検索バー (ツールバー直下の 2 行目相当) ─────────
        self.render_favsearch_bar(ctx);

        // ── Ctrl+G グローバルメタ検索バー (docs §10.3) ────────────────
        self.render_global_search_bar(ctx);

        // ── サムネイルグリッド ────────────────────────────────────────
        let t_pre_grid = frame_t0.elapsed();
        let grid_nav = self.render_grid(ctx);
        let t_grid = frame_t0.elapsed();

        // 可視外 keep_range のサムネ補正を背後で逐次適用する。
        // 1 フレーム 8 枚: 600px で ~3ms/枚 = 最大 24ms (半フレーム分の UI 予算)。
        // ドラッグ中はスキップ (process_thumb_adjust_budget 内で判定)。
        self.process_thumb_adjust_budget(ctx, 8);

        // ── 選択情報オーバーレイ ─────────────────────────────────────
        self.render_selection_info(ctx);

        // ── DEL キー ──────────────────────────────────────────────────
        self.handle_delete_key(ctx);

        // ── ペースト後のフォルダ再読み込み ────────────────────────
        if self.pending_reload {
            self.pending_reload = false;
            if self.current_folder.is_some() {
                // 少し遅延してからリロード（ペースト処理の完了を待つ）
                ctx.request_repaint();
                // 変換済みアーカイブ閲覧中は元パス文脈を保持して再読み込みする。
                self.reload_current_folder_preserving_override();
            }
        }

        // ── 非同期フォルダナビゲーションのポーリング ────────────────
        // 優先度 (旧来踏襲): fav_nav > toolbar_fav_nav > keyboard_nav > folder_nav
        //                     > address_nav > open_folder_nav > context_nav > grid_nav
        // folder_nav は fav/toolbar/keyboard より後、address 以下より先。
        let folder_nav_result = self.poll_folder_nav();
        let folder_nav_wins = folder_nav_result.is_some()
            && fav_nav.is_none()
            && toolbar_fav_nav.is_none()
            && keyboard_nav.is_none();

        if folder_nav_wins {
            // folder_nav 勝利: モードに応じて load_folder / close+load+open_fullscreen /
            // favsearch 処理に分岐。address_nav 以下の低優先度 nav は破棄する
            // (旧実装の `.or()` 短絡と同じ挙動)。
            if let Some(result) = folder_nav_result {
                self.apply_folder_nav_result(ctx, result);
                // 累積ステップが残っていれば次の DFS を連鎖起動。
                self.chain_folder_nav_if_pending();
            }
        } else {
            // folder_nav が未完了 or 他の高優先 nav 源が勝ったケース
            let navigate = fav_nav
                .or(toolbar_fav_nav)
                .or(keyboard_nav)
                .or(address_nav)
                .or(open_folder_nav)
                .or(context_nav)
                .or(grid_nav);
            if let Some(p) = navigate {
                // 検索コンテキスト中の前方ナビゲーションはスタックに積む
                // (BS は favsearch_back 経由で navigate には流れないので二重 push にならない)
                if self.favsearch.active {
                    self.favsearch.nav_stack.push(p.clone());
                }
                // Ctrl+G 絞り込みビュー中に container (PDF/ZIP/サブフォルダ) を開いたら
                // current_path を進めておく。BS で「PDF ページ → ヒット一覧 →
                // Aggregated」の 2 段階で戻れるようにする修正 (2026-04 ユーザー報告)。
                // Aggregated / inactive では no-op。
                self.advance_drilled_current_path(&p);
                self.load_folder(p);
                // 他 nav 源が勝った: 累積をクリアして連打バーストを中断する
                // (start_loading_items が folder_nav_pending と累積をリセット済みだが、
                //  folder_nav_result が Some かつ他 nav 優先のケースを拾うため明示)
                self.pending_folder_nav_steps = 0;
                self.pending_folder_nav_mode = FolderNavMode::Grid;
                if self.favsearch.active {
                    self.update_favsearch_address();
                }
            }
        }

        // Pending なサムネイルがある間は毎フレーム再描画をリクエストする。
        // バックグラウンドスレッドがチャネルに送信しても egui は自動では
        // 起きないため、ここで継続的に repaint を要求しておく必要がある。
        if self.folder_nav_pending.is_some()
            || self
                .thumbnails
                .iter()
                .any(|t| matches!(t, ThumbnailState::Pending))
            || self.pdf_enumerate_pending.is_some()
        {
            ctx.request_repaint();
        }

        // フレーム計測: 8 ms (≈120 fps) 超えた場合のみログに出力
        let frame_total = frame_t0.elapsed();
        if frame_total.as_millis() > 8 {
            crate::logger::log(format!(
                "  [SLOW FRAME] {:.1}ms  poll={:.1}ms keep={:.1}ms pre_grid={:.1}ms grid={:.1}ms  backlog={} requested={}",
                frame_total.as_secs_f64() * 1000.0,
                t_poll.as_secs_f64() * 1000.0,
                (t_keep - t_poll).as_secs_f64() * 1000.0,
                (t_pre_grid - t_keep).as_secs_f64() * 1000.0,
                (t_grid - t_pre_grid).as_secs_f64() * 1000.0,
                self.texture_backlog.len(),
                self.requested.len(),
            ));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.persist_window_state_and_flush();
    }
}

// -----------------------------------------------------------------------
// セル描画
// -----------------------------------------------------------------------

/// 先読み対象を距離順・forward 先で交互配置: +1, -1, +2, -2, +3, -3, …
/// 同距離の組では forward (次ページ方向) が先。片側が尽きたら反対側だけ続く。
/// fs_cache / AI アップスケール / サムネイルグリッド の全先読みで方針統一。
fn interleaved_prefetch_targets(
    image_indices: &[usize],
    pos: usize,
    n: usize,
    pf_forward: usize,
    pf_back: usize,
) -> Vec<usize> {
    let max_d = pf_forward.max(pf_back);
    let mut out = Vec::with_capacity(pf_forward + pf_back);
    for d in 1..=max_d {
        if d <= pf_forward {
            if let Some(p) = pos.checked_add(d) {
                if p < n {
                    out.push(image_indices[p]);
                }
            }
        }
        if d <= pf_back {
            if let Some(p) = pos.checked_sub(d) {
                out.push(image_indices[p]);
            }
        }
    }
    out
}

/// GridItem から LoadRequest を構築する。画像 / ZIP 内画像 / PDF ページ / フォルダ以外は None を返す。
fn make_load_request(
    item: &GridItem,
    idx: usize,
    mtime: i64,
    file_size: i64,
    skip_cache: bool,
    pdf_password: Option<&str>,
    folder_thumb_sort: Option<crate::settings::SortOrder>,
    folder_thumb_depth: u32,
) -> Option<LoadRequest> {
    // 共通フィールド (idx/path/mtime/file_size/skip_cache) 以外は Default (0/None/false)
    // を基底にして差分だけ上書きする。入力 seq / items_gen は後段のエンキューで上書きされる。
    let base = LoadRequest {
        idx,
        mtime,
        file_size,
        skip_cache,
        ..Default::default()
    };
    match item {
        GridItem::Image(p) => Some(LoadRequest {
            path: p.clone(),
            ..base
        }),
        GridItem::ZipImage {
            zip_path,
            entry_name,
        } => Some(LoadRequest {
            path: zip_path.clone(),
            zip_entry: Some(entry_name.clone()),
            ..base
        }),
        GridItem::PdfPage {
            pdf_path, page_num, ..
        } => Some(LoadRequest {
            path: pdf_path.clone(),
            pdf_page: Some(*page_num),
            pdf_password: pdf_password.map(String::from),
            ..base
        }),
        GridItem::ZipFile(p) => {
            // zip_entry は None のままにしておき、ワーカー側でキャッシュミス時に
            // 遅延解決する (UI スレッドで ZIP を開く I/O を避けるため)。
            let fname = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            Some(LoadRequest {
                path: p.clone(),
                cache_key_override: Some(format!("{}{fname}", CACHE_KEY_ZIP)),
                ..base
            })
        }
        GridItem::PdfFile(p) => {
            let fname = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            Some(LoadRequest {
                path: p.clone(),
                pdf_page: Some(0),
                pdf_password: pdf_password.map(String::from),
                cache_key_override: Some(format!("{}{fname}", CACHE_KEY_PDF)),
                ..base
            })
        }
        GridItem::Folder(p) => {
            let fname = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            Some(LoadRequest {
                path: p.clone(),
                cache_key_override: Some(format!("{}{fname}", CACHE_KEY_FOLDER)),
                folder_thumb_sort,
                folder_thumb_depth,
                ..base
            })
        }
        GridItem::SearchContainer {
            representative: Some(rep),
            ..
        } => {
            // 代表サムネのキャッシュキー (Codex P1 対応):
            // - filename 単体を使うと別コンテナの同名画像 (例: 複数フォルダの `cover.jpg`)
            //   で衝突し、placeholder mtime=0 の entry で先に書かれた thumb を読んでしまう。
            // - `{CACHE_KEY_SEARCH_REP}{full_path}[::{zip_entry|#page}]` をキーにしてユニーク化。
            //   通常閲覧のキャッシュ (filename 単体キー) とは別空間なので互いを壊さない。
            let path_str = rep.path.to_string_lossy();
            let key = match (&rep.zip_entry, rep.pdf_page) {
                (Some(entry), _) => format!("{}{}::{}", CACHE_KEY_SEARCH_REP, path_str, entry),
                (None, Some(page)) => {
                    format!("{}{}#p{}", CACHE_KEY_SEARCH_REP, path_str, page)
                }
                (None, None) => format!("{}{}", CACHE_KEY_SEARCH_REP, path_str),
            };
            Some(LoadRequest {
                path: rep.path.clone(),
                zip_entry: rep.zip_entry.clone(),
                pdf_page: rep.pdf_page,
                pdf_password: pdf_password.map(String::from),
                cache_key_override: Some(key),
                ..base
            })
        }
        _ => None,
    }
}

/// サムネイルテクスチャをアスペクト保持で中央配置して描画する（回転対応）。
fn draw_thumb_texture(
    painter: &egui::Painter,
    inner: egui::Rect,
    tex: &egui::TextureHandle,
    rotation: crate::rotation_db::Rotation,
) {
    let tex_size = tex.size_vec2();
    // 90°/270° 回転時は幅と高さが入れ替わる
    let display_size = match rotation {
        crate::rotation_db::Rotation::Cw90 | crate::rotation_db::Rotation::Cw270 => {
            egui::vec2(tex_size.y, tex_size.x)
        }
        _ => tex_size,
    };
    let scale = (inner.width() / display_size.x).min(inner.height() / display_size.y);
    let img_rect = egui::Rect::from_center_size(inner.center(), display_size * scale);

    // 透過画像の背景はフルスクリーンと同じ黒に揃える (v0.7.0 フィードバック反映)。
    // セル全体ではなく img_rect (実際に画像が描かれる領域) だけを塗るので、
    // フォルダラベルや letterbox の白背景は維持される。
    painter.rect_filled(img_rect, 0.0, egui::Color32::BLACK);

    if rotation.is_none() {
        painter.image(
            tex.id(),
            img_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        // 回転したテクスチャを Mesh で描画
        draw_rotated_image(painter, tex.id(), img_rect, rotation);
    }
}

/// DynamicImage を egui::ColorImage に変換する (リサイズなし)。
/// フルスクリーン表示や PDF 再レンダリング結果の変換で使用。
pub(crate) fn dynamic_image_to_color_image(img: &image::DynamicImage) -> egui::ColorImage {
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

/// wgpu テクスチャの最大次元。wgpu デフォルト制限は 8192px。
/// GPU 実機はもっと大きいが (RTX 4090 = 16384)、eframe が
/// デフォルト Limits で初期化するため 8192 を超えるとパニックする。
pub(crate) const MAX_TEXTURE_DIM: usize = 8192;

/// GPU テクスチャ上限を超える `ColorImage` を縮小して返す。
/// 上限内であればクローンせず共有参照をそのまま `Cow::Borrowed` で返す。
///
/// UI スレッドからの呼び出し時に上限超過を検知するとここで
/// `resize_exact(Triangle)` が走り、7K-9K クラスの画像で 5 秒単位の同期ハングになる。
/// フルスクリーン静止画の主経路 (start_fs_load) は worker 側で先に
/// `clamp_dynamic_for_gpu` を掛けているので、ここは通常 `Cow::Borrowed` のみを
/// 辿る前提になっている。異常経路の安全網として残してある。
pub(crate) fn clamp_for_gpu(ci: &egui::ColorImage) -> std::borrow::Cow<'_, egui::ColorImage> {
    let [w, h] = ci.size;
    if w <= MAX_TEXTURE_DIM && h <= MAX_TEXTURE_DIM {
        return std::borrow::Cow::Borrowed(ci);
    }
    // 長辺を MAX_TEXTURE_DIM に収めるスケール
    let scale = MAX_TEXTURE_DIM as f64 / w.max(h) as f64;
    let new_w = ((w as f64 * scale).round() as u32).max(1);
    let new_h = ((h as f64 * scale).round() as u32).max(1);
    let t0 = std::time::Instant::now();
    let dynimg = color_image_to_dynamic(ci);
    let resized = dynimg.resize_exact(new_w, new_h, image::imageops::FilterType::Triangle);
    crate::logger::log(format!(
        "  clamp_for_gpu (UI-thread fallback): {w}x{h} → {new_w}x{new_h} (limit {MAX_TEXTURE_DIM}) in {:.0}ms",
        t0.elapsed().as_secs_f64() * 1000.0
    ));
    std::borrow::Cow::Owned(dynamic_image_to_color_image(&resized))
}

/// worker スレッド向け: GPU 上限を超える `DynamicImage` を Bilinear リサイズで縮小する。
/// `fast_image_resize` (AVX2/SSE4.1 SIMD) 実装を使うので、image crate の
/// スカラー `resize_exact(Triangle)` に比べて 7K-9K クラスで 5-10 倍速い。
/// 上限内なら入力をそのまま返す (クローンなし)。
pub(crate) fn clamp_dynamic_for_gpu(img: image::DynamicImage) -> image::DynamicImage {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w <= MAX_TEXTURE_DIM && h <= MAX_TEXTURE_DIM {
        return img;
    }
    let scale = MAX_TEXTURE_DIM as f64 / w.max(h) as f64;
    let new_w = ((w as f64 * scale).round() as u32).max(1);
    let new_h = ((h as f64 * scale).round() as u32).max(1);
    let t0 = std::time::Instant::now();
    let resized = crate::fast_resize::resize_dynamic_exact(
        &img,
        new_w,
        new_h,
        crate::fast_resize::Quality::Bilinear,
    );
    crate::logger::log(format!(
        "  clamp_dynamic_for_gpu: {w}x{h} → {new_w}x{new_h} (limit {MAX_TEXTURE_DIM}) in {:.0}ms",
        t0.elapsed().as_secs_f64() * 1000.0
    ));
    resized
}

/// 回転した画像を Mesh で描画する。
/// `free_rotation_rad` が非ゼロの場合、`center` 基準で頂点を任意角度回転する。
pub(crate) fn draw_rotated_image_ex(
    painter: &egui::Painter,
    texture_id: egui::TextureId,
    rect: egui::Rect,
    rotation: crate::rotation_db::Rotation,
    free_rotation_rad: f32,
    center: egui::Pos2,
) {
    // UV 座標を回転に合わせて変換
    // 頂点順: 左上, 右上, 右下, 左下 (画面座標)
    let uvs = match rotation {
        crate::rotation_db::Rotation::None => [
            egui::pos2(0.0, 0.0),
            egui::pos2(1.0, 0.0),
            egui::pos2(1.0, 1.0),
            egui::pos2(0.0, 1.0),
        ],
        crate::rotation_db::Rotation::Cw90 => [
            egui::pos2(0.0, 1.0),
            egui::pos2(0.0, 0.0),
            egui::pos2(1.0, 0.0),
            egui::pos2(1.0, 1.0),
        ],
        crate::rotation_db::Rotation::Cw180 => [
            egui::pos2(1.0, 1.0),
            egui::pos2(0.0, 1.0),
            egui::pos2(0.0, 0.0),
            egui::pos2(1.0, 0.0),
        ],
        crate::rotation_db::Rotation::Cw270 => [
            egui::pos2(1.0, 0.0),
            egui::pos2(1.0, 1.0),
            egui::pos2(0.0, 1.0),
            egui::pos2(0.0, 0.0),
        ],
    };

    let mut positions = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];

    // 任意角度回転: 頂点を center 基準で回転
    if free_rotation_rad.abs() > 0.001 {
        let cos_r = free_rotation_rad.cos();
        let sin_r = free_rotation_rad.sin();
        for p in &mut positions {
            let dx = p.x - center.x;
            let dy = p.y - center.y;
            p.x = center.x + dx * cos_r - dy * sin_r;
            p.y = center.y + dx * sin_r + dy * cos_r;
        }
    }

    let mut mesh = egui::Mesh::with_texture(texture_id);
    for i in 0..4 {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: positions[i],
            uv: uvs[i],
            color: egui::Color32::WHITE,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(egui::Shape::mesh(mesh));
}

/// 回転した画像を Mesh で描画する（90° 単位のみ）。
pub(crate) fn draw_rotated_image(
    painter: &egui::Painter,
    texture_id: egui::TextureId,
    rect: egui::Rect,
    rotation: crate::rotation_db::Rotation,
) {
    draw_rotated_image_ex(painter, texture_id, rect, rotation, 0.0, rect.center());
}

/// 画像系アイテム (Image / ZipImage) のサムネイル状態に応じた描画。
fn draw_thumb(
    painter: &egui::Painter,
    inner: egui::Rect,
    thumb: &ThumbnailState,
    rotation: crate::rotation_db::Rotation,
    dark: bool,
    adjusted_tex: Option<&egui::TextureHandle>,
) {
    match thumb {
        ThumbnailState::Loaded { tex, .. } => {
            let use_tex = adjusted_tex.unwrap_or(tex);
            draw_thumb_texture(painter, inner, use_tex, rotation);
        }
        ThumbnailState::Pending | ThumbnailState::Evicted => {
            let bg = if dark {
                egui::Color32::from_gray(50)
            } else {
                egui::Color32::from_gray(220)
            };
            painter.rect_filled(inner, 2.0, bg);
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                "読込中",
                egui::FontId::proportional(12.0),
                egui::Color32::from_gray(140),
            );
        }
        ThumbnailState::Failed => {
            let bg = if dark {
                egui::Color32::from_rgb(80, 30, 30)
            } else {
                egui::Color32::from_rgb(255, 220, 220)
            };
            let fg = if dark {
                egui::Color32::from_rgb(255, 160, 160)
            } else {
                egui::Color32::DARK_RED
            };
            painter.rect_filled(inner, 2.0, bg);
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                "読込失敗",
                egui::FontId::proportional(12.0),
                fg,
            );
        }
    }
}

pub(crate) fn draw_cell(
    ui: &egui::Ui,
    rect: egui::Rect,
    is_selected: bool,
    is_checked: bool,
    has_page_override: bool, // true なら左上に補正済みバッジ「補」を表示
    has_mask: bool,          // true なら左上に消しゴムマスクバッジ「消」を表示
    rating: u8,              // 0 = 非表示, 1-5 = ★バッジ
    item: &GridItem,
    thumb: &ThumbnailState,
    rotation: crate::rotation_db::Rotation,
    // Some(tex) なら `ThumbnailState::Loaded.tex` の代わりにこちらを描画する
    // (色調補正済みサムネイルテクスチャ)。None または Loaded 以外なら生サムネ。
    adjusted_tex: Option<&egui::TextureHandle>,
    // 画像セルに表示する XMP dc:subject 由来のタグ (`#原神` 等)。空なら非表示。
    tags: &[String],
    // コンテナセルに出す「フィルタ一致の子孫件数」。None ならバッジ非表示。
    filter_match_count: Option<u32>,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter();
    let padding = 4.0;
    let inner = rect.shrink(padding);

    let dark = ui.visuals().dark_mode;
    let name_text_color = if dark {
        egui::Color32::from_gray(210)
    } else {
        egui::Color32::from_gray(30)
    };
    let pending_placeholder_bg = if dark {
        egui::Color32::from_gray(50)
    } else {
        egui::Color32::from_gray(230)
    };

    let bg = if is_selected {
        if dark {
            egui::Color32::from_rgb(40, 70, 110)
        } else {
            egui::Color32::from_rgb(180, 210, 255)
        }
    } else if dark {
        egui::Color32::from_gray(28)
    } else {
        egui::Color32::WHITE
    };
    painter.rect_filled(rect, 2.0, bg);

    match item {
        GridItem::Folder(path) => {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            match thumb {
                ThumbnailState::Loaded { tex, .. } => {
                    let use_tex = adjusted_tex.unwrap_or(tex);
                    draw_thumb_texture(painter, inner, use_tex, rotation);
                    draw_folder_badge(painter, inner, name);
                }
                ThumbnailState::Pending | ThumbnailState::Evicted | ThumbnailState::Failed => {
                    painter.text(
                        inner.center() - egui::vec2(0.0, 14.0),
                        egui::Align2::CENTER_CENTER,
                        "📁",
                        egui::FontId::proportional(42.0),
                        egui::Color32::from_rgb(220, 170, 30),
                    );
                    painter.text(
                        egui::pos2(inner.center().x, inner.max.y - 4.0),
                        egui::Align2::CENTER_BOTTOM,
                        truncate_name(name, 18),
                        egui::FontId::proportional(11.0),
                        name_text_color,
                    );
                }
            }
        }
        GridItem::Image(_) => {
            draw_thumb(painter, inner, thumb, rotation, dark, adjusted_tex);
        }
        GridItem::Video(path) => {
            match thumb {
                ThumbnailState::Loaded { tex, .. } => {
                    // 動画サムネは補正対象外 (adjusted_tex は常に None)
                    draw_thumb_texture(painter, inner, tex, rotation);
                }
                ThumbnailState::Pending | ThumbnailState::Evicted => {
                    painter.rect_filled(inner, 2.0, egui::Color32::from_gray(40));
                    painter.text(
                        inner.center(),
                        egui::Align2::CENTER_CENTER,
                        "動画",
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_gray(160),
                    );
                }
                ThumbnailState::Failed => {
                    painter.rect_filled(inner, 2.0, egui::Color32::from_gray(40));
                }
            }
            // 再生ボタンオーバーレイ（常時表示）
            let r = (inner.width().min(inner.height()) * 0.18).max(10.0);
            draw_play_icon(painter, inner.center(), r);
            // ファイル名
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            painter.text(
                egui::pos2(inner.center().x, inner.max.y - 4.0),
                egui::Align2::CENTER_BOTTOM,
                truncate_name(name, 18),
                egui::FontId::proportional(11.0),
                name_text_color,
            );
        }
        GridItem::ZipImage { .. } | GridItem::PdfPage { .. } => {
            draw_thumb(painter, inner, thumb, rotation, dark, adjusted_tex);
        }
        GridItem::ZipFile(path) | GridItem::PdfFile(path) => {
            let (icon, badge_fn): (&str, fn(&egui::Painter, egui::Rect)) =
                if matches!(item, GridItem::ZipFile(_)) {
                    ("📦", draw_zip_badge)
                } else {
                    ("📄", draw_pdf_badge)
                };
            match thumb {
                ThumbnailState::Loaded { tex, .. } => {
                    // ZipFile/PdfFile の代表サムネは補正対象外 (adjusted_tex は常に None)
                    draw_thumb_texture(painter, inner, tex, rotation);
                }
                ThumbnailState::Pending | ThumbnailState::Evicted | ThumbnailState::Failed => {
                    painter.rect_filled(inner, 2.0, pending_placeholder_bg);
                    painter.text(
                        inner.center(),
                        egui::Align2::CENTER_CENTER,
                        icon,
                        egui::FontId::proportional(32.0),
                        egui::Color32::from_gray(120),
                    );
                }
            }
            badge_fn(painter, inner);
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            painter.text(
                egui::pos2(inner.center().x, inner.max.y - 4.0),
                egui::Align2::CENTER_BOTTOM,
                truncate_name(name, 18),
                egui::FontId::proportional(11.0),
                name_text_color,
            );
        }
        GridItem::ConvertibleArchive { path, format } => {
            // 7z / LZH: クリック時に ZIP 変換→閲覧のフロー。サムネイルなしで
            // 汎用アーカイブアイコン + 形式バッジで表示する。
            painter.rect_filled(inner, 2.0, pending_placeholder_bg);
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                "🗜",
                egui::FontId::proportional(32.0),
                egui::Color32::from_gray(120),
            );
            crate::ui_helpers::draw_archive_badge(painter, inner, format.label());
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            painter.text(
                egui::pos2(inner.center().x, inner.max.y - 4.0),
                egui::Align2::CENTER_BOTTOM,
                truncate_name(name, 18),
                egui::FontId::proportional(11.0),
                name_text_color,
            );
        }
        GridItem::ZipSeparator { dir_display } => {
            // 作品境界のセパレータ: 1 セル全体に目立つ背景 + フォルダ名
            let (sep_bg, sep_stroke, sep_title, sep_small) = if dark {
                (
                    egui::Color32::from_rgb(35, 55, 85),
                    egui::Color32::from_rgb(100, 140, 200),
                    egui::Color32::from_rgb(200, 220, 250),
                    egui::Color32::from_gray(180),
                )
            } else {
                (
                    egui::Color32::from_rgb(235, 242, 252),
                    egui::Color32::from_rgb(120, 160, 220),
                    egui::Color32::from_rgb(40, 70, 140),
                    egui::Color32::from_gray(100),
                )
            };
            painter.rect_filled(inner, 6.0, sep_bg);
            painter.rect_stroke(
                inner,
                6.0,
                egui::Stroke::new(2.0, sep_stroke),
                egui::StrokeKind::Middle,
            );
            // 下部 "📁 作品の区切り" 用の予約幅 (タイトル領域算出にも使うため先に確定)
            let small = (inner.height() * 0.08).clamp(9.0, 16.0);
            let bottom_reserve = small * 1.4 + 6.0;
            // 階層フォルダは A/B ではなく `\n` 区切りで複数行に分け、Ctrl+G 検索結果と
            // 同じ自動縮小ロジック (draw_path_hierarchy) でセル幅にフィットさせる。
            const TOP_PAD: f32 = 6.0;
            const SIDE_PAD: f32 = 6.0;
            const MIN_TITLE_H: f32 = 14.0;
            let title_top = inner.min.y + TOP_PAD;
            let title_bottom_full = inner.max.y - bottom_reserve;
            let title_bottom = if title_bottom_full - title_top >= MIN_TITLE_H {
                title_bottom_full
            } else if (inner.max.y - TOP_PAD) - title_top >= MIN_TITLE_H {
                // 極小セルでは下部ラベル予約を諦める
                inner.max.y - TOP_PAD
            } else {
                inner.max.y
            };
            let title_rect = egui::Rect::from_min_max(
                egui::pos2(inner.min.x + SIDE_PAD, title_top),
                egui::pos2(inner.max.x - SIDE_PAD, title_bottom),
            );
            let components = crate::ui_helpers::split_path_components(dir_display);
            let max_font = (inner.height() * 0.14).clamp(14.0, 36.0);
            let min_font = 10.0;
            crate::ui_helpers::draw_path_hierarchy(
                painter,
                title_rect,
                &components,
                sep_title,
                max_font,
                min_font,
            );
            // 下部にフォルダアイコン的な記号
            painter.text(
                egui::pos2(inner.center().x, inner.max.y - 6.0),
                egui::Align2::CENTER_BOTTOM,
                "📁  作品の区切り",
                egui::FontId::proportional(small),
                sep_small,
            );
        }
        GridItem::SearchContainer {
            path,
            kind,
            hit_count,
            representative,
        } => {
            let (icon, label_color) = match kind {
                crate::grid_item::SearchContainerKind::Folder => (
                    "📁",
                    if dark {
                        egui::Color32::from_gray(220)
                    } else {
                        egui::Color32::from_gray(60)
                    },
                ),
                crate::grid_item::SearchContainerKind::Zip => (
                    "📦",
                    if dark {
                        egui::Color32::from_rgb(220, 200, 150)
                    } else {
                        egui::Color32::from_rgb(130, 90, 30)
                    },
                ),
            };

            // 代表サムネがあって GPU テクスチャがロード済みなら、セル上部にサムネ、
            // 下部に少し背景色の付いたボックスで「フォルダ階層 + ヒット件数」を出す。
            // 未ロード / 代表サムネなしのときは従来どおりアイコン + 階層パスで埋める
            // (サムネが読み込まれるまでの placeholder)。
            let thumb_loaded = representative.is_some()
                && matches!(thumb, ThumbnailState::Loaded { .. });

            if thumb_loaded {
                let thumb_h = inner.height() * 0.62;
                let thumb_rect = egui::Rect::from_min_max(
                    inner.min,
                    egui::pos2(inner.max.x, inner.min.y + thumb_h),
                );
                if let ThumbnailState::Loaded { tex, .. } = thumb {
                    // 代表サムネは色調補正対象外 (adjusted_tex は常に None)
                    draw_thumb_texture(painter, thumb_rect, tex, rotation);
                }
                // 種別アイコン (小) を左上隅に重ねて Folder/ZIP を示す
                let badge_size = (thumb_rect.height() * 0.22).clamp(14.0, 28.0);
                painter.text(
                    egui::pos2(thumb_rect.min.x + 4.0, thumb_rect.min.y + 4.0),
                    egui::Align2::LEFT_TOP,
                    icon,
                    egui::FontId::proportional(badge_size),
                    label_color,
                );

                // 下部の「少し背景色を付けたボックス」: ユーザー要望どおりフォルダ名を
                // サムネから切り離して読みやすくする。
                let label_rect = egui::Rect::from_min_max(
                    egui::pos2(inner.min.x, thumb_rect.max.y + 2.0),
                    inner.max,
                );
                let label_bg = if dark {
                    egui::Color32::from_rgb(38, 42, 50)
                } else {
                    egui::Color32::from_rgb(240, 240, 246)
                };
                painter.rect_filled(label_rect, 3.0, label_bg);

                let badge_font = (label_rect.height() * 0.19).clamp(10.0, 14.0);
                let text_rect = egui::Rect::from_min_max(
                    egui::pos2(label_rect.min.x + 4.0, label_rect.min.y + 2.0),
                    egui::pos2(label_rect.max.x - 4.0, label_rect.max.y - badge_font * 1.3),
                );
                let path_str = path.to_string_lossy();
                let components = crate::ui_helpers::split_path_components(&path_str);
                let max_font = (label_rect.height() * 0.24).clamp(10.0, 13.0);
                crate::ui_helpers::draw_path_hierarchy(
                    painter,
                    text_rect,
                    &components,
                    label_color,
                    max_font,
                    5.0,
                );
                let badge_text = format!("{} 枚", hit_count);
                let badge_color = if dark {
                    egui::Color32::from_rgb(240, 200, 100)
                } else {
                    egui::Color32::from_rgb(180, 80, 0)
                };
                painter.text(
                    egui::pos2(label_rect.max.x - 6.0, label_rect.max.y - 4.0),
                    egui::Align2::RIGHT_BOTTOM,
                    &badge_text,
                    egui::FontId::proportional(badge_font),
                    badge_color,
                );
            } else {
                // 代表サムネなし or 未ロード: 従来どおりアイコン + 階層パス + バッジ
                // (日付フォルダ `2025-01-01` 等を単独で識別できるよう階層を多行表示)
                let icon_size = (inner.height() * 0.18).clamp(22.0, 56.0);
                painter.text(
                    egui::pos2(inner.center().x, inner.min.y + icon_size * 0.75),
                    egui::Align2::CENTER_CENTER,
                    icon,
                    egui::FontId::proportional(icon_size),
                    label_color,
                );
                let badge_font = (inner.height() * 0.07).clamp(10.0, 14.0);
                let text_rect = egui::Rect::from_min_max(
                    egui::pos2(inner.min.x + 4.0, inner.min.y + icon_size * 1.35),
                    egui::pos2(inner.max.x - 4.0, inner.max.y - badge_font * 2.2),
                );
                let path_str = path.to_string_lossy();
                let components = crate::ui_helpers::split_path_components(&path_str);
                let max_font = (inner.height() * 0.075).clamp(11.0, 15.0);
                crate::ui_helpers::draw_path_hierarchy(
                    painter,
                    text_rect,
                    &components,
                    label_color,
                    max_font,
                    8.0,
                );
                let badge_text = format!("{} 枚", hit_count);
                let badge_color = if dark {
                    egui::Color32::from_rgb(240, 200, 100)
                } else {
                    egui::Color32::from_rgb(180, 80, 0)
                };
                painter.text(
                    egui::pos2(inner.max.x - 6.0, inner.max.y - 6.0),
                    egui::Align2::RIGHT_BOTTOM,
                    &badge_text,
                    egui::FontId::proportional(badge_font),
                    badge_color,
                );
            }
        }
    }

    let border = if is_selected {
        egui::Stroke::new(2.0, egui::Color32::from_rgb(60, 120, 220))
    } else {
        egui::Stroke::new(
            1.0,
            if dark {
                egui::Color32::from_gray(70)
            } else {
                egui::Color32::from_gray(200)
            },
        )
    };
    painter.rect_stroke(rect, 2.0, border, egui::StrokeKind::Middle);

    // チェックマークオーバーレイ
    if is_checked {
        let check_r = 12.0;
        let check_center = egui::pos2(rect.max.x - check_r - 4.0, rect.min.y + check_r + 4.0);
        painter.circle_filled(check_center, check_r, egui::Color32::from_rgb(40, 140, 40));
        // チェックマーク (✓)
        let s = check_r * 0.55;
        let stroke = egui::Stroke::new(2.5, egui::Color32::WHITE);
        painter.line_segment(
            [
                egui::pos2(check_center.x - s * 0.6, check_center.y),
                egui::pos2(check_center.x - s * 0.1, check_center.y + s * 0.5),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(check_center.x - s * 0.1, check_center.y + s * 0.5),
                egui::pos2(check_center.x + s * 0.7, check_center.y - s * 0.5),
            ],
            stroke,
        );
    }

    // 左上バッジ列: 補 (ページ個別補正) → 消 (消しゴムマスク) → タグバッジ。
    // 横並びで、収まらなければ末尾省略。
    {
        let badge_w = 18.0;
        let badge_h = 16.0;
        let mut x = rect.min.x + 3.0;
        let y = rect.min.y + 3.0;
        if has_page_override {
            let badge_rect =
                egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(badge_w, badge_h));
            painter.rect_filled(badge_rect, 3.0, egui::Color32::from_rgb(50, 120, 220));
            painter.text(
                badge_rect.center(),
                egui::Align2::CENTER_CENTER,
                "補",
                egui::FontId::proportional(11.0),
                egui::Color32::WHITE,
            );
            x += badge_w + 2.0;
        }
        if has_mask {
            let badge_rect =
                egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(badge_w, badge_h));
            painter.rect_filled(badge_rect, 3.0, egui::Color32::from_rgb(200, 80, 40));
            painter.text(
                badge_rect.center(),
                egui::Align2::CENTER_CENTER,
                "消",
                egui::FontId::proportional(11.0),
                egui::Color32::WHITE,
            );
            x += badge_w + 2.0;
        }
        if !tags.is_empty() {
            // 残り幅 (右端 - 現在 x - 余白) に収まるだけ並べる。チェックマーク領域 (右上 24px)
            // と被らないように max_x を絞る。
            let max_x = rect.max.x - 28.0;
            draw_tag_badges(painter, egui::pos2(x, y), max_x, tags);
        }
    }

    // レーティングバッジ（1-5 ★、左下に半透明の背景付きで表示）
    // 画像系 (Image / ZipImage / PdfPage): 金色の ★
    // コンテナ系 (Folder / ZipFile / PdfFile): 銀青色の ★ + 先頭に 📁 アイコンを付与して
    //   「コンテナ自体への評価」であることを一目で区別できるようにする。
    if rating >= 1 && rating <= 5 {
        let is_container = item.is_container_ratable();
        let star_color = if is_container {
            egui::Color32::from_rgb(180, 220, 255)
        } else {
            egui::Color32::from_rgb(255, 215, 50)
        };
        let text = if is_container {
            format!("📁{}", "★".repeat(rating as usize))
        } else {
            "★".repeat(rating as usize)
        };
        let font = egui::FontId::proportional(12.0);
        // 背景矩形のサイズを見積もる (コンテナは 📁 ぶん ~14px 広くする)
        let prefix_w = if is_container { 14.0 } else { 0.0 };
        let text_w = 10.5 * rating as f32 + 6.0 + prefix_w;
        let text_h = 16.0;
        let bg_rect = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 3.0, rect.max.y - text_h - 3.0),
            egui::vec2(text_w, text_h),
        );
        painter.rect_filled(
            bg_rect,
            3.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 150),
        );
        painter.text(
            bg_rect.left_center() + egui::vec2(3.0, 0.0),
            egui::Align2::LEFT_CENTER,
            text,
            font,
            star_color,
        );
    }

    if let Some(count) = filter_match_count {
        if item.is_container_ratable() && count > 0 {
            draw_filter_match_badge(painter, rect, count);
        }
    }

}

/// resume 対象とみなす最小 position (秒)。これ未満は「最初から」扱い。
pub(crate) const VIDEO_RESUME_MIN_POSITION_SECS: f64 = 3.0;
/// 動画末端からこの秒数以下に位置していたら完走扱いで削除する。
pub(crate) const VIDEO_RESUME_END_GUARD_SECS: f64 = 5.0;

/// 動画再生位置を `Settings::video_resume_positions` に保存する。
/// - position が `VIDEO_RESUME_MIN_POSITION_SECS` 未満 → エントリ削除 (最初から)
/// - 残り `VIDEO_RESUME_END_GUARD_SECS` 以下 → エントリ削除 (完走済み)
/// - それ以外 → position を保存
fn save_video_resume_position(
    map: &mut std::collections::HashMap<String, f64>,
    key: String,
    position: f64,
    duration: f64,
) {
    if position < VIDEO_RESUME_MIN_POSITION_SECS {
        map.remove(&key);
        return;
    }
    if duration > 0.0 && position >= duration - VIDEO_RESUME_END_GUARD_SECS {
        map.remove(&key);
        return;
    }
    map.insert(key, position);
}

fn draw_filter_match_badge(painter: &egui::Painter, cell_rect: egui::Rect, count: u32) {
    let text = if count >= 1000 {
        "999+".to_string()
    } else {
        count.to_string()
    };
    let font = egui::FontId::proportional(11.0);
    let galley = painter.layout_no_wrap(text, font, egui::Color32::WHITE);
    let pad_x = 5.0;
    let pad_y = 2.0;
    let bg_w = galley.size().x + pad_x * 2.0;
    let bg_h = galley.size().y + pad_y * 2.0;
    let bg_rect = egui::Rect::from_min_size(
        egui::pos2(cell_rect.max.x - bg_w - 3.0, cell_rect.max.y - bg_h - 3.0),
        egui::vec2(bg_w, bg_h),
    );
    painter.rect_filled(bg_rect, 3.0, egui::Color32::from_rgb(0xE6, 0x7E, 0x22));
    let text_pos = bg_rect.left_top() + egui::vec2(pad_x, pad_y);
    painter.galley(text_pos, galley, egui::Color32::WHITE);
}

/// サムネイル左上 (補/消 バッジの右隣) にタグ (`#xxx #yyy`) を 1 つの緑バッジで描画する。
/// 幅は `painter.layout_no_wrap` で実測するので CJK / 絵文字でも正確に収まる。
/// `start.x` 以降、`max_x` まで使えるので、`max_x - start.x` を超える分は文字単位で削って
/// 末尾を `…` にする。空配列の呼び出しは `draw_cell` 側で弾かれている前提。
fn draw_tag_badges(painter: &egui::Painter, start: egui::Pos2, max_x: f32, tags: &[String]) {
    let font = egui::FontId::proportional(11.0);
    let badge_h = 16.0;
    let pad_x = 5.0;
    let max_text_w = (max_x - start.x - pad_x * 2.0).max(0.0);
    if max_text_w < 8.0 {
        return; // 領域不足 → 表示諦め
    }
    // `#` 始まり (mIV 付与) を優先、続いて他ソフト由来の裸タグを並べる。
    let mut combined = String::new();
    for t in tags.iter().filter(|t| t.starts_with('#')) {
        if !combined.is_empty() {
            combined.push(' ');
        }
        combined.push_str(t);
    }
    for t in tags.iter().filter(|t| !t.starts_with('#')) {
        if !combined.is_empty() {
            combined.push(' ');
        }
        combined.push_str(t);
    }
    if combined.is_empty() {
        return;
    }

    // 実測ベースで省略する。CJK は 1 文字 ≒ 11px、ASCII は ≒ 6px と幅が大きく違うので、
    // 平均幅近似は使えない (`avg_char_w` で計算すると CJK が枠外にはみ出す)。
    let text_color = egui::Color32::from_rgb(180, 255, 180);
    let mut galley =
        painter.layout_no_wrap(combined.clone(), font.clone(), text_color);
    if galley.size().x > max_text_w {
        // 末尾から 1 文字ずつ削って `…` 付きで再 layout。最低 1 文字 + `…` は残す。
        let chars: Vec<char> = combined.chars().collect();
        for take in (1..chars.len()).rev() {
            let candidate: String = chars[..take].iter().collect::<String>() + "…";
            let g = painter.layout_no_wrap(candidate, font.clone(), text_color);
            if g.size().x <= max_text_w {
                galley = g;
                break;
            }
        }
        // それでも入らなければ `…` だけにする (極端に狭いセル)
        if galley.size().x > max_text_w {
            galley = painter.layout_no_wrap("…".to_string(), font.clone(), text_color);
            if galley.size().x > max_text_w {
                return;
            }
        }
    }

    let bg_w = galley.size().x + pad_x * 2.0;
    let bg_rect = egui::Rect::from_min_size(start, egui::vec2(bg_w, badge_h));
    painter.rect_filled(
        bg_rect,
        3.0,
        egui::Color32::from_rgba_unmultiplied(0, 40, 20, 170),
    );
    let text_pos = bg_rect.left_top() + egui::vec2(pad_x, (badge_h - galley.size().y) * 0.5);
    painter.galley(text_pos, galley, text_color);
}

/// サムネイル画質プレビュー用: 実グリッドと同じ `cell_w × cell_h` のセルを描画する。
/// 白背景 + 4px パディング、画像はアスペクト保持で中央配置（draw_cell と同じ方式）。
/// クリック可能で、クリック時は Response.clicked() が true になる。
pub(crate) fn tq_draw_preview(
    ui: &mut egui::Ui,
    tex: &Option<egui::TextureHandle>,
    cell_w: f32,
    cell_h: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(cell_w, cell_h), egui::Sense::click());
    let painter = ui.painter();
    // 白背景（選択状態ではないグリッドセルと同じ）
    painter.rect_filled(rect, 2.0, egui::Color32::WHITE);

    let padding = 4.0;
    let inner = rect.shrink(padding);

    match tex {
        Some(t) => {
            let tex_size = t.size_vec2();
            let scale = (inner.width() / tex_size.x).min(inner.height() / tex_size.y);
            let img_size = tex_size * scale;
            let img_rect = egui::Rect::from_center_size(inner.center(), img_size);
            painter.image(
                t.id(),
                img_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                "エンコード失敗",
                egui::FontId::proportional(14.0),
                egui::Color32::from_gray(120),
            );
        }
    }

    // ホバー時にカーソル変更 + 縁を青くしてクリック可能さを示す
    if response.hovered() {
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 150, 220)),
            egui::StrokeKind::Outside,
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    response
}

/// egui::ColorImage → image::DynamicImage 変換ヘルパー。
/// AI 推論の入力に使う。
fn color_image_to_dynamic(ci: &egui::ColorImage) -> image::DynamicImage {
    let w = ci.size[0] as u32;
    let h = ci.size[1] as u32;
    // Color32 は premultiplied で格納されているため、unmultiply してから書き出す。
    let mut buf = image::RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let c = ci.pixels[(y * w + x) as usize];
            buf.put_pixel(x, y, image::Rgba(c.to_srgba_unmultiplied()));
        }
    }
    image::DynamicImage::ImageRgba8(buf)
}

/// 透明度を持つ ColorImage を単色背景に合成して RGB の DynamicImage を返す。
/// AI アップスケールの composite-first 入力作成用。完全不透明の画像なら合成は no-op になる。
pub(crate) fn color_image_to_dynamic_composited(
    ci: &egui::ColorImage,
    bg: [u8; 3],
) -> image::DynamicImage {
    let w = ci.size[0] as u32;
    let h = ci.size[1] as u32;
    let mut buf = image::RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let c = ci.pixels[(y * w + x) as usize];
            // Color32 は premultiplied 格納。 result = fg_premul + bg * (1 - a)
            let a = c.a() as u32;
            let inv = 255 - a;
            let r = ((c.r() as u32 * 255 + bg[0] as u32 * inv) / 255).min(255) as u8;
            let g = ((c.g() as u32 * 255 + bg[1] as u32 * inv) / 255).min(255) as u8;
            let b = ((c.b() as u32 * 255 + bg[2] as u32 * inv) / 255).min(255) as u8;
            buf.put_pixel(x, y, image::Rgb([r, g, b]));
        }
    }
    image::DynamicImage::ImageRgb8(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_converter::ArchiveFormat;
    use std::path::PathBuf;

    // ── passes_rating_filter (コンテナ/画像/Video の挙動) ──

    #[test]
    fn rating_filter_container_uses_all_6_buckets() {
        let folder = GridItem::Folder(PathBuf::from("/a"));
        // ★なし OFF → 未評価フォルダも隠れる (「★5 のみ表示」が実際に効くために必要)
        let mut f = [true; 6];
        f[0] = false;
        assert!(!passes_rating_filter(&folder, 0, &f));
        // ★3 フォルダ、★3 ON なら可視
        assert!(passes_rating_filter(&folder, 3, &[true; 6]));
        // ★3 フォルダ、★3 OFF なら非可視
        let mut f = [true; 6];
        f[3] = false;
        assert!(!passes_rating_filter(&folder, 3, &f));
    }

    #[test]
    fn rating_filter_zip_pdf_containers_behave_like_folder() {
        let zip = GridItem::ZipFile(PathBuf::from("/a.zip"));
        let pdf = GridItem::PdfFile(PathBuf::from("/a.pdf"));
        let mut f = [true; 6];
        f[0] = false;
        assert!(!passes_rating_filter(&zip, 0, &f));
        assert!(!passes_rating_filter(&pdf, 0, &f));
        let mut f = [true; 6];
        f[4] = false;
        assert!(!passes_rating_filter(&zip, 4, &f));
        assert!(!passes_rating_filter(&pdf, 4, &f));
    }

    #[test]
    fn rating_filter_image_page_uses_all_6_buckets() {
        let img = GridItem::Image(PathBuf::from("/a.jpg"));
        let mut f = [true; 6];
        f[0] = false;
        assert!(!passes_rating_filter(&img, 0, &f));
        let f = [true; 6];
        assert!(passes_rating_filter(&img, 2, &f));
        let mut f = [true; 6];
        f[2] = false;
        assert!(!passes_rating_filter(&img, 2, &f));
    }

    #[test]
    fn rating_filter_zip_image_and_pdf_page_behave_like_image() {
        // ページ系の残り 2 種 (ZipImage / PdfPage) が Image と同じ 6 バケット判定で
        // 動いていることを担保 (コンテナと対称)。
        let zip_img = GridItem::ZipImage {
            zip_path: PathBuf::from("/a.zip"),
            entry_name: "x.jpg".to_string(),
        };
        let pdf_page = GridItem::PdfPage {
            pdf_path: PathBuf::from("/a.pdf"),
            page_num: 1,
            content_type: None,
        };
        let mut f = [true; 6];
        f[0] = false;
        assert!(!passes_rating_filter(&zip_img, 0, &f));
        assert!(!passes_rating_filter(&pdf_page, 0, &f));
        let mut f = [true; 6];
        f[3] = false;
        assert!(!passes_rating_filter(&zip_img, 3, &f));
        assert!(!passes_rating_filter(&pdf_page, 3, &f));
    }

    #[test]
    fn rating_filter_star5_only_hides_unrated_containers() {
        // ユーザが明示的に「★5 だけ見たい」(★5 のみ ON、他全部 OFF) を選んだとき、
        // 未評価のフォルダも確実に非表示になること (本修正の主目的)
        let folder = GridItem::Folder(PathBuf::from("/a"));
        let img = GridItem::Image(PathBuf::from("/b.jpg"));
        let mut f = [false; 6];
        f[5] = true;
        assert!(!passes_rating_filter(&folder, 0, &f));
        assert!(!passes_rating_filter(&img, 0, &f));
        assert!(passes_rating_filter(&folder, 5, &f));
        assert!(passes_rating_filter(&img, 5, &f));
    }

    #[test]
    fn rating_filter_applies_to_video_when_unrated_off() {
        // Video もレーティング対象なので「なし」フィルタ OFF で未評価動画は隠れる。
        // 通常画像と同じ扱い。
        let vid = GridItem::Video(PathBuf::from("/a.mp4"));
        // 全 OFF: ★0 (未評価) の動画は隠れる
        assert!(!passes_rating_filter(&vid, 0, &[false; 6]));
        // ★0 ON: ★0 の動画は通る
        let mut rf = [false; 6];
        rf[0] = true;
        assert!(passes_rating_filter(&vid, 0, &rf));
        // ★3 のみ ON: ★0 動画は通らない、★3 動画は通る
        let mut rf = [false; 6];
        rf[3] = true;
        assert!(!passes_rating_filter(&vid, 0, &rf));
        assert!(passes_rating_filter(&vid, 3, &rf));
    }

    #[test]
    fn separator_is_not_rating_target() {
        // ZipSeparator はレーティング対象外なのでフィルタ素通り (常に可視)。
        let sep = GridItem::ZipSeparator {
            dir_display: "x".into(),
        };
        assert!(passes_rating_filter(&sep, 0, &[false; 6]));
    }

    #[test]
    fn rating_filter_defensive_against_corrupt_stars() {
        // 想定外の stars>5 はインデックス越境を避けるため非可視にする (防御)
        let folder = GridItem::Folder(PathBuf::from("/a"));
        let img = GridItem::Image(PathBuf::from("/a.jpg"));
        assert!(!passes_rating_filter(&folder, 99, &[true; 6]));
        assert!(!passes_rating_filter(&img, 99, &[true; 6]));
    }

    /// ★フィルタ suppression の scope 判定: folder anchor。
    /// case-insensitive + component-boundary + cross-drive を `path_in_subtree_ci` で担保する。
    #[test]
    fn rating_filter_suppression_scope_folder_anchor() {
        use std::path::Path;
        let anchor = PathBuf::from("/books/book-a");
        // 同一 / 子孫
        assert!(path_in_subtree_ci(Path::new("/books/book-a"), &anchor));
        assert!(path_in_subtree_ci(
            Path::new("/books/book-a/chapter1"),
            &anchor
        ));
        assert!(path_in_subtree_ci(
            Path::new("/books/book-a/chapter1/sub"),
            &anchor
        ));
        // 親 / sibling は外
        assert!(!path_in_subtree_ci(Path::new("/books"), &anchor));
        assert!(!path_in_subtree_ci(Path::new("/books/book-b"), &anchor));
        // name prefix match (`book-a-extra`) は component 境界を守って外れる
        assert!(!path_in_subtree_ci(
            Path::new("/books/book-a-extra"),
            &anchor
        ));
    }

    #[test]
    fn rating_filter_suppression_scope_zip_pdf_anchor() {
        use std::path::Path;
        let zip = PathBuf::from("/books/vol1.zip");
        assert!(path_in_subtree_ci(Path::new("/books/vol1.zip"), &zip));
        assert!(!path_in_subtree_ci(Path::new("/books"), &zip)); // BS で親へ → outside
        assert!(!path_in_subtree_ci(Path::new("/books/vol2.zip"), &zip)); // sibling

        let pdf = PathBuf::from("/books/manual.pdf");
        assert!(path_in_subtree_ci(Path::new("/books/manual.pdf"), &pdf));
        assert!(!path_in_subtree_ci(Path::new("/books"), &pdf));
    }

    /// Windows の case-insensitive FS: ドライブ文字や階層名の casing 違いを同一視する。
    /// これが効かないと、address bar 入力や Explorer D&D で casing が変わった瞬間に
    /// suppression scope から脱落してフィルタが復元されてしまう (ユーザー視点では
    /// 「なぜか filter が急に戻った」= 旧バグ)。
    #[test]
    fn path_in_subtree_ci_is_case_insensitive() {
        use std::path::Path;
        let anchor = PathBuf::from(r"C:\Books\Vol1.zip");
        assert!(path_in_subtree_ci(Path::new(r"c:\books\vol1.zip"), &anchor));
        assert!(path_in_subtree_ci(Path::new(r"C:\BOOKS\VOL1.ZIP"), &anchor));
        // 区切り文字の違いも吸収する
        assert!(path_in_subtree_ci(Path::new("C:/Books/Vol1.zip"), &anchor));
    }

    /// cross-drive 偶然一致を起こさない (C:/foo と D:/foo は別扱い)。
    /// path_key::normalize がドライブ文字を剥がすのに対し、こちらは保持することで
    /// 別ドライブの同名コンテナを誤って同一 scope と判定しない。
    #[test]
    fn path_in_subtree_ci_keeps_drive_letter_distinct() {
        use std::path::Path;
        let anchor = PathBuf::from(r"C:\books\vol1.zip");
        assert!(!path_in_subtree_ci(Path::new(r"D:\books\vol1.zip"), &anchor));
        assert!(!path_in_subtree_ci(
            Path::new(r"D:\books\vol1.zip\page001.jpg"),
            &anchor
        ));
    }

    /// 同名フォルダがある ZIP/PDF/ConvertibleArchive (7z/LZH) は
    /// `filter_virtual_folder_duplicates` でスキップされる。
    /// v0.7.0 の Task 17 で 7z/LZH への拡張を入れた回帰テスト。
    #[test]
    fn filter_virtual_folder_skips_archive_matching_folder() {
        let mut folders: Vec<GridItem> = vec![
            GridItem::Folder(PathBuf::from("/r/vol01")),
            GridItem::ZipFile(PathBuf::from("/r/vol01.zip")), // 同名フォルダあり → 消える
            GridItem::ZipFile(PathBuf::from("/r/other.zip")), // 同名フォルダなし → 残る
            GridItem::PdfFile(PathBuf::from("/r/vol01.pdf")), // 同名フォルダあり → 消える
            GridItem::ConvertibleArchive {
                path: PathBuf::from("/r/vol01.7z"), // 同名フォルダあり → 消える
                format: ArchiveFormat::SevenZ,
            },
            GridItem::ConvertibleArchive {
                path: PathBuf::from("/r/bonus.lzh"), // 同名フォルダなし → 残る
                format: ArchiveFormat::Lzh,
            },
        ];
        let mut folder_metas: Vec<Option<(i64, i64)>> = vec![None, None, None, None, None, None];

        App::filter_virtual_folder_duplicates(&mut folders, &mut folder_metas);

        let remaining_names: Vec<String> = folders
            .iter()
            .map(|item| match item {
                GridItem::Folder(p) | GridItem::ZipFile(p) | GridItem::PdfFile(p) => {
                    p.file_name().unwrap().to_string_lossy().into_owned()
                }
                GridItem::ConvertibleArchive { path, .. } => {
                    path.file_name().unwrap().to_string_lossy().into_owned()
                }
                _ => String::new(),
            })
            .collect();

        assert_eq!(
            remaining_names,
            vec!["vol01", "other.zip", "bonus.lzh"],
            "同名フォルダ vol01 があるアーカイブ 3 件は消え、他は残る",
        );
        assert_eq!(folders.len(), folder_metas.len(), "metas も同期して削除");
    }

    /// 大文字小文字は無視して同名判定する (Windows 文化圏での実運用に合わせる)。
    #[test]
    fn filter_virtual_folder_case_insensitive() {
        let mut folders: Vec<GridItem> = vec![
            GridItem::Folder(PathBuf::from("/r/VOL01")),
            GridItem::ConvertibleArchive {
                path: PathBuf::from("/r/vol01.7z"),
                format: ArchiveFormat::SevenZ,
            },
        ];
        let mut folder_metas: Vec<Option<(i64, i64)>> = vec![None, None];

        App::filter_virtual_folder_duplicates(&mut folders, &mut folder_metas);

        assert_eq!(folders.len(), 1, "大文字小文字違いでも一致扱い");
    }

    /// `clamp_dynamic_for_gpu` は 8192 以内の画像には触れず、超えるときだけ
    /// 長辺 8192 にアスペクト比保持で縮小する。
    #[test]
    fn clamp_dynamic_for_gpu_noop_within_limit() {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(4096, 2048));
        let out = clamp_dynamic_for_gpu(img);
        assert_eq!((out.width(), out.height()), (4096, 2048));
    }

    #[test]
    fn clamp_dynamic_for_gpu_scales_portrait_oversize() {
        // 7168x9216 は再現バグのテストサイズ。長辺 9216 → 8192 で縮小され、
        // 短辺もアスペクト比を保って縮む (7168 * 8192/9216 = 6371.55… ≈ 6372)。
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(7168, 9216));
        let out = clamp_dynamic_for_gpu(img);
        assert_eq!(out.height(), 8192, "long edge clamped to MAX_TEXTURE_DIM");
        assert_eq!(out.width(), 6372, "aspect-preserving short edge");
    }

    #[test]
    fn clamp_dynamic_for_gpu_scales_landscape_oversize() {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(16384, 4096));
        let out = clamp_dynamic_for_gpu(img);
        assert_eq!(out.width(), 8192);
        assert_eq!(out.height(), 2048);
    }
}

// =======================================================================
// Phase C (App-level) テスト
//
// docs/search-test-plan.md §Phase C の位置付け。App 全体を構築して、
// 検索バー起動ヘルパ (open_favsearch / open_global_search /
// open_local_metadata_search) の相互排他ロジックを回帰テストとして固定する。
//
// 完全な Ctrl+G キー → update() 経由のフルスタックテストは eframe::Frame の
// モック化が必要で重いため、本ラウンドでは **public 起動 API の状態遷移** を
// 対象にする。検索バー同時表示バグ (2026-04 ユーザー報告) の回帰防止が主目的。
// =======================================================================

/// Phase C 共通 setup。`data_dir::TEST_OVERRIDE` (プロセス全域のグローバル状態) を
/// 使うテストはすべてここの `PHASE_C_LOCK` と `setup_app()` を経由する。
///
/// 旧実装は 3 つの test モジュールがそれぞれ独自の `PHASE_C_LOCK` を持っていて、
/// 別モジュール同士で並列実行されると data_dir override が干渉するリスクがあった
/// (Codex P3 指摘、2026-04)。親モジュールの 1 本に統合して直列化を保証する。
#[cfg(test)]
mod phase_c_support {
    use super::{App, AppTestConfig};
    use std::sync::Mutex;
    use tempfile::TempDir;

    pub(super) static PHASE_C_LOCK: Mutex<()> = Mutex::new(());

    /// テスト終了時に必ず `data_dir::set_test_override(None)` を呼ぶ RAII ガード。
    /// panic 経路でも確実にオーバーライドを解除して後続テストに影響させない。
    pub(super) struct OverrideGuard;
    impl Drop for OverrideGuard {
        fn drop(&mut self) {
            crate::data_dir::set_test_override(None);
        }
    }

    /// TempDir を data_dir として差し替え、空の settings で App を構築する。
    /// TempDir / OverrideGuard / App は declared order と逆順で drop されるので、
    /// App (supervisor join 含む) → OverrideGuard (data_dir clear) → TempDir (削除)
    /// の正しい順序で片付く。
    pub(super) fn setup_app() -> (
        App,
        OverrideGuard,
        TempDir,
        std::sync::MutexGuard<'static, ()>,
    ) {
        let lock = PHASE_C_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tmp = TempDir::new().expect("tempdir");
        crate::data_dir::set_test_override(Some(tmp.path().to_path_buf()));
        let guard = OverrideGuard;
        let config = AppTestConfig {
            data_dir: tmp.path().to_path_buf(),
            settings: None,
        };
        let app = App::new_for_test(config);
        (app, guard, tmp, lock)
    }
}

#[cfg(test)]
mod phase_c_key_tests {
    use super::*;
    use super::phase_c_support::setup_app;

    /// ベースライン: 新規 App はどの検索バーも開いていないこと。
    #[test]
    fn new_app_has_no_search_bar_open() {
        let (app, _g, _tmp, _l) = setup_app();
        assert!(!app.show_search_bar, "Ctrl+F bar must be closed");
        assert!(!app.favsearch.active, "Ctrl+S bar must be closed");
        assert!(!app.global_search.active, "Ctrl+G bar must be closed");
    }

    /// Ctrl+F 相当の起動ヘルパを呼ぶと Ctrl+F バーのみが立ち、他 2 つは閉じたままであること。
    #[test]
    fn open_local_metadata_search_activates_only_ctrl_f() {
        let (mut app, _g, _tmp, _l) = setup_app();
        app.open_local_metadata_search();
        assert!(app.show_search_bar);
        assert!(!app.favsearch.active);
        assert!(!app.global_search.active);
    }

    /// Ctrl+S 相当の起動ヘルパを呼ぶと Ctrl+S バーのみが立つこと。
    #[test]
    fn open_favsearch_activates_only_ctrl_s() {
        let (mut app, _g, _tmp, _l) = setup_app();
        app.open_favsearch();
        assert!(!app.show_search_bar);
        assert!(app.favsearch.active);
        assert!(!app.global_search.active);
    }

    /// Ctrl+G 相当の起動ヘルパを呼ぶと Ctrl+G バーのみが立つこと。
    #[test]
    fn open_global_search_activates_only_ctrl_g() {
        let (mut app, _g, _tmp, _l) = setup_app();
        app.open_global_search();
        assert!(!app.show_search_bar);
        assert!(!app.favsearch.active);
        assert!(app.global_search.active);
    }

    /// 既に別の検索バーが開いているところで Ctrl+F を起動すると、先行バーが閉じて
    /// Ctrl+F だけが残ること (相互排他、2026-04 バグ回帰ガード)。
    #[test]
    fn ctrl_f_closes_ctrl_s_and_ctrl_g() {
        let (mut app, _g, _tmp, _l) = setup_app();
        app.open_favsearch();
        app.open_global_search();
        assert!(!app.favsearch.active, "Ctrl+G should have closed Ctrl+S");
        assert!(app.global_search.active);
        app.open_local_metadata_search();
        assert!(app.show_search_bar);
        assert!(!app.favsearch.active);
        assert!(!app.global_search.active, "Ctrl+F should close Ctrl+G");
    }

    /// 既に Ctrl+F が開いているところで Ctrl+S を起動すると Ctrl+F が閉じて
    /// Ctrl+S だけが残ること (回帰)。
    #[test]
    fn ctrl_s_closes_ctrl_f() {
        let (mut app, _g, _tmp, _l) = setup_app();
        app.open_local_metadata_search();
        assert!(app.show_search_bar);
        app.open_favsearch();
        assert!(app.favsearch.active);
        assert!(!app.show_search_bar, "Ctrl+S should close Ctrl+F");
        assert!(!app.global_search.active);
    }

    /// 既に Ctrl+F が開いているところで Ctrl+G を起動すると Ctrl+F が閉じて
    /// Ctrl+G だけが残ること (回帰)。
    #[test]
    fn ctrl_g_closes_ctrl_f() {
        let (mut app, _g, _tmp, _l) = setup_app();
        app.open_local_metadata_search();
        app.open_global_search();
        assert!(app.global_search.active);
        assert!(!app.show_search_bar, "Ctrl+G should close Ctrl+F");
        assert!(!app.favsearch.active);
    }

    /// Codex P2 #3: 選択中の Ctrl+S お気に入りフィルタが設定から消えたら、
    /// `execute_favsearch` が UI と整合を取るために filter を None にクリアする。
    #[test]
    fn favsearch_clears_stale_favorite_filter() {
        let (mut app, _g, _tmp, _l) = setup_app();
        // 存在しない UUID を filter に立てて search を走らせる
        let bogus = uuid::Uuid::new_v4();
        app.favsearch.favorite_filter = Some(bogus);
        app.favsearch.query = "x".to_string();
        app.execute_favsearch();
        assert_eq!(
            app.favsearch.favorite_filter, None,
            "無効 filter は None に戻さないと UI ラベルと検索スコープが食い違う"
        );
    }

    /// Codex P2 #3 (Ctrl+G 側): 選択中の Ctrl+G お気に入りフィルタが対象セットに
    /// いなくなったら、`spawn_global_search` が filter を None にクリアする。
    #[test]
    fn global_search_clears_stale_favorite_filter() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let bogus = uuid::Uuid::new_v4();
        app.global_search.active = true;
        app.global_search.filters.favorite = Some(bogus);
        app.global_search.query = "x".to_string();
        // spawn_global_search は indexer_manager が None のときに reject_message を出して早期 return するが、
        // その前に filter の健全化は行う (コードは filter 正規化 → manager 存在確認 → spawn の順)。
        app.spawn_global_search();
        assert_eq!(
            app.global_search.filters.favorite, None,
            "無効 filter は None に戻さないと UI ラベルと検索スコープが食い違う"
        );
    }

    /// どの順番で 3 検索モードを切り替えても、同時に 2 つ以上が active にならないこと。
    /// 2026-04 報告「検索バーが 2 つでることがあった」の総合回帰ガード。
    #[test]
    fn at_most_one_search_bar_ever_active() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let check_invariant = |app: &App, label: &str| {
            let count = [
                app.show_search_bar,
                app.favsearch.active,
                app.global_search.active,
            ]
            .iter()
            .filter(|b| **b)
            .count();
            assert!(
                count <= 1,
                "{label}: 同時に active なバーが {count} 個 (F={}, S={}, G={})",
                app.show_search_bar,
                app.favsearch.active,
                app.global_search.active,
            );
        };
        // F → S → G → F → G → S → F と順番に切り替えて各ステップで不変量を確認
        check_invariant(&app, "initial");
        app.open_local_metadata_search();
        check_invariant(&app, "after open F");
        app.open_favsearch();
        check_invariant(&app, "after open S (should close F)");
        app.open_global_search();
        check_invariant(&app, "after open G (should close S)");
        app.open_local_metadata_search();
        check_invariant(&app, "after open F (should close G)");
        app.open_global_search();
        check_invariant(&app, "after open G (should close F)");
        app.open_favsearch();
        check_invariant(&app, "after open S (should close G)");
        app.open_local_metadata_search();
        check_invariant(&app, "after open F (should close S)");
    }
}

// =======================================================================
// Phase C (App-level) - Ctrl+G drill ナビゲーション状態機械テスト
//
// 2026-04 ユーザー報告バグ:
//   「Ctrl+G → 検索 → 結果のフォルダを開く → フォルダの中の PDF 一覧 →
//    PDF をクリック → ページ一覧 → BS で戻ると、PDF 一覧まで戻るはずが
//    検索結果 (Aggregated) まで 1 段多く戻ってしまう」
//
// 原因: PDF/ZIP を開いても `global_search.view.DrilledInto.current_path` が
// 更新されず、drill_back_one_level が「current_path == container_root」を
// 根拠に drill_back_to_aggregated を直接呼ぶ。
//
// 修正: container (PDF/ZIP/Folder) を開く時点で current_path をその path に
// 進めておく。BS 時は drill_back_one_level が親へ戻す動作になり、
// PDF 一覧に正しく復帰する。
// =======================================================================

#[cfg(test)]
mod phase_c_drill_nav_tests {
    use super::phase_c_support::setup_app;
    use crate::global_search::GlobalHit;
    use crate::global_search_ui::GlobalSearchView;

    /// Ctrl+G 絞り込みビューで folder_path に drill-in したあと、その配下の PDF を
    /// 開くと、drill_back_one_level が「PDF → folder_path (ヒット一覧) → Aggregated」
    /// の 2 段階 BS で辿れる状態になること。
    ///
    /// 修正前: PDF を開いても current_path=folder_path のままなので、
    /// drill_back_one_level が即 drill_back_to_aggregated を呼び、ヒット一覧を
    /// スキップして検索結果に戻ってしまう。
    #[test]
    fn bs_after_opening_pdf_in_drilled_returns_to_folder_not_aggregated() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let folder_path = std::path::PathBuf::from("C:/fav/scansnap");
        let pdf_path = folder_path.join("doc.pdf");

        // Aggregated 状態でヒットだけ用意する (実検索は行わない、
        // build_drilled_items が current_path でフィルタするため)
        app.global_search.active = true;
        app.global_search.accumulate_hit(&GlobalHit {
            path: format!("{}/doc.pdf", folder_path.display()).to_lowercase(),
            score: 1.0,
            stars: 0,
        });
        // コンテナへ drill-in (SearchContainer を Enter 相当)
        app.drill_into_container(folder_path.clone(), false);
        assert!(matches!(
            app.global_search.view,
            GlobalSearchView::DrilledInto { ref current_path, .. }
                if current_path == &folder_path
        ));

        // PDF を開く操作を模擬: 新ヘルパ `advance_drilled_current_path` が current_path を
        // pdf_path に更新する (修正前は何もしない = 下の 1 段目 BS で Aggregated に飛ぶ)
        app.advance_drilled_current_path(&pdf_path);

        // 1 段目 BS: PDF ページ → drilled folder view (ヒット一覧)
        app.drill_back_one_level();
        match &app.global_search.view {
            GlobalSearchView::DrilledInto { current_path, .. } => {
                assert_eq!(
                    current_path, &folder_path,
                    "1段目 BS で drilled folder view に戻るべき (current_path=folder)"
                );
            }
            GlobalSearchView::Aggregated => {
                panic!("BUG: BS が PDF 一覧をスキップして Aggregated に飛んだ");
            }
        }

        // 2 段目 BS: drilled folder view → Aggregated
        app.drill_back_one_level();
        assert!(
            matches!(app.global_search.view, GlobalSearchView::Aggregated),
            "2段目 BS で Aggregated に戻るべき"
        );
    }

    /// ZIP 版: PDF と同じ状態機械で動くこと (GridItem::ZipFile の click も同じ
    /// advance_drilled_current_path 経路を通る想定)。
    #[test]
    fn bs_after_opening_zip_in_drilled_returns_to_folder_not_aggregated() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let folder_path = std::path::PathBuf::from("C:/fav/archives");
        let zip_path = folder_path.join("album.zip");

        app.global_search.active = true;
        app.global_search.accumulate_hit(&GlobalHit {
            path: format!("{}/album.zip", folder_path.display()).to_lowercase(),
            score: 1.0,
            stars: 0,
        });
        app.drill_into_container(folder_path.clone(), false);
        app.advance_drilled_current_path(&zip_path);

        app.drill_back_one_level();
        match &app.global_search.view {
            GlobalSearchView::DrilledInto { current_path, .. } => {
                assert_eq!(current_path, &folder_path);
            }
            _ => panic!("BUG: BS が ZIP 一覧をスキップして Aggregated に飛んだ"),
        }

        app.drill_back_one_level();
        assert!(matches!(
            app.global_search.view,
            GlobalSearchView::Aggregated
        ));
    }

    /// 2026-04 ユーザー報告: Ctrl+G で 1 つめのコンテナに drill-in して Ctrl+↓ を
    /// 押したとき、現コンテナ subtree を抜けたら**次コンテナの container_root** に
    /// 跳ぶこと。旧実装は cross-container フラットリスト + dedup で、ネスト関係にある
    /// コンテナで「次コンテナ深部」(例: `output > 2025-12-30-1`) に直接ワープしていた。
    #[test]
    fn ctrl_down_at_container_end_jumps_to_next_container_root() {
        use crate::global_search_ui::GlobalSearchView;
        let (mut app, _g, _tmp, _l) = setup_app();
        app.global_search.active = true;

        // コンテナ A: 直接ヒット 2 件 (件数で先頭になる)。
        // parent_container("c:/root/2025-11-30/a.jpg") = "c:/root/2025-11-30" → これがコンテナ root
        // hit 親まで walk up すると "2025-11-30" のみ → DFS = [2025-11-30]
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/2025-11-30/a.jpg".into(),
            score: 1.0,
            stars: 0,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/2025-11-30/b.jpg".into(),
            score: 1.0,
            stars: 0,
        });
        // コンテナ B: deep ヒット 1 件。parent_container = "c:/root/output/2025-12-30-1"
        // DFS = [2025-12-30-1] のみ (container_root より上は walk しない)
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/output/2025-12-30-1/x.jpg".into(),
            score: 1.0,
            stars: 0,
        });

        // A に drill-in。
        let a_root = std::path::PathBuf::from("c:/root/2025-11-30");
        app.drill_into_container(a_root.clone(), false);
        match &app.global_search.view {
            GlobalSearchView::DrilledInto { current_path, container_root, .. } => {
                assert_eq!(current_path, &a_root);
                assert_eq!(container_root, &a_root);
            }
            _ => panic!("drill_into_container 後は DrilledInto"),
        }

        // Ctrl+↓: A の subtree は [a_root] のみ → 末端 → 次コンテナ B の container_root に跳ぶ。
        // ここで「container_root と current_path が同じ」になることが重要 (= breadcrumb は
        // `> 2025-12-30-1` だけになり、深部ワープ `> output > 2025-12-30-1` は起きない)。
        app.global_search_ctrl_nav(true);
        let b_root = std::path::PathBuf::from("c:/root/output/2025-12-30-1");
        match &app.global_search.view {
            GlobalSearchView::DrilledInto { current_path, container_root, .. } => {
                assert_eq!(container_root, &b_root, "次コンテナ root に跳ぶべき");
                assert_eq!(
                    current_path, &b_root,
                    "current_path が container_root と一致 (= breadcrumb は root_name だけ)"
                );
            }
            _ => panic!("Ctrl+↓ 後も DrilledInto"),
        }
    }

    /// Ctrl+G drilled view で Ctrl+↓ がコンテナ subtree 内では DFS 順で潜ること。
    /// (cross-container 跨ぎではない通常ケースの回帰ガード)
    #[test]
    fn ctrl_down_within_container_descends_dfs() {
        use crate::global_search_ui::GlobalSearchView;
        let (mut app, _g, _tmp, _l) = setup_app();
        app.global_search.active = true;
        // コンテナ root に直接ヒット + サブフォルダにもヒット。
        // parent_container 単位で見ると "root" と "root/sub" の 2 コンテナだが、
        // Newer/Older ではなく HitCount ソートで root (件数 2) → root/sub (件数 1) の順。
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/x/root/a.jpg".into(),
            score: 1.0,
            stars: 0,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/x/root/b.jpg".into(),
            score: 1.0,
            stars: 0,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/x/root/sub/c.jpg".into(),
            score: 1.0,
            stars: 0,
        });

        // コンテナ root に drill-in。subtree DFS は [root, root/sub]。
        let root = std::path::PathBuf::from("c:/x/root");
        app.drill_into_container(root.clone(), false);

        // Ctrl+↓: subtree 内 DFS の次 (root/sub)。container_root は root のまま。
        app.global_search_ctrl_nav(true);
        match &app.global_search.view {
            GlobalSearchView::DrilledInto { current_path, container_root, .. } => {
                assert_eq!(container_root, &root, "container_root は不変");
                assert_eq!(
                    current_path,
                    &std::path::PathBuf::from("c:/x/root/sub"),
                    "subtree 内 DFS で sub に潜る"
                );
            }
            _ => panic!("Ctrl+↓ 後も DrilledInto"),
        }
    }

    /// 2026-04 ユーザー要望: Ctrl+G drilled view のサブフォルダ表示は通常フォルダと
    /// 同じ filter ルールに揃える。「★3 のみ」では未評価サブフォルダは隠れ、
    /// 「なし+★3」では未評価サブフォルダが (descendant 件数バッジつきで) 表示される。
    #[test]
    fn drilled_unrated_subfolder_hidden_when_unrated_filter_off() {
        use crate::global_search::GlobalHit;
        use crate::grid_item::GridItem;
        let (mut app, _g, _tmp, _l) = setup_app();
        app.global_search.active = true;
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub_unrated/a.jpg".into(),
            score: 1.0,
            stars: 3,
        });
        app.drill_into_container(std::path::PathBuf::from("c:/root"), false);

        // ★3 のみ ON (なし=OFF) → unrated サブフォルダは隠れる
        let mut rf = [false; 6];
        rf[3] = true;
        app.settings.rating_filter = rf;
        app.rebuild_items_from_global_search();
        let folder_visible_star_only = app
            .visible_indices
            .iter()
            .any(|&i| matches!(app.items.get(i), Some(GridItem::Folder(_))));
        assert!(
            !folder_visible_star_only,
            "★3 のみフィルタでは unrated subfolder は通常フォルダと同様に隠れるべき"
        );

        // なし + ★3 → unrated subfolder 表示 + ★3 件数バッジ
        let mut rf = [false; 6];
        rf[0] = true;
        rf[3] = true;
        app.settings.rating_filter = rf;
        app.rebuild_items_from_global_search();
        let sub_idx = app.items.iter().position(|it| {
            matches!(it, GridItem::Folder(p) if p == &std::path::PathBuf::from("c:/root/sub_unrated"))
        }).expect("なし+★3 では unrated subfolder が items に並ぶ");
        assert!(
            app.idx_visible(sub_idx),
            "なし+★3 では unrated subfolder が visible_indices にも入る"
        );
        let badge = app.folder_rating_match(sub_idx).expect("badge expected");
        assert_eq!(badge.0, 1, "badge total = ★3 descendant 1 件");
    }

    /// 通常表示 (Ctrl+G 外) でも unrated folder は ★3-only で隠れる従来挙動を維持。
    #[test]
    fn unrated_folder_hidden_in_normal_view_under_star_only_filter() {
        use crate::grid_item::GridItem;
        let (mut app, _g, _tmp, _l) = setup_app();
        app.items
            .push(GridItem::Folder(std::path::PathBuf::from("c:/some/folder")));
        let mut rf = [false; 6];
        rf[3] = true;
        app.settings.rating_filter = rf;
        app.rebuild_visible_indices();
        assert!(
            app.visible_indices.is_empty(),
            "通常表示の unrated folder は ★3-only で隠れる"
        );
    }

    /// 2026-04 ユーザー報告: Ctrl+G で検索 → SearchContainer に drill-in →
    /// BS で Aggregated に戻ったとき、開いていたコンテナのセルにカーソルが
    /// 復帰してほしい (旧実装は selected=None で先頭に飛んでいた)。
    #[test]
    fn bs_back_to_aggregated_restores_cursor_on_previous_container() {
        use crate::global_search::GlobalHit;
        use crate::grid_item::GridItem;
        let (mut app, _g, _tmp, _l) = setup_app();
        // 2 つの SearchContainer を accumulate (folder_b の方を開く想定)
        app.global_search.active = true;
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/folder_a/x.jpg".into(),
            score: 1.0,
            stars: 0,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/folder_b/y.jpg".into(),
            score: 1.0,
            stars: 0,
        });
        // 初回 rebuild (Aggregated)
        app.rebuild_items_from_global_search();
        // folder_b に drill-in
        app.drill_into_container(std::path::PathBuf::from("c:/folder_b"), false);
        // BS で Aggregated に戻る
        app.drill_back_one_level();
        // 戻った先で folder_b の SearchContainer が selected であること
        let selected_path = app.selected.and_then(|i| app.items.get(i)).map(|it| match it {
            GridItem::SearchContainer { path, .. } => path.clone(),
            _ => std::path::PathBuf::new(),
        });
        assert_eq!(
            selected_path,
            Some(std::path::PathBuf::from("c:/folder_b")),
            "BS で Aggregated に戻ったとき、直前に開いた folder_b にカーソルが残るべき"
        );
    }

    /// Codex P2: ★コンテナ drill-in で suppression 起動 → drilled view 内の
    /// サブフォルダへ入る → BS で 1 階層戻る、を順に行ったとき、suppression が
    /// 維持されること。subtree 内の上下移動で復元されると未評価の中身が突然
    /// 消える挙動になる。
    #[test]
    fn drill_back_within_suppression_anchor_keeps_filter_disabled() {
        use crate::global_search::GlobalHit;
        let (mut app, _g, _tmp, _l) = setup_app();
        // ★5 フィルタ ON
        let mut rf = [false; 6];
        rf[5] = true;
        app.settings.rating_filter = rf;
        // ★5 のコンテナを rating_db に登録
        let container = std::path::PathBuf::from("c:/books/vol1");
        let key = crate::adjustment_db::normalize_path(&container);
        app.rating_db.as_ref().unwrap().set(&key, 5).unwrap();
        // 検索ヒット: コンテナ配下のサブフォルダ画像 (未評価)
        app.global_search.active = true;
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/books/vol1/sub/p1.jpg".into(),
            score: 1.0,
            stars: 0,
        });
        // SearchContainer を開く (path-based suppression を起動)
        app.maybe_suppress_rating_filter_for_opened_container_path(&container);
        app.drill_into_container(container.clone(), false);
        assert!(
            app.rating_filter_suppressed_at.is_some(),
            "drill 直後は suppression 起動中"
        );
        // サブフォルダにドリルイン
        app.drill_into_subfolder(std::path::PathBuf::from("c:/books/vol1/sub"));
        assert!(
            app.rating_filter_suppressed_at.is_some(),
            "サブフォルダへ進んでも suppression 維持"
        );
        // BS で 1 階層戻る (sub → vol1)
        app.drill_back_one_level();
        assert!(
            app.rating_filter_suppressed_at.is_some(),
            "subtree 内 BS では suppression 維持されるべき"
        );
        // さらに BS で Aggregated に戻る → suppression 解除
        app.drill_back_one_level();
        assert!(
            app.rating_filter_suppressed_at.is_none(),
            "anchor の外 (Aggregated) に出たら suppression 解除"
        );
    }

    /// 2026-04 ユーザー報告: Ctrl+G ストリーミング中に rebuild が走って items に
    /// アイテムが挿入されたとき、選択中のアイテムが別の物を指してしまっていた
    /// (旧実装は `replace_search_view_items` で `selected = None`)。
    /// 修正後: 内容キー (`thumb_reuse_key`) で旧選択を新 idx に再マップする。
    /// `checked` set も同じ仕組みで追従する。
    #[test]
    fn streaming_rebuild_preserves_selected_and_checked_by_content_key() {
        use crate::grid_item::GridItem;
        let (mut app, _g, _tmp, _l) = setup_app();
        let initial = vec![
            GridItem::Image(std::path::PathBuf::from("c:/a.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/b.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/c.jpg")),
        ];
        app.replace_search_view_items(initial, vec![None, None, None]);
        // B (idx=1) を選択、A と C をチェック
        app.selected = Some(1);
        app.checked.insert(0);
        app.checked.insert(2);

        // ストリーミング rebuild: 先頭に X が追加されて A/B/C が後ろにシフト
        let after = vec![
            GridItem::Image(std::path::PathBuf::from("c:/x.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/a.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/b.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/c.jpg")),
        ];
        app.replace_search_view_items(after, vec![None; 4]);

        assert_eq!(
            app.selected,
            Some(2),
            "選択は同じ B (内容キー) を指し続けるよう新 idx=2 に追従"
        );
        assert!(app.checked.contains(&1), "A の checked が新 idx=1 に追従");
        assert!(app.checked.contains(&3), "C の checked が新 idx=3 に追従");
        assert!(!app.checked.contains(&0), "X (新規) は checked ではない");
        assert!(!app.checked.contains(&2), "B は selected のみ、checked ではない");
    }

    /// 旧選択アイテムが新 items から消えた場合は selected = None に戻し、
    /// 先頭スクロールにフォールバックする (= 復元不能時の安全側挙動)。
    #[test]
    fn streaming_rebuild_clears_selected_when_item_disappears() {
        use crate::grid_item::GridItem;
        let (mut app, _g, _tmp, _l) = setup_app();
        let initial = vec![
            GridItem::Image(std::path::PathBuf::from("c:/a.jpg")),
            GridItem::Image(std::path::PathBuf::from("c:/b.jpg")),
        ];
        app.replace_search_view_items(initial, vec![None, None]);
        app.selected = Some(1); // B を選択

        // B が消えて A だけになる
        let after = vec![GridItem::Image(std::path::PathBuf::from("c:/a.jpg"))];
        app.replace_search_view_items(after, vec![None]);

        assert_eq!(app.selected, None, "旧選択アイテムが消えたので None");
        assert_eq!(app.scroll_offset_y, 0.0, "復元失敗時は先頭スクロール");
    }

    /// Codex P2: Ctrl+G から実フォルダ/ZIP/PDF を開いた状態で rating 変更すると、
    /// items が検索合成ビューに置き換わってはならない。
    /// `items_are_global_search_view` フラグが install_new_items 時に false に倒され、
    /// rebuild_items_from_global_search が走らないことを確認する。
    #[test]
    fn rating_change_in_real_view_does_not_rebuild_search_items() {
        use crate::grid_item::GridItem;
        let (mut app, _g, _tmp, _l) = setup_app();
        // Ctrl+G 中に実体ビュー (例: PDF を開いた直後の状態) を install_new_items 経由で構築
        app.global_search.active = true;
        let pdf_pages = vec![
            GridItem::PdfPage {
                pdf_path: std::path::PathBuf::from("c:/doc.pdf"),
                page_num: 0,
                content_type: None,
            },
            GridItem::PdfPage {
                pdf_path: std::path::PathBuf::from("c:/doc.pdf"),
                page_num: 1,
                content_type: None,
            },
        ];
        app.install_new_items(pdf_pages, vec![None, None]);
        assert!(
            !app.items_are_global_search_view,
            "install_new_items は flag を false に倒す (= 実体ビュー)"
        );
        let before_len = app.items.len();
        // rating 変更を発火 (= apply_rating_to_selection 内の rebuild 分岐)
        // ここでは rebuild_items_from_global_search を直接トリガーするか check する代わりに、
        // 分岐ロジックで実体ビュー時には visible_indices だけ更新されることを確認する。
        // ※ targets が空なら refresh_global_search_hit_stars もスキップ。
        // ここでは「items.len() が変わらない」ことだけを最低限の不変条件として確認。
        app.rebuild_visible_indices();
        assert_eq!(app.items.len(), before_len, "実体ビュー items が消えない");
        assert!(
            !app.items_are_global_search_view,
            "rebuild_visible_indices ではフラグが書き換わらない"
        );
    }

    /// Codex P2: Ctrl+G 集約結果で ★コンテナを開いたとき、コンテナ★が現在
    /// フィルタを通っていれば一時解除されること。これにより内部画像が未評価でも
    /// drilled view が空にならない。
    #[test]
    fn search_container_drill_in_suppresses_rating_filter_when_container_starred() {
        let (mut app, _g, _tmp, _l) = setup_app();
        // ★5 フィルタ ON
        let mut rf = [false; 6];
        rf[5] = true;
        app.settings.rating_filter = rf;
        // rating_db に ★5 でコンテナ (フォルダ) を登録。
        // setup_app は rating_db を必ず開ける前提。開けないなら test 環境がおかしい
        // ので `expect` で即時 panic させて原因を読みやすくする (Codex P3)。
        let folder = std::path::PathBuf::from("c:/books/vol1");
        let key = crate::adjustment_db::normalize_path(&folder);
        let db = app
            .rating_db
            .as_ref()
            .expect("rating_db must be open in test (setup_app の data_dir override 失敗?)");
        db.set(&key, 5).expect("set rating");
        // ★コンテナを「開く」前は suppression なし
        assert!(app.rating_filter_suppressed_at.is_none());
        // 開く
        app.maybe_suppress_rating_filter_for_opened_container_path(&folder);
        // suppression 起動 + filter は全 ON に
        assert_eq!(
            app.rating_filter_suppressed_at.as_ref().map(|(p, _)| p.clone()),
            Some(folder),
            "★5 コンテナを開いたら suppression anchor が設定される"
        );
        assert_eq!(
            app.settings.rating_filter, [true; 6],
            "suppression 起動中は filter が全 ON"
        );
    }

    /// Codex P2 スコープ外: コンテナ★が現フィルタを通らない場合は suppression しない。
    #[test]
    fn search_container_drill_in_does_not_suppress_when_rating_does_not_match() {
        let (mut app, _g, _tmp, _l) = setup_app();
        // ★5 フィルタ ON
        let mut rf = [false; 6];
        rf[5] = true;
        app.settings.rating_filter = rf;
        // ★3 コンテナ (★5 フィルタを通らない)
        let folder = std::path::PathBuf::from("c:/notes/draft");
        let key = crate::adjustment_db::normalize_path(&folder);
        let db = app
            .rating_db
            .as_ref()
            .expect("rating_db must be open in test");
        db.set(&key, 3).expect("set rating");
        app.maybe_suppress_rating_filter_for_opened_container_path(&folder);
        assert!(
            app.rating_filter_suppressed_at.is_none(),
            "コンテナ★がフィルタ外なら suppression しない"
        );
    }

    /// Codex P3: Ctrl+G 中にレーティング変更すると、`all_hits.stars` が更新され、
    /// drilled view のバッジ件数が新しい値に追従すること。
    #[test]
    fn rating_change_updates_global_search_hit_stars_snapshot() {
        use crate::global_search::GlobalHit;
        let (mut app, _g, _tmp, _l) = setup_app();
        app.global_search.active = true;
        // 単一画像を accumulate (★3 で受信)
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub/a.jpg".into(),
            score: 1.0,
            stars: 3,
        });
        // items に直接置く (drill_into_container は load_folder を呼ばないテスト用簡易セットアップ)
        app.items.push(crate::grid_item::GridItem::Image(
            std::path::PathBuf::from("c:/root/sub/a.jpg"),
        ));
        // rating を 1 に変更
        app.set_rating(0, 1);
        let idxs = vec![0_usize];
        app.refresh_global_search_hit_stars(&idxs);
        // all_hits.stars が更新されているはず
        assert_eq!(
            app.global_search.all_hits[0].stars, 1,
            "rating 変更後、all_hits.stars が 1 に追従するはず"
        );
    }

    /// 2026-04 ユーザー報告: Ctrl+G drilled view で「なし+★2」と「★2 のみ」で
    /// 見た目が同じだった問題の修正検証。
    /// 通常フォルダと同じ仕様に揃える:
    /// - フィルタ「なし+★2」: 未評価 subfolder は表示、バッジは ★1..★5 で集計 (= ★2 件数のみ)
    /// - フィルタ「★2 のみ」: 未評価 subfolder は visibility check で隠れる
    #[test]
    fn drilled_subfolder_badge_matches_normal_folder_semantics() {
        use crate::global_search::GlobalHit;
        use crate::grid_item::GridItem;
        let (mut app, _g, _tmp, _l) = setup_app();
        app.global_search.active = true;
        // /root/sub に: ★なし 2 件、★2 が 1 件、★4 が 1 件 を accumulate
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub/a.jpg".into(),
            score: 1.0,
            stars: 0,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub/b.jpg".into(),
            score: 1.0,
            stars: 0,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub/c.jpg".into(),
            score: 1.0,
            stars: 2,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub/d.jpg".into(),
            score: 1.0,
            stars: 4,
        });
        app.drill_into_container(std::path::PathBuf::from("c:/root"), false);

        // ── フィルタ「なし + ★2」: subfolder visible + badge=1 (★2 件数のみ) ──
        let mut rf = [false; 6];
        rf[0] = true;
        rf[2] = true;
        app.settings.rating_filter = rf;
        app.rebuild_items_from_global_search();
        let sub_idx = app.items.iter().position(|it| {
            matches!(it, GridItem::Folder(p) if p == &std::path::PathBuf::from("c:/root/sub"))
        }).expect("subfolder in items");
        assert!(app.idx_visible(sub_idx), "なし+★2 で unrated subfolder が表示");
        let badge = app.folder_rating_match(sub_idx).expect("badge");
        assert_eq!(
            badge.0, 1,
            "badge は ★1..★5 のみ集計。★2 が 1 件なので 1 (なしは folder visibility だけに使う)"
        );

        // ── フィルタ「★2 のみ」: subfolder 自体が visible_indices から落ちる ──
        let mut rf = [false; 6];
        rf[2] = true;
        app.settings.rating_filter = rf;
        app.rebuild_items_from_global_search();
        let sub_idx = app.items.iter().position(|it| {
            matches!(it, GridItem::Folder(p) if p == &std::path::PathBuf::from("c:/root/sub"))
        }).expect("subfolder in items");
        assert!(
            !app.idx_visible(sub_idx),
            "★2 のみで unrated subfolder は隠れる (通常フォルダと同じ挙動)"
        );
    }

    /// `restore_select_path` のターゲットが items 中に存在しない (= 何らかの理由で
    /// 消えた) 場合は selected=None のまま放置せず、先頭の visible item に
    /// フォールバックする。次の方向キーで idx 0 に飛ぶ事故を防ぐため。
    #[test]
    fn bs_back_falls_back_to_first_visible_when_target_missing() {
        use crate::global_search::GlobalHit;
        let (mut app, _g, _tmp, _l) = setup_app();
        app.global_search.active = true;
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/folder_a/x.jpg".into(),
            score: 1.0,
            stars: 0,
        });
        // 存在しないパスを restore target に設定して rebuild
        app.global_search.restore_select_path =
            Some(std::path::PathBuf::from("c:/nonexistent/folder"));
        app.rebuild_items_from_global_search();
        // フォールバック: 先頭の visible item が選択されているはず
        assert_eq!(
            app.selected,
            app.visible_indices.first().copied(),
            "target 不在のときは先頭の visible item に落ちるべき"
        );
        assert!(app.selected.is_some(), "items が空でない限り selected は Some");
    }

    /// drilled view 内で deeper subfolder に drill-in → BS で 1 階層戻ったとき、
    /// 直前に居た subfolder の Folder セルにカーソルが復帰すること。
    #[test]
    fn bs_back_within_drilled_restores_cursor_on_previous_subfolder() {
        use crate::global_search::GlobalHit;
        use crate::grid_item::GridItem;
        let (mut app, _g, _tmp, _l) = setup_app();
        app.global_search.active = true;
        // /root/sub1/x.jpg と /root/sub2/y.jpg のヒットを accumulate
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub1/x.jpg".into(),
            score: 1.0,
            stars: 0,
        });
        app.global_search.accumulate_hit(&GlobalHit {
            path: "c:/root/sub2/y.jpg".into(),
            score: 1.0,
            stars: 0,
        });
        // /root に drill-in
        app.drill_into_container(std::path::PathBuf::from("c:/root"), false);
        // /root/sub2 にさらに drill-in
        app.drill_into_subfolder(std::path::PathBuf::from("c:/root/sub2"));
        // BS で /root に戻る
        app.drill_back_one_level();
        let selected_path = app.selected.and_then(|i| app.items.get(i)).map(|it| match it {
            GridItem::Folder(p) => p.clone(),
            _ => std::path::PathBuf::new(),
        });
        assert_eq!(
            selected_path,
            Some(std::path::PathBuf::from("c:/root/sub2")),
            "drilled view で BS したとき、直前のサブフォルダ (sub2) にカーソルが残るべき"
        );
    }

    /// Ctrl+G が非アクティブな状態で advance_drilled_current_path を呼んでも
    /// view に影響しないこと (no-op)。
    #[test]
    fn advance_drilled_is_noop_when_not_in_drilled_view() {
        let (mut app, _g, _tmp, _l) = setup_app();
        assert!(matches!(
            app.global_search.view,
            GlobalSearchView::Aggregated
        ));
        app.advance_drilled_current_path(std::path::Path::new("C:/anything.pdf"));
        assert!(
            matches!(app.global_search.view, GlobalSearchView::Aggregated),
            "Aggregated 時の advance は no-op であるべき"
        );
    }
}

// =======================================================================
// Phase C - Ctrl+G drill view アドレスバー表示テスト (2026-04 報告)
//
// 期待: "🌐 全検索: \"グルグル\" > scansnap > 衛藤ヒロユキ_魔法陣グルグル01_ipad.pdf"
// バグ: PDF を開くと raw パス "d:/oldpc_backup/data2/scansnap/衛藤..._ipad.pdf"
// が address に書かれて、ブレッドクラムが失われる。
//
// 修正: `load_pdf_as_folder` (sync 経路) / `start_loading_items` (async 経路) /
// `advance_drilled_current_path` の 3 箇所で、self.address 設定の直後に
// `update_global_search_address()` を呼び直して breadcrumb を再適用する。
// =======================================================================

#[cfg(test)]
mod phase_c_drill_address_tests {
    use super::phase_c_support::setup_app;
    use crate::global_search::GlobalHit;

    /// Ctrl+G drill-in → PDF を開いた時点で address がブレッドクラム表示
    /// (`🌐 全検索: "query" > container > filename.pdf`) になること。
    /// 旧実装は raw PDF パス (`d:/.../...pdf`) が入っていた (2026-04 バグ)。
    #[test]
    fn address_shows_breadcrumb_after_opening_pdf_in_drilled() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let folder_path = std::path::PathBuf::from("d:/oldpc_backup/data2/scansnap");
        let pdf_path = folder_path.join("衛藤ヒロユキ_魔法陣グルグル01_ipad.pdf");

        app.global_search.active = true;
        app.global_search.last_executed = "グルグル".to_string();
        app.global_search.accumulate_hit(&GlobalHit {
            path: format!(
                "{}/衛藤ヒロユキ_魔法陣グルグル01_ipad.pdf",
                folder_path.display()
            )
            .to_lowercase(),
            score: 1.0,
            stars: 0,
        });
        app.drill_into_container(folder_path.clone(), false);
        // drill 直後は container_root のみの breadcrumb
        assert!(
            app.address.contains("scansnap"),
            "drill 直後: {}",
            app.address
        );
        assert!(
            app.address.contains("グルグル"),
            "drill 直後のクエリ: {}",
            app.address
        );

        // PDF を開く: advance_drilled_current_path + load_pdf_as_folder の
        // 同期 address 書き込みパスを模擬する
        app.advance_drilled_current_path(&pdf_path);
        // 「load_pdf_as_folder 内部で一旦 address = pdf_path を書く」の再現
        app.address = pdf_path.to_string_lossy().to_string();
        // 修正: 直後に update_global_search_address() が走って breadcrumb に戻す
        app.update_global_search_address();

        // 期待: raw path ではなく breadcrumb
        assert!(
            !app.address.starts_with("d:/"),
            "raw PDF path が address に残っている (修正前のバグ): {}",
            app.address
        );
        assert!(
            app.address.contains("🌐 全検索"),
            "breadcrumb prefix 欠落: {}",
            app.address
        );
        assert!(
            app.address.contains("グルグル"),
            "クエリ欠落: {}",
            app.address
        );
        assert!(
            app.address.contains("scansnap"),
            "container_root 欠落: {}",
            app.address
        );
        assert!(
            app.address.contains("衛藤ヒロユキ_魔法陣グルグル01_ipad.pdf"),
            "PDF ファイル名欠落: {}",
            app.address
        );
    }

    /// Ctrl+G が非アクティブなときは `update_global_search_address` が no-op で
    /// address を書き換えないこと (本番経路で raw path が壊れない回帰ガード)。
    #[test]
    fn update_address_is_noop_when_ctrl_g_inactive() {
        let (mut app, _g, _tmp, _l) = setup_app();
        app.address = "C:/some/folder".to_string();
        app.update_global_search_address();
        assert_eq!(
            app.address, "C:/some/folder",
            "Ctrl+G 非アクティブ時に address を書き換えてはならない"
        );
    }

    /// Aggregated 状態 → breadcrumb は N 件表示で、raw パスには戻らないこと。
    #[test]
    fn aggregated_address_shows_hit_count_not_raw_path() {
        let (mut app, _g, _tmp, _l) = setup_app();
        app.global_search.active = true;
        app.global_search.last_executed = "グルグル".to_string();
        app.global_search.accumulate_hit(&GlobalHit {
            path: "d:/scansnap/a.pdf".to_string(),
            score: 1.0,
            stars: 0,
        });
        // Aggregated のまま update_global_search_address
        app.update_global_search_address();
        assert!(
            app.address.contains("🌐 全検索"),
            "Aggregated でも prefix は付く: {}",
            app.address
        );
        assert!(
            !app.address.starts_with("d:/"),
            "Aggregated 中は raw path が入ってはならない: {}",
            app.address
        );
    }
}

/// 補正パラメータのお気に入り単位標準 (v0.8.1) に関する回帰テスト。
///
/// 3 層カスケード (個別 → お気に入り → global)、入れ子時の nearest-favorite 優先、
/// `resolve_adjust_scope` によるスコープ判定、`set_favorite_default` で冗長な個別設定が
/// 自動的に解除されること (Codex P2) を担保する。
#[cfg(test)]
mod favorite_adjustment_defaults_tests {
    use super::*;
    use super::phase_c_support::setup_app;
    use crate::adjustment::AdjustParams;
    use crate::settings::FavoriteEntry;
    use crate::ui_fullscreen::AdjustScope;
    use std::path::PathBuf;

    /// テスト用: 画像 1 枚だけを items に詰めて idx 0 を返す。
    fn push_image(app: &mut App, path: &str) -> usize {
        app.items.push(GridItem::Image(PathBuf::from(path)));
        app.thumbnails.push(ThumbnailState::Pending);
        app.items.len() - 1
    }

    fn params_with_brightness(v: f32) -> AdjustParams {
        let mut p = AdjustParams::default();
        p.brightness = v;
        p
    }

    /// effective_params は「個別 → お気に入り → global」の順で解決する。
    #[test]
    fn cascade_individual_beats_favorite_beats_global() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let fav = FavoriteEntry::new("test".to_string(), PathBuf::from("C:/pics"));
        let fav_id = fav.id;
        app.settings.favorites.push(fav);
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        // 初期状態: global
        app.settings.global_preset = params_with_brightness(5.0);
        assert_eq!(app.effective_params(idx).brightness, 5.0);

        // お気に入り標準を入れる → 優先
        app.adjustment_favorite_params
            .insert(fav_id, params_with_brightness(20.0));
        assert_eq!(app.effective_params(idx).brightness, 20.0);

        // 個別設定を入れる → 最優先
        app.adjustment_page_params.insert(idx, params_with_brightness(50.0));
        assert_eq!(app.effective_params(idx).brightness, 50.0);

        // 個別解除 → お気に入り、お気に入り解除 → global に戻る
        app.adjustment_page_params.remove(&idx);
        assert_eq!(app.effective_params(idx).brightness, 20.0);
        app.adjustment_favorite_params.remove(&fav_id);
        assert_eq!(app.effective_params(idx).brightness, 5.0);
    }

    /// 入れ子お気に入りでは最も近い祖先 (パス最長) が優先される。
    #[test]
    fn nested_favorite_picks_nearest_ancestor() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let outer = FavoriteEntry::new("outer".to_string(), PathBuf::from("C:/pics"));
        let inner = FavoriteEntry::new("inner".to_string(), PathBuf::from("C:/pics/ai"));
        let inner_id = inner.id;
        app.settings.favorites.push(outer);
        app.settings.favorites.push(inner);

        let idx = push_image(&mut app, "C:/pics/ai/gen.jpg");
        let nearest = app.current_favorite_id_for_idx(idx);
        assert_eq!(
            nearest,
            Some(inner_id),
            "深い方のお気に入りが優先されるべき"
        );
    }

    /// resolve_adjust_scope は個別 > favorite > global の順に層を報告する。
    #[test]
    fn resolve_adjust_scope_picks_effective_layer() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let fav = FavoriteEntry::new("t".to_string(), PathBuf::from("C:/pics"));
        let fav_id = fav.id;
        app.settings.favorites.push(fav);
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        assert!(matches!(app.resolve_adjust_scope(idx), AdjustScope::Global));
        app.adjustment_favorite_params
            .insert(fav_id, params_with_brightness(10.0));
        assert!(
            matches!(app.resolve_adjust_scope(idx), AdjustScope::FavoriteDefault(id) if id == fav_id)
        );
        app.adjustment_page_params
            .insert(idx, params_with_brightness(30.0));
        assert!(matches!(
            app.resolve_adjust_scope(idx),
            AdjustScope::PageOverride
        ));
    }

    /// set_favorite_default 直後に、ちょうど同じ値の個別設定を持っていたページは
    /// 冗長なので自動的に解除され、スコープは FavoriteDefault になる (Codex P2 回帰)。
    #[test]
    fn set_favorite_default_collapses_redundant_page_override() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let fav = FavoriteEntry::new("t".to_string(), PathBuf::from("C:/pics"));
        let fav_id = fav.id;
        app.settings.favorites.push(fav);
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        // 個別に brightness=25 を設定 (= これから新しい favorite 標準にしたい値)
        let custom = params_with_brightness(25.0);
        app.adjustment_page_params.insert(idx, custom.clone());
        assert!(matches!(
            app.resolve_adjust_scope(idx),
            AdjustScope::PageOverride
        ));

        // 「このお気に入りの標準にする」と同じ操作
        app.set_favorite_default(fav_id, custom);

        assert!(
            !app.adjustment_page_params.contains_key(&idx),
            "新 favorite 標準と一致する個別は解除されるべき"
        );
        assert!(
            matches!(
                app.resolve_adjust_scope(idx),
                AdjustScope::FavoriteDefault(id) if id == fav_id
            ),
            "scope は FavoriteDefault に正規化されるべき"
        );
    }

    /// clear_favorite_default でそのお気に入り配下の、global と同値な個別もまとめて解除される。
    #[test]
    fn clear_favorite_default_collapses_overrides_matching_global() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let fav = FavoriteEntry::new("t".to_string(), PathBuf::from("C:/pics"));
        let fav_id = fav.id;
        app.settings.favorites.push(fav);
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        // favorite 標準 = 20, global = 5, 個別 = 5 (= global)
        app.settings.global_preset = params_with_brightness(5.0);
        app.adjustment_favorite_params
            .insert(fav_id, params_with_brightness(20.0));
        app.adjustment_page_params.insert(idx, params_with_brightness(5.0));

        // favorite 未設定のとき effective_default は global なので、個別は当初から冗長
        // (ただし UI 経路では set_page_params が弾くのでここでは手動 insert)。
        // favorite 解除後は「global が新しい default」に戻り、同値の個別は冗長になる。
        app.clear_favorite_default(fav_id);

        assert!(
            !app.adjustment_page_params.contains_key(&idx),
            "clear 後、新 default (global) と一致する個別は解除されるべき"
        );
    }

    /// set_page_params は新 3 層カスケード用の「effective_default_for_idx」との等価比較で
    /// 冗長判定を行う。お気に入り標準と一致する params を渡しても個別は作られない。
    #[test]
    fn set_page_params_drops_individual_when_matching_favorite_default() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let fav = FavoriteEntry::new("t".to_string(), PathBuf::from("C:/pics"));
        let fav_id = fav.id;
        app.settings.favorites.push(fav);
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        let fav_default = params_with_brightness(15.0);
        app.adjustment_favorite_params.insert(fav_id, fav_default.clone());

        // 個別を入れてから、favorite と同値を書く → 削除される
        app.adjustment_page_params
            .insert(idx, params_with_brightness(99.0));
        app.set_page_params(idx, fav_default);
        assert!(
            !app.adjustment_page_params.contains_key(&idx),
            "favorite 標準と等価な個別は保存しないべき"
        );
    }

    // ── Undo/Redo for image adjustments ────────────────────────────────

    /// ページ個別補正の Undo/Redo: 1 回スライダーを動かして Ctrl+Z で戻る、Ctrl+Y で戻し直し。
    #[test]
    fn page_adjustment_undo_redo_round_trip() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        // 初期: 個別なし
        assert!(!app.adjustment_page_params.contains_key(&idx));

        // 個別設定 brightness=30 を書く (capture_adjust_full でラップ)
        app.capture_adjust_full("test slider".into(), |a| {
            a.set_page_params(idx, params_with_brightness(30.0));
        });
        assert_eq!(
            app.adjustment_page_params.get(&idx).unwrap().brightness,
            30.0
        );

        // Undo: 個別が消える
        app.apply_meta_undo();
        assert!(
            !app.adjustment_page_params.contains_key(&idx),
            "Undo 後はエントリが消える"
        );

        // Redo: 個別が brightness=30 に戻る
        app.apply_meta_redo();
        assert_eq!(
            app.adjustment_page_params.get(&idx).unwrap().brightness,
            30.0,
            "Redo で再適用される"
        );
    }

    /// Codex P1 回帰: お気に入り標準の更新で冗長な個別ページが pruning される。
    /// Undo するとお気に入り標準は元に戻り、かつ削除された個別ページも復元される。
    #[test]
    fn favorite_default_undo_restores_pruned_page_overrides() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let fav = FavoriteEntry::new("t".to_string(), PathBuf::from("C:/pics"));
        let fav_id = fav.id;
        app.settings.favorites.push(fav);
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        // 個別 = 25 (favorite 未設定)
        app.adjustment_page_params
            .insert(idx, params_with_brightness(25.0));

        // 「このお気に入りの標準にする」(= 個別と同値) を capture_adjust_full でラップ
        let new_fav_default = params_with_brightness(25.0);
        app.capture_adjust_full("set favorite".into(), |a| {
            a.set_favorite_default(fav_id, new_fav_default);
        });
        // pruning が走り、個別は消える
        assert!(
            !app.adjustment_page_params.contains_key(&idx),
            "set_favorite_default は冗長な個別を pruning する"
        );
        assert!(app.adjustment_favorite_params.contains_key(&fav_id));

        // Undo: お気に入り標準が消え、個別 25 が復元される (Codex P1)
        app.apply_meta_undo();
        assert!(
            !app.adjustment_favorite_params.contains_key(&fav_id),
            "Undo でお気に入り標準が解除"
        );
        assert_eq!(
            app.adjustment_page_params.get(&idx).unwrap().brightness,
            25.0,
            "pruning された個別ページも復元されること (Codex P1)"
        );

        // Redo: 元の状態 (お気に入り標準 + 個別なし) に戻る
        app.apply_meta_redo();
        assert!(app.adjustment_favorite_params.contains_key(&fav_id));
        assert!(
            !app.adjustment_page_params.contains_key(&idx),
            "Redo で再 pruning される"
        );
    }

    /// Codex P2 回帰: バルク操作 (apply_to_all / clear_all) も Undo に積まれる。
    #[test]
    fn bulk_apply_clear_all_pages_pushes_undo_entry() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let idx_a = push_image(&mut app, "C:/pics/a.jpg");
        let idx_b = push_image(&mut app, "C:/pics/b.jpg");

        // a, b に個別設定を入れる
        app.adjustment_page_params
            .insert(idx_a, params_with_brightness(40.0));
        app.adjustment_page_params
            .insert(idx_b, params_with_brightness(60.0));

        // 「全画像から解除」をラップして実行
        app.capture_adjust_full("clear all".into(), |a| {
            a.clear_all_page_params();
        });
        assert!(app.adjustment_page_params.is_empty(), "全画像が解除される");

        // Undo: 個別設定が両方復元される
        app.apply_meta_undo();
        assert_eq!(
            app.adjustment_page_params.get(&idx_a).unwrap().brightness,
            40.0,
            "個別設定 a が復元"
        );
        assert_eq!(
            app.adjustment_page_params.get(&idx_b).unwrap().brightness,
            60.0,
            "個別設定 b が復元"
        );

        // Redo: 全画像が再び解除される
        app.apply_meta_redo();
        assert!(app.adjustment_page_params.is_empty(), "Redo で再 clear");
    }

    /// 新しい補正操作を行うと redo スタックがクリアされる (Ctrl+Z 後の操作で
    /// ぶら下がっていた redo は無効化される)。
    #[test]
    fn new_adjustment_op_clears_redo_stack() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let idx = push_image(&mut app, "C:/pics/a.jpg");

        // 操作 1: brightness=10
        app.capture_adjust_full("op1".into(), |a| {
            a.set_page_params(idx, params_with_brightness(10.0));
        });
        // Undo → redo に積まれる
        app.apply_meta_undo();
        assert!(app.meta_undo.can_redo());

        // 操作 2: brightness=20 → redo がクリアされる
        app.capture_adjust_full("op2".into(), |a| {
            a.set_page_params(idx, params_with_brightness(20.0));
        });
        assert!(!app.meta_undo.can_redo(), "新しい操作で redo がクリアされる");
        assert_eq!(
            app.adjustment_page_params.get(&idx).unwrap().brightness,
            20.0
        );
    }

    /// `is_meaningful` 抑止: 何も変化しない write_op は Undo に積まれない。
    #[test]
    fn no_op_write_does_not_push_undo() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let _idx = push_image(&mut app, "C:/pics/a.jpg");
        let undo_len_before = app.meta_undo.undo_len();

        // 何も変更しない write_op
        app.capture_adjust_full("noop".into(), |_a| {});

        assert_eq!(
            app.meta_undo.undo_len(),
            undo_len_before,
            "no-op は積まれない"
        );
    }

    /// Codex P2 #1 回帰: グリッド Ctrl+1〜0 のスロット一括適用 (`apply_slot_to_grid_selection`)
    /// が `capture_adjust_full` でラップされ、N 枚の個別設定が 1 回の Ctrl+Z で全て戻る。
    #[test]
    fn grid_slot_apply_is_undoable() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let idx_a = push_image(&mut app, "C:/pics/a.jpg");
        let idx_b = push_image(&mut app, "C:/pics/b.jpg");
        // ratable_page_targets は visible_indices で絞るのでテストで埋めておく
        app.visible_indices = vec![idx_a, idx_b];
        // 2 枚チェック (= 一括対象)
        app.checked.insert(idx_a);
        app.checked.insert(idx_b);
        // スロット 0 に brightness=42 を保存
        app.settings.preset_slots.slots[0] = Some(crate::adjustment::PresetSlot {
            name: "test".into(),
            params: params_with_brightness(42.0),
        });

        app.capture_adjust_full("slot apply".into(), |a| {
            a.apply_slot_to_grid_selection(0);
        });
        assert_eq!(
            app.adjustment_page_params.get(&idx_a).unwrap().brightness,
            42.0
        );
        assert_eq!(
            app.adjustment_page_params.get(&idx_b).unwrap().brightness,
            42.0
        );

        // Ctrl+Z で両方戻る
        app.apply_meta_undo();
        assert!(!app.adjustment_page_params.contains_key(&idx_a));
        assert!(!app.adjustment_page_params.contains_key(&idx_b));
    }

    // ── タグ Undo: pending → finalize の検証 (Codex P3 完全対応) ──────────────

    /// stale cache + Toggle で worker が逆方向 (Remove) に解決した場合、
    /// 楽観 cache 更新は予測値だが、Undo entry は **worker が読んだ実 disk の
    /// before/after** で作られる。これにより Ctrl+Z が真の逆操作になる。
    #[test]
    fn tag_undo_uses_worker_actual_disk_state_not_optimistic_prediction() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let path = PathBuf::from("C:/pics/a.jpg");

        // 1) 操作開始: pending を register
        let tx = app.next_tag_tx_id();
        app.register_pending_tag_op(tx, "#A のトグル".into(), 1);

        // 2) 楽観 cache 更新 (UI 即時反映): cache=[] と仮定し、predicted after=[#A]
        // (実際の試験では cache に何があってもよい — Undo の正しさには影響しない)

        // 3) worker 結果到着: 実 disk は実は既に [#A] だった (= 外部ツールで付与済み)。
        //    worker は Remove を選び、tags_after=[]. tags_before=[#A] を返す。
        let actual_before = vec!["#A".to_string()];
        let actual_after: Vec<String> = vec![];
        app.test_finalize_tag_success(
            tx,
            crate::undo_stack::TagChange {
                path: path.clone(),
                before: actual_before.clone(),
                after: actual_after.clone(),
            },
        );

        // 4) meta_undo に **実 disk** の before/after で entry が積まれている
        assert_eq!(app.meta_undo.undo_len(), 1);
        match app.meta_undo.peek_undo().unwrap() {
            crate::undo_stack::UndoEntry::Tag { changes, .. } => {
                assert_eq!(changes.len(), 1);
                assert_eq!(changes[0].before, actual_before);
                assert_eq!(changes[0].after, actual_after);
            }
            _ => panic!("expected Tag entry"),
        }

        // 5) pending は finalize 後に消えている
        assert!(app.pending_tag_undos.is_empty());
    }

    /// XMP 書き込みが失敗したジョブは Undo entry に含まれない (= 失敗パスを Ctrl+Z すると
    /// 「実ディスクは変わっていないのに書き戻し命令が飛ぶ」事故を防ぐ)。
    #[test]
    fn tag_undo_skips_failed_writes() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let path_ok = PathBuf::from("C:/pics/ok.jpg");

        let tx = app.next_tag_tx_id();
        app.register_pending_tag_op(tx, "#A のトグル".into(), 2);

        // 1 件成功、1 件失敗
        app.test_finalize_tag_success(
            tx,
            crate::undo_stack::TagChange {
                path: path_ok,
                before: vec![],
                after: vec!["#A".into()],
            },
        );
        app.test_finalize_tag_failure(tx);

        // 完了 (1 success + 1 failure = 2 = expected_total)、Undo entry は成功 1 件分のみ
        assert_eq!(app.meta_undo.undo_len(), 1);
        match app.meta_undo.peek_undo().unwrap() {
            crate::undo_stack::UndoEntry::Tag { changes, .. } => {
                assert_eq!(changes.len(), 1, "失敗分は entry に含まれない");
            }
            _ => panic!(),
        }
    }

    /// 全件失敗した場合は Undo entry が積まれない (空エントリの抑止)。
    #[test]
    fn tag_undo_all_failed_pushes_no_entry() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let tx = app.next_tag_tx_id();
        app.register_pending_tag_op(tx, "all fail".into(), 2);

        app.test_finalize_tag_failure(tx);
        app.test_finalize_tag_failure(tx);

        assert_eq!(app.meta_undo.undo_len(), 0, "全件失敗は entry なし");
        assert!(app.pending_tag_undos.is_empty());
    }

    /// stale cache のシナリオ C 防止: 連続操作 + Ctrl+Z 連打で外部由来タグを
    /// 破壊しないことを検証。worker 結果ベースで Undo entry が作られているので
    /// Ctrl+Z は真の disk 状態に戻る (#A は保持される)。
    #[test]
    fn tag_undo_chain_does_not_destroy_external_tags() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let path = PathBuf::from("C:/pics/a.jpg");

        // 操作 1: 外部で #A 付与済みの状態で mIV が #B トグル
        // worker が読む disk: [#A] → 書く: [#A, #B]
        let tx1 = app.next_tag_tx_id();
        app.register_pending_tag_op(tx1, "#B".into(), 1);
        app.test_finalize_tag_success(
            tx1,
            crate::undo_stack::TagChange {
                path: path.clone(),
                before: vec!["#A".into()],
                after: vec!["#A".into(), "#B".into()],
            },
        );

        // 操作 2: #C トグル
        // worker が読む disk: [#A, #B] → 書く: [#A, #B, #C]
        let tx2 = app.next_tag_tx_id();
        app.register_pending_tag_op(tx2, "#C".into(), 1);
        app.test_finalize_tag_success(
            tx2,
            crate::undo_stack::TagChange {
                path: path.clone(),
                before: vec!["#A".into(), "#B".into()],
                after: vec!["#A".into(), "#B".into(), "#C".into()],
            },
        );

        assert_eq!(app.meta_undo.undo_len(), 2);

        // 直近 (op2) を Ctrl+Z: pop entry — before=[#A,#B] が正しい逆操作
        let entry = app.meta_undo.pop_undo().unwrap();
        match &entry {
            crate::undo_stack::UndoEntry::Tag { changes, .. } => {
                assert_eq!(changes[0].before, vec!["#A", "#B"]);
                assert_eq!(changes[0].after, vec!["#A", "#B", "#C"]);
            }
            _ => panic!(),
        }

        // 1 つ前 (op1) を Ctrl+Z: before=[#A] が正しい逆操作 — **空ではない**
        // (旧実装ではここが [] になり、Ctrl+Z で #A まで消えてしまう破壊)
        let entry = app.meta_undo.pop_undo().unwrap();
        match &entry {
            crate::undo_stack::UndoEntry::Tag { changes, .. } => {
                assert_eq!(
                    changes[0].before,
                    vec!["#A"],
                    "Ctrl+Z は外部由来 #A を保持しなければならない (シナリオ C 防止)"
                );
                assert_eq!(changes[0].after, vec!["#A", "#B"]);
            }
            _ => panic!(),
        }
    }

    /// pending が消えている (= clear_meta_undo で boundary を跨いだ) tx_id の worker 結果は
    /// 静かに drop し、Undo stack には影響しない。
    #[test]
    fn tag_undo_orphan_result_after_boundary_does_not_push() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let tx = app.next_tag_tx_id();
        app.register_pending_tag_op(tx, "abandoned".into(), 1);

        // boundary clear (フォルダ移動相当) で pending が破棄される
        app.clear_meta_undo();
        assert!(app.pending_tag_undos.is_empty());

        // 後から worker 結果が届いても entry は積まれない
        app.test_finalize_tag_success(
            tx,
            crate::undo_stack::TagChange {
                path: PathBuf::from("C:/pics/a.jpg"),
                before: vec![],
                after: vec!["#A".into()],
            },
        );
        assert_eq!(app.meta_undo.undo_len(), 0);
    }

    /// Codex P2 #1 回帰: グリッド Q / Ctrl+Backspace の一括解除
    /// (`clear_page_params_for_selection`) が capture_adjust_full でラップされ、
    /// N 枚の個別設定が 1 回の Ctrl+Z で全て復元される。
    #[test]
    fn grid_clear_selection_is_undoable() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let idx_a = push_image(&mut app, "C:/pics/a.jpg");
        let idx_b = push_image(&mut app, "C:/pics/b.jpg");
        app.visible_indices = vec![idx_a, idx_b];
        // 既存の個別設定
        app.adjustment_page_params
            .insert(idx_a, params_with_brightness(15.0));
        app.adjustment_page_params
            .insert(idx_b, params_with_brightness(35.0));
        // 2 枚チェック
        app.checked.insert(idx_a);
        app.checked.insert(idx_b);

        app.capture_adjust_full("clear sel".into(), |a| {
            a.clear_page_params_for_selection();
        });
        assert!(!app.adjustment_page_params.contains_key(&idx_a));
        assert!(!app.adjustment_page_params.contains_key(&idx_b));

        // Ctrl+Z で両方の個別設定が復元
        app.apply_meta_undo();
        assert_eq!(
            app.adjustment_page_params.get(&idx_a).unwrap().brightness,
            15.0
        );
        assert_eq!(
            app.adjustment_page_params.get(&idx_b).unwrap().brightness,
            35.0
        );
    }

    /// Codex P3 (2026-04): BS で深い階層から戻ったとき、`select_after_load` が
    /// `folder_history` の古い選択より優先されること。Ctrl+↓ で進んだ先から
    /// 戻ったとき「最初に入った位置」ではなく「今いる位置」にカーソルを合わせる
    /// ための優先順位反転 (load_folder の選択復元) の回帰テスト。
    #[test]
    fn select_after_load_overrides_folder_history() {
        use crate::grid_item::GridItem;
        let (mut app, _g, _tmp, _l) = setup_app();
        app.items.push(GridItem::Folder("c:/root/folderA".into()));
        app.items.push(GridItem::Folder("c:/root/folderB".into()));
        app.rebuild_visible_indices();

        // 履歴: folderA (idx=0) を保存
        let source = std::path::PathBuf::from("c:/root");
        app.folder_history.insert(source.clone(), (123.0, Some(0)));
        // ヒント: folderB を選びたい (BS で戻った直後の状態を模擬)
        app.select_after_load = Some("folderB".to_string());

        let restored = app.try_select_after_load();
        assert!(restored, "ヒントが items に存在するなら true を返す");
        assert_eq!(
            app.selected,
            Some(1),
            "folderB (idx=1) がヒントで選ばれるべき (履歴の folderA に負けない)"
        );
        assert!(
            app.select_after_load.is_none(),
            "ヒントは take() で消費される (次回 load に持ち越さない)"
        );
        assert!(app.scroll_to_selected, "選択行を可視化するスクロール要求が立つ");
    }

    /// Codex P3 (2026-04): ヒントの指す名前が items に無い場合 (削除等) は false を
    /// 返し、`start_loading_items` 側の履歴フォールバック分岐に委ねる。
    #[test]
    fn try_select_after_load_returns_false_on_missing_name() {
        use crate::grid_item::GridItem;
        let (mut app, _g, _tmp, _l) = setup_app();
        app.items.push(GridItem::Folder("c:/root/folderA".into()));
        app.rebuild_visible_indices();
        app.selected = None;

        app.select_after_load = Some("ghost".to_string());
        let restored = app.try_select_after_load();
        assert!(!restored, "ヒストリにフォールバックさせるため false を返す");
        assert_eq!(app.selected, None, "selected は変更されない");
        assert!(
            app.select_after_load.is_none(),
            "見つからなかった場合でもヒントは消費する (持ち越さない)"
        );
    }

    /// Codex P3 (動画レーティング対応): `rating_path_key` / `set_rating` / `get_rating`
    /// の Video 経路がフィルタテストだけでなく永続化往復で機能することを確認する。
    /// パス正規化キーが取れ、rating_db に書いた値がキャッシュ経由でなく DB ヒットでも
    /// 同じ値で読み戻せることを assert する。
    #[test]
    fn video_rating_roundtrips_through_rating_db() {
        use crate::grid_item::GridItem;
        let (mut app, _g, _tmp, _l) = setup_app();
        // Video アイテムを 1 件登録
        app.items.push(GridItem::Video(std::path::PathBuf::from("C:/clips/movie.mp4")));
        app.thumbnails.push(ThumbnailState::Pending);
        let idx = app.items.len() - 1;

        // rating_path_key が None でない (= rating_db キーを取れる)
        let key = app
            .rating_path_key(idx)
            .expect("Video の rating_path_key は Some を返す");
        assert!(
            !key.is_empty(),
            "正規化されたパスキーが空でない (= normalize_path 経由で生成される)"
        );

        // set_rating → rating_db に書き込みかつ rating_cache が更新される
        app.set_rating(idx, 3);
        assert_eq!(app.get_rating(idx), 3, "set 直後に get で同値が読める (cache hit)");
        // DB にも入っているはず
        let db_value = app
            .rating_db
            .as_ref()
            .expect("rating_db must be open in test")
            .get(&key);
        assert_eq!(db_value, 3, "rating_db に書き込まれている");

        // cache を捨てて DB から再取得 → 永続化を確認
        app.rating_cache.clear();
        assert_eq!(
            app.get_rating(idx),
            3,
            "cache を捨てても DB から ★3 が読み戻る (永続化済み)"
        );

        // 0 への戻し (= 「★解除」) も DB に反映
        app.set_rating(idx, 0);
        let db_value = app.rating_db.as_ref().unwrap().get(&key);
        assert_eq!(db_value, 0, "★0 で DB を上書きできる");
    }

    /// 見開きから消しゴムに入ったあと `reset_erase_mode` で元の見開き状態に戻ること。
    /// (Apply [E] / Cancel [Esc] どちらの経路でも内部的に reset_erase_mode が呼ばれる。)
    #[test]
    fn reset_erase_mode_restores_saved_spread_state() {
        use crate::grid_item::GridItem;
        use crate::settings::SpreadMode;
        let (mut app, _g, _tmp, _l) = setup_app();
        // ペア: idx 0 (left), idx 1 (right) を仮想的に保存している状態を組み立てる。
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/a.jpg")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/b.jpg")));
        app.thumbnails.push(ThumbnailState::Pending);
        app.thumbnails.push(ThumbnailState::Pending);
        // 編集対象は左ページとする
        app.fullscreen_idx = Some(0);
        app.spread_mode = SpreadMode::Single; // 消しゴム中の状態
        app.erase_spread_ctx = Some(crate::app::EraseSpreadCtx {
            saved_mode: SpreadMode::Ltr,
            pair: (0, 1),
        });
        app.fs_zoom = 2.0;
        app.fs_pan = egui::Vec2::new(50.0, 30.0);
        app.erase_mode = true;

        app.reset_erase_mode();

        assert_eq!(
            app.spread_mode,
            SpreadMode::Ltr,
            "reset 後に spread_mode が Ltr へ復元される"
        );
        assert_eq!(
            app.fullscreen_idx,
            Some(0),
            "fullscreen_idx は左ページに戻る (resolve_spread_pair で同ペアが復元される)"
        );
        assert!(
            app.erase_spread_ctx.is_none(),
            "spread_ctx は消費される"
        );
        assert_eq!(app.fs_zoom, 1.0, "ズームはリセット");
        assert_eq!(app.fs_pan, egui::Vec2::ZERO, "パンはリセット");
        assert!(!app.erase_mode);
    }

    /// 単一ページから入った場合 (= erase_spread_ctx が None) は spread_mode を
    /// 触らずに reset すること。誤って Single に書き換えないことの回帰ガード。
    #[test]
    fn reset_erase_mode_leaves_spread_state_untouched_when_single_entry() {
        use crate::settings::SpreadMode;
        let (mut app, _g, _tmp, _l) = setup_app();
        app.spread_mode = SpreadMode::Single;
        app.erase_spread_ctx = None;
        app.erase_mode = true;
        app.fullscreen_idx = Some(7);

        app.reset_erase_mode();

        assert_eq!(app.spread_mode, SpreadMode::Single);
        assert_eq!(
            app.fullscreen_idx,
            Some(7),
            "Single 経由なら fullscreen_idx は弄らない"
        );
    }

    /// PDF を仮想フォルダとして開いた直後、親フォルダ catalog の `pdfthumb:foo.pdf`
    /// が PDF 自身の catalog に `page_0000` として seed されることを確認する回帰
    /// テスト。これによって PDFium による初回 1 ページ目レンダリング (200-500ms)
    /// が省略される。
    #[test]
    fn seed_virtual_folder_first_thumb_copies_pdf_thumb_from_parent() {
        use crate::grid_item::GridItem;
        let (mut app, _g, tmp, _l) = setup_app();
        // 親フォルダと PDF パスを準備 (実ファイルは要らない、catalog DB のキーだけが
        // 関心事)。
        let parent_dir = tmp.path().join("parent");
        std::fs::create_dir_all(&parent_dir).unwrap();
        let pdf_path = parent_dir.join("foo.pdf");

        // 親 catalog に pdfthumb:foo.pdf を仕込む (4×4 の dummy WebP をでっち上げる)。
        let cache_dir = crate::catalog::default_cache_dir();
        let parent_db = crate::catalog::CatalogDb::open(&cache_dir, &parent_dir).unwrap();
        // 4x4 white WebP を image::ImageBuffer から生成。
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 255, 255, 255]));
        let mut webp_bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut webp_bytes),
                image::ImageFormat::WebP,
            )
            .unwrap();
        let parent_key = format!(
            "{}{}",
            crate::thumb_loader::CACHE_KEY_PDF,
            "foo.pdf"
        );
        parent_db
            .save(&parent_key, 12345, 67890, 4, 4, Some((100, 100)), &webp_bytes)
            .unwrap();

        // 仮想 catalog (PDF パスで別 SQLite) と空 cache_map を用意。
        let pdf_db = crate::catalog::CatalogDb::open(&cache_dir, &pdf_path).unwrap();
        let cache_map: Arc<
            std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        > = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));

        // PdfPage(page_num=0) を items に積む (= setup が target_idx を見つけられるように)。
        app.items.push(GridItem::PdfPage {
            pdf_path: pdf_path.clone(),
            page_num: 0,
            content_type: None,
        });

        // seed + writeback ターゲット設定を実行。PDF として認識させるため source_path には
        // pdf_path を渡す。
        app.setup_virtual_folder_seed_and_writeback(&pdf_path, &pdf_db, &cache_map);

        // 仮想 catalog の DB と cache_map の両方に page_0000 が入っているはず。
        let target_key = crate::grid_item::pdf_page_cache_key(0);
        let in_db = pdf_db.load_one(&target_key).unwrap();
        assert!(in_db.is_some(), "PDF catalog DB に page_0000 が seed される");
        let entry = in_db.unwrap();
        assert_eq!(entry.mtime, 12345);
        assert_eq!(entry.file_size, 67890);
        assert_eq!(entry.source_dims, Some((100, 100)));
        assert_eq!(entry.jpeg_data, webp_bytes);

        let in_map = cache_map.read().unwrap();
        assert!(
            in_map.contains_key(&target_key),
            "ワーカーが即時 hit するよう cache_map にも入る"
        );

        // PDF prefetch grace が 100ms 以内で設定されていること (Ctrl+↑↓ 連打時の
        // PDFium thrash 抑制)。
        assert!(
            app.pdf_prefetch_grace_until.is_some(),
            "PDF を開いた瞬間に prefetch grace を仕掛ける"
        );
    }

    /// 親フォルダ catalog にエントリが無い場合は seed しない (= no-op)。
    /// ただし writeback ターゲットと grace は仕掛ける (将来の write-back 経路用)。
    #[test]
    fn seed_virtual_folder_first_thumb_no_parent_entry_is_noop() {
        use crate::grid_item::GridItem;
        let (mut app, _g, tmp, _l) = setup_app();
        let parent_dir = tmp.path().join("parent2");
        std::fs::create_dir_all(&parent_dir).unwrap();
        let pdf_path = parent_dir.join("missing.pdf");
        let cache_dir = crate::catalog::default_cache_dir();
        let pdf_db = crate::catalog::CatalogDb::open(&cache_dir, &pdf_path).unwrap();
        let cache_map: Arc<
            std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        > = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));

        app.items.push(GridItem::PdfPage {
            pdf_path: pdf_path.clone(),
            page_num: 0,
            content_type: None,
        });

        app.setup_virtual_folder_seed_and_writeback(&pdf_path, &pdf_db, &cache_map);

        let target_key = crate::grid_item::pdf_page_cache_key(0);
        assert!(pdf_db.load_one(&target_key).unwrap().is_none());
        assert!(!cache_map.read().unwrap().contains_key(&target_key));
        // writeback ターゲットは設定される (= 後続の worker 完成時に親 catalog にミラー)。
        assert!(
            app.virtual_folder_writeback.is_some(),
            "親 catalog がミスでも writeback ターゲットは設定する"
        );
    }

    /// 仮想 catalog で先頭ページが完成したとき、`fire_virtual_folder_writeback_if_ready`
    /// が親 catalog の `pdfthumb:foo.pdf` 行に WebP データを書き戻すこと。
    #[test]
    fn fire_virtual_folder_writeback_mirrors_to_parent() {
        let (mut app, _g, tmp, _l) = setup_app();
        let parent_dir = tmp.path().join("parent3");
        std::fs::create_dir_all(&parent_dir).unwrap();
        let _pdf_path = parent_dir.join("bar.pdf");
        let cache_dir = crate::catalog::default_cache_dir();
        let parent_db_arc =
            Arc::new(crate::catalog::CatalogDb::open(&cache_dir, &parent_dir).unwrap());

        // worker が page_0000 として cache_map に入れた WebP を準備。
        let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 128, 255, 255]));
        let mut webp_bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut webp_bytes),
                image::ImageFormat::WebP,
            )
            .unwrap();
        let target_key = crate::grid_item::pdf_page_cache_key(0);
        let cache_map: Arc<
            std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        > = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
        cache_map.write().unwrap().insert(
            target_key.clone(),
            crate::catalog::CacheEntry {
                mtime: 999,
                file_size: 4096,
                jpeg_data: webp_bytes.clone(),
                source_dims: Some((400, 400)),
            },
        );

        let parent_key = format!(
            "{}{}",
            crate::thumb_loader::CACHE_KEY_PDF,
            "bar.pdf"
        );
        let cur_gen = app.items_generation;
        app.virtual_folder_writeback = Some(crate::app::VirtualFolderWriteback {
            parent_catalog: Arc::clone(&parent_db_arc),
            parent_key: parent_key.clone(),
            target_key: target_key.clone(),
            target_idx: 3,
            items_gen: cur_gen,
            cache_map: Arc::clone(&cache_map),
            parent_entry_mtime: 1234567,
            parent_entry_size: 9999,
        });

        // 別 idx で発火しても何も起きない。
        app.fire_virtual_folder_writeback_if_ready(2);
        assert!(parent_db_arc.load_one(&parent_key).unwrap().is_none());
        assert!(app.virtual_folder_writeback.is_some());

        // target_idx で発火すると親 catalog にミラーされ、writeback がクリアされる。
        app.fire_virtual_folder_writeback_if_ready(3);
        let entry = parent_db_arc
            .load_one(&parent_key)
            .unwrap()
            .expect("親 catalog にミラー保存される");
        assert_eq!(entry.mtime, 1234567);
        assert_eq!(entry.file_size, 9999);
        assert_eq!(entry.source_dims, Some((400, 400)));
        assert_eq!(entry.jpeg_data, webp_bytes);
        assert!(
            app.virtual_folder_writeback.is_none(),
            "発火後は writeback を None に戻して二重書き込みを防ぐ"
        );
    }

    /// Codex P2: items_generation がずれた古い writeback ターゲットは発火させずに
    /// クリアするだけにする (= 別フォルダに飛んで戻ったときの誤発火防止)。
    #[test]
    fn fire_virtual_folder_writeback_drops_stale_generation() {
        let (mut app, _g, tmp, _l) = setup_app();
        let parent_dir = tmp.path().join("parent4");
        std::fs::create_dir_all(&parent_dir).unwrap();
        let cache_dir = crate::catalog::default_cache_dir();
        let parent_db_arc =
            Arc::new(crate::catalog::CatalogDb::open(&cache_dir, &parent_dir).unwrap());
        let cache_map: Arc<
            std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        > = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
        let parent_key = format!("{}{}", crate::thumb_loader::CACHE_KEY_PDF, "stale.pdf");
        app.virtual_folder_writeback = Some(crate::app::VirtualFolderWriteback {
            parent_catalog: Arc::clone(&parent_db_arc),
            parent_key: parent_key.clone(),
            target_key: crate::grid_item::pdf_page_cache_key(0),
            target_idx: 0,
            // 古い世代 (現在 items_generation が進んだ状態を模す)。
            items_gen: 0,
            cache_map: Arc::clone(&cache_map),
            parent_entry_mtime: 0,
            parent_entry_size: 0,
        });
        // app.items_generation を進めて発火 → 親 catalog には何も書かれず writeback が
        // None に戻る。`replace_search_view_items` / `remove_items_batch` 経由で
        // items_generation だけが進む実機シナリオの再現。
        app.items_generation = 7;
        app.fire_virtual_folder_writeback_if_ready(0);
        assert!(parent_db_arc.load_one(&parent_key).unwrap().is_none());
        assert!(
            app.virtual_folder_writeback.is_none(),
            "古い世代の writeback はクリアして次に持ち越さない"
        );
    }

    /// Codex P2: 同じ世代・対象 idx で発火しても `cache_map` が空なら親 catalog には
    /// 何も書かず、writeback だけクリアする (= one-shot)。実機では from_cache 経路で
    /// finalize シグナルが届かないため永続的に再発火しない、その場で諦める。
    #[test]
    fn fire_virtual_folder_writeback_clears_when_cache_map_empty() {
        let (mut app, _g, tmp, _l) = setup_app();
        let parent_dir = tmp.path().join("parent_empty");
        std::fs::create_dir_all(&parent_dir).unwrap();
        let cache_dir = crate::catalog::default_cache_dir();
        let parent_db_arc =
            Arc::new(crate::catalog::CatalogDb::open(&cache_dir, &parent_dir).unwrap());
        // cache_map は空のまま (= worker が page_0000 を入れていない)。
        let cache_map: Arc<
            std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        > = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
        let parent_key = format!("{}{}", crate::thumb_loader::CACHE_KEY_PDF, "empty.pdf");
        let cur_gen = app.items_generation;
        app.virtual_folder_writeback = Some(crate::app::VirtualFolderWriteback {
            parent_catalog: Arc::clone(&parent_db_arc),
            parent_key: parent_key.clone(),
            target_key: crate::grid_item::pdf_page_cache_key(0),
            target_idx: 0,
            items_gen: cur_gen,
            cache_map: Arc::clone(&cache_map),
            parent_entry_mtime: 0,
            parent_entry_size: 0,
        });
        app.fire_virtual_folder_writeback_if_ready(0);
        assert!(parent_db_arc.load_one(&parent_key).unwrap().is_none());
        assert!(
            app.virtual_folder_writeback.is_none(),
            "cache_map が空でも 1 度試したらクリアして次の finalize に持ち越さない"
        );
    }

    /// Codex P2: 親 catalog のサムネが壊れていた場合、`save_thumb_bytes` が `Ok(false)`
    /// を返し、seed 経路は `cache_map` に入れない。これがないと表示時にデコード失敗で
    /// 永続的に Failed になる退行が起きる。
    #[test]
    fn seed_skips_cache_map_when_parent_thumb_undecodable() {
        use crate::grid_item::GridItem;
        let (mut app, _g, tmp, _l) = setup_app();
        let parent_dir = tmp.path().join("parent_corrupt");
        std::fs::create_dir_all(&parent_dir).unwrap();
        let pdf_path = parent_dir.join("corrupt.pdf");
        let cache_dir = crate::catalog::default_cache_dir();
        let parent_db = crate::catalog::CatalogDb::open(&cache_dir, &parent_dir).unwrap();
        // 寸法が取れないバイト列 (適当な ASCII)。`decode_thumb_dims` が None を返す。
        let bad_bytes: Vec<u8> = b"NOT-A-WEBP-OR-JPEG".to_vec();
        let parent_key = format!("{}{}", crate::thumb_loader::CACHE_KEY_PDF, "corrupt.pdf");
        // 直接 save() で壊れたバイトを置く (= 旧版や外部経路で壊れた行が残った状況を再現)。
        parent_db
            .save(&parent_key, 0, 0, 1, 1, None, &bad_bytes)
            .unwrap();

        let pdf_db = crate::catalog::CatalogDb::open(&cache_dir, &pdf_path).unwrap();
        let cache_map: Arc<
            std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
        > = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));

        app.items.push(GridItem::PdfPage {
            pdf_path: pdf_path.clone(),
            page_num: 0,
            content_type: None,
        });
        app.setup_virtual_folder_seed_and_writeback(&pdf_path, &pdf_db, &cache_map);

        // cache_map は空のまま (= 表示時に decode 失敗で Failed にならない)。
        let target_key = crate::grid_item::pdf_page_cache_key(0);
        assert!(
            !cache_map.read().unwrap().contains_key(&target_key),
            "壊れたサムネは cache_map に入れない (= Failed 退行を防ぐ)"
        );
        // writeback ターゲットは設定される (= 後続 worker が完成させたら親 catalog に
        // 書き戻して corrupt 行を上書きする経路は維持)。
        assert!(app.virtual_folder_writeback.is_some());
    }

    /// `save_thumb_bytes` 単体: 寸法が取れないバイト列を渡したら `Ok(false)` を返し、
    /// DB には何も書き込まない。
    #[test]
    fn save_thumb_bytes_returns_false_for_undecodable() {
        let (_app, _g, tmp, _l) = setup_app();
        let cache_dir = crate::catalog::default_cache_dir();
        let dir = tmp.path().join("save_thumb_test");
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::catalog::CatalogDb::open(&cache_dir, &dir).unwrap();
        let bad: Vec<u8> = b"DEADBEEF-NOT-AN-IMAGE".to_vec();
        let saved = db
            .save_thumb_bytes("k1", 1, 2, None, &bad)
            .expect("Err なし、Ok(false) で skip 表現");
        assert!(!saved, "壊れたバイト列は save 断念 → false");
        assert!(db.load_one("k1").unwrap().is_none(), "DB にも何も書かない");
    }

    /// Codex P1 (0.8.2 後半): `release_fs_nav_lock` が `fs_nav_locked_gen` /
    /// `fs_holdover_tex` を確実に解除すること。境界・キャンセル経路で items_generation
    /// が進まないまま return すると `poll_fs_nav_lock` では解除されないため、
    /// その経路で本ヘルパが呼ばれることを別途確認する必要がある。
    #[test]
    fn release_fs_nav_lock_clears_both_fields() {
        let (mut app, _g, _tmp, _l) = setup_app();
        app.fs_nav_locked_gen = Some(42);
        app.fs_holdover_tex = None; // None でも問題ない
        app.release_fs_nav_lock();
        assert!(app.fs_nav_locked_gen.is_none());
        assert!(app.fs_holdover_tex.is_none());
        // 何度呼んでも safe (idempotent)
        app.release_fs_nav_lock();
        assert!(app.fs_nav_locked_gen.is_none());
    }

    /// `find_fullscreen_nav_target`: 常にフォルダ先頭の画像系アイテムを返すこと。
    /// Ctrl+↑ でも前フォルダの最後ではなく最初の画像に着地させる仕様変更の回帰ガード。
    /// (旧 API の `forward: bool` 引数は仕様統一に伴って削除済み。)
    #[test]
    fn find_fullscreen_nav_target_returns_first_image() {
        use crate::grid_item::GridItem;
        let (mut app, _g, _tmp, _l) = setup_app();
        // 並び: Folder, Image_a, Image_b, Image_c (visible_indices は items 全部)
        app.items.push(GridItem::Folder("c:/sub".into()));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/a.jpg")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/b.jpg")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/c.jpg")));
        for _ in 0..app.items.len() {
            app.thumbnails.push(ThumbnailState::Pending);
        }
        app.rebuild_visible_indices();
        assert_eq!(
            app.find_fullscreen_nav_target(),
            Some(1),
            "先頭の画像系アイテム idx=1 (Image_a) を返す (Folder idx=0 はスキップ)"
        );
    }

    /// `poll_fs_nav_lock`: items_generation が進んだあとに新ページのサムネが
    /// Loaded になって初めてロック解除されること。`items_generation` チェックが
    /// 無いと、ナビ発火直後 (まだ items 入れ替え前) で旧ページのサムネが Loaded だと
    /// 誤って解除されて、後で実際に items が入れ替わった瞬間に「ファイル名のみ表示」が
    /// 一瞬出てしまう不具合になる。その回帰テスト。
    #[test]
    fn poll_fs_nav_lock_waits_for_items_generation_bump() {
        use crate::grid_item::{GridItem, ThumbnailState};
        let (mut app, _g, _tmp, _l) = setup_app();
        let ctx = egui::Context::default();
        let dummy_tex = ctx.load_texture(
            "test_dummy",
            egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        // 初期: items_generation = 0、idx 0 が「現在表示中ページ」 (Loaded 済) として置く。
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/a.jpg")));
        app.thumbnails.push(ThumbnailState::Loaded {
            tex: dummy_tex.clone(),
            from_cache: false,
            rendered_at_px: 64,
            source_dims: None,
        });
        app.fullscreen_idx = Some(0);
        // items_generation は変えない (= 0)、ロック発火時の gen は 0 を記録する想定
        let lock_gen = app.items_generation;
        app.fs_nav_locked_gen = Some(lock_gen);
        app.fs_holdover_tex = Some(dummy_tex.clone());

        // ① items_generation が進んでいない (= まだナビが完了していない) →
        //    現ページが Loaded でもロック解除されない (主要バグの回帰防止)。
        app.poll_fs_nav_lock();
        assert!(
            app.fs_nav_locked_gen.is_some(),
            "items_generation 据え置きならロック維持 (旧ページの Loaded で誤解除しない)"
        );

        // ② items_generation を進めて再度 poll: 新ページのサムネ Loaded を見て解除。
        app.items_generation += 1;
        app.poll_fs_nav_lock();
        assert!(app.fs_nav_locked_gen.is_none(), "gen 進行 + Loaded で解除");
        assert!(app.fs_holdover_tex.is_none(), "holdover も解放");

        // ③ フルスクリーン抜け (fs_idx None) でも、items_generation 進行が必要。
        //    そうしないとナビ最中の close_fullscreen 経路で誤解除される。
        app.fs_nav_locked_gen = Some(app.items_generation);
        app.fs_holdover_tex = Some(dummy_tex.clone());
        app.fullscreen_idx = None;
        app.poll_fs_nav_lock();
        assert!(
            app.fs_nav_locked_gen.is_some(),
            "items_generation 据え置きなら fs_idx=None でも維持 (apply_folder_nav_result 内の close→open 遷移を許容)"
        );
        // items が進んでから抜け検知すれば解除される。
        app.items_generation += 1;
        app.poll_fs_nav_lock();
        assert!(app.fs_nav_locked_gen.is_none());
        assert!(app.fs_holdover_tex.is_none());
    }

    /// `get_or_open_catalog` の LRU キャッシュ動作: 同じフォルダで 2 回呼ぶと
    /// 同じ `Arc<CatalogDb>` (= 同じ Arc::ptr) が返ること、容量を超えると古いものから
    /// 抜けること。Ctrl+↓ 連打時に毎ステップ `CatalogDb::open` を走らせないための
    /// 本キャッシュの回帰ガード。
    #[test]
    fn catalog_cache_reuses_arc_for_same_folder_and_evicts_oldest() {
        let (mut app, _g, tmp, _l) = setup_app();
        // tmp 配下に 18 個 (LRU 上限 16 を 2 つ超える) のフォルダを作る。
        let mut folders: Vec<std::path::PathBuf> = Vec::new();
        for i in 0..18 {
            let p = tmp.path().join(format!("cat_{i:02}"));
            std::fs::create_dir_all(&p).unwrap();
            folders.push(p);
        }
        // 最初の open: 新規挿入。
        let arc0_first = app.get_or_open_catalog(&folders[0]).expect("open ok");
        // 同じフォルダを再度 open: 同 Arc が返る (= キャッシュヒット)。
        let arc0_again = app.get_or_open_catalog(&folders[0]).expect("hit");
        assert!(
            std::sync::Arc::ptr_eq(&arc0_first, &arc0_again),
            "同フォルダの 2 回目は同じ Arc を返す"
        );
        assert_eq!(app.catalog_cache_order.len(), 1);
        // 16 個目までは順に挿入。
        for f in &folders[1..16] {
            app.get_or_open_catalog(f);
        }
        assert_eq!(app.catalog_cache_order.len(), 16);
        assert!(app.catalog_cache.contains_key(&folders[0]));
        // 17 個目: folders[0] が evict される (LRU 順で古いから)。
        // ただし「folders[0] を最後に hit」した直後ではない (上の hit は新規挿入の直後の
        // 触り直しで再度末尾移動しているはず) なので、ここで先に folders[1] を hit させて
        // folders[0] を本当に最古に位置づけ直す。
        app.get_or_open_catalog(&folders[1]); // [1] を末尾へ
        // この時点で LRU 順は [0, 2, 3, ..., 15, 1] (= 0 が最古)。
        // 17 個目 folders[16] を入れると [0] が evict される。
        app.get_or_open_catalog(&folders[16]);
        assert_eq!(app.catalog_cache_order.len(), 16);
        assert!(
            !app.catalog_cache.contains_key(&folders[0]),
            "LRU 上限超過で最古 (folders[0]) が evict される"
        );
        assert!(app.catalog_cache.contains_key(&folders[16]));
    }

    /// Codex P1 (見開き消しゴム): `erase_inpaint_pending` が idx 別 HashMap になり、
    /// 左ページ apply 直後に右ページに切り替えても左の inpaint がキャンセルされない
    /// (旧実装は `Option` で 1 件しか持たず、新ジョブ投入が旧ジョブを cancel していた)。
    /// 同じ idx への再投入だけは旧版どおり cancel する。
    #[test]
    fn erase_inpaint_pending_keeps_jobs_for_different_pages() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let (mut app, _g, _tmp, _l) = setup_app();
        // 2 つの idx (0=左, 1=右) に対して dummy pending を入れる。
        let make_pending = |idx: usize| {
            let (_tx, rx) = std::sync::mpsc::channel::<egui::ColorImage>();
            crate::ui_erase::EraseInpaintPending {
                idx,
                items_generation: 0,
                path_key: None,
                rx,
                cancel: Arc::new(AtomicBool::new(false)),
                started_at: std::time::Instant::now(),
                log_prefix: "test",
            }
        };
        let p0 = make_pending(0);
        let cancel_p0 = p0.cancel.clone();
        app.erase_inpaint_pending.insert(0, p0);
        app.erase_inpaint_pending.insert(1, make_pending(1));
        assert_eq!(app.erase_inpaint_pending.len(), 2, "両ページの pending が並走");

        // 異なる idx (1) への再投入をシミュレート: idx=0 の pending はそのまま残るべき。
        // (run_inpaint_and_cache の挙動を最小再現)
        if let Some(prev) = app.erase_inpaint_pending.remove(&1) {
            prev.cancel.store(true, Ordering::Relaxed);
        }
        app.erase_inpaint_pending.insert(1, make_pending(1));
        assert_eq!(app.erase_inpaint_pending.len(), 2);
        assert!(
            !cancel_p0.load(Ordering::Relaxed),
            "他ページ (idx=0) の pending は cancel されない"
        );

        // 同じ idx (0) への再投入は旧 pending を cancel する。
        if let Some(prev) = app.erase_inpaint_pending.remove(&0) {
            prev.cancel.store(true, Ordering::Relaxed);
        }
        assert!(
            cancel_p0.load(Ordering::Relaxed),
            "同 idx の再投入で旧 pending が cancel される"
        );
    }

    /// `switch_erase_target_in_spread`: 左→右ボタンで `fullscreen_idx` がペアの
    /// もう一方に切り替わり、`erase_spread_ctx` は維持されること (= パネルボタンが
    /// 引き続きトグルとして使える)。マスクが空の状態で呼んでも安全 (空 inpaint は
    /// 早期 return、reset 経由で spread_ctx が消えても再保存される)。
    #[test]
    fn switch_erase_target_in_spread_moves_to_other_page_keeping_ctx() {
        use crate::grid_item::GridItem;
        use crate::settings::SpreadMode;
        let (mut app, _g, _tmp, _l) = setup_app();
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/a.jpg")));
        app.items
            .push(GridItem::Image(std::path::PathBuf::from("c:/p/b.jpg")));
        app.thumbnails.push(ThumbnailState::Pending);
        app.thumbnails.push(ThumbnailState::Pending);
        app.fullscreen_idx = Some(0);
        app.spread_mode = SpreadMode::Single; // 消しゴム中
        app.erase_spread_ctx = Some(crate::app::EraseSpreadCtx {
            saved_mode: SpreadMode::Ltr,
            pair: (0, 1),
        });
        app.erase_mode = true;
        // erase_base_cache に dummy ピクセルを入れる必要は無い: マスクが空なので
        // apply_inpaint_only は composite_mask の段階で false を返し、
        // base_cache 参照は走らない。

        let dummy_ctx = egui::Context::default();
        app.switch_erase_target_in_spread(&dummy_ctx, 1);

        assert_eq!(
            app.fullscreen_idx,
            Some(1),
            "fullscreen_idx が右ページ idx=1 に切り替わる"
        );
        assert_eq!(
            app.spread_mode,
            SpreadMode::Single,
            "spread_mode は Single のまま (見開き表示には戻らない)"
        );
        let ctx = app.erase_spread_ctx.expect("spread_ctx は維持される");
        assert_eq!(ctx.saved_mode, SpreadMode::Ltr);
        assert_eq!(ctx.pair, (0, 1));
        assert!(app.erase_mode, "erase_mode は維持される (= 編集継続)");
        assert_eq!(app.fs_zoom, 1.0, "ズームはリセット");
    }

    /// Codex 0.8.2 P1: 検索バー (Ctrl+F/S/G) の TextEdit にフォーカスがある間は
    /// グリッドショートカットを抑止する。`global_search.has_focus` の追加が無いと
    /// Ctrl+G の検索入力欄で BS が `close_global_search` に流れて入力が破壊される。
    #[test]
    fn shortcuts_are_blocked_while_search_text_input_has_focus() {
        let (mut app, _g, _tmp, _l) = setup_app();
        // 全フォーカスフラグが false の初期状態ではブロックされない。
        assert!(!app.shortcuts_blocked_by_text_input());
        // Ctrl+F バー
        app.search_has_focus = true;
        assert!(app.shortcuts_blocked_by_text_input());
        app.search_has_focus = false;
        // Ctrl+S バー
        app.favsearch.has_focus = true;
        assert!(app.shortcuts_blocked_by_text_input());
        app.favsearch.has_focus = false;
        // Ctrl+G バー (本コミットの修正対象)
        app.global_search.has_focus = true;
        assert!(
            app.shortcuts_blocked_by_text_input(),
            "Ctrl+G TextEdit フォーカス中も BS / Enter / Ctrl+C 等を grid に漏らさない"
        );
        app.global_search.has_focus = false;
        // アドレスバー
        app.address_has_focus = true;
        assert!(app.shortcuts_blocked_by_text_input());
    }

    /// `poll_delete_pending` が削除完了時に `current_folder_signature` を None に
    /// 倒すことを検証 (= 後続の外部 mtime 変化で誤 reload しない)。
    ///
    /// 0.8.x で導入: 削除成功直後は items が UI 側で in-place に絞り込まれており、
    /// stale な signature が残っていると `check_external_folder_changes` が
    /// 「内容変化あり」と誤判定して数千件の items を再走査して UI が固まる。
    #[test]
    fn poll_delete_pending_clears_current_folder_signature() {
        use crate::delete_worker::{DeleteMsg, DeletePending};
        use crate::grid_item::{GridItem, ThumbnailState};
        use std::sync::atomic::AtomicBool;

        let (mut app, _g, tmp, _l) = setup_app();
        let folder = tmp.path().join("delete_test_folder");
        std::fs::create_dir_all(&folder).unwrap();
        let target_path = folder.join("a.jpg");
        std::fs::write(&target_path, b"dummy").unwrap();

        app.current_folder = Some(folder.clone());
        app.current_folder_signature = Some(0xDEAD_BEEF);
        app.items.push(GridItem::Image(target_path.clone()));
        app.thumbnails.push(ThumbnailState::Pending);

        // 完了済み worker を疑似する: succeeded に target_path を入れて Done を即送る。
        let (tx, rx) = std::sync::mpsc::channel::<DeleteMsg>();
        tx.send(DeleteMsg::Batch {
            succeeded: vec![target_path.clone()],
            failed: vec![],
        })
        .unwrap();
        tx.send(DeleteMsg::Done { canceled: false }).unwrap();
        drop(tx);

        app.delete_pending = Some(DeletePending {
            cancel: std::sync::Arc::new(AtomicBool::new(false)),
            rx,
            total: 1,
            succeeded: vec![],
            failed: vec![],
        });

        // 実 ファイルも消しておかないと metadata() が古いまま (実機では worker が消す)。
        std::fs::remove_file(&target_path).unwrap();

        app.poll_delete_pending();

        assert!(
            app.delete_pending.is_none(),
            "Done 受信で delete_pending は take される"
        );
        assert!(
            app.current_folder_signature.is_none(),
            "削除完了で signature が None reset されること"
        );
        assert_eq!(
            app.items.len(),
            0,
            "削除成功した path は items から抜かれる"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // 見開き左右独立補正: copy_spread_adjust のテスト
    // ─────────────────────────────────────────────────────────────────────

    /// 左ページに +30 を設定 → 右ページにコピー → 右ページの effective が +30 になる。
    #[test]
    fn copy_spread_adjust_left_to_right() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let left = push_image(&mut app, "C:/pics/a.jpg");
        let right = push_image(&mut app, "C:/pics/b.jpg");
        app.set_page_params(left, params_with_brightness(30.0));

        app.copy_spread_adjust(left, right);

        assert!(
            (app.effective_params(right).brightness - 30.0).abs() < f32::EPSILON,
            "右ページの effective brightness が +30 にコピーされる"
        );
        assert!(
            app.adjustment_page_params.contains_key(&right),
            "右ページの page_params エントリが作成される"
        );
    }

    /// コピー先がデフォルトと一致する値になる場合、page_params エントリは作られず
    /// カスケード解決に任せる (DB が無駄に増えない)。
    #[test]
    fn copy_spread_adjust_clears_when_matches_default() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let left = push_image(&mut app, "C:/pics/a.jpg");
        let right = push_image(&mut app, "C:/pics/b.jpg");
        // 左はデフォルト (= global_preset = identity) のまま、右に +30 を設定。
        app.set_page_params(right, params_with_brightness(30.0));
        assert!(app.adjustment_page_params.contains_key(&right));

        // 左 (= デフォルト) を右にコピー → 右の page_params は冗長なので削除される
        app.copy_spread_adjust(left, right);

        assert!(
            !app.adjustment_page_params.contains_key(&right),
            "コピー後、デフォルトと一致した page_params エントリは削除される"
        );
        assert!(
            app.effective_params(right).brightness.abs() < f32::EPSILON,
            "右ページの effective はカスケード経由でデフォルト値に戻る"
        );
    }

    /// コピー操作は capture_adjust_full 経由で Undo に乗り、Ctrl+Z で元に戻る。
    /// Redo (Ctrl+Y) で再度コピーされる。
    #[test]
    fn copy_spread_adjust_undo_redo() {
        let (mut app, _g, _tmp, _l) = setup_app();
        let left = push_image(&mut app, "C:/pics/a.jpg");
        let right = push_image(&mut app, "C:/pics/b.jpg");
        app.set_page_params(left, params_with_brightness(30.0));
        app.set_page_params(right, params_with_brightness(-10.0));
        let right_before = app.effective_params(right).brightness;
        assert!((right_before - (-10.0)).abs() < f32::EPSILON);

        app.copy_spread_adjust(left, right);
        assert!((app.effective_params(right).brightness - 30.0).abs() < f32::EPSILON);

        // Undo: 右ページが元の -10 に戻る
        app.apply_meta_undo();
        assert!(
            (app.effective_params(right).brightness - (-10.0)).abs() < f32::EPSILON,
            "Undo で右ページの brightness が -10 に戻る"
        );

        // Redo: 再度コピーされる
        app.apply_meta_redo();
        assert!(
            (app.effective_params(right).brightness - 30.0).abs() < f32::EPSILON,
            "Redo で右ページの brightness が +30 に再適用される"
        );
    }
}
