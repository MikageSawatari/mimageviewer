//! リネーム時の path-keyed 永続データ移行 (rename transaction)。
//!
//! アプリ内リネーム (`ui_dialogs/rename_item.rs`) の成功後に、旧 path をキーにした
//! ユーザーデータ (★ / タグ / 回転 / 補正 / マスク / 隠蔽 / ローカル調整 / テキスト注釈 /
//! 出力範囲 / 動画ピン / 動画・音楽ブックマーク / 本ページブックマーク / 代表サムネピン / 本 resume / 見開き /
//! 閲覧履歴 / 編集プレビュー / PDF パスワード / 動画 .xmp サイドカー) を新 path キーへ引き継ぐ
//! (docs/next-release-backlog.md §1.8 の段階 1+2、review-v2.3.0 角度④ (C))。
//!
//! 方式は [`crate::zip_key_migration`] と同じ:
//! - **data_dir の DB ファイルを直接開いて移行する** (App の型付きハンドルを使わない)。
//!   busy_timeout 付きなので本体側の接続と共存できる。
//! - **worker スレッドで実行する** (UI スレッド禁止 — cold open は 1 DB で 100ms を
//!   超えることがある)。呼び出しは `App::spawn_rename_key_migration` 経由。
//! - **冪等 + 新キー優先**: 一意キー列は `UPDATE OR IGNORE` → 旧行 `DELETE`。新キー側に
//!   既に行がある (= リネーム後に先へ操作した) 場合は新データを優先して旧行を捨てる。
//! - **exact + prefix の 3 面**: リネーム対象そのもの (`old` = `new`)、フォルダ配下
//!   (`old/…` → `new/…`)、アーカイブ内エントリ / PDF ページ (`old::…` → `new::…`、
//!   `adjustment_db::zip_entry_key` 形式)。prefix 照合は LIKE ではなく `substr` 等値
//!   (path に `%` / `_` が含まれても誤爆しない)。
//!
//! ## 対象外 (許容する制限)
//! - **フォルダ改名時の配下 PDF パスワード**: キーが SHA-256 ハッシュのため列挙不可。
//!   単一 PDF の改名だけ平文を読み直して付け替える。
//! - **閲覧履歴の配下 prefix**: 履歴は自己修復する (次に開いたとき新キーで upsert)
//!   ため exact のみ移行し、title も次回オープンで更新されるのに任せる。
//! - **代表サムネピンの親フォルダ側 `source_rel`**: 親ピンが改名した子を container 相対
//!   パスで指しているケース。大文字小文字を保った照合が SQL では難しく、壊れても
//!   自動サムネへのフォールバック + 1 操作で付け直せるため見送り。
//! - **通常のサムネイルカタログ / 検索索引 / 変換アーカイブ対応表など rebuildable なキャッシュ**:
//!   再生成に任せる (フォルダ改名直後はサムネが再生成される)。通常サムネイルは mtime / size
//!   を含む catalog identity なので移行しない。一方、フルスクリーン編集経路だけが生成できる
//!   `edit_preview_cache.db/edit_previews.item_key` は再生成可能な通常キャッシュではないため移行する。
//! - **エクスプローラー等アプリ外でのリネーム**: このモジュールはアプリ内リネームの
//!   成功ハンドラからしか呼ばれない (外部リネームの検知は将来課題)。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

#[derive(Debug)]
struct JournalWriteSnapshot {
    revision: u64,
    data_dir: PathBuf,
    entries: Vec<(PathBuf, PathBuf)>,
}

#[derive(Default)]
struct JournalWriteState {
    next_revision: u64,
    latest: Option<JournalWriteSnapshot>,
    completed_revision: u64,
    shutdown: bool,
    worker_stopped: bool,
}

impl JournalWriteState {
    fn enqueue(&mut self, data_dir: PathBuf, entries: Vec<(PathBuf, PathBuf)>) -> u64 {
        self.next_revision = self.next_revision.wrapping_add(1).max(1);
        let revision = self.next_revision;
        // App owns the contents. Keep only the newest waiting snapshot while an older write runs.
        self.latest = Some(JournalWriteSnapshot {
            revision,
            data_dir,
            entries,
        });
        revision
    }

    fn take_latest(&mut self) -> Option<JournalWriteSnapshot> {
        self.latest.take()
    }

    fn complete(&mut self, revision: u64) {
        self.completed_revision = self.completed_revision.max(revision);
    }

    fn is_flushed(&self, revision: u64) -> bool {
        self.completed_revision >= revision
    }
}

type JournalWriterShared = Arc<(Mutex<JournalWriteState>, Condvar)>;

/// SQLite の既定可変長 parameter 上限 (999) を十分下回る exact purge の batch 幅。
const PURGE_EXACT_BATCH_SIZE: usize = 500;

/// 移行結果。`rows` = 書き換えた行数合計 (sidecar / パスワードは 1 件 = 1)。
pub struct RenameMigrationReport {
    pub rows: usize,
    pub errors: Vec<String>,
    /// worker が panic した (= 残りのストアを試行しないまま中断した) 場合 true。
    /// per-store エラー (全ストア試行済み・best-effort 確定) と違い、ジャーナルに残して
    /// 次回起動で冪等に再実行する (Sol 角度⑤検収)。
    pub panicked: bool,
}

/// 未完了移行のジャーナルファイル名 (data_dir 直下)。
///
/// リネーム移行は in-memory FIFO で直列実行されるため、通常終了・クラッシュでキュー / 実行中
/// ジョブが失われると、ファイルは新名なのにメタデータが旧キーに取り残される
/// (角度⑤ Sol/Terra P1)。そこで状態変化ごとに完全 snapshot を latest-value writer へ送り、
/// **report を受信できたジョブだけ**消し込む。通常終了は最新 revision まで flush する。
/// 起動時に残エントリを再実行すれば、クラッシュで一部ストアだけ
/// commit された移行も冪等性 (UPDATE OR IGNORE + DELETE / 存在確認付き sidecar 改名)
/// により安全に完走する。ジャーナルは「移行が少なくとも 1 回走ること」を保証するもので、
/// per-store エラーの再試行はしない (通常経路と同じ best-effort)。
pub const JOURNAL_FILE: &str = "rename_migration_journal.json";

/// ジャーナルを読み込む (無い / 壊れている場合は空)。
pub fn journal_load(data_dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    let path = data_dir.join(JOURNAL_FILE);
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    match serde_json::from_slice::<Vec<(PathBuf, PathBuf)>>(&bytes) {
        Ok(entries) => entries,
        Err(e) => {
            crate::logger::log(format!("[RENAME-MIG] journal parse failed (discard): {e}"));
            Vec::new()
        }
    }
}

/// ジャーナルを書き出す (temp + rename の atomic 置換、空なら削除)。best-effort:
/// 失敗はログのみ (移行自体は続行する。ジャーナルはクラッシュ回復の追加保険)。
pub fn journal_save(data_dir: &Path, entries: &[(PathBuf, PathBuf)]) {
    let path = data_dir.join(JOURNAL_FILE);
    if entries.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let result = (|| -> std::io::Result<()> {
        let json = serde_json::to_vec(entries)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    })();
    if let Err(e) = result {
        crate::logger::log(format!("[RENAME-MIG] journal save failed: {e}"));
    }
}

