//! 検索インデクサ / notify-rs 監視系テスト用の共通ハーネス。
//!
//! `tests/common/mod.rs` は Rust 統合テストの慣例で、各テストバイナリが `mod common;`
//! と書くと中身を取り込める。`tests/common.rs` と違って cargo は個別のテストバイナリを
//! 生成しない。
//!
//! ## 提供するもの
//!
//! - [`FixtureRoot`]: `tempfile::TempDir` を包んだお気に入りルート。画像・PDF・ZIP の
//!   書き込みヘルパを持つ。Drop 時に Windows のロック取りこぼしで消えないフォルダが
//!   残ることがあるので `best_effort_cleanup` する。
//! - [`write_png_with_text`]: 1x1 の PNG ファイルを `tEXt` チャンク付きで生成。
//!   `build_all_text_for_file` が拾える形式 (A1111 "parameters" キー) で埋め込む。
//! - [`write_png_plain`]: メタデータ無し PNG (ファイル名索引のみテスト用)。
//! - [`make_favorite`]: `auto_index_metadata=true` の `FavoriteEntry` を生成。
//! - [`wait_until`] / [`wait_scan_done`] / [`collect_search_hits`]: 非同期処理の完了を
//!   タイムアウト付きでポーリングするユーティリティ。
//!
//! ## ログ
//!
//! テスト失敗時にどこでタイムアウトしたかを切り分けられるよう、各ヘルパは
//! `eprintln!` で進捗を出す (cargo test は `--nocapture` で拾える)。

#![allow(dead_code)] // 一部テストからしか使われないヘルパが混ざる

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use tempfile::TempDir;
use uuid::Uuid;

use mimageviewer::fts_meta::FtsMetaDb;
use mimageviewer::global_search::{GlobalHit, SearchStreamEvent};
use mimageviewer::indexer_manager::IndexerManager;
use mimageviewer::name_index_supervisor::{self, NameIndexSupervisorHandle};
use mimageviewer::search_index_db::SearchIndexDb;
use mimageviewer::settings::{FavoriteEntry, IndexerSpeedProfile};

// -----------------------------------------------------------------------
// タイムアウト
// -----------------------------------------------------------------------

/// notify-rs の FS イベントが届くまでに待つ最大時間。
///
/// Windows `ReadDirectoryChangesW` は create/modify の発火が platform 依存で
/// 数百 ms かかる。さらに `search_watcher` 側の 500ms debounce、supervisor 経由の
/// `apply_single_change` までを含めると 1.5〜2 秒は見ておく必要がある。
/// CI の CPU 割当が薄い環境も想定して余裕を持たせる。
pub const FS_EVENT_TIMEOUT: Duration = Duration::from_secs(8);

/// 初期スキャン + Tantivy commit が済むまでの最大時間。少数ファイルなら 1 秒以内だが、
/// 共有 writer のロック待ちで若干膨らむ可能性を見て 10 秒。
pub const INITIAL_SCAN_TIMEOUT: Duration = Duration::from_secs(10);

/// ポーリング間隔。細かくすると notify-rs のイベントを速く拾えるが CPU を焼く。
pub const POLL_INTERVAL: Duration = Duration::from_millis(50);

// -----------------------------------------------------------------------
// FixtureRoot
// -----------------------------------------------------------------------

/// テスト用お気に入りルート。`TempDir` のラッパ。
pub struct FixtureRoot {
    tmp: Option<TempDir>,
    root: PathBuf,
}

impl FixtureRoot {
    pub fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().to_path_buf();
        Self {
            tmp: Some(tmp),
            root,
        }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// ルート配下に空のサブフォルダを掘る。
    pub fn mkdir(&self, rel: &str) -> PathBuf {
        let p = self.root.join(rel);
        std::fs::create_dir_all(&p).expect("mkdir");
        p
    }
}

