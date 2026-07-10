//! リネーム時の path-keyed 永続データ移行 (rename transaction)。
//!
//! アプリ内リネーム (`ui_dialogs/rename_item.rs`) の成功後に、旧 path をキーにした
//! ユーザーデータ (★ / タグ / 回転 / 補正 / マスク / 隠蔽 / ローカル調整 / テキスト注釈 /
//! 出力範囲 / 動画ピン / 動画・音楽ブックマーク / 代表サムネピン / 本 resume / 見開き /
//! 読書履歴 / PDF パスワード / 動画 .xmp サイドカー) を新 path キーへ引き継ぐ
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
//! - **読書履歴の配下 prefix**: 履歴は自己修復する (次に開いたとき新キーで upsert)
//!   ため exact のみ移行し、title も次回オープンで更新されるのに任せる。
//! - **代表サムネピンの親フォルダ側 `source_rel`**: 親ピンが改名した子を container 相対
//!   パスで指しているケース。大文字小文字を保った照合が SQL では難しく、壊れても
//!   自動サムネへのフォールバック + 1 操作で付け直せるため見送り。
//! - **サムネイルカタログ / 検索索引 / 変換アーカイブ対応表など rebuildable なキャッシュ**:
//!   再生成に任せる (フォルダ改名直後はサムネが再生成される)。
//! - **エクスプローラー等アプリ外でのリネーム**: このモジュールはアプリ内リネームの
//!   成功ハンドラからしか呼ばれない (外部リネームの検知は将来課題)。

use std::path::{Path, PathBuf};

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
/// リネーム移行は in-memory FIFO で直列実行されるため、通常終了・クラッシュ・トレイの
/// 「終了」(hidden 時は `std::process::exit` で `on_exit` を通らない) でキュー / 実行中
/// ジョブが失われると、ファイルは新名なのにメタデータが旧キーに取り残される
/// (角度⑤ Sol/Terra P1)。そこで enqueue 時にジャーナルへ追記し、**report を受信できた
/// ジョブだけ**消し込む。起動時に残エントリを再実行すれば、クラッシュで一部ストアだけ
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

/// keep-drive 正規化キー ([`crate::adjustment_db::normalize_path`]) のストア。
/// `(<db ファイル名>, <テーブル>, <キー列>, <キー列に一意制約があるか>)`。
/// 列挙は各モジュールの CREATE TABLE (スキーマ正本) と一致させること。
const KEEP_DRIVE_TARGETS: &[(&str, &str, &str, bool)] = &[
    ("rating.db", "ratings", "path", true),
    ("adjustment.db", "page_params", "page_path", true),
    ("adjustment.db", "sidecar_sync", "folder_key", true),
    ("mask.db", "masks", "path", true),
    ("conceal.db", "conceal_entries", "page_path", true),
    ("local_adjust.db", "local_adjust_pages", "page_path", true),
    ("comic.db", "comic_entries", "page_path", true),
    ("export_crop.db", "export_crop_pages", "page_path", true),
    ("tags.db", "item_tags", "item_key", true),
    ("tags.db", "tag_item_state", "item_key", true),
    ("tags.db", "tag_sidecar_sync", "folder_key", true),
    ("rotation.db", "rotations", "path", true),
    // 表示トリムのページ上書き (Sol rename-mig P2: 調査時の見落としを補完)。
    ("view_trim.db", "view_trim_pages", "page_path", true),
    ("video_pins.db", "video_pins", "path", true),
    // video_bookmarks は id が PK で path は非一意 (1 ファイル複数ブックマーク)。
    ("video_bookmarks.db", "video_bookmarks", "path", false),
    (
        "folder_thumb_pins.db",
        "folder_thumb_pins",
        "container_key",
        true,
    ),
];

/// drive 除去正規化キー ([`crate::path_key::normalize`]) のストア (USB ドライブ等で
/// ドライブレターが変わっても引き継ぐ設計の、コンテナ単位の軽量設定)。
const DRIVE_STRIPPED_TARGETS: &[(&str, &str, &str, bool)] = &[
    ("book_resume.db", "book_resume", "path", true),
    ("spread.db", "spreads", "path", true),
    // 表示トリムの本単位設定 (Sol rename-mig P2)。
    ("view_trim.db", "view_trim_books", "book_key", true),
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

    // 3. keep-drive キーのストア群 (exact + `/` prefix + `::` prefix)。
    let old_k = crate::adjustment_db::normalize_path(old_path);
    let new_k = crate::adjustment_db::normalize_path(new_path);
    if old_k != new_k {
        for (file, table, col, unique) in KEEP_DRIVE_TARGETS {
            migrate_store(
                &data_dir.join(file),
                table,
                col,
                *unique,
                &old_k,
                &new_k,
                &mut report,
            );
        }
    }

    // 4. drive 除去キーのストア群。
    let old_s = crate::path_key::normalize(old_path);
    let new_s = crate::path_key::normalize(new_path);
    if old_s != new_s {
        for (file, table, col, unique) in DRIVE_STRIPPED_TARGETS {
            migrate_store(
                &data_dir.join(file),
                table,
                col,
                *unique,
                &old_s,
                &new_s,
                &mut report,
            );
        }
    }

    // 5. 読書履歴 (exact のみ。raw path 列も更新する)。
    if old_k != new_k {
        migrate_reading_history(data_dir, new_path, &old_k, &new_k, &mut report);
    }

    report
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

/// 読書履歴の exact 移行。key (正規化) と path (raw) の両方を新 path へ更新する。
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
                "INSERT INTO ratings (path, stars, rated_at_ms, source_path) VALUES (?1, 4, 111, ?1)",
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
        assert!(report.rows >= 5);

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

    /// 読書履歴は exact のみ: key と raw path が新 path へ更新される。
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