/// Single background owner for rename-migration journal writes. The UI publishes complete
/// latest-value snapshots, and one worker performs all filesystem I/O in revision order.
pub(crate) struct RenameMigrationJournalWriter {
    shared: JournalWriterShared,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl RenameMigrationJournalWriter {
    pub(crate) fn spawn() -> Self {
        let shared = Arc::new((Mutex::new(JournalWriteState::default()), Condvar::new()));
        let copy = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("rename-migration-journal".into())
            .spawn(move || run_journal_writer(copy))
            .ok();
        if handle.is_none() {
            shared.as_ref().0.lock().unwrap().worker_stopped = true;
            shared.as_ref().1.notify_all();
            crate::logger::log("[RENAME-MIG] journal writer spawn failed");
        }
        Self { shared, handle }
    }

    pub(crate) fn enqueue(&self, data_dir: PathBuf, entries: Vec<(PathBuf, PathBuf)>) {
        let (state, cv) = self.shared.as_ref();
        let mut state = state.lock().unwrap();
        if state.worker_stopped || state.shutdown {
            crate::logger::log("[RENAME-MIG] journal writer unavailable; snapshot not saved");
            return;
        }
        state.enqueue(data_dir, entries);
        cv.notify_one();
    }

    pub(crate) fn flush(&self) {
        let (state, cv) = self.shared.as_ref();
        let mut state = state.lock().unwrap();
        let target = state.next_revision;
        while !state.is_flushed(target) && !state.worker_stopped {
            state = cv.wait(state).unwrap();
        }
        if !state.is_flushed(target) {
            crate::logger::log(format!(
                "[RENAME-MIG] journal writer stopped before flush revision {target}"
            ));
        }
    }
}

impl Drop for RenameMigrationJournalWriter {
    fn drop(&mut self) {
        self.flush();
        {
            let (state, cv) = self.shared.as_ref();
            state.lock().unwrap().shutdown = true;
            cv.notify_all();
        }
        if let Some(handle) = self.handle.take() {
            if handle.join().is_err() {
                crate::logger::log("[RENAME-MIG] journal writer panicked during shutdown");
            }
        }
    }
}

struct JournalWriterStopGuard(JournalWriterShared);

impl Drop for JournalWriterStopGuard {
    fn drop(&mut self) {
        let (state, cv) = self.0.as_ref();
        if let Ok(mut state) = state.lock() {
            state.worker_stopped = true;
            cv.notify_all();
        }
    }
}

fn run_journal_writer(shared: JournalWriterShared) {
    let _stop_guard = JournalWriterStopGuard(Arc::clone(&shared));
    loop {
        let snapshot = {
            let (state, cv) = shared.as_ref();
            let mut state = state.lock().unwrap();
            while state.latest.is_none() && !state.shutdown {
                state = cv.wait(state).unwrap();
            }
            match state.take_latest() {
                Some(snapshot) => snapshot,
                None if state.shutdown => return,
                None => continue,
            }
        };
        journal_save(&snapshot.data_dir, &snapshot.entries);
        let (state, cv) = shared.as_ref();
        state.lock().unwrap().complete(snapshot.revision);
        cv.notify_all();
    }
}

/// path-keyed SQLite ストアのキー正規化規則。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoreKeyNormalization {
    /// [`crate::adjustment_db::normalize_path`] / `path_key::normalize_keep_drive`。
    KeepDrive,
    /// [`crate::path_key::normalize`]。USB 等のドライブレターを除去する軽量設定用。
    DriveStripped,
}

/// リネームと mIV 内削除成功時 hard purge が共有する path-keyed SQLite 記述子。
///
/// `unique` は rename の衝突処理にだけ使う。purge は全行を素の `DELETE` にする。
/// `rename_generic=false` は raw `path` 列も同時更新する閲覧履歴だけで、rename 側は専用処理を
/// 使うが purge 側は同じ記述子を使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StoreDescriptor {
    pub(crate) file: &'static str,
    pub(crate) table: &'static str,
    pub(crate) column: &'static str,
    pub(crate) unique: bool,
    pub(crate) normalization: StoreKeyNormalization,
    pub(crate) rename_generic: bool,
}

const fn store(
    file: &'static str,
    table: &'static str,
    column: &'static str,
    unique: bool,
    normalization: StoreKeyNormalization,
) -> StoreDescriptor {
    StoreDescriptor {
        file,
        table,
        column,
        unique,
        normalization,
        rename_generic: true,
    }
}