impl Drop for FixtureRoot {
    fn drop(&mut self) {
        // notify-rs が tempdir を握っているとき、TempDir::drop が失敗しても
        // テストを落とさない (Supervisor 側で drop されるまでに若干 gap がある)。
        if let Some(t) = self.tmp.take() {
            let _ = t.close();
        }
    }
}

// -----------------------------------------------------------------------
// データ生成ヘルパ
// -----------------------------------------------------------------------

/// 1x1 ピクセルの PNG に A1111 形式の `parameters` tEXt チャンクを埋めて書く。
///
/// `ingest_text::build_all_text_for_file` は
/// `png_metadata::build_searchable_from_path` を呼んでおり、`parameters` キーの
/// prompt (先頭行) が検索テキストに入る。
pub fn write_png_with_text(path: &Path, parameters_value: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir");
    }
    let mut file = std::fs::File::create(path).expect("create png");
    let writer = write_png_with_chunks(&mut file, 1, 1, &[("parameters", parameters_value)]);
    writer.expect("encode png");
}

/// PNG を tEXt メタ無しで書く (ファイル名だけが検索対象)。
pub fn write_png_plain(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir");
    }
    let mut file = std::fs::File::create(path).expect("create png");
    let writer = write_png_with_chunks(&mut file, 1, 1, &[]);
    writer.expect("encode png");
}