/// path-keyed SQLite ストアの正本。
///
/// 新しい rename 対象ストアを追加するときは必ずここへ足すこと。この同じ表を削除 worker の
/// hard purge も走査するため、rename と purge の対象が将来ずれない。PDF パスワードだけは
/// JSON の SHA-256 キーなので、この表と並ぶ専用処理を両経路が共有する。
pub(crate) const STORES: &[StoreDescriptor] = &[
    store(
        "rating.db",
        "ratings",
        "path",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "content_identity.db",
        "edit_origin",
        "file_key",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "adjustment.db",
        "page_params",
        "page_path",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "adjustment.db",
        "sidecar_sync",
        "folder_key",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "mask.db",
        "masks",
        "path",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "conceal.db",
        "conceal_entries",
        "page_path",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "local_adjust.db",
        "local_adjust_pages",
        "page_path",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "comic.db",
        "comic_entries",
        "page_path",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "export_crop.db",
        "export_crop_pages",
        "page_path",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "edit_preview_cache.db",
        "edit_previews",
        "item_key",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "tags.db",
        "item_tags",
        "item_key",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "tags.db",
        "tag_item_state",
        "item_key",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "tags.db",
        "tag_sidecar_sync",
        "folder_key",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "rotation.db",
        "rotations",
        "path",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "view_trim.db",
        "view_trim_pages",
        "page_path",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "video_pins.db",
        "video_pins",
        "path",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    // id が PK で path は非一意 (1 ファイル複数ブックマーク)。
    store(
        "video_bookmarks.db",
        "video_bookmarks",
        "path",
        false,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "folder_thumb_pins.db",
        "folder_thumb_pins",
        "container_key",
        true,
        StoreKeyNormalization::KeepDrive,
    ),
    store(
        "book_resume.db",
        "book_resume",
        "path",
        true,
        StoreKeyNormalization::DriveStripped,
    ),
    store(
        "spread.db",
        "spreads",
        "path",
        true,
        StoreKeyNormalization::DriveStripped,
    ),
    store(
        "view_trim.db",
        "view_trim_books",
        "book_key",
        true,
        StoreKeyNormalization::DriveStripped,
    ),
    StoreDescriptor {
        file: "reading_history.db",
        table: "reading_history",
        column: "key",
        unique: true,
        normalization: StoreKeyNormalization::KeepDrive,
        rename_generic: false,
    },
];

/// リネーム移行の本体 (worker スレッドで呼ぶ)。`old_path` は改名前 (もう存在しない)、
/// `new_path` は改名後の実 path。
pub fn run(old_path: &Path, new_path: &Path) -> RenameMigrationReport {
    run_at(&crate::data_dir::get(), old_path, new_path)
}

/// data_dir を差し替え可能にしたテスト用エントリポイント。
pub fn run_at(data_dir: &Path, old_path: &Path, new_path: &Path) -> RenameMigrationReport {
    let mut report = RenameMigrationReport {
        rows: 0,
        errors: Vec::new(),
        panicked: false,
    };

    // 1. 動画/音声の .xmp サイドカー (タグ・★の実体) をファイルごと改名する。
    //    フォルダ改名では中のサイドカーがフォルダと一緒に移動しているので対象外。
    migrate_sidecar_file(old_path, new_path, &mut report);

    // 2. PDF パスワード (キーが正規化 path の SHA-256 なので UPDATE では移行できない。
    //    平文を読み直して新キーで保存し直す)。
    migrate_pdf_password(old_path, new_path, &mut report);

    // 3. 共通 STORES の generic rename 群 (exact + `/` prefix + `::` prefix)。
    //    削除 hard purge も同じ STORES を使う。追加時は片側だけに別表を作らないこと。
    let old_k = crate::adjustment_db::normalize_path(old_path);
    let new_k = crate::adjustment_db::normalize_path(new_path);
    let old_s = crate::path_key::normalize(old_path);
    let new_s = crate::path_key::normalize(new_path);
    for descriptor in STORES.iter().filter(|store| store.rename_generic) {
        let (old_key, new_key) = match descriptor.normalization {
            StoreKeyNormalization::KeepDrive => (&old_k, &new_k),
            StoreKeyNormalization::DriveStripped => (&old_s, &new_s),
        };
        if old_key == new_key {
            continue;
        }
        migrate_store(
            &data_dir.join(descriptor.file),
            descriptor.table,
            descriptor.column,
            descriptor.unique,
            old_key,
            new_key,
            &mut report,
        );
    }

    // 本ページブックマークは container_key だけでなく raw container_path と、画像本では
    // container 相対 page identity も同時更新する必要があるため generic STORES へは載せない。
    // また missing 行を保持する仕様上、STORES と共有される delete hard purge の対象にも
    // してはならない。専用 transaction で case-only rename も含めて追従させる。
    migrate_book_bookmarks(data_dir, old_path, new_path, &mut report);

    // 5. 閲覧履歴 (exact のみ。raw path 列も更新する)。記述子自体は STORES にあり、
    //    purge は exact + prefix で同じ行を削除する。
    if old_k != new_k {
        migrate_reading_history(data_dir, new_path, &old_k, &new_k, &mut report);
    }

    report
}

fn migrate_book_bookmarks(
    data_dir: &Path,
    old_path: &Path,
    new_path: &Path,
    report: &mut RenameMigrationReport,
) {
    let db_path = data_dir.join("book_bookmarks.db");
    if !db_path.exists() {
        return;
    }
    match crate::book_bookmarks::migrate_paths_at(
        &db_path,
        &[(old_path.to_path_buf(), new_path.to_path_buf())],
    ) {
        Ok(rows) => report.rows += rows,
        Err(error) => report.errors.push(format!("book_bookmarks: {error}")),
    }
}

/// mIV 内削除の成功 path に対応する全 path-keyed メタストア hard purge 結果。
#[derive(Debug, Default)]
pub(crate) struct PurgeReport {
    pub(crate) rows: usize,
    /// SQLite connection open を試みた回数。delete worker の perf 計装用。
    pub(crate) db_open_count: usize,
    pub(crate) errors: Vec<String>,
}

/// `delete_worker` 専用。Shell が削除成功と確認した path だけを hard purge する。
///
/// スキャン・検索・ロード中の missing 判定からは絶対に呼ばない。SQLite ストアは共通
/// [`STORES`] を走査し、exact + `<key>/` + `<key>::` を素の `DELETE` にする。
/// `pdf_paths` は SHA-256 キーの逆引きができない PDF password 用に、worker が削除前に
/// 列挙した実 path 群。
pub(crate) fn purge_removed_paths_at(
    data_dir: &Path,
    removed: &[PathBuf],
    pdf_paths: &[PathBuf],
) -> PurgeReport {
    let mut report = PurgeReport::default();
    if removed.is_empty() {
        return report;
    }

    let keep_drive_keys = normalized_removed_keys(removed, StoreKeyNormalization::KeepDrive);
    let drive_stripped_keys =
        normalized_removed_keys(removed, StoreKeyNormalization::DriveStripped);
    for descriptor in STORES {
        let keys = match descriptor.normalization {
            StoreKeyNormalization::KeepDrive => &keep_drive_keys,
            StoreKeyNormalization::DriveStripped => &drive_stripped_keys,
        };
        purge_store(data_dir, descriptor, keys, &mut report);
    }

    match crate::pdf_passwords::PdfPasswordStore::purge_paths_at(data_dir, pdf_paths) {
        Ok(rows) => report.rows += rows,
        Err(error) => report.errors.push(format!("pdf_passwords.json: {error}")),
    }
    purge_sidecar_backups(removed, &mut report);
    report
}

fn purge_sidecar_backups(removed: &[PathBuf], report: &mut PurgeReport) {
    purge_sidecar_backups_with_flush(removed, report, |sidecar| sidecar.flush());
}

fn purge_sidecar_backups_with_flush<F>(removed: &[PathBuf], report: &mut PurgeReport, mut flush: F)
where
    F: FnMut(&mut crate::sidecar::SidecarFile) -> bool,
{
    let mut roots_by_parent = std::collections::HashMap::<PathBuf, Vec<String>>::new();
    for path in removed {
        let (Some(parent), Some(file_name)) = (path.parent(), path.file_name()) else {
            continue;
        };
        // 親が残る単一ファイル/コンテナ削除だけを安全に更新できる。削除済みフォルダ内の
        // sidecar はごみ箱側へ移動済みで所在を追えない。
        if !parent.is_dir() {
            continue;
        }
        roots_by_parent
            .entry(parent.to_path_buf())
            .or_default()
            .push(file_name.to_string_lossy().to_lowercase());
    }
    for (parent, roots) in roots_by_parent {
        let sidecar_path = parent.join(crate::sidecar::SIDECAR_FILENAME);
        if !sidecar_path.exists() {
            continue;
        }
        let mut sidecar = crate::sidecar::SidecarFile::load(&parent);
        let changed = roots.iter().fold(false, |changed, root| {
            sidecar.purge_deleted_root(root) || changed
        });
        if changed {
            if flush(&mut sidecar) {
                report.rows += 1;
            } else {
                report.errors.push(format!(
                    "{}: sidecar purge flush failed",
                    sidecar_path.display()
                ));
            }
        }
    }
}

fn normalized_removed_keys(
    removed: &[PathBuf],
    normalization: StoreKeyNormalization,
) -> Vec<String> {
    let mut keys = removed
        .iter()
        .map(|path| match normalization {
            StoreKeyNormalization::KeepDrive => crate::adjustment_db::normalize_path(path),
            StoreKeyNormalization::DriveStripped => crate::path_key::normalize(path),
        })
        .filter(|key| !key.is_empty())
        .collect::<Vec<_>>();
    keys.sort();
    keys.dedup();
    keys
}

/// BINARY collation で `prefix` から始まる文字列の排他的 upper bound を返す。
///
/// UTF-8 の辞書順は Unicode code point 順を保つため、最後の scalar value を次へ進めれば
/// `value >= prefix AND value < upper` が prefix 一致と同じ集合になる。最後が `char::MAX`
/// または次が surrogate で scalar value にできない場合は `None` とし、呼び出し側で従来の
/// `substr` 条件へ安全に fallback する。hard purge が渡す prefix は `/` / `:` 終端なので
/// 通常は必ず index range 条件を使える。
fn prefix_upper_bound(prefix: &str) -> Option<String> {
    let (last_index, last) = prefix.char_indices().next_back()?;
    let next = char::from_u32((last as u32).checked_add(1)?)?;
    let mut upper = prefix[..last_index].to_owned();
    upper.push(next);
    Some(upper)
}

fn purge_store(
    data_dir: &Path,
    descriptor: &StoreDescriptor,
    removed_keys: &[String],
    report: &mut PurgeReport,
) {
    let db_path = data_dir.join(descriptor.file);
    if removed_keys.is_empty() || !db_path.exists() {
        return;
    }
    report.db_open_count += 1;
    let result = (|| -> Result<usize, rusqlite::Error> {
        let mut conn = rusqlite::Connection::open(&db_path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let table_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [descriptor.table],
            |row| row.get(0),
        )?;
        if !table_exists {
            return Ok(0);
        }
        let tx = conn.transaction()?;
        let mut changed = 0usize;

        // exact は PK / index を使う IN へまとめる。batch 幅は SQLite の既定 parameter
        // 上限 999 より小さくし、削除数に比例した statement 数を抑える。
        for keys in removed_keys.chunks(PURGE_EXACT_BATCH_SIZE) {
            let placeholders = (0..keys.len()).map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "DELETE FROM {} WHERE {} IN ({placeholders})",
                descriptor.table, descriptor.column
            );
            changed += tx.execute(&sql, rusqlite::params_from_iter(keys.iter()))?;
        }

        // 全 STORES のキー列は既定 BINARY collation で、PK または path index を持つ。
        // substr(col, ...) を列へ適用せず、同じ collation 上の range scan にする。
        let range_sql = format!(
            "DELETE FROM {} WHERE {} >= ?1 AND {} < ?2",
            descriptor.table, descriptor.column, descriptor.column
        );
        let fallback_sql = format!(
            "DELETE FROM {} WHERE substr({}, 1, ?1) = ?2",
            descriptor.table, descriptor.column
        );
        {
            let mut range_statement = tx.prepare(&range_sql)?;
            let mut fallback_statement = tx.prepare(&fallback_sql)?;
            for key in removed_keys {
                for prefix in [format!("{key}/"), format!("{key}::")] {
                    if let Some(upper) = prefix_upper_bound(&prefix) {
                        changed += range_statement.execute(rusqlite::params![prefix, upper])?;
                    } else {
                        changed += fallback_statement
                            .execute(rusqlite::params![prefix.chars().count() as i64, prefix,])?;
                    }
                }
            }
        }
        tx.commit()?;
        Ok(changed)
    })();
    match result {
        Ok(rows) => report.rows += rows,
        Err(error) => report.errors.push(format!(
            "{}: {}.{}: {error}",
            descriptor.file, descriptor.table, descriptor.column
        )),
    }
}

/// 1 ストア分の移行: exact + `<old>/` prefix + `<old>::` prefix。
fn migrate_store(
    db_path: &Path,
    table: &str,
    col: &str,
    unique: bool,
    old_key: &str,
    new_key: &str,
    report: &mut RenameMigrationReport,
) {
    if !db_path.exists() {
        return;
    }
    let result = (|| -> Result<usize, rusqlite::Error> {
        let mut conn = rusqlite::Connection::open(db_path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let tx = conn.transaction()?;
        let mut changed = 0usize;
        changed += move_exact(&tx, table, col, unique, old_key, new_key)?;
        changed += move_prefix(
            &tx,
            table,
            col,
            unique,
            &format!("{old_key}/"),
            &format!("{new_key}/"),
        )?;
        changed += move_prefix(
            &tx,
            table,
            col,
            unique,
            &format!("{old_key}::"),
            &format!("{new_key}::"),
        )?;
        // rating.db はキーから導出される source_path 列 (一覧ビューがコンテナを開くのに
        // 使う) も新キーに合わせる (`RatingDb::copy_entry_key` と同じ導出規則 =
        // "::" より前、無ければキー自身)。
        if table == "ratings" && changed > 0 {
            tx.execute(
                "UPDATE ratings SET source_path = CASE
                     WHEN instr(path, '::') > 0 THEN substr(path, 1, instr(path, '::') - 1)
                     ELSE path
                 END
                 WHERE path = ?1
                    OR substr(path, 1, ?2) = ?3
                    OR substr(path, 1, ?4) = ?5",
                rusqlite::params![
                    new_key,
                    format!("{new_key}/").chars().count() as i64,
                    format!("{new_key}/"),
                    format!("{new_key}::").chars().count() as i64,
                    format!("{new_key}::"),
                ],
            )?;
        }
        tx.commit()?;
        Ok(changed)
    })();
    match result {
        Ok(n) => report.rows += n,
        Err(e) => report.errors.push(format!(
            "{}: {table}.{col}: {e}",
            db_path.file_name().unwrap_or_default().to_string_lossy()
        )),
    }
}

/// exact キーの移動。一意キーは `UPDATE OR IGNORE` + 旧行 `DELETE` (新キー優先)、
/// 非一意キー (video_bookmarks) は素の UPDATE (衝突が起きない)。
fn move_exact(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    col: &str,
    unique: bool,
    old_key: &str,
    new_key: &str,
) -> Result<usize, rusqlite::Error> {
    let changed = if unique {
        let n = tx.execute(
            &format!("UPDATE OR IGNORE {table} SET {col} = ?1 WHERE {col} = ?2"),
            rusqlite::params![new_key, old_key],
        )?;
        tx.execute(&format!("DELETE FROM {table} WHERE {col} = ?1"), [old_key])?;
        n
    } else {
        tx.execute(
            &format!("UPDATE {table} SET {col} = ?1 WHERE {col} = ?2"),
            rusqlite::params![new_key, old_key],
        )?
    };
    Ok(changed)
}

/// prefix キーの移動。対象キーを `substr` 等値で列挙してから 1 行ずつ付け替える
/// (LIKE を使わないのは path 中の `%` / `_` をワイルドカード扱いさせないため。
/// substr の長さ引数は SQLite では文字数なので `chars().count()` を渡す)。
fn move_prefix(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    col: &str,
    unique: bool,
    old_prefix: &str,
    new_prefix: &str,
) -> Result<usize, rusqlite::Error> {
    let keys: Vec<String> = {
        let mut stmt = tx.prepare(&format!(
            "SELECT DISTINCT {col} FROM {table} WHERE substr({col}, 1, ?1) = ?2"
        ))?;
        let rows = stmt.query_map(
            rusqlite::params![old_prefix.chars().count() as i64, old_prefix],
            |r| r.get::<_, String>(0),
        )?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut changed = 0usize;
    for key in keys {
        let Some(suffix) = key.strip_prefix(old_prefix) else {
            continue;
        };
        let new_key = format!("{new_prefix}{suffix}");
        changed += move_exact(tx, table, col, unique, &key, &new_key)?;
    }
    Ok(changed)
}

/// 動画/音声の .xmp サイドカーをファイルごと改名する。新側に既にサイドカーがある場合は
/// 新側を優先して旧側を残す (上書きしない。孤児はログのみ)。
fn migrate_sidecar_file(old_path: &Path, new_path: &Path, report: &mut RenameMigrationReport) {
    if new_path.is_dir() {
        return;
    }
    let old_sidecar = crate::xmp_writer::sidecar_path_for(old_path);
    if !old_sidecar.exists() {
        return;
    }
    let new_sidecar = crate::xmp_writer::sidecar_path_for(new_path);
    if new_sidecar.exists() {
        crate::logger::log(format!(
            "[RENAME-MIG] sidecar already exists at new path, keeping both: {}",
            new_sidecar.display()
        ));
        return;
    }
    match std::fs::rename(&old_sidecar, &new_sidecar) {
        Ok(()) => report.rows += 1,
        Err(e) => report.errors.push(format!("sidecar: {e}")),
    }
}

/// PDF パスワードの引き継ぎ (単一 PDF の改名のみ)。
fn migrate_pdf_password(old_path: &Path, new_path: &Path, report: &mut RenameMigrationReport) {
    let is_pdf = old_path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if !is_pdf || new_path.is_dir() {
        return;
    }
    let mut store = crate::pdf_passwords::PdfPasswordStore::load();
    if let Some(password) = store.get(old_path) {
        store.set(new_path, &password);
        store.remove(old_path);
        store.save();
        report.rows += 1;
    }
}

/// 閲覧履歴の exact 移行。key (正規化) と path (raw) の両方を新 path へ更新する。
/// title は次回オープン時の upsert で自然に新名へ更新されるため触らない。
fn migrate_reading_history(
    data_dir: &Path,
    new_path: &Path,
    old_key: &str,
    new_key: &str,
    report: &mut RenameMigrationReport,
) {
    let db_path = data_dir.join("reading_history.db");
    if !db_path.exists() {
        return;
    }
    let result = (|| -> Result<usize, rusqlite::Error> {
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let n = conn.execute(
            "UPDATE OR IGNORE reading_history SET key = ?1, path = ?2 WHERE key = ?3",
            rusqlite::params![new_key, new_path.to_string_lossy(), old_key],
        )?;
        conn.execute("DELETE FROM reading_history WHERE key = ?1", [old_key])?;
        Ok(n)
    })();
    match result {
        Ok(n) => report.rows += n,
        Err(e) => report.errors.push(format!("reading_history: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn open(dir: &Path, file: &str) -> rusqlite::Connection {
        rusqlite::Connection::open(dir.join(file)).unwrap()
    }

    fn sorted_keys(connection: &rusqlite::Connection, table: &str) -> Vec<String> {
        let mut statement = connection
            .prepare(&format!("SELECT path FROM {table} ORDER BY path"))
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn prefix_upper_bound_advances_the_last_unicode_scalar() {
        assert_eq!(prefix_upper_bound("a/b/").as_deref(), Some("a/b0"));
        assert_eq!(prefix_upper_bound("a.zip::").as_deref(), Some("a.zip:;"));
        assert_eq!(prefix_upper_bound("本").as_deref(), Some("札"));
        assert_eq!(prefix_upper_bound(""), None);
        assert_eq!(prefix_upper_bound("\u{d7ff}"), None, "surrogate gap");
        assert_eq!(prefix_upper_bound("\u{10ffff}"), None, "char::MAX");
    }

    #[test]
    fn exact_and_prefix_query_plans_use_the_binary_primary_key_index() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE entries (path TEXT PRIMARY KEY, value INTEGER)")
            .unwrap();

        for (sql, parameters) in [
            (
                "EXPLAIN QUERY PLAN DELETE FROM entries WHERE path IN (?1, ?2)",
                vec!["a", "b"],
            ),
            (
                "EXPLAIN QUERY PLAN DELETE FROM entries WHERE path >= ?1 AND path < ?2",
                vec!["a/", "a0"],
            ),
        ] {
            let mut statement = connection.prepare(sql).unwrap();
            let plan = statement
                .query_map(rusqlite::params_from_iter(parameters), |row| {
                    row.get::<_, String>(3)
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join(" | ");
            assert!(plan.contains("SEARCH"), "{sql}: {plan}");
            assert!(!plan.contains("SCAN"), "{sql}: {plan}");
        }
    }

    #[test]
    fn range_purge_matches_legacy_substr_for_exact_folder_and_container_keys() {
        let dir = tempfile::tempdir().unwrap();
        let connection = open(dir.path(), "equivalence.db");
        connection
            .execute_batch(
                "CREATE TABLE legacy_rows (path TEXT PRIMARY KEY);
                 CREATE TABLE range_rows (path TEXT PRIMARY KEY);",
            )
            .unwrap();
        let values = [
            "c:/root/a/b.jpg",
            "c:/root/a/b",
            "c:/root/a/b/c.jpg",
            "c:/root/a/b/sub/d.jpg",
            "c:/root/a/b2/keep.jpg",
            "c:/root/a.zip",
            "c:/root/a.zip::x",
            "c:/root/a.zip::dir/y",
            "c:/root/a.zip:;keep",
            "c:/root/100%_本",
            "c:/root/100%_本/child.jpg",
            "c:/root/100x_本/keep.jpg",
        ];
        for value in values {
            connection
                .execute("INSERT INTO legacy_rows(path) VALUES (?1)", [value])
                .unwrap();
            connection
                .execute("INSERT INTO range_rows(path) VALUES (?1)", [value])
                .unwrap();
        }
        let removed_keys = vec![
            "c:/root/a/b.jpg".to_owned(),
            "c:/root/a/b".to_owned(),
            "c:/root/a.zip".to_owned(),
            "c:/root/100%_本".to_owned(),
        ];

        let legacy_sql = "DELETE FROM legacy_rows WHERE path = ?1
                          OR substr(path, 1, ?2) = ?3
                          OR substr(path, 1, ?4) = ?5";
        let mut legacy_changed = 0usize;
        for key in &removed_keys {
            let folder_prefix = format!("{key}/");
            let container_prefix = format!("{key}::");
            legacy_changed += connection
                .execute(
                    legacy_sql,
                    rusqlite::params![
                        key,
                        folder_prefix.chars().count() as i64,
                        folder_prefix,
                        container_prefix.chars().count() as i64,
                        container_prefix,
                    ],
                )
                .unwrap();
        }

        let descriptor = StoreDescriptor {
            file: "equivalence.db",
            table: "range_rows",
            column: "path",
            unique: true,
            normalization: StoreKeyNormalization::KeepDrive,
            rename_generic: true,
        };
        let mut report = PurgeReport::default();
        purge_store(dir.path(), &descriptor, &removed_keys, &mut report);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.rows, legacy_changed);
        assert_eq!(
            sorted_keys(&connection, "range_rows"),
            sorted_keys(&connection, "legacy_rows")
        );
        assert_eq!(
            sorted_keys(&connection, "range_rows"),
            [
                "c:/root/100x_本/keep.jpg",
                "c:/root/a.zip:;keep",
                "c:/root/a/b2/keep.jpg"
            ]
        );
    }

    #[test]
    fn index_fast_purge_handles_27000_rows_and_1000_removed_keys_within_seconds() {
        const TOTAL_ROWS: usize = 27_000;
        const REMOVED_ROWS: usize = 1_000;

        let dir = tempfile::tempdir().unwrap();
        let mut connection = open(dir.path(), "perf.db");
        connection
            .execute_batch("CREATE TABLE entries (path TEXT PRIMARY KEY, value INTEGER)")
            .unwrap();
        let removed_keys = (0..REMOVED_ROWS)
            .map(|index| format!("c:/removed/{index:04}.jpg"))
            .collect::<Vec<_>>();
        {
            let transaction = connection.transaction().unwrap();
            {
                let mut insert = transaction
                    .prepare("INSERT INTO entries(path, value) VALUES (?1, ?2)")
                    .unwrap();
                for (index, key) in removed_keys.iter().enumerate() {
                    insert
                        .execute(rusqlite::params![key, index as i64])
                        .unwrap();
                }
                for index in REMOVED_ROWS..TOTAL_ROWS {
                    insert
                        .execute(rusqlite::params![
                            format!("c:/kept/{index:05}.jpg"),
                            index as i64
                        ])
                        .unwrap();
                }
            }
            transaction.commit().unwrap();
        }
        drop(connection);

        let descriptor = StoreDescriptor {
            file: "perf.db",
            table: "entries",
            column: "path",
            unique: true,
            normalization: StoreKeyNormalization::KeepDrive,
            rename_generic: true,
        };
        let started = std::time::Instant::now();
        let mut report = PurgeReport::default();
        purge_store(dir.path(), &descriptor, &removed_keys, &mut report);
        let elapsed = started.elapsed();

        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.rows, REMOVED_ROWS);
        let connection = open(dir.path(), "perf.db");
        let remaining: usize = connection
            .query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, TOTAL_ROWS - REMOVED_ROWS);
        eprintln!(
            "index-fast purge: total_rows={TOTAL_ROWS} removed_keys={REMOVED_ROWS} elapsed_ms={:.1}",
            elapsed.as_secs_f64() * 1000.0
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "27k rows / 1k keys purge took {elapsed:?}"
        );
    }

    #[test]
    fn shared_store_list_hard_purge_covers_exact_folder_and_container_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let removed = PathBuf::from(r"C:\Root\Gone");
        for descriptor in STORES {
            let conn = open(dir.path(), descriptor.file);
            conn.execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {} ({} TEXT)",
                descriptor.table, descriptor.column
            ))
            .unwrap();
            let base = match descriptor.normalization {
                StoreKeyNormalization::KeepDrive => crate::adjustment_db::normalize_path(&removed),
                StoreKeyNormalization::DriveStripped => crate::path_key::normalize(&removed),
            };
            for key in [
                base.clone(),
                format!("{base}/child.jpg"),
                format!("{base}::page_1"),
                format!("{base}2/keep.jpg"),
            ] {
                conn.execute(
                    &format!(
                        "INSERT INTO {} ({}) VALUES (?1)",
                        descriptor.table, descriptor.column
                    ),
                    [key],
                )
                .unwrap();
            }
        }

        let pdf_exact = PathBuf::from(r"C:\Root\Gone.pdf");
        let pdf_nested = PathBuf::from(r"C:\Root\Gone\nested.pdf");
        let pdf_keep = PathBuf::from(r"C:\Root\Gone2\keep.pdf");
        let password_entries = serde_json::json!({
            crate::pdf_passwords::PdfPasswordStore::path_hash(&pdf_exact): "exact",
            crate::pdf_passwords::PdfPasswordStore::path_hash(&pdf_nested): "nested",
            crate::pdf_passwords::PdfPasswordStore::path_hash(&pdf_keep): "keep",
        });
        std::fs::write(
            dir.path().join("pdf_passwords.json"),
            serde_json::to_vec(&password_entries).unwrap(),
        )
        .unwrap();

        let report = purge_removed_paths_at(
            dir.path(),
            std::slice::from_ref(&removed),
            &[pdf_exact, pdf_nested],
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.rows, STORES.len() * 3 + 2);

        for descriptor in STORES {
            let conn = open(dir.path(), descriptor.file);
            let remaining: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM {}", descriptor.table),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                remaining, 1,
                "{}.{} must retain only the adjacent prefix",
                descriptor.table, descriptor.column
            );
        }
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("pdf_passwords.json")).unwrap())
                .unwrap();
        assert_eq!(saved.as_object().unwrap().len(), 1);
        assert!(
            saved
                .get(crate::pdf_passwords::PdfPasswordStore::path_hash(&pdf_keep))
                .is_some()
        );
    }

    #[test]
    fn rating_count_drops_immediately_after_shared_hard_purge() {
        let dir = tempfile::tempdir().unwrap();
        let path = PathBuf::from(r"C:\Pics\rated.jpg");
        let key = crate::adjustment_db::normalize_path(&path);
        let db_path = dir.path().join("rating.db");
        let db = crate::rating_db::RatingDb::open_at(&db_path).unwrap();
        db.set(&key, 5).unwrap();
        assert_eq!(db.count_by_stars().unwrap()[5], 1);

        let report = purge_removed_paths_at(dir.path(), &[path], &[]);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(db.count_by_stars().unwrap()[5], 0);
        assert_eq!(db.count(), 0);
    }

    #[test]
    fn failed_sidecar_flush_is_not_counted_as_a_purged_row() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        std::fs::create_dir(&media).unwrap();
        let removed = media.join("gone.jpg");
        let sidecar_path = media.join(crate::sidecar::SIDECAR_FILENAME);
        std::fs::write(
            &sidecar_path,
            br#"{"version":1,"items":{"gone.jpg":{"tags":["old"]}}}"#,
        )
        .unwrap();
        let mut report = PurgeReport::default();
        purge_sidecar_backups_with_flush(&[removed], &mut report, |_sidecar| false);
        assert_eq!(report.rows, 0, "failed sidecar write is not a success row");
        assert_eq!(report.errors.len(), 1);
        assert!(
            std::fs::read_to_string(&sidecar_path)
                .unwrap()
                .contains("gone.jpg"),
            "failed flush must leave the previous sidecar intact"
        );
    }

    /// An in-flight old snapshot completes first; queued intermediate values coalesce to newest.
    #[test]
    fn journal_writer_state_keeps_only_newest_waiting_snapshot() {
        let mut state = JournalWriteState::default();
        let dir = PathBuf::from("data");
        let rev1 = state.enqueue(dir.clone(), vec![("a".into(), "b".into())]);
        let first = state.take_latest().unwrap();
        let _rev2 = state.enqueue(dir.clone(), vec![("b".into(), "c".into())]);
        let rev3 = state.enqueue(dir, vec![("c".into(), "d".into())]);

        state.complete(first.revision);
        assert_eq!(first.revision, rev1);
        assert!(!state.is_flushed(rev3));
        let newest = state.take_latest().unwrap();
        assert_eq!(newest.revision, rev3);
        assert_eq!(newest.entries, vec![("c".into(), "d".into())]);
        state.complete(newest.revision);
        assert!(state.is_flushed(rev3));
    }

    #[test]
    fn journal_writer_drop_flushes_latest_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let entries = vec![(PathBuf::from("old"), PathBuf::from("new"))];
        let writer = RenameMigrationJournalWriter::spawn();
        writer.enqueue(dir.path().to_path_buf(), entries.clone());
        drop(writer);
        assert_eq!(journal_load(dir.path()), entries);
    }

    /// ジャーナルの往復と消し込み (空で削除・無ければ空・壊れていたら破棄)。
    #[test]
    fn journal_roundtrip_and_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        assert!(journal_load(dir.path()).is_empty(), "無ければ空");
        let entries = vec![
            (PathBuf::from(r"D:\a.jpg"), PathBuf::from(r"D:\b.jpg")),
            (
                PathBuf::from(r"D:\フォルダ"),
                PathBuf::from(r"D:\新フォルダ"),
            ),
        ];
        journal_save(dir.path(), &entries);
        assert_eq!(journal_load(dir.path()), entries, "往復で一致");
        journal_save(dir.path(), &[]);
        assert!(
            !dir.path().join(JOURNAL_FILE).exists(),
            "空になったらファイルごと削除"
        );
        std::fs::write(dir.path().join(JOURNAL_FILE), b"broken json").unwrap();
        assert!(journal_load(dir.path()).is_empty(), "壊れていたら空で続行");
    }

    /// 連続リネーム A→B→C は **実行順どおり**なら C に集約される。逆順で実行すると
    /// B に取り残される (= App 側で FIFO 直列化が必須である根拠。Sol rename-mig P1)。
    #[test]
    fn sequential_chained_renames_require_fifo_ordering() {
        let dir = tempfile::tempdir().unwrap();
        let a = PathBuf::from(r"D:\Pics\a.jpg");
        let b = PathBuf::from(r"D:\Pics\b.jpg");
        let c = PathBuf::from(r"D:\Pics\c.jpg");
        let key = |p: &PathBuf| crate::adjustment_db::normalize_path(p);
        let setup = |stars: i64| {
            let conn = open(dir.path(), "rotation.db");
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS rotations (path TEXT PRIMARY KEY, angle INTEGER NOT NULL DEFAULT 0)",
            )
            .unwrap();
            conn.execute("DELETE FROM rotations", []).unwrap();
            conn.execute(
                "INSERT INTO rotations (path, angle) VALUES (?1, ?2)",
                rusqlite::params![key(&a), stars],
            )
            .unwrap();
        };

        // 実行順どおり (A→B → B→C) なら C に届く。
        setup(90);
        let _ = run_at(dir.path(), &a, &b);
        let _ = run_at(dir.path(), &b, &c);
        let conn = open(dir.path(), "rotation.db");
        let angle: i64 = conn
            .query_row(
                "SELECT angle FROM rotations WHERE path = ?1",
                [key(&c)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(angle, 90, "順序どおりなら最終 path に集約される");
        drop(conn);

        // 逆順 (B→C が先に走る) だと B で取り残される = FIFO が必要な理由。
        setup(180);
        let _ = run_at(dir.path(), &b, &c);
        let _ = run_at(dir.path(), &a, &b);
        let conn = open(dir.path(), "rotation.db");
        let stranded: i64 = conn
            .query_row(
                "SELECT angle FROM rotations WHERE path = ?1",
                [key(&b)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stranded, 180, "逆順実行では中間 path に取り残される");
    }

    #[test]
    fn rename_migrates_book_bookmark_but_hard_purge_keeps_missing_row() {
        let dir = tempfile::tempdir().unwrap();
        let old = PathBuf::from(r"D:\Books\old.cbz");
        let new = PathBuf::from(r"D:\Books\new.cbz");
        let db_path = dir.path().join("book_bookmarks.db");
        crate::book_bookmarks::ensure_schema_at(&db_path).unwrap();
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute(
                "INSERT INTO book_bookmarks
                    (container_key, container_path, container_kind, page_kind, page_value,
                     page_key, page_index_hint, created_at_ms, title)
                 VALUES (?1, ?2, 'zip', 'archive_entry', 'chapter/001.jpg',
                         'chapter/001.jpg', 0, 1, '表紙')",
                rusqlite::params![
                    crate::book_bookmarks::container_key(&old),
                    old.to_string_lossy().as_ref()
                ],
            )
            .unwrap();
        }

        let report = run_at(dir.path(), &old, &new);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let (key, path, title): (String, String, String) = conn
            .query_row(
                "SELECT container_key, container_path, title FROM book_bookmarks",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(key, crate::book_bookmarks::container_key(&new));
        assert_eq!(PathBuf::from(path), new);
        assert_eq!(title, "表紙");
        drop(conn);

        let purge = purge_removed_paths_at(dir.path(), &[new], &[]);
        assert!(purge.errors.is_empty(), "{:?}", purge.errors);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM book_bookmarks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 1, "missing bookmarks remain user-deletable");
    }

    /// view_trim.db の両テーブル (keep-drive の page / drive 除去の book) が移行される。
    #[test]
    fn migrates_view_trim_page_and_book_keys() {
        let dir = tempfile::tempdir().unwrap();
        let old = PathBuf::from(r"D:\Comics\旧.zip");
        let new = PathBuf::from(r"D:\Comics\新.zip");
        {
            let conn = open(dir.path(), "view_trim.db");
            conn.execute_batch(
                "CREATE TABLE view_trim_books (book_key TEXT PRIMARY KEY, state_json TEXT NOT NULL,
                    updated_at INTEGER NOT NULL DEFAULT (unixepoch()));
                 CREATE TABLE view_trim_pages (page_path TEXT PRIMARY KEY, override_json TEXT NOT NULL,
                    updated_at INTEGER NOT NULL DEFAULT (unixepoch()));",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO view_trim_books (book_key, state_json) VALUES (?1, '{}')",
                [crate::path_key::normalize(&old)],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO view_trim_pages (page_path, override_json) VALUES (?1, '{}')",
                [format!(
                    "{}::p1.jpg",
                    crate::adjustment_db::normalize_path(&old)
                )],
            )
            .unwrap();
        }

        let report = run_at(dir.path(), &old, &new);
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        let conn = open(dir.path(), "view_trim.db");
        let books: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM view_trim_books WHERE book_key = ?1",
                [crate::path_key::normalize(&new)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(books, 1, "本単位の表示トリム設定が移る (drive 除去キー)");
        let pages: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM view_trim_pages WHERE page_path = ?1",
                [format!(
                    "{}::p1.jpg",
                    crate::adjustment_db::normalize_path(&new)
                )],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pages, 1, "ページ上書きも移る (keep-drive キー)");
    }

    /// 単一ファイル改名: ★ (rated_at / source_path 込み)・タグ・回転・複数ブックマークが
    /// 新キーへ移り、新キー側の既存行が優先される。再実行は no-op (冪等)。
    #[test]
    fn migrates_exact_file_keys_across_stores() {
        let dir = tempfile::tempdir().unwrap();
        let old = PathBuf::from(r"D:\Media\Old Name.mp4");
        let new = PathBuf::from(r"D:\Media\New Name.mp4");
        let old_k = crate::adjustment_db::normalize_path(&old);
        let new_k = crate::adjustment_db::normalize_path(&new);

        {
            let conn = open(dir.path(), "rating.db");
            conn.execute_batch(
                "CREATE TABLE ratings (path TEXT PRIMARY KEY, stars INTEGER NOT NULL,
                    rated_at_ms INTEGER, source_path TEXT, kind INTEGER, entry_name TEXT,
                    page_num INTEGER, dir_prefix TEXT, archive_format TEXT,
                    zipdir_is_archive INTEGER, zipdir_representative TEXT)",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO ratings (path, stars, rated_at_ms, source_path)
                 VALUES (?1, 4, 111, ?1)",
                [&old_k],
            )
            .unwrap();
        }
        {
            let conn = open(dir.path(), "content_identity.db");
            conn.execute_batch(
                "CREATE TABLE edit_origin (
                    file_key TEXT PRIMARY KEY,
                    size INTEGER NOT NULL,
                    head_hash TEXT NOT NULL,
                    full_hash TEXT,
                    hashed_mtime INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    last_edit_at INTEGER NOT NULL)",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO edit_origin
                    (file_key, size, head_hash, full_hash, hashed_mtime, kind, last_edit_at)
                 VALUES (?1, 10, 'head', 'full', 20, 'image', 30)",
                [&old_k],
            )
            .unwrap();
        }
        {
            let conn = open(dir.path(), "tags.db");
            conn.execute_batch(
                "CREATE TABLE item_tags (item_key TEXT NOT NULL, tag TEXT NOT NULL,
                    tag_key TEXT NOT NULL, applied_at INTEGER NOT NULL,
                    PRIMARY KEY(item_key, tag_key));
                 CREATE TABLE tag_item_state (item_key TEXT PRIMARY KEY,
                    decided_at INTEGER NOT NULL, source TEXT NOT NULL);
                 CREATE TABLE tag_sidecar_sync (folder_key TEXT PRIMARY KEY,
                    sidecar_mtime INTEGER NOT NULL);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO item_tags (item_key, tag, tag_key, applied_at) VALUES (?1, '#原神', '原神', 1)",
                [&old_k],
            )
            .unwrap();
            // 新キー側に別タグが既にある (改名後に先へ付けた想定) → 両立する。
            conn.execute(
                "INSERT INTO item_tags (item_key, tag, tag_key, applied_at) VALUES (?1, '#風景', '風景', 2)",
                [&new_k],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO tag_item_state (item_key, decided_at, source) VALUES (?1, 1, 'user')",
                [&old_k],
            )
            .unwrap();
        }
        {
            let conn = open(dir.path(), "rotation.db");
            conn.execute_batch(
                "CREATE TABLE rotations (path TEXT PRIMARY KEY, angle INTEGER NOT NULL DEFAULT 0)",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO rotations (path, angle) VALUES (?1, 90)",
                [&old_k],
            )
            .unwrap();
        }
        {
            let conn = open(dir.path(), "video_bookmarks.db");
            conn.execute_batch(
                "CREATE TABLE video_bookmarks (id INTEGER PRIMARY KEY AUTOINCREMENT,
                    path TEXT NOT NULL, pts_secs REAL NOT NULL, title TEXT,
                    thumb_webp BLOB, created_at INTEGER NOT NULL)",
            )
            .unwrap();
            for pts in [1.0_f64, 2.0] {
                conn.execute(
                    "INSERT INTO video_bookmarks (path, pts_secs, created_at) VALUES (?1, ?2, 1)",
                    rusqlite::params![&old_k, pts],
                )
                .unwrap();
            }
        }

        let report = run_at(dir.path(), &old, &new);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.rows >= 6);

        let conn = open(dir.path(), "rating.db");
        let (stars, rated_at, source_path): (i64, i64, String) = conn
            .query_row(
                "SELECT stars, rated_at_ms, source_path FROM ratings WHERE path = ?1",
                [&new_k],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((stars, rated_at), (4, 111), "★と設定時刻が引き継がれる");
        assert_eq!(source_path, new_k, "source_path も新キー由来に更新される");

        let conn = open(dir.path(), "content_identity.db");
        let (file_key, full_hash): (String, String) = conn
            .query_row(
                "SELECT file_key, full_hash FROM edit_origin WHERE file_key = ?1",
                [&new_k],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((file_key, full_hash), (new_k.clone(), "full".to_string()));

        let conn = open(dir.path(), "tags.db");
        let tags: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM item_tags WHERE item_key = ?1",
                [&new_k],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tags, 2, "旧キーのタグと新キーの既存タグが両立する");
        let old_left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM item_tags WHERE item_key = ?1",
                [&old_k],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old_left, 0);

        let conn = open(dir.path(), "video_bookmarks.db");
        let bms: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM video_bookmarks WHERE path = ?1",
                [&new_k],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bms, 2, "非一意キーのブックマークは全行移る");

        // 冪等: 再実行しても何も動かない。
        let again = run_at(dir.path(), &old, &new);
        assert_eq!(again.rows, 0);
        assert!(again.errors.is_empty());
    }

    /// フォルダ改名: 配下キーが prefix 書換され、似た名前の隣接フォルダは巻き込まれない。
    /// drive 除去キーのストア (spread) もフォルダ自身の行が移る。
    #[test]
    fn migrates_folder_prefix_keys() {
        let dir = tempfile::tempdir().unwrap();
        let old = PathBuf::from(r"D:\Pics\Trip");
        let new = PathBuf::from(r"D:\Pics\Trip 2026");
        let old_k = crate::adjustment_db::normalize_path(&old);
        let new_k = crate::adjustment_db::normalize_path(&new);

        {
            let conn = open(dir.path(), "adjustment.db");
            conn.execute_batch(
                "CREATE TABLE page_params (page_path TEXT PRIMARY KEY, params_json TEXT);
                 CREATE TABLE sidecar_sync (folder_key TEXT PRIMARY KEY, sidecar_mtime INTEGER NOT NULL);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO page_params (page_path, params_json) VALUES (?1, 'p1')",
                [format!("{old_k}/a.jpg")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO page_params (page_path, params_json) VALUES (?1, 'sub')",
                [format!("{old_k}/sub/b.jpg")],
            )
            .unwrap();
            // 似た名前の隣接フォルダ (Trip2) は対象外。
            conn.execute(
                "INSERT INTO page_params (page_path, params_json) VALUES (?1, 'other')",
                [format!("{old_k}2/c.jpg")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sidecar_sync (folder_key, sidecar_mtime) VALUES (?1, 5)",
                [&old_k],
            )
            .unwrap();
        }
        {
            let conn = open(dir.path(), "spread.db");
            conn.execute_batch(
                "CREATE TABLE spreads (path TEXT PRIMARY KEY, mode INTEGER NOT NULL DEFAULT 0)",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO spreads (path, mode) VALUES (?1, 2)",
                [crate::path_key::normalize(&old)],
            )
            .unwrap();
        }

        let report = run_at(dir.path(), &old, &new);
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        let conn = open(dir.path(), "adjustment.db");
        for key in [format!("{new_k}/a.jpg"), format!("{new_k}/sub/b.jpg")] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM page_params WHERE page_path = ?1",
                    [&key],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "配下キーが新 prefix へ移る: {key}");
        }
        let other: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM page_params WHERE page_path = ?1",
                [format!("{old_k}2/c.jpg")],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(other, 1, "隣接フォルダ (Trip2) は巻き込まれない");
        let sync: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sidecar_sync WHERE folder_key = ?1",
                [&new_k],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sync, 1, "sidecar_sync のフォルダ行も移る");

        let conn = open(dir.path(), "spread.db");
        let mode: i64 = conn
            .query_row(
                "SELECT mode FROM spreads WHERE path = ?1",
                [crate::path_key::normalize(&new)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mode, 2, "drive 除去キーの見開き設定も移る");
    }

    /// ZIP コンテナ改名: `::` 合成キー (アーカイブ内ページの★等) が prefix 書換され、
    /// 新キー側の既存行が優先される。path 中の `%` / `_` も誤爆しない。
    #[test]
    fn migrates_container_entry_keys_and_tolerates_like_wildcards() {
        let dir = tempfile::tempdir().unwrap();
        let old = PathBuf::from(r"D:\Comics\100%_orig.zip");
        let new = PathBuf::from(r"D:\Comics\100%_renamed.zip");
        let old_k = crate::adjustment_db::normalize_path(&old);
        let new_k = crate::adjustment_db::normalize_path(&new);

        {
            let conn = open(dir.path(), "rating.db");
            conn.execute_batch(
                "CREATE TABLE ratings (path TEXT PRIMARY KEY, stars INTEGER NOT NULL,
                    rated_at_ms INTEGER, source_path TEXT, kind INTEGER, entry_name TEXT,
                    page_num INTEGER, dir_prefix TEXT, archive_format TEXT,
                    zipdir_is_archive INTEGER, zipdir_representative TEXT)",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO ratings (path, stars, source_path) VALUES (?1, 3, ?2)",
                rusqlite::params![format!("{old_k}::pages/p1.jpg"), &old_k],
            )
            .unwrap();
            // 新キー側に既存行 (改名後に付け直した★5) → 新優先。
            conn.execute(
                "INSERT INTO ratings (path, stars, source_path) VALUES (?1, 2, ?2)",
                rusqlite::params![format!("{old_k}::pages/p2.jpg"), &old_k],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO ratings (path, stars, source_path) VALUES (?1, 5, ?2)",
                rusqlite::params![format!("{new_k}::pages/p2.jpg"), &new_k],
            )
            .unwrap();
            // `%` をワイルドカード解釈すると巻き込まれる無関係キー。
            conn.execute(
                "INSERT INTO ratings (path, stars) VALUES ('d:/comics/100x_other.zip::p.jpg', 1)",
                [],
            )
            .unwrap();
        }

        let report = run_at(dir.path(), &old, &new);
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        let conn = open(dir.path(), "rating.db");
        let (p1, src): (i64, String) = conn
            .query_row(
                "SELECT stars, source_path FROM ratings WHERE path = ?1",
                [format!("{new_k}::pages/p1.jpg")],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(p1, 3, "アーカイブ内ページの★が移る");
        assert_eq!(src, new_k, "source_path がコンテナ新キーになる");
        let p2: i64 = conn
            .query_row(
                "SELECT stars FROM ratings WHERE path = ?1",
                [format!("{new_k}::pages/p2.jpg")],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(p2, 5, "新キー側の既存行が優先される");
        let unrelated: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ratings WHERE path = 'd:/comics/100x_other.zip::p.jpg'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(unrelated, 1, "% をワイルドカード扱いしない (substr 等値)");
    }

    /// 永続 edit preview は通常 catalog と別の page-key 行だけを移す。exact / folder prefix /
    /// archive entry の 3 面、新キー優先、再実行時の冪等性を同時に固定する。
    #[test]
    fn migrates_edit_preview_keys_without_touching_adjacent_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let old = PathBuf::from(r"D:\Comics\Old");
        let new = PathBuf::from(r"D:\Comics\New");
        let old_k = crate::adjustment_db::normalize_path(&old);
        let new_k = crate::adjustment_db::normalize_path(&new);
        {
            let conn = open(dir.path(), "edit_preview_cache.db");
            conn.execute_batch(
                "CREATE TABLE edit_previews (
                    item_key TEXT PRIMARY KEY,
                    cached_path TEXT NOT NULL
                )",
            )
            .unwrap();
            for (key, cached_path) in [
                (old_k.clone(), "exact-old"),
                (format!("{old_k}/sub/page.jpg"), "prefix-old"),
                (format!("{old_k}::pages/p1.jpg"), "archive-old"),
                (format!("{old_k}::pages/p2.jpg"), "archive-old-2"),
                (format!("{new_k}::pages/p1.jpg"), "archive-new"),
                (format!("{old_k}2/keep.jpg"), "adjacent"),
            ] {
                conn.execute(
                    "INSERT INTO edit_previews (item_key, cached_path) VALUES (?1, ?2)",
                    rusqlite::params![key, cached_path],
                )
                .unwrap();
            }
        }

        let report = run_at(dir.path(), &old, &new);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(
            report.rows, 3,
            "exact / folder prefix / archive prefix の更新行を report に加算する"
        );

        let conn = open(dir.path(), "edit_preview_cache.db");
        for (key, expected_path) in [
            (new_k.clone(), "exact-old"),
            (format!("{new_k}/sub/page.jpg"), "prefix-old"),
            (format!("{new_k}::pages/p1.jpg"), "archive-new"),
            (format!("{new_k}::pages/p2.jpg"), "archive-old-2"),
            (format!("{old_k}2/keep.jpg"), "adjacent"),
        ] {
            let cached_path: String = conn
                .query_row(
                    "SELECT cached_path FROM edit_previews WHERE item_key = ?1",
                    [&key],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(cached_path, expected_path, "{key}");
        }
        let old_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM edit_previews
                 WHERE item_key = ?1
                    OR substr(item_key, 1, ?2) = ?3
                    OR substr(item_key, 1, ?4) = ?5",
                rusqlite::params![
                    old_k,
                    format!("{old_k}/").chars().count() as i64,
                    format!("{old_k}/"),
                    format!("{old_k}::").chars().count() as i64,
                    format!("{old_k}::"),
                ],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_rows, 0, "旧 exact / prefix / archive 行を残さない");

        drop(conn);
        let again = run_at(dir.path(), &old, &new);
        assert_eq!(again.rows, 0, "ジャーナル再実行相当は no-op");
        assert!(again.errors.is_empty());
    }

    /// 閲覧履歴は exact のみ: key と raw path が新 path へ更新される。
    #[test]
    fn migrates_reading_history_exact() {
        let dir = tempfile::tempdir().unwrap();
        let old = PathBuf::from(r"D:\Comics\旧名.zip");
        let new = PathBuf::from(r"D:\Comics\新名.zip");
        {
            let conn = open(dir.path(), "reading_history.db");
            conn.execute_batch(
                "CREATE TABLE reading_history (key TEXT PRIMARY KEY, path TEXT NOT NULL,
                    kind TEXT NOT NULL, archive_format TEXT, title TEXT NOT NULL,
                    last_read_at_ms INTEGER NOT NULL, last_page INTEGER, page_count INTEGER,
                    file_size INTEGER, mtime_ms INTEGER)",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO reading_history (key, path, kind, title, last_read_at_ms, last_page)
                 VALUES (?1, ?2, 'zip', '旧名', 1, 42)",
                rusqlite::params![
                    crate::adjustment_db::normalize_path(&old),
                    old.to_string_lossy()
                ],
            )
            .unwrap();
        }

        let report = run_at(dir.path(), &old, &new);
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        let conn = open(dir.path(), "reading_history.db");
        let (path, page): (String, i64) = conn
            .query_row(
                "SELECT path, last_page FROM reading_history WHERE key = ?1",
                [crate::adjustment_db::normalize_path(&new)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(path, new.to_string_lossy(), "raw path 列も更新される");
        assert_eq!(page, 42, "続きページが引き継がれる");
    }

    /// .xmp サイドカーのファイル改名: 旧サイドカーが新名へ移り、新側に既存があれば温存。
    #[test]
    fn migrates_video_sidecar_file() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        let old = media.join("old.mp4");
        let new = media.join("new.mp4");
        // 実ファイルはリネーム済みの想定 (new のみ存在)。
        std::fs::write(&new, b"x").unwrap();
        let old_sc = crate::xmp_writer::sidecar_path_for(&old);
        std::fs::write(&old_sc, b"<xmp/>").unwrap();

        let report = run_at(dir.path(), &old, &new);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let new_sc = crate::xmp_writer::sidecar_path_for(&new);
        assert!(new_sc.exists(), "サイドカーが新名へ移る");
        assert!(!old_sc.exists());

        // 新側に既存サイドカーがある場合は上書きしない。
        let old2 = media.join("old2.mp4");
        let new2 = media.join("new2.mp4");
        std::fs::write(&new2, b"x").unwrap();
        std::fs::write(crate::xmp_writer::sidecar_path_for(&old2), b"<old/>").unwrap();
        std::fs::write(crate::xmp_writer::sidecar_path_for(&new2), b"<new/>").unwrap();
        let _ = run_at(dir.path(), &old2, &new2);
        assert_eq!(
            std::fs::read(crate::xmp_writer::sidecar_path_for(&new2)).unwrap(),
            b"<new/>",
            "新側の既存サイドカーを上書きしない"
        );
    }
}