/// 低レベル PNG writer。`png` クレートの `Encoder::add_text_chunk` で tEXt を入れる。
fn write_png_with_chunks(
    w: &mut impl Write,
    width: u32,
    height: u32,
    text_chunks: &[(&str, &str)],
) -> std::io::Result<()> {
    let mut encoder = png::Encoder::new(w, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    for (k, v) in text_chunks {
        encoder
            .add_text_chunk(k.to_string(), v.to_string())
            .map_err(std::io::Error::other)?;
    }
    let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
    let pixels = vec![255u8; (width * height * 4) as usize];
    writer
        .write_image_data(&pixels)
        .map_err(std::io::Error::other)?;
    Ok(())
}

/// ファイルを同期削除する (notify-rs に Remove イベントを起こすため)。
pub fn delete_file(path: &Path) {
    std::fs::remove_file(path).expect("remove file");
}

/// ファイルをリネームする (notify-rs は rename を Remove+Create で届ける場合が多い)。
pub fn rename_file(from: &Path, to: &Path) {
    std::fs::rename(from, to).expect("rename file");
}

// -----------------------------------------------------------------------
// FavoriteEntry
// -----------------------------------------------------------------------

/// `auto_index_metadata=true` のお気に入りを生成。
pub fn make_favorite(name: &str, path: &Path) -> FavoriteEntry {
    let mut fav = FavoriteEntry::new(name.to_string(), path.to_path_buf());
    fav.auto_index_metadata = true;
    fav
}

// -----------------------------------------------------------------------
// IndexerManager ライフサイクル
// -----------------------------------------------------------------------

/// `data_dir` 配下に fts_meta.db / fts_index/ を作って IndexerManager を起動。
///
/// `speed` は Low で 1 permit にしておくと、テストで多スレッドが並ぶときも
/// I/O の順序が決まりやすく flake が減る。
pub fn start_indexer_at(data_dir: &Path, favorites: &[FavoriteEntry]) -> IndexerManager {
    // テストでは idle 閾値 0ms にして wait_until_idle を即抜けさせる (活動ゲートの影響を排除)。
    let gate = std::sync::Arc::new(mimageviewer::activity_gate::ActivityGate::new(0));
    IndexerManager::new_at(data_dir, favorites, IndexerSpeedProfile::Low, gate)
        .expect("IndexerManager::new_at succeeds")
}

// -----------------------------------------------------------------------
// NameIndexSupervisor ライフサイクル (名前索引 / Ctrl+S 用)
// -----------------------------------------------------------------------

/// `data_dir` 配下に `search_index.db` を作って `NameIndexSupervisor` を起動。
///
/// メタ索引側と違って Tantivy writer の共有制約がないので、複数 favorite の
/// supervisor を単純に並べて spawn できる。
pub fn start_name_index_at(
    data_dir: &Path,
    favorite: &FavoriteEntry,
) -> (Arc<SearchIndexDb>, NameIndexSupervisorHandle) {
    std::fs::create_dir_all(data_dir).ok();
    let db = Arc::new(
        SearchIndexDb::open_at(&data_dir.join("search_index.db")).expect("SearchIndexDb::open_at"),
    );
    let handle =
        name_index_supervisor::spawn(favorite.id, favorite.path.clone(), Arc::clone(&db), None);
    (db, handle)
}

/// 名前索引の初期バルクスキャンが完了するまで待つ。
pub fn wait_name_scan_done(handle: &NameIndexSupervisorHandle) {
    wait_until(
        || handle.snapshot_stats().initial_scan_done,
        INITIAL_SCAN_TIMEOUT,
        &format!(
            "name index initial scan for favorite {}",
            handle.favorite_id
        ),
    );
}

/// `search_index_db::search()` で指定 query + お気に入りルートで検索し、結果を返す。
pub fn name_index_search(
    db: &SearchIndexDb,
    query: &str,
    favorite_roots: &[PathBuf],
) -> Vec<mimageviewer::search_index_db::IndexEntry> {
    db.search(
        query,
        favorite_roots,
        None,
        mimageviewer::search_query::MatchMode::And,
    )
    .expect("search_index_db::search")
}

/// 名前索引の検索結果が predicate を満たすまで polling。
pub fn wait_for_name_index_hits<F>(
    db: &SearchIndexDb,
    query: &str,
    favorite_roots: &[PathBuf],
    mut predicate: F,
    timeout: Duration,
    desc: &str,
) -> Vec<mimageviewer::search_index_db::IndexEntry>
where
    F: FnMut(&[mimageviewer::search_index_db::IndexEntry]) -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        let hits = name_index_search(db, query, favorite_roots);
        if predicate(&hits) {
            return hits;
        }
        if Instant::now() >= deadline {
            panic!(
                "wait_for_name_index_hits timed out after {:?} (last hits={} entries): {desc}",
                timeout,
                hits.len()
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

// -----------------------------------------------------------------------
// 非同期待ちユーティリティ
// -----------------------------------------------------------------------

/// `cond()` が true を返すまで `timeout` まで polling。タイムアウトで panic。
pub fn wait_until<F>(mut cond: F, timeout: Duration, desc: &str)
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        if cond() {
            return;
        }
        if Instant::now() >= deadline {
            panic!("wait_until timed out after {:?}: {desc}", timeout);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 指定 favorite の initial scan が完了するまで待つ。
pub fn wait_scan_done(mgr: &IndexerManager, favorite_id: Uuid) {
    wait_until(
        || {
            mgr.all_stats()
                .into_iter()
                .find(|s| s.favorite_id == favorite_id)
                .map(|s| s.stats.initial_scan_done)
                .unwrap_or(false)
        },
        INITIAL_SCAN_TIMEOUT,
        &format!("initial scan for favorite {favorite_id}"),
    );
}

/// `fts_meta.db` に指定 path (正規化済み) の行が現れるまで待つ。
///
/// path は小文字化された abs path (forward slash) で渡す。`normalize_path_for_key` の
/// ルールは `src/fts_meta.rs` の store ロジックを参照。ここではシンプルに
/// 呼び出し側で正規化済みの path を渡す前提にする。
pub fn wait_meta_contains(meta_db: &FtsMetaDb, normalized_path: &str) {
    wait_until(
        || {
            meta_db
                .get(normalized_path)
                .ok()
                .flatten()
                .map(|row| row.status == mimageviewer::fts_meta::FileStatus::Ok)
                .unwrap_or(false)
        },
        FS_EVENT_TIMEOUT,
        &format!("fts_meta row appears for {normalized_path}"),
    );
}

/// `fts_meta.db` から指定 path が消える (tombstone or 無し) まで待つ。
pub fn wait_meta_absent(meta_db: &FtsMetaDb, normalized_path: &str) {
    wait_until(
        || match meta_db.get(normalized_path).ok().flatten() {
            None => true,
            Some(row) => row.status != mimageviewer::fts_meta::FileStatus::Ok,
        },
        FS_EVENT_TIMEOUT,
        &format!("fts_meta row disappears for {normalized_path}"),
    );
}

/// path 正規化 (fts_meta / FtsIndex のキーと一致させる)。
///
/// 本体の `search_index_db::normalize_path` をそのまま使う (挙動が 1 箇所で定義される
/// ように)。Windows のバックスラッシュ → 正スラッシュ + 小文字化。
pub fn normalize_path(path: &Path) -> String {
    mimageviewer::search_index_db::normalize_path(path)
}

// -----------------------------------------------------------------------
// Ctrl+G 検索の結果収集
// -----------------------------------------------------------------------

/// `spawn_search` を駆動して `Done` が来るまで結果をすべて集める。
/// `Error` が来たらそのエラー文字列を含めて panic する。
pub fn collect_search_hits(
    mgr: &IndexerManager,
    query: &str,
    favorite_ids: &[Uuid],
) -> Vec<GlobalHit> {
    let handle = mgr.spawn_search(
        query.to_string(),
        favorite_ids.to_vec(),
        mimageviewer::global_search::SearchScope::default(),
    );
    drain_rx(&handle.rx, &handle.cancel)
}

/// SearchStreamEvent の受信ループ。`Done` で終了、`Error` で panic。
fn drain_rx(rx: &Receiver<SearchStreamEvent>, _cancel: &Arc<AtomicBool>) -> Vec<GlobalHit> {
    let mut hits = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!(
                "search did not complete within 10s (collected {} hits)",
                hits.len()
            );
        }
        match rx.recv_timeout(remaining) {
            Ok(SearchStreamEvent::Batch { hits: batch, .. }) => hits.extend(batch),
            Ok(SearchStreamEvent::Done { .. }) => return hits,
            Ok(SearchStreamEvent::Error(e)) => panic!("search error: {e}"),
            Err(_) => panic!("search rx disconnected before Done"),
        }
    }
}

/// 初期スキャン完了 (= walker + ingest flush 済み) 直後でも Tantivy Reader は
/// `ReloadPolicy::OnCommitWithDelay` で reload されるまでラグがある。
/// 繰り返し `spawn_search` して `predicate` が満たされるまで待つ。
///
/// 返り値は predicate が満たされた時点の hits。`timeout` を超えたら panic。
pub fn wait_for_search_hits<F>(
    mgr: &IndexerManager,
    query: &str,
    favorite_ids: &[Uuid],
    mut predicate: F,
    timeout: Duration,
    desc: &str,
) -> Vec<GlobalHit>
where
    F: FnMut(&[GlobalHit]) -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        let hits = collect_search_hits(mgr, query, favorite_ids);
        if predicate(&hits) {
            return hits;
        }
        if Instant::now() >= deadline {
            panic!(
                "wait_for_search_hits timed out after {:?} (last hits={:?}): {desc}",
                timeout, hits
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 単発 `SearchStreamEvent::Done` だけ欲しいケース (reject テスト用)。
pub fn run_search_expecting_done(
    mgr: &IndexerManager,
    query: &str,
    favorite_ids: &[Uuid],
) -> SearchStreamEvent {
    let handle = mgr.spawn_search(
        query.to_string(),
        favorite_ids.to_vec(),
        mimageviewer::global_search::SearchScope::default(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("search did not reach Done within 5s");
        }
        match handle.rx.recv_timeout(remaining) {
            Ok(ev @ SearchStreamEvent::Done { .. }) => return ev,
            Ok(SearchStreamEvent::Error(e)) => panic!("search error: {e}"),
            Ok(SearchStreamEvent::Batch { .. }) => continue,
            Err(_) => panic!("search rx disconnected"),
        }
    }
}
